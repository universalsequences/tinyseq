use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

use crate::sequencer::{StepParam, STEPS_PER_PAGE};

use super::draw::{is_in_selection, param_color, region_border_style};
use super::{App, InputMode, Region, BAR_HEIGHT, COL_WIDTH};

// ── App impl: cirklon input ──

impl App {
    pub(super) fn handle_cirklon_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
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
                if self.selection_anchor.is_some() {
                    self.shift_selection(-1);
                } else if self.cursor_step > 0 {
                    self.cursor_step -= 1;
                } else {
                    self.cursor_step = ns - 1;
                }
            }
            KeyCode::Right => {
                if self.selection_anchor.is_some() {
                    self.shift_selection(1);
                } else {
                    self.cursor_step = (self.cursor_step + 1) % ns;
                }
            }

            // Shift+Up/Down: adjust step param value
            KeyCode::Up if has_shift => {
                self.adjust_selected(self.active_param.increment());
            }
            KeyCode::Down if has_shift => {
                self.adjust_selected(-self.active_param.increment());
            }

            // Up/Down: switch tracks
            KeyCode::Up => {
                if self.cursor_track > 0 {
                    self.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.cursor_track = self.tracks.len() - 1;
                }
                self.clamp_cursor_to_steps();
                self.sync_sidebar_to_track();
            }
            KeyCode::Down => {
                if !self.tracks.is_empty() {
                    self.cursor_track = (self.cursor_track + 1) % self.tracks.len();
                }
                self.clamp_cursor_to_steps();
                self.sync_sidebar_to_track();
            }

            KeyCode::Enter => {
                if !self.tracks.is_empty() {
                    let track = self.cursor_track;
                    for step in self.selected_steps() {
                        self.state.toggle_step_and_clear_plocks(track, step);
                    }
                }
            }

            KeyCode::Backspace | KeyCode::Delete => {
                if !self.tracks.is_empty() {
                    let track = self.cursor_track;
                    for step in self.selected_steps() {
                        if self.state.patterns[track].is_active(step) {
                            self.state.toggle_step_and_clear_plocks(track, step);
                        }
                    }
                    self.selection_anchor = None;
                    self.visual_steps.clear();
                }
            }

            KeyCode::Char('+') | KeyCode::Char('=') => {
                if !self.tracks.is_empty() {
                    let old_len = self.num_steps();
                    let new_len = self.state.duplicate_track_pattern(self.cursor_track);
                    if new_len == old_len {
                        self.status_message = Some((
                            format!("Already at max ({} steps)", new_len),
                            Instant::now(),
                        ));
                    } else {
                        self.status_message = Some((
                            format!("Pattern doubled to {} steps", new_len),
                            Instant::now(),
                        ));
                    }
                    self.clamp_cursor_to_steps();
                }
            }
            KeyCode::Char('-') => {
                if !self.tracks.is_empty() {
                    let old_len = self.num_steps();
                    let new_len = self.state.halve_track_pattern(self.cursor_track);
                    if new_len == old_len {
                        self.status_message =
                            Some(("Already at minimum (1 step)".to_string(), Instant::now()));
                    } else {
                        self.status_message = Some((
                            format!("Pattern halved to {} steps", new_len),
                            Instant::now(),
                        ));
                    }
                    self.clamp_cursor_to_steps();
                }
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
            KeyCode::Char('b') => {
                self.bpm_entry = true;
                self.value_buffer.clear();
                self.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char('p') => {
                self.input_mode = InputMode::PatternSelect;
                self.value_buffer.clear();
                self.pattern_clone_pending = false;
            }
            KeyCode::Char(']') => {
                let total_pages = ns.div_ceil(STEPS_PER_PAGE);
                if total_pages > 1 {
                    let current_page = self.current_page();
                    self.cursor_step = ((current_page + 1) % total_pages) * STEPS_PER_PAGE;
                }
            }
            KeyCode::Char('[') => {
                let total_pages = ns.div_ceil(STEPS_PER_PAGE);
                if total_pages > 1 {
                    let current_page = self.current_page();
                    self.cursor_step =
                        ((current_page + total_pages - 1) % total_pages) * STEPS_PER_PAGE;
                }
            }
            KeyCode::Char(c) => {
                if let Some(param) = StepParam::from_hotkey(c) {
                    if StepParam::VISIBLE.contains(&param) {
                        self.active_param = param;
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Drawing ──

pub(super) fn draw_cirklon_region(frame: &mut Frame, app: &mut App, area: Rect) {
    app.layout.cirklon_area = area;

    let mode_label = match app.input_mode {
        super::InputMode::StepInsert => Some(" INSERT "),
        super::InputMode::StepSelect => Some(" SELECT "),
        super::InputMode::StepArm => Some(" ARM "),
        _ => None,
    };
    let title_line = if let Some(label) = mode_label {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // Smooth sine-wave pulse between 192 (75%) and 255 (100%) brightness
        let t = (ms as f64) / 800.0 * std::f64::consts::TAU;
        let brightness = 192.0 + 63.0 * (0.5 + 0.5 * t.sin());
        let b = brightness as u8;
        let mode_style = Style::default().fg(Color::Rgb(b, b, b)).bold();
        Line::from(vec![
            Span::raw(" Cirklon "),
            Span::styled(label, mode_style),
        ])
    } else {
        Line::from(" Cirklon ")
    };
    let block = Block::default()
        .title(title_line)
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
            Constraint::Length(10), // Track list column
            Constraint::Min(0),     // Sequencer content
        ])
        .split(inner);

    app.layout.track_list = h_chunks[0];

    draw_track_list(frame, app, h_chunks[0]);

    // Sequencer content vertical layout
    let seq_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                 // param tabs
            Constraint::Length(BAR_HEIGHT as u16), // bars
            Constraint::Length(2),                 // trigger + step numbers
            Constraint::Length(2),                 // page blocks + playhead dot
            Constraint::Length(1),                 // value line
            Constraint::Length(1),                 // spacer
            Constraint::Length(3),                 // piano keyboard
        ])
        .split(h_chunks[1]);

    app.layout.param_tabs = seq_chunks[0];
    app.layout.bars = seq_chunks[1];
    app.layout.trigger_row = seq_chunks[2];
    app.layout.page_blocks_area = seq_chunks[3];

    draw_param_tabs(frame, app, seq_chunks[0]);
    draw_bars(frame, app, seq_chunks[1]);
    draw_trigger_row(frame, app, seq_chunks[2]);
    draw_page_blocks(frame, app, seq_chunks[3]);
    draw_value_line(frame, app, seq_chunks[4]);
    // seq_chunks[5] is the spacer row
    draw_piano_roll(frame, app, seq_chunks[6]);
}

fn draw_track_list(frame: &mut Frame, app: &App, area: Rect) {
    // Clear the entire track list area first to prevent stale content
    let buf = frame.buffer_mut();
    for y in area.y..(area.y + area.height) {
        for x in area.x..(area.x + area.width) {
            buf[(x, y)].reset();
        }
    }

    for (i, name) in app.tracks.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let y = area.y + i as u16;
        let is_selected = i == app.cursor_track;
        let sanitized: String = name
            .chars()
            .filter(|c| !c.is_control() && unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0) > 0)
            .collect();
        let trimmed = sanitized.trim_start();
        let truncated: String = trimmed.chars().take(2).collect();

        // Arm indicator: 2-char block for visibility, clickable
        let armed = i < app.record_armed.len() && app.record_armed[i];
        let arm_sym = if armed {
            "\u{2588}\u{2588}"
        } else {
            "\u{2591}\u{2591}"
        }; // "██" vs "░░"
        let arm_style = if armed {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let label = format!("{} {:<2} ", i + 1, truncated);

        let style = if is_selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).bold()
        } else {
            // Flash decay: read value, decay by 30 per frame, interpolate gray(90)→white(255)
            let flash = app.state.trigger_flash[i].load(Ordering::Relaxed);
            let decayed = flash.saturating_sub(30);
            app.state.trigger_flash[i].store(decayed, Ordering::Relaxed);
            let base = 90u8;
            let brightness = base + ((255 - base) as u32 * decayed / 255) as u8;
            Style::default().fg(Color::Rgb(brightness, brightness, brightness))
        };

        // Write directly to buffer using set_string for proper unicode width handling
        let buf = frame.buffer_mut();
        let mut x = area.x;
        buf.set_string(x, y, &label, style);
        x += UnicodeWidthStr::width(label.as_str()) as u16;
        buf.set_string(x, y, arm_sym, arm_style);
        x += UnicodeWidthStr::width(arm_sym) as u16;
        // Fill remaining with spaces in label style
        let remaining = (area.x + area.width).saturating_sub(x) as usize;
        if remaining > 0 {
            buf.set_string(x, y, &" ".repeat(remaining), style);
        }
    }
}

