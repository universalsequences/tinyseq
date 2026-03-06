use std::ffi::CString;
use std::os::raw::c_void;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::effects::EffectDescriptor;
use crate::lisp_effect::{self, DGenManifest, LoadedDGenLib};
use crate::sequencer::{InstrumentType, MAX_TRACKS};
use crate::voice::MAX_VOICES;

use super::{App, TrackNodeIds};

struct TrackShell {
    voice_sum_id: i32,
    filter_id: i32,
    delay_id: i32,
    send_id: i32,
}

struct SamplerVoiceSetup {
    sampler_ids: Vec<i32>,
    voice_lids: Vec<u64>,
}

struct CustomVoiceSetup {
    synth_ids: Vec<i32>,
    gatepitch_ids: Vec<i32>,
    voice_lids: Vec<u64>,
}

enum InstrumentRegistration<'a> {
    Sampler {
        buffer_id: i32,
        sampler_ids: Vec<i32>,
    },
    Custom {
        engine_id: usize,
        synth_ids: Vec<i32>,
        gatepitch_ids: Vec<i32>,
        manifest: &'a DGenManifest,
    },
}

struct TrackRegistration<'a> {
    idx: usize,
    track_name: String,
    shell: TrackShell,
    voice_lids: Vec<u64>,
    instrument: InstrumentRegistration<'a>,
}

pub struct GraphController<'a> {
    app: &'a mut App,
}

impl App {
    pub(super) fn graph_controller(&mut self) -> GraphController<'_> {
        GraphController { app: self }
    }
}

