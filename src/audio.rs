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
use crate::sequencer::{SequencerClock, SequencerState, StepParam};

/// Holds all node IDs for a single track's signal chain.
pub struct TrackNodes {
    pub name: String,
    pub sampler_lid: u64,
    pub filter_lid: u64,
    pub delay_lid: u64,
}

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
    track_nodes: Vec<TrackNodeIds>,
    num_channels: usize,
    chop_state: Vec<ChopTracker>,
    swing_state: Vec<SwingPending>,
    sample_rate: f64,
}

/// Minimal copy of node IDs needed in audio thread.
struct TrackNodeIds {
    sampler_lid: u64,
    delay_lid: u64,
}

/// Send a trigger to the sampler with the given per-step params and gate length.
unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    sd: &crate::sequencer::StepData,
    step: usize,
    gate_samples: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
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
            fvalue: sd.get(step, StepParam::Transpose),
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
fn fire_step_trigger(data: &mut AudioCallbackData, track_idx: usize, step: usize) {
    let tn = &data.track_nodes[track_idx];
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

    unsafe {
        send_trigger(
            data.lg.0,
            tn.sampler_lid,
            sd,
            step,
            chop_gate,
            attack_samples,
            release_samples,
            gate_mode,
        );
    }

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

    let triggers = data.clock.process_block(nframes, &data.state);
    let samples_per_step = data.clock.current_samples_per_step();

    // Push BPM to all delay nodes (slot index 1 by convention)
    let bpm = data.state.bpm.load(Ordering::Relaxed) as f32;
    for tn in &data.track_nodes {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: delay::DELAY_PARAM_BPM,
                    logical_id: tn.delay_lid,
                    fvalue: bpm,
                },
            );
        }
    }

    // Process clock triggers
    for trigger in &triggers {
        for track_idx in 0..data.track_nodes.len() {
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
    for track_idx in 0..data.track_nodes.len() {
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

    // Process pending chop re-triggers
    for track_idx in 0..data.track_nodes.len() {
        let cs = &mut data.chop_state[track_idx];
        if cs.remaining > 0 {
            cs.counter -= nframes as f64;
            let tn = &data.track_nodes[track_idx];
            let sd = &data.state.step_data[track_idx];
            let tp = &data.state.track_params[track_idx];
            let attack_samples = tp.get_attack_ms() * data.sample_rate as f32 / 1000.0;
            let release_samples = tp.get_release_ms() * data.sample_rate as f32 / 1000.0;
            let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
            while cs.counter <= 0.0 && cs.remaining > 0 {
                unsafe {
                    send_trigger(
                        data.lg.0,
                        tn.sampler_lid,
                        sd,
                        cs.step,
                        cs.chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                    );
                }
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
    track_nodes: &[TrackNodes],
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
) -> Result<Stream, String> {
    let clock = SequencerClock::new(sample_rate, state.bpm.load(Ordering::Relaxed));

    let num_tracks = track_nodes.len();

    let audio_track_nodes: Vec<TrackNodeIds> = track_nodes
        .iter()
        .map(|tn| TrackNodeIds {
            sampler_lid: tn.sampler_lid,
            delay_lid: tn.delay_lid,
        })
        .collect();

    let chop_state = (0..num_tracks)
        .map(|_| ChopTracker {
            remaining: 0,
            counter: 0.0,
            interval: 0.0,
            step: 0,
            chop_gate: 0.0,
        })
        .collect();

    let swing_state = (0..num_tracks)
        .map(|_| SwingPending {
            countdown: 0.0,
            step: 0,
            active: false,
        })
        .collect();

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        clock,
        state,
        track_nodes: audio_track_nodes,
        num_channels,
        chop_state,
        swing_state,
        sample_rate: sample_rate as f64,
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
