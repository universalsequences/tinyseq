use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::draw::region_border_style;
use super::{App, BrowserState, InputMode, PresetPromptKind, Region, SidebarMode, UiState};
use crate::lisp_effect;

// ── Sample Browser tree ──

pub struct BrowserEntry {
    pub depth: usize,
    pub is_dir: bool,
    pub name: String,
    pub path: std::path::PathBuf,
    pub expanded: bool,
}

pub struct BrowserNode {
    pub name: String,
    pub path: std::path::PathBuf,
    pub is_dir: bool,
    pub children: Vec<BrowserNode>,
    pub expanded: bool,
}

impl BrowserNode {
    /// Recursively scan a directory, including only dirs that contain .wav descendants and .wav files.
    pub fn scan_root(root: &str) -> Vec<BrowserNode> {
        let root_path = std::path::Path::new(root);
        if !root_path.is_dir() {
            return Vec::new();
        }
        Self::scan_dir(root_path)
    }

    fn scan_dir(dir: &std::path::Path) -> Vec<BrowserNode> {
        let mut nodes = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return nodes,
        };

        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                let children = Self::scan_dir(&path);
                if !children.is_empty() {
                    nodes.push(BrowserNode {
                        name,
                        path,
                        is_dir: true,
                        children,
                        expanded: false,
                    });
                }
            } else if path
                .extension()
                .map(|ext| ext.to_ascii_lowercase() == "wav")
                .unwrap_or(false)
            {
                nodes.push(BrowserNode {
                    name,
                    path,
                    is_dir: false,
                    children: Vec::new(),
                    expanded: false,
                });
            }
        }
        nodes
    }

    /// Flatten the tree respecting expanded/collapsed state.
    pub fn flatten_visible(nodes: &[BrowserNode], depth: usize) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            result.push(BrowserEntry {
                depth,
                is_dir: node.is_dir,
                name: node.name.clone(),
                path: node.path.clone(),
                expanded: node.expanded,
            });
            if node.is_dir && node.expanded {
                result.extend(Self::flatten_visible(&node.children, depth + 1));
            }
        }
        result
    }

    /// Flatten with search filter — show matching .wav files with their ancestor context (auto-expanded).
    /// Matches against both file names and folder names. When a folder name matches,
    /// all its descendants are included.
    pub fn flatten_filtered(
        nodes: &[BrowserNode],
        filter_lower: &str,
        depth: usize,
    ) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            if node.is_dir {
                let dir_matches = node.name.to_lowercase().contains(filter_lower);
                let child_results = if dir_matches {
                    // Folder name matches — include all children
                    Self::flatten_all(&node.children, depth + 1)
                } else {
                    Self::flatten_filtered(&node.children, filter_lower, depth + 1)
                };
                if !child_results.is_empty() {
                    result.push(BrowserEntry {
                        depth,
                        is_dir: true,
                        name: node.name.clone(),
                        path: node.path.clone(),
                        expanded: true,
                    });
                    result.extend(child_results);
                }
            } else if node.name.to_lowercase().contains(filter_lower) {
                result.push(BrowserEntry {
                    depth,
                    is_dir: false,
                    name: node.name.clone(),
                    path: node.path.clone(),
                    expanded: false,
                });
            }
        }
        result
    }

    /// Flatten all descendants (used when a parent folder matches the filter).
    fn flatten_all(nodes: &[BrowserNode], depth: usize) -> Vec<BrowserEntry> {
        let mut result = Vec::new();
        for node in nodes {
            result.push(BrowserEntry {
                depth,
                is_dir: node.is_dir,
                name: node.name.clone(),
                path: node.path.clone(),
                expanded: node.is_dir,
            });
            if node.is_dir {
                result.extend(Self::flatten_all(&node.children, depth + 1));
            }
        }
        result
    }

    /// Toggle expanded state for a node at a given path in the tree.
    pub fn toggle_expanded(nodes: &mut [BrowserNode], target_path: &std::path::Path) {
        for node in nodes.iter_mut() {
            if node.path == target_path && node.is_dir {
                node.expanded = !node.expanded;
                return;
            }
            if node.is_dir && node.expanded {
                Self::toggle_expanded(&mut node.children, target_path);
            }
        }
    }

    /// Set expanded state for a node.
    pub fn set_expanded(nodes: &mut [BrowserNode], target_path: &std::path::Path, expanded: bool) {
        for node in nodes.iter_mut() {
            if node.path == target_path && node.is_dir {
                node.expanded = expanded;
                return;
            }
            if node.is_dir {
                Self::set_expanded(&mut node.children, target_path, expanded);
            }
        }
    }

    /// Expand all ancestor directories of a target file path. Returns true if found.
    pub fn expand_to_file(nodes: &mut [BrowserNode], target_stem: &str) -> bool {
        for node in nodes.iter_mut() {
            if node.is_dir {
                if Self::expand_to_file(&mut node.children, target_stem) {
                    node.expanded = true;
                    return true;
                }
            } else {
                let stem = node.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem == target_stem {
                    return true;
                }
            }
        }
        false
    }
}

