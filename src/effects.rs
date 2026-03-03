use std::sync::atomic::{AtomicU32, Ordering};

use crate::sequencer::MAX_STEPS;

// Total effect params: 4 filter + 6 delay = 10
pub const NUM_FILTER_PARAMS: usize = 4;
pub const NUM_DELAY_PARAMS: usize = 6;
pub const TOTAL_EFFECT_PARAMS: usize = NUM_FILTER_PARAMS + NUM_DELAY_PARAMS;

/// NaN sentinel stored as bits — means "no p-lock override".
const NAN_BITS: u32 = f32::NAN.to_bits();

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectType {
    Filter = 0,
    Delay = 1,
}

impl EffectType {
    pub const ALL: [EffectType; 2] = [EffectType::Filter, EffectType::Delay];

    pub fn label(self) -> &'static str {
        match self {
            EffectType::Filter => "Filter",
            EffectType::Delay => "Delay",
        }
    }

    pub fn next(self) -> EffectType {
        match self {
            EffectType::Filter => EffectType::Delay,
            EffectType::Delay => EffectType::Filter,
        }
    }

    pub fn prev(self) -> EffectType {
        self.next() // only 2 variants
    }

    /// Number of params for this effect type.
    pub fn num_params(self) -> usize {
        match self {
            EffectType::Filter => NUM_FILTER_PARAMS,
            EffectType::Delay => NUM_DELAY_PARAMS,
        }
    }

    /// Global param index offset for this effect type.
    pub fn param_offset(self) -> usize {
        match self {
            EffectType::Filter => 0,
            EffectType::Delay => NUM_FILTER_PARAMS,
        }
    }
}

// ── Filter params ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FilterParam {
    Enabled = 0,
    Mode = 1,
    Cutoff = 2,
    Resonance = 3,
}

impl FilterParam {
    pub const ALL: [FilterParam; NUM_FILTER_PARAMS] = [
        FilterParam::Enabled,
        FilterParam::Mode,
        FilterParam::Cutoff,
        FilterParam::Resonance,
    ];

    pub fn global_index(self) -> usize {
        self as usize
    }

    pub fn default_value(self) -> f32 {
        match self {
            FilterParam::Enabled => 0.0,   // off
            FilterParam::Mode => 0.0,      // LP
            FilterParam::Cutoff => 1000.0, // Hz
            FilterParam::Resonance => 1.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            FilterParam::Enabled => 0.0,
            FilterParam::Mode => 0.0,
            FilterParam::Cutoff => 20.0,
            FilterParam::Resonance => 0.5,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            FilterParam::Enabled => 1.0,
            FilterParam::Mode => 2.0,   // LP=0, HP=1, BP=2
            FilterParam::Cutoff => 20000.0,
            FilterParam::Resonance => 10.0,
        }
    }

    pub fn increment(self) -> f32 {
        match self {
            FilterParam::Enabled => 1.0,
            FilterParam::Mode => 1.0,
            FilterParam::Cutoff => 50.0,
            FilterParam::Resonance => 0.1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FilterParam::Enabled => "enabled",
            FilterParam::Mode => "mode",
            FilterParam::Cutoff => "cutoff",
            FilterParam::Resonance => "resonance",
        }
    }

    pub fn is_boolean(self) -> bool {
        matches!(self, FilterParam::Enabled)
    }

    pub fn is_enum(self) -> bool {
        matches!(self, FilterParam::Mode)
    }

    pub fn format_value(self, val: f32) -> String {
        match self {
            FilterParam::Enabled => {
                if val > 0.5 { "ON".to_string() } else { "OFF".to_string() }
            }
            FilterParam::Mode => {
                match val.round() as i32 {
                    0 => "lowpass".to_string(),
                    1 => "highpass".to_string(),
                    2 => "bandpass".to_string(),
                    _ => "lowpass".to_string(),
                }
            }
            FilterParam::Cutoff => format!("{:.0} Hz", val),
            FilterParam::Resonance => format!("{:.1}", val),
        }
    }
}

