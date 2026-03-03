mod audio;
mod audiograph;
#[allow(dead_code)]
mod delay;
#[allow(dead_code)]
mod effects;
#[allow(dead_code)]
mod filter;
#[allow(dead_code)]
mod lisp_effect;
mod sampler;
#[allow(dead_code)]
mod sequencer;
mod ui;

use std::ffi::CString;
use std::sync::Arc;

use crate::audio::TrackNodes;

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
        audiograph::create_live_graph(64, block_size as i32, label.as_ptr(), channels as i32)
    };
    if lg.is_null() {
        return Err("Failed to create live graph".into());
    }

    // Create two bus nodes (L and R) instead of a single bus
    let bus_l_name = CString::new("bus_L").unwrap();
    let bus_r_name = CString::new("bus_R").unwrap();
    let bus_l_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_l_name.as_ptr()) };
    let bus_r_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_r_name.as_ptr()) };

    // bus_L → DAC channel 0, bus_R → DAC channel 1
    unsafe {
        audiograph::graph_connect(lg, bus_l_id, 0, 0, 0); // bus_L → DAC ch0
        if channels > 1 {
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 1); // bus_R → DAC ch1
        } else {
            // Mono: both go to ch0
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 0);
        }
    }

    // Load samples and create per-track signal chains:
    // sampler → filter → delay(stereo out)
    // delay out 0 → bus_L, delay out 1 → bus_R
    let mut track_nodes = Vec::new();
    for wav_path in &wav_paths {
        match sampler::create_sampler_track(lg, wav_path) {
            Ok(sampler_track) => {
                let track_name = sampler_track.name.clone();

                // Create filter node (1 in, 1 out)
                let filter_name = CString::new(format!("{}_filter", track_name)).unwrap();
                let filter_id = unsafe {
                    audiograph::add_node(
                        lg,
                        filter::filter_vtable(),
                        filter::FILTER_STATE_SIZE * std::mem::size_of::<f32>(),
                        filter_name.as_ptr(),
                        1, // 1 input
                        1, // 1 output
                        std::ptr::null(),
                        0,
                    )
                };

                // Create delay node (1 in, 2 out — stereo)
                let delay_name = CString::new(format!("{}_delay", track_name)).unwrap();
                let delay_id = unsafe {
                    audiograph::add_node(
                        lg,
                        delay::delay_vtable(),
                        delay::DELAY_STATE_SIZE * std::mem::size_of::<f32>(),
                        delay_name.as_ptr(),
                        1, // 1 input
                        2, // 2 outputs (stereo)
                        std::ptr::null(),
                        0,
                    )
                };

                // Connect: sampler → filter → delay
                unsafe {
                    audiograph::graph_connect(lg, sampler_track.node_id, 0, filter_id, 0);
                    audiograph::graph_connect(lg, filter_id, 0, delay_id, 0);
                    // delay out 0 (L) → bus_L
                    audiograph::graph_connect(lg, delay_id, 0, bus_l_id, 0);
                    // delay out 1 (R) → bus_R
                    audiograph::graph_connect(lg, delay_id, 1, bus_r_id, 0);
                }

                track_nodes.push(TrackNodes {
                    name: track_name,
                    sampler_lid: sampler_track.logical_id,
                    filter_lid: filter_id as u64,
                    delay_lid: delay_id as u64,
                });
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
    let state = Arc::new(sequencer::SequencerState::new(track_nodes.len()));

    // Build cpal audio stream
    let _stream = audio::build_output_stream(
        lg,
        Arc::clone(&state),
        &track_nodes,
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
    let lg_ptr = audiograph::LiveGraphPtr(lg);
    let mut app = ui::App::new(Arc::clone(&state), &track_nodes, lg_ptr, sample_rate);

    // Main loop
    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;
        app.handle_input()?;
        if app.should_quit {
            break;
        }
        if app.pending_lisp_edit {
            app.pending_lisp_edit = false;

            // Suspend terminal for editor
            crossterm::terminal::disable_raw_mode()?;
            crossterm::execute!(
                terminal.backend_mut(),
                crossterm::terminal::LeaveAlternateScreen,
                crossterm::event::DisableMouseCapture
            )?;
            terminal.show_cursor()?;

            app.run_lisp_editor_flow();

            // Resume terminal
            crossterm::terminal::enable_raw_mode()?;
            crossterm::execute!(
                terminal.backend_mut(),
                crossterm::terminal::EnterAlternateScreen,
                crossterm::event::EnableMouseCapture
            )?;
            terminal.clear()?;
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
