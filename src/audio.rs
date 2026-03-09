use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::audiograph::*;
use crate::delay;
use crate::gatepitch;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_GATE_MODE, PARAM_GATE_SAMPLES, PARAM_PLAYHEAD,
    PARAM_RELEASE_SAMPLES, PARAM_SPEED, PARAM_TRANSPOSE, PARAM_TRIGGER, PARAM_VELOCITY,
};
use crate::sequencer::{KeyboardTrigger, SequencerClock, SequencerState, StepParam, MAX_TRACKS};
use crate::voice::{VoicePool, MAX_VOICES};

/// Per-track chop re-trigger state.
struct ChopTracker {
    /// How many chop triggers remain (excluding the initial trigger).
    remaining: u32,
    /// Samples countdown until next chop trigger.
    counter: f64,
    /// Samples between each chop trigger.
    interval: f64,
    /// The step whose params to re-use.
    step: usize,
    /// Gate length in samples for each chop subdivision.
    chop_gate: f32,
}

/// Per-track swing pending state.
struct SwingPending {
    /// Samples remaining until swung trigger fires.
    countdown: f64,
    /// The step that's pending.
    step: usize,
    /// Whether there's a pending swing trigger.
    active: bool,
}

/// Pending gate-off events for custom instrument voices.
struct GateOffPending {
    lid: u64,
    countdown: f64,
}

/// Per-track gate-off queue for custom instruments.
struct GateOffTracker {
    pending: Vec<GateOffPending>,
}

impl GateOffTracker {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Schedule a gate-off after `delay_samples` for the given voice LID.
    fn schedule(&mut self, lid: u64, delay_samples: f64) {
        // If there's already a pending gate-off for this LID, replace it
        for p in &mut self.pending {
            if p.lid == lid {
                p.countdown = delay_samples;
                return;
            }
        }
        self.pending.push(GateOffPending {
            lid,
            countdown: delay_samples,
        });
    }

    fn cancel(&mut self, lid: u64) {
        self.pending.retain(|p| p.lid != lid);
    }

    /// Advance all countdowns by nframes. Returns LIDs that expired.
    fn process(&mut self, nframes: usize) -> Vec<u64> {
        let mut expired = Vec::new();
        self.pending.retain_mut(|p| {
            p.countdown -= nframes as f64;
            if p.countdown <= 0.0 {
                expired.push(p.lid);
                false
            } else {
                true
            }
        });
        expired
    }
}

fn cancel_gate_off_for_lid(gate_off_state: &mut [GateOffTracker], lid: u64) {
    for tracker in gate_off_state {
        tracker.cancel(lid);
    }
}

struct AudioCallbackData {
    lg: LiveGraphPtr,
    clock: SequencerClock,
    state: Arc<SequencerState>,
    num_channels: usize,
    chop_state: Vec<ChopTracker>,
    swing_state: Vec<SwingPending>,
    gate_off_state: Vec<GateOffTracker>,
    sample_rate: f64,
    last_bpm: u32,
    last_mod_reset_counter: u32,
    voice_pools: Vec<VoicePool>,
    custom_engine_pools: Vec<CustomEnginePool>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
}

struct CustomVoiceSlot {
    logical_id: u64,
    age: u64,
    active: bool,
    note: f32,
    assigned_track: Option<usize>,
    fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CustomVoiceAllocation {
    voice_idx: usize,
    logical_id: u64,
    previous_track: Option<usize>,
    stole_active_voice: bool,
}

struct CustomEnginePool {
    voices: [CustomVoiceSlot; MAX_VOICES],
    num_voices: usize,
    age_counter: u64,
}

impl CustomEnginePool {
    fn new() -> Self {
        Self {
            voices: std::array::from_fn(|_| CustomVoiceSlot {
                logical_id: 0,
                age: 0,
                active: false,
                note: 0.0,
                assigned_track: None,
                fingerprint: 0,
            }),
            num_voices: 0,
            age_counter: 0,
        }
    }

    fn add_voice(&mut self, logical_id: u64) {
        if self.num_voices < MAX_VOICES {
            self.voices[self.num_voices] = CustomVoiceSlot {
                logical_id,
                age: 0,
                active: false,
                note: 0.0,
                assigned_track: None,
                fingerprint: 0,
            };
            self.num_voices += 1;
        }
    }

