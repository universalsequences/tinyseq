use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;
use crate::effects::{ParamDescriptor, ParamKind, ParamScaling};

// Inputs: gate, pitch_hz, velocity, trigger
// Outputs: mod1, mod2, mod3, mod4, mod5, mod6
pub const NUM_OUTPUTS: usize = 6;
pub const MOD_PARAM_BASE: u32 = 1_000_000;

const IDX_LFO1_PHASE: usize = 0;
const IDX_LFO2_PHASE: usize = 1;
const IDX_LFO3_PHASE: usize = 2;
const IDX_ENV2: usize = 3;
const IDX_RAND_HOLD: usize = 4;
const IDX_DRIFT: usize = 5;
const IDX_PREV_GATE: usize = 6;
const IDX_RNG: usize = 7;
const IDX_RAND_SMOOTH: usize = 8;

const PARAM_LFO1_RATE_HZ: usize = 9;
const PARAM_LFO2_RATE_HZ: usize = 10;
const PARAM_LFO3_RATE_HZ: usize = 11;
const PARAM_ENV_ATTACK_MS: usize = 12;
const PARAM_ENV_DECAY_MS: usize = 13;
const PARAM_ENV_SUSTAIN: usize = 14;
const PARAM_ENV_RELEASE_MS: usize = 15;
const PARAM_RAND_SLEW: usize = 16;
const PARAM_DRIFT_RATE: usize = 17;
const PARAM_MOD1_DEPTH: usize = 18;
const PARAM_MOD2_DEPTH: usize = 19;
const PARAM_MOD3_DEPTH: usize = 20;
const PARAM_MOD4_DEPTH: usize = 21;
const PARAM_MOD5_DEPTH: usize = 22;
const PARAM_MOD6_DEPTH: usize = 23;

pub const PARAM_COUNT: usize = 15;
pub const STATE_SIZE: usize = 24;

fn next_rand(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    let v = ((*state >> 8) as f32) / ((u32::MAX >> 8) as f32);
    v * 2.0 - 1.0
}

fn triangle(phase: f32) -> f32 {
    1.0 - 4.0 * (phase - 0.5).abs()
}

unsafe fn init_param_defaults(s: *mut f32) {
    *s.add(PARAM_LFO1_RATE_HZ) = 5.0;
    *s.add(PARAM_LFO2_RATE_HZ) = 1.7;
    *s.add(PARAM_LFO3_RATE_HZ) = 0.37;
    *s.add(PARAM_ENV_ATTACK_MS) = 6.0;
    *s.add(PARAM_ENV_DECAY_MS) = 180.0;
    *s.add(PARAM_ENV_SUSTAIN) = 0.55;
    *s.add(PARAM_ENV_RELEASE_MS) = 240.0;
    *s.add(PARAM_RAND_SLEW) = 0.0;
    *s.add(PARAM_DRIFT_RATE) = 0.00035;
    *s.add(PARAM_MOD1_DEPTH) = 1.0;
    *s.add(PARAM_MOD2_DEPTH) = 1.0;
    *s.add(PARAM_MOD3_DEPTH) = 1.0;
    *s.add(PARAM_MOD4_DEPTH) = 1.0;
    *s.add(PARAM_MOD5_DEPTH) = 1.0;
    *s.add(PARAM_MOD6_DEPTH) = 1.0;
}

pub fn param_descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor {
            name: "mod_lfo1_rate".to_string(),
            min: 0.05,
            max: 20.0,
            default: 5.0,
            kind: ParamKind::Continuous { unit: Some("Hz".to_string()) },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO1_RATE_HZ as u32,
        },
        ParamDescriptor {
            name: "mod_lfo2_rate".to_string(),
            min: 0.05,
            max: 20.0,
            default: 1.7,
            kind: ParamKind::Continuous { unit: Some("Hz".to_string()) },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO2_RATE_HZ as u32,
        },
        ParamDescriptor {
            name: "mod_lfo3_rate".to_string(),
            min: 0.01,
            max: 8.0,
            default: 0.37,
            kind: ParamKind::Continuous { unit: Some("Hz".to_string()) },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO3_RATE_HZ as u32,
        },
        ParamDescriptor {
            name: "mod_env_attack".to_string(),
            min: 1.0,
            max: 2000.0,
            default: 6.0,
            kind: ParamKind::Continuous { unit: Some("ms".to_string()) },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_ATTACK_MS as u32,
        },
        ParamDescriptor {
            name: "mod_env_decay".to_string(),
            min: 5.0,
            max: 4000.0,
            default: 180.0,
            kind: ParamKind::Continuous { unit: Some("ms".to_string()) },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_DECAY_MS as u32,
        },
        ParamDescriptor {
            name: "mod_rand_slew".to_string(),
            min: 0.0,
            max: 0.999,
            default: 0.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_RAND_SLEW as u32,
        },
        ParamDescriptor {
            name: "mod_env_sustain".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.55,
            kind: ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_SUSTAIN as u32,
        },
        ParamDescriptor {
            name: "mod_env_release".to_string(),
            min: 5.0,
            max: 4000.0,
            default: 240.0,
            kind: ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_RELEASE_MS as u32,
        },
        ParamDescriptor {
            name: "mod_drift_rate".to_string(),
            min: 0.00001,
            max: 0.01,
            default: 0.00035,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32,
        },
        ParamDescriptor {
            name: "mod1_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD1_DEPTH as u32,
        },
        ParamDescriptor {
            name: "mod2_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD2_DEPTH as u32,
        },
        ParamDescriptor {
            name: "mod3_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD3_DEPTH as u32,
        },
        ParamDescriptor {
            name: "mod4_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD4_DEPTH as u32,
        },
        ParamDescriptor {
            name: "mod5_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD5_DEPTH as u32,
        },
        ParamDescriptor {
            name: "mod6_depth".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_MOD6_DEPTH as u32,
        },
    ]
}

