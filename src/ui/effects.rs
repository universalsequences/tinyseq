use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, EffectSlotState, ParamKind, SyncDivision, BUILTIN_SLOT_COUNT,
};
use crate::lisp_effect::{self, MAX_CUSTOM_FX};
use crate::reverb;
use crate::sequencer::InstrumentType;

use super::params::draw_dropdown;
use super::{App, CompileTarget, EffectTab, InputMode, PendingCompile, PendingEditor, Region};

const SYNTH_TWO_COLUMN_MIN_WIDTH: u16 = 88;
const SYNTH_COLUMN_GAP: u16 = 2;

fn fit_cell(text: &str, width: usize) -> String {
    let clipped: String = text.chars().take(width).collect();
    format!("{clipped:<width$}")
}

// ── App impl: effect methods ──

impl App {
    fn cached_instrument_engine_idx(&self, name: &str, source: &str) -> Option<usize> {
        self.editor
            .cached_instruments
            .iter()
            .position(|entry| entry.name == name && entry.source == source)
    }

    fn cache_instrument_engine(
        &mut self,
        name: &str,
        source: &str,
        manifest: &lisp_effect::DGenManifest,
        lib: lisp_effect::LoadedDGenLib,
    ) -> usize {
        let lib_index = self.editor.instrument_libs.len();
        self.editor.instrument_libs.push(lib);
        let entry = super::CachedInstrumentEngine {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest.clone(),
            lib_index,
        };
        if let Some(existing_idx) = self.cached_instrument_engine_idx(name, source) {
            self.editor.cached_instruments[existing_idx] = entry;
            existing_idx
        } else {
            self.editor.cached_instruments.push(entry);
            self.editor.cached_instruments.len() - 1
        }
    }

