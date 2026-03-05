use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use crate::audiograph::LiveGraphPtr;
use crate::effects::{EffectDescriptor, BUILTIN_SLOT_COUNT};
use crate::lisp_effect::LoadedDGenLib;
use crate::sequencer::{KeyboardTrigger, SequencerState, StepParam, STEPS_PER_PAGE};

mod browser;
mod cirklon;
mod draw;
mod effects;
mod input;
mod params;
mod tracks;

pub use browser::BrowserNode;
pub use draw::draw;

const BAR_HEIGHT: usize = 8;
const COL_WIDTH: u16 = 3;

/// Sentinel value for `effect_slot_cursor` indicating the Reverb tab is selected.
const REVERB_TAB: usize = usize::MAX;

#[derive(Clone, Debug)]
pub enum PatternBtn {
    PrevPage,
    Pattern(usize),
    NextPage,
    Clone,
    Delete,
}

// Track param cursor indices
const TP_GATE: usize = 0;
const TP_ATTACK: usize = 1;
const TP_RELEASE: usize = 2;
const TP_SWING: usize = 3;
const TP_STEPS: usize = 4;
const TP_TIMEBASE: usize = 5;
const TP_SEND: usize = 6;
const TP_POLY: usize = 7;
const TP_LAST: usize = TP_POLY;

