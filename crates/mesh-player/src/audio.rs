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
use crate::config::JackPortConfig;
use mesh_core::db::DatabaseService;
use mesh_core::engine::{command_channel, AudioEngine, DeckAtomics, EngineCommand, LinkedStemAtomics, SlicerAtomics};
use mesh_core::loader::LinkedStemResultReceiver;
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
/// Result type for start_jack_client
pub type JackClientResult = (
    JackHandle,
    CommandSender,
    [Arc<DeckAtomics>; NUM_DECKS],
    [Arc<SlicerAtomics>; NUM_DECKS],
    [Arc<LinkedStemAtomics>; NUM_DECKS],
    LinkedStemResultReceiver,
    u32,
);

pub fn start_jack_client(
    client_name: &str,
    db_service: Arc<DatabaseService>,
) -> Result<JackClientResult, JackError> {
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
    let engine = AudioEngine::new_with_sample_rate(jack_sample_rate, db_service);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();
    let linked_stem_atomics = engine.linked_stem_atomics();
    // Get linked stem result receiver before engine is moved to processor
    let linked_stem_receiver = engine.linked_stem_result_receiver();

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
        linked_stem_atomics,
        linked_stem_receiver,
        jack_sample_rate,
    ))
}

/// Stereo output pair (L/R port names)
///
/// Represents a pair of JACK ports that together form a stereo output.
/// Used for displaying available audio outputs in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StereoPair {
    /// Human-readable label (e.g., "Outputs 1-2")
    pub label: String,
    /// Left channel port name (e.g., "system:playback_1")
    pub left: String,
    /// Right channel port name (e.g., "system:playback_2")
    pub right: String,
}

impl std::fmt::Display for StereoPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

/// Get available JACK stereo output pairs for UI dropdown
///
/// Queries the JACK server for available playback ports and groups them
/// into stereo pairs. Supports both traditional JACK naming (playback_1, playback_2)
/// and PipeWire surround naming (playback_FL, playback_FR, playback_RL, playback_RR).
/// Returns an empty list if JACK is not running.
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    // Create a temporary client to query ports
    let (client, _) = match Client::new("mesh_port_query", ClientOptions::NO_START_SERVER) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    // Get ALL playback ports (inputs to audio devices) - not just "system:"
    // PipeWire names ports after the actual device, e.g., "DDJ-SB2 Analog Surround 4.0:playback_FL"
    let ports = client.ports(
        Some(".*:playback_.*"),
        None,
        jack::PortFlags::IS_INPUT,
    );

    // Group ports by device (everything before the last colon)
    let mut devices: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for port in ports {
        if let Some(colon_pos) = port.rfind(':') {
            let device = port[..colon_pos].to_string();
            devices.entry(device).or_default().push(port);
        }
    }

    let mut pairs = Vec::new();

    // Process each device
    for (device_name, mut device_ports) in devices {
        // Sort ports to ensure consistent ordering
        device_ports.sort();

        // Try to find stereo pairs using PipeWire surround naming (FL/FR, RL/RR)
        let fl = device_ports.iter().find(|p| p.ends_with("_FL")).cloned();
        let fr = device_ports.iter().find(|p| p.ends_with("_FR")).cloned();
        let rl = device_ports.iter().find(|p| p.ends_with("_RL")).cloned();
        let rr = device_ports.iter().find(|p| p.ends_with("_RR")).cloned();

        // Shorten device name for display
        let short_name = device_name.split(' ').next().unwrap_or(&device_name);

        if fl.is_some() && fr.is_some() {
            // PipeWire surround naming - Front pair
            pairs.push(StereoPair {
                label: format!("{} Front", short_name),
                left: fl.unwrap(),
                right: fr.unwrap(),
            });

            // Rear pair if available
            if rl.is_some() && rr.is_some() {
                pairs.push(StereoPair {
                    label: format!("{} Rear", short_name),
                    left: rl.unwrap(),
                    right: rr.unwrap(),
                });
            }
        } else {
            // Traditional JACK numbered naming - group consecutive pairs
            device_ports
                .chunks(2)
                .enumerate()
                .filter(|(_, chunk)| chunk.len() == 2)
                .for_each(|(i, chunk)| {
                    pairs.push(StereoPair {
                        label: format!("{} {}-{}", short_name, i * 2 + 1, i * 2 + 2),
                        left: chunk[0].clone(),
                        right: chunk[1].clone(),
                    });
                });
        }
    }

    // Sort pairs by label for consistent UI ordering
    pairs.sort_by(|a, b| a.label.cmp(&b.label));
    pairs
}

/// Connect JACK ports using configuration
///
/// Connects mesh-player's master and cue outputs to the specified stereo pairs.
/// Falls back to auto-detection if no configuration is provided.
pub fn connect_ports(client_name: &str, config: &JackPortConfig) -> Result<(), JackError> {
    // Create a temporary client just for connecting
    let (client, _) = Client::new(&format!("{}_connect", client_name), ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    // Get available stereo pairs
    let pairs = get_available_stereo_pairs();
    if pairs.is_empty() {
        eprintln!("Warning: No JACK playback ports found");
        return Ok(());
    }

    // Determine master pair index (default to first pair)
    let master_idx = config.master_pair.unwrap_or(0);

    // Determine cue pair index (default to second pair, or same as master if only one pair)
    let cue_idx = config.cue_pair.unwrap_or_else(|| {
        if pairs.len() >= 2 { 1 } else { 0 }
    });

    // Connect master outputs
    if let Some(master) = pairs.get(master_idx) {
        let master_left_port = format!("{}:{}", client_name, MASTER_LEFT);
        let master_right_port = format!("{}:{}", client_name, MASTER_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&master_left_port, &master.left) {
            eprintln!("Warning: Could not connect master left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&master_right_port, &master.right) {
            eprintln!("Warning: Could not connect master right: {}", e);
        }

        println!("Connected master outputs to {} and {}", master.left, master.right);
    }

    // Connect cue outputs
    if let Some(cue) = pairs.get(cue_idx) {
        let cue_left_port = format!("{}:{}", client_name, CUE_LEFT);
        let cue_right_port = format!("{}:{}", client_name, CUE_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&cue_left_port, &cue.left) {
            eprintln!("Warning: Could not connect cue left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&cue_right_port, &cue.right) {
            eprintln!("Warning: Could not connect cue right: {}", e);
        }

        println!("Connected cue outputs to {} and {}", cue.left, cue.right);
    }

    Ok(())
}

/// Try to auto-connect to system playback ports (legacy function)
///
/// Deprecated: Use `connect_ports()` with `JackPortConfig` instead.

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
