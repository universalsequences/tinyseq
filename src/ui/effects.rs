use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::prelude::*;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{
    EffectDescriptor, EffectSlotState, ParamKind, SyncDivision, BUILTIN_SLOT_COUNT,
};
use crate::lisp_effect::{self, MAX_CUSTOM_FX};
use crate::reverb;
use crate::sequencer::InstrumentType;

use super::{
    App, CompileTarget, EffectTab, InputMode, ParamMouseDragTarget, PendingCompile,
    PendingEditor, Region,
};

pub(super) const SYNTH_TWO_COLUMN_MIN_WIDTH: u16 = 88;
pub(super) const SYNTH_COLUMN_GAP: u16 = 2;

#[derive(Clone, Copy)]
pub(super) enum OverlayPickerKind {
    Effect,
    Instrument,
}

// ── App impl: effect methods ──

impl App {
    pub fn add_saved_instrument_track_sync(&mut self, name: &str) -> Result<usize, String> {
        let source = lisp_effect::load_instrument_source(name).map_err(|e| e.to_string())?;

        if let Some(cache_idx) = self.cached_instrument_engine_idx(name, &source) {
            let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
            let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
            let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
            return unsafe {
                self.graph_controller()
                    .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
            };
        }

        let result = lisp_effect::compile_and_load_instrument(&source, self.graph.sample_rate)?;
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        }
    }

    fn cached_instrument_engine_idx(&self, name: &str, source: &str) -> Option<usize> {
        self.editor.engine_registry.find_by_name_and_source(name, source)
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
        let entry = super::EngineDescriptor {
            name: name.to_string(),
            source: source.to_string(),
            manifest: manifest.clone(),
            lib_index,
        };
        self.editor.engine_registry.upsert(entry)
    }

    fn try_add_cached_instrument_track(&mut self, name: &str, source: &str) -> bool {
        let Some(cache_idx) = self.cached_instrument_engine_idx(name, source) else {
            return false;
        };
        let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
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

    pub(super) fn instrument_base_note_offset(&self, track: usize) -> f32 {
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

    pub(super) fn synth_rows_per_column(&self, area: Rect) -> usize {
        area.height as usize
    }

    pub(super) fn synth_visible_capacity(&self, area: Rect) -> usize {
        self.synth_rows_per_column(area) * self.synth_column_count(area)
    }

    pub(super) fn clamp_synth_scroll(&mut self, area: Rect) {
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
        let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
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
                let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
                let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
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
                        if let Some(sound) =
                            self.state.track_sound_state.lock().unwrap().get_mut(track)
                        {
                            sound.engine_id = Some(cache_idx);
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
                let manifest = self.editor.engine_registry.engines[cache_idx].manifest.clone();
                let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
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

    pub(super) fn overlay_new_label(kind: OverlayPickerKind) -> &'static str {
        match kind {
            OverlayPickerKind::Effect => "+ New effect",
            OverlayPickerKind::Instrument => "+ New instrument",
        }
    }

    pub(super) fn filtered_overlay_items(&self, kind: OverlayPickerKind) -> Vec<String> {
        let mut items = vec![Self::overlay_new_label(kind).to_string()];
        let filter_lower = self.editor.picker_filter.to_lowercase();
        for name in &self.editor.picker_items {
            if filter_lower.is_empty() || name.to_lowercase().contains(&filter_lower) {
                items.push(name.clone());
            }
        }
        items
    }

    fn handle_overlay_picker_input(&mut self, kind: OverlayPickerKind, code: KeyCode) {
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
                let max = self.filtered_overlay_items(kind).len();
                if self.editor.picker_cursor + 1 < max {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let items = self.filtered_overlay_items(kind);
                if self.editor.picker_cursor < items.len() {
                    let selected = &items[self.editor.picker_cursor];
                    if selected == Self::overlay_new_label(kind) {
                        match kind {
                            OverlayPickerKind::Effect => {
                                if let Some(slot_idx) = self.next_free_custom_slot() {
                                    self.editor.pending_editor = Some(PendingEditor::Effect {
                                        slot_idx,
                                        name: None,
                                    });
                                }
                            }
                            OverlayPickerKind::Instrument => {
                                self.editor.pending_editor =
                                    Some(PendingEditor::Instrument { name: None });
                            }
                        }
                    } else {
                        let name = selected.clone();
                        match kind {
                            OverlayPickerKind::Effect => {
                                if let Some(slot_idx) = self.next_free_custom_slot() {
                                    self.start_effect_compile(&name, slot_idx);
                                }
                            }
                            OverlayPickerKind::Instrument => {
                                self.start_instrument_compile(&name);
                            }
                        }
                    }
                }
                self.ui.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.ui.input_mode = InputMode::Normal;
                if matches!(kind, OverlayPickerKind::Instrument) && !self.tracks.is_empty() {
                    self.ui.sidebar_mode = super::SidebarMode::Audition;
                    self.ui.focused_region = super::Region::Cirklon;
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_effect_picker(&mut self, code: KeyCode) {
        self.handle_overlay_picker_input(OverlayPickerKind::Effect, code);
    }

    pub(super) fn handle_instrument_picker_overlay(&mut self, code: KeyCode) {
        self.handle_overlay_picker_input(OverlayPickerKind::Instrument, code);
    }

    pub(super) fn instrument_usage_count(&self, instrument_name: &str) -> usize {
        self.graph
            .track_engine_ids
            .iter()
            .filter_map(|engine_id| {
                engine_id.and_then(|id| self.editor.engine_registry.engines.get(id))
            })
            .filter(|engine| engine.name == instrument_name)
            .count()
    }

    pub(super) fn instrument_picker_label(&self, instrument_name: &str) -> String {
        let usage_count = self.instrument_usage_count(instrument_name);
        if usage_count == 0 {
            instrument_name.to_string()
        } else {
            format!("{instrument_name}  [in use x{usage_count}]")
        }
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

    pub(super) fn synth_row_display_value(&self, track: usize, row_idx: usize) -> Option<f32> {
        if row_idx == 0 {
            return Some(self.instrument_base_note_offset(track));
        }

        let param_idx = row_idx.checked_sub(1)?;
        let desc = self.graph.instrument_descriptors.get(track)?;
        let param_desc = desc.params.get(param_idx)?;
        let stored = self.state.instrument_slots[track].defaults.get(param_idx);
        Some(param_desc.stored_to_user(stored))
    }

    fn scrub_param_display_value(
        &self,
        param_desc: &crate::effects::ParamDescriptor,
        start_display_value: f32,
        dx: i32,
    ) -> f32 {
        let display_min = param_desc.stored_to_user(param_desc.min);
        let display_max = param_desc.stored_to_user(param_desc.max);
        let display_range = (display_max - display_min).abs();
        match &param_desc.kind {
            ParamKind::Boolean => {
                if dx >= 2 {
                    1.0
                } else if dx <= -2 {
                    0.0
                } else {
                    start_display_value
                }
            }
            ParamKind::Enum { .. } => {
                let step = (dx as f32 / 2.0).round();
                (start_display_value + step).clamp(display_min, display_max)
            }
            ParamKind::Continuous { .. } => {
                let sensitivity = if display_range > 0.0 {
                    display_range / 48.0
                } else {
                    0.0
                };
                (start_display_value + dx as f32 * sensitivity).clamp(display_min, display_max)
            }
        }
    }

    pub(super) fn effect_row_display_value(
        &self,
        track: usize,
        slot_idx: usize,
        param_idx: usize,
    ) -> Option<f32> {
        let desc = self.graph.effect_descriptors.get(track)?.get(slot_idx)?;
        let param_desc = desc.params.get(param_idx)?;
        let slot = self.state.effect_chains.get(track)?.get(slot_idx)?;
        Some(param_desc.stored_to_user(slot.defaults.get(param_idx)))
    }

    pub(super) fn apply_param_mouse_drag(&mut self, col: u16) {
        let Some(drag) = self.ui.param_mouse_drag else {
            return;
        };
        if drag.track >= self.tracks.len() || drag.track != self.ui.cursor_track {
            return;
        }

        let dx = col as i32 - drag.start_col as i32;
        match drag.target {
            ParamMouseDragTarget::TrackParam { row_idx } => {
                let tp = &self.state.track_params[drag.track];
                match row_idx {
                    super::TP_ATTACK => tp.set_attack_ms((drag.start_display_value + dx as f32 * 5.0).clamp(0.0, 500.0)),
                    super::TP_RELEASE => tp.set_release_ms((drag.start_display_value + dx as f32 * 10.0).clamp(0.0, 2000.0)),
                    super::TP_SWING => tp.set_swing((drag.start_display_value + dx as f32 * 0.5).clamp(50.0, 75.0)),
                    super::TP_STEPS => tp.set_num_steps((drag.start_display_value + (dx as f32 / 2.0).round()).clamp(1.0, crate::sequencer::MAX_STEPS as f32) as usize),
                    super::TP_SEND => {
                        tp.set_send((drag.start_display_value + dx as f32 * 0.01).clamp(0.0, 1.0));
                        self.push_send_gain(drag.track);
                    }
                    _ => {}
                }
            }
            ParamMouseDragTarget::SynthParam { row_idx } => {
                if row_idx == 0 {
                    let new_val = (drag.start_display_value + dx as f32 * 0.5).clamp(-48.0, 48.0);
                    self.set_instrument_base_note_offset(drag.track, new_val);
                    return;
                }

                let param_idx = row_idx - 1;
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                let slot = &self.state.instrument_slots[drag.track];
                slot.defaults.set(param_idx, new_stored);
                self.send_instrument_param(drag.track, param_idx, new_stored);
                self.mark_track_sound_dirty(drag.track);
            }
            ParamMouseDragTarget::EffectParam { slot_idx, param_idx } => {
                let Some(desc) = self
                    .graph
                    .effect_descriptors
                    .get(drag.track)
                    .and_then(|d| d.get(slot_idx))
                else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                let Some(slot) = self.state.effect_chains.get(drag.track).and_then(|c| c.get(slot_idx)) else {
                    return;
                };
                slot.defaults.set(param_idx, new_stored);
                self.send_slot_param(drag.track, slot_idx, param_idx, new_stored);
            }
            ParamMouseDragTarget::ReverbParam { param_idx } => {
                let sensitivity = 1.0 / 48.0;
                self.set_reverb_param(param_idx, drag.start_display_value + dx as f32 * sensitivity);
            }
        }
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