// ── Delay params ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DelayParam {
    Wet = 0,
    Synced = 1,
    DelayTime = 2,
    Feedback = 3,
    Dampening = 4,
    StereoWidth = 5,
}

impl DelayParam {
    pub const ALL: [DelayParam; NUM_DELAY_PARAMS] = [
        DelayParam::Wet,
        DelayParam::Synced,
        DelayParam::DelayTime,
        DelayParam::Feedback,
        DelayParam::Dampening,
        DelayParam::StereoWidth,
    ];

    pub fn global_index(self) -> usize {
        NUM_FILTER_PARAMS + self as usize
    }

    pub fn default_value(self) -> f32 {
        match self {
            DelayParam::Wet => 0.0,
            DelayParam::Synced => 0.0,       // off
            DelayParam::DelayTime => 250.0,   // ms
            DelayParam::Feedback => 0.3,
            DelayParam::Dampening => 0.5,
            DelayParam::StereoWidth => 1.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            DelayParam::Wet => 0.0,
            DelayParam::Synced => 0.0,
            DelayParam::DelayTime => 1.0,
            DelayParam::Feedback => 0.0,
            DelayParam::Dampening => 0.0,
            DelayParam::StereoWidth => 0.0,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            DelayParam::Wet => 1.0,
            DelayParam::Synced => 1.0,
            DelayParam::DelayTime => 2000.0,  // ms
            DelayParam::Feedback => 0.95,
            DelayParam::Dampening => 1.0,
            DelayParam::StereoWidth => 2.0,
        }
    }

    pub fn increment(self) -> f32 {
        match self {
            DelayParam::Wet => 0.05,
            DelayParam::Synced => 1.0,
            DelayParam::DelayTime => 10.0,
            DelayParam::Feedback => 0.05,
            DelayParam::Dampening => 0.05,
            DelayParam::StereoWidth => 0.1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DelayParam::Wet => "wet",
            DelayParam::Synced => "synced",
            DelayParam::DelayTime => "time",
            DelayParam::Feedback => "feedback",
            DelayParam::Dampening => "dampening",
            DelayParam::StereoWidth => "width",
        }
    }

    pub fn is_boolean(self) -> bool {
        matches!(self, DelayParam::Synced)
    }

    pub fn format_value(self, val: f32) -> String {
        match self {
            DelayParam::Wet => format!("{:.0}%", val * 100.0),
            DelayParam::Synced => {
                if val > 0.5 { "ON".to_string() } else { "OFF".to_string() }
            }
            DelayParam::DelayTime => format!("{:.0} ms", val),
            DelayParam::Feedback => format!("{:.2}", val),
            DelayParam::Dampening => format!("{:.2}", val),
            DelayParam::StereoWidth => format!("{:.1}", val),
        }
    }
}

// ── Sync divisions ──

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncDivision {
    ThirtySecond = 0,
    Sixteenth = 1,
    SixteenthTriplet = 2,
    Eighth = 3,
    EighthTriplet = 4,
    EighthDotted = 5,
    Quarter = 6,
    QuarterTriplet = 7,
    QuarterDotted = 8,
    Half = 9,
    Whole = 10,
}

impl SyncDivision {
    pub const ALL: [SyncDivision; 11] = [
        SyncDivision::ThirtySecond,
        SyncDivision::Sixteenth,
        SyncDivision::SixteenthTriplet,
        SyncDivision::Eighth,
        SyncDivision::EighthTriplet,
        SyncDivision::EighthDotted,
        SyncDivision::Quarter,
        SyncDivision::QuarterTriplet,
        SyncDivision::QuarterDotted,
        SyncDivision::Half,
        SyncDivision::Whole,
    ];

