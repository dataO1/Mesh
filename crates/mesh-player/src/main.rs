//! Mesh DJ Player - 4-deck stem-based mixing with neural effects
//!
//! This is the main entry point for the GUI application. It:
//! 1. Starts the JACK audio client in a background thread
//! 2. Launches the iced GUI application
//! 3. Passes shared state between UI and audio

mod audio;
mod config;
mod loader;
mod ui;

use iced::{Size, Task};

use audio::{start_jack_client, auto_connect_ports};
use ui::{MeshApp, app::Message};

const CLIENT_NAME: &str = "mesh-player";

fn main() -> iced::Result {
    // Initialize logger - set RUST_LOG=debug for verbose output
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("mesh-player starting up");

    // Initialize Rayon thread pool before audio starts
    // This prevents lazy initialization from causing latency in the audio callback
    // (Rayon's default lazy init would happen on first parallel call, which is in JACK callback)
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

    // Try to start JACK client
    // Returns CommandSender (lock-free queue), DeckAtomics, and sample rate
    let (jack_handle, command_sender, deck_atomics, jack_sample_rate) = match start_jack_client(CLIENT_NAME) {
        Ok((handle, sender, atomics, sample_rate)) => {
            println!("JACK client started successfully (lock-free command queue, {} Hz)", sample_rate);

            // Try to auto-connect to system outputs
            if let Err(e) = auto_connect_ports(CLIENT_NAME) {
                eprintln!("Warning: Could not auto-connect ports: {}", e);
            }

            (Some(handle), Some(sender), Some(atomics), sample_rate)
        }
        Err(e) => {
            eprintln!("Warning: Could not start JACK client: {}", e);
            eprintln!("Running in UI-only mode (no audio output)");
            eprintln!();
            eprintln!("To enable audio, make sure JACK server is running:");
            eprintln!("  jackd -d alsa -r 44100");
            eprintln!("or use QjackCtl/Cadence to start it.");
            // Default to 48000 Hz when JACK is not available (matches SAMPLE_RATE constant)
            (None, None, None, 48000)
        }
    };

    println!();
    println!("Starting Mesh DJ Player GUI...");

    // Wrap command_sender in a cell so the boot closure can be Fn (required by iced)
    // The boot function is only called once, but iced requires Fn for API consistency
    let command_sender_cell = std::cell::RefCell::new(command_sender);
    let deck_atomics_cell = std::cell::RefCell::new(deck_atomics);

    // Run the iced application using the functional API
    let result = iced::application(
        move || {
            // Boot function: creates initial state with lock-free command channel
            // Take ownership from the cells (only called once)
            let sender = command_sender_cell.borrow_mut().take();
            let atomics = deck_atomics_cell.borrow_mut().take();
            let app = MeshApp::new(sender, atomics, jack_sample_rate);
            (app, Task::none())
        },
        update,
        view,
    )
    .subscription(subscription)
    .theme(theme)
    .title("Mesh DJ Player")
    .window_size(Size::new(1200.0, 800.0))
    .run();

    // Keep JACK handle alive until we're done (it will be dropped here)
    drop(jack_handle);
    println!("Mesh DJ Player stopped.");

    result
}

/// Update function for iced
fn update(app: &mut MeshApp, message: Message) -> Task<Message> {
    app.update(message)
}

/// View function for iced
fn view(app: &MeshApp) -> iced::Element<Message> {
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
