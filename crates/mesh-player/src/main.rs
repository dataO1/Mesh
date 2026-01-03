//! Mesh DJ Player - 4-deck stem-based mixing with neural effects
//!
//! This is the main entry point for the GUI application. It:
//! 1. Starts the JACK audio client in a background thread
//! 2. Launches the iced GUI application
//! 3. Passes shared state between UI and audio

mod audio;
mod ui;

use iced::{Size, Task};

use audio::{start_jack_client, auto_connect_ports};
use ui::{MeshApp, app::Message};

const CLIENT_NAME: &str = "mesh-player";

fn main() -> iced::Result {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                     Mesh DJ Player                            ║");
    println!("║              4-deck stem-based mixing                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Try to start JACK client
    let (jack_handle, audio_state) = match start_jack_client(CLIENT_NAME) {
        Ok((handle, state)) => {
            println!("JACK client started successfully");

            // Try to auto-connect to system outputs
            if let Err(e) = auto_connect_ports(CLIENT_NAME) {
                eprintln!("Warning: Could not auto-connect ports: {}", e);
            }

            (Some(handle), Some(state))
        }
        Err(e) => {
            eprintln!("Warning: Could not start JACK client: {}", e);
            eprintln!("Running in UI-only mode (no audio output)");
            eprintln!();
            eprintln!("To enable audio, make sure JACK server is running:");
            eprintln!("  jackd -d alsa -r 44100");
            eprintln!("or use QjackCtl/Cadence to start it.");
            (None, None)
        }
    };

    println!();
    println!("Starting Mesh DJ Player GUI...");

    // Run the iced application using the functional API
    let result = iced::application(
        move || {
            // Boot function: creates initial state
            let app = MeshApp::new(audio_state.clone());
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