    fn allocate_voice(
        &mut self,
        track: usize,
        note: f32,
        polyphonic: bool,
    ) -> CustomVoiceAllocation {
        self.age_counter += 1;
        if !polyphonic {
            if let Some(idx) =
                (0..self.num_voices).find(|&i| self.voices[i].assigned_track == Some(track))
            {
                let slot = &mut self.voices[idx];
                let previous_track = slot.assigned_track;
                let stole_active_voice = slot.active;
                slot.age = self.age_counter;
                slot.active = true;
                slot.note = note;
                slot.assigned_track = Some(track);
                return CustomVoiceAllocation {
                    voice_idx: idx,
                    logical_id: slot.logical_id,
                    previous_track,
                    stole_active_voice,
                };
            }
        }

        let mut free_idx = None;
        let mut free_age = u64::MAX;
        let mut oldest_same_track = None;
        let mut oldest_same_track_age = u64::MAX;
        let mut oldest_idx = 0;
        let mut oldest_age = u64::MAX;

        for i in 0..self.num_voices {
            let voice = &self.voices[i];
            if !voice.active && voice.age < free_age {
                free_idx = Some(i);
                free_age = voice.age;
            }
            if voice.assigned_track == Some(track) && voice.age < oldest_same_track_age {
                oldest_same_track = Some(i);
                oldest_same_track_age = voice.age;
            }
            if voice.age < oldest_age {
                oldest_idx = i;
                oldest_age = voice.age;
            }
        }

        let idx = free_idx.or(oldest_same_track).unwrap_or(oldest_idx);
        let slot = &mut self.voices[idx];
        let previous_track = slot.assigned_track;
        let stole_active_voice = slot.active;
        slot.age = self.age_counter;
        slot.active = true;
        slot.note = note;
        slot.assigned_track = Some(track);
        CustomVoiceAllocation {
            voice_idx: idx,
            logical_id: slot.logical_id,
            previous_track,
            stole_active_voice,
        }
    }

    fn release_voice_by_note(&mut self, track: usize, note: f32) -> Option<u64> {
        for i in 0..self.num_voices {
            let voice = &mut self.voices[i];
            if voice.active
                && voice.assigned_track == Some(track)
                && (voice.note - note).abs() < 0.01
            {
                voice.active = false;
                return Some(voice.logical_id);
            }
        }
        None
    }

    fn release_voice_by_logical_id(&mut self, logical_id: u64) {
        for i in 0..self.num_voices {
            if self.voices[i].logical_id == logical_id {
                self.voices[i].active = false;
                return;
            }
        }
    }
}

/// Send a trigger to the sampler with the given per-step params, gate length, and explicit transpose.
unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    sd: &crate::sequencer::StepData,
    step: usize,
    gate_samples: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
    transpose: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_VELOCITY,
            logical_id: lid,
            fvalue: sd.get(step, StepParam::Velocity),
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SPEED,
            logical_id: lid,
            fvalue: sd.get(step, StepParam::Speed),
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_SAMPLES,
            logical_id: lid,
            fvalue: gate_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRANSPOSE,
            logical_id: lid,
            fvalue: transpose,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ATTACK_SAMPLES,
            logical_id: lid,
            fvalue: attack_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_RELEASE_SAMPLES,
            logical_id: lid,
            fvalue: release_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_MODE,
            logical_id: lid,
            fvalue: gate_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_PLAYHEAD,
            logical_id: lid,
            fvalue: 0.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRIGGER,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
}

/// Send a keyboard trigger directly to a voice (no step data lookup).
unsafe fn send_keyboard_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    transpose: f32,
    velocity: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_VELOCITY,
            logical_id: lid,
            fvalue: velocity,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_SPEED,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_SAMPLES,
            logical_id: lid,
            fvalue: f32::MAX,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRANSPOSE,
            logical_id: lid,
            fvalue: transpose,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_ATTACK_SAMPLES,
            logical_id: lid,
            fvalue: attack_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_RELEASE_SAMPLES,
            logical_id: lid,
            fvalue: release_samples,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_GATE_MODE,
            logical_id: lid,
            fvalue: gate_mode,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_PLAYHEAD,
            logical_id: lid,
            fvalue: 0.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: PARAM_TRIGGER,
            logical_id: lid,
            fvalue: 1.0,
        },
    );
}

