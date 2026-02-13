//! Native JACK audio backend for Linux
//!
//! Provides direct JACK integration with full port-level routing control.
//! This backend is used on Linux when the `jack-backend` feature is enabled.
//!
//! # Features
//!
//! - **Port enumeration**: See all JACK ports (e.g., "system:playback_1-2")
//! - **Flexible routing**: Route master and cue to different port pairs
//! - **Lock-free design**: Same architecture as CPAL backend
//! - **Pro-audio support**: Works with PipeWire's JACK compatibility layer
//!
//! # Architecture
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

use std::sync::Arc;

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};

use super::backend::{AudioHandle, AudioSystemResult, CommandSender, StereoPair};
use super::config::AudioConfig;
use super::error::{AudioError, AudioResult};
use crate::db::DatabaseService;
use crate::engine::{command_channel, AudioEngine, EngineCommand};
use crate::types::StereoBuffer;

/// Maximum buffer size to pre-allocate (covers all JACK configurations)
const MAX_BUFFER_SIZE: usize = 8192;

/// JACK output port names
const MASTER_LEFT: &str = "master_left";
const MASTER_RIGHT: &str = "master_right";
const CUE_LEFT: &str = "cue_left";
const CUE_RIGHT: &str = "cue_right";

/// JACK-specific audio handle
///
/// Keeps the JACK client active. Drop this to disconnect from JACK.
pub struct JackAudioHandle {
    /// The async client (keeps JACK running)
    _async_client: jack::AsyncClient<JackNotifications, JackProcessor>,
    /// Sample rate from JACK server
    sample_rate: u32,
    /// Buffer size from JACK server
    buffer_size: u32,
}

impl JackAudioHandle {
    /// Get the sample rate of the audio system
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the actual buffer size in frames
    pub fn buffer_size(&self) -> u32 {
        self.buffer_size
    }