fn draw_param_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::raw("  ")];
    for param in StepParam::VISIBLE {
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

/// Whether a step is in the "odd" beat group (steps 4-7, 12-15, ...).
fn is_beat_group_odd(step: usize) -> bool {
    (step / 4) % 2 == 1
}

/// Background color for a step column. Alternates every 4 steps for beat grouping.
fn step_bg(app: &App, step: usize, is_playing: bool, playhead: usize) -> Color {
    let is_cursor = step == app.cursor_step;
    let is_sel = app.has_selection() && is_in_selection(app, step);
    let is_ph = is_playing && step == playhead;

    if is_cursor {
        Color::Rgb(80, 80, 80)
    } else if is_sel {
        Color::Rgb(50, 50, 50)
    } else if is_ph {
        Color::Rgb(50, 50, 50)
    } else if is_beat_group_odd(step) {
        Color::Rgb(22, 22, 22)
    } else {
        Color::Reset
    }
}

/// Dim foreground color for inactive elements, alternating by beat group.
fn step_dim_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(70, 70, 70)
    } else {
        Color::Rgb(45, 45, 45)
    }
}

/// Step number color, alternating by beat group.
fn step_num_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(110, 110, 110)
    } else {
        Color::Rgb(70, 70, 70)
    }
}

/// Active (filled) foreground color, alternating by beat group.
/// Full white vs ~80% brightness white.
fn step_active_fg(step: usize) -> Color {
    if is_beat_group_odd(step) {
        Color::Rgb(200, 200, 200)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

fn draw_bars(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    // Branch to effect slot bars when effects column is focused
    if app.focused_region == Region::Params && app.params_column == 1 {
        draw_slot_bars(frame, app, area);
        return;
    }

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let sd = &app.state.step_data[app.cursor_track];
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

                let dim = step_dim_fg(step);
                let active_fg = step_active_fg(step);
                let (cell_text, fg_override) = if going_up {
                    if row < center {
                        let dist_from_center = center - row;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (
                                " \u{2588} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else if half_levels >= threshold + 1 {
                            (
                                " \u{2584} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else {
                            ("   ".to_string(), dim)
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), dim)
                    } else {
                        ("   ".to_string(), dim)
                    }
                } else {
                    if row > center {
                        let dist_from_center = row - center;
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (
                                " \u{2588} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else if half_levels >= threshold + 1 {
                            (
                                " \u{2580} ".to_string(),
                                if active { active_fg } else { dim },
                            )
                        } else {
                            ("   ".to_string(), dim)
                        }
                    } else if row == center {
                        ("\u{2500}\u{2500}\u{2500}".to_string(), dim)
                    } else {
                        ("   ".to_string(), dim)
                    }
                };

                let style = Style::default().fg(fg_override).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            } else {
                let rows_from_bottom = BAR_HEIGHT - 1 - row;
                let threshold = rows_from_bottom * 2;
                let level = if fill_levels >= threshold + 2 {
                    2
                } else if fill_levels >= threshold + 1 {
                    1
                } else {
                    0
                };

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

                let fg = if active {
                    step_active_fg(step)
                } else {
                    step_dim_fg(step)
                };
                let style = Style::default().fg(fg).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            }
        }
    }
}