struct PendingCompile {
    receiver: std::sync::mpsc::Receiver<Result<crate::lisp_effect::CompileResult, String>>,
    name: String,
    slot_idx: usize,
    cursor_track: usize,
    tick: usize,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum InputMode {
    Normal,
    ValueEntry,
    Dropdown,
    PatternSelect,
    EffectPicker,
    StepInsert,
    StepSelect,
    StepArm,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SidebarMode {
    AddTrack,
    Audition,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Region {
    Cirklon,
    Sidebar,
    Params,
}

impl Region {
    fn next(self) -> Region {
        match self {
            Region::Cirklon => Region::Params,
            Region::Sidebar => Region::Cirklon, // Tab exits sidebar to Cirklon
            Region::Params => Region::Cirklon,
        }
    }

    fn prev(self) -> Region {
        match self {
            Region::Cirklon => Region::Params,
            Region::Sidebar => Region::Cirklon, // BackTab exits sidebar to Cirklon
            Region::Params => Region::Cirklon,
        }
    }
}

#[derive(Default, Clone)]
pub struct LayoutRects {
    pub cirklon_area: ratatui::prelude::Rect,
    pub track_list: ratatui::prelude::Rect,
    pub param_tabs: ratatui::prelude::Rect,
    pub bars: ratatui::prelude::Rect,
    pub trigger_row: ratatui::prelude::Rect,
    pub track_params_inner: ratatui::prelude::Rect,
    pub effects_inner: ratatui::prelude::Rect,
    pub effects_block: ratatui::prelude::Rect,
    pub info_bar: ratatui::prelude::Rect,
    pub rec_button: ratatui::prelude::Rect,
    pub pattern_buttons_area: ratatui::prelude::Rect,
    pub page_blocks_area: ratatui::prelude::Rect,
    pub sidebar_inner: ratatui::prelude::Rect,
}

/// Per-track node IDs needed for graph rewiring.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TrackNodeIds {
    pub sampler_ids: Vec<i32>, // up to MAX_VOICES
    pub voice_sum_id: i32,     // dedicated sum node
    pub filter_id: i32,
    pub delay_id: i32,
    pub send_id: i32,
}

/// Audio bus node IDs, passed to App::new to reduce parameter count.
pub struct AudioBuses {
    pub bus_l_id: i32,
    pub bus_r_id: i32,
    pub reverb_bus_id: i32,
    pub reverb_node_id: i32,
}

pub struct App {
    pub state: Arc<SequencerState>,
    pub tracks: Vec<String>,
    pub cursor_step: usize,
    pub cursor_track: usize,
    pub active_param: StepParam,
    pub input_mode: InputMode,
    pub value_buffer: String,
    pub selection_anchor: Option<usize>,
    pub visual_steps: HashSet<usize>,
    pub should_quit: bool,

    // Region/focus system
    pub focused_region: Region,
    pub sidebar_mode: SidebarMode,
    pub params_column: usize, // 0 = track params (left), 1 = effects (right)
    pub track_param_cursor: usize, // 0=gate, 1=attack, 2=release, 3=swing, 4=steps
    pub effect_slot_cursor: usize, // index into effect_descriptors[track]
    pub effect_param_cursor: usize, // param index within focused slot
    pub dropdown_open: bool,
    pub dropdown_cursor: usize,
    /// True when the dropdown is for a track param (e.g. timebase), false for effect params.
    pub track_param_dropdown: bool,
    pub layout: LayoutRects,
    last_step_click: Option<(usize, Instant)>, // (step, time) for double-click detection
    last_x_press: Option<Instant>,              // for xx (clear pattern) detection
    pub pattern_clone_pending: bool,
    pub pattern_page: usize,
    pub pattern_btn_layout: Vec<(u16, u16, PatternBtn)>,
    pub page_btn_layout: Vec<(u16, u16, usize)>,

    // Per-track effect descriptors (metadata for UI rendering)
    pub effect_descriptors: Vec<Vec<EffectDescriptor>>,

    // DGenLisp integration
    pub lg: LiveGraphPtr,
    pub track_node_ids: Vec<TrackNodeIds>,
    pub sample_rate: u32,
    pub pending_lisp_edit: bool,
    pub pending_lisp_slot: usize, // chain index being edited/added
    pub pending_lisp_name: Option<String>, // effect name if editing existing
    lisp_libs: Vec<LoadedDGenLib>, // keep loaded dylibs alive

    // Effect picker
    pub picker_cursor: usize,
    pub picker_filter: String,
    pub picker_items: Vec<String>, // cached list from effects/ folder

    // Status message (shown briefly in help bar)
    pub status_message: Option<(String, Instant)>,

    // BPM entry mode
    pub bpm_entry: bool,

    // Bus node IDs for wiring new tracks
    pub bus_l_id: i32,
    pub bus_r_id: i32,
    pub reverb_bus_id: i32,
    pub reverb_node_id: i32,

    // Reverb tab state
    pub reverb_param_cursor: usize,
    pub reverb_size: f32,
    pub reverb_brightness: f32,
    pub reverb_replace: f32,

    // Async effect compilation
    pending_compile: Option<PendingCompile>,

    // Sample browser
    pub browser_tree: Vec<BrowserNode>,
    pub browser_cursor: usize,
    pub browser_filter: String,
    pub browser_scroll_offset: usize,

    // Per-track buffer IDs
    pub track_buffer_ids: Vec<i32>,

    // Keyboard playing & recording
    pub record_armed: Vec<bool>,
    pub recording: bool, // true = record into pattern on key-up
    pub keyboard_octave: i32,
    pub keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,

    // Held notes for key-up duration tracking: (key_char, transpose, step_at_press, press_instant)
    pub held_notes: Vec<(char, f32, usize, Instant)>,

    // Per-track voice logical IDs (UI-side tracking)
    pub track_voice_lids: Vec<Vec<u64>>,

    // Piano keyboard visualization: notes ringing with expiry times
    pub piano_notes: Vec<(i32, Instant)>, // (semitone, expires_at)
    pub piano_last_step: usize,
    pub piano_last_track: usize,
}

impl App {
    pub fn new(
        state: Arc<SequencerState>,
        lg: LiveGraphPtr,
        sample_rate: u32,
        buses: AudioBuses,
        keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
    ) -> Self {
        let has_tracks = state.active_track_count() > 0;
        let focused_region = if has_tracks {
            Region::Cirklon
        } else {
            Region::Sidebar
        };
        let sidebar_mode = if has_tracks {
            SidebarMode::Audition
        } else {
            SidebarMode::AddTrack
        };

        let browser_tree = BrowserNode::scan_root("samples");

        Self {
            state,
            tracks: Vec::new(),
            cursor_step: 0,
            cursor_track: 0,
            active_param: StepParam::Velocity,
            input_mode: InputMode::Normal,
            value_buffer: String::new(),
            selection_anchor: None,
            visual_steps: HashSet::new(),
            should_quit: false,
            focused_region,
            sidebar_mode,
            params_column: 0,
            track_param_cursor: 0,
            effect_slot_cursor: 0,
            effect_param_cursor: 0,
            dropdown_open: false,
            dropdown_cursor: 0,
            track_param_dropdown: false,
            layout: LayoutRects::default(),
            last_step_click: None,
            last_x_press: None,
            pattern_clone_pending: false,
            pattern_page: 0,
            pattern_btn_layout: Vec::new(),
            page_btn_layout: Vec::new(),
            effect_descriptors: Vec::new(),
            lg,
            track_node_ids: Vec::new(),
            sample_rate,
            pending_lisp_edit: false,
            pending_lisp_slot: BUILTIN_SLOT_COUNT,
            pending_lisp_name: None,
            lisp_libs: Vec::new(),
            picker_cursor: 0,
            picker_filter: String::new(),
            picker_items: Vec::new(),
            status_message: None,
            bpm_entry: false,
            bus_l_id: buses.bus_l_id,
            bus_r_id: buses.bus_r_id,
            reverb_bus_id: buses.reverb_bus_id,
            reverb_node_id: buses.reverb_node_id,
            reverb_param_cursor: 0,
            reverb_size: 0.2,
            reverb_brightness: 0.8,
            reverb_replace: 0.3,
            browser_tree,
            browser_cursor: 0,
            browser_filter: String::new(),
            browser_scroll_offset: 0,
            pending_compile: None,
            track_buffer_ids: Vec::new(),
            record_armed: Vec::new(),
            recording: false,
            keyboard_octave: 0,
            keyboard_tx,
            held_notes: Vec::new(),
            track_voice_lids: Vec::new(),
            piano_notes: Vec::new(),
            piano_last_step: usize::MAX,
            piano_last_track: usize::MAX,
        }
    }

    fn selected_range(&self) -> (usize, usize) {
        match self.selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(self.cursor_step);
                let hi = anchor.max(self.cursor_step);
                (lo, hi)
            }
            None => (self.cursor_step, self.cursor_step),
        }
    }

