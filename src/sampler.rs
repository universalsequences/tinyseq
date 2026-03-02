use crate::audiograph::*;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::path::Path;

// State layout indices (f32 slots)
const STATE_BUFFER_ID: usize = 0;
const STATE_PLAYHEAD: usize = 1;
const STATE_PLAYING: usize = 2;
const STATE_GAIN: usize = 3;
const STATE_VELOCITY: usize = 4;
const STATE_SPEED: usize = 5;
const STATE_GATE_SAMPLES: usize = 6; // absolute gate length in samples (computed by audio callback)
const STATE_TRANSPOSE: usize = 7;
const STATE_ATTACK_SAMPLES: usize = 8;  // musical attack in samples
const STATE_RELEASE_SAMPLES: usize = 9; // musical release in samples
const STATE_GATE_MODE: usize = 10;      // 1.0=gate on, 0.0=gate off
const SAMPLER_STATE_SIZE: usize = 11;

// Param indices (match state layout for direct write)
pub const PARAM_PLAYHEAD: u64 = STATE_PLAYHEAD as u64;
pub const PARAM_TRIGGER: u64 = STATE_PLAYING as u64;
pub const PARAM_VELOCITY: u64 = STATE_VELOCITY as u64;
pub const PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const PARAM_GATE_SAMPLES: u64 = STATE_GATE_SAMPLES as u64;
pub const PARAM_TRANSPOSE: u64 = STATE_TRANSPOSE as u64;
pub const PARAM_ATTACK_SAMPLES: u64 = STATE_ATTACK_SAMPLES as u64;
pub const PARAM_RELEASE_SAMPLES: u64 = STATE_RELEASE_SAMPLES as u64;
pub const PARAM_GATE_MODE: u64 = STATE_GATE_MODE as u64;

pub struct SamplerTrack {
    pub name: String,
    pub node_id: i32,
    pub logical_id: u64,
}

/// extern "C" init — called by audiograph when node is created.
unsafe extern "C" fn sampler_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    if !initial_state.is_null() {
        let init = initial_state as *const f32;
        *s.add(STATE_BUFFER_ID) = *init.add(0);
    }
    *s.add(STATE_PLAYHEAD) = 0.0;
    *s.add(STATE_PLAYING) = 0.0;
    *s.add(STATE_GAIN) = 0.8;
    *s.add(STATE_VELOCITY) = 1.0;
    *s.add(STATE_SPEED) = 1.0;
    *s.add(STATE_GATE_SAMPLES) = f32::MAX; // ungated by default until first trigger
    *s.add(STATE_TRANSPOSE) = 0.0;
    *s.add(STATE_ATTACK_SAMPLES) = 0.0;
    *s.add(STATE_RELEASE_SAMPLES) = 0.0;
    *s.add(STATE_GATE_MODE) = 1.0; // gate on by default
}

