use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::TrackNodes;
use crate::audiograph::LiveGraphPtr;
use crate::effects::{EffectType, FilterParam};
use crate::lisp_effect::{self, LoadedDGenLib};
use crate::sequencer::{SequencerState, StepParam, NUM_PARAMS, MAX_STEPS, STEPS_PER_PAGE};

const BAR_HEIGHT: usize = 8;
const COL_WIDTH: u16 = 3;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum InputMode {
    Normal,
    ValueEntry,
    Dropdown,
    PatternSelect,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Region {
    Cirklon,
    Params,
}

impl Region {
    fn next(self) -> Region {
        match self {
            Region::Cirklon => Region::Params,
            Region::Params => Region::Cirklon,
        }
    }

    fn prev(self) -> Region {
        match self {
            Region::Cirklon => Region::Params,
            Region::Params => Region::Cirklon,
        }
    }
}

#[derive(Default, Clone)]
pub struct LayoutRects {
    pub track_list: Rect,
    pub param_tabs: Rect,
    pub bars: Rect,
    pub trigger_row: Rect,
    pub track_params_inner: Rect,
    pub effects_inner: Rect,
    pub effects_block: Rect,
}

/// Per-track node IDs needed for graph rewiring.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TrackNodeIds {
    pub sampler_id: i32,
    pub filter_id: i32,
    pub delay_id: i32,
}

pub struct App {
    pub state: Arc<SequencerState>,
    pub tracks: Vec<String>,
    pub cursor_step: usize,
    pub cursor_track: usize,
    pub active_param: StepParam,
    pub input_mode: InputMode,
    pub value_buffer: String,
    pub selection_anchor: Option<usize>,
    pub should_quit: bool,

    // Region/focus system
    pub focused_region: Region,
    pub params_column: usize, // 0 = track params (left), 1 = effects (right)
    pub track_param_cursor: usize, // 0=gate, 1=attack, 2=release, 3=swing, 4=steps
    pub effect_cursor: EffectType,
    pub effect_param_cursor: usize, // param index within focused effect
    pub dropdown_open: bool,
    pub dropdown_cursor: usize,
    pub layout: LayoutRects,
    last_step_click: Option<(usize, Instant)>, // (step, time) for double-click detection
    pub pattern_clone_pending: bool,

    // DGenLisp integration
    pub lg: LiveGraphPtr,
    pub track_node_ids: Vec<TrackNodeIds>,
    pub sample_rate: u32,
    pub pending_lisp_edit: bool,
    pub lisp_sources: Vec<String>,                          // per-track last edited source
    pub lisp_effects: Vec<Option<i32>>,                     // per-track custom effect node ID
    pub lisp_params: Vec<Option<Vec<lisp_effect::DGenParam>>>, // per-track manifest params
    pub lisp_tab_active: bool,                              // true = Lisp tab selected
    pub lisp_param_cursor: usize,                           // cursor within lisp params
    pub lisp_param_values: Vec<Vec<f32>>,                   // per-track current param values
    lisp_libs: Vec<LoadedDGenLib>,                          // keep loaded dylibs alive
}

impl App {
    pub fn new(
        state: Arc<SequencerState>,
        tracks: &[TrackNodes],
        lg: LiveGraphPtr,
        sample_rate: u32,
    ) -> Self {
        let num_tracks = tracks.len();
        let track_node_ids: Vec<TrackNodeIds> = tracks
            .iter()
            .map(|t| TrackNodeIds {
                sampler_id: t.sampler_lid as i32,
                filter_id: t.filter_lid as i32,
                delay_id: t.delay_lid as i32,
            })
            .collect();

        Self {
            state,
            tracks: tracks.iter().map(|t| t.name.clone()).collect(),
            cursor_step: 0,
            cursor_track: 0,
            active_param: StepParam::Velocity,
            input_mode: InputMode::Normal,
            value_buffer: String::new(),
            selection_anchor: None,
            should_quit: false,
            focused_region: Region::Cirklon,
            params_column: 0,
            track_param_cursor: 0,
            effect_cursor: EffectType::Filter,
            effect_param_cursor: 0,
            dropdown_open: false,
            dropdown_cursor: 0,
            layout: LayoutRects::default(),
            last_step_click: None,
            pattern_clone_pending: false,
            lg,
            track_node_ids,
            sample_rate,
            pending_lisp_edit: false,
            lisp_sources: vec![String::new(); num_tracks],
            lisp_effects: vec![None; num_tracks],
            lisp_params: vec![None; num_tracks],
            lisp_tab_active: false,
            lisp_param_cursor: 0,
            lisp_param_values: vec![Vec::new(); num_tracks],
            lisp_libs: Vec::new(),
        }
    }

