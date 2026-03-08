use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::sequencer::StepParam;

use super::browser::draw_sidebar;
use super::cirklon::draw_cirklon_region;
use super::effects_draw::{draw_compiling_overlay, draw_effect_picker, draw_instrument_picker};
use super::params::draw_params_region;
use super::{App, InputMode, Region, SidebarMode};

pub(super) fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

pub(super) fn param_color(_param: StepParam) -> Color {
    Color::White
}

pub(super) fn is_in_selection(app: &App, step: usize) -> bool {
    if app.ui.visual_steps.contains(&step) {
        return true;
    }
    if app.ui.selection_anchor.is_some() {
        let (lo, hi) = app.selected_range();
        return step >= lo && step <= hi;
    }
    false
}

pub(super) fn region_border_style(app: &App, region: Region) -> Style {
    if app.ui.focused_region == region {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Global info bar + Cirklon+Sidebar row + Params + Help
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),  // Global info bar + meter
            Constraint::Min(13),    // Cirklon + Sidebar row
            Constraint::Length(10), // Params region
            Constraint::Length(2),  // Help bar
        ])
        .split(area);

    // Split middle row horizontally: Cirklon | Sidebar
    let mid_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(40),    // Cirklon
            Constraint::Length(30), // Sidebar
        ])
        .split(chunks[1]);

    app.ui.layout.info_bar = chunks[0];
    draw_global_info(frame, app, chunks[0]);
    draw_cirklon_region(frame, app, mid_chunks[0]);
    draw_sidebar(frame, app, mid_chunks[1]);
    draw_params_region(frame, app, chunks[2]);
    draw_help_bar(frame, app, chunks[3]);
    draw_stereo_meter(frame, app, area);

    // Draw picker overlay on top of everything
    if app.ui.input_mode == InputMode::EffectPicker {
        draw_effect_picker(frame, app, area);
    }
    if app.ui.input_mode == InputMode::InstrumentPicker {
        draw_instrument_picker(frame, app, area);
    }
    if app.ui.input_mode == InputMode::PresetNameEntry {
        draw_preset_name_prompt(frame, app, area);
    }

    // Draw compiling overlay
    if let Some(ref pending) = app.editor.pending_compile {
        draw_compiling_overlay(frame, pending, area);
    }
}

