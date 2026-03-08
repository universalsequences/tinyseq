use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::voice::MAX_VOICES;

pub const MAX_TRACKS: usize = 64;
pub const MAX_STEPS: usize = 64;
pub const STEPS_PER_PAGE: usize = 16;
pub const NUM_PARAMS: usize = 8;
pub const DEFAULT_BPM: u32 = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstrumentType {
    Sampler,
    Custom,
}

impl InstrumentType {
    pub const COUNT: usize = 2;
    pub const ALL: [Self; Self::COUNT] = [Self::Sampler, Self::Custom];

    pub fn label(&self) -> &'static str {
        match self {
            InstrumentType::Sampler => "Sampler",
            InstrumentType::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Timebase {
    Whole = 0,
    Half = 1,
    Quarter = 2,
    Eighth = 3,
    Sixteenth = 4,
    ThirtySecond = 5,
    SixtyFourth = 6,
    HalfTriplet = 7,
    QuarterTriplet = 8,
    EighthTriplet = 9,
    SixteenthTriplet = 10,
    ThirtySecondTriplet = 11,
    SixtyFourthTriplet = 12,
    Polyrhythm = 13,
}

impl Timebase {
    pub const COUNT: usize = 14;

    pub const ALL: [Timebase; Self::COUNT] = [
        Timebase::Whole,
        Timebase::Half,
        Timebase::Quarter,
        Timebase::Eighth,
        Timebase::Sixteenth,
        Timebase::ThirtySecond,
        Timebase::SixtyFourth,
        Timebase::HalfTriplet,
        Timebase::QuarterTriplet,
        Timebase::EighthTriplet,
        Timebase::SixteenthTriplet,
        Timebase::ThirtySecondTriplet,
        Timebase::SixtyFourthTriplet,
        Timebase::Polyrhythm,
    ];

    pub const LABELS: [&'static str; Self::COUNT] = [
        "1", "2", "4", "8", "16", "32", "64", "2T", "4T", "8T", "16T", "32T", "64T", "Prh",
    ];

    pub fn from_index(i: u32) -> Self {
        Self::ALL
            .get(i as usize)
            .copied()
            .unwrap_or(Timebase::Sixteenth)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Timebase::Whole => "1",
            Timebase::Half => "2",
            Timebase::Quarter => "4",
            Timebase::Eighth => "8",
            Timebase::Sixteenth => "16",
            Timebase::ThirtySecond => "32",
            Timebase::SixtyFourth => "64",
            Timebase::HalfTriplet => "2T",
            Timebase::QuarterTriplet => "4T",
            Timebase::EighthTriplet => "8T",
            Timebase::SixteenthTriplet => "16T",
            Timebase::ThirtySecondTriplet => "32T",
            Timebase::SixtyFourthTriplet => "64T",
            Timebase::Polyrhythm => "Prh",
        }
    }

    pub fn step_beats(&self, num_steps: usize) -> f64 {
        match self {
            Timebase::Whole => 4.0,
            Timebase::Half => 2.0,
            Timebase::Quarter => 1.0,
            Timebase::Eighth => 0.5,
            Timebase::Sixteenth => 0.25,
            Timebase::ThirtySecond => 0.125,
            Timebase::SixtyFourth => 0.0625,
            Timebase::HalfTriplet => 4.0 / 3.0,
            Timebase::QuarterTriplet => 2.0 / 3.0,
            Timebase::EighthTriplet => 1.0 / 3.0,
            Timebase::SixteenthTriplet => 1.0 / 6.0,
            Timebase::ThirtySecondTriplet => 1.0 / 12.0,
            Timebase::SixtyFourthTriplet => 1.0 / 24.0,
            Timebase::Polyrhythm => 4.0 / num_steps.max(1) as f64,
        }
    }

    pub fn samples_per_step(&self, sample_rate: f64, bpm: f64, num_steps: usize) -> f64 {
        let samples_per_quarter = sample_rate * 60.0 / bpm;
        samples_per_quarter * self.step_beats(num_steps)
    }
}

pub const SYNC_RESOLUTIONS: [(f64, &str); 8] = [
    (0.0, "Off"),
    (0.25, "1/16"),
    (0.5, "1/8"),
    (1.0, "1/4"),
    (2.0, "1/2 bar"),
    (4.0, "1 bar"),
    (8.0, "2 bars"),
    (16.0, "4 bars"),
];
pub const SYNC_COUNT: usize = SYNC_RESOLUTIONS.len();

pub fn sync_beats(val: f32) -> f64 {
    let idx = val.round() as usize;
    if idx > 0 && idx < SYNC_COUNT {
        SYNC_RESOLUTIONS[idx].0
    } else {
        0.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepParam {
    Duration = 0,
    Velocity = 1,
    Speed = 2,
    AuxA = 3,
    AuxB = 4,
    Transpose = 5,
    Chop = 6,
    Sync = 7,
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
        StepParam::Sync,
    ];

    pub const VISIBLE: [StepParam; 4] = [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Transpose,
        StepParam::Sync,
    ];

    pub fn default_value(self) -> f32 {
        match self {
            StepParam::Duration => 1.0,
            StepParam::Velocity => 1.0,
            StepParam::Speed => 1.0,
            StepParam::AuxA => 0.0,
            StepParam::AuxB => 0.0,
            StepParam::Transpose => 0.0,
            StepParam::Chop => 1.0,
            StepParam::Sync => 0.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            StepParam::Duration => 0.0,
            StepParam::Velocity => 0.0,
            StepParam::Speed => 0.5,
            StepParam::AuxA => 0.0,
            StepParam::AuxB => 0.0,
            StepParam::Transpose => -48.0,
            StepParam::Chop => 1.0,
            StepParam::Sync => 0.0,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            StepParam::Duration => 32.0,
            StepParam::Velocity => 1.0,
            StepParam::Speed => 2.0,
            StepParam::AuxA => 1.0,
            StepParam::AuxB => 1.0,
            StepParam::Transpose => 48.0,
            StepParam::Chop => 8.0,
            StepParam::Sync => (SYNC_COUNT - 1) as f32,
        }
    }

    pub fn increment(self) -> f32 {
        match self {
            StepParam::Duration => 0.05,
            StepParam::Velocity => 0.05,
            StepParam::Speed => 0.05,
            StepParam::AuxA => 0.05,
            StepParam::AuxB => 0.05,
            StepParam::Transpose => 1.0,
            StepParam::Chop => 1.0,
            StepParam::Sync => 1.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StepParam::Duration => "Duration",
            StepParam::Velocity => "Velocity",
            StepParam::Speed => "Speed",
            StepParam::AuxA => "Aux A",
            StepParam::AuxB => "Aux B",
            StepParam::Transpose => "Transpose",
            StepParam::Chop => "Chop",
            StepParam::Sync => "Sync",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            StepParam::Duration => "dur",
            StepParam::Velocity => "vel",
            StepParam::Speed => "spd",
            StepParam::AuxA => "axA",
            StepParam::AuxB => "axB",
            StepParam::Transpose => "trn",
            StepParam::Chop => "chp",
            StepParam::Sync => "syn",
        }
    }

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
            StepParam::Chop => format!("{:.0}", val),
            StepParam::Sync => {
                let idx = val.round() as usize;
                if idx < SYNC_COUNT {
                    SYNC_RESOLUTIONS[idx].1.to_string()
                } else {
                    "Off".to_string()
                }
            }
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
        let idx = if self.index() == 0 {
            NUM_PARAMS - 1
        } else {
            self.index() - 1
        };
        StepParam::ALL[idx]
    }

    pub fn hotkey(self) -> char {
        match self {
            StepParam::Duration => 'd',
            StepParam::Velocity => 'v',
            StepParam::Speed => 's',
            StepParam::AuxA => 'a',
            StepParam::AuxB => 'b',
            StepParam::Transpose => 't',
            StepParam::Chop => 'c',
            StepParam::Sync => 'y',
        }
    }

    pub fn from_hotkey(c: char) -> Option<StepParam> {
        match c {
            'd' => Some(StepParam::Duration),
            'v' => Some(StepParam::Velocity),
            's' => Some(StepParam::Speed),
            't' => Some(StepParam::Transpose),
            'y' => Some(StepParam::Sync),
            _ => None,
        }
    }

    pub fn tab_parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            StepParam::Duration => ("", "d", "ur"),
            StepParam::Velocity => ("", "v", "el"),
            StepParam::Speed => ("", "s", "pd"),
            StepParam::AuxA => ("", "a", "xA"),
            StepParam::AuxB => ("ax", "B", ""),
            StepParam::Transpose => ("", "t", "rn"),
            StepParam::Chop => ("", "c", "hp"),
            StepParam::Sync => ("s", "y", "n"),
        }
    }
}

