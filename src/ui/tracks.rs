use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::effects::EffectDescriptor;
use crate::lisp_effect::{self, DGenManifest, LoadedDGenLib};
use crate::sequencer::{InstrumentType, MAX_TRACKS};
use crate::voice::MAX_VOICES;

use super::{App, TrackNodeIds};

impl App {
    /// Add a new track from a .wav file path. Returns the track index.
    pub fn add_track(&mut self, wav_path: &Path) -> Result<usize, String> {
        let idx = self.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        // Load WAV buffer once
        let (buffer_id, track_name) = crate::sampler::load_wav_buffer(self.lg.0, wav_path)?;

        // Create MAX_VOICES sampler nodes sharing the same buffer
        let mut sampler_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);
        for v in 0..MAX_VOICES {
            let node_name = format!("{}_{}", track_name, v);
            let st = crate::sampler::create_sampler_node(self.lg.0, buffer_id, &node_name)?;
            sampler_ids.push(st.node_id);
            voice_lids.push(st.logical_id);
        }

        // Create voice_sum gain node (gain=1.0)
        let sum_name = CString::new(format!("{}_sum", track_name)).unwrap();
        let voice_sum_id =
            unsafe { crate::audiograph::live_add_gain(self.lg.0, 1.0, sum_name.as_ptr()) };

        // Create filter node (1 in, 1 out)
        let filter_name = CString::new(format!("{}_filter", track_name)).unwrap();
        let filter_id = unsafe {
            crate::audiograph::add_node(
                self.lg.0,
                crate::filter::filter_vtable(),
                crate::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
                filter_name.as_ptr(),
                1,
                1,
                std::ptr::null(),
                0,
            )
        };