    fn has_selection(&self) -> bool {
        self.selection_anchor.is_some() || !self.visual_steps.is_empty()
    }

    /// Return all selected step indices (visual or contiguous range, falls back to cursor).
    fn selected_steps(&self) -> Vec<usize> {
        if !self.visual_steps.is_empty() {
            let mut steps: Vec<usize> = self.visual_steps.iter().copied().collect();
            steps.sort();
            steps
        } else {
            let (lo, hi) = self.selected_range();
            (lo..=hi).collect()
        }
    }

    fn num_steps(&self) -> usize {
        if self.tracks.is_empty() {
            STEPS_PER_PAGE
        } else {
            self.state.track_params[self.cursor_track].get_num_steps()
        }
    }

    fn current_page(&self) -> usize {
        self.cursor_step / STEPS_PER_PAGE
    }

    fn page_range(&self) -> (usize, usize) {
        let page_start = self.current_page() * STEPS_PER_PAGE;
        let page_end = (page_start + STEPS_PER_PAGE).min(self.num_steps());
        (page_start, page_end)
    }

    /// Clamp cursor_step to the current track's num_steps.
    fn clamp_cursor_to_steps(&mut self) {
        let ns = self.num_steps();
        if self.cursor_step >= ns {
            self.cursor_step = ns - 1;
        }
    }
}
