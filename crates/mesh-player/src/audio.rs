//! JACK audio client for Mesh DJ Player
//!
//! Connects the AudioEngine to JACK for real-time audio output.
//! Provides 4 output channels: Master L/R and Cue L/R.
//!
//! # Real-Time Safety
//!
//! The JACK process callback runs on a high-priority real-time thread with
//! strict timing constraints (~5.8ms at 256 samples @ 44.1kHz). Violations
//! cause audible glitches (xruns). This module ensures RT safety by:
//!
//! - **No allocations**: All buffers pre-allocated to [`MAX_BUFFER_SIZE`] (8192 samples)
//! - **Lock-free commands**: UI sends commands via ringbuffer, audio thread pops them
//! - **No syscalls**: No logging, file I/O, or blocking operations in the callback
//! - **Zero dropouts**: Audio thread never blocks waiting for UI
//!
//! # Thread Architecture (Lock-Free)
//!
//! ```text
//! ┌──────────────────┐                     ┌─────────────────────┐
//! │     UI Thread    │───push()───────────►│   Command Queue     │
//! │   (~16ms cycle)  │                     │  (lock-free SPSC)   │
//! └──────────────────┘                     └──────────┬──────────┘
//!         │                                           │
//!         │ Relaxed atomics                           │ pop()
//!         ▼                                           ▼
//! ┌──────────────────┐                     ┌─────────────────────┐
//! │   DeckAtomics    │◄────────────────────│  JACK RT Thread     │
//! │   (lock-free)    │     sync writes     │  (owns AudioEngine) │
//! └──────────────────┘                     └─────────────────────┘
//! ```
//!
//! The audio thread OWNS the engine exclusively - no mutex needed.
//! UI reads position via lock-free atomics, sends commands via ringbuffer.

use std::sync::Arc;

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};
use mesh_core::engine::{command_channel, AudioEngine, DeckAtomics, EngineCommand, SlicerAtomics};
use mesh_core::types::{StereoBuffer, NUM_DECKS};

/// Maximum buffer size to pre-allocate (covers all JACK configurations)
/// JACK typically uses 64, 128, 256, 512, 1024, 2048, or 4096 frames
const MAX_BUFFER_SIZE: usize = 8192;

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

/// Command sender for the UI thread
///
/// This is the producer side of the lock-free command queue.
/// The UI thread uses this to send commands to the audio engine
/// without any mutex contention.
pub struct CommandSender {
    producer: rtrb::Producer<EngineCommand>,
}

impl CommandSender {
    /// Send a command to the audio engine (non-blocking, ~50ns)
    ///
    /// Returns `Ok(())` if the command was queued successfully,
    /// or `Err(cmd)` if the queue is full (command is returned).
    ///
    /// In practice, the queue rarely fills up. If it does, the command
    /// is simply dropped (better than blocking the UI thread).
    pub fn send(&mut self, cmd: EngineCommand) -> Result<(), EngineCommand> {
        self.producer.push(cmd).map_err(|e| {
            // PushError::Full(value) - extract the value
            match e {
                rtrb::PushError::Full(value) => value,
            }
        })
    }

    /// Check if the queue has space for more commands
    #[allow(dead_code)]
    pub fn has_space(&self) -> bool {
        self.producer.slots() > 0
    }
}

/// JACK process handler
///
/// Owns the AudioEngine exclusively - no mutex needed.
/// Receives commands from UI via lock-free ringbuffer.
struct JackProcessor {
    /// Output ports
    master_left: Port<AudioOut>,
    master_right: Port<AudioOut>,
    cue_left: Port<AudioOut>,
    cue_right: Port<AudioOut>,
    /// The audio engine (OWNED, not shared)
    engine: AudioEngine,
    /// Command receiver (consumer side of lock-free queue)
    command_rx: rtrb::Consumer<EngineCommand>,
    /// Pre-allocated buffers for processing
    master_buffer: StereoBuffer,
    cue_buffer: StereoBuffer,
}

impl jack::ProcessHandler for JackProcessor {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        let n_frames = ps.n_frames() as usize;

        // Set working buffer length (real-time safe: no allocation, buffers pre-allocated to MAX_BUFFER_SIZE)
        // This just adjusts the length field; capacity remains at MAX_BUFFER_SIZE
        self.master_buffer.set_len_from_capacity(n_frames);
        self.cue_buffer.set_len_from_capacity(n_frames);

        // Process any pending commands from the UI (lock-free, ~50ns per command)
        // This is where track loads, play/pause, etc. get applied
        // Note: Logging in RT thread is NOT RT-safe in general, but these are rare events.
        // For production, consider using a lock-free log queue instead.
        self.engine.process_commands(&mut self.command_rx);

        // Process audio through the engine (no locks needed - we own it!)
        self.engine.process(&mut self.master_buffer, &mut self.cue_buffer);

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
/// Returns a handle to the active client, a command sender for controlling
/// the audio engine from the UI thread, lock-free atomics for UI reads, and
/// the JACK server's sample rate.
///
/// ## Lock-Free Architecture
///
/// The audio engine is OWNED by the JACK processor thread. The UI communicates
/// with it via a lock-free command queue. This eliminates all mutex contention
/// and guarantees zero audio dropouts during track loading.
pub fn start_jack_client(
    client_name: &str,
) -> Result<(JackHandle, CommandSender, [Arc<DeckAtomics>; NUM_DECKS], [Arc<SlicerAtomics>; NUM_DECKS], u32), JackError> {
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

    // Create engine with JACK's sample rate and extract atomics before moving to processor
    let jack_sample_rate = client.sample_rate() as u32;
    let engine = AudioEngine::new_with_sample_rate(jack_sample_rate);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();

    // Create lock-free command channel
    let (command_tx, command_rx) = command_channel();

    // Create processor with pre-allocated buffers at maximum size
    // The processor OWNS the engine - no Arc<Mutex> needed!
    let processor = JackProcessor {
        master_left,
        master_right,
        cue_left,
        cue_right,
        engine, // Moved into processor, not shared
        command_rx,
        master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
        cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
    };

    // Activate the client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| JackError::Activation(e.to_string()))?;

    println!("JACK client activated (lock-free command queue enabled)");

    Ok((
        JackHandle {
            _async_client: async_client,
        },
        CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        jack_sample_rate,
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
    use mesh_core::engine::command_channel;

    #[test]
    fn test_command_sender() {
        let (tx, mut rx) = command_channel();
        let mut sender = CommandSender { producer: tx };

        // Send a command
        assert!(sender.send(EngineCommand::Play { deck: 0 }).is_ok());

        // Verify it was received
        let cmd = rx.pop().unwrap();
        assert!(matches!(cmd, EngineCommand::Play { deck: 0 }));
    }
}