/// Unified bar drawing for any effect slot.
fn draw_slot_bars(frame: &mut Frame, app: &App, area: Rect) {
    let track = app.cursor_track;
    let slot_idx = app.effect_slot_cursor;
    let param_idx = app.effect_param_cursor;

    let desc = match app
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

    // Skip boolean params
    if param_desc.is_boolean() {
        return;
    }

    let chain = &app.state.effect_chains[track];
    if slot_idx >= chain.len() {
        return;
    }
    let slot = &chain[slot_idx];

    let ns = app.num_steps();
    let global_playhead = app.state.current_step();
    let playhead = global_playhead % ns;
    let is_playing = app.state.is_playing();
    let (page_start, page_end) = app.page_range();
    let x_offset = 2u16;

    let default_color = Color::White;

    for step in page_start..page_end {
        let col_x = area.x + x_offset + (step - page_start) as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let plock_val = slot.plocks.get(step, param_idx);
        let value = plock_val.unwrap_or_else(|| slot.defaults.get(param_idx));
        let normalized = param_desc.normalize(value);

        let active = app.state.patterns[track].is_active(step);
        let playhead_on_page = playhead >= page_start && playhead < page_end;
        let bg = step_bg(app, step, is_playing && playhead_on_page, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;
            let rows_from_bottom = BAR_HEIGHT - 1 - row;
            let threshold = rows_from_bottom * 2;
            let level = if fill_levels >= threshold + 2 {
                2
            } else if fill_levels >= threshold + 1 {
                1
            } else {
                0
            };

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
                step_dim_fg(step)
            } else if plock_val.is_some() {
                Color::White
            } else {
                default_color
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
        // Check all slots for p-locks
        let has_plock = app.state.effect_chains[app.cursor_track]
            .iter()
            .any(|slot| {
                let np = slot.num_params.load(Ordering::Relaxed) as usize;
                slot.plocks.step_has_any_plock(step, np)
            });
        let ch = if active && has_plock {
            " \u{25c6} " // ◆ diamond — active with p-lock
        } else if active {
            " \u{25a0} " // ■ filled square — active
        } else {
            " \u{00b7} " // · middle dot — inactive
        };
        let fg = if active && has_plock {
            Color::Cyan
        } else if active {
            step_active_fg(step)
        } else {
            step_dim_fg(step)
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
        let beat_bg = step_bg(app, step, false, 0); // just for beat-group shading
        let style = if step == app.cursor_step {
            Style::default().fg(Color::White).bg(Color::Rgb(80, 80, 80))
        } else if is_sel {
            Style::default().fg(Color::Rgb(160, 160, 160)).bg(beat_bg)
        } else {
            Style::default().fg(step_num_fg(step)).bg(beat_bg)
        };
        let cell = Rect::new(col_x, area.y + 1, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(num).style(style), cell);
    }
}

fn draw_page_blocks(frame: &mut Frame, app: &mut App, area: Rect) {
    let ns = app.num_steps();
    let total_pages = ns.div_ceil(STEPS_PER_PAGE);
    if total_pages <= 1 || area.height < 2 {
        app.page_btn_layout.clear();
        return;
    }

    let current_page = app.current_page();
    let playing_page = (app.state.current_step() % ns) / STEPS_PER_PAGE;

    let mut btn_layout: Vec<(u16, u16, usize)> = Vec::new();
    let mut x = area.x + 2;

    // Row 1: page number buttons
    for p in 0..total_pages {
        let label = format!(" {} ", p + 1);
        let w = label.len().max(3) as u16;

        let style = if p == current_page {
            Style::default().fg(Color::Black).bg(Color::White).bold()
        } else {
            Style::default()
                .fg(Color::Gray)
                .bg(Color::Rgb(50, 50, 50))
        };

        if x + w <= area.x + area.width {
            frame.render_widget(
                Paragraph::new(Span::styled(&label, style)),
                Rect::new(x, area.y, w, 1),
            );
            btn_layout.push((x, x + w, p));
        }
        x += w;
    }

    // Row 2: playhead dot centered under the playing page
    for &(bx, bw, page) in &btn_layout {
        if page == playing_page {
            let block_w = bw - bx;
            let dot_x = bx + block_w / 2;
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "\u{2022}",
                    Style::default().fg(Color::White),
                )),
                Rect::new(dot_x, area.y + 1, 1, 1),
            );
            break;
        }
    }

    app.page_btn_layout = btn_layout;
}

