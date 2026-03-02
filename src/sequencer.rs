use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::effects::{EffectPLockData, TrackEffectDefaults};

pub const MAX_STEPS: usize = 64;
pub const STEPS_PER_PAGE: usize = 16;
pub const NUM_PARAMS: usize = 7;
pub const DEFAULT_BPM: u32 = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepParam {
    Duration  = 0, // 0.0..4.0 (fraction of full sample)
    Velocity  = 1, // 0.0..1.0
    Speed     = 2, // 0.5..2.0 (playback rate)
    AuxA      = 3, // 0.0..1.0
    AuxB      = 4, // 0.0..1.0
    Transpose = 5, // -12.0..12.0 (semitones)
    Chop      = 6, // 1..8 (number of re-triggers per step)
}

impl StepParam {
    pub const ALL: [StepParam; NUM_PARAMS] = [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Speed,
        StepParam::AuxA,
        StepParam::AuxB,
        StepParam::Transpose,
        StepParam::Chop,
    ];

    pub fn default_value(self) -> f32 {
        match self {
            StepParam::Duration  => 1.0,
            StepParam::Velocity  => 1.0,
            StepParam::Speed     => 1.0,
            StepParam::AuxA      => 0.0,
            StepParam::AuxB      => 0.0,
            StepParam::Transpose => 0.0,
            StepParam::Chop      => 1.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            StepParam::Duration  => 0.0,
            StepParam::Velocity  => 0.0,
            StepParam::Speed     => 0.5,
            StepParam::AuxA      => 0.0,
            StepParam::AuxB      => 0.0,
            StepParam::Transpose => -12.0,
            StepParam::Chop      => 1.0,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            StepParam::Duration  => 4.0,
            StepParam::Velocity  => 1.0,
            StepParam::Speed     => 2.0,
            StepParam::AuxA      => 1.0,
            StepParam::AuxB      => 1.0,
            StepParam::Transpose => 12.0,
            StepParam::Chop      => 8.0,
        }
    }

    pub fn increment(self) -> f32 {
        match self {
            StepParam::Duration  => 0.05,
            StepParam::Velocity  => 0.05,
            StepParam::Speed     => 0.05,
            StepParam::AuxA      => 0.05,
            StepParam::AuxB      => 0.05,
            StepParam::Transpose => 1.0,
            StepParam::Chop      => 1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StepParam::Duration  => "Duration",
            StepParam::Velocity  => "Velocity",
            StepParam::Speed     => "Speed",
            StepParam::AuxA      => "Aux A",
            StepParam::AuxB      => "Aux B",
            StepParam::Transpose => "Transpose",
            StepParam::Chop      => "Chop",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            StepParam::Duration  => "dur",
            StepParam::Velocity  => "vel",
            StepParam::Speed     => "spd",
            StepParam::AuxA      => "axA",
            StepParam::AuxB      => "axB",
            StepParam::Transpose => "trn",
            StepParam::Chop      => "chp",
        }
    }

    /// Normalize value to 0.0..1.0 range for display purposes.
    pub fn normalize(self, val: f32) -> f32 {
        let min = self.min();
        let max = self.max();
        if (max - min).abs() < f32::EPSILON {
            return 0.0;
        }
        ((val - min) / (max - min)).clamp(0.0, 1.0)
    }

    pub fn format_value(self, val: f32) -> String {
        match self {
            StepParam::Transpose => format!("{:+.0}", val),
            StepParam::Chop      => format!("{:.0}", val),
            _ => format!("{:.2}", val),
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn next(self) -> StepParam {
        let idx = (self.index() + 1) % NUM_PARAMS;
        StepParam::ALL[idx]
    }

    pub fn prev(self) -> StepParam {
        let idx = if self.index() == 0 { NUM_PARAMS - 1 } else { self.index() - 1 };
        StepParam::ALL[idx]
    }

    pub fn hotkey(self) -> char {
        match self {
            StepParam::Duration  => 'd',
            StepParam::Velocity  => 'v',
            StepParam::Speed     => 's',
            StepParam::AuxA      => 'a',
            StepParam::AuxB      => 'b',
            StepParam::Transpose => 't',
            StepParam::Chop      => 'c',
        }
    }

    pub fn from_hotkey(c: char) -> Option<StepParam> {
        match c {
            'd' => Some(StepParam::Duration),
            'v' => Some(StepParam::Velocity),
            's' => Some(StepParam::Speed),
            'a' => Some(StepParam::AuxA),
            'b' => Some(StepParam::AuxB),
            't' => Some(StepParam::Transpose),
            'c' => Some(StepParam::Chop),
            _ => None,
        }
    }

    /// Returns (prefix, hotkey_char, suffix) for rendering with underlined hotkey.
    pub fn tab_parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            StepParam::Duration  => ("", "d", "ur"),
            StepParam::Velocity  => ("", "v", "el"),
            StepParam::Speed     => ("", "s", "pd"),
            StepParam::AuxA      => ("", "a", "xA"),
            StepParam::AuxB      => ("ax", "B", ""),
            StepParam::Transpose => ("", "t", "rn"),
            StepParam::Chop      => ("", "c", "hp"),
        }
    }
}