/// Send a gate-on trigger to a GatePitch node with pitch in Hz and normalized velocity.
unsafe fn send_custom_trigger(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    pitch_hz: f32,
    velocity: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_TRIGGER,
            logical_id: gatepitch_lid,
            fvalue: 1.0,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_PITCH,
            logical_id: gatepitch_lid,
            fvalue: pitch_hz,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_VELOCITY,
            logical_id: gatepitch_lid,
            fvalue: velocity,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_GATE,
            logical_id: gatepitch_lid,
            fvalue: 1.0,
        },
    );
}

/// Send a gate-off to a GatePitch node.
unsafe fn send_custom_note_off(lg: *mut LiveGraph, gatepitch_lid: u64) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: gatepitch::PARAM_GATE,
            logical_id: gatepitch_lid,
            fvalue: 0.0,
        },
    );
}

fn custom_pitch_hz(transpose: f32, base_note_offset: f32) -> f32 {
    440.0 * 2f32.powf((transpose + base_note_offset) / 12.0)
}

fn track_engine_id(state: &SequencerState, track_idx: usize) -> Option<usize> {
    let engine_id = state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed);
    if engine_id == u32::MAX {
        None
    } else {
        Some(engine_id as usize)
    }
}

fn instrument_sound_fingerprint(
    state: &SequencerState,
    track_idx: usize,
    engine_id: usize,
    step: Option<usize>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    state.pattern.instrument_base_note_offsets[track_idx]
        .load(Ordering::Relaxed)
        .hash(&mut hasher);

    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let value_bits = if let Some(step_idx) = step {
            slot.plocks
                .get(step_idx, param_idx)
                .unwrap_or_else(|| slot.defaults.get(param_idx))
                .to_bits()
        } else {
            slot.defaults.get(param_idx).to_bits()
        };
        value_bits.hash(&mut hasher);
    }

    hasher.finish()
}

/// Dispatch effect p-locks for all slots in a track's effect chain.
unsafe fn dispatch_effect_chain_for_track(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    step: usize,
) {
    for slot in &state.pattern.effect_chains[track_idx] {
        let node_id = slot.node_id.load(Ordering::Relaxed);
        if node_id == 0 {
            continue;
        }
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..num_params {
            let value = slot
                .plocks
                .get(step, param_idx)
                .unwrap_or_else(|| slot.defaults.get(param_idx));
            let idx = slot.resolve_node_idx(param_idx);
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx,
                    logical_id: node_id as u64,
                    fvalue: value,
                },
            );
        }
    }
}

unsafe fn route_custom_voice_to_track(
    lg: *mut LiveGraph,
    state: &SequencerState,
    engine_id: usize,
    voice_idx: usize,
    track_idx: usize,
) {
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        let lid = state.runtime.engine_route_lids[engine_id][voice_idx][t].load(Ordering::Relaxed);
        if lid == 0 {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: 0,
                logical_id: lid,
                fvalue: if t == track_idx { 1.0 } else { 0.0 },
            },
        );
    }
}

/// Dispatch instrument param values (with p-lock support) to a selected synth node.
unsafe fn dispatch_instrument_params_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    step: usize,
    synth_id: u64,
    modulator_id: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if num_params == 0 {
        return;
    }
    for param_idx in 0..num_params {
        let value = slot
            .plocks
            .get(step, param_idx)
            .unwrap_or_else(|| slot.defaults.get(param_idx));
        let idx = slot.resolve_node_idx(param_idx);
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let logical_id = if is_mod_param { modulator_id } else { synth_id };
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: resolved_idx,
                logical_id,
                fvalue: value,
            },
        );
    }
}

unsafe fn dispatch_instrument_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    synth_id: u64,
    modulator_id: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let logical_id = if is_mod_param { modulator_id } else { synth_id };
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: resolved_idx,
                logical_id,
                fvalue: slot.defaults.get(param_idx),
            },
        );
    }
}

