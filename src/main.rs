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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Ensure samples/ and effects/ directories exist
    std::fs::create_dir_all("samples").ok();
    std::fs::create_dir_all("effects").ok();

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

    // Create two bus nodes (L and R)
    let bus_l_name = CString::new("bus_L").unwrap();
    let bus_r_name = CString::new("bus_R").unwrap();
    let bus_l_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_l_name.as_ptr()) };
    let bus_r_id = unsafe { audiograph::live_add_gain(lg, 1.0, bus_r_name.as_ptr()) };

    // bus_L → DAC channel 0, bus_R → DAC channel 1
    unsafe {
        audiograph::graph_connect(lg, bus_l_id, 0, 0, 0);
        if channels > 1 {
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 1);
        } else {
            audiograph::graph_connect(lg, bus_r_id, 0, 0, 0);
        }
    }

    // Start engine with 0 workers (audio callback is single-threaded)
    unsafe {
        audiograph::engine_start_workers(0);
    }

    // Create shared sequencer state (start with 0 tracks)
    let state = Arc::new(sequencer::SequencerState::new(0, vec![]));

    // Build cpal audio stream
    let _stream = audio::build_output_stream(
        lg,
        Arc::clone(&state),
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
    let mut app = ui::App::new(Arc::clone(&state), lg_ptr, sample_rate, bus_l_id, bus_r_id);

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