    /// Get the audio latency in milliseconds
    pub fn latency_ms(&self) -> f32 {
        (self.buffer_size as f32 / self.sample_rate as f32) * 1000.0
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

        // Set working buffer length (RT-safe: no allocation)
        self.master_buffer.set_len_from_capacity(n_frames);
        self.cue_buffer.set_len_from_capacity(n_frames);

        // Process commands from UI (lock-free, ~50ns per command)
        self.engine.process_commands(&mut self.command_rx);

        // Process audio through the engine
        self.engine
            .process(&mut self.master_buffer, &mut self.cue_buffer);

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
    fn sample_rate(&mut self, _client: &Client, srate: jack::Frames) -> Control {
        log::info!("JACK sample rate changed to: {}", srate);
        Control::Continue
    }

    fn xrun(&mut self, _client: &Client) -> Control {
        log::warn!("JACK xrun detected");
        Control::Continue
    }
}

/// Start the JACK audio system
///
/// Creates a JACK client, registers ports, and starts processing.
/// Returns handles for UI communication and the JACK sample rate.
pub fn start_audio_system(
    config: &AudioConfig,
    db_service: Arc<DatabaseService>,
) -> AudioResult<AudioSystemResult> {
    // Create JACK client (JACK may rename if another client has the same name)
    let (client, _status) = Client::new(&config.client_name, ClientOptions::NO_START_SERVER)
        .map_err(|e| AudioError::ConfigError(format!("Failed to create JACK client: {}", e)))?;
    let actual_client_name = client.name().to_string();

    let sample_rate = client.sample_rate() as u32;
    let buffer_size = client.buffer_size();

    log::info!(
        "JACK client '{}' created (sample rate: {}Hz, buffer: {} frames, latency: {:.1}ms)",
        actual_client_name,
        sample_rate,
        buffer_size,
        (buffer_size as f32 / sample_rate as f32) * 1000.0
    );

    // Register output ports
    let master_left = client
        .register_port(MASTER_LEFT, AudioOut::default())
        .map_err(|e| AudioError::ConfigError(format!("Failed to register port: {}", e)))?;

    let master_right = client
        .register_port(MASTER_RIGHT, AudioOut::default())
        .map_err(|e| AudioError::ConfigError(format!("Failed to register port: {}", e)))?;

    let cue_left = client
        .register_port(CUE_LEFT, AudioOut::default())
        .map_err(|e| AudioError::ConfigError(format!("Failed to register port: {}", e)))?;

    let cue_right = client
        .register_port(CUE_RIGHT, AudioOut::default())
        .map_err(|e| AudioError::ConfigError(format!("Failed to register port: {}", e)))?;

    // Create engine with JACK's sample rate
    let engine = AudioEngine::new_with_sample_rate(sample_rate, db_service);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();
    let linked_stem_atomics = engine.linked_stem_atomics();
    let linked_stem_receiver = engine.linked_stem_result_receiver();
    let clip_indicator = engine.clip_indicator();

    // Create lock-free command channel
    let (command_tx, command_rx) = command_channel();

    // Create processor with pre-allocated buffers
    let processor = JackProcessor {
        master_left,
        master_right,
        cue_left,
        cue_right,
        engine,
        command_rx,
        master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
        cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
    };

    // Activate the client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| AudioError::ConfigError(format!("Failed to activate JACK client: {}", e)))?;

    log::info!("JACK client activated");

    // Auto-connect to system ports based on config
    let pairs = get_available_stereo_pairs();
    if !pairs.is_empty() {
        let master_idx = config.master_pair_index.unwrap_or(0);
        let cue_idx = config.cue_pair_index.unwrap_or_else(|| {
            if pairs.len() >= 2 {
                1
            } else {
                0
            }
        });

        if let Err(e) = connect_ports(&actual_client_name, Some(master_idx), Some(cue_idx)) {
            log::warn!("Auto-connect failed: {}", e);
        }
    }

    let latency_ms = (buffer_size as f32 / sample_rate as f32) * 1000.0;

    let handle = JackAudioHandle {
        _async_client: async_client,
        sample_rate,
        buffer_size,
    };

    Ok(AudioSystemResult {
        client_name: actual_client_name,
        handle: AudioHandle::Jack(handle),
        command_sender: CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        linked_stem_atomics,
        linked_stem_receiver,
        clip_indicator,
        sample_rate,
        buffer_size,
        latency_ms,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Port Enumeration
// ═══════════════════════════════════════════════════════════════════════════════

/// Get available JACK stereo output pairs for UI dropdown
///
/// Queries the JACK server for available playback ports and groups them
/// into stereo pairs. Supports both traditional JACK naming (playback_1, playback_2)
/// and PipeWire surround naming (playback_FL, playback_FR, playback_RL, playback_RR).
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    // Create a temporary client to query ports
    let (client, _) = match Client::new("mesh_port_query", ClientOptions::NO_START_SERVER) {
        Ok(c) => c,
        Err(e) => {
            log::debug!("Could not connect to JACK to enumerate ports: {}", e);
            return vec![];
        }
    };

    // Get ALL playback ports (inputs to audio devices)
    let ports = client.ports(Some(".*:playback_.*"), None, jack::PortFlags::IS_INPUT);

    // Group ports by device (everything before the last colon)
    let mut devices: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for port in ports {
        if let Some(colon_pos) = port.rfind(':') {
            let device = port[..colon_pos].to_string();
            devices.entry(device).or_default().push(port);
        }
    }

    let mut pairs = Vec::new();

    // Process each device
    for (device_name, mut device_ports) in devices {
        device_ports.sort();

        // Try PipeWire surround naming (FL/FR, RL/RR)
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
            // Traditional JACK numbered naming
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

    pairs.sort_by(|a, b| a.label.cmp(&b.label));
    log::debug!("Found {} JACK stereo pairs", pairs.len());
    pairs
}

/// Connect JACK ports to specified stereo pairs
///
/// Connects mesh-player's master and cue outputs to the specified port pairs.
pub fn connect_ports(
    client_name: &str,
    master_pair: Option<usize>,
    cue_pair: Option<usize>,
) -> AudioResult<()> {
    let pairs = get_available_stereo_pairs();
    if pairs.is_empty() {
        log::warn!("No JACK playback ports found for connection");
        return Ok(());
    }

    // Create a temporary client for connecting
    let (client, _) = Client::new(
        &format!("{}_connect", client_name),
        ClientOptions::NO_START_SERVER,
    )
    .map_err(|e| AudioError::ConfigError(format!("Failed to create JACK client: {}", e)))?;

    let master_idx = master_pair.unwrap_or(0);
    let cue_idx = cue_pair.unwrap_or_else(|| if pairs.len() >= 2 { 1 } else { 0 });

    // Connect master outputs
    if let Some(master) = pairs.get(master_idx) {
        let master_left_port = format!("{}:{}", client_name, MASTER_LEFT);
        let master_right_port = format!("{}:{}", client_name, MASTER_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&master_left_port, &master.left) {
            log::warn!("Could not connect master left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&master_right_port, &master.right) {
            log::warn!("Could not connect master right: {}", e);
        }

        log::info!(
            "Connected master to {} and {}",
            master.left,
            master.right
        );
    }

    // Connect cue outputs
    if let Some(cue) = pairs.get(cue_idx) {
        let cue_left_port = format!("{}:{}", client_name, CUE_LEFT);
        let cue_right_port = format!("{}:{}", client_name, CUE_RIGHT);

        if let Err(e) = client.connect_ports_by_name(&cue_left_port, &cue.left) {
            log::warn!("Could not connect cue left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&cue_right_port, &cue.right) {
            log::warn!("Could not connect cue right: {}", e);
        }

        log::info!("Connected cue to {} and {}", cue.left, cue.right);
    }

    Ok(())
}

/// Reconnect audio outputs to different stereo pairs (hot-swap)
///
/// Disconnects existing connections first, then connects to new pairs.
/// This allows changing output routing without restarting the audio system.
pub fn reconnect_ports(
    client_name: &str,
    master_pair: Option<usize>,
    cue_pair: Option<usize>,
) -> AudioResult<()> {
    let pairs = get_available_stereo_pairs();
    if pairs.is_empty() {
        log::warn!("No JACK playback ports found for reconnection");
        return Ok(());
    }

    // Create a temporary client for port management
    let (client, _) = Client::new(
        &format!("{}_reconnect", client_name),
        ClientOptions::NO_START_SERVER,
    )
    .map_err(|e| AudioError::ConfigError(format!("Failed to create JACK client: {}", e)))?;

    // Our port names
    let master_left_port = format!("{}:{}", client_name, MASTER_LEFT);
    let master_right_port = format!("{}:{}", client_name, MASTER_RIGHT);
    let cue_left_port = format!("{}:{}", client_name, CUE_LEFT);
    let cue_right_port = format!("{}:{}", client_name, CUE_RIGHT);

    // Disconnect all existing connections from our ports
    // Try disconnecting from all system input ports (JACK ignores if not connected)
    let input_ports = client.ports(None, None, jack::PortFlags::IS_INPUT);
    for our_port in [&master_left_port, &master_right_port, &cue_left_port, &cue_right_port] {
        for target_port in &input_ports {
            // Try to disconnect (ignore errors - port might not be connected)
            let _ = client.disconnect_ports_by_name(our_port, target_port);
        }
    }

    log::info!("Disconnected existing JACK port connections");

    // Now connect to new pairs
    let master_idx = master_pair.unwrap_or(0);
    let cue_idx = cue_pair.unwrap_or_else(|| if pairs.len() >= 2 { 1 } else { 0 });

    // Connect master outputs
    if let Some(master) = pairs.get(master_idx) {
        if let Err(e) = client.connect_ports_by_name(&master_left_port, &master.left) {
            log::warn!("Could not connect master left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&master_right_port, &master.right) {
            log::warn!("Could not connect master right: {}", e);
        }
        log::info!("Reconnected master to {} and {}", master.left, master.right);
    }

    // Connect cue outputs
    if let Some(cue) = pairs.get(cue_idx) {
        if let Err(e) = client.connect_ports_by_name(&cue_left_port, &cue.left) {
            log::warn!("Could not connect cue left: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name(&cue_right_port, &cue.right) {
            log::warn!("Could not connect cue right: {}", e);
        }
        log::info!("Reconnected cue to {} and {}", cue.left, cue.right);
    }

    Ok(())
}