fn draw_global_info(frame: &mut Frame, app: &mut App, area: Rect) {
    use super::PatternBtn;
    const AUDIO_STATUS_WIDTH: u16 = 31;

    // ── Play button (3 chars) ──
    let play_label = if app.state.is_playing() {
        " \u{25b6} "
    } else {
        " \u{23f8} "
    };
    let play_style = if app.state.is_playing() {
        Style::default().fg(Color::Black).bg(Color::Green).bold()
    } else {
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(60, 60, 60))
            .bold()
    };
    let play_rect = Rect::new(area.x, area.y, 3, 1);
    app.ui.layout.info_bar = play_rect; // play button rect
    frame.render_widget(
        Paragraph::new(Span::styled(play_label, play_style)),
        play_rect,
    );

    // ── REC button (5 chars) ──
    let rec_style = if app.ui.recording {
        Style::default().fg(Color::White).bg(Color::Red).bold()
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .bg(Color::Rgb(40, 40, 40))
            .bold()
    };
    let rec_rect = Rect::new(area.x + 3, area.y, 5, 1);
    app.ui.layout.rec_button = rec_rect;
    frame.render_widget(Paragraph::new(Span::styled(" REC ", rec_style)), rec_rect);

    // ── Info text (rest of row 0, without [pat X/Y]) ──
    let bpm = app.state.bpm.load(Ordering::Relaxed);

    let mut spans = vec![Span::styled(
        format!("  {} BPM", bpm),
        Style::default().fg(Color::White).bg(Color::DarkGray).bold(),
    )];

    if !app.tracks.is_empty() {
        let track = app.ui.cursor_track;
        let tp = &app.state.track_params[track];
        let default_tb = tp.get_timebase();
        let current_step = app.state.track_step(track);
        let resolved_tb = app.state.timebase_plocks[track].resolve(current_step, default_tb);
        spans.push(Span::styled(
            format!("  {:<3}", resolved_tb.label()),
            Style::default().fg(Color::Yellow),
        ));

        let sample_name = &app.tracks[app.ui.cursor_track];
        spans.push(Span::styled(
            format!("  {}", sample_name),
            Style::default().fg(Color::White),
        ));
    }

    if app.any_track_armed() {
        spans.push(Span::styled(
            format!("  Oct:{}", app.ui.keyboard_octave / 12),
            Style::default().fg(Color::Cyan),
        ));
        let thresh = f32::from_bits(app.state.record_quantize_thresh.load(Ordering::Relaxed));
        spans.push(Span::styled(
            format!("  Q:{:.0}%", thresh * 100.0),
            Style::default().fg(Color::Magenta),
        ));
    }

    if let Some((ref msg, ref when)) = app.editor.status_message {
        if when.elapsed() < Duration::from_secs(3) {
            spans.push(Span::styled(
                format!("  {}", msg),
                Style::default().fg(Color::Yellow),
            ));
        }
    }

    let info_x = area.x + 8;
    let info_w = area.width.saturating_sub(8);
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(info_x, area.y, info_w, 1),
    );

    // ── Row 1: Pattern buttons ──
    let cur_pat = app.state.current_pattern.load(Ordering::Relaxed) as usize;
    let num_pats = app.state.num_patterns.load(Ordering::Relaxed) as usize;

    let row1_y = area.y + 1;
    let row1_w = area.width.saturating_sub(AUDIO_STATUS_WIDTH); // leave room for meter + CPU box
    let row1_area = Rect::new(area.x, row1_y, row1_w, 1);
    app.ui.layout.pattern_buttons_area = row1_area;

    let page_start = app.ui.pattern_page * 10;
    let page_end = (page_start + 10).min(num_pats);

    let mut btn_layout: Vec<(u16, u16, PatternBtn)> = Vec::new();
    let mut x = area.x; // flush left, aligned with play button

    // Prev-page indicator
    if page_start > 0 {
        let span = Span::styled(" \u{25c0} ", Style::default().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(span), Rect::new(x, row1_y, 3, 1));
        btn_layout.push((x, x + 3, PatternBtn::PrevPage));
        x += 4;
    }

    // Pattern number buttons
    let active_style = Style::default().fg(Color::Black).bg(Color::Yellow).bold();
    let inactive_style = Style::default().fg(Color::White).bg(Color::Rgb(60, 60, 60));

    for i in page_start..page_end {
        let label = format!(" {} ", i + 1); // 1-indexed display
        let w = label.len() as u16;
        let style = if i == cur_pat {
            active_style
        } else {
            inactive_style
        };
        frame.render_widget(
            Paragraph::new(Span::styled(&label, style)),
            Rect::new(x, row1_y, w, 1),
        );
        btn_layout.push((x, x + w, PatternBtn::Pattern(i)));
        x += w + 1;
    }

    // Next-page indicator
    if page_end < num_pats {
        let span = Span::styled(" \u{25b6} ", Style::default().fg(Color::DarkGray));
        frame.render_widget(Paragraph::new(span), Rect::new(x, row1_y, 3, 1));
        btn_layout.push((x, x + 3, PatternBtn::NextPage));
        x += 4;
    }

    // Clone button [+]
    let clone_style = Style::default().fg(Color::Green).bg(Color::Rgb(40, 40, 40));
    frame.render_widget(
        Paragraph::new(Span::styled(" + ", clone_style)),
        Rect::new(x, row1_y, 3, 1),
    );
    btn_layout.push((x, x + 3, PatternBtn::Clone));
    x += 4;

    // Delete button [-]
    let delete_style = Style::default().fg(Color::Red).bg(Color::Rgb(40, 40, 40));
    frame.render_widget(
        Paragraph::new(Span::styled(" - ", delete_style)),
        Rect::new(x, row1_y, 3, 1),
    );
    btn_layout.push((x, x + 3, PatternBtn::Delete));

    app.ui.pattern_btn_layout = btn_layout;
}