        // Create delay node (1 in, 2 out — stereo)
        let delay_name = CString::new(format!("{}_delay", track_name)).unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                self.lg.0,
                crate::delay::delay_vtable(),
                crate::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                1,
                2,
                std::ptr::null(),
                0,
            )
        };

        // Create send gain node (default gain 0 = silent, controlled per-step)
        let send_name = CString::new(format!("{}_send", track_name)).unwrap();
        let send_id =
            unsafe { crate::audiograph::live_add_gain(self.lg.0, 0.0, send_name.as_ptr()) };

        // Wire: voice_0..5 → voice_sum → [custom_fx] → filter → delay → bus_L/bus_R
        //        voice_sum → send → reverb_bus (send path)
        unsafe {
            for &sid in &sampler_ids {
                crate::audiograph::graph_connect(self.lg.0, sid, 0, voice_sum_id, 0);
            }
            crate::audiograph::graph_connect(self.lg.0, voice_sum_id, 0, filter_id, 0);
            crate::audiograph::graph_connect(self.lg.0, filter_id, 0, delay_id, 0);
            crate::audiograph::graph_connect(self.lg.0, delay_id, 0, self.bus_l_id, 0);
            crate::audiograph::graph_connect(self.lg.0, delay_id, 1, self.bus_r_id, 0);
            crate::audiograph::graph_connect(self.lg.0, voice_sum_id, 0, send_id, 0);
            crate::audiograph::graph_connect(self.lg.0, send_id, 0, self.reverb_bus_id, 0);
        }

        // Store voice LIDs in state for audio thread
        for (v, &lid) in voice_lids.iter().enumerate() {
            self.state.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.state.voice_counts[idx].store(MAX_VOICES as u32, Ordering::Release);

        // Also store first voice as sampler_lid for backward compat
        self.state.sampler_lids[idx].store(voice_lids[0], Ordering::Release);
        self.state.delay_lids[idx].store(delay_id as u64, Ordering::Release);
        self.state.send_lids[idx].store(send_id as u64, Ordering::Release);

        // Initialize effect chain for this track slot
        let filter_desc = EffectDescriptor::builtin_filter();
        let delay_desc = EffectDescriptor::builtin_delay();
        let chain = &self.state.effect_chains[idx];
        chain[0].apply_descriptor(&filter_desc, filter_id as u32);
        chain[1].apply_descriptor(&delay_desc, delay_id as u32);

        // Push to App's UI-side tracking
        self.tracks.push(track_name);
        self.track_buffer_ids.push(buffer_id);
        self.track_node_ids.push(TrackNodeIds {
            sampler_ids,
            voice_sum_id,
            filter_id,
            delay_id,
            send_id,
        });
        self.effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.record_armed.push(false);
        self.track_voice_lids.push(voice_lids);
        self.track_instrument_types.push(InstrumentType::Sampler);
        self.track_synth_node_ids.push(Vec::new());
        self.track_gatepitch_node_ids.push(Vec::new());
        self.instrument_descriptors.push(EffectDescriptor::empty_custom_slot());

        // Extend all pattern bank snapshots to cover the new track
        {
            let mut bank = self.state.pattern_bank.lock().unwrap();
            for snap in bank.iter_mut() {
                snap.extend_to_tracks(idx + 1, &self.effect_descriptors);
            }
        }

        // Make the new track visible to the audio thread (Release ordering).
        // The graph edit commands (add_node, graph_connect) are queued and
        // applied atomically by process_next_block, so the new nodes will be
        // fully wired before the audio thread processes them.
        self.state
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);

        Ok(idx)
    }

    /// Apply sample assignments from a restored pattern snapshot.
    /// For each track with a valid buffer_id (>= 0), send a ParamMsg to swap the
    /// buffer and update the UI's track name / buffer_id.
    pub(super) fn apply_sample_ids(&mut self, sample_ids: &[(i32, String)]) {
        for (track, (buffer_id, name)) in sample_ids.iter().enumerate() {
            if *buffer_id < 0 {
                continue;
            }
            if track >= self.tracks.len() {
                break;
            }
            // Skip buffer swap for non-sampler tracks
            if !self.is_sampler_track(track) {
                continue;
            }
            // Send buffer swap to ALL voice LIDs for this track
            self.send_buffer_to_all_voices(track, *buffer_id);
            self.track_buffer_ids[track] = *buffer_id;
            self.tracks[track] = name.clone();
        }
    }

    /// Send PARAM_BUFFER_ID to all voice logical IDs for a track.
    pub(super) fn send_buffer_to_all_voices(&self, track: usize, buffer_id: i32) {
        if track < self.track_voice_lids.len() {
            for &lid in &self.track_voice_lids[track] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: crate::sampler::PARAM_BUFFER_ID,
                            logical_id: lid,
                            fvalue: buffer_id as f32,
                        },
                    );
                }
            }
        }
    }

    /// Add a new custom instrument track. Returns the track index.
    /// Creates GatePitch + DGenLisp synth pairs for each voice, wired to
    /// voice_sum → filter → delay → buses (same downstream as sampler tracks).
    pub fn add_custom_track(
        &mut self,
        name: &str,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<usize, String> {
        let idx = self.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        // Create voice_sum gain node
        let sum_name = CString::new(format!("{}_sum", name)).unwrap();
        let voice_sum_id =
            unsafe { crate::audiograph::live_add_gain(self.lg.0, 1.0, sum_name.as_ptr()) };

        // Create filter node
        let filter_name = CString::new(format!("{}_filter", name)).unwrap();
        let filter_id = unsafe {
            crate::audiograph::add_node(
                self.lg.0,
                crate::filter::filter_vtable(),
                crate::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
                filter_name.as_ptr(),
                1,
                1,
                std::ptr::null(),
                0,
            )
        };

        // Create delay node
        let delay_name = CString::new(format!("{}_delay", name)).unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                self.lg.0,
                crate::delay::delay_vtable(),
                crate::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                1,
                2,
                std::ptr::null(),
                0,
            )
        };

        // Create send gain node
        let send_name = CString::new(format!("{}_send", name)).unwrap();
        let send_id =
            unsafe { crate::audiograph::live_add_gain(self.lg.0, 0.0, send_name.as_ptr()) };

        // Create GatePitch + synth pairs for each voice
        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            // Create GatePitch node (0 in, 4 out)
            let gp_name = CString::new(format!("{}_gp_{}", name, v)).unwrap();
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.lg.0,
                    crate::gatepitch::gatepitch_vtable(),
                    crate::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    4,
                    std::ptr::null(),
                    0,
                )
            };

            // Register instrument process fn for this voice slot
            let slot_id = idx * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);

            // Build init message with voice index
            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = (lisp_effect::HEADER_SLOTS + manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            // Create DGenLisp synth node using the manifest-declared input count.
            let synth_name = CString::new(format!("{}_synth_{}", name, v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    1,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };

            // Wire available gatepitch outputs into the synth's declared inputs.
            unsafe {
                crate::audiograph::graph_connect(self.lg.0, gp_id, 0, synth_id, 0);
                if manifest.n_inputs > 1 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 1, synth_id, 1);
                }
                if manifest.n_inputs > 2 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 2, synth_id, 2);
                }
                if manifest.n_inputs > 3 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 3, synth_id, 3);
                }
                crate::audiograph::graph_connect(self.lg.0, synth_id, 0, voice_sum_id, 0);
            }

            gatepitch_ids.push(gp_id);
            synth_ids.push(synth_id);
            // Use gatepitch node LID for triggers — audio thread sends gate/pitch to these
            voice_lids.push(gp_id as u64);
        }

        // Wire downstream: voice_sum → filter → delay → bus_L/bus_R, voice_sum → send → reverb
        unsafe {
            crate::audiograph::graph_connect(self.lg.0, voice_sum_id, 0, filter_id, 0);
            crate::audiograph::graph_connect(self.lg.0, filter_id, 0, delay_id, 0);
            crate::audiograph::graph_connect(self.lg.0, delay_id, 0, self.bus_l_id, 0);
            crate::audiograph::graph_connect(self.lg.0, delay_id, 1, self.bus_r_id, 0);
            crate::audiograph::graph_connect(self.lg.0, voice_sum_id, 0, send_id, 0);
            crate::audiograph::graph_connect(self.lg.0, send_id, 0, self.reverb_bus_id, 0);
        }

        // Store voice LIDs (GatePitch LIDs) in state for audio thread
        for (v, &lid) in voice_lids.iter().enumerate() {
            self.state.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.state.voice_counts[idx].store(MAX_VOICES as u32, Ordering::Release);
        self.state.sampler_lids[idx].store(voice_lids[0], Ordering::Release);
        self.state.delay_lids[idx].store(delay_id as u64, Ordering::Release);
        self.state.send_lids[idx].store(send_id as u64, Ordering::Release);
        self.state.instrument_type_flags[idx].store(1, Ordering::Release);

        // Initialize effect chain
        let filter_desc = EffectDescriptor::builtin_filter();
        let delay_desc = EffectDescriptor::builtin_delay();
        let chain = &self.state.effect_chains[idx];
        chain[0].apply_descriptor(&filter_desc, filter_id as u32);
        chain[1].apply_descriptor(&delay_desc, delay_id as u32);

        // Push to App's UI-side tracking
        self.tracks.push(name.to_string());
        self.track_buffer_ids.push(-1); // no buffer for custom tracks
        self.track_node_ids.push(TrackNodeIds {
            sampler_ids: gatepitch_ids.clone(),
            voice_sum_id,
            filter_id,
            delay_id,
            send_id,
        });
        self.effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.record_armed.push(false);
        self.track_voice_lids.push(voice_lids);
        self.track_instrument_types.push(InstrumentType::Custom);
        self.track_synth_node_ids.push(synth_ids.clone());
        self.track_gatepitch_node_ids.push(gatepitch_ids);

        // Store synth node IDs in state for audio thread
        for (v, &sid) in synth_ids.iter().enumerate() {
            self.state.synth_node_ids[idx][v].store(sid as u32, Ordering::Release);
        }

        // Populate instrument param slot for Synth tab
        let inst_desc = EffectDescriptor::from_lisp_manifest(name, &manifest.params);
        let inst_slot = &self.state.instrument_slots[idx];
        inst_slot.num_params.store(manifest.params.len() as u32, Ordering::Relaxed);
        for (i, p) in manifest.params.iter().enumerate() {
            inst_slot.defaults.set(i, p.default);
            if i < inst_slot.param_node_indices.len() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                inst_slot.param_node_indices[i].store(node_idx, Ordering::Relaxed);
            }
        }
        self.instrument_descriptors.push(inst_desc);

        // Send initial default values to all synth nodes
        for &synth_id in &self.track_synth_node_ids[idx] {
            for p in manifest.params.iter() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: node_idx as u64,
                            logical_id: synth_id as u64,
                            fvalue: p.default,
                        },
                    );
                }
            }
        }

        // Extend pattern bank
        {
            let mut bank = self.state.pattern_bank.lock().unwrap();
            for snap in bank.iter_mut() {
                snap.extend_to_tracks(idx + 1, &self.effect_descriptors);
            }
        }

        self.state
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);

        Ok(idx)
    }

    /// Hot-reload the synth code on an existing custom track.
    /// Keeps GatePitch nodes and downstream chain intact; only swaps the synth nodes.
    pub fn hot_reload_instrument(
        &mut self,
        track: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if self.track_instrument_types[track] != InstrumentType::Custom {
            return Err("Not a custom instrument track".to_string());
        }

        let old_synth_ids = &self.track_synth_node_ids[track];
        let gatepitch_ids = &self.track_gatepitch_node_ids[track];
        let voice_sum_id = self.track_node_ids[track].voice_sum_id;

        let mut new_synth_ids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let old_synth = old_synth_ids[v];
            let gp_id = gatepitch_ids[v];

            // Disconnect old synth
            unsafe {
                crate::audiograph::graph_disconnect(self.lg.0, gp_id, 0, old_synth, 0);
                crate::audiograph::graph_disconnect(self.lg.0, gp_id, 1, old_synth, 1);
                crate::audiograph::graph_disconnect(self.lg.0, gp_id, 2, old_synth, 2);
                crate::audiograph::graph_disconnect(self.lg.0, gp_id, 3, old_synth, 3);
                crate::audiograph::graph_disconnect(self.lg.0, old_synth, 0, voice_sum_id, 0);
                crate::audiograph::delete_node(self.lg.0, old_synth);
            }

            // Register new process fn
            let slot_id = track * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);

            // Create new synth node
            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = (lisp_effect::HEADER_SLOTS + manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name =
                CString::new(format!("{}_synth_{}", self.tracks[track], v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    1,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };

            // Reconnect: gatepitch → synth → voice_sum
            unsafe {
                crate::audiograph::graph_connect(self.lg.0, gp_id, 0, synth_id, 0);
                if manifest.n_inputs > 1 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 1, synth_id, 1);
                }
                if manifest.n_inputs > 2 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 2, synth_id, 2);
                }
                if manifest.n_inputs > 3 {
                    crate::audiograph::graph_connect(self.lg.0, gp_id, 3, synth_id, 3);
                }
                crate::audiograph::graph_connect(self.lg.0, synth_id, 0, voice_sum_id, 0);
            }

            new_synth_ids.push(synth_id);
        }

        self.track_synth_node_ids[track] = new_synth_ids.clone();

        // Store new synth node IDs in state for audio thread
        for (v, &sid) in new_synth_ids.iter().enumerate() {
            self.state.synth_node_ids[track][v].store(sid as u32, Ordering::Release);
        }

        // Update instrument param slot for Synth tab
        let inst_desc = EffectDescriptor::from_lisp_manifest(&self.tracks[track], &manifest.params);
        let inst_slot = &self.state.instrument_slots[track];
        inst_slot.num_params.store(manifest.params.len() as u32, Ordering::Relaxed);
        for (i, p) in manifest.params.iter().enumerate() {
            inst_slot.defaults.set(i, p.default);
            if i < inst_slot.param_node_indices.len() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                inst_slot.param_node_indices[i].store(node_idx, Ordering::Relaxed);
            }
        }
        if track < self.instrument_descriptors.len() {
            self.instrument_descriptors[track] = inst_desc;
        }

        // Send initial default values to all new synth nodes
        for &synth_id in &self.track_synth_node_ids[track] {
            for p in manifest.params.iter() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: node_idx as u64,
                            logical_id: synth_id as u64,
                            fvalue: p.default,
                        },
                    );
                }
            }
        }

        Ok(())
    }
}
