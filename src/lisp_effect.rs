use std::ffi::{CStr, CString};
use std::io::{self, Write};
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::audiograph::{self, LiveGraph, NodeVTable};

/// Monotonic counter so each compile produces a unique dylib filename,
/// preventing dlopen from returning a stale cached handle.
static COMPILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

// ── dlopen FFI (macOS) ──

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

type DGenProcessFn = unsafe extern "C" fn(
    inputs: *const *mut f32,
    outputs: *const *mut f32,
    frame_count: c_int,
    memory_read: *mut c_void,
    memory_write: *mut c_void,
);

// ── Global process function registry ──
// Each track can have up to MAX_CUSTOM_FX custom effects.
// The process fn pointer is stored here, indexed by slot_id = track * MAX_CUSTOM_FX + offset.

use crate::sequencer::MAX_TRACKS;
pub const MAX_CUSTOM_FX: usize = 4;
const REGISTRY_SIZE: usize = MAX_TRACKS * MAX_CUSTOM_FX;
static DGEN_PROCESS_FNS: [AtomicUsize; REGISTRY_SIZE] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; REGISTRY_SIZE]
};

fn set_dgen_process_fn(slot_id: usize, f: DGenProcessFn) {
    DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].store(f as usize, Ordering::Release);
}

// ── Node state layout ──
// state[0] = slot_id (f32), where slot_id = track_idx * MAX_CUSTOM_FX + offset
// state[1] = total_memory_slots (f32)
// state[2..2+N] = DGenLisp memory buffer

pub const HEADER_SLOTS: usize = 2;

unsafe extern "C" fn dgenlisp_wrapper_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let slot_id = (*s) as usize;
    let fn_ptr = DGEN_PROCESS_FNS[slot_id % REGISTRY_SIZE].load(Ordering::Acquire);
    if fn_ptr != 0 {
        let process_fn: DGenProcessFn = std::mem::transmute(fn_ptr);
        let memory = s.add(HEADER_SLOTS) as *mut c_void;
        process_fn(inp, out, nframes, memory, memory);
    } else {
        // Passthrough: copy input to output
        let nf = nframes as usize;
        let in0 = *inp.add(0);
        let out0 = *out.add(0);
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
    }
}

/// Initial state message format (compact, not full-size):
///   [0] = slot_id
///   [1] = total_memory_slots
///   [2] = num_entries (N)
///   [3..3+2N] = pairs of (index, value)
unsafe extern "C" fn dgenlisp_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    initial_state: *const c_void,
) {
    if initial_state.is_null() {
        return;
    }
    let src = initial_state as *const f32;
    let dst = state as *mut f32;

    // Copy header
    *dst = *src;                // slot_id
    *dst.add(1) = *src.add(1); // total_memory_slots

    // Apply sparse index/value pairs into the memory region
    let num_entries = (*src.add(2)) as usize;
    let mem = dst.add(HEADER_SLOTS);
    for i in 0..num_entries {
        let idx = (*src.add(3 + i * 2)) as usize;
        let val = *src.add(3 + i * 2 + 1);
        *mem.add(idx) = val;
    }
}

fn dgenlisp_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dgenlisp_wrapper_process),
        init: Some(dgenlisp_init),
        reset: None,
        migrate: None,
    }
}

// ── Manifest types ──

pub struct DGenManifest {
    pub dylib_path: PathBuf,
    pub total_memory_slots: usize,
    pub params: Vec<DGenParam>,
    pub n_inputs: usize,
    pub n_outputs: usize,
    pub tensor_init_data: Vec<TensorInit>,
}

#[derive(Clone)]
pub struct DGenParam {
    pub name: String,
    pub cell_id: usize,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub unit: Option<String>,
}

pub struct TensorInit {
    pub offset: usize,
    pub data: Vec<f32>,
}

// ── Loaded dylib handle ──

pub struct LoadedDGenLib {
    pub process_fn: DGenProcessFn,
    _handle: *mut c_void,
}

unsafe impl Send for LoadedDGenLib {}
unsafe impl Sync for LoadedDGenLib {}

// ── Compile result (for async compilation) ──

pub struct CompileResult {
    pub manifest: DGenManifest,
    pub lib: LoadedDGenLib,
}