/// Fire a step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
fn fire_step_trigger(data: &mut AudioCallbackData, track_idx: usize, step: usize) {
    let sd = &data.state.pattern.step_data[track_idx];
    let tp = &data.state.pattern.track_params[track_idx];
    let samples_per_step = data.clock.samples_per_step_for_track(track_idx);
    let is_custom = data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed) == 1;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    if !is_custom && sampler_lid == 0 {
        return;
    }

    let chop = sd.get(step, StepParam::Chop).round() as u32;
    let chop = chop.max(1);
    let dur = sd.get(step, StepParam::Duration);

    let total_gate = (dur as f64 * samples_per_step) as f32;
    let chop_gate = total_gate / chop as f32;

    // Envelope params from track params
    let attack_ms = tp.get_attack_ms();
    let release_ms = tp.get_release_ms();
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let velocity = sd.get(step, StepParam::Velocity);
    let base_note_offset =
        f32::from_bits(data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed));

    // Sync polyphonic setting from track params
    let track_polyphonic = tp.is_polyphonic();
    data.voice_pools[track_idx].polyphonic = track_polyphonic;
    let engine_id = if is_custom {
        track_engine_id(&data.state, track_idx)
    } else {
        None
    };

    // Check chord data: if chord has notes, trigger each note on its own voice
    let chord_count = data.state.pattern.chord_data[track_idx].count(step);
    if chord_count > 0 {
        for n in 0..chord_count {
            let transpose = data.state.pattern.chord_data[track_idx].get(step, n);
            if is_custom {
                let Some(engine_id) = engine_id else {
                    continue;
                };
                let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                    track_idx,
                    transpose,
                    track_polyphonic,
                );
                let voice_idx = allocation.voice_idx;
                let lid = allocation.logical_id;
                let fingerprint =
                    instrument_sound_fingerprint(&data.state, track_idx, engine_id, Some(step));
                let synth_id =
                    data.state.runtime.engine_synth_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
                let modulator_id =
                    data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
                if lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
                cancel_gate_off_for_lid(&mut data.gate_off_state, lid);
                if allocation.stole_active_voice || !track_polyphonic {
                    unsafe {
                        send_custom_note_off(data.lg.0, lid);
                        route_custom_voice_to_track(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                &data.state,
                                track_idx,
                                step,
                                synth_id as u64,
                                modulator_id as u64,
                            );
                        }
                    }
                } else {
                    unsafe {
                        route_custom_voice_to_track(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                &data.state,
                                track_idx,
                                step,
                                synth_id as u64,
                                modulator_id as u64,
                            );
                        }
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                unsafe {
                    send_custom_trigger(data.lg.0, lid, pitch_hz, velocity);
                }
                if gate_mode > 0.5 {
                    data.gate_off_state[track_idx].schedule(lid, total_gate as f64);
                }
            } else {
                let voice = data.voice_pools[track_idx].allocate_voice(transpose);
                let voice_lid = voice.logical_id;
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                unsafe {
                    send_trigger(
                        data.lg.0,
                        lid,
                        sd,
                        step,
                        chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose,
                    );
                }
            }
        }
    } else {
        // Single-note mode: use StepParam::Transpose
        let transpose = sd.get(step, StepParam::Transpose);
        if is_custom {
            let Some(engine_id) = engine_id else {
                return;
            };
            let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                track_idx,
                transpose,
                track_polyphonic,
            );
            let voice_idx = allocation.voice_idx;
            let lid = allocation.logical_id;
            let fingerprint =
                instrument_sound_fingerprint(&data.state, track_idx, engine_id, Some(step));
            let synth_id =
                data.state.runtime.engine_synth_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
            let modulator_id =
                data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return;
            }
            let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
            cancel_gate_off_for_lid(&mut data.gate_off_state, lid);
            if allocation.stole_active_voice || !track_polyphonic {
                unsafe {
                    send_custom_note_off(data.lg.0, lid);
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            &data.state,
                            track_idx,
                            step,
                            synth_id as u64,
                            modulator_id as u64,
                        );
                    }
                }
            } else {
                unsafe {
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            &data.state,
                            track_idx,
                            step,
                            synth_id as u64,
                            modulator_id as u64,
                        );
                    }
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
            unsafe {
                send_custom_trigger(data.lg.0, lid, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                data.gate_off_state[track_idx].schedule(lid, total_gate as f64);
            }
        } else {
            let voice = data.voice_pools[track_idx].allocate_voice(transpose);
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            unsafe {
                send_trigger(
                    data.lg.0,
                    lid,
                    sd,
                    step,
                    chop_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose,
                );
            }
        }
    }

    // Update send gain (reverb send amount from track-level param)
    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: tp.get_send(),
                },
            );
        }
    }

    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);

    // Setup chop re-triggers (sampler only — custom instruments handle gate duration internally)
    if !is_custom && chop > 1 {
        data.chop_state[track_idx] = ChopTracker {
            remaining: chop - 1,
            counter: samples_per_step / chop as f64,
            interval: samples_per_step / chop as f64,
            step,
            chop_gate,
        };
    } else {
        data.chop_state[track_idx].remaining = 0;
    }
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let callback_start = Instant::now();
    let nframes = output.len() / data.num_channels;
    let num_tracks = data.state.active_track_count();

    // Sync voice pools for any newly added tracks
    for t in 0..num_tracks {
        let pool = &mut data.voice_pools[t];
        let vc = data.state.runtime.voice_counts[t].load(Ordering::Acquire) as usize;
        if pool.num_voices < vc {
            for v in pool.num_voices..vc {
                let lid = data.state.runtime.voice_lids[t][v].load(Ordering::Acquire);
                if lid != 0 {
                    // We need the node_id too, but we can derive it from lid (they match for new nodes)
                    pool.add_voice(lid, lid as i32);
                }
            }
        }

        if let Some(engine_id) = track_engine_id(&data.state, t) {
            let pool = &mut data.custom_engine_pools[engine_id];
            let vc = data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
            if pool.num_voices < vc {
                for v in pool.num_voices..vc.min(MAX_VOICES) {
                    let lid = data.state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
                    if lid != 0 {
                        pool.add_voice(lid);
                    }
                }
            }
        }
    }

    // Process keyboard triggers
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        if kt.track >= num_tracks {
            continue;
        }
        let is_custom = data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed) == 1;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        data.voice_pools[kt.track].polyphonic = track_polyphonic;
        let base_note_offset = f32::from_bits(
            data.state.pattern.instrument_base_note_offsets[kt.track].load(Ordering::Relaxed),
        );

        if kt.note_off {
            // Note-off: find the voice playing this note and stop it
            if is_custom {
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let pool = &mut data.custom_engine_pools[engine_id];
                if !track_polyphonic {
                    if let Some(lid) = pool.release_voice_by_note(kt.track, kt.transpose) {
                        unsafe {
                            send_custom_note_off(data.lg.0, lid);
                        }
                    }
                    continue;
                }
                if let Some(lid) = pool.release_voice_by_note(kt.track, kt.transpose) {
                    unsafe {
                        send_custom_note_off(data.lg.0, lid);
                    }
                }
            } else {
                let pool = &mut data.voice_pools[kt.track];
                for v in 0..pool.num_voices {
                    if pool.voices[v].active && (pool.voices[v].note - kt.transpose).abs() < 0.01 {
                        let lid = pool.voices[v].logical_id;
                        pool.voices[v].active = false;
                        if lid != 0 {
                            unsafe {
                                params_push_wrapper(
                                    data.lg.0,
                                    ParamMsg {
                                        idx: PARAM_GATE_SAMPLES,
                                        logical_id: lid,
                                        fvalue: 0.0,
                                    },
                                );
                            }
                        }
                        break;
                    }
                }
            }
        } else {
            // Note-on: allocate voice and trigger
            if is_custom {
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let allocation = data.custom_engine_pools[engine_id].allocate_voice(
                    kt.track,
                    kt.transpose,
                    track_polyphonic,
                );
                let voice_idx = allocation.voice_idx;
                let voice_lid = allocation.logical_id;
                let fingerprint =
                    instrument_sound_fingerprint(&data.state, kt.track, engine_id, None);
                let synth_id =
                    data.state.runtime.engine_synth_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
                let modulator_id =
                    data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                let pitch_hz = custom_pitch_hz(kt.transpose, base_note_offset);
                cancel_gate_off_for_lid(&mut data.gate_off_state, voice_lid);
                unsafe {
                    route_custom_voice_to_track(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        kt.track,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
                    {
                        dispatch_instrument_defaults_to_voice(
                            data.lg.0,
                            &data.state,
                            kt.track,
                            synth_id as u64,
                            modulator_id as u64,
                        );
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                if allocation.stole_active_voice || !track_polyphonic {
                    unsafe {
                        send_custom_note_off(data.lg.0, voice_lid);
                    }
                }
                unsafe {
                    send_custom_trigger(data.lg.0, voice_lid, pitch_hz, kt.velocity);
                }
            } else {
                let voice = data.voice_pools[kt.track].allocate_voice(kt.transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
                let tp = &data.state.pattern.track_params[kt.track];
                let attack_samples = tp.get_attack_ms() * data.sample_rate as f32 / 1000.0;
                let release_samples = tp.get_release_ms() * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        kt.transpose,
                        kt.velocity,
                        attack_samples,
                        release_samples,
                        gate_mode,
                    );
                }
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }

    let triggers = data.clock.process_block(nframes, &data.state);

    // Push BPM to all delay nodes only when it changes
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for i in 0..num_tracks {
            let delay_lid = data.state.runtime.delay_lids[i].load(Ordering::Acquire);
            if delay_lid != 0 {
                unsafe {
                    params_push_wrapper(
                        data.lg.0,
                        ParamMsg {
                            idx: delay::DELAY_PARAM_BPM,
                            logical_id: delay_lid,
                            fvalue: bpm_f,
                        },
                    );
                }
            }
        }
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::voice_modulator::PARAM_BPM as u64,
                                logical_id: logical_id as u64,
                                fvalue: bpm_f,
                            },
                        );
                    }
                }
            }
        }
    }

    let mod_reset_counter = data
        .state
        .transport
        .mod_reset_counter
        .load(Ordering::Relaxed);
    if mod_reset_counter != data.last_mod_reset_counter {
        data.last_mod_reset_counter = mod_reset_counter;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::voice_modulator::PARAM_RESET_COUNTER as u64,
                                logical_id: logical_id as u64,
                                fvalue: mod_reset_counter as f32,
                            },
                        );
                    }
                }
            }
        }
    }

    // Process clock triggers (each trigger is now per-track)
    for trigger in &triggers {
        let track_idx = trigger.track;
        let local_step = trigger.step; // already local (derived by clock)
        if data.state.pattern.patterns[track_idx].is_active(local_step) {
            // Dispatch unified effect chain p-locks
            unsafe {
                dispatch_effect_chain_for_track(data.lg.0, &data.state, track_idx, local_step);
            }

            let samples_per_step = data.clock.samples_per_step_for_track(track_idx);
            let tp = &data.state.pattern.track_params[track_idx];
            let swing_pct = tp.get_swing();
            let is_odd_step = local_step % 2 == 1;

            if is_odd_step && swing_pct > 50.0 {
                // Delay this trigger by swing amount
                let swing_delay = (swing_pct as f64 / 100.0 - 0.5) * samples_per_step;
                data.swing_state[track_idx] = SwingPending {
                    countdown: swing_delay,
                    step: local_step,
                    active: true,
                };
            } else {
                fire_step_trigger(data, track_idx, local_step);
            }
        }
    }

    // Process pending swing triggers
    for track_idx in 0..num_tracks {
        let sw = &mut data.swing_state[track_idx];
        if sw.active {
            sw.countdown -= nframes as f64;
            if sw.countdown <= 0.0 {
                sw.active = false;
                let step = sw.step;
                fire_step_trigger(data, track_idx, step);
            }
        }
    }

    // Process pending chop re-triggers (voice-aware)
    for track_idx in 0..num_tracks {
        let cs = &mut data.chop_state[track_idx];
        if cs.remaining > 0 {
            cs.counter -= nframes as f64;
            let tp = &data.state.pattern.track_params[track_idx];
            let attack_samples = tp.get_attack_ms() * data.sample_rate as f32 / 1000.0;
            let release_samples = tp.get_release_ms() * data.sample_rate as f32 / 1000.0;
            let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
            let sd = &data.state.pattern.step_data[track_idx];
            while cs.counter <= 0.0 && cs.remaining > 0 {
                // Allocate a voice for the chop re-trigger
                let transpose = sd.get(cs.step, StepParam::Transpose);
                let voice = data.voice_pools[track_idx].allocate_voice(transpose);
                let voice_lid = voice.logical_id;
                let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                unsafe {
                    send_trigger(
                        data.lg.0,
                        lid,
                        sd,
                        cs.step,
                        cs.chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose,
                    );
                }
                data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
                cs.remaining -= 1;
                cs.counter += cs.interval;
            }
        }
    }

    // Process pending gate-off events for custom instruments
    for track_idx in 0..num_tracks {
        let expired = data.gate_off_state[track_idx].process(nframes);
        for lid in expired {
            if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
                data.custom_engine_pools[engine_id].release_voice_by_logical_id(lid);
            } else {
                data.voice_pools[track_idx].release_voice_by_logical_id(lid);
            }
            unsafe {
                send_custom_note_off(data.lg.0, lid);
            }
        }
    }

    unsafe {
        process_next_block(data.lg.0, output.as_mut_ptr(), nframes as i32);
    }

    // Scan interleaved output for peak levels
    let mut peak_l: f32 = 0.0;
    let mut peak_r: f32 = 0.0;
    let nch = data.num_channels;
    for i in 0..nframes {
        let l = output[i * nch].abs();
        if l > peak_l {
            peak_l = l;
        }
        if nch > 1 {
            let r = output[i * nch + 1].abs();
            if r > peak_r {
                peak_r = r;
            }
        }
    }
    data.state.transport.peak_l.store(peak_l.to_bits(), Ordering::Relaxed);
    data.state.transport.peak_r.store(peak_r.to_bits(), Ordering::Relaxed);

    if nframes > 0 {
        let elapsed_secs = callback_start.elapsed().as_secs_f32();
        let block_budget_secs = nframes as f32 / data.sample_rate as f32;
        let raw_load_pct = if block_budget_secs > 0.0 {
            (elapsed_secs / block_budget_secs) * 100.0
        } else {
            0.0
        };
        let prev_load_pct = f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed));
        let smoothed_load_pct = if prev_load_pct <= 0.0 {
            raw_load_pct
        } else {
            prev_load_pct * 0.97 + raw_load_pct * 0.03
        };
        data.state
            .transport.cpu_load_pct
            .store(smoothed_load_pct.to_bits(), Ordering::Relaxed);
    }
}

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
) -> Result<Stream, String> {
    let clock = SequencerClock::new(sample_rate, state.transport.bpm.load(Ordering::Relaxed));

    let chop_state = (0..MAX_TRACKS)
        .map(|_| ChopTracker {
            remaining: 0,
            counter: 0.0,
            interval: 0.0,
            step: 0,
            chop_gate: 0.0,
        })
        .collect();

    let swing_state = (0..MAX_TRACKS)
        .map(|_| SwingPending {
            countdown: 0.0,
            step: 0,
            active: false,
        })
        .collect();

    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> = (0..MAX_TRACKS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> =
        (0..MAX_TRACKS).map(|_| CustomEnginePool::new()).collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        let vc = state.runtime.voice_counts[t].load(Ordering::Acquire) as usize;
        for v in 0..vc.min(MAX_VOICES) {
            let lid = state.runtime.voice_lids[t][v].load(Ordering::Acquire);
            if lid != 0 {
                voice_pools[t].add_voice(lid, lid as i32);
            }
        }

        if let Some(engine_id) = track_engine_id(&state, t) {
            let vc = state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
            for v in custom_engine_pools[engine_id].num_voices..vc.min(MAX_VOICES) {
                let lid = state.runtime.engine_voice_lids[engine_id][v].load(Ordering::Acquire);
                if lid != 0 {
                    custom_engine_pools[engine_id].add_voice(lid);
                }
            }
        }
    }

    let gate_off_state = (0..MAX_TRACKS).map(|_| GateOffTracker::new()).collect();

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        clock,
        state,
        num_channels,
        chop_state,
        swing_state,
        gate_off_state,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        keyboard_rx,
    };

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;

    let config = cpal::StreamConfig {
        channels: num_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(block_size as u32),
    };

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(&mut cb_data, data);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Query the default output device for sample rate and channel count.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    Ok((config.sample_rate().0, config.channels()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::{instrument_sound_fingerprint, CustomEnginePool, GateOffTracker};
    use crate::sequencer::SequencerState;

    #[test]
    fn custom_engine_pool_prefers_inactive_voices_before_stealing() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=4 {
            pool.add_voice(lid);
        }

        let a = pool.allocate_voice(0, 0.0, true);
        let b = pool.allocate_voice(0, 4.0, true);
        assert_eq!(a.logical_id, 1);
        assert_eq!(b.logical_id, 2);
        assert!(!a.stole_active_voice);
        assert!(!b.stole_active_voice);

        pool.release_voice_by_logical_id(a.logical_id);
        pool.release_voice_by_logical_id(b.logical_id);

        let c = pool.allocate_voice(0, 7.0, true);
        let d = pool.allocate_voice(0, 11.0, true);
        assert_eq!(c.logical_id, 3);
        assert_eq!(d.logical_id, 4);
        assert!(!c.stole_active_voice);
        assert!(!d.stole_active_voice);
    }

    #[test]
    fn custom_engine_pool_steals_same_tracks_active_voice_first() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=2 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_voice(0, 0.0, true);
        let second = pool.allocate_voice(1, 4.0, true);
        assert_eq!(first.logical_id, 1);
        assert_eq!(second.logical_id, 2);

        let stolen = pool.allocate_voice(1, 7.0, true);
        assert!(stolen.stole_active_voice);
        assert_eq!(stolen.previous_track, Some(1));
        assert_eq!(stolen.logical_id, 2);
    }

    #[test]
    fn custom_engine_pool_mono_reuses_same_tracks_voice_and_marks_active_steal() {
        let mut pool = CustomEnginePool::new();
        for lid in 1..=2 {
            pool.add_voice(lid);
        }

        let first = pool.allocate_voice(3, 0.0, false);
        let reused = pool.allocate_voice(3, 12.0, false);

        assert_eq!(reused.logical_id, first.logical_id);
        assert!(reused.stole_active_voice);
        assert_eq!(reused.previous_track, Some(3));
    }

    #[test]
    fn gate_off_tracker_cancel_removes_matching_pending_lids() {
        let mut tracker = GateOffTracker::new();
        tracker.schedule(10, 100.0);
        tracker.schedule(20, 100.0);
        tracker.cancel(10);

        let expired = tracker.process(200);
        assert_eq!(expired, vec![20]);
    }

    #[test]
    fn sound_fingerprint_changes_when_step_sound_changes() {
        let state = Arc::new(SequencerState::new(1, Vec::new()));
        state.runtime.track_engine_ids[0].store(2, Ordering::Relaxed);
        state.pattern.instrument_slots[0]
            .num_params
            .store(2, Ordering::Relaxed);
        state.pattern.instrument_slots[0].defaults.set(0, 0.2);
        state.pattern.instrument_slots[0].defaults.set(1, 0.4);

        let base = instrument_sound_fingerprint(&state, 0, 2, Some(3));
        state.pattern.instrument_slots[0].plocks.set(3, 1, 0.9);
        let changed = instrument_sound_fingerprint(&state, 0, 2, Some(3));

        assert_ne!(base, changed);
    }

    #[test]
    fn sound_fingerprint_changes_when_base_note_changes() {
        let state = Arc::new(SequencerState::new(1, Vec::new()));
        state.runtime.track_engine_ids[0].store(5, Ordering::Relaxed);
        state.pattern.instrument_slots[0]
            .num_params
            .store(1, Ordering::Relaxed);
        state.pattern.instrument_slots[0].defaults.set(0, 0.5);

        let base = instrument_sound_fingerprint(&state, 0, 5, None);
        state.pattern.instrument_base_note_offsets[0].store(12.0f32.to_bits(), Ordering::Relaxed);
        let changed = instrument_sound_fingerprint(&state, 0, 5, None);

        assert_ne!(base, changed);
    }
}