impl BrowserState {
    pub(super) fn visible_items(&self) -> Vec<BrowserEntry> {
        if self.filter.is_empty() {
            BrowserNode::flatten_visible(&self.tree, 0)
        } else {
            let filter_lower = self.filter.to_lowercase();
            BrowserNode::flatten_filtered(&self.tree, &filter_lower, 0)
        }
    }

    fn max_visible(&self, ui: &UiState) -> usize {
        let h = ui.layout.sidebar_inner.height as usize;
        if h > 1 {
            h - 1
        } else {
            1
        }
    }

    pub(super) fn sync_to_track(
        &mut self,
        tracks: &[String],
        cursor_track: usize,
        is_sampler_track: bool,
        ui: &UiState,
    ) {
        if tracks.is_empty() || !is_sampler_track {
            return;
        }
        let sample_name = &tracks[cursor_track];
        if sample_name.is_empty() {
            return;
        }

        self.filter.clear();
        BrowserNode::expand_to_file(&mut self.tree, sample_name);

        let items = self.visible_items();
        for (i, entry) in items.iter().enumerate() {
            if !entry.is_dir {
                let stem = entry
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if stem == sample_name {
                    self.cursor = i;
                    let max_visible = self.max_visible(ui);
                    self.scroll_offset = i.saturating_sub(max_visible / 2);
                    return;
                }
            }
        }
    }