/// extern "C" process — reads sample data from buffer, writes to output.
/// Supports variable-rate playback with linear interpolation.
unsafe extern "C" fn sampler_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let buffer_id = (*s.add(STATE_BUFFER_ID)) as usize;
    let mut playhead = *s.add(STATE_PLAYHEAD);
    let playing = *s.add(STATE_PLAYING);
    let gain = *s.add(STATE_GAIN);
    let velocity = *s.add(STATE_VELOCITY);
    let speed = *s.add(STATE_SPEED);
    let gate_samples = *s.add(STATE_GATE_SAMPLES);
    let transpose = *s.add(STATE_TRANSPOSE);
    let musical_attack = *s.add(STATE_ATTACK_SAMPLES);
    let musical_release = *s.add(STATE_RELEASE_SAMPLES);
    let gate_mode = *s.add(STATE_GATE_MODE);

    let buf_desc = buffers as *const BufferDesc;
    let desc = &*buf_desc.add(buffer_id);
    let sample_data = desc.buffer;
    let sample_len = desc.size as usize;

    let out0 = *out.add(0);
    let nf = nframes as usize;

    if playing <= 0.0 || sample_data.is_null() || sample_len == 0 {
        for i in 0..nf {
            *out0.add(i) = 0.0;
        }
        return;
    }

    // Compute effective playback rate: speed * 2^(transpose/12)
    let effective_rate = speed * (2.0_f32).powf(transpose / 12.0);

    // Gate cutoff depends on gate mode
    let max_playhead = if gate_mode > 0.5 {
        // Gate ON: respect duration gating
        gate_samples.min(sample_len as f32)
    } else {
        // Gate OFF: play full sample
        sample_len as f32
    };

    // Amplitude: velocity * gain
    let amplitude = velocity * gain;

    // Click prevention envelope (always on, hardcoded)
    const CLICK_ATTACK: f32 = 128.0;
    const CLICK_RELEASE: f32 = 256.0;
    let click_release_start = (max_playhead - CLICK_RELEASE).max(0.0);

    // Musical envelope (user-controlled attack/release)
    let mus_release_start = (max_playhead - musical_release).max(0.0);

    for i in 0..nf {
        if playhead >= max_playhead || playhead < 0.0 {
            *out0.add(i) = 0.0;
            *s.add(STATE_PLAYING) = 0.0;
            for j in (i + 1)..nf {
                *out0.add(j) = 0.0;
            }
            break;
        }

        // Linear interpolation for non-integer playhead
        let idx = playhead as usize;
        let frac = playhead - idx as f32;

        let s0 = if idx < sample_len {
            *sample_data.add(idx)
        } else {
            0.0
        };
        let s1 = if idx + 1 < sample_len {
            *sample_data.add(idx + 1)
        } else {
            0.0
        };

        let sample = s0 + frac * (s1 - s0);

        // Click prevention envelope
        let click_env = if playhead < CLICK_ATTACK {
            playhead / CLICK_ATTACK
        } else if playhead > click_release_start {
            ((max_playhead - playhead) / CLICK_RELEASE).max(0.0)
        } else {
            1.0
        };

        // Musical envelope (only applies if attack or release > 0)
        let mus_env = if musical_attack > 0.0 && playhead < musical_attack {
            playhead / musical_attack
        } else if musical_release > 0.0 && playhead > mus_release_start {
            ((max_playhead - playhead) / musical_release).max(0.0)
        } else {
            1.0
        };

        *out0.add(i) = sample * amplitude * click_env * mus_env;

        playhead += effective_rate;
    }

    *s.add(STATE_PLAYHEAD) = playhead;
}

pub fn sampler_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(sampler_process),
        init: Some(sampler_init),
        reset: None,
        migrate: None,
    }
}

/// Load a WAV file, create an audiograph buffer and sampler node.
pub fn create_sampler_track(
    lg: *mut LiveGraph,
    wav_path: &Path,
) -> Result<SamplerTrack, String> {
    let reader =
        hound::WavReader::open(wav_path).map_err(|e| format!("Failed to open WAV: {e}"))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;

    let samples_f32: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1u32 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max_val)
                .collect()
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    let mono: Vec<f32> = if channels > 1 {
        samples_f32
            .chunks(channels)
            .map(|ch| ch.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples_f32
    };

    let buffer_id = unsafe { create_buffer(lg, mono.len() as c_int, 1, mono.as_ptr()) };
    if buffer_id < 0 {
        return Err("Failed to create buffer".to_string());
    }

    let initial_state = [buffer_id as f32];

    let name = wav_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sample")
        .to_string();
    let c_name = CString::new(name.as_str()).unwrap();

    let node_id = unsafe {
        add_node(
            lg,
            sampler_vtable(),
            SAMPLER_STATE_SIZE * std::mem::size_of::<f32>(),
            c_name.as_ptr(),
            0,
            1,
            initial_state.as_ptr() as *const c_void,
            initial_state.len() * std::mem::size_of::<f32>(),
        )
    };

    if node_id < 0 {
        return Err("Failed to add sampler node".to_string());
    }

    Ok(SamplerTrack {
        name,
        node_id,
        logical_id: node_id as u64,
    })
}