fn draw_stereo_meter(frame: &mut Frame, app: &App, area: Rect) {
    let meter_width = 20u16;
    let cpu_width = 11u16;
    let gap_width = 1u16;
    let meter_height = 2u16;
    let total_width = meter_width + gap_width + cpu_width;
    if area.width < total_width + 2 || area.height < meter_height {
        return;
    }

    let cpu_x = area.x + area.width - cpu_width - 1;
    let x = cpu_x - gap_width - meter_width;
    let y = area.y;

    let peak_l = f32::from_bits(app.state.peak_l.load(Ordering::Relaxed));
    let peak_r = f32::from_bits(app.state.peak_r.load(Ordering::Relaxed));
    let cpu_load_pct = f32::from_bits(app.state.cpu_load_pct.load(Ordering::Relaxed));

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
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        spans
    };

    // L channel
    let mut l_spans = vec![Span::styled("L ", Style::default().fg(Color::DarkGray))];
    l_spans.extend(render_bar(peak_l));
    let l_line = Line::from(l_spans);
    frame.render_widget(Paragraph::new(l_line), Rect::new(x, y, meter_width, 1));

    // R channel
    let mut r_spans = vec![Span::styled("R ", Style::default().fg(Color::DarkGray))];
    r_spans.extend(render_bar(peak_r));
    let r_line = Line::from(r_spans);
    frame.render_widget(Paragraph::new(r_line), Rect::new(x, y + 1, meter_width, 1));

    let cpu_color = if cpu_load_pct >= 95.0 {
        Color::Red
    } else if cpu_load_pct >= 75.0 {
        Color::Yellow
    } else {
        Color::Cyan
    };

    let cpu_label = Line::from(vec![Span::styled(
        format!("{:>10}", "CPU"),
        Style::default().fg(Color::DarkGray).bold(),
    )]);
    let cpu_value = Line::from(vec![Span::styled(
        format!("{:>10.1}%", cpu_load_pct),
        Style::default().fg(cpu_color).bold(),
    )]);
    frame.render_widget(Paragraph::new(cpu_label), Rect::new(cpu_x, y, cpu_width, 1));
    frame.render_widget(
        Paragraph::new(cpu_value),
        Rect::new(cpu_x, y + 1, cpu_width, 1),
    );
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if app.ui.input_mode == InputMode::StepArm {
        vec![Line::from(Span::styled(
            "  [ARM] 1-8: toggle tracks 1-8  q-u: tracks 9-15  ,: rec  r/Esc/Enter: exit",
            Style::default().fg(Color::Rgb(255, 100, 100)),
        ))]
    } else if app.ui.input_mode == InputMode::StepInsert {
        vec![Line::from(Span::styled(
            "  [INSERT] 1-8: steps 1-8  q-i: steps 9-16  Shift: accent (vel=1)  [/]: page  Esc/Enter: exit",
            Style::default().fg(Color::Rgb(100, 220, 100)),
        ))]
    } else if app.ui.input_mode == InputMode::StepSelect {
        vec![Line::from(Span::styled(
            "  [SELECT] 1-8: select 1-8  q-i: select 9-16  x: delete  [/]: page  Esc/Enter: exit",
            Style::default().fg(Color::Rgb(220, 180, 100)),
        ))]
    } else if app.ui.focused_region == Region::Sidebar {
        let hint_text = match app.effective_sidebar_mode() {
            SidebarMode::InstrumentPicker => {
                "  \u{2191}\u{2193}: navigate  Enter: select instrument  Esc: back".to_string()
            }
            SidebarMode::Presets => {
                "  Type to filter  \u{2191}\u{2193}: navigate  Enter: load  Ctrl+S: save new  Ctrl+O: overwrite  Ctrl+R: revert  Esc: back".to_string()
            }
            _ => {
                let action = match app.ui.sidebar_mode {
                    SidebarMode::AddTrack => "Enter: add track",
                    SidebarMode::Audition => "Enter: swap sample",
                    _ => unreachable!(),
                };
                format!("  Type to filter  \u{2191}\u{2193}: navigate  \u{2190}\u{2192}: collapse/expand  {}  Esc: back", action)
            }
        };
        vec![Line::from(Span::styled(
            hint_text,
            Style::default().fg(Color::Yellow),
        ))]
    } else if app.ui.input_mode == InputMode::EffectPicker {
        vec![Line::from(Span::styled(
            "  Type to filter  \u{2191}\u{2193}: navigate  Enter: select  Esc: cancel",
            Style::default().fg(Color::Yellow),
        ))]
    } else if app.ui.input_mode == InputMode::PatternSelect {
        vec![Line::from(Span::styled(
            "  0-9: pattern number  c: clone  x: delete  Enter: confirm  Esc: cancel",
            Style::default().fg(Color::Yellow),
        ))]
    } else {
        match app.ui.focused_region {
            Region::Cirklon => {
                if app.ui.input_mode == InputMode::ValueEntry {
                    vec![Line::from(Span::styled(
                        "  0-9: digits  .: decimal  -: negate  Enter: set  Esc: cancel",
                        Style::default().fg(Color::DarkGray),
                    ))]
                } else if app.has_selection() {
                    vec![Line::from(Span::styled(
                    "  Shift+\u{2190}\u{2192}: extend  S-\u{2191}\u{2193}: value  +/-: dbl/hlf  Enter: toggle  0-9: type  Esc: deselect",
                    Style::default().fg(Color::Rgb(120, 150, 220)),
                ))]
                } else {
                    vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: step  \u{2191}\u{2193}: track  S-\u{2191}\u{2193}: value  +/-: dbl/hlf  ,: rec  /: search  r: arm  i: ins  s: sel",
                    Style::default().fg(Color::DarkGray),
                ))]
                }
            }
            Region::Params => {
                if app.ui.dropdown_open {
                    vec![Line::from(Span::styled(
                        "  \u{2191}\u{2193}: select  Enter: confirm  Esc: cancel",
                        Style::default().fg(Color::Yellow),
                    ))]
                } else if app.ui.params_column == 1 {
                    vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: column/effect  \u{2191}\u{2193}: param  S-\u{2191}\u{2193}: adjust  -/0-9: type  [/]: step  Enter: toggle  Tab: region",
                    Style::default().fg(Color::DarkGray),
                ))]
                } else {
                    vec![Line::from(Span::styled(
                    "  \u{2190}\u{2192}: column/effect  \u{2191}\u{2193}: param  S-\u{2191}\u{2193}: adjust  -/0-9: type  Enter: toggle  Tab: region",
                    Style::default().fg(Color::DarkGray),
                ))]
                }
            }
            Region::Sidebar => {
                // Handled by the focused_region == Sidebar check above; unreachable
                vec![]
            }
        }
    };

    let text = Text::from(lines);
    let help = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60))),
    );
    frame.render_widget(help, area);
}

fn draw_preset_name_prompt(frame: &mut Frame, app: &App, area: Rect) {
    let width = 40.min(area.width.saturating_sub(4));
    let height = 3;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect::new(x, y, width, height);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Save Preset ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let text = format!("{}█", app.ui.value_buffer);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Yellow)),
        inner,
    );
}