    /// Duration in beats (quarter notes).
    pub fn to_beats(self) -> f64 {
        match self {
            SyncDivision::ThirtySecond => 0.125,
            SyncDivision::Sixteenth => 0.25,
            SyncDivision::SixteenthTriplet => 1.0 / 6.0,
            SyncDivision::Eighth => 0.5,
            SyncDivision::EighthTriplet => 1.0 / 3.0,
            SyncDivision::EighthDotted => 0.75,
            SyncDivision::Quarter => 1.0,
            SyncDivision::QuarterTriplet => 2.0 / 3.0,
            SyncDivision::QuarterDotted => 1.5,
            SyncDivision::Half => 2.0,
            SyncDivision::Whole => 4.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SyncDivision::ThirtySecond => "1/32",
            SyncDivision::Sixteenth => "1/16",
            SyncDivision::SixteenthTriplet => "1/16t",
            SyncDivision::Eighth => "1/8",
            SyncDivision::EighthTriplet => "1/8t",
            SyncDivision::EighthDotted => "1/8.",
            SyncDivision::Quarter => "1/4",
            SyncDivision::QuarterTriplet => "1/4t",
            SyncDivision::QuarterDotted => "1/4.",
            SyncDivision::Half => "1/2",
            SyncDivision::Whole => "1",
        }
    }

    pub fn from_index(idx: usize) -> SyncDivision {
        SyncDivision::ALL[idx.min(SyncDivision::ALL.len() - 1)]
    }
}

// ── Generic helpers for effect param access by global index ──

/// Get default for a global effect param index (0..TOTAL_EFFECT_PARAMS).
pub fn effect_param_default(global_idx: usize) -> f32 {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].default_value()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].default_value()
    }
}

pub fn effect_param_min(global_idx: usize) -> f32 {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].min()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].min()
    }
}

pub fn effect_param_max(global_idx: usize) -> f32 {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].max()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].max()
    }
}

pub fn effect_param_increment(global_idx: usize) -> f32 {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].increment()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].increment()
    }
}

pub fn effect_param_label(global_idx: usize) -> &'static str {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].label()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].label()
    }
}

pub fn effect_param_is_boolean(global_idx: usize) -> bool {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].is_boolean()
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].is_boolean()
    }
}

/// Returns true if this param is displayed as a percentage (0-100) but stored as 0.0-1.0.
pub fn effect_param_is_percent(global_idx: usize) -> bool {
    if global_idx >= NUM_FILTER_PARAMS {
        matches!(DelayParam::ALL[global_idx - NUM_FILTER_PARAMS], DelayParam::Wet)
    } else {
        false
    }
}

pub fn effect_param_format(global_idx: usize, val: f32) -> String {
    if global_idx < NUM_FILTER_PARAMS {
        FilterParam::ALL[global_idx].format_value(val)
    } else {
        DelayParam::ALL[global_idx - NUM_FILTER_PARAMS].format_value(val)
    }
}

// ── P-lock data ──

/// Per-track per-step effect parameter overrides.
/// NaN = no override (use track default).
pub struct EffectPLockData {
    data: Vec<AtomicU32>,
}

impl EffectPLockData {
    pub fn new() -> Self {
        let size = MAX_STEPS * TOTAL_EFFECT_PARAMS;
        let data: Vec<AtomicU32> = (0..size)
            .map(|_| AtomicU32::new(NAN_BITS))
            .collect();
        Self { data }
    }

    fn index(step: usize, param_idx: usize) -> usize {
        step * TOTAL_EFFECT_PARAMS + param_idx
    }

    /// Get p-locked value, or None if NaN (no override).
    pub fn get(&self, step: usize, param_idx: usize) -> Option<f32> {
        let bits = self.data[Self::index(step, param_idx)].load(Ordering::Relaxed);
        let val = f32::from_bits(bits);
        if val.is_nan() { None } else { Some(val) }
    }

    pub fn set(&self, step: usize, param_idx: usize, val: f32) {
        let min = effect_param_min(param_idx);
        let max = effect_param_max(param_idx);
        let clamped = val.clamp(min, max);
        self.data[Self::index(step, param_idx)].store(clamped.to_bits(), Ordering::Relaxed);
    }

    /// Clear all p-locks for a step (set all to NaN).
    pub fn clear_step(&self, step: usize) {
        for p in 0..TOTAL_EFFECT_PARAMS {
            self.data[Self::index(step, p)].store(NAN_BITS, Ordering::Relaxed);
        }
    }