/// Per-step parameter data for one track, stored as atomics for lock-free audio access.
pub struct StepData {
    data: [AtomicU32; MAX_STEPS * NUM_PARAMS],
}

impl StepData {
    pub fn new() -> Self {
        let data: [AtomicU32; MAX_STEPS * NUM_PARAMS] =
            std::array::from_fn(|i| {
                let param_idx = i % NUM_PARAMS;
                let param = StepParam::ALL[param_idx];
                AtomicU32::new(param.default_value().to_bits())
            });
        Self { data }
    }

    pub fn get(&self, step: usize, param: StepParam) -> f32 {
        assert!(step < MAX_STEPS);
        let idx = step * NUM_PARAMS + param.index();
        f32::from_bits(self.data[idx].load(Ordering::Relaxed))
    }

    pub fn set(&self, step: usize, param: StepParam, val: f32) {
        assert!(step < MAX_STEPS);
        let clamped = val.clamp(param.min(), param.max());
        let idx = step * NUM_PARAMS + param.index();
        self.data[idx].store(clamped.to_bits(), Ordering::Relaxed);
    }
}

/// One track's step pattern — 64 bits, one per step.
pub struct TrackPattern {
    bits: AtomicU64,
}

impl TrackPattern {
    pub fn new() -> Self {
        Self {
            bits: AtomicU64::new(0),
        }
    }

    pub fn toggle_step(&self, step: usize) {
        assert!(step < MAX_STEPS);
        self.bits.fetch_xor(1 << step, Ordering::Relaxed);
    }

    pub fn is_active(&self, step: usize) -> bool {
        assert!(step < MAX_STEPS);
        (self.bits.load(Ordering::Relaxed) >> step) & 1 == 1
    }
}

/// Per-track parameters (track-wide, not per-step).
pub struct TrackParams {
    /// When true, sample is gated by duration. When false, plays to completion.
    pub gate: AtomicBool,
    /// Attack time in ms (stored as f32 bits). 0-500ms.
    pub attack_ms: AtomicU32,
    /// Release time in ms (stored as f32 bits). 0-2000ms.
    pub release_ms: AtomicU32,
    /// Swing percentage (stored as f32 bits). 50.0-75.0%.
    pub swing: AtomicU32,
    /// Number of active steps for this track (1..MAX_STEPS).
    pub num_steps: AtomicU32,
}

impl TrackParams {
    pub fn new() -> Self {
        Self {
            gate: AtomicBool::new(true),
            attack_ms: AtomicU32::new(0.0_f32.to_bits()),
            release_ms: AtomicU32::new(0.0_f32.to_bits()),
            swing: AtomicU32::new(50.0_f32.to_bits()),
            num_steps: AtomicU32::new(STEPS_PER_PAGE as u32),
        }
    }

    pub fn get_attack_ms(&self) -> f32 {
        f32::from_bits(self.attack_ms.load(Ordering::Relaxed))
    }

