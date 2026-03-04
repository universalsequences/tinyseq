use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, EffectSlotState, ParamKind, SyncDivision, BUILTIN_SLOT_COUNT,
};
use crate::lisp_effect::{self, MAX_CUSTOM_FX};
use crate::reverb;

use super::params::draw_dropdown;
use super::{App, InputMode, PendingCompile, Region, REVERB_TAB};

// ── App impl: effect methods ──

impl App {
    /// Get the current slot's descriptor, if available.
    pub(super) fn current_slot_descriptor(&self) -> Option<&EffectDescriptor> {
        if self.tracks.is_empty() {
            return None;
        }
        self.effect_descriptors
            .get(self.cursor_track)
            .and_then(|descs| descs.get(self.effect_slot_cursor))
    }

    /// Get the current slot's runtime state, if available.
    pub(super) fn current_slot(&self) -> Option<&EffectSlotState> {
        if self.tracks.is_empty() {
            return None;
        }
        self.state
            .effect_chains
            .get(self.cursor_track)
            .and_then(|chain| chain.get(self.effect_slot_cursor))
    }

    /// Indices of visible (non-empty) effect slots for the current track.
    pub(super) fn visible_effect_indices(&self) -> Vec<usize> {
        if self.tracks.is_empty() {
            return Vec::new();
        }
        let track = self.cursor_track;
        let chain = &self.state.effect_chains[track];
        let descs = &self.effect_descriptors[track];
        let mut visible = Vec::new();
        for i in 0..descs.len() {
            if i < BUILTIN_SLOT_COUNT {
                visible.push(i); // Always show built-in
            } else if i < chain.len() && chain[i].node_id.load(Ordering::Relaxed) != 0 {
                visible.push(i); // Show loaded custom
            }
        }
        visible
    }

    /// Find the first free custom slot index for the current track, or None.
    fn next_free_custom_slot(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let chain = &self.state.effect_chains[self.cursor_track];
        for offset in 0..MAX_CUSTOM_FX {
            let idx = BUILTIN_SLOT_COUNT + offset;
            if idx < chain.len() && chain[idx].node_id.load(Ordering::Relaxed) == 0 {
                return Some(idx);
            }
        }
        None
    }

