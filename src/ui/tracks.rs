use std::ffi::CString;
use std::path::Path;
use std::sync::atomic::Ordering;

use crate::effects::EffectDescriptor;
use crate::sequencer::MAX_TRACKS;
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
}