    fn try_add_cached_instrument_track(&mut self, name: &str, source: &str) -> bool {
        let Some(cache_idx) = self.cached_instrument_engine_idx(name, source) else {
            return false;
        };
        let manifest = self.editor.cached_instruments[cache_idx].manifest.clone();
        let lib_index = self.editor.cached_instruments[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        } {
            Ok(idx) => {
                self.ui.cursor_track = idx;
                self.ui.sidebar_mode = super::SidebarMode::Presets;
                self.ui.focused_region = super::Region::Cirklon;
                self.editor.status_message = Some((
                    format!("Added synth track '{}' (cached)", name),
                    Instant::now(),
                ));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
        true
    }

    fn instrument_base_note_offset(&self, track: usize) -> f32 {
        f32::from_bits(self.state.instrument_base_note_offsets[track].load(Ordering::Relaxed))
    }

    fn set_instrument_base_note_offset(&self, track: usize, value: f32) {
        self.state.instrument_base_note_offsets[track].store(value.to_bits(), Ordering::Relaxed);
        self.mark_track_sound_dirty(track);
    }

    pub(super) fn synth_row_count(&self) -> usize {
        self.current_instrument_descriptor()
            .map(|d| d.params.len() + 1)
            .unwrap_or(1)
    }

    pub(super) fn synth_column_count(&self, area: Rect) -> usize {
        if area.height == 0 {
            return 1;
        }
        if area.width >= SYNTH_TWO_COLUMN_MIN_WIDTH && self.synth_row_count() > area.height as usize
        {
            2
        } else {
            1
        }
    }

    fn synth_rows_per_column(&self, area: Rect) -> usize {
        area.height as usize
    }

    pub(super) fn synth_visible_capacity(&self, area: Rect) -> usize {
        self.synth_rows_per_column(area) * self.synth_column_count(area)
    }

    fn clamp_synth_scroll(&mut self, area: Rect) {
        let visible = self.synth_visible_capacity(area);
        if visible == 0 {
            self.ui.synth_scroll_offset = 0;
            return;
        }
        let max_scroll = self.synth_row_count().saturating_sub(visible);
        self.ui.synth_scroll_offset = self.ui.synth_scroll_offset.min(max_scroll);
    }

    pub(super) fn ensure_synth_cursor_visible(&mut self) {
        let area = self.ui.layout.effects_inner;
        let visible = self.synth_visible_capacity(area);
        if visible == 0 {
            self.ui.synth_scroll_offset = 0;
            return;
        }

        let max_cursor = self.synth_row_count().saturating_sub(1);
        self.ui.instrument_param_cursor = self.ui.instrument_param_cursor.min(max_cursor);
        self.clamp_synth_scroll(area);

        if self.ui.instrument_param_cursor < self.ui.synth_scroll_offset {
            self.ui.synth_scroll_offset = self.ui.instrument_param_cursor;
        } else if self.ui.instrument_param_cursor >= self.ui.synth_scroll_offset + visible {
            self.ui.synth_scroll_offset = self.ui.instrument_param_cursor + 1 - visible;
        }

        self.clamp_synth_scroll(area);
    }

    pub(super) fn synth_row_at_position(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        if area.height == 0
            || col < area.x
            || col >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }

        let columns = self.synth_column_count(area);
        let rows_per_column = self.synth_rows_per_column(area);
        if rows_per_column == 0 {
            return None;
        }

        let column_width = if columns == 1 {
            area.width
        } else {
            area.width.saturating_sub(SYNTH_COLUMN_GAP) / 2
        };
        if column_width == 0 {
            return None;
        }

        let rel_x = col - area.x;
        let column = if columns == 1 {
            0
        } else if rel_x < column_width {
            0
        } else if rel_x >= column_width + SYNTH_COLUMN_GAP
            && rel_x < (column_width * 2) + SYNTH_COLUMN_GAP
        {
            1
        } else {
            return None;
        };

        let rel_y = (row - area.y) as usize;
        let absolute = self.ui.synth_scroll_offset + column * rows_per_column + rel_y;
        (absolute < self.synth_row_count()).then_some(absolute)
    }

    /// Get the current slot's descriptor, if available.
    pub(super) fn current_slot_descriptor(&self) -> Option<&EffectDescriptor> {
        if self.tracks.is_empty() {
            return None;
        }
        let slot_idx = self.selected_effect_slot()?;
        self.graph
            .effect_descriptors
            .get(self.ui.cursor_track)
            .and_then(|descs| descs.get(slot_idx))
    }

    /// Get the current slot's runtime state, if available.
    pub(super) fn current_slot(&self) -> Option<&EffectSlotState> {
        if self.tracks.is_empty() {
            return None;
        }
        let slot_idx = self.selected_effect_slot()?;
        self.state
            .effect_chains
            .get(self.ui.cursor_track)
            .and_then(|chain| chain.get(slot_idx))
    }

    /// Indices of visible (non-empty) effect slots for the current track.
    pub(super) fn visible_effect_indices(&self) -> Vec<usize> {
        if self.tracks.is_empty() {
            return Vec::new();
        }
        let track = self.ui.cursor_track;
        let descs = &self.graph.effect_descriptors[track];
        let mut visible = Vec::new();
        for i in 0..descs.len() {
            if i < BUILTIN_SLOT_COUNT {
                visible.push(i); // Always show built-in
            } else if !descs[i].name.is_empty() {
                visible.push(i); // Show loaded custom (by descriptor, not pattern-local node_id)
            }
        }
        visible
    }

    /// Find the first free custom slot index for the current track, or None.
    fn next_free_custom_slot(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let chain = &self.state.effect_chains[self.ui.cursor_track];
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
        self.graph.track_node_ids[track].voice_sum_id
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
        self.graph.track_node_ids[track].filter_id
    }

    /// Whether there are fewer than MAX_CUSTOM_FX loaded custom effects (can add more).
    pub(super) fn can_add_custom_effect(&self) -> bool {
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
        self.graph.effect_descriptors[track][slot_idx] = desc;

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

    fn run_effect_editor(&mut self, slot_idx: usize, existing_name: Option<String>) {
        if self.tracks.is_empty() {
            return;
        }
        let track = self.ui.cursor_track;
        let (slot_id, predecessor_id, successor_id, existing) =
            self.resolve_custom_slot_wiring(track, slot_idx);

        // Load source: from file (if editing existing) or empty for new
        let last_source = match &existing_name {
            Some(name) => lisp_effect::load_effect_source(name).unwrap_or_default(),
            None => String::new(),
        };
        let track_name = self.tracks[track].clone();

        let result = lisp_effect::run_editor_flow(
            self.graph.lg.0,
            slot_id,
            &track_name,
            predecessor_id,
            successor_id,
            existing,
            &last_source,
            existing_name.as_deref(),
            self.graph.sample_rate,
        );

        if let Some(r) = result {
            self.apply_effect_to_slot(track, slot_idx, r.node_id, &r.name, &r.params);
            self.ui.effect_tab = EffectTab::Slot(slot_idx);
            self.ui.effect_param_cursor = 0;
            self.ui.focused_region = Region::Params;
            self.ui.params_column = 1;
            self.editor.lisp_libs.push(r.lib);
        }
    }

    /// Spawn a background thread to compile an effect, storing a PendingCompile.
    fn start_effect_compile(&mut self, name: &str, slot_idx: usize) {
        let source = match lisp_effect::load_effect_source(name) {
            Ok(s) => s,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.graph.sample_rate;
        std::thread::spawn(move || {
            let result = lisp_effect::compile_and_load(&source, sample_rate);
            let _ = tx.send(result);
        });
        self.editor.pending_compile = Some(PendingCompile {
            receiver: rx,
            target: CompileTarget::Effect {
                name: name.to_string(),
                slot_idx,
                track: self.ui.cursor_track,
            },
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
                self.graph.lg.0,
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
                self.editor.lisp_libs.push(result.lib);
                self.ui.effect_tab = EffectTab::Slot(slot_idx);
                self.ui.effect_param_cursor = 0;
                self.ui.focused_region = Region::Params;
                self.ui.params_column = 1;
                self.editor.status_message = Some((format!("Loaded '{}'", name), Instant::now()));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    /// Apply a compiled instrument result: add a custom track.
    pub(super) fn apply_compiled_instrument(
        &mut self,
        result: lisp_effect::CompileResult,
        name: &str,
    ) {
        let source = lisp_effect::load_instrument_source(name).unwrap_or_default();
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.cached_instruments[cache_idx].manifest.clone();
        let lib_index = self.editor.cached_instruments[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        match unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        } {
            Ok(idx) => {
                self.ui.cursor_track = idx;
                self.ui.sidebar_mode = super::SidebarMode::Presets;
                self.ui.focused_region = super::Region::Cirklon;
                self.editor.status_message =
                    Some((format!("Added synth track '{}'", name), Instant::now()));
            }
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {}", e), Instant::now()));
            }
        }
    }

    fn run_instrument_editor(&mut self, existing_name: Option<String>) {
        let last_source = match &existing_name {
            Some(name) => lisp_effect::load_instrument_source(name).unwrap_or_default(),
            None => String::new(),
        };

        let result = lisp_effect::run_instrument_editor_flow(
            &last_source,
            existing_name.as_deref(),
            self.graph.sample_rate,
        );

        if let Some(r) = result {
            // Hot-reload if editing an existing custom track
            let is_existing_custom = self.ui.cursor_track < self.graph.track_instrument_types.len()
                && self.graph.track_instrument_types[self.ui.cursor_track]
                    == InstrumentType::Custom;

            if is_existing_custom {
                let track = self.ui.cursor_track;
                let cache_idx =
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib);
                let manifest = self.editor.cached_instruments[cache_idx].manifest.clone();
                let lib_index = self.editor.cached_instruments[cache_idx].lib_index;
                let lib_ptr: *const lisp_effect::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller()
                        .hot_reload_instrument(track, &manifest, &*lib_ptr)
                } {
                    Ok(()) => {
                        self.tracks[self.ui.cursor_track] = r.name.clone();
                        if track < self.graph.track_engine_ids.len() {
                            self.graph.track_engine_ids[track] = Some(cache_idx);
                        }
                        self.editor.status_message =
                            Some((format!("Reloaded instrument '{}'", r.name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            } else {
                let cache_idx =
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib);
                let manifest = self.editor.cached_instruments[cache_idx].manifest.clone();
                let lib_index = self.editor.cached_instruments[cache_idx].lib_index;
                let lib_ptr: *const lisp_effect::LoadedDGenLib =
                    &self.editor.instrument_libs[lib_index];
                match unsafe {
                    self.graph_controller()
                        .add_custom_track(&r.name, cache_idx, &manifest, &*lib_ptr)
                } {
                    Ok(idx) => {
                        self.ui.cursor_track = idx;
                        self.ui.sidebar_mode = super::SidebarMode::Presets;
                        self.ui.focused_region = super::Region::Cirklon;
                        self.editor.status_message =
                            Some((format!("Added synth track '{}'", r.name), Instant::now()));
                    }
                    Err(e) => {
                        self.editor.status_message =
                            Some((format!("Error: {}", e), Instant::now()));
                    }
                }
            }
        }
    }

    pub fn has_pending_editor(&self) -> bool {
        self.editor.pending_editor.is_some()
    }

    pub fn run_pending_editor(&mut self) {
        let Some(action) = self.editor.pending_editor.take() else {
            return;
        };

        match action {
            PendingEditor::Effect { slot_idx, name } => self.run_effect_editor(slot_idx, name),
            PendingEditor::Instrument { name } => self.run_instrument_editor(name),
        }
    }

    fn filtered_picker_items(&self) -> Vec<String> {
        let mut items = vec!["+ New effect".to_string()];
        let filter_lower = self.editor.picker_filter.to_lowercase();
        for name in &self.editor.picker_items {
            if filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower) {
                items.push(name.clone());
            }
        }
        items
    }

    pub(super) fn handle_effect_picker(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.editor.picker_filter.push(c);
                self.editor.picker_cursor = 0;
            }
            KeyCode::Backspace => {
                self.editor.picker_filter.pop();
                self.editor.picker_cursor = 0;
            }
            KeyCode::Up => {
                if self.editor.picker_cursor > 0 {
                    self.editor.picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.filtered_picker_items().len();
                if self.editor.picker_cursor + 1 < max {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_picker_items();
                if self.editor.picker_cursor < items.len() {
                    let selected = &items[self.editor.picker_cursor];
                    if selected == "+ New effect" {
                        // Open editor for new effect
                        if let Some(slot_idx) = self.next_free_custom_slot() {
                            self.editor.pending_editor = Some(PendingEditor::Effect {
                                slot_idx,
                                name: None,
                            });
                        }
                    } else {
                        // Load saved effect from picker (async)
                        let name = selected.clone();
                        if let Some(slot_idx) = self.next_free_custom_slot() {
                            self.start_effect_compile(&name, slot_idx);
                        }
                    }
                }
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    /// Handle keyboard input for the instrument picker overlay.
    pub(super) fn handle_instrument_picker_overlay(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.editor.picker_filter.push(c);
                self.editor.picker_cursor = 0;
            }
            KeyCode::Backspace => {
                self.editor.picker_filter.pop();
                self.editor.picker_cursor = 0;
            }
            KeyCode::Up => {
                if self.editor.picker_cursor > 0 {
                    self.editor.picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                let max = self.filtered_instrument_items().len();
                if self.editor.picker_cursor + 1 < max {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_instrument_items();
                if self.editor.picker_cursor < items.len() {
                    let selected = &items[self.editor.picker_cursor];
                    if selected == "+ New instrument" {
                        self.editor.pending_editor = Some(PendingEditor::Instrument { name: None });
                    } else {
                        // Load saved instrument (compile in background, add track)
                        let name = selected.clone();
                        self.start_instrument_compile(&name);
                    }
                }
                self.ui.input_mode = super::InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.input_mode = super::InputMode::Normal;
                if !self.tracks.is_empty() {
                    self.ui.sidebar_mode = super::SidebarMode::Audition;
                    self.ui.focused_region = super::Region::Cirklon;
                }
            }
            _ => {}
        }
    }

    pub(super) fn filtered_instrument_items(&self) -> Vec<String> {
        let mut items = vec!["+ New instrument".to_string()];
        let filter_lower = self.editor.picker_filter.to_lowercase();
        for name in &self.editor.picker_items {
            if filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower) {
                items.push(name.clone());
            }
        }
        items
    }

    /// Spawn a background thread to compile an instrument.
    fn start_instrument_compile(&mut self, name: &str) {
        let source = match lisp_effect::load_instrument_source(name) {
            Ok(s) => s,
            Err(e) => {
                self.editor.status_message = Some((format!("Error: {e}"), Instant::now()));
                return;
            }
        };
        if self.try_add_cached_instrument_track(name, &source) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let sample_rate = self.graph.sample_rate;
        std::thread::spawn(move || {
            let result = lisp_effect::compile_and_load_instrument(&source, sample_rate);
            let _ = tx.send(result);
        });
        self.editor.pending_compile = Some(PendingCompile {
            receiver: rx,
            target: CompileTarget::Instrument {
                name: name.to_string(),
            },
            tick: 0,
        });
    }

    pub(super) fn handle_effects_column(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Synth tab input handling
        if self.ui.effect_tab == EffectTab::Synth {
            let shift = modifiers.contains(KeyModifiers::SHIFT);
            match code {
                KeyCode::Left => {
                    self.ui.params_column = 0; // Go to track params column
                }
                KeyCode::Right => {
                    let visible = self.visible_effect_indices();
                    if let Some(&first) = visible.first() {
                        self.ui.effect_tab = EffectTab::Slot(first);
                        self.ui.effect_param_cursor = 0;
                    } else {
                        self.ui.effect_tab = EffectTab::Reverb;
                        self.ui.reverb_param_cursor = 0;
                    }
                }
                KeyCode::Up => {
                    if shift {
                        if self.ui.instrument_param_cursor == 0 {
                            let next = (self.instrument_base_note_offset(self.ui.cursor_track)
                                + 1.0)
                                .clamp(-48.0, 48.0);
                            self.set_instrument_base_note_offset(self.ui.cursor_track, next);
                        } else {
                            self.adjust_instrument_param(1.0);
                        }
                    } else if self.ui.instrument_param_cursor > 0 {
                        self.ui.instrument_param_cursor -= 1;
                        self.ensure_synth_cursor_visible();
                    }
                }
                KeyCode::Down => {
                    if shift {
                        if self.ui.instrument_param_cursor == 0 {
                            let next = (self.instrument_base_note_offset(self.ui.cursor_track)
                                - 1.0)
                                .clamp(-48.0, 48.0);
                            self.set_instrument_base_note_offset(self.ui.cursor_track, next);
                        } else {
                            self.adjust_instrument_param(-1.0);
                        }
                    } else {
                        let max = self.synth_row_count().saturating_sub(1);
                        if self.ui.instrument_param_cursor < max {
                            self.ui.instrument_param_cursor += 1;
                            self.ensure_synth_cursor_visible();
                        }
                    }
                }
                KeyCode::Enter => {
                    if self.ui.instrument_param_cursor == 0 {
                        self.ui.value_buffer.clear();
                        self.ui.input_mode = InputMode::ValueEntry;
                    } else if let Some(desc) = self.current_instrument_descriptor() {
                        let param_idx = self.ui.instrument_param_cursor - 1;
                        if param_idx < desc.params.len() {
                            let param = &desc.params[param_idx];
                            if param.is_boolean() {
                                self.toggle_instrument_boolean();
                            } else if param.is_enum() {
                                self.ui.dropdown_open = true;
                                self.ui.dropdown_cursor = 0;
                                self.ui.input_mode = InputMode::Dropdown;
                                let slot = &self.state.instrument_slots[self.ui.cursor_track];
                                let val = slot.defaults.get(param_idx);
                                self.ui.dropdown_cursor = val.round() as usize;
                            }
                        }
                    }
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                    if self.ui.instrument_param_cursor == 0 {
                        self.ui.value_buffer.clear();
                        self.ui.value_buffer.push(c);
                        self.ui.input_mode = InputMode::ValueEntry;
                    } else if let Some(desc) = self.current_instrument_descriptor() {
                        let param_idx = self.ui.instrument_param_cursor - 1;
                        if param_idx < desc.params.len() {
                            let param = &desc.params[param_idx];
                            if !param.is_boolean() {
                                self.ui.value_buffer.clear();
                                self.ui.value_buffer.push(c);
                                self.ui.input_mode = InputMode::ValueEntry;
                            }
                        }
                    }
                }
                KeyCode::Char('[') => {
                    let ns = self.num_steps();
                    self.ui.cursor_step = if self.ui.cursor_step == 0 {
                        ns - 1
                    } else {
                        self.ui.cursor_step - 1
                    };
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                KeyCode::Char(']') => {
                    let ns = self.num_steps();
                    self.ui.cursor_step = if self.ui.cursor_step + 1 >= ns {
                        0
                    } else {
                        self.ui.cursor_step + 1
                    };
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                _ => {}
            }
            return;
        }

        if self.ui.effect_tab == EffectTab::Reverb {
            let shift = modifiers.contains(KeyModifiers::SHIFT);
            match code {
                KeyCode::Left => {
                    let visible = self.visible_effect_indices();
                    if let Some(&last) = visible.last() {
                        self.ui.effect_tab = EffectTab::Slot(last);
                        self.ui.effect_param_cursor = 0;
                    } else if self.is_current_custom_track() {
                        self.ui.effect_tab = EffectTab::Synth;
                        self.ui.instrument_param_cursor = 0;
                        self.ui.synth_scroll_offset = 0;
                    } else {
                        self.ui.params_column = 0;
                    }
                }
                KeyCode::Right => {} // Already at rightmost tab
                KeyCode::Up => {
                    if shift {
                        self.adjust_reverb_param(0.05);
                    } else if self.ui.reverb_param_cursor > 0 {
                        self.ui.reverb_param_cursor -= 1;
                    }
                }
                KeyCode::Down => {
                    if shift {
                        self.adjust_reverb_param(-0.05);
                    } else if self.ui.reverb_param_cursor < 2 {
                        self.ui.reverb_param_cursor += 1;
                    }
                }
                KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                    self.ui.value_buffer.clear();
                    self.ui.value_buffer.push(c);
                    self.ui.input_mode = InputMode::ValueEntry;
                }
                KeyCode::Char('[') => {
                    let ns = self.num_steps();
                    self.ui.cursor_step = if self.ui.cursor_step == 0 {
                        ns - 1
                    } else {
                        self.ui.cursor_step - 1
                    };
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                KeyCode::Char(']') => {
                    let ns = self.num_steps();
                    self.ui.cursor_step = if self.ui.cursor_step + 1 >= ns {
                        0
                    } else {
                        self.ui.cursor_step + 1
                    };
                    self.ui.selection_anchor = Some(self.ui.cursor_step);
                }
                _ => {}
            }
            return;
        }

        let visible = self.visible_effect_indices();
        let shift = modifiers.contains(KeyModifiers::SHIFT);

        match code {
            KeyCode::Left => {
                if let Some(pos) = visible
                    .iter()
                    .position(|&i| self.ui.effect_tab == EffectTab::Slot(i))
                {
                    if pos > 0 {
                        self.ui.effect_tab = EffectTab::Slot(visible[pos - 1]);
                        self.ui.effect_param_cursor = 0;
                    } else if self.is_current_custom_track() {
                        self.ui.effect_tab = EffectTab::Synth;
                        self.ui.instrument_param_cursor = 0;
                        self.ui.synth_scroll_offset = 0;
                    } else {
                        self.ui.params_column = 0;
                    }
                } else if self.is_current_custom_track() {
                    self.ui.effect_tab = EffectTab::Synth;
                    self.ui.instrument_param_cursor = 0;
                    self.ui.synth_scroll_offset = 0;
                } else {
                    self.ui.params_column = 0;
                }
            }
            KeyCode::Right => {
                if let Some(pos) = visible
                    .iter()
                    .position(|&i| self.ui.effect_tab == EffectTab::Slot(i))
                {
                    if pos + 1 < visible.len() {
                        self.ui.effect_tab = EffectTab::Slot(visible[pos + 1]);
                        self.ui.effect_param_cursor = 0;
                    } else {
                        // Move to reverb tab
                        self.ui.effect_tab = EffectTab::Reverb;
                        self.ui.reverb_param_cursor = 0;
                    }
                } else {
                    // Move to reverb tab
                    self.ui.effect_tab = EffectTab::Reverb;
                    self.ui.reverb_param_cursor = 0;
                }
            }
            KeyCode::Up => {
                if shift {
                    self.adjust_slot_param(1.0);
                } else if self.ui.effect_param_cursor > 0 {
                    self.ui.effect_param_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if shift {
                    self.adjust_slot_param(-1.0);
                } else if let Some(desc) = self.current_slot_descriptor() {
                    let max = desc.params.len().saturating_sub(1);
                    if self.ui.effect_param_cursor < max {
                        self.ui.effect_param_cursor += 1;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.ui.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.ui.effect_param_cursor];
                        if param.is_boolean() {
                            self.toggle_slot_boolean();
                            self.update_delay_time_param_kind();
                        } else if param.is_enum() {
                            self.ui.dropdown_open = true;
                            self.ui.dropdown_cursor = 0;
                            self.ui.input_mode = InputMode::Dropdown;
                            let val = self.get_current_slot_value();
                            self.ui.dropdown_cursor = val.round() as usize;
                        }
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                if let Some(desc) = self.current_slot_descriptor() {
                    if self.ui.effect_param_cursor < desc.params.len() {
                        let param = &desc.params[self.ui.effect_param_cursor];
                        if !param.is_boolean() {
                            self.ui.value_buffer.clear();
                            self.ui.value_buffer.push(c);
                            self.ui.input_mode = InputMode::ValueEntry;
                        }
                    }
                }
            }
            KeyCode::Char('[') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step == 0 {
                    ns - 1
                } else {
                    self.ui.cursor_step - 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            KeyCode::Char(']') => {
                let ns = self.num_steps();
                self.ui.cursor_step = if self.ui.cursor_step + 1 >= ns {
                    0
                } else {
                    self.ui.cursor_step + 1
                };
                self.ui.selection_anchor = Some(self.ui.cursor_step);
            }
            _ => {}
        }
    }

    pub(super) fn set_reverb_param(&mut self, cursor: usize, value: f32) {
        let clamped = value.clamp(0.0, 1.0);
        let param_idx = match cursor {
            0 => {
                self.ui.reverb_size = clamped;
                reverb::REVERB_PARAM_SIZE
            }
            1 => {
                self.ui.reverb_brightness = clamped;
                reverb::REVERB_PARAM_BRIGHT
            }
            2 => {
                self.ui.reverb_replace = clamped;
                reverb::REVERB_PARAM_REPLACE
            }
            _ => return,
        };
        unsafe {
            crate::audiograph::params_push_wrapper(
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx: param_idx,
                    logical_id: self.graph.reverb_node_id as u64,
                    fvalue: clamped,
                },
            );
        }
    }

    fn adjust_reverb_param(&mut self, delta: f32) {
        let current = match self.ui.reverb_param_cursor {
            0 => self.ui.reverb_size,
            1 => self.ui.reverb_brightness,
            2 => self.ui.reverb_replace,
            _ => return,
        };
        self.set_reverb_param(self.ui.reverb_param_cursor, current + delta);
    }

    fn adjust_slot_param(&self, direction: f32) {
        let track = self.ui.cursor_track;
        let Some(slot_idx) = self.selected_effect_slot() else {
            return;
        };
        let param_idx = self.ui.effect_param_cursor;

        let desc = match self
            .graph
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
                self.graph.lg.0,
                crate::audiograph::ParamMsg {
                    idx,
                    logical_id: node_id as u64,
                    fvalue: value,
                },
            );
        }
    }

    fn toggle_slot_boolean(&self) {
        let param_idx = self.ui.effect_param_cursor;

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

        if self.ui.effect_tab != EffectTab::Slot(DELAY_SLOT)
            || self.ui.effect_param_cursor != SYNCED_PARAM
        {
            return;
        }

        let track = self.ui.cursor_track;
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
            .graph
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
            Some(slot) => slot.defaults.get(self.ui.effect_param_cursor),
            None => 0.0,
        }
    }

    /// Whether the current track is a custom instrument (has Synth tab).
    pub(super) fn is_current_custom_track(&self) -> bool {
        !self.is_sampler_track(self.ui.cursor_track)
    }

    /// Get the instrument descriptor for the current track, if it's a custom instrument.
    pub(super) fn current_instrument_descriptor(&self) -> Option<&EffectDescriptor> {
        if !self.is_current_custom_track() {
            return None;
        }
        self.graph.instrument_descriptors.get(self.ui.cursor_track)
    }

    /// Adjust an instrument param (Synth tab) by direction (+1 or -1).
    fn adjust_instrument_param(&self, direction: f32) {
        let track = self.ui.cursor_track;
        if self.ui.instrument_param_cursor == 0 {
            return;
        }
        let param_idx = self.ui.instrument_param_cursor - 1;

        let desc = match self.graph.instrument_descriptors.get(track) {
            Some(d) => d,
            None => return,
        };
        if param_idx >= desc.params.len() {
            return;
        }
        let param_desc = &desc.params[param_idx];
        let slot = &self.state.instrument_slots[track];

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
            self.send_instrument_param(track, param_idx, new_val);
            self.mark_track_sound_dirty(track);
        }
    }

    /// Send an instrument param value to ALL synth nodes for a track.
    pub(super) fn send_instrument_param(&self, track: usize, param_idx: usize, value: f32) {
        let slot = &self.state.instrument_slots[track];
        let idx = slot.resolve_node_idx(param_idx);
        let Some(engine_id) = self.graph.track_engine_ids.get(track).and_then(|id| *id) else {
            return;
        };
        let engine_track_uses = self
            .graph
            .track_engine_ids
            .iter()
            .filter(|bound| **bound == Some(engine_id))
            .count();
        if engine_track_uses > 1 {
            return;
        }
        let synth_ids = self
            .graph
            .engine_node_ids
            .get(engine_id)
            .and_then(|engine| engine.as_ref().map(|engine| &engine.synth_ids));
        if let Some(synth_ids) = synth_ids {
            for &synth_id in synth_ids {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx,
                            logical_id: synth_id as u64,
                            fvalue: value,
                        },
                    );
                }
            }
        }
    }

    pub(super) fn push_instrument_defaults_for_track(&self, track: usize) {
        let slot = &self.state.instrument_slots[track];
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..num_params {
            self.send_instrument_param(track, param_idx, slot.defaults.get(param_idx));
        }
    }

    pub(super) fn push_all_restored_instrument_defaults(&self) {
        for track in 0..self.tracks.len() {
            if self.is_sampler_track(track) {
                continue;
            }
            self.push_instrument_defaults_for_track(track);
        }
    }

    /// Toggle a boolean instrument param (Synth tab).
    fn toggle_instrument_boolean(&self) {
        let track = self.ui.cursor_track;
        if self.ui.instrument_param_cursor == 0 {
            return;
        }
        let param_idx = self.ui.instrument_param_cursor - 1;
        let slot = &self.state.instrument_slots[track];

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
            self.send_instrument_param(track, param_idx, new_val);
            self.mark_track_sound_dirty(track);
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
    let col_focused = region_focused && app.ui.params_column == 1;
    let border_style = if col_focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Rgb(60, 60, 60))
    };

    // Build title with slot tabs — only show non-empty slots + [+] button
    let visible = app.visible_effect_indices();
    let mut title_spans = vec![];

    // Synth tab (only for custom instrument tracks, shown first)
    if app.is_current_custom_track() {
        let synth_selected = app.ui.effect_tab == EffectTab::Synth;
        let synth_style = if synth_selected && col_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(100, 200, 140))
                .bold()
        } else if synth_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(60, 120, 80))
        } else {
            Style::default().fg(Color::Rgb(60, 120, 80))
        };
        let synth_label = if synth_selected {
            "[< Synth >]"
        } else {
            "[  Synth  ]"
        };
        title_spans.push(Span::styled(synth_label, synth_style));
        title_spans.push(Span::raw(" "));
    }