    /// Clear a single p-lock (set to NaN = no override).
    pub fn clear_param(&self, step: usize, param_idx: usize) {
        self.data[Self::index(step, param_idx)].store(NAN_BITS, Ordering::Relaxed);
    }

    /// Check if a step has any p-locked value.
    pub fn step_has_any_plock(&self, step: usize) -> bool {
        for p in 0..TOTAL_EFFECT_PARAMS {
            let bits = self.data[Self::index(step, p)].load(Ordering::Relaxed);
            if !f32::from_bits(bits).is_nan() {
                return true;
            }
        }
        false
    }
}

// ── Lisp effect p-lock types ──

pub const MAX_LISP_PARAMS: usize = 16;

/// Per-track per-step lisp effect parameter overrides.
/// NaN = no override (use track default).
/// Unlike EffectPLockData, does NOT clamp internally (min/max are dynamic per-effect; UI clamps).
pub struct LispPLockData {
    data: Vec<AtomicU32>,
}

impl LispPLockData {
    pub fn new() -> Self {
        let size = MAX_STEPS * MAX_LISP_PARAMS;
        let data: Vec<AtomicU32> = (0..size)
            .map(|_| AtomicU32::new(NAN_BITS))
            .collect();
        Self { data }
    }

    fn index(step: usize, param_idx: usize) -> usize {
        step * MAX_LISP_PARAMS + param_idx
    }

    pub fn get(&self, step: usize, param_idx: usize) -> Option<f32> {
        let bits = self.data[Self::index(step, param_idx)].load(Ordering::Relaxed);
        let val = f32::from_bits(bits);
        if val.is_nan() { None } else { Some(val) }
    }

    pub fn set(&self, step: usize, param_idx: usize, val: f32) {
        self.data[Self::index(step, param_idx)].store(val.to_bits(), Ordering::Relaxed);
    }

    pub fn clear_step(&self, step: usize) {
        for p in 0..MAX_LISP_PARAMS {
            self.data[Self::index(step, p)].store(NAN_BITS, Ordering::Relaxed);
        }
    }

    pub fn clear_param(&self, step: usize, param_idx: usize) {
        self.data[Self::index(step, param_idx)].store(NAN_BITS, Ordering::Relaxed);
    }

    pub fn step_has_any_plock(&self, step: usize, num_params: usize) -> bool {
        for p in 0..num_params.min(MAX_LISP_PARAMS) {
            let bits = self.data[Self::index(step, p)].load(Ordering::Relaxed);
            if !f32::from_bits(bits).is_nan() {
                return true;
            }
        }
        false
    }
}

/// Per-track lisp effect default parameter values.
pub struct LispParamDefaults {
    data: [AtomicU32; MAX_LISP_PARAMS],
}

impl LispParamDefaults {
    pub fn new() -> Self {
        let data: [AtomicU32; MAX_LISP_PARAMS] = std::array::from_fn(|_| {
            AtomicU32::new(0.0_f32.to_bits())
        });
        Self { data }
    }

    pub fn get(&self, idx: usize) -> f32 {
        f32::from_bits(self.data[idx].load(Ordering::Relaxed))
    }

    pub fn set(&self, idx: usize, val: f32) {
        self.data[idx].store(val.to_bits(), Ordering::Relaxed);
    }
}

// ── Track-level effect defaults ──

pub struct TrackEffectDefaults {
    data: [AtomicU32; TOTAL_EFFECT_PARAMS],
}

impl TrackEffectDefaults {
    pub fn new() -> Self {
        let data: [AtomicU32; TOTAL_EFFECT_PARAMS] = std::array::from_fn(|i| {
            AtomicU32::new(effect_param_default(i).to_bits())
        });
        Self { data }
    }

    pub fn get(&self, param_idx: usize) -> f32 {
        f32::from_bits(self.data[param_idx].load(Ordering::Relaxed))
    }

    pub fn set(&self, param_idx: usize, val: f32) {
        let min = effect_param_min(param_idx);
        let max = effect_param_max(param_idx);
        let clamped = val.clamp(min, max);
        self.data[param_idx].store(clamped.to_bits(), Ordering::Relaxed);
    }
}