    pub(super) fn handle_sidebar_input(app: &mut App, code: KeyCode) {
        // Instrument picker mode: separate input handling
        if app.effective_sidebar_mode() == SidebarMode::InstrumentPicker {
            Self::handle_instrument_picker_input(app, code);
            return;
        }

        if app.effective_sidebar_mode() == SidebarMode::Presets {
            app.clamp_preset_browser();
            match code {
                KeyCode::Char(c) => {
                    app.preset_browser.filter.push(c);
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                }
                KeyCode::Backspace => {
                    app.preset_browser.filter.pop();
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                }
                KeyCode::Up => {
                    if app.preset_browser.cursor > 0 {
                        app.preset_browser.cursor -= 1;
                    }
                    app.clamp_preset_browser();
                }
                KeyCode::Down => {
                    let items = app.visible_preset_items();
                    if app.preset_browser.cursor + 1 < items.len() {
                        app.preset_browser.cursor += 1;
                    }
                    app.clamp_preset_browser();
                }
                KeyCode::Enter => app.load_selected_preset_into_track(),
                KeyCode::Esc => {
                    app.preset_browser.filter.clear();
                    app.preset_browser.cursor = 0;
                    app.preset_browser.scroll_offset = 0;
                    app.ui.focused_region = Region::Cirklon;
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char(c) => {
                app.browser.filter.push(c);
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
            }
            KeyCode::Backspace => {
                app.browser.filter.pop();
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
            }
            KeyCode::Up => {
                if app.browser.cursor > 0 {
                    app.browser.cursor -= 1;
                    if app.browser.cursor < app.browser.scroll_offset {
                        app.browser.scroll_offset = app.browser.cursor;
                    }
                }
            }
            KeyCode::Down => {
                let items = app.browser.visible_items();
                if app.browser.cursor + 1 < items.len() {
                    app.browser.cursor += 1;
                    let max_visible = app.browser.max_visible(&app.ui);
                    if app.browser.cursor >= app.browser.scroll_offset + max_visible {
                        app.browser.scroll_offset = app.browser.cursor + 1 - max_visible;
                    }
                }
            }
            KeyCode::Right => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    if item.is_dir && !item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut app.browser.tree, &path, true);
                    }
                }
            }
            KeyCode::Left => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    if item.is_dir && item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut app.browser.tree, &path, false);
                    }
                }
            }
            KeyCode::Enter => {
                let items = app.browser.visible_items();
                if app.browser.cursor < items.len() {
                    let item = &items[app.browser.cursor];
                    let path = item.path.clone();
                    if item.is_dir {
                        BrowserNode::toggle_expanded(&mut app.browser.tree, &path);
                    } else {
                        app.sidebar_select_file(&path);
                    }
                }
            }
            KeyCode::Esc => {
                app.browser.filter.clear();
                app.browser.cursor = 0;
                app.browser.scroll_offset = 0;
                app.ui.focused_region = Region::Cirklon;
                if !app.tracks.is_empty() {
                    app.ui.sidebar_mode = SidebarMode::Audition;
                }
            }
            _ => {}
        }
    }

    fn handle_instrument_picker_input(app: &mut App, code: KeyCode) {
        use crate::sequencer::InstrumentType;
        match code {
            KeyCode::Up => {
                if app.ui.instrument_picker_cursor > 0 {
                    app.ui.instrument_picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if app.ui.instrument_picker_cursor + 1 < InstrumentType::COUNT {
                    app.ui.instrument_picker_cursor += 1;
                }
            }
            KeyCode::Enter => match InstrumentType::ALL[app.ui.instrument_picker_cursor] {
                InstrumentType::Sampler => {
                    app.browser.cursor = 0;
                    app.browser.filter.clear();
                    app.browser.scroll_offset = 0;
                    app.ui.sidebar_mode = SidebarMode::AddTrack;
                }
                InstrumentType::Custom => {
                    app.editor.picker_cursor = 0;
                    app.editor.picker_filter.clear();
                    app.editor.picker_items = crate::lisp_effect::list_saved_instruments();
                    app.ui.input_mode = super::InputMode::InstrumentPicker;
                }
            },
            KeyCode::Esc => {
                app.ui.focused_region = Region::Cirklon;
                if !app.tracks.is_empty() {
                    app.ui.sidebar_mode = SidebarMode::Audition;
                }
            }
            _ => {}
        }
    }

    pub(super) fn scroll(&mut self, delta: isize, ui: &UiState) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let max_visible = self.max_visible(ui);
        let max_scroll = items.len().saturating_sub(max_visible);
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
        } else {
            self.scroll_offset = (self.scroll_offset + delta as usize).min(max_scroll);
        }
    }
}

impl App {
    fn current_custom_instrument_name(&self) -> Option<&str> {
        if self.tracks.is_empty() || self.is_sampler_track(self.ui.cursor_track) {
            None
        } else if let Some(Some(engine_id)) = self.graph.track_engine_ids.get(self.ui.cursor_track)
        {
            self.editor
                .cached_instruments
                .get(*engine_id)
                .map(|engine| engine.name.as_str())
        } else {
            self.tracks.get(self.ui.cursor_track).map(String::as_str)
        }
    }

