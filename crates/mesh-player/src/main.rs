// Hide console window on Windows (GUI application)
// This sets the executable subsystem to WINDOWS instead of CONSOLE
// Use --features console to show the console for debugging
#![cfg_attr(
    all(target_os = "windows", not(feature = "console")),
    windows_subsystem = "windows"
)]

//! Mesh DJ Player - 4-deck stem-based mixing with neural effects
//!
//! This is the main entry point for the GUI application. It:
//! 1. Starts the CPAL audio system in background threads
//! 2. Launches the iced GUI application
//! 3. Passes shared state between UI and audio
//!
//! ## Command line flags
//!
//! - `--midi-learn`: Start in MIDI learn mode for creating controller profiles

mod audio;
mod config;
mod direct_dispatch;
mod domain;
mod loader;
mod plugin_gui;
mod suggestions;
mod ui;

use iced::{Size, Task};

use audio::start_audio_system;
use mesh_core::db::DatabaseService;
use ui::{MeshApp, app::Message, midi_learn::MidiLearnMessage, theme};

const CLIENT_NAME: &str = "mesh-player";

fn main() -> iced::Result {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    let start_midi_learn = args.iter().any(|arg| arg == "--midi-learn");

    // Initialize logger - set RUST_LOG=debug for verbose output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("mesh-player starting up");
    if start_midi_learn {
        log::info!("Mapping mode requested via --midi-learn flag");
    } else {
        log::info!("Starting in performance mode (use --midi-learn for full UI)");
    }

    // Initialize Rayon thread pool before audio starts
    // This prevents lazy initialization from causing latency in the audio callback
    // (Rayon's default lazy init would happen on first parallel call, which is in audio callback)
    rayon::ThreadPoolBuilder::new()
        .num_threads(4) // Match NUM_DECKS for optimal stem parallelism
        .thread_name(|i| format!("rayon-audio-{}", i))
        .build_global()
        .expect("Failed to initialize Rayon thread pool");
    log::info!("Rayon thread pool initialized with 4 threads");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                     Mesh DJ Player                            ║");
    println!("║              4-deck stem-based mixing                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Load config to get collection path for database service
    let config_path = config::default_config_path();
    let config: config::PlayerConfig = config::load_config(&config_path);

    // Create database service (required for track metadata loading)
    let db_service = DatabaseService::new(&config.collection_path)
        .expect("Failed to create database service - this is required for mesh-player");
    log::info!("Database service initialized at {:?}", config.collection_path);

    // Try to start audio system
    // Returns AudioHandle, CommandSender (lock-free queue), DeckAtomics, SlicerAtomics,
    // LinkedStemAtomics, LinkedStemResultReceiver, and sample rate
    let (audio_handle, command_sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, audio_sample_rate, audio_client_name, output_latency_samples, internal_latency_samples, direct_command_producer) =
        match start_audio_system(CLIENT_NAME, db_service.clone()) {
            Ok((handle, sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, sample_rate, client_name, output_lat, internal_lat, direct_producer)) => {
                println!("Audio system started successfully ({} Hz, client: {})", sample_rate, client_name);
                (Some(handle), Some(sender), Some(deck_atomics), Some(slicer_atomics), Some(linked_stem_atomics), Some(linked_stem_receiver), Some(clip_indicator), sample_rate, client_name, Some(output_lat), Some(internal_lat), Some(direct_producer))
            }
            Err(e) => {
                eprintln!("Warning: Could not start audio system: {}", e);
                eprintln!("Running in UI-only mode (no audio output)");
                eprintln!();
                eprintln!("Check that audio devices are available and not in use by other applications.");
                // Default to 44100 Hz when audio is not available
                (None, None, None, None, None, None, None, 44100, "mesh-player".to_string(), None, None, None)
            }
        };

    println!();
    println!("Starting Mesh DJ Player GUI...");

    // Initialize theme from ~/Music/mesh-collection/theme.yaml
    theme::init_theme();

    // Wrap resources in cells so the boot closure can be Fn (required by iced)
    // The boot function is only called once, but iced requires Fn for API consistency
    let db_service_cell = std::cell::RefCell::new(Some(db_service));
    let command_sender_cell = std::cell::RefCell::new(command_sender);
    let deck_atomics_cell = std::cell::RefCell::new(deck_atomics);
    let slicer_atomics_cell = std::cell::RefCell::new(slicer_atomics);
    let linked_stem_atomics_cell = std::cell::RefCell::new(linked_stem_atomics);
    let linked_stem_receiver_cell = std::cell::RefCell::new(linked_stem_receiver);
    let clip_indicator_cell = std::cell::RefCell::new(clip_indicator);
    let output_latency_cell = std::cell::RefCell::new(output_latency_samples);
    let internal_latency_cell = std::cell::RefCell::new(internal_latency_samples);
    let direct_producer_cell = std::cell::RefCell::new(direct_command_producer);

    // Run the iced application using the functional API
    let result = iced::application(
        move || {
            // Boot function: creates initial state with lock-free command channel
            // Take ownership from the cells (only called once)
            let db_service = db_service_cell.borrow_mut().take().expect("db_service already taken");
            let sender = command_sender_cell.borrow_mut().take();
            let deck_atomics = deck_atomics_cell.borrow_mut().take();
            let slicer_atomics = slicer_atomics_cell.borrow_mut().take();
            let linked_stem_atomics = linked_stem_atomics_cell.borrow_mut().take();
            let linked_stem_receiver = linked_stem_receiver_cell.borrow_mut().take();
            let clip_indicator = clip_indicator_cell.borrow_mut().take();
            let output_latency = output_latency_cell.borrow_mut().take();
            let internal_latency = internal_latency_cell.borrow_mut().take();
            let direct_producer = direct_producer_cell.borrow_mut().take();

            // Create direct dispatch for timing-critical MIDI→engine path
            let direct_dispatch = direct_producer.map(|producer| {
                std::sync::Arc::new(direct_dispatch::EngineDirectDispatch::new(producer))
                    as std::sync::Arc<dyn mesh_midi::DirectDispatch>
            });

            // mapping_mode = true shows full UI with controls, false = performance mode
            let mut app = MeshApp::new(db_service, sender, deck_atomics, slicer_atomics, linked_stem_atomics, linked_stem_receiver, clip_indicator, audio_sample_rate, audio_client_name.clone(), start_midi_learn, output_latency, internal_latency);

            // Wire direct dispatch to controller for bypassing iced tick on timing-critical commands
            if let (Some(ref mut controller), Some(dispatch)) = (&mut app.controller, direct_dispatch) {
                controller.set_direct_dispatch(dispatch);
                log::info!("Direct MIDI→engine dispatch enabled");
            }

            // If --midi-learn flag was passed, start MIDI learn mode (opens the drawer)
            let startup_task = if start_midi_learn {
                Task::done(Message::MidiLearn(MidiLearnMessage::Start))
            } else {
                Task::none()
            };

            (app, startup_task)
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(theme)
    .title("Mesh DJ Player")
    .window_size(Size::new(1200.0, 800.0))
    .run();

    // Keep audio handle alive until we're done (it will be dropped here)
    drop(audio_handle);
    println!("Mesh DJ Player stopped.");

    result
}

/// Update function for iced
fn update(app: &mut MeshApp, message: Message) -> Task<Message> {
    app.update(message)
}

/// View function for iced
fn view(app: &MeshApp) -> iced::Element<'_, Message> {
    app.view()
}

/// Subscription function for iced
fn subscription(app: &MeshApp) -> iced::Subscription<Message> {
    app.subscription()
}

/// Theme function for iced
fn theme(app: &MeshApp) -> iced::Theme {
    app.theme()
}