fn draw_value_line(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        return;
    }

    let is_pattern_select = app.input_mode == InputMode::PatternSelect;
    let is_bpm_entry = app.input_mode == InputMode::ValueEntry && app.bpm_entry;
    let is_cirklon_entry = app.input_mode == InputMode::ValueEntry
        && !app.bpm_entry
        && app.focused_region == Region::Cirklon;

    let line = if is_bpm_entry {
        Line::from(vec![
            Span::styled("  BPM: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{}\u{2588}", app.value_buffer),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(60, 60, 60))
                    .bold(),
            ),
            Span::styled(
                "  Enter: set  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if is_pattern_select {
        if app.pattern_clone_pending {
            Line::from(vec![
                Span::styled(
                    "  Clone pattern \u{2192} new  ",
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    "Enter: confirm  Esc: cancel",
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("  Pattern: ", Style::default().fg(Color::White)),
                Span::styled(
                    format!("{}\u{2588}", app.value_buffer),
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(60, 60, 60))
                        .bold(),
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
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(60, 60, 60))
                    .bold(),
            ),
            Span::styled(
                "  Enter: set  Esc: cancel  -: negate",
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else if app.has_selection() {
        let sel_text = if !app.visual_steps.is_empty() {
            let count = app.visual_steps.len();
            format!("  {} steps selected", count)
        } else {
            let (lo, hi) = app.selected_range();
            let count = hi - lo + 1;
            format!("  Steps {}-{} selected ({} steps)", lo + 1, hi + 1, count)
        };
        Line::from(Span::styled(
            format!(
                "{}  {} = \u{2191}\u{2193}",
                sel_text,
                app.active_param.label(),
            ),
            Style::default().fg(Color::Rgb(160, 160, 160)),
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

fn draw_piano_roll(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.tracks.is_empty() || area.height < 2 {
        return;
    }

    let track = app.cursor_track;
    let now = Instant::now();

    // Reset note state on track change
    if track != app.piano_last_track {
        app.piano_notes.clear();
        app.piano_last_step = usize::MAX;
        app.piano_last_track = track;
    }

    // Detect new step triggers and add notes with duration-based expiry
    if app.state.is_playing() {
        let ns = app.num_steps();
        let step = app.state.current_step() % ns;

        if step != app.piano_last_step {
            app.piano_last_step = step;

            if app.state.patterns[track].is_active(step) {
                // Calculate note duration in wall-clock seconds
                let bpm = app.state.bpm.load(Ordering::Relaxed) as f64;
                let secs_per_step = 60.0 / bpm / 4.0;
                let dur = app.state.step_data[track].get(step, StepParam::Duration) as f64;
                let release_ms = app.state.track_params[track].get_release_ms() as f64;
                let total_secs = dur * secs_per_step + release_ms / 1000.0;
                let expires = now + Duration::from_secs_f64(total_secs);

                let cc = app.state.chord_data[track].count(step);
                if cc > 0 {
                    for n in 0..cc {
                        let t = app.state.chord_data[track].get(step, n).round() as i32;
                        app.piano_notes.push((t, expires));
                    }
                } else {
                    let t = app.state.step_data[track]
                        .get(step, StepParam::Transpose)
                        .round() as i32;
                    app.piano_notes.push((t, expires));
                }
            }
        }
    } else {
        // Sequencer stopped — let notes expire naturally but don't add new ones
        app.piano_last_step = usize::MAX;
    }

    // Prune expired notes
    app.piano_notes.retain(|(_, exp)| *exp > now);

    // Build active note set: ringing sequencer notes + held keyboard notes
    let mut active: Vec<i32> = app.piano_notes.iter().map(|(s, _)| *s).collect();
    for &(_, t, _, _) in &app.held_notes {
        active.push(t.round() as i32);
    }

    // 1 char per semitone, snap to full octaves, left-aligned
    let octaves = ((area.width as i32) / 12).min(5);
    let num_keys = octaves * 12;
    if num_keys <= 0 {
        return;
    }
    let half = num_keys / 2;
    let lo = -((half / 12) * 12);
    let hi = lo + num_keys;
    let x0 = area.x; // left-aligned with sequencer content

    let buf = frame.buffer_mut();

    let white_bg = Color::Rgb(200, 200, 200);
    let black_bg = Color::Rgb(30, 30, 30);
    let active_bg = Color::Cyan;
    let gap_bg = Color::Rgb(50, 50, 50);

    // Row 1 (back): All keys visible — white bright, black dark
    let y1 = area.y;
    for s in lo..hi {
        let x = x0 + (s - lo) as u16;
        if x >= area.x + area.width {
            break;
        }
        let black = matches!(s.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
        let lit = active.contains(&s);

        let bg = if lit {
            active_bg
        } else if black {
            black_bg
        } else {
            white_bg
        };
        buf.set_string(x, y1, " ", Style::default().bg(bg));
    }

    // Row 2 (front): White keys extend, black key positions become gaps
    if area.height >= 2 {
        let y2 = area.y + 1;
        for s in lo..hi {
            let x = x0 + (s - lo) as u16;
            if x >= area.x + area.width {
                break;
            }
            let black = matches!(s.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
            let lit = active.contains(&s);

            let bg = if lit {
                active_bg
            } else if black {
                gap_bg
            } else {
                white_bg
            };
            buf.set_string(x, y2, " ", Style::default().bg(bg));
        }
    }

    // Row 3: Octave labels
    if area.height >= 3 {
        let y3 = area.y + 2;
        for s in lo..hi {
            if s.rem_euclid(12) == 0 {
                let x = x0 + (s - lo) as u16;
                if x + 2 <= area.x + area.width {
                    let oct = s.div_euclid(12) + 4;
                    buf.set_string(
                        x,
                        y3,
                        &format!("C{}", oct),
                        Style::default().fg(Color::Rgb(70, 70, 70)),
                    );
                }
            }
        }
    }
}
