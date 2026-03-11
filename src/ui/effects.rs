use crossterm::event::KeyCode;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::effects::{EffectDescriptor, BUILTIN_SLOT_COUNT};
use crate::lisp_effect::{self, MAX_CUSTOM_FX};
use crate::sequencer::InstrumentType;

use super::{App, CompileTarget, EffectTab, InputMode, PendingCompile, PendingEditor, Region};

#[derive(Clone, Copy)]
pub(super) enum OverlayPickerKind {
    Effect,
    Instrument,
}

impl App {
    pub fn add_saved_instrument_track_sync(&mut self, name: &str) -> Result<usize, String> {
        let source = lisp_effect::load_instrument_source(name).map_err(|e| e.to_string())?;

        if let Some(cache_idx) = self.cached_instrument_engine_idx(name, &source) {
            let manifest = self.editor.engine_registry.engines[cache_idx]
                .manifest
                .clone();
            let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
            let lib_ptr: *const lisp_effect::LoadedDGenLib =
                &self.editor.instrument_libs[lib_index];
            return unsafe {
                self.graph_controller()
                    .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
            };
        }

        let result = lisp_effect::compile_and_load_instrument(&source, self.graph.sample_rate)?;
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
        let lib_index = self.editor.engine_registry.engines[cache_idx].lib_index;
        let lib_ptr: *const lisp_effect::LoadedDGenLib = &self.editor.instrument_libs[lib_index];
        unsafe {
            self.graph_controller()
                .add_custom_track(name, cache_idx, &manifest, &*lib_ptr)
        }
    }

    fn cached_instrument_engine_idx(&self, name: &str, source: &str) -> Option<usize> {
        self.editor
            .engine_registry
            .find_by_name_and_source(name, source)
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
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
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

    pub(super) fn next_free_custom_slot(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        let chain = &self.state.pattern.effect_chains[self.ui.cursor_track];
        for offset in 0..MAX_CUSTOM_FX {
            let idx = BUILTIN_SLOT_COUNT + offset;
            if idx < chain.len() && chain[idx].node_id.load(Ordering::Relaxed) == 0 {
                return Some(idx);
            }
        }
        None
    }

    fn find_custom_slot_predecessor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.state.pattern.effect_chains[track];
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

    fn find_custom_slot_successor(&self, track: usize, offset: usize) -> i32 {
        let chain = &self.state.pattern.effect_chains[track];
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

    fn resolve_custom_slot_wiring(
        &self,
        track: usize,
        slot_idx: usize,
    ) -> (usize, i32, i32, Option<i32>) {
        let offset = slot_idx - BUILTIN_SLOT_COUNT;
        let slot_id = track * MAX_CUSTOM_FX + offset;
        let predecessor_id = self.find_custom_slot_predecessor(track, offset);
        let successor_id = self.find_custom_slot_successor(track, offset);
        let existing_node = self.state.pattern.effect_chains[track]
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

        let slot = &self.state.pattern.effect_chains[track][slot_idx];
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

    pub(super) fn load_saved_effect_to_slot_sync(
        &mut self,
        track: usize,
        slot_idx: usize,
        name: &str,
    ) -> Result<(), String> {
        let source = lisp_effect::load_effect_source(name).map_err(|e| e.to_string())?;
        let result = lisp_effect::compile_and_load(&source, self.graph.sample_rate)?;
        let (slot_id, pred, succ, existing) = self.resolve_custom_slot_wiring(track, slot_idx);
        let node_id = unsafe {
            lisp_effect::add_effect_to_chain_at(
                self.graph.lg.0,
                slot_id,
                &result.manifest,
                &result.lib,
                pred,
                succ,
                existing,
            )
        }?;
        self.apply_effect_to_slot(track, slot_idx, node_id, name, &result.manifest.params);
        self.editor.lisp_libs.push(result.lib);
        Ok(())
    }

    pub(super) fn apply_compiled_instrument(
        &mut self,
        result: lisp_effect::CompileResult,
        name: &str,
    ) {
        let source = lisp_effect::load_instrument_source(name).unwrap_or_default();
        let cache_idx = self.cache_instrument_engine(name, &source, &result.manifest, result.lib);
        let manifest = self.editor.engine_registry.engines[cache_idx]
            .manifest
            .clone();
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
            let is_existing_custom = self.ui.cursor_track < self.graph.track_instrument_types.len()
                && self.graph.track_instrument_types[self.ui.cursor_track]
                    == InstrumentType::Custom;

            if is_existing_custom {
                let track = self.ui.cursor_track;
                let cache_idx =
                    self.cache_instrument_engine(&r.name, &r.source, &r.manifest, r.lib);
                let manifest = self.editor.engine_registry.engines[cache_idx]
                    .manifest
                    .clone();
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
                        if let Some(sound) = self
                            .state
                            .pattern
                            .track_sound_state
                            .lock()
                            .unwrap()
                            .get_mut(track)
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
                let manifest = self.editor.engine_registry.engines[cache_idx]
                    .manifest
                    .clone();
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
}
