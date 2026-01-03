//! JACK audio client for Mesh DJ Player
//!
//! Connects the AudioEngine to JACK for real-time audio output.
//! Provides 4 output channels: Master L/R and Cue L/R.

use std::sync::{Arc, Mutex};

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};
use mesh_core::engine::AudioEngine;
use mesh_core::types::StereoBuffer;

/// JACK output port names
const MASTER_LEFT: &str = "master_left";
const MASTER_RIGHT: &str = "master_right";
const CUE_LEFT: &str = "cue_left";
const CUE_RIGHT: &str = "cue_right";

/// Handle to the active JACK client
pub struct JackHandle {
    /// The async client (keeps JACK running)
    _async_client: jack::AsyncClient<JackNotifications, JackProcessor>,
}

impl JackHandle {
    /// Check if the client is still active
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        true // AsyncClient keeps running until dropped
    }
}

/// Shared state between main thread and audio thread
pub struct SharedState {
    /// The audio engine
    pub engine: AudioEngine,
    /// Buffer size (set by JACK)
    pub buffer_size: usize,
}

impl SharedState {
    /// Create new shared state
    pub fn new() -> Self {
        Self {
            engine: AudioEngine::new(),
            buffer_size: 256, // Default, will be updated by JACK
        }
    }
}

impl Default for SharedState {
    fn default() -> Self {
        Self::new()
    }
}

/// JACK process handler
struct JackProcessor {
    /// Output ports
    master_left: Port<AudioOut>,
    master_right: Port<AudioOut>,
    cue_left: Port<AudioOut>,
    cue_right: Port<AudioOut>,
    /// Shared state with the engine
    state: Arc<Mutex<SharedState>>,
    /// Pre-allocated buffers for processing
    master_buffer: StereoBuffer,
    cue_buffer: StereoBuffer,
}

impl jack::ProcessHandler for JackProcessor {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        let n_frames = ps.n_frames() as usize;

        // Resize buffers if needed
        if self.master_buffer.len() != n_frames {
            self.master_buffer.resize(n_frames);
            self.cue_buffer.resize(n_frames);
        }

        // Try to lock the engine (non-blocking to avoid priority inversion)
        if let Ok(mut state) = self.state.try_lock() {
            // Update buffer size if changed
            if state.buffer_size != n_frames {
                state.buffer_size = n_frames;
            }

            // Process audio through the engine
            state.engine.process(&mut self.master_buffer, &mut self.cue_buffer);
        } else {
            // Couldn't get lock, output silence
            self.master_buffer.fill_silence();
            self.cue_buffer.fill_silence();
        }

        // Copy to JACK output buffers
        let master_left_out = self.master_left.as_mut_slice(ps);
        let master_right_out = self.master_right.as_mut_slice(ps);
        let cue_left_out = self.cue_left.as_mut_slice(ps);
        let cue_right_out = self.cue_right.as_mut_slice(ps);

        for i in 0..n_frames {
            let master = self.master_buffer[i];
            let cue = self.cue_buffer[i];

            master_left_out[i] = master.left;
            master_right_out[i] = master.right;
            cue_left_out[i] = cue.left;
            cue_right_out[i] = cue.right;
        }

        Control::Continue
    }
}

/// JACK notification handler
struct JackNotifications;

impl jack::NotificationHandler for JackNotifications {
    unsafe fn shutdown(&mut self, _status: jack::ClientStatus, _reason: &str) {
        eprintln!("JACK server shut down");
    }

    fn sample_rate(&mut self, _client: &Client, srate: jack::Frames) -> Control {
        println!("JACK sample rate: {}", srate);
        Control::Continue
    }

    fn xrun(&mut self, _client: &Client) -> Control {
        eprintln!("JACK xrun!");
        Control::Continue
    }
}

