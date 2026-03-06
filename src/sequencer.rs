use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::effects::{EffectDescriptor, EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::voice::MAX_VOICES;

pub const MAX_TRACKS: usize = 64;
pub const MAX_STEPS: usize = 64;
pub const STEPS_PER_PAGE: usize = 16;
pub const NUM_PARAMS: usize = 8;
pub const DEFAULT_BPM: u32 = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstrumentType {
    Sampler,
    Custom, // placeholder for future Lisp-defined instruments
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

/// Timebase determines the duration of each step as a note division.
/// Inspired by the Sequentix Cirklon sequencer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Timebase {
    Whole = 0,                // 1  — each step = whole note
    Half = 1,                 // 2  — each step = half note
    Quarter = 2,              // 4  — each step = quarter note
    Eighth = 3,               // 8  — each step = eighth note
    Sixteenth = 4,            // 16 — each step = sixteenth note (default)
    ThirtySecond = 5,         // 32
    SixtyFourth = 6,          // 64
    HalfTriplet = 7,          // 2T
    QuarterTriplet = 8,       // 4T
    EighthTriplet = 9,        // 8T
    SixteenthTriplet = 10,    // 16T
    ThirtySecondTriplet = 11, // 32T
    SixtyFourthTriplet = 12,  // 64T
    Polyrhythm = 13,          // Prh — bar ÷ num_steps
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

    /// Duration of one step in quarter notes (musical time).
    /// num_steps only matters for Polyrhythm mode.
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

    /// Duration of one step in samples (for gate/chop calculations).
    pub fn samples_per_step(&self, sample_rate: f64, bpm: f64, num_steps: usize) -> f64 {
        let samples_per_quarter = sample_rate * 60.0 / bpm;
        samples_per_quarter * self.step_beats(num_steps)
    }
}

/// Sync resolution options: (beats, label).
/// Index 0 = Off. Indices 1..SYNC_COUNT map to beat values.
pub const SYNC_RESOLUTIONS: [(f64, &str); 8] = [
    (0.0, "Off"),     // 0 = off
    (0.25, "1/16"),   // 16th note
    (0.5, "1/8"),     // 8th note
    (1.0, "1/4"),     // quarter note
    (2.0, "1/2 bar"), // half bar
    (4.0, "1 bar"),   // 1 bar
    (8.0, "2 bars"),  // 2 bars
    (16.0, "4 bars"), // 4 bars
];
pub const SYNC_COUNT: usize = SYNC_RESOLUTIONS.len();

/// Get the beat resolution for a sync param value (0.0 = off → returns 0.0).
pub fn sync_beats(val: f32) -> f64 {
    let idx = val.round() as usize;
    if idx > 0 && idx < SYNC_COUNT {
        SYNC_RESOLUTIONS[idx].0
    } else {
        0.0
    }
}

/// Round `value` up to the next multiple of `grid`. Returns `value` unchanged
/// if already on the grid (within epsilon tolerance).
fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepParam {
    Duration = 0,  // 0.0..4.0 (fraction of full sample)
    Velocity = 1,  // 0.0..1.0
    Speed = 2,     // 0.5..2.0 (playback rate)
    AuxA = 3,      // 0.0..1.0
    AuxB = 4,      // 0.0..1.0
    Transpose = 5, // -12.0..12.0 (semitones)
    Chop = 6,      // 1..8 (number of re-triggers per step)
    Sync = 7,      // 0=off, 1..SYNC_COUNT = resolution index (see SYNC_RESOLUTIONS)
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

    /// Params visible in the step param tabs.
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

    /// Returns (prefix, hotkey_char, suffix) for rendering with underlined hotkey.
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

/// Per-step parameter data for one track, stored as atomics for lock-free audio access.
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

    pub fn load_bits(&self) -> u64 {
        self.bits.load(Ordering::Relaxed)
    }

    pub fn store_bits(&self, bits: u64) {
        self.bits.store(bits, Ordering::Relaxed);
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
    /// Reverb send level (stored as f32 bits). 0.0–1.0.
    pub send: AtomicU32,
    /// Polyphonic mode (default false = mono).
    pub polyphonic: AtomicBool,
    /// Timebase: step duration as a note division. Index into Timebase enum.
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

    pub fn get_attack_ms(&self) -> f32 {
        f32::from_bits(self.attack_ms.load(Ordering::Relaxed))
    }

    pub fn set_attack_ms(&self, val: f32) {
        self.attack_ms
            .store(val.clamp(0.0, 500.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_release_ms(&self) -> f32 {
        f32::from_bits(self.release_ms.load(Ordering::Relaxed))
    }

    pub fn set_release_ms(&self, val: f32) {
        self.release_ms
            .store(val.clamp(0.0, 2000.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get_swing(&self) -> f32 {
        f32::from_bits(self.swing.load(Ordering::Relaxed))
    }

    pub fn set_swing(&self, val: f32) {
        self.swing
            .store(val.clamp(50.0, 75.0).to_bits(), Ordering::Relaxed);
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

    pub fn get_send(&self) -> f32 {
        f32::from_bits(self.send.load(Ordering::Relaxed))
    }

    pub fn set_send(&self, val: f32) {
        self.send
            .store(val.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn is_polyphonic(&self) -> bool {
        self.polyphonic.load(Ordering::Relaxed)
    }

    pub fn toggle_polyphonic(&self) {
        self.polyphonic.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn get_timebase(&self) -> Timebase {
        Timebase::from_index(self.timebase.load(Ordering::Relaxed))
    }

    pub fn set_timebase(&self, tb: Timebase) {
        self.timebase.store(tb as u32, Ordering::Relaxed);
    }

    /// Cycle to next timebase value.
    pub fn next_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = (cur + 1) % Timebase::COUNT as u32;
        self.timebase.store(next, Ordering::Relaxed);
    }

    /// Cycle to previous timebase value.
    pub fn prev_timebase(&self) {
        let cur = self.timebase.load(Ordering::Relaxed);
        let next = if cur == 0 {
            Timebase::COUNT as u32 - 1
        } else {
            cur - 1
        };
        self.timebase.store(next, Ordering::Relaxed);
    }
}

// ── Pattern snapshot (for inactive patterns in the bank) ──

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
pub struct TrackPresetMeta {
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

#[derive(Clone)]
pub struct PatternSnapshot {
    pub track_bits: Vec<u64>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<TrackParamsSnapshot>,
    pub effect_slots: Vec<Vec<EffectSlotSnapshot>>,
    pub instrument_slots: Vec<EffectSlotSnapshot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_preset_meta: Vec<TrackPresetMeta>,
    /// Per-track (buffer_id, sample_name). -1 means no sample assigned.
    pub sample_ids: Vec<(i32, String)>,
    /// Per-track chord data snapshots.
    pub chord_snapshots: Vec<ChordSnapshot>,
    /// Per-track timebase p-lock snapshots.
    pub timebase_plock_snapshots: Vec<[Option<u32>; MAX_STEPS]>,
    /// Per-track instrument type.
    pub instrument_types: Vec<InstrumentType>,
}

impl PatternSnapshot {
    pub fn capture(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Self {
        let mut track_bits = Vec::with_capacity(num_tracks);
        let mut step_data = Vec::with_capacity(num_tracks);
        let mut track_params = Vec::with_capacity(num_tracks);
        let mut effect_slots = Vec::with_capacity(num_tracks);
        let mut instrument_slots = Vec::with_capacity(num_tracks);
        let mut instrument_base_note_offsets = Vec::with_capacity(num_tracks);
        let track_preset_meta = state.track_preset_meta.lock().unwrap();
        let mut preset_meta = Vec::with_capacity(num_tracks);
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut chord_snapshots = Vec::with_capacity(num_tracks);
        let mut timebase_plock_snapshots = Vec::with_capacity(num_tracks);
        let mut inst_types = Vec::with_capacity(num_tracks);

        for t in 0..num_tracks {
            track_bits.push(state.patterns[t].load_bits());

            let mut steps = Vec::with_capacity(MAX_STEPS);
            for s in 0..MAX_STEPS {
                let mut params = [0.0f32; NUM_PARAMS];
                for p in StepParam::ALL {
                    params[p.index()] = state.step_data[t].get(s, p);
                }
                steps.push(params);
            }
            step_data.push(steps);

            let tp = &state.track_params[t];
            track_params.push(TrackParamsSnapshot {
                gate: tp.is_gate_on(),
                attack_ms: tp.get_attack_ms(),
                release_ms: tp.get_release_ms(),
                swing: tp.get_swing(),
                num_steps: tp.get_num_steps(),
                send: tp.get_send(),
                polyphonic: tp.is_polyphonic(),
                timebase: tp.get_timebase(),
            });

            // Capture effect chain
            let chain: Vec<EffectSlotSnapshot> = state.effect_chains[t]
                .iter()
                .map(|slot| EffectSlotSnapshot::capture(slot))
                .collect();
            effect_slots.push(chain);
            instrument_slots.push(EffectSlotSnapshot::capture(&state.instrument_slots[t]));
            instrument_base_note_offsets.push(f32::from_bits(
                state.instrument_base_note_offsets[t].load(Ordering::Relaxed),
            ));
            preset_meta.push(track_preset_meta.get(t).cloned().unwrap_or_default());

            // Capture sample assignment
            let buf_id = if t < track_buffer_ids.len() {
                track_buffer_ids[t]
            } else {
                -1
            };
            let name = if t < track_names.len() {
                track_names[t].clone()
            } else {
                String::new()
            };
            sample_ids.push((buf_id, name));

            // Capture chord data
            chord_snapshots.push(ChordSnapshot::capture(&state.chord_data[t]));

            // Capture timebase p-locks
            timebase_plock_snapshots.push(state.timebase_plocks[t].snapshot());

            // Capture instrument type
            inst_types.push(if t < instrument_types.len() {
                instrument_types[t]
            } else {
                InstrumentType::Sampler
            });
        }

        Self {
            track_bits,
            step_data,
            track_params,
            effect_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_preset_meta: preset_meta,
            sample_ids,
            chord_snapshots,
            timebase_plock_snapshots,
            instrument_types: inst_types,
        }
    }

    pub fn restore(&self, state: &SequencerState) {
        let num_tracks = self.track_bits.len();
        let mut track_preset_meta = state.track_preset_meta.lock().unwrap();
        for t in 0..num_tracks {
            state.patterns[t].store_bits(self.track_bits[t]);

            for s in 0..MAX_STEPS {
                for p in StepParam::ALL {
                    state.step_data[t].set(s, p, self.step_data[t][s][p.index()]);
                }
            }

            let tp = &state.track_params[t];
            let snap = &self.track_params[t];
            tp.gate.store(snap.gate, Ordering::Relaxed);
            tp.set_attack_ms(snap.attack_ms);
            tp.set_release_ms(snap.release_ms);
            tp.set_swing(snap.swing);
            tp.set_num_steps(snap.num_steps);
            tp.set_send(snap.send);
            tp.polyphonic.store(snap.polyphonic, Ordering::Relaxed);
            tp.set_timebase(snap.timebase);

            // Restore effect chain slots
            for (slot_idx, slot_snap) in self.effect_slots[t].iter().enumerate() {
                if slot_idx < state.effect_chains[t].len() {
                    slot_snap.restore(&state.effect_chains[t][slot_idx]);
                }
            }

            if t < self.instrument_slots.len() {
                self.instrument_slots[t].restore(&state.instrument_slots[t]);
            }
            if t < self.instrument_base_note_offsets.len() {
                state.instrument_base_note_offsets[t].store(
                    self.instrument_base_note_offsets[t].to_bits(),
                    Ordering::Relaxed,
                );
            }
            if t < self.track_preset_meta.len() && t < track_preset_meta.len() {
                track_preset_meta[t] = self.track_preset_meta[t].clone();
            }

            // Restore chord data
            if t < self.chord_snapshots.len() {
                self.chord_snapshots[t].restore(&state.chord_data[t]);
            }

            // Restore timebase p-locks
            if t < self.timebase_plock_snapshots.len() {
                state.timebase_plocks[t].restore(&self.timebase_plock_snapshots[t]);
            }
        }
    }

    fn default_step_data() -> Vec<[f32; NUM_PARAMS]> {
        (0..MAX_STEPS)
            .map(|_| {
                let mut params = [0.0f32; NUM_PARAMS];
                for p in StepParam::ALL {
                    params[p.index()] = p.default_value();
                }
                params
            })
            .collect()
    }

    fn default_effect_slots(
        t: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) -> Vec<EffectSlotSnapshot> {
        if t < slot_descriptors.len() {
            slot_descriptors[t]
                .iter()
                .map(|desc| EffectSlotSnapshot::new_default(desc, 0))
                .collect()
        } else {
            Vec::new()
        }
    }

    fn default_instrument_slot() -> EffectSlotSnapshot {
        EffectSlotSnapshot::new_empty()
    }

    /// Push a single default track onto this snapshot.
    fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
        self.track_bits.push(0u64);
        self.step_data.push(Self::default_step_data());
        self.track_params.push(TrackParamsSnapshot::default());
        self.effect_slots
            .push(Self::default_effect_slots(t, slot_descriptors));
        self.instrument_slots.push(Self::default_instrument_slot());
        self.instrument_base_note_offsets.push(0.0);
        self.track_preset_meta.push(TrackPresetMeta::default());
        self.sample_ids.push((-1, String::new()));
        self.chord_snapshots.push(ChordSnapshot::new_default());
        self.timebase_plock_snapshots.push([None; MAX_STEPS]);
        self.instrument_types.push(InstrumentType::Sampler);
    }

    pub fn new_default(num_tracks: usize, slot_descriptors: &[Vec<EffectDescriptor>]) -> Self {
        let mut snap = Self {
            track_bits: Vec::with_capacity(num_tracks),
            step_data: Vec::with_capacity(num_tracks),
            track_params: Vec::with_capacity(num_tracks),
            effect_slots: Vec::with_capacity(num_tracks),
            instrument_slots: Vec::with_capacity(num_tracks),
            instrument_base_note_offsets: Vec::with_capacity(num_tracks),
            track_preset_meta: Vec::with_capacity(num_tracks),
            sample_ids: Vec::with_capacity(num_tracks),
            chord_snapshots: Vec::with_capacity(num_tracks),
            timebase_plock_snapshots: Vec::with_capacity(num_tracks),
            instrument_types: Vec::with_capacity(num_tracks),
        };
        for t in 0..num_tracks {
            snap.push_default_track(t, slot_descriptors);
        }
        snap
    }

    /// Extend a snapshot to cover more tracks (for when tracks are dynamically added).
    pub fn extend_to_tracks(
        &mut self,
        new_count: usize,
        slot_descriptors: &[Vec<EffectDescriptor>],
    ) {
        while self.track_bits.len() < new_count {
            let t = self.track_bits.len();
            self.push_default_track(t, slot_descriptors);
        }
    }
}

/// Build a default empty effect chain for unused track slots.
pub fn default_empty_effect_chain() -> Vec<EffectSlotState> {
    use crate::lisp_effect::MAX_CUSTOM_FX;
    let filter_desc = EffectDescriptor::builtin_filter();
    let delay_desc = EffectDescriptor::builtin_delay();
    let filter_slot = EffectSlotState::new(&filter_desc, 0);
    let delay_slot = EffectSlotState::new(&delay_desc, 0);
    let mut chain = vec![filter_slot, delay_slot];
    for _ in 0..MAX_CUSTOM_FX {
        chain.push(EffectSlotState::empty());
    }
    chain
}

/// Shared state visible to both audio thread and UI thread.
pub struct SequencerState {
    pub patterns: Vec<TrackPattern>,
    pub step_data: Vec<StepData>,
    pub chord_data: Vec<ChordData>,
    pub track_params: Vec<TrackParams>,
    pub effect_chains: Vec<Vec<EffectSlotState>>,
    pub playhead: AtomicU32,
    pub playing: AtomicBool,
    pub bpm: AtomicU32,
    /// Peak level for L channel (f32 bits, 0.0..1.0+), updated by audio thread.
    pub peak_l: AtomicU32,
    /// Peak level for R channel (f32 bits, 0.0..1.0+), updated by audio thread.
    pub peak_r: AtomicU32,
    /// Per-track trigger flash intensity (0-255). Audio writes 255, UI decays each frame.
    pub trigger_flash: Vec<AtomicU32>,
    /// Inactive patterns stored here (only accessed by UI thread during switches).
    pub pattern_bank: Mutex<Vec<PatternSnapshot>>,
    /// Index of the currently active pattern.
    pub current_pattern: AtomicU32,
    /// Total number of patterns.
    pub num_patterns: AtomicU32,
    /// Live track count, read by audio thread.
    pub num_tracks: AtomicU32,
    /// Sampler node logical IDs, pre-allocated to MAX_TRACKS.
    pub sampler_lids: Vec<AtomicU64>,
    /// Delay node logical IDs, pre-allocated to MAX_TRACKS.
    pub delay_lids: Vec<AtomicU64>,
    /// Send gain node logical IDs, pre-allocated to MAX_TRACKS.
    pub send_lids: Vec<AtomicU64>,
    /// Per-track voice logical IDs (up to MAX_VOICES per track).
    pub voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    /// Number of voices per track.
    pub voice_counts: Vec<AtomicU32>,
    /// Per-track step position, written by the per-track clock in audio thread.
    pub track_playheads: Vec<AtomicU32>,
    /// Per-track, per-step timebase p-locks.
    pub timebase_plocks: Vec<TimebasePLockData>,
    /// Fractional phase within current step (0.0–1.0), written by audio thread.
    /// Used by UI to round-to-nearest-step when recording keyboard input.
    pub playhead_phase: AtomicU32,
    /// Recording quantize threshold (0.0–1.0). Key presses landing past this
    /// phase within a step snap to the next step. Default 0.5 (midpoint).
    /// Adjust with [ / ] when armed to compensate for output latency.
    pub record_quantize_thresh: AtomicU32,
    /// Per-track instrument type flag: 0=Sampler, 1=Custom. Read by audio thread.
    pub instrument_type_flags: Vec<AtomicU32>,
    /// Per-track synth node IDs (up to MAX_VOICES per track), for sending instrument params.
    pub synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    /// Per-track engine binding. `u32::MAX` means no engine / sampler track.
    pub track_engine_ids: Vec<AtomicU32>,
    /// Per-engine custom voice logical IDs (GatePitch node LIDs).
    pub engine_voice_lids: Vec<[AtomicU64; MAX_VOICES]>,
    /// Per-engine synth node IDs.
    pub engine_synth_node_ids: Vec<[AtomicU32; MAX_VOICES]>,
    /// Number of voices allocated for each engine.
    pub engine_voice_counts: Vec<AtomicU32>,
    /// Per-engine, per-voice, per-track route gain logical IDs.
    pub engine_route_lids: Vec<[[AtomicU64; MAX_TRACKS]; MAX_VOICES]>,
    /// Per-track instrument param slots (synth params for custom instruments).
    pub instrument_slots: Vec<EffectSlotState>,
    /// Per-track host-side semitone offset for custom instrument pitch interpretation.
    pub instrument_base_note_offsets: Vec<AtomicU32>,
    /// Per-track loaded preset metadata for custom instruments.
    pub track_preset_meta: Mutex<Vec<TrackPresetMeta>>,
}

#[derive(Clone)]
struct StepSlotPlocks {
    params: Vec<Option<f32>>,
}

#[derive(Clone)]
struct StepSnapshot {
    active: bool,
    params: [f32; NUM_PARAMS],
    chord: Vec<f32>,
    timebase: Option<Timebase>,
    effect_plocks: Vec<StepSlotPlocks>,
    instrument_plocks: StepSlotPlocks,
}

impl SequencerState {
    pub fn new(num_tracks: usize, initial_chains: Vec<Vec<EffectSlotState>>) -> Self {
        let patterns: Vec<TrackPattern> = (0..MAX_TRACKS).map(|_| TrackPattern::new()).collect();
        let step_data: Vec<StepData> = (0..MAX_TRACKS).map(|_| StepData::new()).collect();
        let track_params: Vec<TrackParams> = (0..MAX_TRACKS).map(|_| TrackParams::new()).collect();
        let trigger_flash: Vec<AtomicU32> = (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect();

        let mut effect_chains = initial_chains;
        for _ in effect_chains.len()..MAX_TRACKS {
            effect_chains.push(default_empty_effect_chain());
        }

        // Build slot descriptors for default pattern snapshot
        let slot_descriptors: Vec<Vec<EffectDescriptor>> = (0..num_tracks)
            .map(|_| EffectDescriptor::default_full_chain())
            .collect();

        let chord_data: Vec<ChordData> = (0..MAX_TRACKS).map(|_| ChordData::new()).collect();

        Self {
            patterns,
            step_data,
            chord_data,
            track_params,
            effect_chains,
            playhead: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            bpm: AtomicU32::new(DEFAULT_BPM),
            peak_l: AtomicU32::new(0.0_f32.to_bits()),
            peak_r: AtomicU32::new(0.0_f32.to_bits()),
            trigger_flash,
            pattern_bank: Mutex::new(vec![PatternSnapshot::new_default(
                num_tracks,
                &slot_descriptors,
            )]),
            current_pattern: AtomicU32::new(0),
            num_patterns: AtomicU32::new(1),
            num_tracks: AtomicU32::new(num_tracks as u32),
            sampler_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
            delay_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
            send_lids: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
            voice_lids: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            track_playheads: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            timebase_plocks: (0..MAX_TRACKS).map(|_| TimebasePLockData::new()).collect(),
            playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
            record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
            instrument_type_flags: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            synth_node_ids: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                .collect(),
            track_engine_ids: (0..MAX_TRACKS).map(|_| AtomicU32::new(u32::MAX)).collect(),
            engine_voice_lids: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                .collect(),
            engine_synth_node_ids: (0..MAX_TRACKS)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                .collect(),
            engine_voice_counts: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            engine_route_lids: (0..MAX_TRACKS)
                .map(|_| {
                    std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0)))
                })
                .collect(),
            instrument_slots: (0..MAX_TRACKS).map(|_| EffectSlotState::empty()).collect(),
            instrument_base_note_offsets: (0..MAX_TRACKS)
                .map(|_| AtomicU32::new(0.0_f32.to_bits()))
                .collect(),
            track_preset_meta: Mutex::new(
                (0..MAX_TRACKS)
                    .map(|_| TrackPresetMeta::default())
                    .collect(),
            ),
        }
    }

    pub fn active_track_count(&self) -> usize {
        self.num_tracks.load(Ordering::Acquire) as usize
    }

    pub fn current_step(&self) -> usize {
        self.playhead.load(Ordering::Relaxed) as usize
    }

    /// Get the current step position for a specific track (per-track timebase).
    pub fn track_step(&self, track: usize) -> usize {
        self.track_playheads[track].load(Ordering::Relaxed) as usize
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    pub fn toggle_play(&self) {
        self.playing.fetch_xor(true, Ordering::Relaxed);
    }

    /// Switch to a different pattern. Returns the sample_ids from the restored
    /// pattern so the UI can apply buffer swaps. Returns None if no switch occurred.
    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern_bank.lock().unwrap();
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        if new_idx == cur || new_idx >= bank.len() {
            return None;
        }
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        bank[new_idx].restore(self);
        self.current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        Some(bank[new_idx].sample_ids.clone())
    }

    pub fn clone_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> usize {
        let mut bank = self.pattern_bank.lock().unwrap();
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        let cloned = bank[cur].clone();
        bank.push(cloned);
        let new_idx = bank.len() - 1;
        self.current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        self.num_patterns
            .store(bank.len() as u32, Ordering::Relaxed);
        new_idx
    }

    /// Delete current pattern. Returns the sample_ids from the restored adjacent
    /// pattern so the UI can apply buffer swaps. Returns None if only 1 pattern.
    pub fn delete_pattern(
        &self,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
        instrument_types: &[InstrumentType],
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern_bank.lock().unwrap();
        if bank.len() <= 1 {
            return None;
        }
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        // Capture current before removing (so other patterns stay consistent)
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names, instrument_types);
        bank.remove(cur);
        let new_idx = cur.min(bank.len() - 1);
        bank[new_idx].restore(self);
        self.current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        self.num_patterns
            .store(bank.len() as u32, Ordering::Relaxed);
        Some(bank[new_idx].sample_ids.clone())
    }

    /// Toggle a step. When toggling OFF, clear all effect slot p-locks.
    pub fn toggle_step_and_clear_plocks(&self, track: usize, step: usize) {
        let was_active = self.patterns[track].is_active(step);
        self.patterns[track].toggle_step(step);
        if was_active {
            for slot in &self.effect_chains[track] {
                slot.plocks.clear_step(step);
            }
            self.chord_data[track].clear_step(step);
        }
    }

    fn capture_step_snapshot(&self, track: usize, step: usize) -> StepSnapshot {
        let mut params = [0.0; NUM_PARAMS];
        for param in StepParam::ALL {
            params[param.index()] = self.step_data[track].get(step, param);
        }

        let chord_count = self.chord_data[track].count(step);
        let mut chord = Vec::with_capacity(chord_count);
        for note_idx in 0..chord_count {
            chord.push(self.chord_data[track].get(step, note_idx));
        }

        let mut effect_plocks = Vec::with_capacity(self.effect_chains[track].len());
        for slot in &self.effect_chains[track] {
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            let mut params = Vec::with_capacity(num_params);
            for param_idx in 0..num_params {
                params.push(slot.plocks.get(step, param_idx));
            }
            effect_plocks.push(StepSlotPlocks { params });
        }

        let instrument_slot = &self.instrument_slots[track];
        let instrument_param_count = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
        let mut instrument_plocks = Vec::with_capacity(instrument_param_count);
        for param_idx in 0..instrument_param_count {
            instrument_plocks.push(instrument_slot.plocks.get(step, param_idx));
        }

        StepSnapshot {
            active: self.patterns[track].is_active(step),
            params,
            chord,
            timebase: self.timebase_plocks[track].get(step),
            effect_plocks,
            instrument_plocks: StepSlotPlocks {
                params: instrument_plocks,
            },
        }
    }

    fn clear_step_payload(&self, track: usize, step: usize) {
        for param in StepParam::ALL {
            self.step_data[track].set(step, param, param.default_value());
        }

        let mut bits = self.patterns[track].load_bits();
        bits &= !(1u64 << step);
        self.patterns[track].store_bits(bits);

        self.chord_data[track].clear_step(step);
        self.timebase_plocks[track].clear(step);

        for slot in &self.effect_chains[track] {
            slot.plocks.clear_step(step);
        }

        for param_idx in 0..MAX_SLOT_PARAMS {
            self.instrument_slots[track]
                .plocks
                .clear_param(step, param_idx);
        }
    }

    fn restore_step_snapshot(&self, track: usize, step: usize, snapshot: &StepSnapshot) {
        for param in StepParam::ALL {
            self.step_data[track].set(step, param, snapshot.params[param.index()]);
        }

        let mut bits = self.patterns[track].load_bits();
        if snapshot.active {
            bits |= 1u64 << step;
        } else {
            bits &= !(1u64 << step);
        }
        self.patterns[track].store_bits(bits);

        self.chord_data[track].clear_step(step);
        for &transpose in &snapshot.chord {
            self.chord_data[track].add_note(step, transpose);
        }

        match snapshot.timebase {
            Some(tb) => self.timebase_plocks[track].set(step, tb),
            None => self.timebase_plocks[track].clear(step),
        }

        for (slot_idx, slot) in self.effect_chains[track].iter().enumerate() {
            let saved = snapshot.effect_plocks.get(slot_idx);
            let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
            for param_idx in 0..num_params {
                let val = saved
                    .and_then(|plocks| plocks.params.get(param_idx))
                    .copied()
                    .flatten();
                match val {
                    Some(val) => slot.plocks.set(step, param_idx, val),
                    None => slot.plocks.clear_param(step, param_idx),
                }
            }
        }

        let instrument_slot = &self.instrument_slots[track];
        let instrument_param_count = instrument_slot.num_params.load(Ordering::Relaxed) as usize;
        for param_idx in 0..instrument_param_count {
            match snapshot
                .instrument_plocks
                .params
                .get(param_idx)
                .copied()
                .flatten()
            {
                Some(val) => instrument_slot.plocks.set(step, param_idx, val),
                None => instrument_slot.plocks.clear_param(step, param_idx),
            }
        }
    }

    pub fn move_step_range(&self, track: usize, lo: usize, hi: usize, new_lo: usize) {
        if lo > hi || hi >= MAX_STEPS {
            return;
        }

        let count = hi - lo + 1;
        let new_hi = new_lo + count - 1;
        if new_lo == lo || new_hi >= MAX_STEPS {
            return;
        }

        let snapshots: Vec<_> = (lo..=hi)
            .map(|step| self.capture_step_snapshot(track, step))
            .collect();

        for step in lo..=hi {
            if step < new_lo || step > new_hi {
                self.clear_step_payload(track, step);
            }
        }

        for (offset, step) in (new_lo..=new_hi).enumerate() {
            self.restore_step_snapshot(track, step, &snapshots[offset]);
        }
    }

    /// Double the track's pattern length, copying triggers, step params, and p-locks
    /// into the new second half so the pattern repeats. Returns the new step count.
    pub fn duplicate_track_pattern(&self, track: usize) -> usize {
        let num_steps = self.track_params[track].get_num_steps();
        let new_len = (num_steps * 2).min(MAX_STEPS);
        if new_len == num_steps {
            return num_steps;
        }

        // Copy trigger bits
        let bits = self.patterns[track].load_bits();
        let mut new_bits = bits;
        for step in num_steps..new_len {
            let src = step - num_steps;
            if (bits >> src) & 1 == 1 {
                new_bits |= 1u64 << step;
            } else {
                new_bits &= !(1u64 << step);
            }
        }
        self.patterns[track].store_bits(new_bits);

        // Copy step data
        for step in num_steps..new_len {
            let src = step - num_steps;
            for param in StepParam::ALL {
                let val = self.step_data[track].get(src, param);
                self.step_data[track].set(step, param, val);
            }
        }

        // Copy effect p-locks
        for slot in &self.effect_chains[track] {
            let np = slot.num_params.load(Ordering::Relaxed) as usize;
            for step in num_steps..new_len {
                let src = step - num_steps;
                for p in 0..np {
                    match slot.plocks.get(src, p) {
                        Some(val) => slot.plocks.set(step, p, val),
                        None => slot.plocks.clear_param(step, p),
                    }
                }
            }
        }

        // Copy chord data
        for step in num_steps..new_len {
            let src = step - num_steps;
            self.chord_data[track].copy_step(src, step);
        }

        // Copy timebase p-locks
        for step in num_steps..new_len {
            let src = step - num_steps;
            match self.timebase_plocks[track].get(src) {
                Some(tb) => self.timebase_plocks[track].set(step, tb),
                None => self.timebase_plocks[track].clear(step),
            }
        }

        self.track_params[track].set_num_steps(new_len);
        new_len
    }

    /// Halve the track's pattern length. Data beyond the new length is retained
    /// but not played. Returns the new step count.
    pub fn halve_track_pattern(&self, track: usize) -> usize {
        let num_steps = self.track_params[track].get_num_steps();
        let new_len = (num_steps / 2).max(1);
        if new_len == num_steps {
            return num_steps;
        }
        self.track_params[track].set_num_steps(new_len);
        new_len
    }
}

/// Per-step chord storage for polyphonic patterns.
/// Each step can hold up to MAX_VOICES notes (transpose values).
/// When count == 0, the step uses the single StepParam::Transpose (backward compat).
/// When count > 0, the chord notes are used instead.
pub struct ChordData {
    /// Transpose values stored as f32 bits, [MAX_STEPS * MAX_VOICES].
    transposes: [AtomicU32; MAX_STEPS * MAX_VOICES],
    /// Number of notes per step.
    counts: [AtomicU32; MAX_STEPS],
}

impl ChordData {
    pub fn new() -> Self {
        Self {
            transposes: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            counts: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Number of chord notes at this step (0 = single-note mode).
    pub fn count(&self, step: usize) -> usize {
        self.counts[step].load(Ordering::Relaxed) as usize
    }

    /// Get the transpose value for note `n` at `step`.
    pub fn get(&self, step: usize, n: usize) -> f32 {
        f32::from_bits(self.transposes[step * MAX_VOICES + n].load(Ordering::Relaxed))
    }

    /// Add a note to the chord at `step`. Returns false if full.
    pub fn add_note(&self, step: usize, transpose: f32) -> bool {
        let c = self.counts[step].load(Ordering::Relaxed) as usize;
        if c >= MAX_VOICES {
            return false;
        }
        self.transposes[step * MAX_VOICES + c].store(transpose.to_bits(), Ordering::Relaxed);
        self.counts[step].store((c + 1) as u32, Ordering::Relaxed);
        true
    }

    /// Clear all notes at `step`.
    pub fn clear_step(&self, step: usize) {
        self.counts[step].store(0, Ordering::Relaxed);
    }

    /// Copy chord data from `src` step to `dst` step.
    pub fn copy_step(&self, src: usize, dst: usize) {
        let c = self.counts[src].load(Ordering::Relaxed);
        self.counts[dst].store(c, Ordering::Relaxed);
        for n in 0..(c as usize).min(MAX_VOICES) {
            let val = self.transposes[src * MAX_VOICES + n].load(Ordering::Relaxed);
            self.transposes[dst * MAX_VOICES + n].store(val, Ordering::Relaxed);
        }
    }
}

/// Snapshot of chord data for one track.
#[derive(Clone)]
pub struct ChordSnapshot {
    /// Per-step: Vec of transpose values.
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
        Self {
            steps: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
        }
    }
}

/// Keyboard trigger event sent from UI to audio thread.
pub struct KeyboardTrigger {
    pub track: usize,
    pub transpose: f32,
    pub velocity: f32,
    /// If true, this is a note-off (release) event.
    pub note_off: bool,
}

/// Trigger event: which track and step fired, and at what sample offset within the block.
pub struct Trigger {
    pub track: usize,
    pub step: usize,
    pub offset: usize,
}

/// Per-step timebase p-lock data (one per track).
/// Stores an optional `Timebase` index per step. `u32::MAX` = no override.
pub struct TimebasePLockData {
    overrides: [AtomicU32; MAX_STEPS],
}

impl TimebasePLockData {
    pub fn new() -> Self {
        Self {
            overrides: std::array::from_fn(|_| AtomicU32::new(u32::MAX)),
        }
    }

    /// Get the timebase override for a step, or None if no p-lock.
    pub fn get(&self, step: usize) -> Option<Timebase> {
        let v = self.overrides[step].load(Ordering::Relaxed);
        if v == u32::MAX {
            None
        } else {
            Some(Timebase::from_index(v))
        }
    }

    /// Set a timebase p-lock for a step.
    pub fn set(&self, step: usize, tb: Timebase) {
        self.overrides[step].store(tb as u32, Ordering::Relaxed);
    }

    /// Clear the p-lock for a step (revert to track default).
    pub fn clear(&self, step: usize) {
        self.overrides[step].store(u32::MAX, Ordering::Relaxed);
    }

    /// Check if a step has a p-lock.
    pub fn has_plock(&self, step: usize) -> bool {
        self.overrides[step].load(Ordering::Relaxed) != u32::MAX
    }

    /// Resolve timebase for a step: p-lock if set, else track default.
    pub fn resolve(&self, step: usize, default: Timebase) -> Timebase {
        self.get(step).unwrap_or(default)
    }

    /// Snapshot all overrides as Option<Timebase>.
    pub fn snapshot(&self) -> [Option<u32>; MAX_STEPS] {
        std::array::from_fn(|i| {
            let v = self.overrides[i].load(Ordering::Relaxed);
            if v == u32::MAX {
                None
            } else {
                Some(v)
            }
        })
    }

    /// Restore from snapshot.
    pub fn restore(&self, snap: &[Option<u32>; MAX_STEPS]) {
        for (i, v) in snap.iter().enumerate() {
            self.overrides[i].store(v.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
    }
}

/// Per-track clock state. Step position is derived from global beat counter,
/// not accumulated — drift is impossible by construction.
pub struct TrackClockState {
    /// Last local step index (for detecting step transitions → triggers).
    pub last_local_step: u32,
    /// Cached samples_per_step for the current step (used by audio for gate/chop).
    pub cached_sps: f64,
    /// Precomputed cumulative beat boundaries for each step in the pattern.
    /// boundaries[s] = beat offset where step s starts. boundaries[num_steps] = end of last step.
    pub boundaries: [f64; MAX_STEPS + 1],
    /// Precomputed end-of-step positions (may differ from boundaries[s+1] when dead space exists).
    pub step_ends: [f64; MAX_STEPS],
    /// Total cycle length in beats.
    pub cycle_beats: f64,
}

/// Clock that runs in the audio callback, counting beats and emitting
/// per-track triggers. Uses beat-based derivation so tracks never drift
/// from the global bar grid regardless of mixed timebases or p-locks.
pub struct SequencerClock {
    sample_rate: f64,
    /// Total beats (quarter notes) elapsed since play start.
    /// THE single source of truth for all timing.
    total_beats: f64,
    /// Per-track state.
    pub track_clocks: Vec<TrackClockState>,
    was_playing: bool,
}

impl SequencerClock {
    pub fn new(sample_rate: u32, _bpm: u32) -> Self {
        let track_clocks = (0..MAX_TRACKS)
            .map(|_| TrackClockState {
                last_local_step: u32::MAX,
                cached_sps: 0.0,
                boundaries: [0.0; MAX_STEPS + 1],
                step_ends: [0.0; MAX_STEPS],
                cycle_beats: 4.0,
            })
            .collect();
        Self {
            sample_rate: sample_rate as f64,
            total_beats: 0.0,
            track_clocks,
            was_playing: false,
        }
    }

    /// Get the cached samples_per_step for a track's current step.
    pub fn samples_per_step_for_track(&self, track: usize) -> f64 {
        self.track_clocks[track].cached_sps
    }

    /// Global 16th-note samples_per_step (for backward compat).
    pub fn current_samples_per_step(&self) -> f64 {
        // Derived from current BPM — callers should use samples_per_step_for_track when possible
        // This is a fallback; the actual value is set each block.
        self.sample_rate * 60.0 / 120.0 / 4.0 // placeholder, overwritten in process_block
    }

    /// Precompute step beat-boundaries for a track (call once per block per track).
    /// Steps pack contiguously; Sync step params insert dead space before a step
    /// to align it to a grid resolution.
    fn precompute_boundaries(&mut self, state: &SequencerState, track: usize) {
        const EPS: f64 = 1e-9;

        let tp = &state.track_params[track];
        let ns = tp.get_num_steps();
        let default_tb = tp.get_timebase();
        let tc = &mut self.track_clocks[track];

        let mut accum = 0.0;
        let sd = &state.step_data[track];

        for s in 0..ns {
            let tb = state.timebase_plocks[track].resolve(s, default_tb);
            let step_dur = tb.step_beats(ns);

            // Sync point: pad to next multiple of the sync resolution
            let sync_b = sync_beats(sd.get(s, StepParam::Sync));
            if sync_b > EPS {
                accum = ceil_to_grid(accum, sync_b);
            }

            tc.boundaries[s] = accum;
            tc.step_ends[s] = accum + step_dur;
            accum += step_dur;
        }

        // boundaries[ns] = end of last step (where dead space begins)
        tc.boundaries[ns] = accum;

        // Cycle = natural pattern length by default.
        // If step 0 has a sync value, snap cycle length to that resolution
        // so step 0 always lands on the grid when the pattern loops.
        let sync0_b = sync_beats(sd.get(0, StepParam::Sync));
        tc.cycle_beats = if sync0_b > EPS {
            ceil_to_grid(accum, sync0_b).max(EPS)
        } else {
            accum.max(EPS)
        };
    }

    /// Derive the local step index from the current beat position within a track's cycle.
    /// Returns None if we're in dead space (past all steps, waiting for next bar boundary).
    fn derive_local_step(
        tc: &TrackClockState,
        pos_in_cycle: f64,
        num_steps: usize,
    ) -> Option<usize> {
        // Past the end of the last step? Dead space until next cycle.
        if pos_in_cycle >= tc.boundaries[num_steps] {
            return None;
        }
        // Binary search: find the last boundary <= pos_in_cycle.
        // partition_point returns the first index where boundary > pos_in_cycle.
        let idx = tc.boundaries[..num_steps + 1].partition_point(|&b| b <= pos_in_cycle);
        let s = if idx > 0 { idx - 1 } else { 0 };
        // Check we're still within this step's actual duration
        // (there may be dead-space padding between step_ends[s] and boundaries[s+1])
        if pos_in_cycle < tc.step_ends[s] {
            Some(s)
        } else {
            None
        }
    }

    pub fn process_block(&mut self, nframes: usize, state: &SequencerState) -> Vec<Trigger> {
        if !state.is_playing() {
            self.was_playing = false;
            return Vec::new();
        }

        let bpm = state.bpm.load(Ordering::Relaxed) as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let samples_per_quarter = self.sample_rate * 60.0 / bpm;
        let num_tracks = state.active_track_count();

        if !self.was_playing {
            self.was_playing = true;
            self.total_beats = 0.0;
            for t in 0..MAX_TRACKS {
                self.track_clocks[t].last_local_step = u32::MAX;
            }
        }

        // Precompute beat-boundaries for all active tracks
        for t in 0..num_tracks {
            self.precompute_boundaries(state, t);
        }

        let mut triggers = Vec::new();
        let mut last_global_16th = (self.total_beats / 0.25) as u32;

        for offset in 0..nframes {
            self.total_beats += beats_per_sample;

            // Global playhead: only store when value changes
            let global_16th = (self.total_beats / 0.25) as u32;
            if global_16th != last_global_16th {
                state.playhead.store(global_16th, Ordering::Relaxed);
                last_global_16th = global_16th;
            }

            // Per-track step derivation
            for t in 0..num_tracks {
                let ns = state.track_params[t].get_num_steps();
                let tc = &self.track_clocks[t];
                let cycle = tc.cycle_beats;
                if cycle <= 0.0 {
                    continue;
                }

                // Position within the bar-snapped cycle
                let pos_in_cycle = self.total_beats % cycle;

                match Self::derive_local_step(tc, pos_in_cycle, ns) {
                    Some(step) => {
                        let step_u32 = step as u32;
                        if step_u32 != self.track_clocks[t].last_local_step {
                            let tc = &mut self.track_clocks[t];
                            tc.last_local_step = step_u32;

                            // Update cached sps for gate/chop calculations
                            let default_tb = state.track_params[t].get_timebase();
                            let tb = state.timebase_plocks[t].resolve(step, default_tb);
                            tc.cached_sps = tb.step_beats(ns) * samples_per_quarter;

                            // Publish track playhead for UI
                            state.track_playheads[t].store(step_u32, Ordering::Relaxed);

                            triggers.push(Trigger {
                                track: t,
                                step,
                                offset,
                            });
                        }
                    }
                    None => {
                        // Dead space: past all steps, waiting for next bar boundary.
                        // Use a sentinel so we re-trigger step 0 when the cycle wraps.
                        self.track_clocks[t].last_local_step = u32::MAX;
                    }
                }
            }
        }

        // Publish fractional phase for recording quantize
        let phase_16th = (self.total_beats / 0.25).fract() as f32;
        state
            .playhead_phase
            .store(phase_16th.to_bits(), Ordering::Relaxed);

        triggers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_step_range_preserves_chords_and_step_plocks() {
        let state = SequencerState::new(
            1,
            vec![vec![EffectSlotState::new(
                &EffectDescriptor::builtin_filter(),
                1,
            )]],
        );
        state.track_params[0].set_num_steps(8);
        state.instrument_slots[0].apply_descriptor(&EffectDescriptor::builtin_delay(), 2);

        state.patterns[0].toggle_step(1);
        state.step_data[0].set(1, StepParam::Velocity, 0.6);
        state.chord_data[0].add_note(1, 0.0);
        state.chord_data[0].add_note(1, 4.0);
        state.chord_data[0].add_note(1, 7.0);
        state.timebase_plocks[0].set(1, Timebase::Eighth);
        state.effect_chains[0][0].plocks.set(1, 2, 440.0);
        state.instrument_slots[0].plocks.set(1, 0, 0.75);

        state.patterns[0].toggle_step(2);
        state.step_data[0].set(2, StepParam::Velocity, 0.3);
        state.chord_data[0].add_note(2, 12.0);
        state.timebase_plocks[0].set(2, Timebase::QuarterTriplet);
        state.effect_chains[0][0].plocks.set(2, 2, 880.0);
        state.instrument_slots[0].plocks.set(2, 0, 0.25);

        state.move_step_range(0, 1, 2, 2);

        assert!(!state.patterns[0].is_active(1));
        assert_eq!(state.chord_data[0].count(1), 0);
        assert_eq!(
            state.step_data[0].get(1, StepParam::Velocity),
            StepParam::Velocity.default_value()
        );
        assert_eq!(state.timebase_plocks[0].get(1), None);
        assert_eq!(state.effect_chains[0][0].plocks.get(1, 2), None);
        assert_eq!(state.instrument_slots[0].plocks.get(1, 0), None);

        assert!(state.patterns[0].is_active(2));
        assert_eq!(state.step_data[0].get(2, StepParam::Velocity), 0.6);
        assert_eq!(state.chord_data[0].count(2), 3);
        assert_eq!(state.chord_data[0].get(2, 0), 0.0);
        assert_eq!(state.chord_data[0].get(2, 1), 4.0);
        assert_eq!(state.chord_data[0].get(2, 2), 7.0);
        assert_eq!(state.timebase_plocks[0].get(2), Some(Timebase::Eighth));
        assert_eq!(state.effect_chains[0][0].plocks.get(2, 2), Some(440.0));
        assert_eq!(state.instrument_slots[0].plocks.get(2, 0), Some(0.75));

        assert!(state.patterns[0].is_active(3));
        assert_eq!(state.step_data[0].get(3, StepParam::Velocity), 0.3);
        assert_eq!(state.chord_data[0].count(3), 1);
        assert_eq!(state.chord_data[0].get(3, 0), 12.0);
        assert_eq!(
            state.timebase_plocks[0].get(3),
            Some(Timebase::QuarterTriplet)
        );
        assert_eq!(state.effect_chains[0][0].plocks.get(3, 2), Some(880.0));
        assert_eq!(state.instrument_slots[0].plocks.get(3, 0), Some(0.25));
    }
}
