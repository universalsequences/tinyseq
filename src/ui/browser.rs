use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::draw::region_border_style;
use super::{App, Region, SidebarMode};

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

// ── App impl: browser/sidebar methods ──

impl App {
    /// Sync the sidebar to show the current track's sample: expand its folder and scroll to it.
    pub(super) fn sync_sidebar_to_track(&mut self) {
        if self.tracks.is_empty() {
            return;
        }
        let sample_name = &self.tracks[self.cursor_track];
        if sample_name.is_empty() {
            return;
        }

        // Clear filter so we see the tree view
        self.browser_filter.clear();

        // Expand ancestor folders to reveal the sample
        BrowserNode::expand_to_file(&mut self.browser_tree, sample_name);

        // Find the sample in the flattened list and set cursor + scroll
        let items = self.browser_visible_items();
        for (i, entry) in items.iter().enumerate() {
            if !entry.is_dir {
                let stem = entry.path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem == sample_name {
                    self.browser_cursor = i;
                    // Center the item in the visible area
                    let max_visible = self.sidebar_max_visible();
                    if i >= max_visible / 2 {
                        self.browser_scroll_offset = i - max_visible / 2;
                    } else {
                        self.browser_scroll_offset = 0;
                    }
                    return;
                }
            }
        }
    }

    pub(super) fn browser_visible_items(&self) -> Vec<BrowserEntry> {
        if self.browser_filter.is_empty() {
            BrowserNode::flatten_visible(&self.browser_tree, 0)
        } else {
            let filter_lower = self.browser_filter.to_lowercase();
            BrowserNode::flatten_filtered(&self.browser_tree, &filter_lower, 0)
        }
    }

    pub(super) fn handle_sidebar_input(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.browser_filter.push(c);
                self.browser_cursor = 0;
                self.browser_scroll_offset = 0;
            }
            KeyCode::Backspace => {
                self.browser_filter.pop();
                self.browser_cursor = 0;
                self.browser_scroll_offset = 0;
            }
            KeyCode::Up => {
                if self.browser_cursor > 0 {
                    self.browser_cursor -= 1;
                    if self.browser_cursor < self.browser_scroll_offset {
                        self.browser_scroll_offset = self.browser_cursor;
                    }
                }
            }
            KeyCode::Down => {
                let items = self.browser_visible_items();
                if self.browser_cursor + 1 < items.len() {
                    self.browser_cursor += 1;
                    let max_visible = self.sidebar_max_visible();
                    if self.browser_cursor >= self.browser_scroll_offset + max_visible {
                        self.browser_scroll_offset = self.browser_cursor + 1 - max_visible;
                    }
                }
            }
            KeyCode::Right => {
                let items = self.browser_visible_items();
                if self.browser_cursor < items.len() {
                    let item = &items[self.browser_cursor];
                    if item.is_dir && !item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut self.browser_tree, &path, true);
                    }
                }
            }
            KeyCode::Left => {
                let items = self.browser_visible_items();
                if self.browser_cursor < items.len() {
                    let item = &items[self.browser_cursor];
                    if item.is_dir && item.expanded {
                        let path = item.path.clone();
                        BrowserNode::set_expanded(&mut self.browser_tree, &path, false);
                    }
                }
            }
            KeyCode::Enter => {
                let items = self.browser_visible_items();
                if self.browser_cursor < items.len() {
                    let item = &items[self.browser_cursor];
                    let path = item.path.clone();
                    if item.is_dir {
                        BrowserNode::toggle_expanded(&mut self.browser_tree, &path);
                    } else {
                        self.sidebar_select_file(&path);
                    }
                }
            }
            KeyCode::Esc => {
                self.browser_filter.clear();
                self.browser_cursor = 0;
                self.browser_scroll_offset = 0;
                self.focused_region = Region::Cirklon;
                if !self.tracks.is_empty() {
                    self.sidebar_mode = SidebarMode::Audition;
                }
            }
            _ => {}
        }
    }

    /// Execute the sidebar action for a file selection (Enter or click).
    pub(super) fn sidebar_select_file(&mut self, path: &std::path::Path) {
        match self.sidebar_mode {
            SidebarMode::AddTrack => {
                match self.add_track(path) {
                    Ok(idx) => {
                        self.cursor_track = idx;
                        self.status_message =
                            Some((format!("Added track {}", idx + 1), Instant::now()));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
            SidebarMode::Audition => {
                if self.tracks.is_empty() || self.cursor_track >= self.tracks.len() {
                    return;
                }
                match crate::sampler::load_wav_buffer(self.lg.0, path) {
                    Ok((new_buffer_id, new_name)) => {
                        let track = self.cursor_track;
                        self.send_buffer_to_all_voices(track, new_buffer_id);
                        self.track_buffer_ids[track] = new_buffer_id;
                        self.tracks[track] = new_name.clone();
                        self.status_message =
                            Some((format!("Swapped: {}", new_name), Instant::now()));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
        }
    }

    /// Max visible items based on sidebar_inner height.
    fn sidebar_max_visible(&self) -> usize {
        let h = self.layout.sidebar_inner.height as usize;
        if h > 1 { h - 1 } else { 1 } // subtract 1 for filter line
    }

    /// Scroll the sidebar browser list by delta (positive = down, negative = up).
    pub(super) fn sidebar_scroll(&mut self, delta: isize) {
        let items = self.browser_visible_items();
        if items.is_empty() {
            return;
        }
        let max_visible = self.sidebar_max_visible();
        let max_scroll = items.len().saturating_sub(max_visible);

        if delta < 0 {
            self.browser_scroll_offset = self.browser_scroll_offset.saturating_sub((-delta) as usize);
        } else {
            self.browser_scroll_offset = (self.browser_scroll_offset + delta as usize).min(max_scroll);
        }
    }
}

// ── Drawing ──

pub(super) fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focused_region == Region::Sidebar;

    let title = if !focused {
        " Samples "
    } else {
        match app.sidebar_mode {
            SidebarMode::AddTrack => " + Add Track ",
            SidebarMode::Audition => " \u{266b} Audition ",
        }
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(region_border_style(app, Region::Sidebar));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.layout.sidebar_inner = inner;

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

    let items = app.browser_visible_items();
    let max_visible = (inner.height as usize).saturating_sub(1); // 1 row for filter

    // Filter input line (only when focused)
    if focused {
        let filter_text = format!("> {}\u{2588}", app.browser_filter);
        let filter_line = Line::from(Span::styled(filter_text, Style::default().fg(Color::White)));
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(Paragraph::new(filter_line), filter_area);
    }

    let list_start_y = if focused { inner.y + 1 } else { inner.y };
    let list_max = if focused { max_visible } else { inner.height as usize };
    let scroll = app.browser_scroll_offset;

    for (vi, i) in (scroll..items.len()).enumerate() {
        if vi >= list_max {
            break;
        }
        let row_y = list_start_y + vi as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let entry = &items[i];
        let is_cursor = focused && i == app.browser_cursor;
        let is_current_sample = !entry.is_dir
            && !app.tracks.is_empty()
            && entry
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == app.tracks[app.cursor_track])
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
