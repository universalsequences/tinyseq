use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::audiograph::*;
use crate::delay;
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

struct AudioCallbackData {
    lg: LiveGraphPtr,
    clock: SequencerClock,
    state: Arc<SequencerState>,
    num_channels: usize,
    chop_state: Vec<ChopTracker>,
    swing_state: Vec<SwingPending>,
    sample_rate: f64,
    last_bpm: u32,
    voice_pools: Vec<VoicePool>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
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
    params_push_wrapper(lg, ParamMsg { idx: PARAM_VELOCITY, logical_id: lid, fvalue: velocity });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_SPEED, logical_id: lid, fvalue: 1.0 });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_GATE_SAMPLES, logical_id: lid, fvalue: f32::MAX });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_TRANSPOSE, logical_id: lid, fvalue: transpose });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_ATTACK_SAMPLES, logical_id: lid, fvalue: attack_samples });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_RELEASE_SAMPLES, logical_id: lid, fvalue: release_samples });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_GATE_MODE, logical_id: lid, fvalue: gate_mode });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_PLAYHEAD, logical_id: lid, fvalue: 0.0 });
    params_push_wrapper(lg, ParamMsg { idx: PARAM_TRIGGER, logical_id: lid, fvalue: 1.0 });
}

/// Dispatch effect p-locks for all slots in a track's effect chain.
unsafe fn dispatch_effect_chain_for_track(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    step: usize,
) {
    for slot in &state.effect_chains[track_idx] {
        let node_id = slot.node_id.load(Ordering::Relaxed);
        if node_id == 0 {
            continue;
        }
        let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..num_params {
            let value = slot.plocks.get(step, param_idx)
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

/// Fire a step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
fn fire_step_trigger(data: &mut AudioCallbackData, track_idx: usize, step: usize) {
    let sampler_lid = data.state.sampler_lids[track_idx].load(Ordering::Acquire);
    if sampler_lid == 0 {
        return;
    }
    let sd = &data.state.step_data[track_idx];
    let tp = &data.state.track_params[track_idx];
    let samples_per_step = data.clock.current_samples_per_step();

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

    // Sync polyphonic setting from track params
    data.voice_pools[track_idx].polyphonic = tp.is_polyphonic();

    // Check chord data: if chord has notes, trigger each note on its own voice
    let chord_count = data.state.chord_data[track_idx].count(step);
    if chord_count > 0 {
        for n in 0..chord_count {
            let transpose = data.state.chord_data[track_idx].get(step, n);
            let voice = data.voice_pools[track_idx].allocate_voice(transpose);
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 { voice_lid } else { sampler_lid };
            unsafe {
                send_trigger(
                    data.lg.0, lid, sd, step,
                    chop_gate, attack_samples, release_samples, gate_mode,
                    transpose,
                );
            }
        }
    } else {
        // Single-note mode: use StepParam::Transpose
        let transpose = sd.get(step, StepParam::Transpose);
        let voice = data.voice_pools[track_idx].allocate_voice(transpose);
        let voice_lid = voice.logical_id;
        let lid = if voice_lid != 0 { voice_lid } else { sampler_lid };
        unsafe {
            send_trigger(
                data.lg.0, lid, sd, step,
                chop_gate, attack_samples, release_samples, gate_mode,
                transpose,
            );
        }
    }

    // Update send gain (reverb send amount from track-level param)
    let send_lid = data.state.send_lids[track_idx].load(Ordering::Acquire);
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

    data.state.trigger_flash[track_idx].store(255, Ordering::Relaxed);

    // Setup chop re-triggers
    if chop > 1 {
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
    let nframes = output.len() / data.num_channels;
    let num_tracks = data.state.active_track_count();

    // Sync voice pools for any newly added tracks
    for t in 0..num_tracks {
        let pool = &mut data.voice_pools[t];
        let vc = data.state.voice_counts[t].load(Ordering::Acquire) as usize;
        if pool.num_voices < vc {
            for v in pool.num_voices..vc {
                let lid = data.state.voice_lids[t][v].load(Ordering::Acquire);
                if lid != 0 {
                    // We need the node_id too, but we can derive it from lid (they match for new nodes)
                    pool.add_voice(lid, lid as i32);
                }
            }
        }
    }

    // Process keyboard triggers
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        if kt.track >= num_tracks {
            continue;
        }
        data.voice_pools[kt.track].polyphonic = data.state.track_params[kt.track].is_polyphonic();

        if kt.note_off {
            // Note-off: find the voice playing this note and stop it
            let pool = &mut data.voice_pools[kt.track];
            // Find the voice with matching note and stop it by setting trigger=0
            for v in 0..pool.num_voices {
                if pool.voices[v].active && (pool.voices[v].note - kt.transpose).abs() < 0.01 {
                    let lid = pool.voices[v].logical_id;
                    pool.voices[v].active = false;
                    if lid != 0 {
                        unsafe {
                            // Set gate to 0 samples to trigger release envelope
                            params_push_wrapper(
                                data.lg.0,
                                ParamMsg { idx: PARAM_GATE_SAMPLES, logical_id: lid, fvalue: 0.0 },
                            );
                        }
                    }
                    break;
                }
            }
        } else {
            // Note-on: allocate voice and trigger
            let voice = data.voice_pools[kt.track].allocate_voice(kt.transpose);
            let voice_lid = voice.logical_id;
            if voice_lid == 0 {
                continue;
            }
            let tp = &data.state.track_params[kt.track];
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
            data.state.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }

    let triggers = data.clock.process_block(nframes, &data.state);
    let samples_per_step = data.clock.current_samples_per_step();

    // Push BPM to all delay nodes only when it changes
    let bpm = data.state.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for i in 0..num_tracks {
            let delay_lid = data.state.delay_lids[i].load(Ordering::Acquire);
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
    }

    // Process clock triggers
    for trigger in &triggers {
        for track_idx in 0..num_tracks {
            let num_steps = data.state.track_params[track_idx].get_num_steps();
            let local_step = trigger.step % num_steps;
            if data.state.patterns[track_idx].is_active(local_step) {
                // Dispatch unified effect chain p-locks
                unsafe {
                    dispatch_effect_chain_for_track(
                        data.lg.0,
                        &data.state,
                        track_idx,
                        local_step,
                    );
                }

                let tp = &data.state.track_params[track_idx];
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
            let tp = &data.state.track_params[track_idx];
            let attack_samples = tp.get_attack_ms() * data.sample_rate as f32 / 1000.0;
            let release_samples = tp.get_release_ms() * data.sample_rate as f32 / 1000.0;
            let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
            let sd = &data.state.step_data[track_idx];
            while cs.counter <= 0.0 && cs.remaining > 0 {
                // Allocate a voice for the chop re-trigger
                let transpose = sd.get(cs.step, StepParam::Transpose);
                let voice = data.voice_pools[track_idx].allocate_voice(transpose);
                let voice_lid = voice.logical_id;
                let sampler_lid = data.state.sampler_lids[track_idx].load(Ordering::Acquire);
                let lid = if voice_lid != 0 { voice_lid } else { sampler_lid };
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
                data.state.trigger_flash[track_idx].store(255, Ordering::Relaxed);
                cs.remaining -= 1;
                cs.counter += cs.interval;
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
    data.state.peak_l.store(peak_l.to_bits(), Ordering::Relaxed);
    data.state.peak_r.store(peak_r.to_bits(), Ordering::Relaxed);
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
    let clock = SequencerClock::new(sample_rate, state.bpm.load(Ordering::Relaxed));

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
    let mut voice_pools: Vec<VoicePool> = (0..MAX_TRACKS)
        .map(|_| VoicePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        let vc = state.voice_counts[t].load(Ordering::Acquire) as usize;
        for v in 0..vc.min(MAX_VOICES) {
            let lid = state.voice_lids[t][v].load(Ordering::Acquire);
            if lid != 0 {
                voice_pools[t].add_voice(lid, lid as i32);
            }
        }
    }

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        clock,
        state,
        num_channels,
        chop_state,
        swing_state,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        voice_pools,
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