pub fn compile_and_load(source: &str, sample_rate: u32) -> Result<CompileResult, String> {
    let json = compile_lisp(source, sample_rate)?;
    let manifest = parse_manifest(&json)?;
    let lib = load_dylib(&manifest.dylib_path)?;
    Ok(CompileResult { manifest, lib })
}

// ── Effect library storage ──

const EFFECTS_DIR: &str = "effects";

pub fn save_effect(name: &str, source: &str) -> io::Result<()> {
    let dir = Path::new(EFFECTS_DIR);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.lisp", name));
    std::fs::write(&path, source)
}

pub fn list_saved_effects() -> Vec<String> {
    let dir = Path::new(EFFECTS_DIR);
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let path = e.path();
                    if path.extension().map(|ext| ext == "lisp").unwrap_or(false) {
                        path.file_stem().map(|s| s.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

pub fn load_effect_source(name: &str) -> io::Result<String> {
    let path = Path::new(EFFECTS_DIR).join(format!("{}.lisp", name));
    std::fs::read_to_string(&path)
}

// ── Editor flow ──

pub fn edit_text(initial: &str) -> io::Result<String> {
    let dir = std::env::temp_dir();
    let path = dir.join("sequencer_lisp_edit.lisp");
    std::fs::write(&path, initial)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vim".to_string());

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("editor exited with status: {status}"),
        ));
    }

    std::fs::read_to_string(&path)
}

// ── Compile ──

fn output_dir() -> PathBuf {
    std::env::temp_dir().join("sequencer_dgenlisp")
}

