use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;

use crate::sequencer::{SwingResolution, Timebase, MAX_STEPS};

use super::effects_draw::draw_effects_column;
use crate::accumulator::{AccumMode, ACCUMULATOR_REGISTRY};

use super::{
    App, EffectTab, InputMode, Region, AC_FN, AC_LAST, AC_LIMIT, AC_MODE, TP_ATTACK, TP_FTS,
    TP_GATE, TP_LAST, TP_MASTER, TP_POLY, TP_RELEASE, TP_SEND, TP_STEPS, TP_SWING,
    TP_SWING_RESOLUTION, TP_TIMEBASE, TP_VOLUME,
};

/// Static labels for the timebase dropdown (derived from Timebase::LABELS).
const TIMEBASE_LABELS: [&str; Timebase::COUNT] = Timebase::LABELS;
const SWING_RESOLUTION_LABELS: [&str; SwingResolution::COUNT] = SwingResolution::LABELS;

// ── App impl: params input ──

impl App {
    pub(super) fn for_each_selected_track<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut Self, usize),
    {
        let tracks = self.selected_tracks();
        for track in tracks {
            f(self, track);
        }
    }

    pub(super) fn handle_params_input(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.tracks.is_empty() {
            return;
        }

        match self.ui.params_column {
            0 => self.handle_track_params_column(code),
            1 => self.handle_effects_column(code, _modifiers),
            _ => {}
        }
    }

    pub(super) fn handle_track_params_column(&mut self, code: KeyCode) {
        if self.ui.track_params_tab == 1 {
            self.handle_accum_params_column(code);
            return;
        }

        let tp = &self.state.pattern.track_params[self.ui.cursor_track];

        match code {
            KeyCode::Up => {
                if self.ui.track_param_cursor > 0 {
                    self.ui.track_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if self.ui.track_param_cursor < TP_LAST {
                    self.ui.track_param_cursor += 1;
                }
            }
            KeyCode::Right => {
                // Track tab → Accum tab (not effects directly)
                self.ui.track_params_tab = 1;
            }
            KeyCode::Left => {} // Already at leftmost column
            KeyCode::Enter => {
                if self.ui.track_param_cursor == TP_GATE {
                    self.for_each_selected_track(|app, track| {
                        app.state.pattern.track_params[track].toggle_gate();
                    });
                } else if self.ui.track_param_cursor == TP_POLY {
                    self.for_each_selected_track(|app, track| {
                        app.state.pattern.track_params[track].toggle_polyphonic();
                    });
                } else if self.ui.track_param_cursor == TP_TIMEBASE {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    // Show p-locked value for selected step, or track default
                    let current_tb = if self.has_selection() {
                        let step = self.selected_steps()[0];
                        self.state.pattern.timebase_plocks[self.ui.cursor_track]
                            .get(step)
                            .unwrap_or(tp.get_timebase())
                    } else {
                        tp.get_timebase()
                    };
                    self.ui.dropdown_cursor = current_tb as u8 as usize;
                    self.ui.input_mode = InputMode::Dropdown;
                } else if self.ui.track_param_cursor == TP_SWING_RESOLUTION {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = tp.get_swing_resolution() as usize;
                    self.ui.input_mode = InputMode::Dropdown;
                } else if self.ui.track_param_cursor == TP_FTS {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = tp.get_fts_scale();
                    self.ui.input_mode = InputMode::Dropdown;
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => match self.ui.track_param_cursor {
                TP_ATTACK => tp.set_attack_ms(tp.get_attack_ms() + 5.0),
                TP_RELEASE => tp.set_release_ms(tp.get_release_ms() + 10.0),
                TP_SWING => tp.set_swing(tp.get_swing() + 1.0),
                TP_SWING_RESOLUTION => tp.next_swing_resolution(),
                TP_STEPS => {
                    tp.set_num_steps(tp.get_num_steps() + 1);
                    self.clamp_cursor_to_steps();
                }
                TP_VOLUME => self.adjust_track_volume(0.05),
                TP_TIMEBASE => tp.next_timebase(),
                TP_SEND => self.adjust_track_send(0.05),
                TP_MASTER => self.adjust_master_volume(0.05),
                TP_FTS => {
                    tp.set_fts_scale((tp.get_fts_scale() + 1).min(crate::scale::SCALES.len() - 1))
                }
                _ => {}
            },
            KeyCode::Char('-') => match self.ui.track_param_cursor {
                TP_ATTACK => tp.set_attack_ms(tp.get_attack_ms() - 5.0),
                TP_RELEASE => tp.set_release_ms(tp.get_release_ms() - 10.0),
                TP_SWING => tp.set_swing(tp.get_swing() - 1.0),
                TP_SWING_RESOLUTION => tp.prev_swing_resolution(),
                TP_STEPS => {
                    tp.set_num_steps(tp.get_num_steps().saturating_sub(1).max(1));
                    self.clamp_cursor_to_steps();
                }
                TP_VOLUME => self.adjust_track_volume(-0.05),
                TP_TIMEBASE => tp.prev_timebase(),
                TP_SEND => self.adjust_track_send(-0.05),
                TP_MASTER => self.adjust_master_volume(-0.05),
                TP_FTS => tp.set_fts_scale(tp.get_fts_scale().saturating_sub(1)),
                _ => {}
            },
            KeyCode::Backspace | KeyCode::Delete => {
                // Clear timebase p-locks on selected steps
                if self.ui.track_param_cursor == TP_TIMEBASE && self.has_selection() {
                    for step in self.selected_steps() {
                        self.state.pattern.timebase_plocks[self.ui.cursor_track].clear(step);
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if self.ui.track_param_cursor > TP_GATE
                    && self.ui.track_param_cursor != TP_POLY
                    && self.ui.track_param_cursor != TP_TIMEBASE
                    && self.ui.track_param_cursor != TP_SWING_RESOLUTION
                {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                }
            }
            _ => {}
        }
    }

    pub(super) fn push_send_gain(&self, track: usize) {
        let send_lid = self.state.runtime.send_lids[track].load(Ordering::Acquire);
        if send_lid != 0 {
            let tp = &self.state.pattern.track_params[track];
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: 0,
                        logical_id: send_lid,
                        fvalue: tp.get_send(),
                    },
                );
            }
        }
    }

    pub(super) fn push_track_volume(&self, track: usize) {
        let Some(node) = self.graph.track_node_ids.get(track) else {
            return;
        };
        let tp = &self.state.pattern.track_params[track];
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: 0,
                    logical_id: node.voice_sum_id as u64,
                    fvalue: tp.get_volume(),
                },
            );
        }
    }

    pub(super) fn push_master_volume(&self) {
        let volume = f32::from_bits(self.state.transport.master_volume.load(Ordering::Relaxed));
        for bus_id in [self.graph.bus_l_id, self.graph.bus_r_id] {
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: 0,
                        logical_id: bus_id as u64,
                        fvalue: volume,
                    },
                );
            }
        }
    }

    fn handle_accum_params_column(&mut self, code: KeyCode) {
        let tp = &self.state.pattern.track_params[self.ui.cursor_track];
        match code {
            KeyCode::Up => {
                if self.ui.accum_cursor > 0 {
                    self.ui.accum_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if self.ui.accum_cursor < AC_LAST {
                    self.ui.accum_cursor += 1;
                }
            }
            KeyCode::Enter => match self.ui.accum_cursor {
                AC_FN | AC_MODE => {
                    self.ui.dropdown_open = true;
                    self.ui.track_param_dropdown = true;
                    self.ui.dropdown_cursor = if self.ui.accum_cursor == AC_FN {
                        tp.get_accumulator_idx()
                    } else {
                        tp.get_accum_mode() as usize
                    };
                    self.ui.input_mode = InputMode::Dropdown;
                }
                _ => {}
            },
            KeyCode::Char('+') | KeyCode::Char('=') => {
                if self.ui.accum_cursor == AC_LIMIT {
                    self.for_each_selected_track(|app, track| {
                        let tp = &app.state.pattern.track_params[track];
                        tp.set_accum_limit(tp.get_accum_limit() + 1.0);
                    });
                }
            }
            KeyCode::Char('-') => {
                if self.ui.accum_cursor == AC_LIMIT {
                    self.for_each_selected_track(|app, track| {
                        let tp = &app.state.pattern.track_params[track];
                        tp.set_accum_limit(tp.get_accum_limit() - 1.0);
                    });
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if self.ui.accum_cursor == AC_LIMIT {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                }
            }
            KeyCode::Left => {
                self.ui.track_params_tab = 0;
            }
            KeyCode::Right => {
                self.ui.params_column = 1;
                if self.is_current_custom_track() {
                    let visible = self.visible_effect_indices();
                    if !matches!(self.ui.effect_tab, EffectTab::Slot(idx) if visible.contains(&idx))
                        && !matches!(
                            self.ui.effect_tab,
                            EffectTab::Synth
                                | EffectTab::Mod
                                | EffectTab::Sources
                                | EffectTab::Reverb
                        )
                    {
                        self.ui.effect_tab = EffectTab::Synth;
                        self.ui.instrument_param_cursor = 0;
                        self.ui.synth_scroll_offset = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn adjust_track_send(&mut self, delta: f32) {
        self.for_each_selected_track(|app, track| {
            let tp = &app.state.pattern.track_params[track];
            tp.set_send(tp.get_send() + delta);
            app.push_send_gain(track);
        });
    }

    fn adjust_track_volume(&mut self, delta: f32) {
        self.for_each_selected_track(|app, track| {
            let tp = &app.state.pattern.track_params[track];
            tp.set_volume(tp.get_volume() + delta);
            app.push_track_volume(track);
        });
    }

    fn adjust_master_volume(&mut self, delta: f32) {
        let current = f32::from_bits(self.state.transport.master_volume.load(Ordering::Relaxed));
        self.state.transport.master_volume.store(
            (current + delta).clamp(0.0, 2.0).to_bits(),
            Ordering::Relaxed,
        );
        self.push_master_volume();
    }

    pub(super) fn handle_dropdown(&mut self, code: KeyCode) {
        match code {
            KeyCode::Up => {
                if self.ui.dropdown_cursor > 0 {
                    self.ui.dropdown_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.dropdown_max_items();
                if self.ui.dropdown_cursor < max.saturating_sub(1) {
                    self.ui.dropdown_cursor += 1;
                }
            }
            KeyCode::Enter => {
                self.apply_dropdown_selection();
                self.ui.dropdown_open = false;
                self.ui.track_param_dropdown = false;
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.dropdown_open = false;
                self.ui.track_param_dropdown = false;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    fn dropdown_max_items(&self) -> usize {
        if self.ui.track_param_dropdown {
            // Accum tab dropdowns
            if self.ui.track_params_tab == 1 {
                return match self.ui.accum_cursor {
                    AC_FN => ACCUMULATOR_REGISTRY.len(),
                    AC_MODE => AccumMode::COUNT,
                    _ => 0,
                };
            }
            if self.ui.track_param_cursor == TP_FTS {
                return crate::scale::SCALES.len();
            }
            if self.ui.track_param_cursor == TP_SWING_RESOLUTION {
                return SwingResolution::COUNT;
            }
            return Timebase::COUNT;
        }
        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            if let Some(desc) = self.current_instrument_descriptor() {
                if self.ui.instrument_param_cursor > 0 {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        if let crate::effects::ParamKind::Enum { ref labels } =
                            desc.params[param_idx].kind
                        {
                            return labels.len();
                        }
                    }
                }
            }
            return 0;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            if let Some(desc) = self.current_mod_descriptor() {
                if self.ui.mod_param_cursor < desc.params.len() {
                    if let crate::effects::ParamKind::Enum { ref labels } =
                        desc.params[self.ui.mod_param_cursor].kind
                    {
                        return labels.len();
                    }
                }
            }
            return 0;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            if let Some(desc) = self.current_source_descriptor() {
                if self.ui.source_param_cursor < desc.params.len() {
                    if let crate::effects::ParamKind::Enum { ref labels } =
                        desc.params[self.ui.source_param_cursor].kind
                    {
                        return labels.len();
                    }
                }
            }
            return 0;
        }
        if let Some(desc) = self.current_slot_descriptor() {
            if self.ui.effect_param_cursor < desc.params.len() {
                if let crate::effects::ParamKind::Enum { ref labels } =
                    desc.params[self.ui.effect_param_cursor].kind
                {
                    return labels.len();
                }
            }
        }
        0
    }

    pub(super) fn dropdown_labels(&self) -> &[String] {
        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            if let Some(desc) = self.current_instrument_descriptor() {
                if self.ui.instrument_param_cursor > 0 {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1)
                    {
                        if let crate::effects::ParamKind::Enum { ref labels } =
                            desc.params[param_idx].kind
                        {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if self.ui.effect_tab == EffectTab::Mod {
            if let Some(desc) = self.current_instrument_descriptor() {
                let mod_indices = self.mod_param_indices(self.ui.cursor_track);
                if let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) {
                    if let Some(param) = desc.params.get(param_idx) {
                        if let crate::effects::ParamKind::Enum { ref labels } = param.kind {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if self.ui.effect_tab == EffectTab::Sources {
            if let Some(desc) = self.current_instrument_descriptor() {
                let source_indices = self.source_param_actual_indices(self.ui.cursor_track);
                if let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) {
                    if let Some(param) = desc.params.get(param_idx) {
                        if let crate::effects::ParamKind::Enum { ref labels } = param.kind {
                            return labels;
                        }
                    }
                }
            }
            return &[];
        }
        if let Some(desc) = self.current_slot_descriptor() {
            if self.ui.effect_param_cursor < desc.params.len() {
                if let crate::effects::ParamKind::Enum { ref labels } =
                    desc.params[self.ui.effect_param_cursor].kind
                {
                    return labels;
                }
            }
        }
        &[]
    }

    fn apply_dropdown_selection(&mut self) {
        if self.ui.track_param_dropdown {
            if self.ui.track_params_tab == 1 {
                self.for_each_selected_track(|app, track| {
                    let tp = &app.state.pattern.track_params[track];
                    match app.ui.accum_cursor {
                        AC_FN => {
                            tp.set_accumulator_idx(app.ui.dropdown_cursor);
                            if let Some(def) = ACCUMULATOR_REGISTRY.get(app.ui.dropdown_cursor) {
                                tp.set_accum_limit(def.default_limit);
                            }
                        }
                        AC_MODE => tp.set_accum_mode(app.ui.dropdown_cursor as u32),
                        _ => {}
                    }
                });
                return;
            }
            if self.ui.track_param_cursor == TP_FTS {
                let scale_idx = self.ui.dropdown_cursor;
                self.for_each_selected_track(|app, track| {
                    app.state.pattern.track_params[track].set_fts_scale(scale_idx);
                });
                return;
            }
            if self.ui.track_param_cursor == TP_SWING_RESOLUTION {
                let resolution = SwingResolution::from_index(self.ui.dropdown_cursor as u32);
                self.for_each_selected_track(|app, track| {
                    app.state.pattern.track_params[track].set_swing_resolution(resolution);
                });
                return;
            }
            let tb = Timebase::from_index(self.ui.dropdown_cursor as u32);
            if self.has_selection() {
                // P-lock: set timebase override for selected steps
                for step in self.selected_steps() {
                    self.state.pattern.timebase_plocks[self.ui.cursor_track].set(step, tb);
                }
            } else {
                self.for_each_selected_track(|app, track| {
                    app.state.pattern.track_params[track].set_timebase(tb);
                });
            }
            return;
        }

        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            let val = self.ui.dropdown_cursor as f32;
            if self.ui.instrument_param_cursor == 0 {
                return;
            }
            let synth_indices = self.synth_param_indices(self.ui.cursor_track);
            let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) else {
                return;
            };
            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
            if self.has_selection() {
                for step in self.selected_steps() {
                    slot.plocks.set(step, param_idx, val);
                }
            } else {
                slot.defaults.set(param_idx, val);
                self.send_instrument_param(self.ui.cursor_track, param_idx, val);
                self.mark_track_sound_dirty(self.ui.cursor_track);
            }
            return;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            let val = self.ui.dropdown_cursor as f32;
            let mod_indices = self.mod_param_indices(self.ui.cursor_track);
            let Some(&param_idx) = mod_indices.get(self.ui.mod_param_cursor) else {
                return;
            };
            self.set_instrument_param_or_plock(self.ui.cursor_track, param_idx, val);
            return;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            let val = self.ui.dropdown_cursor as f32;
            let source_indices = self.source_param_actual_indices(self.ui.cursor_track);
            let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
                return;
            };
            self.set_instrument_param_or_plock(self.ui.cursor_track, param_idx, val);
            return;
        }

        let val = self.ui.dropdown_cursor as f32;
        let param_idx = self.ui.effect_param_cursor;

        let slot = match self.current_slot() {
            Some(s) => s,
            None => return,
        };
        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let Some(desc) = self.current_slot_descriptor() else {
            return;
        };
        let Some(param_desc) = desc.params.get(param_idx) else {
            return;
        };
        if matches!(
            param_desc.host_control,
            Some(crate::effects::HostControl::FxSidechain { .. })
        ) {
            self.apply_effect_sidechain_selection(
                self.ui.cursor_track,
                slot_idx,
                param_idx,
                self.ui.dropdown_cursor,
            );
            slot.defaults.set(param_idx, val);
            return;
        }

        if self.has_selection() {
            for step in self.selected_steps() {
                slot.plocks.set(step, param_idx, val);
            }
        } else {
            slot.defaults.set(param_idx, val);
        }
    }
}

// ── Drawing ──

pub(super) fn draw_params_region(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.ui.focused_region == Region::Params;

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
    let col_focused = region_focused && app.ui.params_column == 0;
    let border_style = if col_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    let tab = app.ui.track_params_tab;

    // Tab labels matching the effects column style
    let track_selected = tab == 0;
    let accum_selected = tab == 1;
    let track_style = if track_selected && col_focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(100, 160, 220))
            .bold()
    } else if track_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(50, 80, 110))
    } else {
        Style::default().fg(Color::Rgb(50, 80, 110))
    };
    let accum_style = if accum_selected && col_focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(200, 100, 140))
            .bold()
    } else if accum_selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Rgb(100, 50, 70))
    } else {
        Style::default().fg(Color::Rgb(100, 50, 70))
    };
    let track_label = if track_selected {
        "[< Track >]"
    } else {
        "[  Track  ]"
    };
    let accum_label = if accum_selected {
        "[< Accum >]"
    } else {
        "[  Accum  ]"
    };

    let title_spans = vec![
        Span::styled(track_label, track_style),
        Span::raw(" "),
        Span::styled(accum_label, accum_style),
    ];

    let block = Block::default()
        .title(Line::from(title_spans))
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.ui.layout.track_params_inner = inner;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    if tab == 1 {
        draw_accum_tab(frame, app, inner, col_focused);
    } else {
        draw_track_tab(frame, app, inner, col_focused);
    }

    if app.ui.dropdown_open && app.ui.track_param_dropdown && col_focused {
        draw_track_param_dropdown(frame, app, inner);
    }
}

fn draw_track_tab(frame: &mut Frame, app: &App, area: Rect, col_focused: bool) {
    let tp = &app.state.pattern.track_params[app.ui.cursor_track];
    let attack = tp.get_attack_ms();
    let release = tp.get_release_ms();
    let swing = tp.get_swing();
    let swing_resolution = tp.get_swing_resolution();
    let steps = tp.get_num_steps();
    let volume = tp.get_volume();
    let default_tb = tp.get_timebase();
    let timebase_display = if app.has_selection() {
        let step = app.selected_steps()[0];
        match app.state.pattern.timebase_plocks[app.ui.cursor_track].get(step) {
            Some(tb) => format!("{} [P]", tb.label()),
            None => default_tb.label().to_string(),
        }
    } else {
        default_tb.label().to_string()
    };
    let send = tp.get_send();
    let master = f32::from_bits(app.state.transport.master_volume.load(Ordering::Relaxed));

    let params: Vec<(&str, String, Option<f32>)> = vec![
        (
            "gate",
            if tp.is_gate_on() {
                "ON".into()
            } else {
                "OFF".into()
            },
            None,
        ),
        ("attack", format!("{:.0} ms", attack), Some(attack / 500.0)),
        (
            "release",
            format!("{:.0} ms", release),
            Some(release / 2000.0),
        ),
        (
            "swing",
            format!("{:.0}%", swing),
            Some((swing - 50.0) / 25.0),
        ),
        ("swing res", swing_resolution.label().to_string(), None),
        (
            "steps",
            format!("{}", steps),
            Some(steps as f32 / MAX_STEPS as f32),
        ),
        ("vol", format!("{:.2}", volume), Some(volume)),
        ("timebase", timebase_display, None),
        ("send", format!("{:.2}", send), Some(send)),
        ("master", format!("{:.2}", master), Some(master / 2.0)),
        (
            "poly",
            if tp.is_polyphonic() {
                "ON".into()
            } else {
                "OFF".into()
            },
            None,
        ),
        (
            "fts",
            crate::scale::SCALES
                .get(tp.get_fts_scale())
                .map(|s| s.name)
                .unwrap_or("Off")
                .to_string(),
            None,
        ),
    ];

    let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;
    draw_param_rows(
        frame,
        app,
        area,
        col_focused,
        &params,
        app.ui.track_param_cursor,
        is_entering_value,
    );
}

fn draw_accum_tab(frame: &mut Frame, app: &App, area: Rect, col_focused: bool) {
    let tp = &app.state.pattern.track_params[app.ui.cursor_track];
    let accum_idx = tp.get_accumulator_idx();
    let accum_name = ACCUMULATOR_REGISTRY
        .get(accum_idx)
        .map(|d| d.name)
        .unwrap_or("Off");
    let limit = tp.get_accum_limit();
    let mode = AccumMode::from_u32(tp.get_accum_mode());

    let params: Vec<(&str, String, Option<f32>)> = vec![
        ("fn", accum_name.to_string(), None),
        ("limit", format!("{:.0}", limit), Some(limit / 127.0)),
        ("mode", mode.label().to_string(), None),
    ];

    let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;
    draw_param_rows(
        frame,
        app,
        area,
        col_focused,
        &params,
        app.ui.accum_cursor,
        is_entering_value,
    );
}

/// Shared row-rendering logic for both tabs.
fn draw_param_rows(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    col_focused: bool,
    params: &[(&str, String, Option<f32>)],
    cursor: usize,
    is_entering_value: bool,
) {
    let visible = area.height as usize;
    let scroll = if cursor >= visible {
        cursor + 1 - visible
    } else {
        0
    };

    for (i, (name, value, slider)) in params.iter().enumerate().skip(scroll) {
        let row = i - scroll;
        if row >= visible {
            break;
        }
        let y = area.y + row as u16;
        let is_cursor_row = col_focused && cursor == i;
        let cur_str = if is_cursor_row { "> " } else { "  " };
        let cursor_style = if is_cursor_row {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let label_width = 10;
        let value_width = 12;

        if is_cursor_row && is_entering_value {
            let spans = vec![
                Span::styled(cur_str, cursor_style),
                Span::styled(
                    format!("{:<width$}", name, width = label_width),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{}\u{2588}", app.ui.value_buffer),
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
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, y, area.width, 1),
            );
            continue;
        }

        let mut spans = vec![
            Span::styled(cur_str, cursor_style),
            Span::styled(
                format!("{:<width$}", name, width = label_width),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!("{:<width$}", value, width = value_width),
                cursor_style,
            ),
        ];

        if let Some(norm) = slider {
            let slider_width = (area.width as usize).saturating_sub(label_width + value_width + 4);
            if slider_width > 2 {
                let filled = ((norm * slider_width as f32).round() as usize).min(slider_width);
                let bar = "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                spans.push(Span::styled(
                    format!("[{}]", bar),
                    Style::default().fg(Color::Cyan),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

/// Draw a generic dropdown overlay given a list of item labels.
fn draw_dropdown_items(
    frame: &mut Frame,
    items: &[&str],
    cursor: usize,
    area: Rect,
    anchor_row: u16,
) {
    if items.is_empty() {
        return;
    }

    let dropdown_x = area.x + 14; // after label
    let dropdown_width = 16u16;

    // How many rows can we show?
    let max_rows = (area.y + area.height).saturating_sub(area.y) as usize;
    let visible_count = items.len().min(max_rows);
    if visible_count == 0 {
        return;
    }

    // Scroll so the cursor is always visible
    let scroll = if cursor >= visible_count {
        cursor + 1 - visible_count
    } else {
        0
    };

    // Position: try to start at the anchor row, but shift up if it would overflow
    let ideal_y = area.y + anchor_row;
    let dropdown_y = ideal_y.min((area.y + area.height).saturating_sub(visible_count as u16));

    for vi in 0..visible_count {
        let i = scroll + vi;
        if i >= items.len() {
            break;
        }
        let y = dropdown_y + vi as u16;
        let is_cursor = i == cursor;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 60))
        };
        let text = format!(
            " {:<width$}",
            items[i],
            width = (dropdown_width - 2) as usize
        );
        let cell = Rect::new(dropdown_x, y, dropdown_width, 1);
        frame.render_widget(Paragraph::new(text).style(style), cell);
    }
}

pub(super) fn draw_dropdown(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<&str> = app.dropdown_labels().iter().map(|s| s.as_str()).collect();
    draw_dropdown_items(
        frame,
        &items,
        app.ui.dropdown_cursor,
        area,
        app.ui.effect_param_cursor as u16,
    );
}

pub(super) fn draw_track_param_dropdown(frame: &mut Frame, app: &App, area: Rect) {
    if app.ui.track_params_tab == 1 {
        let anchor = app.ui.accum_cursor as u16;
        match app.ui.accum_cursor {
            AC_FN => {
                let names: Vec<&str> = ACCUMULATOR_REGISTRY.iter().map(|d| d.name).collect();
                draw_dropdown_items(frame, &names, app.ui.dropdown_cursor, area, anchor);
            }
            AC_MODE => {
                draw_dropdown_items(
                    frame,
                    &AccumMode::LABELS,
                    app.ui.dropdown_cursor,
                    area,
                    anchor,
                );
            }
            _ => {}
        }
        return;
    }
    if app.ui.track_param_cursor == TP_FTS {
        let names: Vec<&str> = crate::scale::SCALES.iter().map(|s| s.name).collect();
        draw_dropdown_items(
            frame,
            &names,
            app.ui.dropdown_cursor,
            area,
            app.ui.track_param_cursor as u16,
        );
        return;
    }
    if app.ui.track_param_cursor == TP_SWING_RESOLUTION {
        draw_dropdown_items(
            frame,
            &SWING_RESOLUTION_LABELS,
            app.ui.dropdown_cursor,
            area,
            app.ui.track_param_cursor as u16,
        );
        return;
    }
    draw_dropdown_items(
        frame,
        &TIMEBASE_LABELS,
        app.ui.dropdown_cursor,
        area,
        app.ui.track_param_cursor as u16,
    );
}
