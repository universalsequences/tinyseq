mod audiograph;
mod audio;
mod sampler;
mod sequencer;
mod ui;

use std::ffi::CString;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Scan samples/ directory for .wav files
    let mut wav_paths: Vec<std::path::PathBuf> = std::fs::read_dir("samples")
        .unwrap_or_else(|_| {
            eprintln!("Warning: samples/ directory not found. Creating it...");
            std::fs::create_dir_all("samples").ok();
            std::fs::read_dir("samples").expect("Failed to create samples/")
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|ext| ext.to_ascii_lowercase() == "wav")
                .unwrap_or(false)
        })
        .collect();
    wav_paths.sort();

    if wav_paths.is_empty() {
        eprintln!("No .wav files found in samples/. Place some .wav files there and re-run.");
    }

    // Query audio device
    let (sample_rate, channels) = audio::query_device_config()?;
    let block_size: usize = 512;

    // Initialize audiograph engine
    unsafe {
        audiograph::initialize_engine(block_size as i32, sample_rate as i32);
    }

    let label = CString::new("sequencer").unwrap();
    let lg = unsafe {
        audiograph::create_live_graph(
            64,
            block_size as i32,
            label.as_ptr(),
            channels as i32,
        )
    };
    if lg.is_null() {
        return Err("Failed to create live graph".into());
    }

    // Create a bus (gain=1.0) node to mix all samplers before the DAC.
    // All samplers connect to the bus input (auto-sum mixes them).
    // The bus output fans out to all DAC channels (same edge, true stereo).
    let bus_name = CString::new("bus").unwrap();
    let bus_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_name.as_ptr()) };

    // Fan out bus → all DAC channels
    for ch in 0..channels as i32 {
        unsafe {
            audiograph::graph_connect(lg, bus_id, 0, 0, ch);
        }
    }

    // Load samples and connect each sampler → bus
    let mut tracks = Vec::new();
    for wav_path in &wav_paths {
        match sampler::create_sampler_track(lg, wav_path) {
            Ok(track) => {
                unsafe {
                    audiograph::graph_connect(lg, track.node_id, 0, bus_id, 0);
                }
                tracks.push(track);
            }
            Err(e) => {
                eprintln!("Warning: Failed to load {}: {e}", wav_path.display());
            }
        }
    }

    // Start engine with 0 workers (audio callback is single-threaded)
    unsafe {
        audiograph::engine_start_workers(0);
    }

    // Create shared sequencer state
    let state = Arc::new(sequencer::SequencerState::new(tracks.len()));

    // Build cpal audio stream
    let _stream = audio::build_output_stream(
        lg,
        Arc::clone(&state),
        tracks
            .iter()
            .map(|t| sampler::SamplerTrack {
                name: t.name.clone(),
                node_id: t.node_id,
                logical_id: t.logical_id,
            })
            .collect(),
        sample_rate,
        channels as usize,
        block_size,
    )?;

    // Setup terminal
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Create UI app
    let mut app = ui::App::new(Arc::clone(&state), &tracks);

    // Main loop
    loop {
        terminal.draw(|f| ui::draw(f, &app))?;
        app.handle_input()?;
        if app.should_quit {
            break;
        }
    }

    // Cleanup
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    unsafe {
        audiograph::engine_stop_workers();
        audiograph::destroy_live_graph(lg);
    }

    Ok(())
}
