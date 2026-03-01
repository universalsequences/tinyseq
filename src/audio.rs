use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::Arc;

use crate::audiograph::*;
use crate::sampler::{
    SamplerTrack, PARAM_GATE_SAMPLES, PARAM_PLAYHEAD, PARAM_SPEED, PARAM_TRANSPOSE, PARAM_TRIGGER,
    PARAM_VELOCITY,
};
use crate::sequencer::{SequencerClock, SequencerState, StepParam};

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

struct AudioCallbackData {
    lg: LiveGraphPtr,
    clock: SequencerClock,
    state: Arc<SequencerState>,
    tracks: Vec<SamplerTrack>,
    num_channels: usize,
    chop_state: Vec<ChopTracker>,
}

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    tracks: Vec<SamplerTrack>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
) -> Result<Stream, String> {
    let clock = SequencerClock::new(
        sample_rate,
        state.bpm.load(std::sync::atomic::Ordering::Relaxed),
    );

    let num_tracks = tracks.len();
    let chop_state = (0..num_tracks)
        .map(|_| ChopTracker {
            remaining: 0,
            counter: 0.0,
            interval: 0.0,
            step: 0,
            chop_gate: 0.0,
        })
        .collect();

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        clock,
        state,
        tracks,
        num_channels,
        chop_state,
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

    stream.play().map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Send a trigger to the sampler with the given per-step params and gate length.
unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    sd: &crate::sequencer::StepData,
    step: usize,
    gate_samples: f32,
) {
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_VELOCITY, logical_id: lid, fvalue: sd.get(step, StepParam::Velocity) },
    );
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_SPEED, logical_id: lid, fvalue: sd.get(step, StepParam::Speed) },
    );
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_GATE_SAMPLES, logical_id: lid, fvalue: gate_samples },
    );
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_TRANSPOSE, logical_id: lid, fvalue: sd.get(step, StepParam::Transpose) },
    );
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_PLAYHEAD, logical_id: lid, fvalue: 0.0 },
    );
    params_push_wrapper(
        lg,
        ParamMsg { idx: PARAM_TRIGGER, logical_id: lid, fvalue: 1.0 },
    );
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let nframes = output.len() / data.num_channels;

    let triggers = data.clock.process_block(nframes, &data.state);
    let samples_per_step = data.clock.current_samples_per_step();

    // Process clock triggers (initial triggers for each step)
    for trigger in &triggers {
        for (track_idx, track) in data.tracks.iter().enumerate() {
            if data.state.patterns[track_idx].is_active(trigger.step) {
                let sd = &data.state.step_data[track_idx];
                let chop = sd.get(trigger.step, StepParam::Chop).round() as u32;
                let chop = chop.max(1);
                let dur = sd.get(trigger.step, StepParam::Duration);

                // Gate time in samples: duration * step_length, divided by chops
                let total_gate = (dur as f64 * samples_per_step) as f32;
                let chop_gate = total_gate / chop as f32;

                // Fire the initial trigger with gate length in samples
                unsafe {
                    send_trigger(data.lg.0, track.logical_id, sd, trigger.step, chop_gate);
                }

                // Set up chop re-triggers if chop > 1
                if chop > 1 {
                    data.chop_state[track_idx] = ChopTracker {
                        remaining: chop - 1,
                        counter: samples_per_step / chop as f64,
                        interval: samples_per_step / chop as f64,
                        step: trigger.step,
                        chop_gate: chop_gate,
                    };
                } else {
                    data.chop_state[track_idx].remaining = 0;
                }
            }
        }
    }

    // Process pending chop re-triggers
    for (track_idx, track) in data.tracks.iter().enumerate() {
        let cs = &mut data.chop_state[track_idx];
        if cs.remaining > 0 {
            cs.counter -= nframes as f64;
            while cs.counter <= 0.0 && cs.remaining > 0 {
                let sd = &data.state.step_data[track_idx];
                unsafe {
                    send_trigger(data.lg.0, track.logical_id, sd, cs.step, cs.chop_gate);
                }
                cs.remaining -= 1;
                cs.counter += cs.interval;
            }
        }
    }

    unsafe {
        process_next_block(data.lg.0, output.as_mut_ptr(), nframes as i32);
    }
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
