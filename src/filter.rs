use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// State layout indices (f32 slots)
const STATE_ENABLED: usize = 0;
const STATE_MODE: usize = 1;      // 0=LP, 1=HP, 2=BP
const STATE_CUTOFF: usize = 2;    // Hz
const STATE_RESONANCE: usize = 3;
const STATE_IC1EQ: usize = 4;     // SVF integrator state
const STATE_IC2EQ: usize = 5;
const STATE_SMOOTH_CUTOFF: usize = 6;
const STATE_SMOOTH_RESO: usize = 7;
const STATE_SAMPLE_RATE: usize = 8;
pub const FILTER_STATE_SIZE: usize = 9;

// Param indices for external control
pub const FILTER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const FILTER_PARAM_MODE: u64 = STATE_MODE as u64;
pub const FILTER_PARAM_CUTOFF: u64 = STATE_CUTOFF as u64;
pub const FILTER_PARAM_RESONANCE: u64 = STATE_RESONANCE as u64;

unsafe extern "C" fn filter_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    let s = state as *mut f32;
    let _ = initial_state;
    *s.add(STATE_ENABLED) = 0.0;
    *s.add(STATE_MODE) = 0.0;
    *s.add(STATE_CUTOFF) = 1000.0;
    *s.add(STATE_RESONANCE) = 1.0;
    *s.add(STATE_IC1EQ) = 0.0;
    *s.add(STATE_IC2EQ) = 0.0;
    *s.add(STATE_SMOOTH_CUTOFF) = 1000.0;
    *s.add(STATE_SMOOTH_RESO) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
}

unsafe extern "C" fn filter_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let enabled = *s.add(STATE_ENABLED);
    let nf = nframes as usize;

    let in0 = *inp.add(0);
    let out0 = *out.add(0);

    if enabled <= 0.5 {
        // Bypass: pass-through and reset integrator state to avoid click on re-enable
        *s.add(STATE_IC1EQ) = 0.0;
        *s.add(STATE_IC2EQ) = 0.0;
        for i in 0..nf {
            *out0.add(i) = *in0.add(i);
        }
        return;
    }

    let mode = (*s.add(STATE_MODE)).round() as i32;
    let target_cutoff = *s.add(STATE_CUTOFF);
    let target_reso = *s.add(STATE_RESONANCE);
    let sr = *s.add(STATE_SAMPLE_RATE);
    let mut smooth_cutoff = *s.add(STATE_SMOOTH_CUTOFF);
    let mut smooth_reso = *s.add(STATE_SMOOTH_RESO);
    let mut ic1eq = *s.add(STATE_IC1EQ);
    let mut ic2eq = *s.add(STATE_IC2EQ);

    // One-pole smoothing coefficient (~20Hz)
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sr).exp();

    for i in 0..nf {
        // Smooth parameters
        smooth_cutoff += smooth_coeff * (target_cutoff - smooth_cutoff);
        smooth_reso += smooth_coeff * (target_reso - smooth_reso);

        // SVF coefficients: k = 1/Q, where Q = resonance value
        // Higher resonance = higher Q = lower k = more resonant
        let g = (std::f32::consts::PI * smooth_cutoff / sr).tan();
        let k = 1.0 / smooth_reso.max(0.5);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v0 = *in0.add(i);
        let v3 = v0 - ic2eq;
        let v1 = a1 * ic1eq + a2 * v3;
        let v2 = ic2eq + a2 * ic1eq + a3 * v3;

        ic1eq = 2.0 * v1 - ic1eq;
        ic2eq = 2.0 * v2 - ic2eq;

        let output = match mode {
            0 => v2,                    // lowpass
            1 => v0 - k * v1 - v2,     // highpass
            2 => v1,                    // bandpass
            _ => v2,
        };

        *out0.add(i) = output;
    }

    *s.add(STATE_IC1EQ) = ic1eq;
    *s.add(STATE_IC2EQ) = ic2eq;
    *s.add(STATE_SMOOTH_CUTOFF) = smooth_cutoff;
    *s.add(STATE_SMOOTH_RESO) = smooth_reso;
}

pub fn filter_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(filter_process),
        init: Some(filter_init),
        reset: None,
        migrate: None,
    }
}