impl GraphController<'_> {
    pub fn add_track(&mut self, wav_path: &Path) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let (buffer_id, track_name) =
            crate::sampler::load_wav_buffer(self.app.graph.lg.0, wav_path)?;
        let shell = self.create_track_shell(&track_name);
        let voices = self.build_sampler_voices(&track_name, buffer_id, shell.voice_sum_id)?;
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Sampler {
                buffer_id,
                sampler_ids: voices.sampler_ids,
            },
        });
        Ok(idx)
    }

    pub fn add_custom_track(
        &mut self,
        name: &str,
        engine_id: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<usize, String> {
        let idx = self.app.state.active_track_count();
        if idx >= MAX_TRACKS {
            return Err("Maximum number of tracks reached".to_string());
        }

        let shell = self.create_track_shell(name);
        let voices = self.build_custom_voices(idx, name, manifest, lib, shell.voice_sum_id);
        self.finish_track_registration(TrackRegistration {
            idx,
            track_name: name.to_string(),
            shell,
            voice_lids: voices.voice_lids,
            instrument: InstrumentRegistration::Custom {
                engine_id,
                synth_ids: voices.synth_ids,
                gatepitch_ids: voices.gatepitch_ids,
                manifest,
            },
        });
        Ok(idx)
    }

    pub fn hot_reload_instrument(
        &mut self,
        track: usize,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
    ) -> Result<(), String> {
        if track >= self.app.tracks.len() {
            return Err("Invalid track index".to_string());
        }
        if self.app.graph.track_instrument_types[track] != InstrumentType::Custom {
            return Err("Not a custom instrument track".to_string());
        }

        let old_synth_ids = &self.app.graph.track_synth_node_ids[track];
        let gatepitch_ids = &self.app.graph.track_gatepitch_node_ids[track];
        let voice_sum_id = self.app.graph.track_node_ids[track].voice_sum_id;
        let mut new_synth_ids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let old_synth = old_synth_ids[v];
            let gp_id = gatepitch_ids[v];

            unsafe {
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 0, old_synth, 0);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 1, old_synth, 1);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 2, old_synth, 2);
                crate::audiograph::graph_disconnect(self.app.graph.lg.0, gp_id, 3, old_synth, 3);
                crate::audiograph::graph_disconnect(
                    self.app.graph.lg.0,
                    old_synth,
                    0,
                    voice_sum_id,
                    0,
                );
                crate::audiograph::delete_node(self.app.graph.lg.0, old_synth);
            }

            let slot_id = track * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);

            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = (lisp_effect::HEADER_SLOTS + manifest.total_memory_slots)
                * std::mem::size_of::<f32>();
            let synth_name =
                CString::new(format!("{}_synth_{}", self.app.tracks[track], v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    1,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };

            unsafe {
                crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 0, synth_id, 0);
                if manifest.n_inputs > 1 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 1, synth_id, 1);
                }
                if manifest.n_inputs > 2 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 2, synth_id, 2);
                }
                if manifest.n_inputs > 3 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 3, synth_id, 3);
                }
                crate::audiograph::graph_connect(self.app.graph.lg.0, synth_id, 0, voice_sum_id, 0);
            }

            new_synth_ids.push(synth_id);
        }

        self.app.graph.track_synth_node_ids[track] = new_synth_ids.clone();
        for (v, &sid) in new_synth_ids.iter().enumerate() {
            self.app.state.synth_node_ids[track][v].store(sid as u32, Ordering::Release);
        }

        let track_name = self.app.tracks[track].clone();
        self.initialize_instrument_slot(track, &track_name, manifest);
        self.push_instrument_defaults(track, manifest);

        Ok(())
    }

    pub(super) fn apply_sample_ids(&mut self, sample_ids: &[(i32, String)]) {
        for (track, (buffer_id, name)) in sample_ids.iter().enumerate() {
            if *buffer_id < 0 {
                continue;
            }
            if track >= self.app.tracks.len() {
                break;
            }
            if !self.app.is_sampler_track(track) {
                continue;
            }
            self.send_buffer_to_all_voices(track, *buffer_id);
            self.app.graph.track_buffer_ids[track] = *buffer_id;
            self.app.tracks[track] = name.clone();
        }
    }

    pub(super) fn send_buffer_to_all_voices(&self, track: usize, buffer_id: i32) {
        if track < self.app.graph.track_voice_lids.len() {
            for &lid in &self.app.graph.track_voice_lids[track] {
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
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

    fn create_track_shell(&mut self, name: &str) -> TrackShell {
        let sum_name = CString::new(format!("{}_sum", name)).unwrap();
        let voice_sum_id = unsafe {
            crate::audiograph::live_add_gain(self.app.graph.lg.0, 1.0, sum_name.as_ptr())
        };

        let filter_name = CString::new(format!("{}_filter", name)).unwrap();
        let filter_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::filter::filter_vtable(),
                crate::filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
                filter_name.as_ptr(),
                1,
                1,
                std::ptr::null(),
                0,
            )
        };

        let delay_name = CString::new(format!("{}_delay", name)).unwrap();
        let delay_id = unsafe {
            crate::audiograph::add_node(
                self.app.graph.lg.0,
                crate::delay::delay_vtable(),
                crate::delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                delay_name.as_ptr(),
                1,
                2,
                std::ptr::null(),
                0,
            )
        };

        let send_name = CString::new(format!("{}_send", name)).unwrap();
        let send_id = unsafe {
            crate::audiograph::live_add_gain(self.app.graph.lg.0, 0.0, send_name.as_ptr())
        };

        unsafe {
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_id, 0, filter_id, 0);
            crate::audiograph::graph_connect(self.app.graph.lg.0, filter_id, 0, delay_id, 0);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                delay_id,
                0,
                self.app.graph.bus_l_id,
                0,
            );
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                delay_id,
                1,
                self.app.graph.bus_r_id,
                0,
            );
            crate::audiograph::graph_connect(self.app.graph.lg.0, voice_sum_id, 0, send_id, 0);
            crate::audiograph::graph_connect(
                self.app.graph.lg.0,
                send_id,
                0,
                self.app.graph.reverb_bus_id,
                0,
            );
        }

        TrackShell {
            voice_sum_id,
            filter_id,
            delay_id,
            send_id,
        }
    }

    fn build_sampler_voices(
        &mut self,
        track_name: &str,
        buffer_id: i32,
        voice_sum_id: i32,
    ) -> Result<SamplerVoiceSetup, String> {
        let mut sampler_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let node_name = format!("{}_{}", track_name, v);
            let st =
                crate::sampler::create_sampler_node(self.app.graph.lg.0, buffer_id, &node_name)?;
            unsafe {
                crate::audiograph::graph_connect(
                    self.app.graph.lg.0,
                    st.node_id,
                    0,
                    voice_sum_id,
                    0,
                );
            }
            sampler_ids.push(st.node_id);
            voice_lids.push(st.logical_id);
        }

        Ok(SamplerVoiceSetup {
            sampler_ids,
            voice_lids,
        })
    }

    fn build_custom_voices(
        &mut self,
        track_idx: usize,
        name: &str,
        manifest: &DGenManifest,
        lib: &LoadedDGenLib,
        voice_sum_id: i32,
    ) -> CustomVoiceSetup {
        let mut gatepitch_ids = Vec::with_capacity(MAX_VOICES);
        let mut synth_ids = Vec::with_capacity(MAX_VOICES);
        let mut voice_lids = Vec::with_capacity(MAX_VOICES);

        for v in 0..MAX_VOICES {
            let gp_name = CString::new(format!("{}_gp_{}", name, v)).unwrap();
            let gp_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    crate::gatepitch::gatepitch_vtable(),
                    crate::gatepitch::GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                    gp_name.as_ptr(),
                    0,
                    4,
                    std::ptr::null(),
                    0,
                )
            };

            let slot_id = track_idx * MAX_VOICES + v;
            lisp_effect::set_dgen_instrument_fn(slot_id, lib.process_fn);
            let init_msg = lisp_effect::build_init_message_for_voice(slot_id, manifest, v);
            let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();
            let state_size = (lisp_effect::HEADER_SLOTS + manifest.total_memory_slots)
                * std::mem::size_of::<f32>();

            let synth_name = CString::new(format!("{}_synth_{}", name, v)).unwrap();
            let synth_id = unsafe {
                crate::audiograph::add_node(
                    self.app.graph.lg.0,
                    lisp_effect::dgenlisp_instrument_vtable(),
                    state_size,
                    synth_name.as_ptr(),
                    manifest.n_inputs as i32,
                    1,
                    init_msg.as_ptr() as *const c_void,
                    init_msg_size,
                )
            };

            unsafe {
                crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 0, synth_id, 0);
                if manifest.n_inputs > 1 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 1, synth_id, 1);
                }
                if manifest.n_inputs > 2 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 2, synth_id, 2);
                }
                if manifest.n_inputs > 3 {
                    crate::audiograph::graph_connect(self.app.graph.lg.0, gp_id, 3, synth_id, 3);
                }
                crate::audiograph::graph_connect(self.app.graph.lg.0, synth_id, 0, voice_sum_id, 0);
            }

            gatepitch_ids.push(gp_id);
            synth_ids.push(synth_id);
            voice_lids.push(gp_id as u64);
        }

        CustomVoiceSetup {
            synth_ids,
            gatepitch_ids,
            voice_lids,
        }
    }

    fn finish_track_registration(&mut self, registration: TrackRegistration<'_>) {
        let TrackRegistration {
            idx,
            track_name,
            shell,
            voice_lids,
            instrument,
        } = registration;
        let instrument_type = match instrument {
            InstrumentRegistration::Sampler { .. } => InstrumentType::Sampler,
            InstrumentRegistration::Custom { .. } => InstrumentType::Custom,
        };

        for (v, &lid) in voice_lids.iter().enumerate() {
            self.app.state.voice_lids[idx][v].store(lid, Ordering::Release);
        }
        self.app.state.voice_counts[idx].store(MAX_VOICES as u32, Ordering::Release);
        self.app.state.sampler_lids[idx].store(voice_lids[0], Ordering::Release);
        self.app.state.delay_lids[idx].store(shell.delay_id as u64, Ordering::Release);
        self.app.state.send_lids[idx].store(shell.send_id as u64, Ordering::Release);
        self.app.state.instrument_type_flags[idx].store(
            (instrument_type == InstrumentType::Custom) as u32,
            Ordering::Release,
        );

        let filter_desc = EffectDescriptor::builtin_filter();
        let delay_desc = EffectDescriptor::builtin_delay();
        let chain = &self.app.state.effect_chains[idx];
        chain[0].apply_descriptor(&filter_desc, shell.filter_id as u32);
        chain[1].apply_descriptor(&delay_desc, shell.delay_id as u32);

        self.app.tracks.push(track_name.clone());
        self.app
            .graph
            .effect_descriptors
            .push(EffectDescriptor::default_full_chain());
        self.app.graph.record_armed.push(false);
        self.app.graph.track_voice_lids.push(voice_lids);
        self.app.graph.track_instrument_types.push(instrument_type);

        match instrument {
            InstrumentRegistration::Sampler {
                buffer_id,
                sampler_ids,
            } => {
                self.app.graph.track_buffer_ids.push(buffer_id);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids,
                    voice_sum_id: shell.voice_sum_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                });
                self.app.graph.track_synth_node_ids.push(Vec::new());
                self.app.graph.track_gatepitch_node_ids.push(Vec::new());
                self.app.graph.track_engine_ids.push(None);
                self.app
                    .graph
                    .instrument_descriptors
                    .push(EffectDescriptor::empty_custom_slot());
            }
            InstrumentRegistration::Custom {
                engine_id,
                synth_ids,
                gatepitch_ids,
                manifest,
            } => {
                for (v, &sid) in synth_ids.iter().enumerate() {
                    self.app.state.synth_node_ids[idx][v].store(sid as u32, Ordering::Release);
                }
                self.app.graph.track_buffer_ids.push(-1);
                self.app.graph.track_node_ids.push(TrackNodeIds {
                    sampler_ids: gatepitch_ids.clone(),
                    voice_sum_id: shell.voice_sum_id,
                    filter_id: shell.filter_id,
                    delay_id: shell.delay_id,
                    send_id: shell.send_id,
                });
                self.app.graph.track_synth_node_ids.push(synth_ids);
                self.app.graph.track_gatepitch_node_ids.push(gatepitch_ids);
                self.app.graph.track_engine_ids.push(Some(engine_id));
                self.initialize_instrument_slot(idx, &track_name, manifest);
                self.push_instrument_defaults(idx, manifest);
            }
        }

        let mut bank = self.app.state.pattern_bank.lock().unwrap();
        for snap in bank.iter_mut() {
            snap.extend_to_tracks(idx + 1, &self.app.graph.effect_descriptors);
        }

        self.app
            .state
            .num_tracks
            .store((idx + 1) as u32, Ordering::Release);
    }

    fn initialize_instrument_slot(&mut self, track: usize, name: &str, manifest: &DGenManifest) {
        let inst_desc = EffectDescriptor::from_lisp_manifest(name, &manifest.params);
        let inst_slot = &self.app.state.instrument_slots[track];
        inst_slot
            .num_params
            .store(manifest.params.len() as u32, Ordering::Relaxed);
        for (i, p) in manifest.params.iter().enumerate() {
            inst_slot.defaults.set(i, p.default);
            if i < inst_slot.param_node_indices.len() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                inst_slot.param_node_indices[i].store(node_idx, Ordering::Relaxed);
            }
        }

        if track < self.app.graph.instrument_descriptors.len() {
            self.app.graph.instrument_descriptors[track] = inst_desc;
        } else {
            self.app.graph.instrument_descriptors.push(inst_desc);
        }
    }

    fn push_instrument_defaults(&self, track: usize, manifest: &DGenManifest) {
        for &synth_id in &self.app.graph.track_synth_node_ids[track] {
            for p in manifest.params.iter() {
                let node_idx = (lisp_effect::HEADER_SLOTS + p.cell_id) as u32;
                unsafe {
                    crate::audiograph::params_push_wrapper(
                        self.app.graph.lg.0,
                        crate::audiograph::ParamMsg {
                            idx: node_idx as u64,
                            logical_id: synth_id as u64,
                            fvalue: p.default,
                        },
                    );
                }
            }
        }
    }
}
