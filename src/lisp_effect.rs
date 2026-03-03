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
// Each track can have one custom DGenLisp effect.
// The process fn pointer is stored here, indexed by track.

const MAX_TRACKS: usize = 16;
static DGEN_PROCESS_FNS: [AtomicUsize; MAX_TRACKS] = {
    const INIT: AtomicUsize = AtomicUsize::new(0);
    [INIT; MAX_TRACKS]
};

fn set_dgen_process_fn(track_idx: usize, f: DGenProcessFn) {
    DGEN_PROCESS_FNS[track_idx].store(f as usize, Ordering::Release);
}

// ── Node state layout ──
// state[0] = track_idx (f32)
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
    let track_idx = (*s) as usize;
    let fn_ptr = DGEN_PROCESS_FNS[track_idx % MAX_TRACKS].load(Ordering::Acquire);
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
    let total_memory_slots = (*src.add(1)) as usize;
    let total_slots = HEADER_SLOTS + total_memory_slots;
    let dst = state as *mut f32;
    std::ptr::copy_nonoverlapping(src, dst, total_slots);
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

    let n_inputs = v["inputs"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
        .max(1);
    let n_outputs = v["outputs"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
        .max(1);

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

// ── Build initial state ──

fn build_initial_state(
    track_idx: usize,
    manifest: &DGenManifest,
) -> Vec<f32> {
    let total_slots = HEADER_SLOTS + manifest.total_memory_slots;
    let mut state = vec![0.0f32; total_slots];

    // Header
    state[0] = track_idx as f32;
    state[1] = manifest.total_memory_slots as f32;

    // Default param values
    let mem = &mut state[HEADER_SLOTS..];
    for param in &manifest.params {
        if param.cell_id < manifest.total_memory_slots {
            mem[param.cell_id] = param.default;
        }
    }

    // Tensor init data
    for tensor in &manifest.tensor_init_data {
        for (i, &val) in tensor.data.iter().enumerate() {
            let idx = tensor.offset + i;
            if idx < manifest.total_memory_slots {
                mem[idx] = val;
            }
        }
    }

    state
}

// ── Add effect to track's audio chain ──

/// Remove an existing custom effect from the track chain and reconnect sampler → filter.
pub unsafe fn remove_custom_effect(
    lg: *mut LiveGraph,
    effect_node_id: i32,
    sampler_id: i32,
    filter_id: i32,
) {
    audiograph::graph_disconnect(lg, sampler_id, 0, effect_node_id, 0);
    audiograph::graph_disconnect(lg, effect_node_id, 0, filter_id, 0);
    audiograph::delete_node(lg, effect_node_id);
    audiograph::graph_connect(lg, sampler_id, 0, filter_id, 0);
}

/// Add a DGenLisp effect between sampler and filter.
pub unsafe fn add_effect_to_chain(
    lg: *mut LiveGraph,
    track_idx: usize,
    manifest: &DGenManifest,
    lib: &LoadedDGenLib,
    sampler_id: i32,
    filter_id: i32,
    existing_effect: Option<i32>,
) -> Result<i32, String> {
    // Remove old effect if present
    if let Some(old_id) = existing_effect {
        remove_custom_effect(lg, old_id, sampler_id, filter_id);
    }

    // Register process function
    set_dgen_process_fn(track_idx, lib.process_fn);

    // Build initial state
    let initial_state = build_initial_state(track_idx, manifest);
    let state_size = initial_state.len() * std::mem::size_of::<f32>();

    let name = CString::new(format!("dgenlisp_fx_{}", track_idx)).unwrap();

    let node_id = audiograph::add_node(
        lg,
        dgenlisp_vtable(),
        state_size,
        name.as_ptr(),
        manifest.n_inputs as c_int,
        1, // mono output for insert chain
        initial_state.as_ptr() as *const c_void,
        state_size,
    );

    if node_id < 0 {
        return Err("Failed to add DGenLisp node to graph".to_string());
    }

    // Rewire: sampler → effect → filter (disconnect existing sampler → filter first)
    audiograph::graph_disconnect(lg, sampler_id, 0, filter_id, 0);
    audiograph::graph_connect(lg, sampler_id, 0, node_id, 0);
    audiograph::graph_connect(lg, node_id, 0, filter_id, 0);

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
}

/// Run the full edit → compile → load → wire flow.
/// Called while terminal is in normal (non-raw) mode.
pub fn run_editor_flow(
    lg: *mut LiveGraph,
    track_idx: usize,
    track_name: &str,
    sampler_id: i32,
    filter_id: i32,
    existing_effect: Option<i32>,
    last_source: &str,
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
                                    add_effect_to_chain(
                                        lg,
                                        track_idx,
                                        &manifest,
                                        &lib,
                                        sampler_id,
                                        filter_id,
                                        existing_effect,
                                    )
                                } {
                                    Ok(node_id) => {
                                        println!(
                                            " OK! Effect added to track '{}'",
                                            track_name
                                        );
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
                                        println!("\nPress Enter to return to sequencer...");
                                        let mut buf = String::new();
                                        std::io::stdin().read_line(&mut buf).ok();
                                        let params = manifest.params.clone();
                                        return Some(LispEditResult {
                                            node_id,
                                            lib,
                                            source,
                                            params,
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
