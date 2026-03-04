use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::effects::{EffectDescriptor, EffectSlotSnapshot, EffectSlotState};
use crate::voice::MAX_VOICES;

pub const MAX_TRACKS: usize = 64;
pub const MAX_STEPS: usize = 64;
pub const STEPS_PER_PAGE: usize = 16;
pub const NUM_PARAMS: usize = 7;
pub const DEFAULT_BPM: u32 = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepParam {
    Duration = 0,  // 0.0..4.0 (fraction of full sample)
    Velocity = 1,  // 0.0..1.0
    Speed = 2,     // 0.5..2.0 (playback rate)
    AuxA = 3,      // 0.0..1.0
    AuxB = 4,      // 0.0..1.0
    Transpose = 5, // -12.0..12.0 (semitones)
    Chop = 6,      // 1..8 (number of re-triggers per step)
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

    /// Params visible in the step param tabs (excludes Speed).
    pub const VISIBLE: [StepParam; 6] = [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::AuxA,
        StepParam::AuxB,
        StepParam::Transpose,
        StepParam::Chop,
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
            StepParam::Duration => ("", "d", "ur"),
            StepParam::Velocity => ("", "v", "el"),
            StepParam::Speed => ("", "s", "pd"),
            StepParam::AuxA => ("", "a", "xA"),
            StepParam::AuxB => ("ax", "B", ""),
            StepParam::Transpose => ("", "t", "rn"),
            StepParam::Chop => ("", "c", "hp"),
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
        }
    }
}

#[derive(Clone)]
pub struct PatternSnapshot {
    pub track_bits: Vec<u64>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<TrackParamsSnapshot>,
    pub effect_slots: Vec<Vec<EffectSlotSnapshot>>,
    /// Per-track (buffer_id, sample_name). -1 means no sample assigned.
    pub sample_ids: Vec<(i32, String)>,
    /// Per-track chord data snapshots.
    pub chord_snapshots: Vec<ChordSnapshot>,
}

impl PatternSnapshot {
    pub fn capture(
        state: &SequencerState,
        num_tracks: usize,
        track_buffer_ids: &[i32],
        track_names: &[String],
    ) -> Self {
        let mut track_bits = Vec::with_capacity(num_tracks);
        let mut step_data = Vec::with_capacity(num_tracks);
        let mut track_params = Vec::with_capacity(num_tracks);
        let mut effect_slots = Vec::with_capacity(num_tracks);
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut chord_snapshots = Vec::with_capacity(num_tracks);

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
            });

            // Capture effect chain
            let chain: Vec<EffectSlotSnapshot> = state.effect_chains[t]
                .iter()
                .map(|slot| EffectSlotSnapshot::capture(slot))
                .collect();
            effect_slots.push(chain);

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
        }

        Self {
            track_bits,
            step_data,
            track_params,
            effect_slots,
            sample_ids,
            chord_snapshots,
        }
    }

    pub fn restore(&self, state: &SequencerState) {
        let num_tracks = self.track_bits.len();
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

            // Restore effect chain slots
            for (slot_idx, slot_snap) in self.effect_slots[t].iter().enumerate() {
                if slot_idx < state.effect_chains[t].len() {
                    slot_snap.restore(&state.effect_chains[t][slot_idx]);
                }
            }

            // Restore chord data
            if t < self.chord_snapshots.len() {
                self.chord_snapshots[t].restore(&state.chord_data[t]);
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

    /// Push a single default track onto this snapshot.
    fn push_default_track(&mut self, t: usize, slot_descriptors: &[Vec<EffectDescriptor>]) {
        self.track_bits.push(0u64);
        self.step_data.push(Self::default_step_data());
        self.track_params.push(TrackParamsSnapshot::default());
        self.effect_slots
            .push(Self::default_effect_slots(t, slot_descriptors));
        self.sample_ids.push((-1, String::new()));
        self.chord_snapshots.push(ChordSnapshot::new_default());
    }

    pub fn new_default(num_tracks: usize, slot_descriptors: &[Vec<EffectDescriptor>]) -> Self {
        let mut snap = Self {
            track_bits: Vec::with_capacity(num_tracks),
            step_data: Vec::with_capacity(num_tracks),
            track_params: Vec::with_capacity(num_tracks),
            effect_slots: Vec::with_capacity(num_tracks),
            sample_ids: Vec::with_capacity(num_tracks),
            chord_snapshots: Vec::with_capacity(num_tracks),
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
    /// Fractional phase within current step (0.0–1.0), written by audio thread.
    /// Used by UI to round-to-nearest-step when recording keyboard input.
    pub playhead_phase: AtomicU32,
    /// Recording quantize threshold (0.0–1.0). Key presses landing past this
    /// phase within a step snap to the next step. Default 0.5 (midpoint).
    /// Adjust with [ / ] when armed to compensate for output latency.
    pub record_quantize_thresh: AtomicU32,
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
            playhead_phase: AtomicU32::new(0.0_f32.to_bits()),
            record_quantize_thresh: AtomicU32::new(0.5_f32.to_bits()),
        }
    }

    pub fn active_track_count(&self) -> usize {
        self.num_tracks.load(Ordering::Acquire) as usize
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

    /// Switch to a different pattern. Returns the sample_ids from the restored
    /// pattern so the UI can apply buffer swaps. Returns None if no switch occurred.
    pub fn switch_pattern(
        &self,
        new_idx: usize,
        num_tracks: usize,
        buffer_ids: &[i32],
        names: &[String],
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern_bank.lock().unwrap();
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        if new_idx == cur || new_idx >= bank.len() {
            return None;
        }
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names);
        bank[new_idx].restore(self);
        self.current_pattern
            .store(new_idx as u32, Ordering::Relaxed);
        Some(bank[new_idx].sample_ids.clone())
    }

    pub fn clone_pattern(&self, num_tracks: usize, buffer_ids: &[i32], names: &[String]) -> usize {
        let mut bank = self.pattern_bank.lock().unwrap();
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names);
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
    ) -> Option<Vec<(i32, String)>> {
        let mut bank = self.pattern_bank.lock().unwrap();
        if bank.len() <= 1 {
            return None;
        }
        let cur = self.current_pattern.load(Ordering::Relaxed) as usize;
        // Capture current before removing (so other patterns stay consistent)
        bank[cur] = PatternSnapshot::capture(self, num_tracks, buffer_ids, names);
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

    pub fn process_block(&mut self, nframes: usize, state: &SequencerState) -> Vec<Trigger> {
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

        // Publish fractional phase so UI can round-to-nearest-step when recording.
        let phase = (self.sample_counter / self.samples_per_step) as f32;
        state.playhead_phase.store(phase.to_bits(), Ordering::Relaxed);

        triggers
    }
}
