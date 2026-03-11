use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::audiograph::LiveGraphPtr;
use crate::effects::EffectDescriptor;
use crate::lisp_effect::{DGenManifest, LoadedDGenLib};
use crate::sequencer::{
    InstrumentType, KeyboardTrigger, SequencerState, StepParam, STEPS_PER_PAGE,
};

mod browser;
mod cirklon;
mod draw;
mod effect_params;
mod effects;
mod effects_draw;
mod graph;
mod input;
mod params;
mod projects;
mod synth;

pub use browser::BrowserNode;
pub use draw::draw;

const BAR_HEIGHT: usize = 8;
const COL_WIDTH: u16 = 3;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum EffectTab {
    Slot(usize),
    Synth,
    Mod,
    Sources,
    Reverb,
}

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

enum PendingEditor {
    Effect {
        slot_idx: usize,
        name: Option<String>,
    },
    Instrument {
        name: Option<String>,
    },
}

enum CompileTarget {
    Effect {
        name: String,
        slot_idx: usize,
        track: usize,
    },
    Instrument {
        name: String,
    },
}

struct PendingCompile {
    receiver: std::sync::mpsc::Receiver<Result<crate::lisp_effect::CompileResult, String>>,
    target: CompileTarget,
    tick: usize,
}

enum PendingProjectLoadPhase {
    ClearExisting,
    AddTrack(usize),
    AddEffect { track_idx: usize, offset: usize },
    BuildPattern(usize),
    Finalize,
}

struct PendingProjectLoad {
    name: String,
    tick: usize,
    project: crate::project::ProjectFile,
    built_patterns: Vec<crate::sequencer::PatternSnapshot>,
    fallback_samples: usize,
    phase: PendingProjectLoadPhase,
}

pub struct EngineDescriptor {
    pub name: String,
    pub source: String,
    pub manifest: DGenManifest,
    pub lib_index: usize,
}

#[derive(Default)]
pub struct EngineRegistry {
    pub engines: Vec<EngineDescriptor>,
}

impl EngineRegistry {
    pub fn find_by_name_and_source(&self, name: &str, source: &str) -> Option<usize> {
        self.engines
            .iter()
            .position(|entry| entry.name == name && entry.source == source)
    }

    pub fn get(&self, engine_id: usize) -> Option<&EngineDescriptor> {
        self.engines.get(engine_id)
    }

    pub fn upsert(&mut self, entry: EngineDescriptor) -> usize {
        if let Some(existing_idx) = self.find_by_name_and_source(&entry.name, &entry.source) {
            self.engines[existing_idx] = entry;
            existing_idx
        } else {
            self.engines.push(entry);
            self.engines.len() - 1
        }
    }
}

pub struct EngineNodeIds {
    pub synth_ids: Vec<i32>,
    pub gatepitch_ids: Vec<i32>,
    pub modulator_ids: Vec<i32>,
    pub route_gain_ids: Vec<Vec<i32>>,
}

#[derive(Clone, Copy)]
pub enum ParamMouseDragTarget {
    TrackParam { row_idx: usize },
    SynthParam { row_idx: usize },
    ModParam { row_idx: usize },
    SourceParam { row_idx: usize },
    EffectParam { slot_idx: usize, param_idx: usize },
    ReverbParam { param_idx: usize },
}

#[derive(Clone, Copy)]
pub struct ParamMouseDrag {
    pub track: usize,
    pub target: ParamMouseDragTarget,
    pub start_col: u16,
    pub start_display_value: f32,
}

pub struct EditorState {
    pending_editor: Option<PendingEditor>,
    pending_compile: Option<PendingCompile>,
    pending_project_load: Option<PendingProjectLoad>,
    lisp_libs: Vec<LoadedDGenLib>,
    pub instrument_libs: Vec<LoadedDGenLib>,
    pub picker_cursor: usize,
    pub picker_filter: String,
    pub picker_items: Vec<String>,
    pub status_message: Option<(String, Instant)>,
    pub engine_registry: EngineRegistry,
}

pub struct BrowserState {
    pub tree: Vec<BrowserNode>,
    pub cursor: usize,
    pub filter: String,
    pub scroll_offset: usize,
}

pub struct PresetBrowserState {
    pub cursor: usize,
    pub filter: String,
    pub scroll_offset: usize,
}

