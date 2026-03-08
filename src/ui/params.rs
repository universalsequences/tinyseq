use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;

use crate::sequencer::{Timebase, MAX_STEPS};

use super::effects_draw::draw_effects_column;
use super::{
    App, EffectTab, InputMode, Region, TP_ATTACK, TP_GATE, TP_LAST, TP_POLY, TP_RELEASE, TP_SEND,
    TP_STEPS, TP_SWING, TP_TIMEBASE,
};

/// Static labels for the timebase dropdown (derived from Timebase::LABELS).
const TIMEBASE_LABELS: [&str; Timebase::COUNT] = Timebase::LABELS;

// ── App impl: params input ──

impl App {
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
                self.ui.params_column = 1;
                if self.is_current_custom_track()
                    && self.ui.effect_tab != EffectTab::Synth
                    && self.ui.effect_tab != EffectTab::Mod
                    && self.ui.effect_tab != EffectTab::Sources
                    && self.ui.effect_tab != EffectTab::Reverb
                {
                    // Default to Synth tab for custom tracks
                    let visible = self.visible_effect_indices();
                    if !matches!(self.ui.effect_tab, EffectTab::Slot(idx) if visible.contains(&idx))
                    {
                        self.ui.effect_tab = EffectTab::Synth;
                        self.ui.instrument_param_cursor = 0;
                        self.ui.synth_scroll_offset = 0;
                    }
                }
            }
            KeyCode::Left => {} // Already at leftmost column
            KeyCode::Enter => {
                if self.ui.track_param_cursor == TP_GATE {
                    tp.toggle_gate();
                } else if self.ui.track_param_cursor == TP_POLY {
                    tp.toggle_polyphonic();
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
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => match self.ui.track_param_cursor {
                TP_ATTACK => tp.set_attack_ms(tp.get_attack_ms() + 5.0),
                TP_RELEASE => tp.set_release_ms(tp.get_release_ms() + 10.0),
                TP_SWING => tp.set_swing(tp.get_swing() + 1.0),
                TP_STEPS => {
                    tp.set_num_steps(tp.get_num_steps() + 1);
                    self.clamp_cursor_to_steps();
                }
                TP_TIMEBASE => tp.next_timebase(),
                TP_SEND => self.adjust_track_send(0.05),
                _ => {}
            },
            KeyCode::Char('-') => match self.ui.track_param_cursor {
                TP_ATTACK => tp.set_attack_ms(tp.get_attack_ms() - 5.0),
                TP_RELEASE => tp.set_release_ms(tp.get_release_ms() - 10.0),
                TP_SWING => tp.set_swing(tp.get_swing() - 1.0),
                TP_STEPS => {
                    tp.set_num_steps(tp.get_num_steps().saturating_sub(1).max(1));
                    self.clamp_cursor_to_steps();
                }
                TP_TIMEBASE => tp.prev_timebase(),
                TP_SEND => self.adjust_track_send(-0.05),
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

    fn adjust_track_send(&mut self, delta: f32) {
        let track = self.ui.cursor_track;
        let tp = &self.state.pattern.track_params[track];
        tp.set_send(tp.get_send() + delta);
        self.push_send_gain(track);
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
            return Timebase::COUNT;
        }
        // Synth tab dropdown
        if self.ui.effect_tab == EffectTab::Synth {
            if let Some(desc) = self.current_instrument_descriptor() {
                if self.ui.instrument_param_cursor > 0 {
                    let synth_indices = self.synth_param_indices(self.ui.cursor_track);
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) {
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
                    if let Some(&param_idx) = synth_indices.get(self.ui.instrument_param_cursor - 1) {
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
                let source_indices = self.source_param_indices(self.ui.cursor_track);
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

    fn apply_dropdown_selection(&self) {
        if self.ui.track_param_dropdown {
            let tb = Timebase::from_index(self.ui.dropdown_cursor as u32);
            if self.has_selection() {
                // P-lock: set timebase override for selected steps
                for step in self.selected_steps() {
                    self.state.pattern.timebase_plocks[self.ui.cursor_track].set(step, tb);
                }
            } else {
                // Track default
                self.state.pattern.track_params[self.ui.cursor_track].set_timebase(tb);
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
            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
            slot.defaults.set(param_idx, val);
            self.send_instrument_param(self.ui.cursor_track, param_idx, val);
            self.mark_track_sound_dirty(self.ui.cursor_track);
            return;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            let val = self.ui.dropdown_cursor as f32;
            let source_indices = self.source_param_indices(self.ui.cursor_track);
            let Some(&param_idx) = source_indices.get(self.ui.source_param_cursor) else {
                return;
            };
            let slot = &self.state.pattern.instrument_slots[self.ui.cursor_track];
            slot.defaults.set(param_idx, val);
            self.send_instrument_param(self.ui.cursor_track, param_idx, val);
            self.mark_track_sound_dirty(self.ui.cursor_track);
            return;
        }

        let val = self.ui.dropdown_cursor as f32;
        let param_idx = self.ui.effect_param_cursor;

        let slot = match self.current_slot() {
            Some(s) => s,
            None => return,
        };

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

    let block = Block::default()
        .title(" Track ")
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.ui.layout.track_params_inner = inner;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    let tp = &app.state.pattern.track_params[app.ui.cursor_track];
    let attack = tp.get_attack_ms();
    let release = tp.get_release_ms();
    let swing = tp.get_swing();
    let steps = tp.get_num_steps();
    let default_tb = tp.get_timebase();
    // Show p-locked timebase for selected step, or track default
    let timebase_display = if app.has_selection() {
        let step = app.selected_steps()[0]; // show first selected step's value
        match app.state.pattern.timebase_plocks[app.ui.cursor_track].get(step) {
            Some(tb) => format!("{} [P]", tb.label()),
            None => default_tb.label().to_string(),
        }
    } else {
        default_tb.label().to_string()
    };

    let send = tp.get_send();

    let params: Vec<(&str, String, Option<f32>)> = vec![
        (
            "gate",
            if tp.is_gate_on() {
                "ON".to_string()
            } else {
                "OFF".to_string()
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
        (
            "steps",
            format!("{}", steps),
            Some(steps as f32 / MAX_STEPS as f32),
        ),
        ("timebase", timebase_display, None),
        ("send", format!("{:.2}", send), Some(send)),
        (
            "poly",
            if tp.is_polyphonic() {
                "ON".to_string()
            } else {
                "OFF".to_string()
            },
            None,
        ),
    ];

    let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;

    for (i, (name, value, slider)) in params.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let y = inner.y + i as u16;
        let is_cursor_row = col_focused && app.ui.track_param_cursor == i;
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
            let line = Line::from(spans);
            let row_area = Rect::new(inner.x, y, inner.width, 1);
            frame.render_widget(Paragraph::new(line), row_area);
            continue;
        }

        let mut spans = vec![
            Span::styled(cursor, cursor_style),
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

    // Track param dropdown overlay (e.g. timebase)
    if app.ui.dropdown_open && app.ui.track_param_dropdown && col_focused {
        draw_track_param_dropdown(frame, app, inner);
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
    draw_dropdown_items(
        frame,
        &TIMEBASE_LABELS,
        app.ui.dropdown_cursor,
        area,
        app.ui.track_param_cursor as u16,
    );
}