pub fn ui_param_descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor {
            name: "mod1 lfo rate".to_string(),
            min: 0.05,
            max: 20.0,
            default: 5.0,
            kind: ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO1_RATE_HZ as u32,
        },
        ParamDescriptor {
            name: "mod2 env attack".to_string(),
            min: 1.0,
            max: 2000.0,
            default: 6.0,
            kind: ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_ATTACK_MS as u32,
        },
        ParamDescriptor {
            name: "mod2 env decay".to_string(),
            min: 5.0,
            max: 4000.0,
            default: 180.0,
            kind: ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_DECAY_MS as u32,
        },
        ParamDescriptor {
            name: "mod2 env sustain".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.55,
            kind: ParamKind::Continuous {
                unit: Some("%".to_string()),
            },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_SUSTAIN as u32,
        },
        ParamDescriptor {
            name: "mod2 env release".to_string(),
            min: 5.0,
            max: 4000.0,
            default: 240.0,
            kind: ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_ENV_RELEASE_MS as u32,
        },
        ParamDescriptor {
            name: "mod3 rand slew".to_string(),
            min: 0.0,
            max: 0.999,
            default: 0.0,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Linear,
            node_param_idx: MOD_PARAM_BASE + PARAM_RAND_SLEW as u32,
        },
        ParamDescriptor {
            name: "mod4 drift rate".to_string(),
            min: 0.00001,
            max: 0.01,
            default: 0.00035,
            kind: ParamKind::Continuous { unit: None },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_DRIFT_RATE as u32,
        },
        ParamDescriptor {
            name: "mod5 lfo 2 rate".to_string(),
            min: 0.05,
            max: 20.0,
            default: 1.7,
            kind: ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO2_RATE_HZ as u32,
        },
        ParamDescriptor {
            name: "mod6 lfo 3 rate".to_string(),
            min: 0.01,
            max: 8.0,
            default: 0.37,
            kind: ParamKind::Continuous {
                unit: Some("Hz".to_string()),
            },
            scaling: ParamScaling::Exponential,
            node_param_idx: MOD_PARAM_BASE + PARAM_LFO3_RATE_HZ as u32,
        },
    ]
}

unsafe extern "C" fn voice_modulator_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    for i in 0..STATE_SIZE {
        *s.add(i) = 0.0;
    }
    *s.add(IDX_RNG) = 0x1234_5678u32 as f32;
    init_param_defaults(s);
}

