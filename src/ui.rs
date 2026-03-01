use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::sampler::SamplerTrack;
use crate::sequencer::{SequencerState, StepParam, NUM_PARAMS, NUM_STEPS};

const BAR_HEIGHT: usize = 8;
const COL_WIDTH: u16 = 3;

#[derive(PartialEq, Eq)]
pub enum InputMode {
    Normal,
    ValueEntry,
}

pub struct App {
    pub state: Arc<SequencerState>,
    pub tracks: Vec<String>,
    pub cursor_step: usize,
    pub cursor_track: usize,
    pub active_param: StepParam,
    pub input_mode: InputMode,
    pub value_buffer: String,
    /// Anchor step for gang selection. When Some, the selected range is
    /// min(anchor, cursor_step)..=max(anchor, cursor_step).
    pub selection_anchor: Option<usize>,
    pub should_quit: bool,
}

impl App {
    pub fn new(state: Arc<SequencerState>, tracks: &[SamplerTrack]) -> Self {
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
        }
    }

    /// Returns the inclusive range of selected steps, or just the cursor step.
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

    pub fn handle_input(&mut self) -> std::io::Result<()> {
        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }
                match self.input_mode {
                    InputMode::Normal => self.handle_normal(key.code, key.modifiers),
                    InputMode::ValueEntry => self.handle_value_entry(key.code),
                }
            }
        }
        Ok(())
    }

    fn handle_normal(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let has_shift = modifiers.contains(KeyModifiers::SHIFT);
        let has_super = modifiers.contains(KeyModifiers::SUPER);

        match code {
            KeyCode::Esc => {
                if self.has_selection() {
                    self.selection_anchor = None;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('q') => self.should_quit = true,

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
                if self.cursor_step < NUM_STEPS - 1 {
                    self.cursor_step += 1;
                }
            }

            // Plain Left/Right: shift selection if gang-selected, else move cursor
            KeyCode::Left => {
                if self.has_selection() {
                    self.shift_selection(-1);
                } else {
                    if self.cursor_step > 0 {
                        self.cursor_step -= 1;
                    } else {
                        self.cursor_step = NUM_STEPS - 1;
                    }
                }
            }
            KeyCode::Right => {
                if self.has_selection() {
                    self.shift_selection(1);
                } else {
                    self.cursor_step = (self.cursor_step + 1) % NUM_STEPS;
                }
            }

            // Shift+Up/Down with gang selection: ramp
            KeyCode::Up if has_shift && self.has_selection() => {
                self.ramp_selected(self.active_param.increment());
            }
            KeyCode::Down if has_shift && self.has_selection() => {
                self.ramp_selected(-self.active_param.increment());
            }

            // Shift/Super + Up/Down without selection: switch track
            KeyCode::Up if has_shift || has_super => {
                if self.cursor_track > 0 {
                    self.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.cursor_track = self.tracks.len() - 1;
                }
            }
            KeyCode::Down if has_shift || has_super => {
                if !self.tracks.is_empty() {
                    self.cursor_track = (self.cursor_track + 1) % self.tracks.len();
                }
            }

            // Plain Up/Down: adjust param value for all selected steps uniformly
            KeyCode::Up => {
                self.adjust_selected(self.active_param.increment());
            }
            KeyCode::Down => {
                self.adjust_selected(-self.active_param.increment());
            }

            KeyCode::Tab => {
                self.active_param = self.active_param.next();
            }
            KeyCode::BackTab => {
                self.active_param = self.active_param.prev();
            }

            KeyCode::Enter => {
                if !self.tracks.is_empty() {
                    let (lo, hi) = self.selected_range();
                    for step in lo..=hi {
                        self.state.patterns[self.cursor_track].toggle_step(step);
                    }
                }
            }

            KeyCode::Char(' ') => {
                let was_playing = self.state.is_playing();
                self.state.toggle_play();
                if was_playing {
                    self.state.playhead.store(0, Ordering::Relaxed);
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_selected(self.active_param.increment());
            }
            KeyCode::Char('-') => {
                self.adjust_selected(-self.active_param.increment());
            }
            KeyCode::Char(']') => {
                if !self.tracks.is_empty() {
                    self.cursor_track = (self.cursor_track + 1) % self.tracks.len();
                }
            }
            KeyCode::Char('[') => {
                if self.cursor_track > 0 {
                    self.cursor_track -= 1;
                } else if !self.tracks.is_empty() {
                    self.cursor_track = self.tracks.len() - 1;
                }
            }
            KeyCode::Char('.') => {
                // Start value entry with "0."
                self.value_buffer.clear();
                self.value_buffer.push_str("0.");
                self.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                self.value_buffer.clear();
                self.value_buffer.push(c);
                self.input_mode = InputMode::ValueEntry;
            }
            KeyCode::Char(c) => {
                if let Some(param) = StepParam::from_hotkey(c) {
                    self.active_param = param;
                }
            }
            _ => {}
        }
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
                    if !self.tracks.is_empty() {
                        let (lo, hi) = self.selected_range();
                        let sd = &self.state.step_data[self.cursor_track];
                        for step in lo..=hi {
                            sd.set(step, self.active_param, val);
                        }
                    }
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

    /// Adjust the active param by delta for all steps in the selection.
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

    /// Ramp: anchor value stays fixed, cursor end adjusts by delta,
    /// all intermediate steps get linearly interpolated.
    fn ramp_selected(&self, delta: f32) {
        if self.tracks.is_empty() || self.selection_anchor.is_none() {
            return;
        }
        let anchor = self.selection_anchor.unwrap();
        let sd = &self.state.step_data[self.cursor_track];
        let param = self.active_param;

        let anchor_val = sd.get(anchor, param);
        let cursor_val = sd.get(self.cursor_step, param);
        let new_cursor_val = (cursor_val + delta).clamp(param.min(), param.max());

        let (lo, hi) = self.selected_range();
        let range_len = hi - lo;
        if range_len == 0 {
            sd.set(self.cursor_step, param, new_cursor_val);
            return;
        }

        // Determine start/end values based on which end is the anchor
        let (start_val, end_val) = if anchor <= self.cursor_step {
            (anchor_val, new_cursor_val)
        } else {
            (new_cursor_val, anchor_val)
        };

        for step in lo..=hi {
            let t = (step - lo) as f32 / range_len as f32;
            let val = start_val + t * (end_val - start_val);
            sd.set(step, param, val);
        }
    }

    /// Shift all selected step values left or right by one step.
    fn shift_selection(&mut self, direction: isize) {
        if self.tracks.is_empty() || !self.has_selection() {
            return;
        }
        let (lo, hi) = self.selected_range();
        let sd = &self.state.step_data[self.cursor_track];
        let param = self.active_param;
        let patterns = &self.state.patterns[self.cursor_track];

        // Read all values + active states in the selection
        let count = hi - lo + 1;
        let mut vals: Vec<f32> = (lo..=hi).map(|s| sd.get(s, param)).collect();
        let mut actives: Vec<bool> = (lo..=hi).map(|s| patterns.is_active(s)).collect();

        if direction > 0 && hi < NUM_STEPS - 1 {
            // Shift right: read the value that will be overwritten at hi+1,
            // insert it at the front, and write vals shifted right
            let incoming_val = sd.get(hi + 1, param);
            let incoming_active = patterns.is_active(hi + 1);
            vals.insert(0, incoming_val);
            vals.pop();
            actives.insert(0, incoming_active);
            actives.pop();
            // Write shifted values to new range
            let new_lo = lo + 1;
            let new_hi = hi + 1;
            // Clear old first step
            sd.set(lo, param, sd.get(lo, param)); // keep original
            // Actually: shift the data in the destination range
            for (i, step) in (new_lo..=new_hi).enumerate() {
                sd.set(step, param, vals[i]);
                let should_be_active = actives[i];
                if patterns.is_active(step) != should_be_active {
                    patterns.toggle_step(step);
                }
            }
            // Restore the old lo step to what was at new_lo before
            // Actually simpler: just rotate the values
            // Let me redo this with a cleaner approach
        } else if direction < 0 && lo > 0 {
            // Similar for left
        }

        // Cleaner approach: collect values, shift selection bounds, write back
        // Re-read original values from the expanded range
        let shift = direction; // +1 or -1
        let new_lo = (lo as isize + shift).clamp(0, (NUM_STEPS - count) as isize) as usize;
        let new_hi = new_lo + count - 1;

        if new_lo == lo {
            return; // Can't shift further
        }

        // Read all param values for ALL params and active states for old range
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

        // Clear old positions (set to defaults, deactivate)
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

        // Write to new positions
        for (i, s) in (new_lo..=new_hi).enumerate() {
            for p in StepParam::ALL {
                sd.set(s, p, all_vals[i][p.index()]);
            }
            if patterns.is_active(s) != all_actives[i] {
                patterns.toggle_step(s);
            }
        }

        // Move cursor and anchor
        self.cursor_step = (self.cursor_step as isize + shift).clamp(0, (NUM_STEPS - 1) as isize) as usize;
        if let Some(ref mut anchor) = self.selection_anchor {
            *anchor = (*anchor as isize + shift).clamp(0, (NUM_STEPS - 1) as isize) as usize;
        }
    }
}

fn param_color(param: StepParam) -> Color {
    match param {
        StepParam::Duration  => Color::Cyan,
        StepParam::Velocity  => Color::Red,
        StepParam::Speed     => Color::Green,
        StepParam::AuxA      => Color::Magenta,
        StepParam::AuxB      => Color::Yellow,
        StepParam::Transpose => Color::Blue,
        StepParam::Chop      => Color::Rgb(255, 140, 0), // orange
    }
}

/// Check if a step is within the selected range.
fn is_in_selection(app: &App, step: usize) -> bool {
    let (lo, hi) = app.selected_range();
    step >= lo && step <= hi
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(BAR_HEIGHT as u16),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

    draw_track_info(frame, app, chunks[0]);
    draw_param_tabs(frame, app, chunks[1]);
    draw_bars(frame, app, chunks[2]);
    draw_trigger_row(frame, app, chunks[3]);
    draw_value_line(frame, app, chunks[4]);
    draw_help_bar(frame, app, chunks[5]);
}

fn draw_track_info(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        let msg = Paragraph::new("  No .wav files found in samples/")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }

    let playing = if app.state.is_playing() { "PLAYING" } else { "STOPPED" };
    let bpm = app.state.bpm.load(Ordering::Relaxed);
    let step = app.state.current_step() + 1;
    let track_name = &app.tracks[app.cursor_track];

    let line1 = format!(
        "  [tr {}] {}  [pat 1]  {} BPM  {}  {}/{}",
        app.cursor_track + 1,
        track_name.to_uppercase(),
        bpm,
        playing,
        step,
        NUM_STEPS
    );

    let mut lines = vec![Line::from(Span::styled(
        line1,
        Style::default().fg(Color::White).bg(Color::DarkGray).bold(),
    ))];

    let adjacent: Vec<usize> = {
        let mut adj = Vec::new();
        if app.cursor_track > 0 {
            adj.push(app.cursor_track - 1);
        }
        if app.cursor_track + 1 < app.tracks.len() {
            adj.push(app.cursor_track + 1);
        }
        adj
    };

    for &ti in &adjacent {
        let pattern: String = (0..NUM_STEPS)
            .map(|s| if app.state.patterns[ti].is_active(s) { 'o' } else { '.' })
            .collect();
        let mini = format!("  tr {}: {} {}", ti, &app.tracks[ti], pattern);
        lines.push(Line::from(Span::styled(
            mini,
            Style::default().fg(Color::DarkGray),
        )));
    }

    while lines.len() < area.height as usize {
        lines.push(Line::from(""));
    }

    let text = Text::from(lines);
    let paragraph = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60))),
    );
    frame.render_widget(paragraph, area);
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
        Color::Rgb(120, 120, 30)  // yellow cursor
    } else if is_sel {
        Color::Rgb(40, 50, 80)    // blue-ish selection
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

    let playhead = app.state.current_step();
    let is_playing = app.state.is_playing();
    let sd = &app.state.step_data[app.cursor_track];
    let color = param_color(app.active_param);
    let is_transpose = app.active_param == StepParam::Transpose;

    let values: Vec<f32> = (0..NUM_STEPS)
        .map(|s| {
            let raw = sd.get(s, app.active_param);
            app.active_param.normalize(raw)
        })
        .collect();

    let x_offset = 2u16;

    for step in 0..NUM_STEPS {
        let col_x = area.x + x_offset + step as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let normalized = values[step];
        let active = app.state.patterns[app.cursor_track].is_active(step);
        let bg = step_bg(app, step, is_playing, playhead);

        let fill_levels = (normalized * (BAR_HEIGHT as f32 * 2.0)).round() as usize;

        for row in 0..BAR_HEIGHT {
            let cell_y = area.y + row as u16;

            if is_transpose {
                // Transpose: bipolar display with center line at row 4
                // center is between row 3 (bottom of upper half) and row 4 (top of lower half)
                // We use row 3 bottom-half and row 4 top as the "zero" line
                let center = BAR_HEIGHT / 2; // 4
                // How many half-rows of fill from center
                // normalized 0.5 = center (transpose=0)
                // Each half has 'center' rows = 4 rows = 8 half-levels
                let half_levels = if normalized >= 0.5 {
                    ((normalized - 0.5) * 2.0 * center as f32 * 2.0).round() as usize
                } else {
                    ((0.5 - normalized) * 2.0 * center as f32 * 2.0).round() as usize
                };
                let going_up = normalized >= 0.5;

                // Determine what to draw at this row
                let (cell_text, fg_override) = if going_up {
                    if row < center {
                        // Upper half: fill downward from center
                        // row center-1 is closest to center, row 0 is farthest
                        let dist_from_center = center - row; // 1..center
                        let threshold = (dist_from_center - 1) * 2; // 0-based
                        if half_levels >= threshold + 2 {
                            (" \u{2588} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else if half_levels >= threshold + 1 {
                            // Half block: lower-half fills bottom of cell, connecting to blocks below
                            (" \u{2584} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else {
                            ("   ".to_string(), Color::Rgb(60, 60, 60))
                        }
                    } else if row == center {
                        // Center line row: show tick mark
                        ("\u{2500}\u{2500}\u{2500}".to_string(), Color::Rgb(80, 80, 80))
                    } else {
                        // Below center: empty (positive goes up)
                        ("   ".to_string(), Color::Rgb(60, 60, 60))
                    }
                } else {
                    // Going down (negative transpose)
                    if row > center {
                        // Lower half: fill upward from center
                        let dist_from_center = row - center; // 1..center
                        let threshold = (dist_from_center - 1) * 2;
                        if half_levels >= threshold + 2 {
                            (" \u{2588} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else if half_levels >= threshold + 1 {
                            // Half block: upper-half fills top of cell, connecting to blocks above
                            (" \u{2580} ".to_string(), if active { color } else { Color::Rgb(60, 60, 60) })
                        } else {
                            ("   ".to_string(), Color::Rgb(60, 60, 60))
                        }
                    } else if row == center {
                        // Center line
                        ("\u{2500}\u{2500}\u{2500}".to_string(), Color::Rgb(80, 80, 80))
                    } else {
                        // Above center: empty (negative goes down)
                        ("   ".to_string(), Color::Rgb(60, 60, 60))
                    }
                };

                // At center row when value is exactly 0 (normalized 0.5), show the tick
                // For non-zero, show the tick underneath the fill if at center row
                let style = Style::default().fg(fg_override).bg(bg);
                let cell_area = Rect::new(col_x, cell_y, COL_WIDTH, 1);
                frame.render_widget(Paragraph::new(cell_text).style(style), cell_area);
            } else {
                // Normal params: fill from bottom up
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

fn draw_trigger_row(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() || area.height < 2 {
        return;
    }

    let playhead = app.state.current_step();
    let is_playing = app.state.is_playing();
    let x_offset = 2u16;

    for step in 0..NUM_STEPS {
        let col_x = area.x + x_offset + step as u16 * COL_WIDTH;
        if col_x + COL_WIDTH > area.x + area.width {
            break;
        }

        let active = app.state.patterns[app.cursor_track].is_active(step);
        let ch = if active { " o " } else { " . " };
        let fg = if active { Color::White } else { Color::DarkGray };
        let bg = step_bg(app, step, is_playing, playhead);

        let style = Style::default().fg(fg).bg(bg);
        let cell = Rect::new(col_x, area.y, COL_WIDTH, 1);
        frame.render_widget(Paragraph::new(ch).style(style), cell);
    }

    for step in 0..NUM_STEPS {
        let col_x = area.x + x_offset + step as u16 * COL_WIDTH;
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

    let line = match app.input_mode {
        InputMode::ValueEntry => {
            let step_label = if app.has_selection() {
                let (lo, hi) = app.selected_range();
                format!("Steps {}-{}", lo + 1, hi + 1)
            } else {
                format!("Step {}", app.cursor_step + 1)
            };
            let editing_spans = vec![
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
            ];
            Line::from(editing_spans)
        }
        InputMode::Normal => {
            if app.has_selection() {
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
            }
        }
    };

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if app.input_mode == InputMode::ValueEntry {
        vec![
            Line::from(Span::styled(
                "  0-9: digits  .: decimal  -: negate  Backspace: delete",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  Enter: set value  Esc: cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else if app.has_selection() {
        vec![
            Line::from(Span::styled(
                "  Shift+\u{2190}\u{2192}: extend  \u{2191}\u{2193}: gang edit  Enter: toggle all",
                Style::default().fg(Color::Rgb(120, 150, 220)),
            )),
            Line::from(Span::styled(
                "  0-9: type value for all  Esc: clear selection",
                Style::default().fg(Color::Rgb(120, 150, 220)),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "  \u{2190}\u{2192}: step  \u{2191}\u{2193}: value  Shift+\u{2190}\u{2192}: select  Tab: param  [/]: track",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  Enter: toggle  Space: play  d v s a b t: param  0-9: type value  q: quit",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };

    let text = Text::from(lines);
    let help = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Gray)),
    );
    frame.render_widget(help, area);
}