    fn selected_range(&self) -> (usize, usize) {
        match self.selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(self.cursor_step);
                let hi = anchor.max(self.cursor_step);
                (lo, hi)
            }
            None => (self.cursor_step, self.cursor_step),
        }
    }

    fn has_selection(&self) -> bool {
        self.selection_anchor.is_some()
    }

    fn num_steps(&self) -> usize {
        if self.tracks.is_empty() {
            STEPS_PER_PAGE
        } else {
            self.state.track_params[self.cursor_track].get_num_steps()
        }
    }

    fn current_page(&self) -> usize {
        self.cursor_step / STEPS_PER_PAGE
    }

    fn page_range(&self) -> (usize, usize) {
        let page_start = self.current_page() * STEPS_PER_PAGE;
        let page_end = (page_start + STEPS_PER_PAGE).min(self.num_steps());
        (page_start, page_end)
    }

    /// Clamp cursor_step to the current track's num_steps.
    fn clamp_cursor_to_steps(&mut self) {
        let ns = self.num_steps();
        if self.cursor_step >= ns {
            self.cursor_step = ns - 1;
        }
    }

    pub fn handle_input(&mut self) -> std::io::Result<()> {
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        return Ok(());
                    }
                    match self.input_mode {
                        InputMode::Normal => self.handle_normal(key.code, key.modifiers),
                        InputMode::ValueEntry => self.handle_value_entry(key.code),
                        InputMode::Dropdown => self.handle_dropdown(key.code),
                        InputMode::PatternSelect => self.handle_pattern_select(key.code),
                    }
                }
                Event::Mouse(mouse) => {
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        self.handle_mouse_click(mouse.column, mouse.row);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_mouse_click(&mut self, col: u16, row: u16) {
        if self.input_mode != InputMode::Normal {
            return;
        }

        let l = &self.layout;

        // Track list: click selects track
        if rect_contains(l.track_list, col, row) {
            let idx = (row - l.track_list.y) as usize;
            if idx < self.tracks.len() {
                self.cursor_track = idx;
                self.clamp_cursor_to_steps();
                self.focused_region = Region::Cirklon;
            }
            return;
        }

        // Param tabs row: click selects active param
        if rect_contains(l.param_tabs, col, row) {
            let x_off = col.saturating_sub(l.param_tabs.x + 2);
            let tab_idx = (x_off / 6) as usize;
            if tab_idx < StepParam::ALL.len() {
                self.active_param = StepParam::ALL[tab_idx];
                self.focused_region = Region::Cirklon;
            }
            return;
        }

        // Bars area: click selects step, double-click toggles
        if rect_contains(l.bars, col, row) {
            if let Some(step) = self.step_from_click_x(col, l.bars.x) {
                self.handle_step_click(step);
            }
            return;
        }

        // Trigger row: click selects step, double-click toggles
        if rect_contains(l.trigger_row, col, row) {
            if let Some(step) = self.step_from_click_x(col, l.trigger_row.x) {
                self.handle_step_click(step);
            }
            return;
        }

        // Track params inner: click selects param row
        if rect_contains(l.track_params_inner, col, row) {
            let row_idx = (row - l.track_params_inner.y) as usize;
            if row_idx <= 4 {
                self.focused_region = Region::Params;
                self.params_column = 0;
                self.track_param_cursor = row_idx;
            }
            return;
        }

        // Effects block title row: click on effect tab
        if row == l.effects_block.y && col >= l.effects_block.x && col < l.effects_block.x + l.effects_block.width {
            let (et, is_lisp) = self.effect_tab_from_click_x(col);
            if let Some(et) = et {
                self.effect_cursor = et;
                self.effect_param_cursor = 0;
                self.lisp_tab_active = false;
                self.focused_region = Region::Params;
                self.params_column = 1;
            } else if is_lisp {
                self.lisp_tab_active = true;
                self.lisp_param_cursor = 0;
                self.focused_region = Region::Params;
                self.params_column = 1;
            }
            return;
        }

        // Effects inner: click selects effect param row
        if rect_contains(l.effects_inner, col, row) {
            let row_idx = (row - l.effects_inner.y) as usize;
            if self.lisp_tab_active {
                let max = self.lisp_params[self.cursor_track]
                    .as_ref()
                    .map(|p| p.len())
                    .unwrap_or(0);
                if row_idx < max {
                    self.focused_region = Region::Params;
                    self.params_column = 1;
                    self.lisp_param_cursor = row_idx;
                }
            } else if row_idx < self.effect_cursor.num_params() {
                self.focused_region = Region::Params;
                self.params_column = 1;
                self.effect_param_cursor = row_idx;
            }
            return;
        }
    }

    fn handle_step_click(&mut self, step: usize) {
        let now = Instant::now();
        let is_double = self
            .last_step_click
            .map(|(prev_step, prev_time)| prev_step == step && now.duration_since(prev_time).as_millis() < 400)
            .unwrap_or(false);

        self.cursor_step = step;
        self.focused_region = Region::Cirklon;

        if is_double && !self.tracks.is_empty() {
            self.state.toggle_step_and_clear_plocks(self.cursor_track, step);
            self.last_step_click = None;
        } else {
            self.last_step_click = Some((step, now));
        }
    }

    fn step_from_click_x(&self, col: u16, area_x: u16) -> Option<usize> {
        let x_offset = 2u16;
        if col < area_x + x_offset {
            return None;
        }
        let rel = col - area_x - x_offset;
        let step_in_page = (rel / COL_WIDTH) as usize;
        let (page_start, page_end) = self.page_range();
        let step = page_start + step_in_page;
        if step < page_end && step < self.num_steps() {
            Some(step)
        } else {
            None
        }
    }

    /// Returns (Some(effect_type), false) for Filter/Delay tabs,
    /// (None, true) for Lisp tab, (None, false) for no match.
    fn effect_tab_from_click_x(&self, col: u16) -> (Option<EffectType>, bool) {
        // Title starts at effects_block.x + 1 (after border)
        let mut x = self.layout.effects_block.x + 1;
        for et in EffectType::ALL {
            let label_len = et.label().len() as u16;
            let tab_width = label_len + 6; // "[< " + label + " >]"
            if col >= x && col < x + tab_width {
                return (Some(et), false);
            }
            x += tab_width + 2; // 2-char separator
        }
        // Check Lisp tab
        let lisp_width = 4u16 + 6; // "Lisp" + brackets
        if col >= x && col < x + lisp_width {
            return (None, true);
        }
        (None, false)
    }

    /// Called from main loop after terminal is suspended.
    pub fn run_lisp_editor_flow(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let track = self.cursor_track;
        let ids = &self.track_node_ids[track];
        let sampler_id = ids.sampler_id;
        let filter_id = ids.filter_id;
        let existing = self.lisp_effects[track];
        let last_source = self.lisp_sources[track].clone();
        let track_name = self.tracks[track].clone();

        let result = lisp_effect::run_editor_flow(
            self.lg.0,
            track,
            &track_name,
            sampler_id,
            filter_id,
            existing,
            &last_source,
            self.sample_rate,
        );

        if let Some(r) = result {
            self.lisp_effects[track] = Some(r.node_id);
            self.lisp_sources[track] = r.source;
            self.lisp_param_values[track] = r.params.iter().map(|p| p.default).collect();

            // Write metadata to shared SequencerState for audio thread
            self.state.lisp_node_ids[track].store(r.node_id as u32, Ordering::Relaxed);
            self.state.lisp_param_count[track].store(r.params.len() as u32, Ordering::Relaxed);
            for (i, p) in r.params.iter().enumerate() {
                self.state.set_lisp_cell_id(track, i, p.cell_id as u32);
                self.state.lisp_defaults[track].set(i, p.default);
            }

            self.lisp_params[track] = Some(r.params);
            self.lisp_tab_active = true;
            self.lisp_param_cursor = 0;
            self.lisp_libs.push(r.lib);
        }
    }

    fn handle_normal(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Global keys first
        match code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char(' ') => {
                let was_playing = self.state.is_playing();
                self.state.toggle_play();
                if was_playing {
                    self.state.playhead.store(0, Ordering::Relaxed);
                }
                return;
            }
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.tracks.is_empty() {
                    self.pending_lisp_edit = true;
                }
                return;
            }
            KeyCode::Tab => {
                self.focused_region = self.focused_region.next();
                return;
            }
            KeyCode::BackTab => {
                self.focused_region = self.focused_region.prev();
                return;
            }
            KeyCode::Esc => {
                if self.has_selection() {
                    self.selection_anchor = None;
                }
                return;
            }
            _ => {}
        }

        // Region-specific dispatch
        match self.focused_region {
            Region::Cirklon => self.handle_cirklon_input(code, modifiers),
            Region::Params => self.handle_params_input(code, modifiers),
        }
    }

    fn handle_cirklon_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let has_shift = modifiers.contains(KeyModifiers::SHIFT);
        let has_alt = modifiers.contains(KeyModifiers::ALT);
        let ns = self.num_steps();

        match code {
            // Option+Left/Right: beat jump (4 steps)
            KeyCode::Left if has_alt => {
                self.cursor_step = self.cursor_step.saturating_sub(4);
            }
            KeyCode::Right if has_alt => {
                self.cursor_step = (self.cursor_step + 4).min(ns - 1);
            }

            // Shift+Left/Right: extend selection
            KeyCode::Left if has_shift => {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_step);
                }
                if self.cursor_step > 0 {
                    self.cursor_step -= 1;
                }
            }
            KeyCode::Right if has_shift => {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_step);
                }
                if self.cursor_step < ns - 1 {
                    self.cursor_step += 1;
                }
            }

            KeyCode::Left => {
                if self.has_selection() {
                    self.shift_selection(-1);
                } else if self.cursor_step > 0 {
                    self.cursor_step -= 1;
                } else {
                    self.cursor_step = ns - 1;
                }
            }
            KeyCode::Right => {
                if self.has_selection() {
                    self.shift_selection(1);
                } else {
                    self.cursor_step = (self.cursor_step + 1) % ns;
                }
            }

            // Up/Down: switch tracks
            KeyCode::Up => {
                if self.cursor_track > 0 {
                    self.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.cursor_track = self.tracks.len() - 1;
                }
                self.clamp_cursor_to_steps();
            }
            KeyCode::Down => {
                if !self.tracks.is_empty() {
                    self.cursor_track = (self.cursor_track + 1) % self.tracks.len();
                }
                self.clamp_cursor_to_steps();
            }

            KeyCode::Enter => {
                if !self.tracks.is_empty() {
                    let (lo, hi) = self.selected_range();
                    for step in lo..=hi {
                        self.state.toggle_step_and_clear_plocks(self.cursor_track, step);
                    }
                }
            }

            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_selected(self.active_param.increment());
            }
            KeyCode::Char('-') => {
                self.adjust_selected(-self.active_param.increment());
            }
            KeyCode::Char('.') => {
                self.value_buffer.clear();
                self.value_buffer.push_str("0.");
                self.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.value_buffer.clear();
                self.value_buffer.push(c);
                self.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('p') => {
                self.input_mode = InputMode::PatternSelect;
                self.value_buffer.clear();
                self.pattern_clone_pending = false;
            }
            KeyCode::Char(c) => {
                if let Some(param) = StepParam::from_hotkey(c) {
                    self.active_param = param;
                }
            }
            _ => {}
        }
    }

    fn handle_pattern_select(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if !self.pattern_clone_pending {
                    self.value_buffer.push(c);
                }
            }
            KeyCode::Char('c') => {
                if self.value_buffer.is_empty() && !self.pattern_clone_pending {
                    self.pattern_clone_pending = true;
                    self.value_buffer = "clone".to_string();
                }
            }
            KeyCode::Char('x') => {
                let num_tracks = self.tracks.len();
                self.state.delete_pattern(num_tracks);
                self.clamp_cursor_to_steps();
                self.value_buffer.clear();
                self.pattern_clone_pending = false;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if self.pattern_clone_pending {
                    let num_tracks = self.tracks.len();
                    self.state.clone_pattern(num_tracks);
                } else if let Ok(n) = self.value_buffer.parse::<usize>() {
                    if n >= 1 {
                        let num_tracks = self.tracks.len();
                        let num_patterns = self.state.num_patterns.load(Ordering::Relaxed) as usize;
                        let idx = n - 1;
                        if idx < num_patterns {
                            self.state.switch_pattern(idx, num_tracks);
                            self.clamp_cursor_to_steps();
                        }
                    }
                }
                self.value_buffer.clear();
                self.pattern_clone_pending = false;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Backspace => {
                if self.pattern_clone_pending {
                    self.pattern_clone_pending = false;
                    self.value_buffer.clear();
                } else {
                    self.value_buffer.pop();
                }
            }
            KeyCode::Esc => {
                self.value_buffer.clear();
                self.pattern_clone_pending = false;
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_params_input(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.tracks.is_empty() {
            return;
        }

        match self.params_column {
            0 => self.handle_track_params_column(code),
            1 => self.handle_effects_column(code),
            _ => {}
        }
    }

    fn handle_track_params_column(&mut self, code: KeyCode) {
        let tp = &self.state.track_params[self.cursor_track];

        match code {
            KeyCode::Up => {
                if self.track_param_cursor > 0 {
                    self.track_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if self.track_param_cursor < 4 {
                    self.track_param_cursor += 1;
                }
            }
            KeyCode::Right => {
                self.params_column = 1;
            }
            KeyCode::Left => {} // Already at leftmost column
            KeyCode::Enter => {
                if self.track_param_cursor == 0 {
                    tp.toggle_gate();
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                match self.track_param_cursor {
                    1 => tp.set_attack_ms(tp.get_attack_ms() + 5.0),
                    2 => tp.set_release_ms(tp.get_release_ms() + 10.0),
                    3 => tp.set_swing(tp.get_swing() + 1.0),
                    4 => {
                        tp.set_num_steps(tp.get_num_steps() + 1);
                        self.clamp_cursor_to_steps();
                    }
                    _ => {}
                }
            }
            KeyCode::Char('-') => {
                match self.track_param_cursor {
                    1 => tp.set_attack_ms(tp.get_attack_ms() - 5.0),
                    2 => tp.set_release_ms(tp.get_release_ms() - 10.0),
                    3 => tp.set_swing(tp.get_swing() - 1.0),
                    4 => {
                        tp.set_num_steps(tp.get_num_steps().saturating_sub(1).max(1));
                        self.clamp_cursor_to_steps();
                    }
                    _ => {}
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if self.track_param_cursor > 0 {
                    self.value_buffer.clear();
                    self.value_buffer.push(c);
                    self.input_mode = InputMode::ValueEntry;
                }
            }
            _ => {}
        }
    }

    fn handle_effects_column(&mut self, code: KeyCode) {
        if self.lisp_tab_active {
            self.handle_lisp_tab(code);
            return;
        }
        match code {
            KeyCode::Left => {
                let idx = EffectType::ALL.iter().position(|&e| e == self.effect_cursor).unwrap_or(0);
                if idx > 0 {
                    self.effect_cursor = EffectType::ALL[idx - 1];
                    self.effect_param_cursor = 0;
                } else {
                    self.params_column = 0;
                }
            }
            KeyCode::Right => {
                let idx = EffectType::ALL.iter().position(|&e| e == self.effect_cursor).unwrap_or(0);
                if idx + 1 < EffectType::ALL.len() {
                    self.effect_cursor = EffectType::ALL[idx + 1];
                    self.effect_param_cursor = 0;
                } else {
                    // Move to Lisp tab
                    self.lisp_tab_active = true;
                    self.lisp_param_cursor = 0;
                }
            }
            KeyCode::Up => {
                if self.effect_param_cursor > 0 {
                    self.effect_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.effect_cursor.num_params().saturating_sub(1);
                if self.effect_param_cursor < max {
                    self.effect_param_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;
                if crate::effects::effect_param_is_boolean(global_idx) {
                    self.toggle_effect_boolean(global_idx);
                } else if self.should_open_dropdown(global_idx) {
                    self.dropdown_open = true;
                    self.dropdown_cursor = 0;
                    self.input_mode = InputMode::Dropdown;
                    if global_idx == FilterParam::Mode.global_index() {
                        let val = self.get_current_effect_value(global_idx);
                        self.dropdown_cursor = val.round() as usize;
                    }
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;
                let inc = crate::effects::effect_param_increment(global_idx);
                self.adjust_effect_param(global_idx, inc);
            }
            KeyCode::Char('-') => {
                let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;
                let inc = crate::effects::effect_param_increment(global_idx);
                self.adjust_effect_param(global_idx, -inc);
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;
                if !crate::effects::effect_param_is_boolean(global_idx) {
                    self.value_buffer.clear();
                    self.value_buffer.push(c);
                    self.input_mode = InputMode::ValueEntry;
                }
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step == 0 { ns - 1 } else { self.cursor_step - 1 };
                self.selection_anchor = Some(self.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step + 1 >= ns { 0 } else { self.cursor_step + 1 };
                self.selection_anchor = Some(self.cursor_step);
            }
            _ => {}
        }
    }

    fn handle_lisp_tab(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left => {
                self.lisp_tab_active = false;
                self.effect_cursor = *EffectType::ALL.last().unwrap();
                self.effect_param_cursor = 0;
            }
            KeyCode::Right => {} // already rightmost tab
            KeyCode::Up => {
                if self.lisp_param_cursor > 0 {
                    self.lisp_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.lisp_params[self.cursor_track]
                    .as_ref()
                    .map(|p| p.len().saturating_sub(1))
                    .unwrap_or(0);
                if self.lisp_param_cursor < max {
                    self.lisp_param_cursor += 1;
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_lisp_param(1.0);
            }
            KeyCode::Char('-') => {
                self.adjust_lisp_param(-1.0);
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                if self.lisp_params[self.cursor_track]
                    .as_ref()
                    .map(|p| !p.is_empty())
                    .unwrap_or(false)
                {
                    self.value_buffer.clear();
                    self.value_buffer.push(c);
                    self.input_mode = InputMode::ValueEntry;
                }
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step == 0 { ns - 1 } else { self.cursor_step - 1 };
                self.selection_anchor = Some(self.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step + 1 >= ns { 0 } else { self.cursor_step + 1 };
                self.selection_anchor = Some(self.cursor_step);
            }
            _ => {}
        }
    }

    fn adjust_lisp_param(&mut self, direction: f32) {
        let track = self.cursor_track;
        let idx = self.lisp_param_cursor;
        if let Some(params) = &self.lisp_params[track] {
            if idx < params.len() {
                let param = &params[idx];
                let inc = (param.max - param.min) * 0.01;
                if self.has_selection() {
                    let (lo, hi) = self.selected_range();
                    for step in lo..=hi {
                        let current = self.state.lisp_plocks[track]
                            .get(step, idx)
                            .unwrap_or_else(|| self.state.lisp_defaults[track].get(idx));
                        let new_val = (current + direction * inc).clamp(param.min, param.max);
                        self.state.lisp_plocks[track].set(step, idx, new_val);
                    }
                } else {
                    let old = self.state.lisp_defaults[track].get(idx);
                    let new_val = (old + direction * inc).clamp(param.min, param.max);
                    self.state.lisp_defaults[track].set(idx, new_val);
                    self.send_lisp_param(track, param.cell_id, new_val);
                }
            }
        }
    }

    fn send_lisp_param(&self, track: usize, cell_id: usize, value: f32) {
        if let Some(node_id) = self.lisp_effects[track] {
            let idx = (lisp_effect::HEADER_SLOTS + cell_id) as u64;
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.lg.0,
                    crate::audiograph::ParamMsg {
                        idx,
                        logical_id: node_id as u64,
                        fvalue: value,
                    },
                );
            }
        }
    }

    fn handle_dropdown(&mut self, code: KeyCode) {
        let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;

        match code {
            KeyCode::Up => {
                if self.dropdown_cursor > 0 {
                    self.dropdown_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.dropdown_max_items(global_idx);
                if self.dropdown_cursor < max - 1 {
                    self.dropdown_cursor += 1;
                }
            }
            KeyCode::Enter => {
                self.apply_dropdown_selection(global_idx);
                self.dropdown_open = false;
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.dropdown_open = false;
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn should_open_dropdown(&self, global_idx: usize) -> bool {
        global_idx == FilterParam::Mode.global_index()
    }

    fn dropdown_max_items(&self, global_idx: usize) -> usize {
        if global_idx == FilterParam::Mode.global_index() {
            3 // LP, HP, BP
        } else {
            0
        }
    }

    fn apply_dropdown_selection(&self, global_idx: usize) {
        let val = self.dropdown_cursor as f32;
        if self.has_selection() {
            let (lo, hi) = self.selected_range();
            let plocks = &self.state.effect_plocks[self.cursor_track];
            for step in lo..=hi {
                plocks.set(step, global_idx, val);
            }
        } else {
            self.state.effect_defaults[self.cursor_track].set(global_idx, val);
        }
    }

    fn toggle_effect_boolean(&self, global_idx: usize) {
        if self.has_selection() {
            let (lo, hi) = self.selected_range();
            let plocks = &self.state.effect_plocks[self.cursor_track];
            let defaults = &self.state.effect_defaults[self.cursor_track];
            for step in lo..=hi {
                let current = plocks.get(step, global_idx).unwrap_or_else(|| defaults.get(global_idx));
                let new_val = if current > 0.5 { 0.0 } else { 1.0 };
                plocks.set(step, global_idx, new_val);
            }
        } else {
            let defaults = &self.state.effect_defaults[self.cursor_track];
            let current = defaults.get(global_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            defaults.set(global_idx, new_val);
        }
    }

    fn adjust_effect_param(&self, global_idx: usize, delta: f32) {
        if self.has_selection() {
            let (lo, hi) = self.selected_range();
            let plocks = &self.state.effect_plocks[self.cursor_track];
            let defaults = &self.state.effect_defaults[self.cursor_track];
            for step in lo..=hi {
                let current = plocks.get(step, global_idx).unwrap_or_else(|| defaults.get(global_idx));
                let min = crate::effects::effect_param_min(global_idx);
                let max = crate::effects::effect_param_max(global_idx);
                let new_val = (current + delta).clamp(min, max);
                plocks.set(step, global_idx, new_val);
            }
        } else {
            let defaults = &self.state.effect_defaults[self.cursor_track];
            let current = defaults.get(global_idx);
            defaults.set(global_idx, current + delta);
        }
    }

    fn get_current_effect_value(&self, global_idx: usize) -> f32 {
        self.state.effect_defaults[self.cursor_track].get(global_idx)
    }

    fn handle_value_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.value_buffer.push(c);
            }
            KeyCode::Char('.') => {
                if !self.value_buffer.contains('.') {
                    self.value_buffer.push('.');
                }
            }
            KeyCode::Char('-') => {
                if self.value_buffer.starts_with('-') {
                    self.value_buffer.remove(0);
                } else {
                    self.value_buffer.insert(0, '-');
                }
            }
            KeyCode::Backspace => {
                self.value_buffer.pop();
                if self.value_buffer.is_empty() {
                    self.input_mode = InputMode::Normal;
                }
            }
            KeyCode::Enter => {
                if let Ok(val) = self.value_buffer.parse::<f32>() {
                    self.apply_value_entry(val);
                }
                self.value_buffer.clear();
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.value_buffer.clear();
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn apply_value_entry(&mut self, val: f32) {
        if self.tracks.is_empty() {
            return;
        }

        match self.focused_region {
            Region::Cirklon => {
                let (lo, hi) = self.selected_range();
                let sd = &self.state.step_data[self.cursor_track];
                for step in lo..=hi {
                    sd.set(step, self.active_param, val);
                }
            }
            Region::Params => {
                if self.params_column == 0 {
                    let tp = &self.state.track_params[self.cursor_track];
                    match self.track_param_cursor {
                        1 => tp.set_attack_ms(val),
                        2 => tp.set_release_ms(val),
                        3 => tp.set_swing(val),
                        4 => {
                            tp.set_num_steps(val as usize);
                            self.clamp_cursor_to_steps();
                        }
                        _ => {}
                    }
                } else if self.lisp_tab_active {
                    // Lisp effect params — p-lock aware
                    let track = self.cursor_track;
                    let idx = self.lisp_param_cursor;
                    if let Some(params) = &self.lisp_params[track] {
                        if idx < params.len() {
                            let clamped = val.clamp(params[idx].min, params[idx].max);
                            if self.has_selection() {
                                let (lo, hi) = self.selected_range();
                                for step in lo..=hi {
                                    self.state.lisp_plocks[track].set(step, idx, clamped);
                                }
                            } else {
                                self.state.lisp_defaults[track].set(idx, clamped);
                                self.send_lisp_param(track, params[idx].cell_id, clamped);
                            }
                        }
                    }
                } else {
                    let global_idx = self.effect_cursor.param_offset() + self.effect_param_cursor;
                    let store_val = if crate::effects::effect_param_is_percent(global_idx) {
                        val / 100.0
                    } else {
                        val
                    };
                    if self.has_selection() {
                        let (lo, hi) = self.selected_range();
                        let plocks = &self.state.effect_plocks[self.cursor_track];
                        for step in lo..=hi {
                            plocks.set(step, global_idx, store_val);
                        }
                    } else {
                        self.state.effect_defaults[self.cursor_track].set(global_idx, store_val);
                    }
                }
            }
        }
    }

    fn adjust_selected(&self, delta: f32) {
        if self.tracks.is_empty() {
            return;
        }
        let (lo, hi) = self.selected_range();
        let sd = &self.state.step_data[self.cursor_track];
        for step in lo..=hi {
            let cur = sd.get(step, self.active_param);
            sd.set(step, self.active_param, cur + delta);
        }
    }

    fn shift_selection(&mut self, direction: isize) {
        if self.tracks.is_empty() || !self.has_selection() {
            return;
        }
        let (lo, hi) = self.selected_range();
        let sd = &self.state.step_data[self.cursor_track];
        let patterns = &self.state.patterns[self.cursor_track];

        let count = hi - lo + 1;
        let shift = direction;
        let ns = self.num_steps();
        let new_lo = (lo as isize + shift).clamp(0, (ns - count) as isize) as usize;
        let new_hi = new_lo + count - 1;

        if new_lo == lo {
            return;
        }

        let mut all_vals: Vec<[f32; NUM_PARAMS]> = Vec::new();
        let mut all_actives: Vec<bool> = Vec::new();
        for s in lo..=hi {
            let mut pvals = [0.0f32; NUM_PARAMS];
            for p in StepParam::ALL {
                pvals[p.index()] = sd.get(s, p);
            }
            all_vals.push(pvals);
            all_actives.push(patterns.is_active(s));
        }

        for s in lo..=hi {
            if s < new_lo || s > new_hi {
                for p in StepParam::ALL {
                    sd.set(s, p, p.default_value());
                }
                if patterns.is_active(s) {
                    patterns.toggle_step(s);
                }
            }
        }

        for (i, s) in (new_lo..=new_hi).enumerate() {
            for p in StepParam::ALL {
                sd.set(s, p, all_vals[i][p.index()]);
            }
            if patterns.is_active(s) != all_actives[i] {
                patterns.toggle_step(s);
            }
        }

        self.cursor_step = (self.cursor_step as isize + shift).clamp(0, (ns - 1) as isize) as usize;
        if let Some(ref mut anchor) = self.selection_anchor {
            *anchor = (*anchor as isize + shift).clamp(0, (ns - 1) as isize) as usize;
        }
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

// ── Drawing ──

fn param_color(param: StepParam) -> Color {
    match param {
        StepParam::Duration  => Color::Cyan,
        StepParam::Velocity  => Color::Red,
        StepParam::Speed     => Color::Green,
        StepParam::AuxA      => Color::Magenta,
        StepParam::AuxB      => Color::Yellow,
        StepParam::Transpose => Color::Blue,
        StepParam::Chop      => Color::Rgb(255, 140, 0),
    }
}

fn is_in_selection(app: &App, step: usize) -> bool {
    let (lo, hi) = app.selected_range();
    step >= lo && step <= hi
}

fn region_border_style(app: &App, region: Region) -> Style {
    if app.focused_region == region {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // 2 regions + help bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(14),     // Cirklon region
            Constraint::Length(8),   // Params region
            Constraint::Length(2),   // Help bar
        ])
        .split(area);

    draw_cirklon_region(frame, app, chunks[0]);
    draw_params_region(frame, app, chunks[1]);
    draw_help_bar(frame, app, chunks[2]);
    draw_stereo_meter(frame, app, area);
}

// ── Cirklon Region ──

fn draw_cirklon_region(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .title(" Cirklon ")
        .borders(Borders::ALL)
        .border_style(region_border_style(app, Region::Cirklon));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 {
        return;
    }

    // Horizontal split: track list | sequencer content
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(6), // Track list column
            Constraint::Min(0),   // Sequencer content
        ])
        .split(inner);

    app.layout.track_list = h_chunks[0];

    draw_track_list(frame, app, h_chunks[0]);

    // Sequencer content vertical layout
    let seq_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),                      // track info (2 lines)
            Constraint::Length(1),                      // param tabs
            Constraint::Length(BAR_HEIGHT as u16),      // bars
            Constraint::Length(2),                      // trigger + step numbers
            Constraint::Length(1),                      // value line
        ])
        .split(h_chunks[1]);

    app.layout.param_tabs = seq_chunks[1];
    app.layout.bars = seq_chunks[2];
    app.layout.trigger_row = seq_chunks[3];

    draw_track_info(frame, app, seq_chunks[0]);
    draw_param_tabs(frame, app, seq_chunks[1]);
    draw_bars(frame, app, seq_chunks[2]);
    draw_trigger_row(frame, app, seq_chunks[3]);
    draw_value_line(frame, app, seq_chunks[4]);
}

fn draw_track_list(frame: &mut Frame, app: &App, area: Rect) {
    for (i, name) in app.tracks.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let is_selected = i == app.cursor_track;
        let truncated: String = name.chars().take(2).collect();
        let label = format!("{} {:<2}", i + 1, truncated);
        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let cell = Rect::new(area.x, y, area.width, 1);
        frame.render_widget(Paragraph::new(label).style(style), cell);
    }
}

fn draw_track_info(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        let msg = Paragraph::new("  No .wav files found in samples/")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let ns = app.num_steps();
    let playing = if app.state.is_playing() { "PLAYING" } else { "STOPPED" };
    let bpm = app.state.bpm.load(Ordering::Relaxed);
    let global_step = app.state.current_step();
    let display_step = (global_step % ns) + 1;

    // Line 1: pattern info
    let cur_pat = app.state.current_pattern.load(Ordering::Relaxed) as usize + 1;
    let num_pats = app.state.num_patterns.load(Ordering::Relaxed) as usize;
    let line1 = format!(
        " [pat {}/{}]  {} BPM  {}  step {}/{}",
        cur_pat, num_pats, bpm, playing, display_step, ns
    );
    let span1 = Span::styled(
        line1,
        Style::default().fg(Color::White).bg(Color::DarkGray).bold(),
    );
    if area.height >= 1 {
        frame.render_widget(
            Paragraph::new(Line::from(span1)),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    // Line 2: sample name + page info
    if area.height >= 2 {
        let sample_name = &app.tracks[app.cursor_track];
        let total_pages = (ns + STEPS_PER_PAGE - 1) / STEPS_PER_PAGE;
        let current_page = app.current_page() + 1;

        let line2 = if ns > STEPS_PER_PAGE {
            format!(" {}  [page {}/{}]", sample_name, current_page, total_pages)
        } else {
            format!(" {}", sample_name)
        };
        let span2 = Span::styled(
            line2,
            Style::default().fg(Color::Gray).bg(Color::Rgb(30, 30, 30)),
        );
        frame.render_widget(
            Paragraph::new(Line::from(span2)),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
}

fn draw_param_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw("  ")];
    for param in StepParam::ALL {
        let (prefix, hotkey, suffix) = param.tab_parts();
        let is_active = param == app.active_param;
        let color = param_color(param);

        let base_style = if is_active {
            Style::default().fg(Color::Black).bg(color).bold()
        } else {
            Style::default().fg(color)
        };
        let hotkey_style = base_style.add_modifier(Modifier::UNDERLINED);

        spans.push(Span::styled(" ", base_style));
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, base_style));
        }
        spans.push(Span::styled(hotkey, hotkey_style));
        if !suffix.is_empty() {
            spans.push(Span::styled(suffix, base_style));
        }
        spans.push(Span::styled(" ", base_style));
        spans.push(Span::raw(" "));
    }
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

fn step_bg(app: &App, step: usize, is_playing: bool, playhead: usize) -> Color {
    let is_cursor = step == app.cursor_step;
    let is_sel = app.has_selection() && is_in_selection(app, step);
    let is_ph = is_playing && step == playhead;

    if is_cursor {
        Color::Rgb(120, 120, 30)
    } else if is_sel {
        Color::Rgb(40, 50, 80)
    } else if is_ph {
        Color::Rgb(50, 50, 50)
    } else {
        Color::Reset
    }
}

fn draw_bars(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    // Branch to effect/lisp bars when effects column is focused in params region
    if app.focused_region == Region::Params && app.params_column == 1 {
        if app.lisp_tab_active {
            draw_lisp_bars(frame, app, area);
        } else {
            draw_effect_bars(frame, app, area);
        }
        return;
    }

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let sd = &app.state.step_data[app.cursor_track];
    let color = param_color(app.active_param);
    let is_transpose = app.active_param == StepParam::Transpose;

    let (page_start, page_end) = app.page_range();

    let x_offset = 2u16;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let raw = sd.get(step, app.active_param);
        let normalized = app.active_param.normalize(raw);
        let active = app.state.patterns[app.cursor_track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;

            if is_transpose {
                let center = BAR_HEIGHT / 2;
                let half_levels = if normalized >= 0.5 {
                    ((normalized - 0.5) * 2.0 * center as f32 * 2.0).round() as usize
                } else {
                    ((0.5 - normalized) * 2.0 * center as f32 * 2.0).round() as usize
                };
                let going_up = normalized >= 0.5;

                let (cell_text, fg_override) = if going_up {
                    if row < center {
                        let dist_from_center = center - row;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (" \u{2588} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else if half_levels >= threshold + 1 {
                            (" \u{2584} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else {
                            ("   ".to_string(), Color::Rgb(60, 60, 60))
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), Color::Rgb(80, 80, 80))
                    } else {
                        ("   ".to_string(), Color::Rgb(60, 60, 60))
                    }
                } else {
                    if row > center {
                        let dist_from_center = row - center;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (" \u{2588} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else if half_levels >= threshold + 1 {
                            (" \u{2580} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else {
                            ("   ".to_string(), Color::Rgb(60, 60, 60))
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), Color::Rgb(80, 80, 80))
                    } else {
                        ("   ".to_string(), Color::Rgb(60, 60, 60))
                    }
                };

                let style = Style::default().fg(fg_override).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            } else {
                let rows_from_bottom = BAR_HEIGHT - 1 - row;
                let threshold = rows_from_bottom * 2;
                let level = if fill_levels >= threshold + 2 { 2 }
                    else if fill_levels >= threshold + 1 { 1 }
                    else { 0 };

                let ch = match level {
                    2 => "\u{2588}",
                    1 => "\u{2584}",
                    _ => " ",
                };

                let cell_text = if ch == " " {
                    "   ".to_string()
                } else {
                    format!(" {} ", ch)
                };

                let fg = if active { color } else { Color::Rgb(60, 60, 60) };
                let style = Style::default().fg(fg).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            }
        }
    }
}

fn draw_lisp_bars(frame: &mut Frame, app: &App, area: Rect) {
    let track = app.cursor_track;
    let idx = app.lisp_param_cursor;
    let params = match &app.lisp_params[track] {
        Some(p) if idx < p.len() => p,
        _ => return,
    };
    let param = &params[idx];

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;
    let range = param.max - param.min;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let plock_val = app.state.lisp_plocks[track].get(step, idx);
        let value = plock_val.unwrap_or_else(|| app.state.lisp_defaults[track].get(idx));
        let normalized = if range > 0.0 {
            ((value - param.min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let active = app.state.patterns[track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;
            let rows_from_bottom = BAR_HEIGHT - 1 - row;
            let threshold = rows_from_bottom * 2;
            let level = if fill_levels >= threshold + 2 { 2 }
                else if fill_levels >= threshold + 1 { 1 }
                else { 0 };

            let ch = match level {
                2 => "\u{2588}",
                1 => "\u{2584}",
                _ => " ",
            };

            let cell_text = if ch == " " {
                "   ".to_string()
            } else {
                format!(" {} ", ch)
            };

            let fg = if !active {
                Color::Rgb(60, 60, 60)
            } else if plock_val.is_some() {
                Color::Cyan
            } else {
                Color::Green
            };
            let style = Style::default().fg(fg).bg(bg);
            let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
            frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
        }
    }
}

fn draw_effect_bars(frame: &mut Frame, app: &App, area: Rect) {
    let track = app.cursor_track;
    let global_idx = app.effect_cursor.param_offset() + app.effect_param_cursor;

    // Skip boolean params (e.g., filter enabled, delay synced)
    if crate::effects::effect_param_is_boolean(global_idx) {
        return;
    }

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;

    let param_min = crate::effects::effect_param_min(global_idx);
    let param_max = crate::effects::effect_param_max(global_idx);
    let range = param_max - param_min;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let plock_val = app.state.effect_plocks[track].get(step, global_idx);
        let value = plock_val.unwrap_or_else(|| app.state.effect_defaults[track].get(global_idx));
        let normalized = if range > 0.0 {
            ((value - param_min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let active = app.state.patterns[track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;
            let rows_from_bottom = BAR_HEIGHT - 1 - row;
            let threshold = rows_from_bottom * 2;
            let level = if fill_levels >= threshold + 2 { 2 }
                else if fill_levels >= threshold + 1 { 1 }
                else { 0 };

            let ch = match level {
                2 => "\u{2588}",
                1 => "\u{2584}",
                _ => " ",
            };

            let cell_text = if ch == " " {
                "   ".to_string()
            } else {
                format!(" {} ", ch)
            };

            let fg = if !active {
                Color::Rgb(60, 60, 60)
            } else if plock_val.is_some() {
                Color::Cyan
            } else {
                Color::Magenta
            };
            let style = Style::default().fg(fg).bg(bg);
            let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
            frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
        }
    }
}

fn draw_trigger_row(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() || area.height < 2 {
        return;
    }

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let active = app.state.patterns[app.cursor_track].is_active(step);
        let has_effect_plock = app.state.effect_plocks[app.cursor_track].step_has_any_plock(step);
        let lisp_param_count = app.state.lisp_param_count[app.cursor_track].load(Ordering::Relaxed) as usize;
        let has_lisp_plock = app.state.lisp_plocks[app.cursor_track].step_has_any_plock(step, lisp_param_count);
        let has_plock = has_effect_plock || has_lisp_plock;
        let ch = if active && has_plock {
            " * "
        } else if active {
            " o "
        } else {
            " . "
        };
        let fg = if active && has_plock {
            Color::Yellow
        } else if active {
            Color::White
        } else {
            Color::DarkGray
        };
        let bg = step_bg(app, step, is_playing, playhead);
        let style = Style::default().fg(fg).bg(bg);
        let cell = Rect::new(col_x, area.y, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(ch).style(style), cell);
    }

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let num = format!("{:>2} ", step + 1);
        let is_sel = app.has_selection() && is_in_selection(app, step);
        let style = if step == app.cursor_step {
            Style::default().fg(Color::Yellow)
        } else if is_sel {
            Style::default().fg(Color::Rgb(120, 150, 220))
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let cell = Rect::new(col_x, area.y + 1, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(num).style(style), cell);
    }
}

fn draw_value_line(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    let is_pattern_select = app.input_mode == InputMode::PatternSelect;
    let is_cirklon_entry = app.input_mode == InputMode::ValueEntry
        && app.focused_region == Region::Cirklon;

    let line = if is_pattern_select {
        if app.pattern_clone_pending {
            Line::from(vec![
                Span::styled("  Clone pattern \u{2192} new  ", Style::default().fg(Color::Cyan)),
                Span::styled("Enter: confirm  Esc: cancel", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::styled("  Pattern: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{}\u{2588}", app.value_buffer),
                    Style::default().fg(Color::Yellow).bg(Color::Rgb(60, 60, 20)).bold(),
                ),
                Span::styled(
                    "  Enter: go  c: clone  x: delete  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        }
    } else if is_cirklon_entry {
        let step_label = if app.has_selection() {
            let (lo, hi) = app.selected_range();
            format!("Steps {}-{}", lo + 1, hi + 1)
        } else {
            format!("Step {}", app.cursor_step + 1)
        };
        Line::from(vec![
            Span::styled(
                format!("  {}: {} = ", step_label, app.active_param.label()),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{}\u{2588}", app.value_buffer),
                Style::default().fg(Color::Yellow).bg(Color::Rgb(60, 60, 20)).bold(),
            ),
            Span::styled(
                "  Enter: set  Esc: cancel  -: negate",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.has_selection() {
        let (lo, hi) = app.selected_range();
        let count = hi - lo + 1;
        Line::from(Span::styled(
            format!(
                "  Steps {}-{} selected ({} steps)  {} = \u{2191}\u{2193}",
                lo + 1, hi + 1, count,
                app.active_param.label(),
            ),
            Style::default().fg(Color::Rgb(120, 150, 220)),
        ))
    } else {
        let sd = &app.state.step_data[app.cursor_track];
        let val = sd.get(app.cursor_step, app.active_param);
        Line::from(Span::styled(
            format!(
                "  Step {}: {} = {}",
                app.cursor_step + 1,
                app.active_param.label(),
                app.active_param.format_value(val),
            ),
            Style::default().fg(Color::White),
        ))
    };

    frame.render_widget(Paragraph::new(line), area);
}

// ── Params Region (Track Params + Effects side by side) ──

fn draw_params_region(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focused_region == Region::Params;

    // Horizontal split: track params | effects
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Track params column
            Constraint::Percentage(60), // Effects column
        ])
        .split(area);

    draw_track_params_column(frame, app, h_chunks[0], is_focused);
    draw_effects_column(frame, app, h_chunks[1], is_focused);
}

fn draw_track_params_column(frame: &mut Frame, app: &mut App, area: Rect, region_focused: bool) {
    let col_focused = region_focused && app.params_column == 0;
    let border_style = if col_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    let block = Block::default()
        .title(" Track ")
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.layout.track_params_inner = inner;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    let tp = &app.state.track_params[app.cursor_track];
    let attack = tp.get_attack_ms();
    let release = tp.get_release_ms();
    let swing = tp.get_swing();
    let steps = tp.get_num_steps();

    let params: Vec<(&str, String, Option<f32>)> = vec![
        ("gate", if tp.is_gate_on() { "ON".to_string() } else { "OFF".to_string() }, None),
        ("attack", format!("{:.0} ms", attack), Some(attack / 500.0)),
        ("release", format!("{:.0} ms", release), Some(release / 2000.0)),
        ("swing", format!("{:.0}%", swing), Some((swing - 50.0) / 25.0)),
        ("steps", format!("{}", steps), Some(steps as f32 / MAX_STEPS as f32)),
    ];

    let is_entering_value = col_focused
        && app.input_mode == InputMode::ValueEntry;

    for (i, (name, value, slider)) in params.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let y = inner.y + i as u16;
        let is_cursor_row = col_focused && app.track_param_cursor == i;
        let cursor = if is_cursor_row { "> " } else { "  " };
        let cursor_style = if is_cursor_row {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let label_width = 10;
        let value_width = 12;

        if is_cursor_row && is_entering_value {
            let spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(format!("{:<width$}", name, width = label_width), Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{}\u{2588}", app.value_buffer),
                    Style::default().fg(Color::Yellow).bg(Color::Rgb(60, 60, 20)).bold(),
                ),
                Span::styled(
                    "  Enter: set  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            let line = Line::from(spans);
            let row_area = Rect::new(inner.x, y, inner.width, 1);
            frame.render_widget(Paragraph::new(line), row_area);
            continue;
        }

        let mut spans = vec![
            Span::styled(cursor, cursor_style),
            Span::styled(format!("{:<width$}", name, width = label_width), Style::default().fg(Color::Gray)),
            Span::styled(format!("{:<width$}", value, width = value_width), cursor_style),
        ];

        if let Some(norm) = slider {
            let slider_width = (inner.width as usize).saturating_sub(label_width + value_width + 4);
            if slider_width > 2 {
                let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                let bar: String = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                spans.push(Span::styled(
                    format!("[{}]", bar),
                    Style::default().fg(Color::Cyan),
                ));
            }
        }

        let line = Line::from(spans);
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        frame.render_widget(Paragraph::new(line), row_area);
    }
}

fn draw_effects_column(frame: &mut Frame, app: &mut App, area: Rect, region_focused: bool) {
    let col_focused = region_focused && app.params_column == 1;
    let border_style = if col_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    // Build title with effect type tabs (Filter, Delay, Lisp)
    let mut title_spans = vec![];
    for et in EffectType::ALL {
        let is_selected = et == app.effect_cursor && !app.lisp_tab_active;
        let style = if is_selected && col_focused {
            Style::default().fg(Color::Black).bg(Color::Cyan).bold()
        } else if is_selected {
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 100, 100))
        } else {
            Style::default().fg(Color::Gray)
        };
        let label = if is_selected {
            format!("[< {} >]", et.label())
        } else {
            format!("[  {}  ]", et.label())
        };
        title_spans.push(Span::styled(label, style));
        title_spans.push(Span::raw("  "));
    }
    // Lisp tab
    {
        let is_selected = app.lisp_tab_active;
        let has_effect = !app.tracks.is_empty() && app.lisp_effects[app.cursor_track].is_some();
        let lisp_label = if has_effect { "Lisp" } else { "Lisp" };
        let style = if is_selected && col_focused {
            Style::default().fg(Color::Black).bg(Color::Green).bold()
        } else if is_selected {
            Style::default().fg(Color::Black).bg(Color::Rgb(100, 100, 100))
        } else if has_effect {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = if is_selected {
            format!("[< {} >]", lisp_label)
        } else {
            format!("[  {}  ]", lisp_label)
        };
        title_spans.push(Span::styled(label, style));
    }

    let block = Block::default()
        .title(Line::from(title_spans))
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.layout.effects_block = area;
    app.layout.effects_inner = inner;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    if app.lisp_tab_active {
        draw_lisp_params(frame, app, inner, col_focused);
        return;
    }

    // Effect params
    let defaults = &app.state.effect_defaults[app.cursor_track];
    let offset = app.effect_cursor.param_offset();
    let num_params = app.effect_cursor.num_params();
    let is_entering_value = col_focused
        && app.input_mode == InputMode::ValueEntry;

    for i in 0..num_params {
        let row_y = inner.y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let global_idx = offset + i;
        let value = defaults.get(global_idx);
        let label = crate::effects::effect_param_label(global_idx);
        let formatted = crate::effects::effect_param_format(global_idx, value);

        let is_cursor_row = col_focused && app.effect_param_cursor == i;
        let cursor = if is_cursor_row { "> " } else { "  " };
        let cursor_style = if is_cursor_row {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let label_width = 12;
        let value_width = 14;

        if is_cursor_row && is_entering_value {
            let target_label = if app.has_selection() {
                let (lo, hi) = app.selected_range();
                format!("p-lock steps {}-{}", lo + 1, hi + 1)
            } else {
                "default".to_string()
            };
            let spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(format!("{:<width$}", label, width = label_width), Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{}\u{2588}", app.value_buffer),
                    Style::default().fg(Color::Yellow).bg(Color::Rgb(60, 60, 20)).bold(),
                ),
                Span::styled(
                    format!("  ({})  Enter: set  Esc: cancel", target_label),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            let line = Line::from(spans);
            let row_area = Rect::new(inner.x, row_y, inner.width, 1);
            frame.render_widget(Paragraph::new(line), row_area);
            continue;
        }

        let mut spans = vec![
            Span::styled(cursor, cursor_style),
            Span::styled(format!("{:<width$}", label, width = label_width), Style::default().fg(Color::Gray)),
            Span::styled(format!("{:<width$}", formatted, width = value_width), cursor_style),
        ];

        // Add slider for numeric params
        if !crate::effects::effect_param_is_boolean(global_idx) {
            let min = crate::effects::effect_param_min(global_idx);
            let max = crate::effects::effect_param_max(global_idx);
            let range = max - min;
            if range > 0.0 {
                let ns = app.num_steps();
                let step = app.state.current_step() % ns;
                let bar_value = app.state.effect_plocks[app.cursor_track]
                    .get(step, global_idx)
                    .unwrap_or(value);
                let norm = ((bar_value - min) / range).clamp(0.0, 1.0);
                let slider_width = (inner.width as usize).saturating_sub(label_width + value_width + 6);
                if slider_width > 2 {
                    let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                    let bar: String = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                    spans.push(Span::styled(
                        format!("[{}]", bar),
                        Style::default().fg(Color::Magenta),
                    ));
                }
            }
        }

        let line = Line::from(spans);
        let row_area = Rect::new(inner.x, row_y, inner.width, 1);
        frame.render_widget(Paragraph::new(line), row_area);
    }

    // Dropdown overlay
    if app.dropdown_open && col_focused {
        draw_dropdown(frame, app, inner);
    }
}

fn draw_lisp_params(frame: &mut Frame, app: &App, inner: Rect, col_focused: bool) {
    let track = app.cursor_track;
    let has_effect = app.lisp_effects[track].is_some();

    if !has_effect {
        // No effect loaded
        let hint = Line::from(vec![
            Span::styled("  Ctrl+L", Style::default().fg(Color::Green).bold()),
            Span::styled(" to create effect", Style::default().fg(Color::DarkGray)),
        ]);
        let row_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(Paragraph::new(hint), row_area);
        return;
    }

    // Effect is loaded
    match &app.lisp_params[track] {
        Some(params) if !params.is_empty() => {
            let is_entering_value = col_focused && app.input_mode == InputMode::ValueEntry;

            for (i, param) in params.iter().enumerate() {
                let row_y = inner.y + i as u16;
                if row_y >= inner.y + inner.height {
                    break;
                }

                let is_cursor_row = col_focused && app.lisp_param_cursor == i;
                let cursor = if is_cursor_row { "> " } else { "  " };
                let cursor_style = if is_cursor_row {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::White)
                };

                let label_width = 12;
                let value_width = 14;

                let default_val = app.state.lisp_defaults[track].get(i);
                let unit_str = param
                    .unit
                    .as_deref()
                    .map(|u| format!(" {}", u))
                    .unwrap_or_default();

                // Value entry mode
                if is_cursor_row && is_entering_value {
                    let target_label = if app.has_selection() {
                        let (lo, hi) = app.selected_range();
                        format!("p-lock steps {}-{}", lo + 1, hi + 1)
                    } else {
                        "default".to_string()
                    };
                    let spans = vec![
                        Span::styled(cursor, cursor_style),
                        Span::styled(format!("{:<width$}", param.name, width = label_width), Style::default().fg(Color::Gray)),
                        Span::styled(
                            format!("{}\u{2588}", app.value_buffer),
                            Style::default().fg(Color::Yellow).bg(Color::Rgb(60, 60, 20)).bold(),
                        ),
                        Span::styled(
                            format!("  ({})  Enter: set  Esc: cancel", target_label),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ];
                    let line = Line::from(spans);
                    let row_area = Rect::new(inner.x, row_y, inner.width, 1);
                    frame.render_widget(Paragraph::new(line), row_area);
                    continue;
                }

                // Determine display value and p-lock status
                let (display_val, plock_label) = if app.has_selection() && is_cursor_row {
                    let plock_val = app.state.lisp_plocks[track].get(app.cursor_step, i);
                    match plock_val {
                        Some(v) => (v, Some(" (p-lock)")),
                        None => (default_val, None),
                    }
                } else {
                    (default_val, None)
                };

                let formatted = format!("{:.2}{}", display_val, unit_str);

                let mut spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(
                        format!("{:<width$}", param.name, width = label_width),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled(
                        format!("{:<width$}", formatted, width = value_width),
                        cursor_style,
                    ),
                ];

                if let Some(lbl) = plock_label {
                    spans.push(Span::styled(lbl, Style::default().fg(Color::Cyan)));
                }

                // Slider
                let range = param.max - param.min;
                if range > 0.0 {
                    let slider_val = if app.has_selection() {
                        app.state.lisp_plocks[track]
                            .get(app.cursor_step, i)
                            .unwrap_or(default_val)
                    } else {
                        default_val
                    };
                    let norm = ((slider_val - param.min) / range).clamp(0.0, 1.0);
                    let slider_width =
                        (inner.width as usize).saturating_sub(label_width + value_width + 6);
                    if slider_width > 2 {
                        let filled =
                            ((norm * slider_width as f32).round() as usize).min(slider_width);
                        let bar: String =
                            "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                        spans.push(Span::styled(
                            format!("[{}]", bar),
                            Style::default().fg(Color::Green),
                        ));
                    }
                }

                let line = Line::from(spans);
                let row_area = Rect::new(inner.x, row_y, inner.width, 1);
                frame.render_widget(Paragraph::new(line), row_area);
            }
        }
        _ => {
            // Effect loaded but no params
            let lines = vec![
                Line::from(Span::styled(
                    "  effect active",
                    Style::default().fg(Color::Green),
                )),
                Line::from(Span::styled(
                    "  (no parameters)",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(vec![
                    Span::styled("  Ctrl+L", Style::default().fg(Color::Green).bold()),
                    Span::styled(" to edit", Style::default().fg(Color::DarkGray)),
                ]),
            ];
            for (i, line) in lines.iter().enumerate() {
                let row_y = inner.y + i as u16;
                if row_y >= inner.y + inner.height {
                    break;
                }
                let row_area = Rect::new(inner.x, row_y, inner.width, 1);
                frame.render_widget(Paragraph::new(line.clone()), row_area);
            }
        }
    }
}

fn draw_dropdown(frame: &mut Frame, app: &App, area: Rect) {
    let global_idx = app.effect_cursor.param_offset() + app.effect_param_cursor;

    let items: Vec<&str> = if global_idx == FilterParam::Mode.global_index() {
        vec!["lowpass", "highpass", "bandpass"]
    } else {
        return;
    };

    let dropdown_y = area.y + app.effect_param_cursor as u16;
    let dropdown_x = area.x + 14; // after label
    let dropdown_width = 16u16;

    for (i, item) in items.iter().enumerate() {
        let y = dropdown_y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let is_cursor = i == app.dropdown_cursor;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
        };
        let text = format!(" {:<width$}", item, width = (dropdown_width - 2) as usize);
        let cell = Rect::new(dropdown_x, y, dropdown_width, 1);
        frame.render_widget(Paragraph::new(text).style(style), cell);
    }
}

// ── Stereo Meter ──

fn draw_stereo_meter(frame: &mut Frame, app: &App, area: Rect) {
    let meter_width = 20u16;
    let meter_height = 2u16;
    if area.width < meter_width + 2 || area.height < meter_height {
        return;
    }

    let x = area.x + area.width - meter_width - 1;
    let y = area.y;

    let peak_l = f32::from_bits(app.state.peak_l.load(Ordering::Relaxed));
    let peak_r = f32::from_bits(app.state.peak_r.load(Ordering::Relaxed));

    let bar_width = (meter_width - 3) as usize;

    let render_bar = |peak: f32| -> Vec<Span<'_>> {
        let norm = if peak <= 0.0 {
            0.0
        } else {
            (peak.sqrt()).min(1.2)
        };
        let filled = ((norm * bar_width as f32).round() as usize).min(bar_width);

        let mut bar_chars = String::new();
        for i in 0..bar_width {
            if i < filled {
                bar_chars.push('\u{2588}');
            } else {
                bar_chars.push('\u{2500}');
            }
        }

        let green_end = bar_width * 6 / 10;
        let yellow_end = bar_width * 85 / 100;

        let mut spans = Vec::new();
        for (i, ch) in bar_chars.chars().enumerate() {
            let color = if i >= filled {
                Color::Rgb(40, 40, 40)
            } else if i >= yellow_end {
                Color::Red
            } else if i >= green_end {
                Color::Yellow
            } else {
                Color::Green
            };
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(color),
            ));
        }
        spans
    };

    // L channel
    let mut l_spans = vec![Span::styled("L ", Style::default().fg(Color::DarkGray))];
    l_spans.extend(render_bar(peak_l));
    let l_line = Line::from(l_spans);
    frame.render_widget(
        Paragraph::new(l_line),
        Rect::new(x, y, meter_width, 1),
    );

    // R channel
    let mut r_spans = vec![Span::styled("R ", Style::default().fg(Color::DarkGray))];
    r_spans.extend(render_bar(peak_r));
    let r_line = Line::from(r_spans);
    frame.render_widget(
        Paragraph::new(r_line),
        Rect::new(x, y + 1, meter_width, 1),
    );
}

// ── Help Bar ──

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if app.input_mode == InputMode::PatternSelect {
        vec![Line::from(Span::styled(
            "  0-9: pattern number  c: clone  x: delete  Enter: confirm  Esc: cancel",
            Style::default().fg(Color::Yellow),
        ))]
    } else {
        match app.focused_region {
        Region::Cirklon => {
            if app.input_mode == InputMode::ValueEntry {
                vec![Line::from(Span::styled(
                    "  0-9: digits  .: decimal  -: negate  Enter: set  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else if app.has_selection() {
                vec![Line::from(Span::styled(
                    "  Shift+\u{2190}\u{2192}: extend  +/-: value  Enter: toggle  0-9: type  Esc: deselect",
                    Style::default().fg(Color::Rgb(120, 150, 220)),
                ))]
            } else {
                vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: step  \u{2191}\u{2193}: track  +/-: value  Shift+\u{2190}\u{2192}: select  Tab: region  p: pattern  d v s a b t c: param",
                    Style::default().fg(Color::DarkGray),
                ))]
            }
        }
        Region::Params => {
            if app.dropdown_open {
                vec![Line::from(Span::styled(
                    "  \u{2191}\u{2193}: select  Enter: confirm  Esc: cancel",
                    Style::default().fg(Color::Yellow),
                ))]
            } else if app.params_column == 1 {
                vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: column/effect  \u{2191}\u{2193}: param  +/-: adjust  [/]: step  Enter: toggle  Tab: region",
                    Style::default().fg(Color::DarkGray),
                ))]
            } else {
                vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: column/effect  \u{2191}\u{2193}: param  +/-: adjust  Enter: toggle  Tab: region",
                    Style::default().fg(Color::DarkGray),
                ))]
            }
        }
    }};

    let text = Text::from(lines);
    let help = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60))),
    );
    frame.render_widget(help, area);
}
