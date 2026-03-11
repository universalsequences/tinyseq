use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::effects::EffectSlotSnapshot;
use crate::sequencer::{
    ChordSnapshot, InstrumentType, PatternSnapshot, Timebase, TrackParamsSnapshot, TrackSoundState,
    NUM_PARAMS, TRACK_PATTERN_WORDS,
};

const PROJECTS_DIR: &str = "projects";
const PROJECT_FILE_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub bpm: u32,
    pub current_pattern: usize,
    pub reverb: ProjectReverbState,
    pub tracks: Vec<ProjectTrack>,
    pub custom_effects: Vec<Vec<Option<String>>>,
    pub patterns: Vec<ProjectPattern>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectReverbState {
    pub size: f32,
    pub brightness: f32,
    pub replace: f32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectTrack {
    Sampler { sample_path: String },
    Custom { instrument_name: String },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectPattern {
    #[serde(deserialize_with = "deserialize_track_bits")]
    pub track_bits: Vec<[u64; TRACK_PATTERN_WORDS]>,
    pub step_data: Vec<Vec<[f32; NUM_PARAMS]>>,
    pub track_params: Vec<ProjectTrackParams>,
    pub effect_slots: Vec<Vec<ProjectEffectSlot>>,
    pub instrument_slots: Vec<ProjectEffectSlot>,
    pub instrument_base_note_offsets: Vec<f32>,
    pub track_sound_states: Vec<ProjectTrackSoundState>,
    pub chord_snapshots: Vec<Vec<Vec<f32>>>,
    pub timebase_plock_snapshots: Vec<Vec<Option<u32>>>,
    pub instrument_types: Vec<ProjectInstrumentType>,
    pub sample_paths: Vec<Option<String>>,
    pub sample_names: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectTrackParams {
    pub gate: bool,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub swing: f32,
    pub num_steps: usize,
    pub send: f32,
    pub polyphonic: bool,
    pub timebase: u8,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectEffectSlot {
    pub num_params: u32,
    pub defaults: Vec<f32>,
    pub plocks: Vec<Vec<Option<f32>>>,
    pub param_node_indices: Vec<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectTrackSoundState {
    pub loaded_preset: Option<String>,
    pub dirty: bool,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectInstrumentType {
    Sampler,
    Custom,
}

impl ProjectPattern {
    pub fn from_snapshot(
        snapshot: &PatternSnapshot,
        sample_paths: Vec<Option<String>>,
        sample_names: Vec<String>,
    ) -> Self {
        Self {
            track_bits: snapshot.track_bits.clone(),
            step_data: snapshot.step_data.clone(),
            track_params: snapshot
                .track_params
                .iter()
                .cloned()
                .map(ProjectTrackParams::from)
                .collect(),
            effect_slots: snapshot
                .effect_slots
                .iter()
                .map(|slots| slots.iter().map(ProjectEffectSlot::from).collect())
                .collect(),
            instrument_slots: snapshot
                .instrument_slots
                .iter()
                .map(ProjectEffectSlot::from)
                .collect(),
            instrument_base_note_offsets: snapshot.instrument_base_note_offsets.clone(),
            track_sound_states: snapshot
                .track_sound_states
                .iter()
                .cloned()
                .map(ProjectTrackSoundState::from)
                .collect(),
            chord_snapshots: snapshot
                .chord_snapshots
                .iter()
                .map(|snap| snap.steps.clone())
                .collect(),
            timebase_plock_snapshots: snapshot
                .timebase_plock_snapshots
                .iter()
                .map(|steps| steps.to_vec())
                .collect(),
            instrument_types: snapshot
                .instrument_types
                .iter()
                .copied()
                .map(ProjectInstrumentType::from)
                .collect(),
            sample_paths,
            sample_names,
        }
    }
}

impl From<TrackParamsSnapshot> for ProjectTrackParams {
    fn from(value: TrackParamsSnapshot) -> Self {
        Self {
            gate: value.gate,
            attack_ms: value.attack_ms,
            release_ms: value.release_ms,
            swing: value.swing,
            num_steps: value.num_steps,
            send: value.send,
            polyphonic: value.polyphonic,
            timebase: value.timebase as u8,
        }
    }
}

impl From<ProjectTrackParams> for TrackParamsSnapshot {
    fn from(value: ProjectTrackParams) -> Self {
        Self {
            gate: value.gate,
            attack_ms: value.attack_ms,
            release_ms: value.release_ms,
            swing: value.swing,
            num_steps: value.num_steps,
            send: value.send,
            polyphonic: value.polyphonic,
            timebase: Timebase::from_index(value.timebase as u32),
        }
    }
}

impl From<&EffectSlotSnapshot> for ProjectEffectSlot {
    fn from(value: &EffectSlotSnapshot) -> Self {
        Self {
            num_params: value.num_params,
            defaults: value.defaults.clone(),
            plocks: value.plocks.clone(),
            param_node_indices: value.param_node_indices.clone(),
        }
    }
}

impl ProjectEffectSlot {
    pub fn into_snapshot_with_node_id(self, node_id: u32) -> EffectSlotSnapshot {
        EffectSlotSnapshot {
            node_id,
            num_params: self.num_params,
            defaults: self.defaults,
            plocks: self.plocks,
            param_node_indices: self.param_node_indices,
        }
    }
}

impl From<TrackSoundState> for ProjectTrackSoundState {
    fn from(value: TrackSoundState) -> Self {
        Self {
            loaded_preset: value.loaded_preset,
            dirty: value.dirty,
        }
    }
}

impl ProjectTrackSoundState {
    pub fn into_track_sound_state(self, engine_id: Option<usize>) -> TrackSoundState {
        TrackSoundState {
            engine_id,
            loaded_preset: self.loaded_preset,
            dirty: self.dirty,
        }
    }
}

impl From<InstrumentType> for ProjectInstrumentType {
    fn from(value: InstrumentType) -> Self {
        match value {
            InstrumentType::Sampler => Self::Sampler,
            InstrumentType::Custom => Self::Custom,
        }
    }
}

impl From<ProjectInstrumentType> for InstrumentType {
    fn from(value: ProjectInstrumentType) -> Self {
        match value {
            ProjectInstrumentType::Sampler => InstrumentType::Sampler,
            ProjectInstrumentType::Custom => InstrumentType::Custom,
        }
    }
}

pub fn ensure_projects_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(projects_dir())
}

pub fn list_project_names() -> std::io::Result<Vec<String>> {
    let dir = projects_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            items.push(stem.to_string());
        }
    }
    items.sort();
    Ok(items)
}

pub fn save_project(name: &str, project: &ProjectFile) -> std::io::Result<PathBuf> {
    ensure_projects_dir()?;
    let file_name = sanitize_project_name(name);
    let path = projects_dir().join(format!("{file_name}.json"));
    let json = serde_json::to_string_pretty(project).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to serialize project '{}': {error}", path.display()),
        )
    })?;
    std::fs::write(&path, json)?;
    Ok(path)
}