pub struct StepData {
    data: [AtomicU32; MAX_STEPS * NUM_PARAMS],
}

impl StepData {
    pub fn new() -> Self {
        let data: [AtomicU32; MAX_STEPS * NUM_PARAMS] = std::array::from_fn(|i| {
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

    pub fn load_bits(&self) -> u64 {
        self.bits.load(Ordering::Relaxed)
    }

    pub fn store_bits(&self, bits: u64) {
        self.bits.store(bits, Ordering::Relaxed);
    }
}

pub struct TrackParams {
    pub gate: AtomicBool,
    pub attack_ms: AtomicU32,
    pub release_ms: AtomicU32,
    pub swing: AtomicU32,
    pub num_steps: AtomicU32,
    pub send: AtomicU32,
    pub polyphonic: AtomicBool,
    pub timebase: AtomicU32,
}

impl TrackParams {
    pub fn new() -> Self {
        Self {
            gate: AtomicBool::new(true),
            attack_ms: AtomicU32::new(0.0_f32.to_bits()),
            release_ms: AtomicU32::new(0.0_f32.to_bits()),
            swing: AtomicU32::new(50.0_f32.to_bits()),
            num_steps: AtomicU32::new(STEPS_PER_PAGE as u32),
            send: AtomicU32::new(0.0_f32.to_bits()),
            polyphonic: AtomicBool::new(true),
            timebase: AtomicU32::new(Timebase::Sixteenth as u32),
        }
    }

    pub fn get_attack_ms(&self) -> f32 { f32::from_bits(self.attack_ms.load(Ordering::Relaxed)) }
    pub fn set_attack_ms(&self, val: f32) { self.attack_ms.store(val.clamp(0.0, 500.0).to_bits(), Ordering::Relaxed); }
    pub fn get_release_ms(&self) -> f32 { f32::from_bits(self.release_ms.load(Ordering::Relaxed)) }
    pub fn set_release_ms(&self, val: f32) { self.release_ms.store(val.clamp(0.0, 2000.0).to_bits(), Ordering::Relaxed); }
    pub fn get_swing(&self) -> f32 { f32::from_bits(self.swing.load(Ordering::Relaxed)) }
    pub fn set_swing(&self, val: f32) { self.swing.store(val.clamp(50.0, 75.0).to_bits(), Ordering::Relaxed); }
    pub fn is_gate_on(&self) -> bool { self.gate.load(Ordering::Relaxed) }
    pub fn toggle_gate(&self) { self.gate.fetch_xor(true, Ordering::Relaxed); }
    pub fn get_num_steps(&self) -> usize { self.num_steps.load(Ordering::Relaxed) as usize }
    pub fn set_num_steps(&self, val: usize) { self.num_steps.store(val.clamp(1, MAX_STEPS) as u32, Ordering::Relaxed); }
    pub fn get_send(&self) -> f32 { f32::from_bits(self.send.load(Ordering::Relaxed)) }
    pub fn set_send(&self, val: f32) { self.send.store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed); }
    pub fn is_polyphonic(&self) -> bool { self.polyphonic.load(Ordering::Relaxed) }
    pub fn toggle_polyphonic(&self) { self.polyphonic.fetch_xor(true, Ordering::Relaxed); }
    pub fn get_timebase(&self) -> Timebase { Timebase::from_index(self.timebase.load(Ordering::Relaxed)) }
    pub fn set_timebase(&self, tb: Timebase) { self.timebase.store(tb as u32, Ordering::Relaxed); }
    pub fn next_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = (cur + 1) % Timebase::COUNT as u32;
        self.timebase.store(next, Ordering::Relaxed);
    }
    pub fn prev_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = if cur == 0 { Timebase::COUNT as u32 - 1 } else { cur - 1 };
        self.timebase.store(next, Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct TrackParamsSnapshot {
    pub gate: bool,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub swing: f32,
    pub num_steps: usize,
    pub send: f32,
    pub polyphonic: bool,
    pub timebase: Timebase,
}

impl Default for TrackParamsSnapshot {
    fn default() -> Self {
        Self {
            gate: true,
            attack_ms: 0.0,
            release_ms: 0.0,
            swing: 50.0,
            num_steps: STEPS_PER_PAGE,
            send: 0.0,
            polyphonic: false,
            timebase: Timebase::Sixteenth,
        }
    }
}

#[derive(Clone, Default)]
pub struct TrackSoundState {
    pub engine_id: Option<usize>,
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

pub struct ChordData {
    transposes: [AtomicU32; MAX_STEPS * MAX_VOICES],
    counts: [AtomicU32; MAX_STEPS],
}

impl ChordData {
    pub fn new() -> Self {
        Self {
            transposes: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            counts: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    pub fn count(&self, step: usize) -> usize { self.counts[step].load(Ordering::Relaxed) as usize }
    pub fn get(&self, step: usize, n: usize) -> f32 { f32::from_bits(self.transposes[step * MAX_VOICES + n].load(Ordering::Relaxed)) }

    pub fn add_note(&self, step: usize, transpose: f32) -> bool {
        let c = self.counts[step].load(Ordering::Relaxed) as usize;
        if c >= MAX_VOICES { return false; }
        self.transposes[step * MAX_VOICES + c].store(transpose.to_bits(), Ordering::Relaxed);
        self.counts[step].store((c + 1) as u32, Ordering::Relaxed);
        true
    }

    pub fn clear_step(&self, step: usize) { self.counts[step].store(0, Ordering::Relaxed); }

    pub fn copy_step(&self, src: usize, dst: usize) {
        let c = self.counts[src].load(Ordering::Relaxed);
        self.counts[dst].store(c, Ordering::Relaxed);
        for n in 0..(c as usize).min(MAX_VOICES) {
            let val = self.transposes[src * MAX_VOICES + n].load(Ordering::Relaxed);
            self.transposes[dst * MAX_VOICES + n].store(val, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub struct ChordSnapshot {
    pub steps: Vec<Vec<f32>>,
}

impl ChordSnapshot {
    pub fn capture(cd: &ChordData) -> Self {
        let mut steps = Vec::with_capacity(MAX_STEPS);
        for s in 0..MAX_STEPS {
            let c = cd.count(s);
            let mut notes = Vec::with_capacity(c);
            for n in 0..c {
                notes.push(cd.get(s, n));
            }
            steps.push(notes);
        }
        Self { steps }
    }

    pub fn restore(&self, cd: &ChordData) {
        for s in 0..MAX_STEPS {
            let notes = &self.steps[s];
            cd.counts[s].store(notes.len() as u32, Ordering::Relaxed);
            for (n, &t) in notes.iter().enumerate() {
                if n < MAX_VOICES {
                    cd.transposes[s * MAX_VOICES + n].store(t.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }

    pub fn new_default() -> Self {
        Self { steps: (0..MAX_STEPS).map(|_| Vec::new()).collect() }
    }
}

pub struct KeyboardTrigger {
    pub track: usize,
    pub transpose: f32,
    pub velocity: f32,
    pub note_off: bool,
}

pub struct Trigger {
    pub track: usize,
    pub step: usize,
    pub offset: usize,
}

pub struct TimebasePLockData {
    overrides: [AtomicU32; MAX_STEPS],
}

impl TimebasePLockData {
    pub fn new() -> Self {
        Self { overrides: std::array::from_fn(|_| AtomicU32::new(u32::MAX)) }
    }

    pub fn get(&self, step: usize) -> Option<Timebase> {
        let v = self.overrides[step].load(Ordering::Relaxed);
        if v == u32::MAX { None } else { Some(Timebase::from_index(v)) }
    }

    pub fn set(&self, step: usize, tb: Timebase) { self.overrides[step].store(tb as u32, Ordering::Relaxed); }
    pub fn clear(&self, step: usize) { self.overrides[step].store(u32::MAX, Ordering::Relaxed); }
    pub fn has_plock(&self, step: usize) -> bool { self.overrides[step].load(Ordering::Relaxed) != u32::MAX }
    pub fn resolve(&self, step: usize, default: Timebase) -> Timebase { self.get(step).unwrap_or(default) }

    pub fn snapshot(&self) -> [Option<u32>; MAX_STEPS] {
        std::array::from_fn(|i| {
            let v = self.overrides[i].load(Ordering::Relaxed);
            if v == u32::MAX { None } else { Some(v) }
        })
    }

    pub fn restore(&self, snap: &[Option<u32>; MAX_STEPS]) {
        for (i, v) in snap.iter().enumerate() {
            self.overrides[i].store(v.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
}