    /// Find the audio graph predecessor for a custom slot at `offset` (0..MAX_CUSTOM_FX).
    fn find_custom_slot_predecessor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.state.effect_chains[track];
        for i in (0..offset).rev() {
            let idx = BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let nid = chain[idx].node_id.load(Ordering::Relaxed);
                if nid != 0 {
                    return nid as i32;
                }
            }
        }
        self.track_node_ids[track].voice_sum_id
    }

    /// Find the audio graph successor for a custom slot at `offset` (0..MAX_CUSTOM_FX).
    fn find_custom_slot_successor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.state.effect_chains[track];
        for i in (offset + 1)..MAX_CUSTOM_FX {
            let idx = BUILTIN_SLOT_COUNT + i;
            if idx < chain.len() {
                let nid = chain[idx].node_id.load(Ordering::Relaxed);
                if nid != 0 {
                    return nid as i32;
                }
            }
        }
        self.track_node_ids[track].filter_id
    }

    /// Whether there are fewer than MAX_CUSTOM_FX loaded custom effects (can add more).
    fn can_add_custom_effect(&self) -> bool {
        self.next_free_custom_slot().is_some()
    }

    /// Compute wiring info for a custom slot at `slot_idx`.
    fn resolve_custom_slot_wiring(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> (usize, i32, i32, Option<i32>) {
        let offset = slot_idx - BUILTIN_SLOT_COUNT;
        let slot_id = track * MAX_CUSTOM_FX + offset;
        let predecessor_id = self.find_custom_slot_predecessor(track, offset);
        let successor_id = self.find_custom_slot_successor(track, offset);
        let existing_node = self.state.effect_chains[track]
            .get(slot_idx)
            .map(|slot| slot.node_id.load(Ordering::Relaxed))
            .unwrap_or(0);
        let existing = if existing_node != 0 {
            Some(existing_node as i32)
        } else {
            None
        };
        (slot_id, predecessor_id, successor_id, existing)
    }

    /// Apply a loaded effect's metadata to the slot state and descriptor.
    fn apply_effect_to_slot(
        &mut self,
        track: usize,
        slot_idx: usize,
        node_id: i32,
        name: &str,
        params: &[lisp_effect::DGenParam],
    ) {
        let desc = EffectDescriptor::from_lisp_manifest(name, params);
        self.effect_descriptors[track][slot_idx] = desc;

        let slot = &self.state.effect_chains[track][slot_idx];
        slot.node_id.store(node_id as u32, Ordering::Relaxed);
        slot.num_params
            .store(params.len() as u32, Ordering::Relaxed);
        for (i, p) in params.iter().enumerate() {
            slot.defaults.set(i, p.default);
            if i < slot.param_node_indices.len() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                slot.param_node_indices[i].store(node_idx, Ordering::Relaxed);
            }
        }
    }

    /// Called from main loop after terminal is suspended.
    pub fn run_lisp_editor_flow(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let track = self.cursor_track;
        let slot_idx = self.pending_lisp_slot;
        let (slot_id, predecessor_id, successor_id, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);

        // Load source: from file (if editing existing) or empty for new
        let last_source = match &self.pending_lisp_name {
            Some(name) => lisp_effect::load_effect_source(name).unwrap_or_default(),
            None => String::new(),
        };
        let existing_name = self.pending_lisp_name.clone();
        let track_name = self.tracks[track].clone();

        let result = lisp_effect::run_editor_flow(
            self.lg.0,
            slot_id,
            &track_name,
            predecessor_id,
            successor_id,
            existing,
            &last_source,
            existing_name.as_deref(),
            self.sample_rate,
        );

        if let Some(r) = result {
            self.apply_effect_to_slot(track, slot_idx, r.node_id, &r.name, &r.params);
            self.effect_slot_cursor = slot_idx;
            self.effect_param_cursor = 0;
            self.focused_region = Region::Params;
            self.params_column = 1;
            self.lisp_libs.push(r.lib);
        }

        // Clear pending state
        self.pending_lisp_name = None;
    }

    /// Spawn a background thread to compile an effect, storing a PendingCompile.
    fn start_effect_compile(&mut self, name: &str, slot_idx: usize) {
        let source = match lisp_effect::load_effect_source(name) {
            Ok(s) => s,
            Err(e) => {
                self.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.sample_rate;
        std::thread::spawn(move || {
            let result = lisp_effect::compile_and_load(&source, sample_rate);
            let _ = tx.send(result);
        });
        self.pending_compile = Some(PendingCompile {
            receiver: rx,
            name: name.to_string(),
            slot_idx,
            cursor_track: self.cursor_track,
            tick: 0,
        });
    }

    /// Apply a compiled effect result to the audio graph (must run on UI thread).
    pub(super) fn apply_compiled_effect(
        &mut self,
        result: lisp_effect::CompileResult,
        name: &str,
        slot_idx: usize,
        track: usize,
    ) {
        let (slot_id, pred, succ, existing) = self.resolve_custom_slot_wiring(track, slot_idx);

        match unsafe {
            lisp_effect::add_effect_to_chain_at(
                self.lg.0,
                slot_id,
                &result.manifest,
                &result.lib,
                pred,
                succ,
                existing,
            )
        } {
            Ok(node_id) => {
                self.apply_effect_to_slot(track, slot_idx, node_id, name, &result.manifest.params);
                self.lisp_libs.push(result.lib);
                self.effect_slot_cursor = slot_idx;
                self.effect_param_cursor = 0;
                self.focused_region = Region::Params;
                self.params_column = 1;
                self.status_message = Some((format!("Loaded '{}'", name), Instant::now()));
            }
            Err(e) => {
                self.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    fn filtered_picker_items(&self) -> Vec<String> {
        let mut items = vec!["+ New effect".to_string()];
        let filter_lower = self.picker_filter.to_lowercase();
        for name in &self.picker_items {
            if filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower) {
                items.push(name.clone());
            }
        }
        items
    }

    pub(super) fn handle_effect_picker(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.picker_filter.push(c);
                self.picker_cursor = 0;
            }
            KeyCode::Backspace => {
                self.picker_filter.pop();
                self.picker_cursor = 0;
            }
            KeyCode::Up => {
                if self.picker_cursor > 0 {
                    self.picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.filtered_picker_items().len();
                if self.picker_cursor + 1 < max {
                    self.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_picker_items();
                if self.picker_cursor < items.len() {
                    let selected = &items[self.picker_cursor];
                    if selected == "+ New effect" {
                        // Open editor for new effect
                        if let Some(slot_idx) = self.next_free_custom_slot() {
                            self.pending_lisp_slot = slot_idx;
                            self.pending_lisp_name = None;
                            self.pending_lisp_edit = true;
                        }
                    } else {
                        // Load saved effect from picker (async)
                        let name = selected.clone();
                        if let Some(slot_idx) = self.next_free_custom_slot() {
                            self.start_effect_compile(&name, slot_idx);
                        }
                    }
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn handle_effects_column(&mut self, code: KeyCode) {
        if self.effect_slot_cursor == REVERB_TAB {
            match code {
                KeyCode::Left => {
                    let visible = self.visible_effect_indices();
                    if let Some(&last) = visible.last() {
                        self.effect_slot_cursor = last;
                        self.effect_param_cursor = 0;
                    } else {
                        self.params_column = 0;
                    }
                }
                KeyCode::Right => {} // Already at rightmost tab
                KeyCode::Up => {
                    if self.reverb_param_cursor > 0 {
                        self.reverb_param_cursor -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.reverb_param_cursor < 2 {
                        self.reverb_param_cursor += 1;
                    }
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    self.adjust_reverb_param(0.05);
                }
                KeyCode::Char('-') => {
                    self.adjust_reverb_param(-0.05);
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                    self.value_buffer.clear();
                    self.value_buffer.push(c);
                    self.input_mode = InputMode::ValueEntry;
                }
                KeyCode::Char('[') => {
                    let ns = self.num_steps();
                    self.cursor_step = if self.cursor_step == 0 {
                        ns - 1
                    } else {
                        self.cursor_step - 1
                    };
                    self.selection_anchor = Some(self.cursor_step);
                }
                KeyCode::Char(']') => {
                    let ns = self.num_steps();
                    self.cursor_step = if self.cursor_step + 1 >= ns {
                        0
                    } else {
                        self.cursor_step + 1
                    };
                    self.selection_anchor = Some(self.cursor_step);
                }
                _ => {}
            }
            return;
        }

        let visible = self.visible_effect_indices();

        match code {
            KeyCode::Left => {
                if let Some(pos) = visible.iter().position(|&i| i == self.effect_slot_cursor) {
                    if pos > 0 {
                        self.effect_slot_cursor = visible[pos - 1];
                        self.effect_param_cursor = 0;
                    } else {
                        self.params_column = 0;
                    }
                } else {
                    self.params_column = 0;
                }
            }
            KeyCode::Right => {
                if let Some(pos) = visible.iter().position(|&i| i == self.effect_slot_cursor) {
                    if pos + 1 < visible.len() {
                        self.effect_slot_cursor = visible[pos + 1];
                        self.effect_param_cursor = 0;
                    } else {
                        // Move to reverb tab
                        self.effect_slot_cursor = REVERB_TAB;
                        self.reverb_param_cursor = 0;
                    }
                } else {
                    // Move to reverb tab
                    self.effect_slot_cursor = REVERB_TAB;
                    self.reverb_param_cursor = 0;
                }
            }
            KeyCode::Up => {
                if self.effect_param_cursor > 0 {
                    self.effect_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if let Some(desc) = self.current_slot_descriptor() {
                    let max = desc.params.len().saturating_sub(1);
                    if self.effect_param_cursor < max {
                        self.effect_param_cursor += 1;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.effect_param_cursor];
                        if param.is_boolean() {
                            self.toggle_slot_boolean();
                            self.update_delay_time_param_kind();
                        } else if param.is_enum() {
                            self.dropdown_open = true;
                            self.dropdown_cursor = 0;
                            self.input_mode = InputMode::Dropdown;
                            let val = self.get_current_slot_value();
                            self.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_slot_param(1.0);
            }
            KeyCode::Char('-') => {
                self.adjust_slot_param(-1.0);
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.effect_param_cursor];
                        if !param.is_boolean() {
                            self.value_buffer.clear();
                            self.value_buffer.push(c);
                            self.input_mode = InputMode::ValueEntry;
                        }
                    }
                }
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step == 0 {
                    ns - 1
                } else {
                    self.cursor_step - 1
                };
                self.selection_anchor = Some(self.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.cursor_step = if self.cursor_step + 1 >= ns {
                    0
                } else {
                    self.cursor_step + 1
                };
                self.selection_anchor = Some(self.cursor_step);
            }
            _ => {}
        }
    }

    pub(super) fn set_reverb_param(&mut self, cursor: usize, value: f32) {
        let clamped = value.clamp(0.0, 1.0);
        let param_idx = match cursor {
            0 => {
                self.reverb_size = clamped;
                reverb::REVERB_PARAM_SIZE
            }
            1 => {
                self.reverb_brightness = clamped;
                reverb::REVERB_PARAM_BRIGHT
            }
            2 => {
                self.reverb_replace = clamped;
                reverb::REVERB_PARAM_REPLACE
            }
            _ => return,
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.lg.0,
                crate::audiograph::ParamMsg {
                    idx: param_idx,
                    logical_id: self.reverb_node_id as u64,
                    fvalue: clamped,
                },
            );
        }
    }

    fn adjust_reverb_param(&mut self, delta: f32) {
        let current = match self.reverb_param_cursor {
            0 => self.reverb_size,
            1 => self.reverb_brightness,
            2 => self.reverb_replace,
            _ => return,
        };
        self.set_reverb_param(self.reverb_param_cursor, current + delta);
    }

    fn adjust_slot_param(&self, direction: f32) {
        let track = self.cursor_track;
        let slot_idx = self.effect_slot_cursor;
        let param_idx = self.effect_param_cursor;

        let desc = match self
            .effect_descriptors
            .get(track)
            .and_then(|d| d.get(slot_idx))
        {
            Some(d) => d,
            None => return,
        };
        if param_idx >= desc.params.len() {
            return;
        }
        let param_desc = &desc.params[param_idx];

        let chain = &self.state.effect_chains[track];
        if slot_idx >= chain.len() {
            return;
        }
        let slot = &chain[slot_idx];

        if self.has_selection() {
            for step in self.selected_steps() {
                let current = slot
                    .plocks
                    .get(step, param_idx)
                    .unwrap_or_else(|| slot.defaults.get(param_idx));
                let inc = param_desc.increment(current);
                let new_val = param_desc.clamp(current + direction * inc);
                slot.plocks.set(step, param_idx, new_val);
            }
        } else {
            let old = slot.defaults.get(param_idx);
            let inc = param_desc.increment(old);
            let new_val = param_desc.clamp(old + direction * inc);
            slot.defaults.set(param_idx, new_val);
            // Send immediate param update to audio graph
            self.send_slot_param(track, slot_idx, param_idx, new_val);
        }
    }

    pub(super) fn send_slot_param(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
        value: f32,
    ) {
        let chain = &self.state.effect_chains[track];
        if slot_idx >= chain.len() {
            return;
        }
        let slot = &chain[slot_idx];
        let node_id = slot.node_id.load(Ordering::Relaxed);
        if node_id == 0 {
            return;
        }
        let idx = slot.resolve_node_idx(param_idx);
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

    fn toggle_slot_boolean(&self) {
        let param_idx = self.effect_param_cursor;

        let slot = match self.current_slot() {
            Some(s) => s,
            None => return,
        };

        if self.has_selection() {
            for step in self.selected_steps() {
                let current = slot
                    .plocks
                    .get(step, param_idx)
                    .unwrap_or_else(|| slot.defaults.get(param_idx));
                let new_val = if current > 0.5 { 0.0 } else { 1.0 };
                slot.plocks.set(step, param_idx, new_val);
            }
        } else {
            let current = slot.defaults.get(param_idx);
            let new_val = if current > 0.5 { 0.0 } else { 1.0 };
            slot.defaults.set(param_idx, new_val);
        }
    }

    /// When the delay's "synced" boolean is toggled, swap the "time" param
    /// between Continuous (ms) and Enum (sync division labels) and reset the value.
    fn update_delay_time_param_kind(&mut self) {
        const DELAY_SLOT: usize = 1;
        const SYNCED_PARAM: usize = 1;
        const TIME_PARAM: usize = 2;

        if self.effect_slot_cursor != DELAY_SLOT || self.effect_param_cursor != SYNCED_PARAM {
            return;
        }

        let track = self.cursor_track;
        let slot = match self
            .state
            .effect_chains
            .get(track)
            .and_then(|c| c.get(DELAY_SLOT))
        {
            Some(s) => s,
            None => return,
        };
        let synced = slot.defaults.get(SYNCED_PARAM) > 0.5;

        let desc = match self
            .effect_descriptors
            .get_mut(track)
            .and_then(|d| d.get_mut(DELAY_SLOT))
        {
            Some(d) => d,
            None => return,
        };
        if TIME_PARAM >= desc.params.len() {
            return;
        }

        if synced {
            let labels: Vec<String> = SyncDivision::ALL
                .iter()
                .map(|d| d.label().to_string())
                .collect();
            desc.params[TIME_PARAM].kind = ParamKind::Enum { labels };
            desc.params[TIME_PARAM].min = 0.0;
            desc.params[TIME_PARAM].max = (SyncDivision::ALL.len() - 1) as f32;
            // Default to 1/4 note (index 6)
            slot.defaults.set(TIME_PARAM, 6.0);
        } else {
            desc.params[TIME_PARAM].kind = ParamKind::Continuous {
                unit: Some("ms".to_string()),
            };
            desc.params[TIME_PARAM].min = 1.0;
            desc.params[TIME_PARAM].max = 2000.0;
            slot.defaults.set(TIME_PARAM, 250.0);
        }
    }

    fn get_current_slot_value(&self) -> f32 {
        match self.current_slot() {
            Some(slot) => slot.defaults.get(self.effect_param_cursor),
            None => 0.0,
        }
    }
}

// ── Drawing ──

pub(super) fn draw_effects_column(
    frame: &mut Frame,
    app: &mut App,
    area: Rect,
    region_focused: bool,
) {
    let col_focused = region_focused && app.params_column == 1;
    let border_style = if col_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    // Build title with slot tabs — only show non-empty slots + [+] button
    let visible = app.visible_effect_indices();
    let mut title_spans = vec![];
    if let Some(descs) = app.effect_descriptors.get(app.cursor_track) {
        for &i in &visible {
            if i >= descs.len() {
                continue;
            }
            let desc = &descs[i];
            let is_selected = i == app.effect_slot_cursor;
            let style = if is_selected && col_focused {
                Style::default().fg(Color::Black).bg(Color::White).bold()
            } else if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(100, 100, 100))
            } else {
                Style::default().fg(Color::Gray)
            };
            let label = if is_selected {
                format!("[< {} >]", desc.name)
            } else {
                format!("[  {}  ]", desc.name)
            };
            title_spans.push(Span::styled(label, style));
            title_spans.push(Span::raw(" "));
        }
        // Show [+] tab if there's room for more custom effects
        if app.can_add_custom_effect() {
            let plus_style = if col_focused {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Rgb(60, 60, 60))
            };
            title_spans.push(Span::styled("[+]", plus_style));
        }

        // Reverb tab (always visible)
        let reverb_selected = app.effect_slot_cursor == REVERB_TAB;
        let reverb_style = if reverb_selected && col_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(180, 140, 220))
                .bold()
        } else if reverb_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(120, 90, 160))
        } else {
            Style::default().fg(Color::Rgb(120, 90, 160))
        };
        title_spans.push(Span::raw(" "));
        let reverb_label = if reverb_selected {
            "[< Reverb >]"
        } else {
            "[  Reverb  ]"
        };
        title_spans.push(Span::styled(reverb_label, reverb_style));
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

    // Reverb tab rendering
    if app.effect_slot_cursor == REVERB_TAB {
        let reverb_params: [(&str, f32); 3] = [
            ("size", app.reverb_size),
            ("brightness", app.reverb_brightness),
            ("replace", app.reverb_replace),
        ];
        let is_entering_value = col_focused && app.input_mode == InputMode::ValueEntry;

        for (i, (name, val)) in reverb_params.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let row_y = inner.y + i as u16;
            let is_cursor_row = col_focused && app.reverb_param_cursor == i;
            let cursor = if is_cursor_row { "> " } else { "  " };
            let cursor_style = if is_cursor_row {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Rgb(180, 140, 220))
            };

            let label_width = 12;
            let value_width = 14;

            if is_cursor_row && is_entering_value {
                let spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(
                        format!("{:<width$}", name, width = label_width),
                        Style::default().fg(Color::Gray),
                    ),
                    Span::styled(
                        format!("{}\u{2588}", app.value_buffer),
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(Color::Rgb(60, 60, 20))
                            .bold(),
                    ),
                    Span::styled(
                        "  Enter: set  Esc: cancel",
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                let line = Line::from(spans);
                let row_area = Rect::new(inner.x, row_y, inner.width, 1);
                frame.render_widget(Paragraph::new(line), row_area);
                continue;
            }

            let formatted = format!("{:.2}", val);
            let norm = val.clamp(0.0, 1.0);
            let mut spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(
                    format!("{:<width$}", name, width = label_width),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{:<width$}", formatted, width = value_width),
                    cursor_style,
                ),
            ];

            let slider_width = (inner.width as usize).saturating_sub(label_width + value_width + 6);
            if slider_width > 2 {
                let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                let bar: String = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                spans.push(Span::styled(
                    format!("[{}]", bar),
                    Style::default().fg(Color::Rgb(160, 130, 200)),
                ));
            }

            let line = Line::from(spans);
            let row_area = Rect::new(inner.x, row_y, inner.width, 1);
            frame.render_widget(Paragraph::new(line), row_area);
        }
        return;
    }

    let track = app.cursor_track;
    let slot_idx = app.effect_slot_cursor;

    // Check if this is an empty custom slot with no effect loaded
    let is_custom_slot = slot_idx >= BUILTIN_SLOT_COUNT;
    let chain = &app.state.effect_chains[track];
    let has_node = if slot_idx < chain.len() {
        chain[slot_idx].node_id.load(Ordering::Relaxed) != 0
    } else {
        false
    };

    if is_custom_slot && !has_node {
        // No effect loaded — show hint
        let hint = Line::from(vec![
            Span::styled("  Ctrl+L", Style::default().fg(Color::White).bold()),
            Span::styled(" to add effect", Style::default().fg(Color::DarkGray)),
        ]);
        let row_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(Paragraph::new(hint), row_area);
        return;
    }

    // Render params for current slot
    let desc = match app
        .effect_descriptors
        .get(track)
        .and_then(|d| d.get(slot_idx))
    {
        Some(d) => d,
        None => return,
    };

    if slot_idx >= chain.len() {
        return;
    }
    let slot = &chain[slot_idx];
    let is_entering_value = col_focused && app.input_mode == InputMode::ValueEntry;

    for (i, param_desc) in desc.params.iter().enumerate() {
        let row_y = inner.y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let default_val = slot.defaults.get(i);
        let is_cursor_row = col_focused && app.effect_param_cursor == i;
        let cursor = if is_cursor_row { "> " } else { "  " };
        let cursor_style = if is_cursor_row {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let label_width = 12;
        let value_width = 14;

        // Value entry mode
        if is_cursor_row && is_entering_value {
            let target_label = if !app.visual_steps.is_empty() {
                format!("p-lock {} steps", app.visual_steps.len())
            } else if app.selection_anchor.is_some() {
                let (lo, hi) = app.selected_range();
                format!("p-lock steps {}-{}", lo + 1, hi + 1)
            } else {
                "default".to_string()
            };
            let spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(
                    format!("{:<width$}", param_desc.name, width = label_width),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{}\u{2588}", app.value_buffer),
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::Rgb(60, 60, 20))
                        .bold(),
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
            let plock_val = slot.plocks.get(app.cursor_step, i);
            match plock_val {
                Some(v) => (v, Some(" (p-lock)")),
                None => (default_val, None),
            }
        } else {
            (default_val, None)
        };

        let formatted = param_desc.format_value(display_val);

        let mut spans = vec![
            Span::styled(cursor, cursor_style),
            Span::styled(
                format!("{:<width$}", param_desc.name, width = label_width),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{:<width$}", formatted, width = value_width),
                cursor_style,
            ),
        ];

        if let Some(lbl) = plock_label {
            spans.push(Span::styled(lbl, Style::default().fg(Color::White)));
        }

        // Slider for numeric params — reflects the "heard" value at the current playhead step
        if !param_desc.is_boolean() {
            let range = param_desc.max - param_desc.min;
            if range > 0.0 {
                let (slider_val, is_plock) = if app.has_selection() {
                    let pv = slot.plocks.get(app.cursor_step, i);
                    (pv.unwrap_or(default_val), pv.is_some())
                } else if app.state.is_playing() {
                    let step = app.state.current_step();
                    let pv = slot.plocks.get(step, i);
                    (pv.unwrap_or(default_val), pv.is_some())
                } else {
                    (default_val, false)
                };
                let norm = param_desc.normalize(slider_val);
                let slider_width =
                    (inner.width as usize).saturating_sub(label_width + value_width + 6);
                if slider_width > 2 {
                    let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                    let bar: String =
                        "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                    let slider_color = if is_plock {
                        Color::Cyan
                    } else {
                        Color::White
                    };
                    spans.push(Span::styled(
                        format!("[{}]", bar),
                        Style::default().fg(slider_color),
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

pub(super) fn draw_effect_picker(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.filtered_picker_items();
    let max_visible = 10usize;
    let list_height = items.len().min(max_visible) as u16;
    let w = 36u16;
    let h = list_height + 4; // 1 border top + 1 filter line + 1 blank + list + 1 border bottom
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let picker_area = Rect::new(x, y, w, h);

    // Clear background
    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        let row_area = Rect::new(x, y + row, w, 1);
        frame.render_widget(Paragraph::new(" ".repeat(w as usize)).style(bg), row_area);
    }

    let block = Block::default()
        .title(" Effects ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(picker_area);
    frame.render_widget(block, picker_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // Filter input line
    let filter_text = format!(" > {}\u{2588}", app.picker_filter);
    let filter_line = Line::from(Span::styled(filter_text, Style::default().fg(Color::White)));
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    // Item list
    let list_start_y = inner.y + 1;
    for (i, item) in items.iter().enumerate() {
        if i >= max_visible {
            break;
        }
        let row_y = list_start_y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let is_cursor = i == app.picker_cursor;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if item == "+ New effect" {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if is_cursor { " > " } else { "   " };
        let truncated: String = item
            .chars()
            .take((inner.width as usize).saturating_sub(4))
            .collect();
        let text = format!(
            "{}{:<width$}",
            prefix,
            truncated,
            width = (inner.width as usize).saturating_sub(3)
        );
        let row_area = Rect::new(inner.x, row_y, inner.width, 1);
        frame.render_widget(Paragraph::new(text).style(style), row_area);
    }
}

pub(super) fn draw_compiling_overlay(frame: &mut Frame, pending: &PendingCompile, area: Rect) {
    const SPINNER: &[char] = &[
        '\u{28F7}', '\u{28EF}', '\u{28DF}', '\u{287F}', '\u{28BF}', '\u{28FB}', '\u{28FD}',
        '\u{28FE}',
    ];
    let spin = SPINNER[pending.tick / 2 % SPINNER.len()];
    let name_display = if pending.name.len() > 14 {
        format!("{}...", &pending.name[..11])
    } else {
        pending.name.clone()
    };

    let w = 20u16;
    let h = 4u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let overlay = Rect::new(x, y, w, h);

    // Clear background
    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        let row_area = Rect::new(x, y + row, w, 1);
        frame.render_widget(Paragraph::new(" ".repeat(w as usize)).style(bg), row_area);
    }

    let block = Block::default()
        .title(" Compiling ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    if inner.height >= 2 && inner.width >= 4 {
        let line1 = Line::from(Span::styled(
            format!("  {} {}  ", spin, name_display),
            Style::default().fg(Color::Yellow),
        ));
        let center_y = inner.y + inner.height / 2;
        frame.render_widget(
            Paragraph::new(line1),
            Rect::new(inner.x, center_y, inner.width, 1),
        );
    }
}