pub fn load_project(name: &str) -> std::io::Result<ProjectFile> {
    let path = projects_dir().join(format!("{}.json", sanitize_project_name(name)));
    let src = std::fs::read_to_string(&path)?;
    serde_json::from_str(&src).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse project '{}': {error}", path.display()),
        )
    })
}

pub fn sanitize_project_name(name: &str) -> String {
    let sanitized: String = name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_string()
}

fn projects_dir() -> &'static Path {
    Path::new(PROJECTS_DIR)
}

pub fn project_file_version() -> u32 {
    PROJECT_FILE_VERSION
}

pub fn chord_snapshot_from_steps(steps: Vec<Vec<f32>>) -> ChordSnapshot {
    ChordSnapshot { steps }
}

fn deserialize_track_bits<'de, D>(
    deserializer: D,
) -> Result<Vec<[u64; TRACK_PATTERN_WORDS]>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TrackBitsRepr {
        Legacy(Vec<u64>),
        Current(Vec<[u64; TRACK_PATTERN_WORDS]>),
    }

    match TrackBitsRepr::deserialize(deserializer)? {
        TrackBitsRepr::Current(bits) => Ok(bits),
        TrackBitsRepr::Legacy(bits) => Ok(bits
            .into_iter()
            .map(|word| {
                let mut words = [0u64; TRACK_PATTERN_WORDS];
                words[0] = word;
                words
            })
            .collect()),
    }
}