pub struct GraphState {
    pub lg: LiveGraphPtr,
    pub track_node_ids: Vec<TrackNodeIds>,
    pub sample_rate: u32,
    pub bus_l_id: i32,
    pub bus_r_id: i32,
    pub reverb_bus_id: i32,
    pub reverb_node_id: i32,
    pub track_buffer_ids: Vec<i32>,
    pub track_voice_lids: Vec<Vec<u64>>,
    pub track_instrument_types: Vec<InstrumentType>,
    pub track_engine_ids: Vec<Option<usize>>,
    pub track_synth_node_ids: Vec<Vec<i32>>,
    pub track_gatepitch_node_ids: Vec<Vec<i32>>,
    pub engine_node_ids: Vec<Option<EngineNodeIds>>,
    pub effect_descriptors: Vec<Vec<EffectDescriptor>>,
    pub instrument_descriptors: Vec<EffectDescriptor>,
    pub record_armed: Vec<bool>,
    pub keyboard_tx: std::sync::mpsc::Sender<KeyboardTrigger>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum InputMode {
    Normal,
    ValueEntry,
    Dropdown,
    PatternSelect,
    PresetNameEntry,
    ProjectNameEntry,
    EffectPicker,
    InstrumentPicker,
    ProjectPicker,
    StepInsert,
    StepSelect,
    StepArm,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum SidebarMode {
    InstrumentPicker,
    AddTrack,
    Audition,
    Presets,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum PresetPromptKind {
    SaveNew,
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
    pub piano_area: ratatui::prelude::Rect,
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

pub struct UiState {
    pub cursor_step: usize,
    pub cursor_track: usize,
    pub active_param: StepParam,
    pub input_mode: InputMode,
    pub value_buffer: String,
    pub selection_anchor: Option<usize>,
    pub visual_steps: HashSet<usize>,
    pub should_quit: bool,
    pub focused_region: Region,
    pub sidebar_mode: SidebarMode,
    pub params_column: usize,
    pub track_param_cursor: usize,
    pub effect_tab: EffectTab,
    pub effect_param_cursor: usize,
    pub dropdown_open: bool,
    pub dropdown_cursor: usize,
    pub track_param_dropdown: bool,
    pub layout: LayoutRects,
    pub last_step_click: Option<(usize, Instant)>,
    pub last_x_press: Option<Instant>,
    pub pattern_clone_pending: bool,
    pub pattern_page: usize,
    pub pattern_btn_layout: Vec<(u16, u16, PatternBtn)>,
    pub page_btn_layout: Vec<(u16, u16, usize)>,
    pub bpm_entry: bool,
    pub reverb_param_cursor: usize,
    pub reverb_size: f32,
    pub reverb_brightness: f32,
    pub reverb_replace: f32,
    pub recording: bool,
    pub keyboard_octave: i32,
    pub held_notes: Vec<(char, f32, usize, Instant)>,
    pub piano_notes: Vec<(i32, Instant)>,
    pub piano_last_step: usize,
    pub piano_last_track: usize,
    pub piano_lo: i32,
    pub follow_override_until: Option<Instant>,
    pub instrument_picker_cursor: usize,
    pub instrument_param_cursor: usize,
    pub synth_scroll_offset: usize,
    pub mod_param_cursor: usize,
    pub mod_scroll_offset: usize,
    pub source_param_cursor: usize,
    pub source_scroll_offset: usize,
    pub preset_prompt_kind: PresetPromptKind,
    pub param_mouse_drag: Option<ParamMouseDrag>,
}

pub struct App {
    pub state: Arc<SequencerState>,
    pub tracks: Vec<String>,
    pub sampler_paths: Vec<Option<PathBuf>>,
    pub sample_path_registry: HashMap<String, PathBuf>,
    pub current_project_name: Option<String>,
    pub ui: UiState,
    pub editor: EditorState,
    pub browser: BrowserState,
    pub preset_browser: PresetBrowserState,
    pub graph: GraphState,
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
            SidebarMode::InstrumentPicker
        };

        let browser_tree = BrowserNode::scan_root("samples");

        Self {
            state,
            tracks: Vec::new(),
            sampler_paths: Vec::new(),
            sample_path_registry: HashMap::new(),
            current_project_name: None,
            ui: UiState {
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
                effect_tab: EffectTab::Slot(0),
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
                bpm_entry: false,
                reverb_param_cursor: 0,
                reverb_size: 0.2,
                reverb_brightness: 0.8,
                reverb_replace: 0.3,
                recording: false,
                keyboard_octave: 0,
                held_notes: Vec::new(),
                piano_notes: Vec::new(),
                piano_last_step: usize::MAX,
                piano_last_track: usize::MAX,
                piano_lo: -12,
                follow_override_until: None,
                instrument_picker_cursor: 0,
                instrument_param_cursor: 0,
                synth_scroll_offset: 0,
                mod_param_cursor: 0,
                mod_scroll_offset: 0,
                source_param_cursor: 0,
                source_scroll_offset: 0,
                preset_prompt_kind: PresetPromptKind::SaveNew,
                param_mouse_drag: None,
            },
            editor: EditorState {
                pending_editor: None,
                pending_compile: None,
                pending_project_load: None,
                lisp_libs: Vec::new(),
                instrument_libs: Vec::new(),
                picker_cursor: 0,
                picker_filter: String::new(),
                picker_items: Vec::new(),
                status_message: None,
                engine_registry: EngineRegistry::default(),
            },
            browser: BrowserState {
                tree: browser_tree,
                cursor: 0,
                filter: String::new(),
                scroll_offset: 0,
            },
            preset_browser: PresetBrowserState {
                cursor: 0,
                filter: String::new(),
                scroll_offset: 0,
            },
            graph: GraphState {
                lg,
                track_node_ids: Vec::new(),
                sample_rate,
                bus_l_id: buses.bus_l_id,
                bus_r_id: buses.bus_r_id,
                reverb_bus_id: buses.reverb_bus_id,
                reverb_node_id: buses.reverb_node_id,
                track_buffer_ids: Vec::new(),
                track_voice_lids: Vec::new(),
                track_instrument_types: Vec::new(),
                track_engine_ids: Vec::new(),
                track_synth_node_ids: Vec::new(),
                track_gatepitch_node_ids: Vec::new(),
                engine_node_ids: Vec::new(),
                effect_descriptors: Vec::new(),
                instrument_descriptors: Vec::new(),
                record_armed: Vec::new(),
                keyboard_tx,
            },
        }
    }

    fn selected_range(&self) -> (usize, usize) {
        match self.ui.selection_anchor {
            Some(anchor) => {
                let lo = anchor.min(self.ui.cursor_step);
                let hi = anchor.max(self.ui.cursor_step);
                (lo, hi)
            }
            None => (self.ui.cursor_step, self.ui.cursor_step),
        }
    }

    fn has_selection(&self) -> bool {
        self.ui.selection_anchor.is_some() || !self.ui.visual_steps.is_empty()
    }

    pub(super) fn effective_sidebar_mode(&self) -> SidebarMode {
        match self.ui.sidebar_mode {
            SidebarMode::InstrumentPicker | SidebarMode::AddTrack => self.ui.sidebar_mode,
            _ => {
                if !self.tracks.is_empty() && !self.is_sampler_track(self.ui.cursor_track) {
                    SidebarMode::Presets
                } else {
                    SidebarMode::Audition
                }
            }
        }
    }

    /// Return all selected step indices (visual or contiguous range, falls back to cursor).
    fn selected_steps(&self) -> Vec<usize> {
        if !self.ui.visual_steps.is_empty() {
            let mut steps: Vec<usize> = self.ui.visual_steps.iter().copied().collect();
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
            self.state.pattern.track_params[self.ui.cursor_track].get_num_steps()
        }
    }

    fn current_page(&self) -> usize {
        self.ui.cursor_step / STEPS_PER_PAGE
    }

    /// Page to display: follows playhead when playing, unless the user recently
    /// interacted or has a selection active.
    fn display_page(&self) -> usize {
        if !self.state.is_playing() {
            return self.current_page();
        }
        // Selection active → stay on cursor page
        if self.ui.selection_anchor.is_some() {
            return self.current_page();
        }
        // User recently interacted → stay on cursor page
        if let Some(until) = self.ui.follow_override_until {
            if Instant::now() < until {
                return self.current_page();
            }
        }
        // Follow playhead
        let ns = self.num_steps();
        let ph = self.state.track_step(self.ui.cursor_track) % ns;
        ph / STEPS_PER_PAGE
    }

    fn page_range(&self) -> (usize, usize) {
        let page = self.display_page();
        let page_start = page * STEPS_PER_PAGE;
        let page_end = (page_start + STEPS_PER_PAGE).min(self.num_steps());
        (page_start, page_end)
    }

    /// Pause page-follow for 5 seconds after user interaction.
    fn touch_follow_timer(&mut self) {
        self.ui.follow_override_until = Some(Instant::now() + std::time::Duration::from_secs(5));
    }

    /// Whether the given track is a Sampler instrument.
    pub fn is_sampler_track(&self, track: usize) -> bool {
        track >= self.graph.track_instrument_types.len()
            || self.graph.track_instrument_types[track] == InstrumentType::Sampler
    }

    fn selected_effect_slot(&self) -> Option<usize> {
        match self.ui.effect_tab {
            EffectTab::Slot(idx) => Some(idx),
            EffectTab::Synth | EffectTab::Mod | EffectTab::Sources | EffectTab::Reverb => None,
        }
    }

    /// Clamp cursor_step to the current track's num_steps.
    fn clamp_cursor_to_steps(&mut self) {
        let ns = self.num_steps();
        if self.ui.cursor_step >= ns {
            self.ui.cursor_step = ns - 1;
        }
    }

    pub(super) fn register_sample_path(&mut self, sample_name: &str, path: PathBuf) {
        self.sample_path_registry
            .insert(sample_name.to_string(), path);
    }

    pub(super) fn sync_sampler_path_from_name(&mut self, track: usize, sample_name: &str) {
        if track >= self.sampler_paths.len() {
            return;
        }
        self.sampler_paths[track] = self.sample_path_registry.get(sample_name).cloned();
    }
}