    pub(super) fn visible_preset_items(&self) -> Vec<String> {
        let Some(name) = self.current_custom_instrument_name() else {
            return Vec::new();
        };
        let mut items: Vec<String> = lisp_effect::load_instrument_presets(name)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.name)
            .collect();
        items.sort();
        if self.preset_browser.filter.is_empty() {
            return items;
        }
        let filter = self.preset_browser.filter.to_lowercase();
        items.retain(|item| item.to_lowercase().contains(&filter));
        items
    }

    pub(super) fn current_preset_engine_name(&self) -> Option<&str> {
        self.current_custom_instrument_name()
    }

    fn current_track_preset_meta(&self) -> crate::sequencer::TrackPresetMeta {
        self.state
            .track_preset_meta
            .lock()
            .unwrap()
            .get(self.ui.cursor_track)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn set_track_preset_meta(
        &self,
        track: usize,
        loaded_preset: Option<String>,
        dirty: bool,
    ) {
        if let Some(meta) = self.state.track_preset_meta.lock().unwrap().get_mut(track) {
            meta.loaded_preset = loaded_preset;
            meta.dirty = dirty;
        }
    }

    pub(super) fn mark_track_sound_dirty(&self, track: usize) {
        if let Some(meta) = self.state.track_preset_meta.lock().unwrap().get_mut(track) {
            meta.dirty = true;
        }
    }

    pub(super) fn clamp_preset_browser(&mut self) {
        let items = self.visible_preset_items();
        if items.is_empty() {
            self.preset_browser.cursor = 0;
            self.preset_browser.scroll_offset = 0;
            return;
        }
        self.preset_browser.cursor = self.preset_browser.cursor.min(items.len() - 1);
        let max_visible = self.preset_max_visible();
        let max_scroll = items.len().saturating_sub(max_visible);
        self.preset_browser.scroll_offset = self.preset_browser.scroll_offset.min(max_scroll);
        if self.preset_browser.cursor < self.preset_browser.scroll_offset {
            self.preset_browser.scroll_offset = self.preset_browser.cursor;
        } else if self.preset_browser.cursor >= self.preset_browser.scroll_offset + max_visible {
            self.preset_browser.scroll_offset = self.preset_browser.cursor + 1 - max_visible;
        }
    }

    pub(super) fn preset_max_visible(&self) -> usize {
        let h = self.ui.layout.sidebar_inner.height as usize;
        if h > 3 {
            h - 3
        } else {
            1
        }
    }

    fn selected_preset_name(&self) -> Option<String> {
        let items = self.visible_preset_items();
        items.get(self.preset_browser.cursor).cloned()
    }

    pub(super) fn load_selected_preset_into_track(&mut self) {
        let Some(instrument_name) = self.current_custom_instrument_name() else {
            return;
        };
        let Some(selected_name) = self.selected_preset_name() else {
            return;
        };
        let presets = match lisp_effect::load_instrument_presets(instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let Some(preset) = presets.into_iter().find(|p| p.name == selected_name) else {
            return;
        };
        let track = self.ui.cursor_track;
        let Some(desc) = self.current_instrument_descriptor() else {
            return;
        };
        let slot = &self.state.instrument_slots[track];
        for (idx, param) in desc.params.iter().enumerate() {
            let value = preset
                .params
                .get(&param.name)
                .copied()
                .unwrap_or(param.default);
            let clamped = param.clamp(value);
            slot.defaults.set(idx, clamped);
            self.send_instrument_param(track, idx, clamped);
        }
        self.state.instrument_base_note_offsets[track].store(
            preset.base_note_offset.to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.set_track_preset_meta(track, Some(preset.name.clone()), false);
        self.editor.status_message =
            Some((format!("Loaded preset '{}'", preset.name), Instant::now()));
    }

    fn save_current_track_as_preset(&mut self, preset_name: &str, overwrite: bool) {
        let Some(instrument_name) = self.current_custom_instrument_name() else {
            return;
        };
        let track = self.ui.cursor_track;
        let Some(desc) = self.current_instrument_descriptor() else {
            return;
        };
        let slot = &self.state.instrument_slots[track];
        let mut params = std::collections::BTreeMap::new();
        for (idx, param) in desc.params.iter().enumerate() {
            params.insert(param.name.clone(), slot.defaults.get(idx));
        }
        let preset = lisp_effect::InstrumentPreset {
            id: preset_name.to_string(),
            name: preset_name.to_string(),
            base_note_offset: f32::from_bits(
                self.state.instrument_base_note_offsets[track]
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
            params,
        };

        let mut presets = match lisp_effect::load_instrument_presets(instrument_name) {
            Ok(p) => p,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };

        if let Some(existing_idx) = presets.iter().position(|p| p.name == preset_name) {
            if overwrite {
                presets[existing_idx] = preset;
            } else {
                self.editor.status_message = Some((
                    format!("Preset '{}' already exists", preset_name),
                    Instant::now(),
                ));
                return;
            }
        } else {
            presets.push(preset);
            presets.sort_by(|a, b| a.name.cmp(&b.name));
        }

        match lisp_effect::save_instrument_presets(instrument_name, &presets) {
            Ok(()) => {
                self.set_track_preset_meta(track, Some(preset_name.to_string()), false);
                self.editor.status_message =
                    Some((format!("Saved preset '{}'", preset_name), Instant::now()));
                self.clamp_preset_browser();
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
            }
        }
    }

    pub(super) fn overwrite_loaded_preset(&mut self) {
        let meta = self.current_track_preset_meta();
        let Some(name) = meta.loaded_preset else {
            self.editor.status_message =
                Some(("No loaded preset to overwrite".to_string(), Instant::now()));
            return;
        };
        self.save_current_track_as_preset(&name, true);
    }

    pub(super) fn revert_loaded_preset(&mut self) {
        let meta = self.current_track_preset_meta();
        if let Some(name) = meta.loaded_preset {
            let items = self.visible_preset_items();
            if let Some(idx) = items.iter().position(|item| item == &name) {
                self.preset_browser.cursor = idx;
            }
            self.load_selected_preset_into_track();
        } else {
            self.editor.status_message =
                Some(("No loaded preset to revert".to_string(), Instant::now()));
        }
    }

    pub(super) fn handle_preset_name_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.ui.value_buffer.push(c),
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
            }
            KeyCode::Enter => {
                let name = self.ui.value_buffer.trim().to_string();
                if !name.is_empty() {
                    match self.ui.preset_prompt_kind {
                        PresetPromptKind::SaveNew => {
                            self.save_current_track_as_preset(&name, false)
                        }
                    }
                }
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    /// Execute the sidebar action for a file selection (Enter or click).
    pub(super) fn sidebar_select_file(&mut self, path: &std::path::Path) {
        match self.effective_sidebar_mode() {
            SidebarMode::InstrumentPicker => return, // no file selection in picker
            SidebarMode::AddTrack => match self.graph_controller().add_track(path) {
                Ok(idx) => {
                    self.ui.cursor_track = idx;
                    self.editor.status_message =
                        Some((format!("Added track {}", idx + 1), Instant::now()));
                }
                Err(e) => {
                    self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
                }
            },
            SidebarMode::Audition => {
                if self.tracks.is_empty() || self.ui.cursor_track >= self.tracks.len() {
                    return;
                }
                match crate::sampler::load_wav_buffer(self.graph.lg.0, path) {
                    Ok((new_buffer_id, new_name)) => {
                        let track = self.ui.cursor_track;
                        self.graph_controller()
                            .send_buffer_to_all_voices(track, new_buffer_id);
                        self.graph.track_buffer_ids[track] = new_buffer_id;
                        self.tracks[track] = new_name.clone();
                        self.editor.status_message =
                            Some((format!("Swapped: {}", new_name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
            SidebarMode::Presets => {}
        }
    }
}

// ── Drawing ──

pub(super) fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.ui.focused_region == Region::Sidebar;

    let title = match app.effective_sidebar_mode() {
        SidebarMode::InstrumentPicker => " Instrument ",
        SidebarMode::AddTrack => " + Add Track ",
        SidebarMode::Audition => " \u{266b} Audition ",
        SidebarMode::Presets => " Presets ",
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(region_border_style(app, Region::Sidebar));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.ui.layout.sidebar_inner = inner;

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // Clear the entire inner area first to prevent stale content
    let buf = frame.buffer_mut();
    for y in inner.y..(inner.y + inner.height) {
        for x in inner.x..(inner.x + inner.width) {
            buf[(x, y)].reset();
        }
    }

    // Instrument picker mode: draw simple list instead of browser
    if app.effective_sidebar_mode() == SidebarMode::InstrumentPicker && focused {
        for (i, inst) in crate::sequencer::InstrumentType::ALL.iter().enumerate() {
            let label = inst.label();
            if i as u16 >= inner.height {
                break;
            }
            let is_cursor = i == app.ui.instrument_picker_cursor;
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            let text = format!("  {} ", label);
            let buf = frame.buffer_mut();
            buf.set_string(inner.x, inner.y + i as u16, &text, style);
            let text_width = UnicodeWidthStr::width(text.as_str());
            let remaining = (inner.width as usize).saturating_sub(text_width);
            if remaining > 0 {
                buf.set_string(
                    inner.x + text_width as u16,
                    inner.y + i as u16,
                    &" ".repeat(remaining),
                    style,
                );
            }
        }
        return;
    }

    if app.effective_sidebar_mode() == SidebarMode::Presets {
        app.clamp_preset_browser();
        let items = app.visible_preset_items();
        let max_visible = (inner.height as usize).saturating_sub(3);
        let meta = app.current_track_preset_meta();
        let engine_name = app.current_preset_engine_name().unwrap_or("None");
        let engine_header = format!(" engine: {}", engine_name);
        let loaded = meta
            .loaded_preset
            .clone()
            .unwrap_or_else(|| "None".to_string());
        let header = format!(" preset: {}{}", loaded, if meta.dirty { " *" } else { "" });
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                engine_header,
                Style::default().fg(Color::Cyan),
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                header,
                Style::default().fg(Color::White),
            ))),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        if focused {
            let filter_text = format!("> {}\u{2588}", app.preset_browser.filter);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    filter_text,
                    Style::default().fg(Color::White),
                ))),
                Rect::new(inner.x, inner.y + 2, inner.width, 1),
            );
        }
        let list_start_y = inner.y + 3;
        let scroll = app.preset_browser.scroll_offset;
        for (vi, i) in (scroll..items.len()).enumerate() {
            if vi >= max_visible {
                break;
            }
            let row_y = list_start_y + vi as u16;
            if row_y >= inner.y + inner.height {
                break;
            }
            let item = &items[i];
            let is_cursor = focused && i == app.preset_browser.cursor;
            let is_loaded = meta
                .loaded_preset
                .as_ref()
                .map(|p| p == item)
                .unwrap_or(false);
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else if is_loaded {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            };
            let text = format!("  {}", item);
            let buf = frame.buffer_mut();
            buf.set_string(inner.x, row_y, &text, style);
            let text_width = UnicodeWidthStr::width(text.as_str());
            let remaining = (inner.width as usize).saturating_sub(text_width);
            if remaining > 0 {
                buf.set_string(
                    inner.x + text_width as u16,
                    row_y,
                    &" ".repeat(remaining),
                    style,
                );
            }
        }
        return;
    }

    let items = app.browser.visible_items();
    let max_visible = (inner.height as usize).saturating_sub(1); // 1 row for filter

    // Filter input line (only when focused)
    if focused {
        let filter_text = format!("> {}\u{2588}", app.browser.filter);
        let filter_line = Line::from(Span::styled(filter_text, Style::default().fg(Color::White)));
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(Paragraph::new(filter_line), filter_area);
    }

    let list_start_y = if focused { inner.y + 1 } else { inner.y };
    let list_max = if focused {
        max_visible
    } else {
        inner.height as usize
    };
    let scroll = app.browser.scroll_offset;

    for (vi, i) in (scroll..items.len()).enumerate() {
        if vi >= list_max {
            break;
        }
        let row_y = list_start_y + vi as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let entry = &items[i];
        let is_cursor = focused && i == app.browser.cursor;
        let is_current_sample = !entry.is_dir
            && !app.tracks.is_empty()
            && entry
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == app.tracks[app.ui.cursor_track])
                .unwrap_or(false);

        let indent = "  ".repeat(entry.depth);
        let icon = if entry.is_dir {
            if entry.expanded {
                "\u{25bc} "
            } else {
                "\u{25b6} "
            }
        } else {
            "  "
        };

        let prefix_width = UnicodeWidthStr::width(indent.as_str()) + UnicodeWidthStr::width(icon);
        let max_name_width = (inner.width as usize).saturating_sub(prefix_width);
        // Truncate name by display width
        let mut truncated = String::new();
        let mut w = 0;
        for ch in entry.name.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w + cw > max_name_width {
                break;
            }
            truncated.push(ch);
            w += cw;
        }
        let text = format!("{}{}{}", indent, icon, truncated);

        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if is_current_sample {
            Style::default().fg(Color::Yellow)
        } else if entry.is_dir {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };

        // Write directly to the buffer for guaranteed cell coverage
        let buf = frame.buffer_mut();
        buf.set_string(inner.x, row_y, &text, style);
        // Fill remaining cells with spaces in the same style
        let text_width = UnicodeWidthStr::width(text.as_str());
        let remaining = (inner.width as usize).saturating_sub(text_width);
        if remaining > 0 {
            buf.set_string(
                inner.x + text_width as u16,
                row_y,
                &" ".repeat(remaining),
                style,
            );
        }
    }
}