pub fn compile_lisp(source: &str, sample_rate: u32) -> Result<String, String> {
    let dir = output_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    // Unique name per compile so dlopen doesn't return a stale cached handle
    let seq = COMPILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dylib_name = format!("effect_{}", seq);

    let src_path = dir.join("effect.lisp");
    std::fs::write(&src_path, source).map_err(|e| format!("Failed to write source: {e}"))?;

    let tool_path = std::env::current_dir()
        .unwrap_or_default()
        .join("tools/DGenLisp");
    let output = std::process::Command::new(&tool_path)
        .args(["compile", src_path.to_str().unwrap()])
        .args(["-o", dir.to_str().unwrap()])
        .args(["--name", &dylib_name])
        .args(["--sample-rate", &sample_rate.to_string()])
        .output()
        .map_err(|e| format!("Failed to run DGenLisp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!("{}{}", stderr, stdout));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout)
}

// ── Parse manifest ──

pub fn parse_manifest(json: &str) -> Result<DGenManifest, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse manifest: {e}"))?;

    let dir = output_dir();
    let dylib_name = v["dylib"].as_str().unwrap_or("effect.dylib");
    let dylib_path = dir.join(dylib_name);

    let params = v["params"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|p| DGenParam {
                    name: p["name"].as_str().unwrap_or("").to_string(),
                    cell_id: p["cellId"].as_u64().unwrap_or(0) as usize,
                    default: p["default"].as_f64().unwrap_or(0.0) as f32,
                    min: p["min"].as_f64().unwrap_or(0.0) as f32,
                    max: p["max"].as_f64().unwrap_or(1.0) as f32,
                    unit: p["unit"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();

    let n_inputs = v["inputs"].as_array().map(|a| a.len()).unwrap_or(0).max(1);
    let n_outputs = v["outputs"].as_array().map(|a| a.len()).unwrap_or(0).max(1);

    let tensor_init_data = v["tensorInitData"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|t| TensorInit {
                    offset: t["offset"].as_u64().unwrap_or(0) as usize,
                    data: t["data"]
                        .as_array()
                        .map(|d| d.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(DGenManifest {
        dylib_path,
        total_memory_slots: v["totalMemorySlots"].as_u64().unwrap_or(256) as usize,
        params,
        n_inputs,
        n_outputs,
        tensor_init_data,
    })
}

// ── Load dylib ──

pub fn load_dylib(path: &Path) -> Result<LoadedDGenLib, String> {
    let c_path =
        CString::new(path.to_str().ok_or("Invalid dylib path")?).map_err(|e| e.to_string())?;

    unsafe {
        let handle = dlopen(c_path.as_ptr(), RTLD_NOW);
        if handle.is_null() {
            let err = CStr::from_ptr(dlerror()).to_string_lossy().to_string();
            return Err(format!("dlopen failed: {err}"));
        }

        let process_sym = CString::new("process").unwrap();
        let process_ptr = dlsym(handle, process_sym.as_ptr());
        if process_ptr.is_null() {
            let err = CStr::from_ptr(dlerror()).to_string_lossy().to_string();
            return Err(format!("dlsym 'process' failed: {err}"));
        }

        Ok(LoadedDGenLib {
            process_fn: std::mem::transmute(process_ptr),
            _handle: handle,
        })
    }
}

// ── Build initial state message (compact) ──

/// Build a compact init message: [slot_id, total_memory_slots, num_entries, idx0, val0, idx1, val1, ...]
/// The engine zeroes state; init only needs to set non-zero values.
fn build_init_message(slot_id: usize, manifest: &DGenManifest) -> Vec<f32> {
    // Collect all non-zero index/value pairs
    let mut entries: Vec<(usize, f32)> = Vec::new();

    for param in &manifest.params {
        if param.cell_id < manifest.total_memory_slots && param.default != 0.0 {
            entries.push((param.cell_id, param.default));
        }
    }

    for tensor in &manifest.tensor_init_data {
        for (i, &val) in tensor.data.iter().enumerate() {
            let idx = tensor.offset + i;
            if idx < manifest.total_memory_slots && val != 0.0 {
                entries.push((idx, val));
            }
        }
    }

    // Header (3) + pairs (2 * N)
    let mut msg = Vec::with_capacity(3 + entries.len() * 2);
    msg.push(slot_id as f32);
    msg.push(manifest.total_memory_slots as f32);
    msg.push(entries.len() as f32);
    for (idx, val) in &entries {
        msg.push(*idx as f32);
        msg.push(*val);
    }
    msg
}

// ── Add effect to track's audio chain ──

/// Remove an effect from the chain and reconnect predecessor → successor.
pub unsafe fn remove_effect_from_chain(
    lg: *mut LiveGraph,
    effect_node_id: i32,
    predecessor_id: i32,
    successor_id: i32,
) {
    audiograph::graph_disconnect(lg, predecessor_id, 0, effect_node_id, 0);
    audiograph::graph_disconnect(lg, effect_node_id, 0, successor_id, 0);
    audiograph::delete_node(lg, effect_node_id);
    audiograph::graph_connect(lg, predecessor_id, 0, successor_id, 0);
}

/// Add a DGenLisp effect between predecessor and successor nodes.
/// slot_id = track_idx * MAX_CUSTOM_FX + offset.
pub unsafe fn add_effect_to_chain_at(
    lg: *mut LiveGraph,
    slot_id: usize,
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    predecessor_id: i32,
    successor_id: i32,
    existing_effect: Option<i32>,
) -> Result<i32, String> {
    // Remove old effect if present
    if let Some(old_id) = existing_effect {
        remove_effect_from_chain(lg, old_id, predecessor_id, successor_id);
    }

    // Register process function
    set_dgen_process_fn(slot_id, lib.process_fn);

    // Full state allocation (header + memory buffer), zeroed by the engine
    let state_size = (HEADER_SLOTS + manifest.total_memory_slots) * std::mem::size_of::<f32>();

    // Compact init message: only header + non-zero index/value pairs
    let init_msg = build_init_message(slot_id, manifest);
    let init_msg_size = init_msg.len() * std::mem::size_of::<f32>();

    let name = CString::new(format!("dgenlisp_fx_{}", slot_id)).unwrap();

    let node_id = audiograph::add_node(
        lg,
        dgenlisp_vtable(),
        state_size,
        name.as_ptr(),
        manifest.n_inputs as c_int,
        1, // mono output for insert chain
        init_msg.as_ptr() as *const c_void,
        init_msg_size,
    );

    if node_id < 0 {
        return Err("Failed to add DGenLisp node to graph".to_string());
    }

    // Rewire: predecessor → effect → successor
    audiograph::graph_disconnect(lg, predecessor_id, 0, successor_id, 0);
    audiograph::graph_connect(lg, predecessor_id, 0, node_id, 0);
    audiograph::graph_connect(lg, node_id, 0, successor_id, 0);

    Ok(node_id)
}

// ── Full interactive editor-compile-load flow ──

const TEMPLATE: &str = r#"; DGenLisp effect — processes audio from the track's sampler
; Input on channel 1, output on channel 1

(def input (in 1 @name signal))
(out input 1 @name audio)
"#;

pub struct LispEditResult {
    pub node_id: i32,
    pub lib: LoadedDGenLib,
    pub source: String,
    pub params: Vec<DGenParam>,
    pub name: String,
}

/// Run the full edit → compile → load → wire → name → save flow.
/// Called while terminal is in normal (non-raw) mode.
pub fn run_editor_flow(
    lg: *mut LiveGraph,
    slot_id: usize,
    track_name: &str,
    predecessor_id: i32,
    successor_id: i32,
    existing_effect: Option<i32>,
    last_source: &str,
    existing_name: Option<&str>,
    sample_rate: u32,
) -> Option<LispEditResult> {
    let initial = if last_source.is_empty() {
        TEMPLATE.to_string()
    } else {
        last_source.to_string()
    };

    let mut source = initial;

    loop {
        // Open editor
        match edit_text(&source) {
            Ok(edited) => {
                source = edited;
            }
            Err(e) => {
                eprintln!("Editor error: {e}");
                return None;
            }
        }

        // Compile
        print!("Compiling...");
        io::stdout().flush().ok();

        match compile_lisp(&source, sample_rate) {
            Ok(json) => {
                match parse_manifest(&json) {
                    Ok(manifest) => {
                        match load_dylib(&manifest.dylib_path) {
                            Ok(lib) => {
                                // Add to graph
                                match unsafe {
                                    add_effect_to_chain_at(
                                        lg,
                                        slot_id,
                                        &manifest,
                                        &lib,
                                        predecessor_id,
                                        successor_id,
                                        existing_effect,
                                    )
                                } {
                                    Ok(node_id) => {
                                        println!(" OK!");
                                        let n = manifest.params.len();
                                        if n > 0 {
                                            println!("  Parameters:");
                                            for p in &manifest.params {
                                                println!(
                                                    "    {} = {} [{}, {}]{}",
                                                    p.name,
                                                    p.default,
                                                    p.min,
                                                    p.max,
                                                    p.unit
                                                        .as_deref()
                                                        .map(|u| format!(" {u}"))
                                                        .unwrap_or_default()
                                                );
                                            }
                                        }

                                        // Name prompt
                                        let default_name = existing_name.unwrap_or("");
                                        if default_name.is_empty() {
                                            print!("\nEffect name: ");
                                        } else {
                                            print!("\nEffect name [{}]: ", default_name);
                                        }
                                        io::stdout().flush().ok();
                                        let mut name_buf = String::new();
                                        std::io::stdin().read_line(&mut name_buf).ok();
                                        let name_input = name_buf.trim();
                                        let name = if name_input.is_empty() {
                                            if default_name.is_empty() {
                                                "untitled".to_string()
                                            } else {
                                                default_name.to_string()
                                            }
                                        } else {
                                            sanitize_effect_name(name_input)
                                        };

                                        // Save to effects/ library
                                        match save_effect(&name, &source) {
                                            Ok(()) => println!("Saved to effects/{}.lisp", name),
                                            Err(e) => eprintln!("Warning: failed to save: {e}"),
                                        }

                                        println!(
                                            "\nEffect '{}' added to track '{}'",
                                            name, track_name
                                        );
                                        println!("Press Enter to return to sequencer...");
                                        let mut buf = String::new();
                                        std::io::stdin().read_line(&mut buf).ok();
                                        let params = manifest.params.clone();
                                        return Some(LispEditResult {
                                            node_id,
                                            lib,
                                            source,
                                            params,
                                            name,
                                        });
                                    }
                                    Err(e) => {
                                        eprintln!(" Failed to add to graph: {e}");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!(" Failed to load dylib: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(" Failed to parse manifest: {e}");
                    }
                }
            }
            Err(e) => {
                println!();
                eprintln!("Compile error:\n{e}");
            }
        }

        // On any error, offer to re-edit
        eprint!("\nPress Enter to re-edit, or 'q' + Enter to cancel: ");
        io::stdout().flush().ok();
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf).ok();
        if buf.trim() == "q" {
            return None;
        }
    }
}

fn sanitize_effect_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}