    pub fn set_attack_ms(&self, val: f32) {
        self.attack_ms.store(val.clamp(0.0, 500.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_release_ms(&self) -> f32 {
        f32::from_bits(self.release_ms.load(Ordering::Relaxed))
    }

    pub fn set_release_ms(&self, val: f32) {
        self.release_ms.store(val.clamp(0.0, 2000.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_swing(&self) -> f32 {
        f32::from_bits(self.swing.load(Ordering::Relaxed))
    }

    pub fn set_swing(&self, val: f32) {
        self.swing.store(val.clamp(50.0, 75.0).to_bits(), Ordering::Relaxed);
    }

    pub fn is_gate_on(&self) -> bool {
        self.gate.load(Ordering::Relaxed)
    }

    pub fn toggle_gate(&self) {
        self.gate.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn get_num_steps(&self) -> usize {
        self.num_steps.load(Ordering::Relaxed) as usize
    }

    pub fn set_num_steps(&self, val: usize) {
        let clamped = val.clamp(1, MAX_STEPS) as u32;
        self.num_steps.store(clamped, Ordering::Relaxed);
    }
}

/// Shared state visible to both audio thread and UI thread.
pub struct SequencerState {
    pub patterns: Vec<TrackPattern>,
    pub step_data: Vec<StepData>,
    pub track_params: Vec<TrackParams>,
    pub effect_defaults: Vec<TrackEffectDefaults>,
    pub effect_plocks: Vec<EffectPLockData>,
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
    /// Peak level for L channel (f32 bits, 0.0..1.0+), updated by audio thread.
    pub peak_l: AtomicU32,
    /// Peak level for R channel (f32 bits, 0.0..1.0+), updated by audio thread.
    pub peak_r: AtomicU32,
}

impl SequencerState {
    pub fn new(num_tracks: usize) -> Self {
        let patterns = (0..num_tracks).map(|_| TrackPattern::new()).collect();
        let step_data = (0..num_tracks).map(|_| StepData::new()).collect();
        let track_params = (0..num_tracks).map(|_| TrackParams::new()).collect();
        let effect_defaults = (0..num_tracks).map(|_| TrackEffectDefaults::new()).collect();
        let effect_plocks = (0..num_tracks).map(|_| EffectPLockData::new()).collect();
        Self {
            patterns,
            step_data,
            track_params,
            effect_defaults,
            effect_plocks,
            playhead: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            bpm: AtomicU32::new(DEFAULT_BPM),
            peak_l: AtomicU32::new(0.0_f32.to_bits()),
            peak_r: AtomicU32::new(0.0_f32.to_bits()),
        }
    }

    pub fn current_step(&self) -> usize {
        self.playhead.load(Ordering::Relaxed) as usize
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn toggle_play(&self) {
        self.playing.fetch_xor(true, Ordering::Relaxed);
    }

    /// Toggle a step. When toggling OFF, clear its effect p-locks.
    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        let was_active = self.patterns[track].is_active(step);
        self.patterns[track].toggle_step(step);
        if was_active {
            // Toggled off — clear p-locks
            self.effect_plocks[track].clear_step(step);
        }
    }
}

/// Trigger event: which step fired, and at what sample offset within the block.
pub struct Trigger {
    pub step: usize,
    pub offset: usize,
}

/// Clock that runs in the audio callback, counting samples and emitting triggers.
pub struct SequencerClock {
    sample_rate: f64,
    sample_counter: f64,
    samples_per_step: f64,
    was_playing: bool,
}

impl SequencerClock {
    pub fn new(sample_rate: u32, bpm: u32) -> Self {
        let sr = sample_rate as f64;
        let samples_per_step = sr * 60.0 / bpm as f64 / 4.0;
        Self {
            sample_rate: sr,
            sample_counter: 0.0,
            samples_per_step,
            was_playing: false,
        }
    }

    pub fn update_bpm(&mut self, bpm: u32) {
        self.samples_per_step = self.sample_rate * 60.0 / bpm as f64 / 4.0;
    }

    pub fn current_samples_per_step(&self) -> f64 {
        self.samples_per_step
    }

    pub fn process_block(
        &mut self,
        nframes: usize,
        state: &SequencerState,
    ) -> Vec<Trigger> {
        if !state.is_playing() {
            self.was_playing = false;
            return Vec::new();
        }

        let bpm = state.bpm.load(Ordering::Relaxed);
        self.update_bpm(bpm);

        if !self.was_playing {
            self.was_playing = true;
            self.sample_counter = self.samples_per_step;
            // u32::MAX so first wrapping_add(1) yields 0
            state.playhead.store(u32::MAX, Ordering::Relaxed);
        }

        let mut triggers = Vec::new();
        let mut current_step = state.playhead.load(Ordering::Relaxed);

        for offset in 0..nframes {
            self.sample_counter += 1.0;
            if self.sample_counter >= self.samples_per_step {
                self.sample_counter -= self.samples_per_step;
                current_step = current_step.wrapping_add(1);
                state.playhead.store(current_step, Ordering::Relaxed);
                triggers.push(Trigger {
                    step: current_step as usize,
                    offset,
                });
            }
        }

        triggers
    }
}
