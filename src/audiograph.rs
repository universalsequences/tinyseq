use std::os::raw::{c_char, c_int, c_void};

/// Mirrors C `NodeVTable` — function pointers for a DSP node.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct NodeVTable {
    pub process: Option<
        unsafe extern "C" fn(
            inp: *const *mut f32,
            out: *const *mut f32,
            nframes: c_int,
            state: *mut c_void,
            buffers: *mut c_void,
        ),
    >,
    pub init: Option<
        unsafe extern "C" fn(
            state: *mut c_void,
            sample_rate: c_int,
            max_block: c_int,
            initial_state: *const c_void,
        ),
    >,
    pub reset: Option<unsafe extern "C" fn(state: *mut c_void)>,
    pub migrate: Option<unsafe extern "C" fn(new_state: *mut c_void, old_state: *const c_void)>,
}

/// Mirrors C `ParamMsg`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ParamMsg {
    pub idx: u64,
    pub logical_id: u64,
    pub fvalue: f32,
}

/// Mirrors C `BufferDesc`.
#[repr(C)]
pub struct BufferDesc {
    pub buffer: *mut f32,
    pub size: c_int,
    pub channel_count: c_int,
}

/// Opaque handle — we only ever hold `*mut LiveGraph`.
#[repr(C)]
pub struct LiveGraph {
    _opaque: [u8; 0],
}

unsafe impl Send for LiveGraphPtr {}
unsafe impl Sync for LiveGraphPtr {}

/// Wrapper so we can send `*mut LiveGraph` across threads.
#[derive(Copy, Clone)]
pub struct LiveGraphPtr(pub *mut LiveGraph);

extern "C" {
    // Engine lifecycle
    pub fn initialize_engine(block_size: c_int, sample_rate: c_int);
    pub fn engine_start_workers(workers: c_int);
    pub fn engine_stop_workers();

    // Graph lifecycle
    pub fn create_live_graph(
        initial_capacity: c_int,
        block_size: c_int,
        label: *const c_char,
        num_channels: c_int,
    ) -> *mut LiveGraph;
    pub fn destroy_live_graph(lg: *mut LiveGraph);

    // Queue-based node API
    pub fn add_node(
        lg: *mut LiveGraph,
        vtable: NodeVTable,
        state_size: usize,
        name: *const c_char,
        n_inputs: c_int,
        n_outputs: c_int,
        initial_state: *const c_void,
        initial_state_size: usize,
    ) -> c_int;

    // Connections
    pub fn graph_connect(
        lg: *mut LiveGraph,
        src_node: c_int,
        src_port: c_int,
        dst_node: c_int,
        dst_port: c_int,
    ) -> bool;

    // Buffer management
    pub fn create_buffer(
        lg: *mut LiveGraph,
        size: c_int,
        channel_count: c_int,
        source_data: *const f32,
    ) -> c_int;

    // Built-in node factories
    pub fn live_add_gain(lg: *mut LiveGraph, gain_value: f32, name: *const c_char) -> c_int;

    // Audio processing
    pub fn process_next_block(lg: *mut LiveGraph, output_buffer: *mut f32, nframes: c_int);

    // Wrapper for the static-inline params_push
    pub fn params_push_wrapper(lg: *mut LiveGraph, m: ParamMsg) -> bool;

    // Debug
    pub fn debug_dump_graph(lg: *mut LiveGraph);
}
