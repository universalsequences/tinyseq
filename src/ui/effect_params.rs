use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::atomic::Ordering;

use crate::effects::{
    EffectDescriptor, EffectSlotState, ParamKind, SyncDivision, BUILTIN_SLOT_COUNT,
};
use crate::reverb;

use super::{App, EffectTab, InputMode, ParamMouseDragTarget};

impl App {
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

    pub(super) fn current_slot(&self) -> Option<&EffectSlotState> {
        if self.tracks.is_empty() {
            return None;
        }
        let slot_idx = self.selected_effect_slot()?;
        self.state
            .pattern
            .effect_chains
            .get(self.ui.cursor_track)
            .and_then(|chain| chain.get(slot_idx))
    }

    pub(super) fn visible_effect_indices(&self) -> Vec<usize> {
        if self.tracks.is_empty() {
            return Vec::new();
        }
        let track = self.ui.cursor_track;
        let descs = &self.graph.effect_descriptors[track];
        let mut visible = Vec::new();
        for i in 0..descs.len() {
            if i < BUILTIN_SLOT_COUNT || !descs[i].name.is_empty() {
                visible.push(i);
            }
        }
        visible
    }

    pub(super) fn can_add_custom_effect(&self) -> bool {
        self.next_free_custom_slot().is_some()
    }

    pub(super) fn handle_effects_column(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if self.ui.effect_tab == EffectTab::Synth {
            self.handle_synth_tab_input(code, modifiers);
            return;
        }
        if self.ui.effect_tab == EffectTab::Mod {
            self.handle_mod_tab_input(code, modifiers);
            return;
        }
        if self.ui.effect_tab == EffectTab::Sources {
            self.handle_sources_tab_input(code, modifiers);
            return;
        }

        if self.ui.effect_tab == EffectTab::Reverb {
            self.handle_reverb_tab_input(code, modifiers);
            return;
        }

        self.handle_effect_slot_input(code, modifiers);
    }

    fn handle_reverb_tab_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Left => {
                let visible = self.visible_effect_indices();
                if let Some(&last) = visible.last() {
                    self.ui.effect_tab = EffectTab::Slot(last);
                    self.ui.effect_param_cursor = 0;
                } else if self.is_current_custom_track() {
                    self.ui.effect_tab = EffectTab::Sources;
                    self.ui.source_param_cursor = 0;
                    self.ui.source_scroll_offset = 0;
                } else {
                    self.ui.params_column = 0;
                }
            }
            KeyCode::Right => {}
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
    }

    fn handle_effect_slot_input(&mut self, code: KeyCode, modifiers: KeyModifiers) {
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
                        self.ui.effect_tab = EffectTab::Sources;
                        self.ui.source_param_cursor = 0;
                        self.ui.source_scroll_offset = 0;
                    } else {
                        self.ui.params_column = 0;
                    }
                } else if self.is_current_custom_track() {
                    self.ui.effect_tab = EffectTab::Sources;
                    self.ui.source_param_cursor = 0;
                    self.ui.source_scroll_offset = 0;
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
                        self.ui.effect_tab = EffectTab::Reverb;
                        self.ui.reverb_param_cursor = 0;
                    }
                } else {
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

        let chain = &self.state.pattern.effect_chains[track];
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
        let chain = &self.state.pattern.effect_chains[track];
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
            .pattern
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
        let slot = self.state.pattern.effect_chains.get(track)?.get(slot_idx)?;
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
                let tp = &self.state.pattern.track_params[drag.track];
                match row_idx {
                    super::TP_ATTACK => tp.set_attack_ms(
                        (drag.start_display_value + dx as f32 * 5.0).clamp(0.0, 500.0),
                    ),
                    super::TP_RELEASE => tp.set_release_ms(
                        (drag.start_display_value + dx as f32 * 10.0).clamp(0.0, 2000.0),
                    ),
                    super::TP_SWING => {
                        tp.set_swing((drag.start_display_value + dx as f32 * 0.5).clamp(50.0, 75.0))
                    }
                    super::TP_STEPS => tp.set_num_steps(
                        (drag.start_display_value + (dx as f32 / 2.0).round())
                            .clamp(1.0, crate::sequencer::MAX_STEPS as f32)
                            as usize,
                    ),
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

                let synth_indices = self.synth_param_indices(drag.track);
                let Some(&param_idx) = synth_indices.get(row_idx - 1) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::ModParam { row_idx } => {
                let mod_indices = self.mod_param_indices(drag.track);
                let Some(&param_idx) = mod_indices.get(row_idx) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::SourceParam { row_idx } => {
                let source_indices = self.source_param_actual_indices(drag.track);
                let Some(&param_idx) = source_indices.get(row_idx) else {
                    return;
                };
                let Some(desc) = self.graph.instrument_descriptors.get(drag.track) else {
                    return;
                };
                let Some(param_desc) = desc.params.get(param_idx) else {
                    return;
                };
                let new_display =
                    self.scrub_param_display_value(param_desc, drag.start_display_value, dx);
                let new_stored = param_desc.clamp(param_desc.user_input_to_stored(new_display));
                self.set_instrument_param_or_plock(drag.track, param_idx, new_stored);
            }
            ParamMouseDragTarget::EffectParam {
                slot_idx,
                param_idx,
            } => {
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
                let Some(slot) = self
                    .state
                    .pattern
                    .effect_chains
                    .get(drag.track)
                    .and_then(|c| c.get(slot_idx))
                else {
                    return;
                };
                slot.defaults.set(param_idx, new_stored);
                self.send_slot_param(drag.track, slot_idx, param_idx, new_stored);
            }
            ParamMouseDragTarget::ReverbParam { param_idx } => {
                let sensitivity = 1.0 / 48.0;
                self.set_reverb_param(
                    param_idx,
                    drag.start_display_value + dx as f32 * sensitivity,
                );
            }
        }
    }
}