    if let Some(descs) = app.graph.effect_descriptors.get(app.ui.cursor_track) {
        for &i in &visible {
            if i >= descs.len() {
                continue;
            }
            let desc = &descs[i];
            let is_selected = app.ui.effect_tab == EffectTab::Slot(i);
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
        let reverb_selected = app.ui.effect_tab == EffectTab::Reverb;
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

    app.ui.layout.effects_block = area;
    app.ui.layout.effects_inner = inner;

    if app.tracks.is_empty() || inner.height < 1 {
        return;
    }

    // Synth tab rendering
    if app.ui.effect_tab == EffectTab::Synth {
        app.ensure_synth_cursor_visible();
        let desc = match app.graph.instrument_descriptors.get(app.ui.cursor_track) {
            Some(d) if !d.params.is_empty() => d,
            _ => return,
        };
        let slot = &app.state.instrument_slots[app.ui.cursor_track];
        let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;
        let total_rows = app.synth_row_count();
        let columns = app.synth_column_count(inner);
        let rows_per_column = app.synth_rows_per_column(inner);
        let visible_capacity = app.synth_visible_capacity(inner);
        let column_width = if columns == 1 {
            inner.width
        } else {
            inner.width.saturating_sub(SYNTH_COLUMN_GAP) / 2
        };

        for visible_idx in 0..visible_capacity {
            let row_idx = app.ui.synth_scroll_offset + visible_idx;
            if row_idx >= total_rows {
                break;
            }

            let column = visible_idx / rows_per_column;
            let local_row = visible_idx % rows_per_column;
            let row_y = inner.y + local_row as u16;
            let row_x = inner.x + column as u16 * (column_width + SYNTH_COLUMN_GAP);
            let row_area = Rect::new(row_x, row_y, column_width, 1);

            let is_base_row = row_idx == 0;
            let param_idx = row_idx.saturating_sub(1);
            let param_desc = if is_base_row {
                None
            } else {
                Some(&desc.params[param_idx])
            };
            let default_val = if is_base_row {
                app.instrument_base_note_offset(app.ui.cursor_track)
            } else {
                slot.defaults.get(param_idx)
            };
            let is_cursor_row = col_focused && app.ui.instrument_param_cursor == row_idx;
            let cursor = if is_cursor_row { "> " } else { "  " };
            let cursor_style = if is_cursor_row {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Rgb(100, 200, 140))
            };

            let label_width = if column_width >= 44 { 14 } else { 11 };
            let value_width = if column_width >= 40 { 12 } else { 9 };
            let slider_width =
                (column_width as usize).saturating_sub(label_width + value_width + 6);
            let label = fit_cell(
                if is_base_row {
                    "base_note"
                } else {
                    &param_desc.unwrap().name
                },
                label_width,
            );

            // Value entry mode
            if is_cursor_row && is_entering_value {
                let target_label = if !app.ui.visual_steps.is_empty() {
                    format!("{} steps", app.ui.visual_steps.len())
                } else if app.ui.selection_anchor.is_some() {
                    let (lo, hi) = app.selected_range();
                    format!("steps {}-{}", lo + 1, hi + 1)
                } else {
                    "default".to_string()
                };
                let spans = vec![
                    Span::styled(cursor, cursor_style),
                    Span::styled(label.clone(), Style::default().fg(Color::Gray)),
                    Span::styled(
                        format!("{}\u{2588}", app.ui.value_buffer),
                        Style::default()
                            .fg(Color::Yellow)
                            .bg(Color::Rgb(60, 60, 20))
                            .bold(),
                    ),
                    Span::styled(
                        format!("  ({target_label})"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                let line = Line::from(spans);
                frame.render_widget(Paragraph::new(line), row_area);
                continue;
            }

            // Determine display value and p-lock status
            let (display_val, plock_label) = if !is_base_row && app.has_selection() && is_cursor_row
            {
                let plock_val = slot.plocks.get(app.ui.cursor_step, param_idx);
                match plock_val {
                    Some(v) => (v, Some(" (p-lock)")),
                    None => (default_val, None),
                }
            } else {
                (default_val, None)
            };

            let formatted = if is_base_row {
                format!("{:.0} st", display_val)
            } else {
                param_desc.unwrap().format_value(display_val)
            };

            let mut spans = vec![
                Span::styled(cursor, cursor_style),
                Span::styled(label, Style::default().fg(Color::Gray)),
                Span::styled(fit_cell(&formatted, value_width), cursor_style),
            ];

            if let Some(lbl) = plock_label {
                spans.push(Span::styled(lbl, Style::default().fg(Color::White)));
            }

            // Slider
            if is_base_row || !param_desc.unwrap().is_boolean() {
                let range = if is_base_row {
                    96.0
                } else {
                    param_desc.unwrap().max - param_desc.unwrap().min
                };
                if range > 0.0 {
                    let (slider_val, is_plock) = if is_base_row {
                        (default_val, false)
                    } else if app.has_selection() {
                        let pv = slot.plocks.get(app.ui.cursor_step, param_idx);
                        (pv.unwrap_or(default_val), pv.is_some())
                    } else if app.state.is_playing() {
                        let step = app.state.track_step(app.ui.cursor_track);
                        let pv = slot.plocks.get(step, param_idx);
                        (pv.unwrap_or(default_val), pv.is_some())
                    } else {
                        (default_val, false)
                    };
                    let norm = if is_base_row {
                        ((slider_val + 48.0) / 96.0).clamp(0.0, 1.0)
                    } else {
                        param_desc.unwrap().normalize(slider_val)
                    };
                    if slider_width > 2 {
                        let filled =
                            ((norm * slider_width as f32).round() as usize).min(slider_width);
                        let bar: String =
                            "\u{2550}".repeat(filled) + &" ".repeat(slider_width - filled);
                        let slider_color = if is_plock {
                            Color::Cyan
                        } else {
                            Color::Rgb(100, 200, 140)
                        };
                        spans.push(Span::styled(
                            format!("[{}]", bar),
                            Style::default().fg(slider_color),
                        ));
                    }
                }
            }

            let line = Line::from(spans);
            frame.render_widget(Paragraph::new(line), row_area);
        }

        if total_rows > visible_capacity && inner.width > 12 {
            let end = (app.ui.synth_scroll_offset + visible_capacity).min(total_rows);
            let summary = format!("{}-{}/{}", app.ui.synth_scroll_offset + 1, end, total_rows);
            let summary_len = summary.len() as u16;
            let x = inner.x + inner.width.saturating_sub(summary_len);
            frame.render_widget(
                Paragraph::new(summary).style(Style::default().fg(Color::DarkGray)),
                Rect::new(x, inner.y, summary_len, 1),
            );
        }

        // Dropdown overlay for synth tab
        if app.ui.dropdown_open && col_focused {
            draw_dropdown(frame, app, inner);
        }
        return;
    }

    // Reverb tab rendering
    if app.ui.effect_tab == EffectTab::Reverb {
        let reverb_params: [(&str, f32); 3] = [
            ("size", app.ui.reverb_size),
            ("brightness", app.ui.reverb_brightness),
            ("replace", app.ui.reverb_replace),
        ];
        let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;

        for (i, (name, val)) in reverb_params.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let row_y = inner.y + i as u16;
            let is_cursor_row = col_focused && app.ui.reverb_param_cursor == i;
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

    let track = app.ui.cursor_track;
    let Some(slot_idx) = app.selected_effect_slot() else {
        return;
    };

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
        .graph
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
    let is_entering_value = col_focused && app.ui.input_mode == InputMode::ValueEntry;

    for (i, param_desc) in desc.params.iter().enumerate() {
        let row_y = inner.y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let default_val = slot.defaults.get(i);
        let is_cursor_row = col_focused && app.ui.effect_param_cursor == i;
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
            let target_label = if !app.ui.visual_steps.is_empty() {
                format!("p-lock {} steps", app.ui.visual_steps.len())
            } else if app.ui.selection_anchor.is_some() {
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
                    format!("{}\u{2588}", app.ui.value_buffer),
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
            let plock_val = slot.plocks.get(app.ui.cursor_step, i);
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
                    let pv = slot.plocks.get(app.ui.cursor_step, i);
                    (pv.unwrap_or(default_val), pv.is_some())
                } else if app.state.is_playing() {
                    let step = app.state.track_step(app.ui.cursor_track);
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
                    let slider_color = if is_plock { Color::Cyan } else { Color::White };
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
    if app.ui.dropdown_open && col_focused {
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
    let filter_text = format!(" > {}\u{2588}", app.editor.picker_filter);
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
        let is_cursor = i == app.editor.picker_cursor;
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
    let name = match &pending.target {
        CompileTarget::Effect { name, .. } | CompileTarget::Instrument { name } => name,
    };
    let name_display = if name.len() > 14 {
        format!("{}...", &name[..11])
    } else {
        name.clone()
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

pub(super) fn draw_instrument_picker(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.filtered_instrument_items();
    let max_visible = 10usize;
    let list_height = items.len().min(max_visible) as u16;
    let w = 36u16;
    let h = list_height + 4;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let picker_area = Rect::new(x, y, w, h);

    let bg = Style::default().bg(Color::Rgb(20, 20, 20));
    for row in 0..h {
        let row_area = Rect::new(x, y + row, w, 1);
        frame.render_widget(Paragraph::new(" ".repeat(w as usize)).style(bg), row_area);
    }

    let block = Block::default()
        .title(" Instruments ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let inner = block.inner(picker_area);
    frame.render_widget(block, picker_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let filter_text = format!(" > {}\u{2588}", app.editor.picker_filter);
    let filter_line = Line::from(Span::styled(filter_text, Style::default().fg(Color::White)));
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(Paragraph::new(filter_line), filter_area);

    let list_start_y = inner.y + 1;
    for (i, item) in items.iter().enumerate() {
        if i >= max_visible {
            break;
        }
        let row_y = list_start_y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }
        let is_cursor = i == app.editor.picker_cursor;
        let style = if is_cursor {
            Style::default().fg(Color::Black).bg(Color::White)
        } else if item == "+ New instrument" {
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
