use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

pub const NUM_STEPS: usize = 16;
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
    data: [AtomicU32; NUM_STEPS * NUM_PARAMS],
}

impl StepData {
    pub fn new() -> Self {
        let data: [AtomicU32; NUM_STEPS * NUM_PARAMS] =
            std::array::from_fn(|i| {
                let param_idx = i % NUM_PARAMS;
                let param = StepParam::ALL[param_idx];
                AtomicU32::new(param.default_value().to_bits())
            });
        Self { data }
    }

    pub fn get(&self, step: usize, param: StepParam) -> f32 {
        assert!(step < NUM_STEPS);
        let idx = step * NUM_PARAMS + param.index();
        f32::from_bits(self.data[idx].load(Ordering::Relaxed))
    }

    pub fn set(&self, step: usize, param: StepParam, val: f32) {
        assert!(step < NUM_STEPS);
        let clamped = val.clamp(param.min(), param.max());
        let idx = step * NUM_PARAMS + param.index();
        self.data[idx].store(clamped.to_bits(), Ordering::Relaxed);
    }
}

/// One track's step pattern — 16 bits, one per step.
pub struct TrackPattern {
    bits: AtomicU16,
}

impl TrackPattern {
    pub fn new() -> Self {
        Self {
            bits: AtomicU16::new(0),
        }
    }

    pub fn toggle_step(&self, step: usize) {
        assert!(step < NUM_STEPS);
        self.bits.fetch_xor(1 << step, Ordering::Relaxed);
    }

    pub fn is_active(&self, step: usize) -> bool {
        assert!(step < NUM_STEPS);
        (self.bits.load(Ordering::Relaxed) >> step) & 1 == 1
    }
}

/// Shared state visible to both audio thread and UI thread.
pub struct SequencerState {
    pub patterns: Vec<TrackPattern>,
    pub step_data: Vec<StepData>,
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
}

impl SequencerState {
    pub fn new(num_tracks: usize) -> Self {
        let patterns = (0..num_tracks).map(|_| TrackPattern::new()).collect();
        let step_data = (0..num_tracks).map(|_| StepData::new()).collect();
        Self {
            patterns,
            step_data,
            playhead: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            bpm: AtomicU32::new(DEFAULT_BPM),
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
            state.playhead.store((NUM_STEPS - 1) as u32, Ordering::Relaxed);
        }

        let mut triggers = Vec::new();
        let mut current_step = state.playhead.load(Ordering::Relaxed) as usize;

        for offset in 0..nframes {
            self.sample_counter += 1.0;
            if self.sample_counter >= self.samples_per_step {
                self.sample_counter -= self.samples_per_step;
                current_step = (current_step + 1) % NUM_STEPS;
                state.playhead.store(current_step as u32, Ordering::Relaxed);
                triggers.push(Trigger {
                    step: current_step,
                    offset,
                });
            }
        }

        triggers
    }
}