/// Error type for JACK operations
#[derive(Debug)]
#[allow(dead_code)]
pub enum JackError {
    /// Failed to create JACK client
    ClientCreation(String),
    /// Failed to register port
    PortRegistration(String),
    /// Failed to activate client
    Activation(String),
    /// Failed to connect ports
    Connection(String),
}

impl std::fmt::Display for JackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JackError::ClientCreation(msg) => write!(f, "Failed to create JACK client: {}", msg),
            JackError::PortRegistration(msg) => write!(f, "Failed to register port: {}", msg),
            JackError::Activation(msg) => write!(f, "Failed to activate client: {}", msg),
            JackError::Connection(msg) => write!(f, "Failed to connect ports: {}", msg),
        }
    }
}

impl std::error::Error for JackError {}

/// Start the JACK audio client
///
/// Returns a handle to the active client and a shared state for controlling
/// the audio engine from the main thread.
pub fn start_jack_client(
    client_name: &str,
) -> Result<(JackHandle, Arc<Mutex<SharedState>>), JackError> {
    // Create JACK client
    let (client, _status) = Client::new(client_name, ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    println!(
        "JACK client '{}' created (sample rate: {}, buffer size: {})",
        client.name(),
        client.sample_rate(),
        client.buffer_size()
    );

    // Register output ports
    let master_left = client
        .register_port(MASTER_LEFT, AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    let master_right = client
        .register_port(MASTER_RIGHT, AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    let cue_left = client
        .register_port(CUE_LEFT, AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    let cue_right = client
        .register_port(CUE_RIGHT, AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    // Create shared state
    let state = Arc::new(Mutex::new(SharedState {
        engine: AudioEngine::new(),
        buffer_size: client.buffer_size() as usize,
    }));

    // Create processor with pre-allocated buffers
    let buffer_size = client.buffer_size() as usize;
    let processor = JackProcessor {
        master_left,
        master_right,
        cue_left,
        cue_right,
        state: Arc::clone(&state),
        master_buffer: StereoBuffer::silence(buffer_size),
        cue_buffer: StereoBuffer::silence(buffer_size),
    };

    // Activate the client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| JackError::Activation(e.to_string()))?;

    println!("JACK client activated");

    Ok((
        JackHandle {
            _async_client: async_client,
        },
        state,
    ))
}

/// Try to auto-connect to system playback ports
pub fn auto_connect_ports(client_name: &str) -> Result<(), JackError> {
    // Create a temporary client just for connecting
    let (client, _) = Client::new(&format!("{}_connect", client_name), ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    // Find system playback ports
    let playback_ports = client.ports(
        Some("system:playback_.*"),
        None,
        jack::PortFlags::IS_INPUT,
    );

    if playback_ports.len() >= 2 {
        // Connect master outputs to system playback
        let master_left_port = format!("{}:{}", client_name, MASTER_LEFT);
        let master_right_port = format!("{}:{}", client_name, MASTER_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&master_left_port, &playback_ports[0]) {
            eprintln!("Warning: Could not connect master left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&master_right_port, &playback_ports[1]) {
            eprintln!("Warning: Could not connect master right: {}", e);
        }

        println!(
            "Connected master outputs to {} and {}",
            playback_ports[0], playback_ports[1]
        );
    }

    // If there are 4+ ports, connect cue to ports 3-4 (if available)
    if playback_ports.len() >= 4 {
        let cue_left_port = format!("{}:{}", client_name, CUE_LEFT);
        let cue_right_port = format!("{}:{}", client_name, CUE_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&cue_left_port, &playback_ports[2]) {
            eprintln!("Warning: Could not connect cue left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&cue_right_port, &playback_ports[3]) {
            eprintln!("Warning: Could not connect cue right: {}", e);
        }

        println!(
            "Connected cue outputs to {} and {}",
            playback_ports[2], playback_ports[3]
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shared_state_creation() {
        let state = SharedState::new();
        assert_eq!(state.buffer_size, 256);
    }
}