unsafe extern "C" fn voice_modulator_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let nf = nframes as usize;
    let s = state as *mut f32;

    let gate_in = *inp.add(0);
    let pitch_in = *inp.add(1);
    let velocity_in = *inp.add(2);
    let trigger_in = *inp.add(3);

    let out_mod1 = *out.add(0);
    let out_mod2 = *out.add(1);
    let out_mod3 = *out.add(2);
    let out_mod4 = *out.add(3);
    let out_mod5 = *out.add(4);
    let out_mod6 = *out.add(5);
    let mut lfo1_phase = *s.add(IDX_LFO1_PHASE);
    let mut lfo2_phase = *s.add(IDX_LFO2_PHASE);
    let mut lfo3_phase = *s.add(IDX_LFO3_PHASE);
    let mut env2 = *s.add(IDX_ENV2);
    let mut rand_hold = *s.add(IDX_RAND_HOLD);
    let mut drift = *s.add(IDX_DRIFT);
    let mut prev_gate = *s.add(IDX_PREV_GATE);
    let mut rng_state = *s.add(IDX_RNG) as u32;
    let mut rand_smooth = *s.add(IDX_RAND_SMOOTH);

    let sample_rate = 44_100.0f32;
    let lfo1_rate = (*s.add(PARAM_LFO1_RATE_HZ)).clamp(0.05, 20.0);
    let lfo2_rate = (*s.add(PARAM_LFO2_RATE_HZ)).clamp(0.05, 20.0);
    let lfo3_rate = (*s.add(PARAM_LFO3_RATE_HZ)).clamp(0.01, 8.0);
    let env_attack_ms = (*s.add(PARAM_ENV_ATTACK_MS)).clamp(1.0, 2000.0);
    let env_decay_ms = (*s.add(PARAM_ENV_DECAY_MS)).clamp(5.0, 4000.0);
    let env_sustain = (*s.add(PARAM_ENV_SUSTAIN)).clamp(0.0, 1.0);
    let env_release_ms = (*s.add(PARAM_ENV_RELEASE_MS)).clamp(5.0, 4000.0);
    let rand_slew = (*s.add(PARAM_RAND_SLEW)).clamp(0.0, 0.999);
    let drift_rate = (*s.add(PARAM_DRIFT_RATE)).clamp(0.00001, 0.01);
    let mod1_depth = (*s.add(PARAM_MOD1_DEPTH)).clamp(0.0, 1.0);
    let mod2_depth = (*s.add(PARAM_MOD2_DEPTH)).clamp(0.0, 1.0);
    let mod3_depth = (*s.add(PARAM_MOD3_DEPTH)).clamp(0.0, 1.0);
    let mod4_depth = (*s.add(PARAM_MOD4_DEPTH)).clamp(0.0, 1.0);
    let mod5_depth = (*s.add(PARAM_MOD5_DEPTH)).clamp(0.0, 1.0);
    let mod6_depth = (*s.add(PARAM_MOD6_DEPTH)).clamp(0.0, 1.0);

    let lfo1_inc = lfo1_rate / sample_rate;
    let lfo2_inc = lfo2_rate / sample_rate;
    let lfo3_inc = lfo3_rate / sample_rate;
    let env_attack = 1.0 / (env_attack_ms * 0.001 * sample_rate);
    let env_decay = 1.0 / (env_decay_ms * 0.001 * sample_rate);
    let env_release = 1.0 / (env_release_ms * 0.001 * sample_rate);

    for i in 0..nf {
        let gate = (*gate_in.add(i)).clamp(0.0, 1.0);
        let _pitch = (*pitch_in.add(i)).max(20.0);
        let velocity = (*velocity_in.add(i)).clamp(0.0, 1.0);
        let trigger = (*trigger_in.add(i)).max(0.0);

        let gate_rising = gate > 0.5 && prev_gate <= 0.5;
        let note_on = gate_rising || trigger > 0.5;

        if note_on {
            env2 = 0.0;
            rand_hold = next_rand(&mut rng_state);
        }

        lfo1_phase = (lfo1_phase + lfo1_inc).fract();
        lfo2_phase = (lfo2_phase + lfo2_inc).fract();
        lfo3_phase = (lfo3_phase + lfo3_inc).fract();

        if gate > 0.5 {
            if env2 < 0.999 {
                env2 = (env2 + env_attack).min(1.0);
            } else {
                env2 += (env_sustain - env2) * env_decay;
            }
        } else if env2 > env_sustain + 0.001 {
            env2 += (env_sustain - env2) * env_decay;
        } else {
            env2 += (0.0 - env2) * env_release;
        }
        env2 = env2.clamp(0.0, 1.0);

        let drift_target = next_rand(&mut rng_state) * 0.7;
        drift += (drift_target - drift) * drift_rate;
        drift = drift.clamp(-1.0, 1.0);
        rand_smooth += (rand_hold - rand_smooth) * (1.0 - rand_slew);

        let mod1 = (triangle(lfo1_phase) * mod1_depth).clamp(-1.0, 1.0);
        let mod2 = ((env2 * 2.0 - 1.0) * mod2_depth).clamp(-1.0, 1.0);
        let mod3 = (rand_smooth * mod3_depth).clamp(-1.0, 1.0);
        let mod4 = ((drift * (0.4 + velocity * 0.6)) * mod4_depth).clamp(-1.0, 1.0);
        let mod5 = (triangle(lfo2_phase) * mod5_depth).clamp(-1.0, 1.0);
        let mod6 = (triangle(lfo3_phase) * mod6_depth).clamp(-1.0, 1.0);

        *out_mod1.add(i) = mod1;
        *out_mod2.add(i) = mod2;
        *out_mod3.add(i) = mod3;
        *out_mod4.add(i) = mod4;
        *out_mod5.add(i) = mod5;
        *out_mod6.add(i) = mod6;
        prev_gate = gate;
    }

    *s.add(IDX_LFO1_PHASE) = lfo1_phase;
    *s.add(IDX_LFO2_PHASE) = lfo2_phase;
    *s.add(IDX_LFO3_PHASE) = lfo3_phase;
    *s.add(IDX_ENV2) = env2;
    *s.add(IDX_RAND_HOLD) = rand_hold;
    *s.add(IDX_DRIFT) = drift;
    *s.add(IDX_PREV_GATE) = prev_gate;
    *s.add(IDX_RNG) = rng_state as f32;
    *s.add(IDX_RAND_SMOOTH) = rand_smooth;
}

pub fn voice_modulator_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(voice_modulator_process),
        init: Some(voice_modulator_init),
        reset: None,
        migrate: None,
    }
}
