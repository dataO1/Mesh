//! JACK audio playback for mesh-cue
//!
//! Lock-free architecture for RT-safe audio preview:
//! - Commands sent via `rtrb` SPSC ringbuffer (UI → Audio)
//! - State read via atomics (Audio → UI)
//! - Audio thread owns the stems data exclusively

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use basedrop::Shared;
use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};
use mesh_core::audio_file::StemBuffers;

/// Commands sent from UI to audio thread
pub enum PreviewCommand {
    /// Load new stems for playback
    LoadStems(Box<Shared<StemBuffers>>, u64), // stems, length
    /// Unload current stems
    UnloadStems,
    /// Start playback
    Play,
    /// Pause playback
    Pause,
    /// Seek to position (samples)
    Seek(u64),
}

/// Create a preview command channel
///
/// Returns (sender, receiver) pair with 64-command capacity
pub fn preview_command_channel() -> (rtrb::Producer<PreviewCommand>, rtrb::Consumer<PreviewCommand>)
{
    rtrb::RingBuffer::new(64)
}

/// Command sender for UI thread
pub struct CommandSender {
    producer: rtrb::Producer<PreviewCommand>,
}

impl CommandSender {
    /// Send a command to the audio thread
    ///
    /// Returns Err if the queue is full (command dropped)
    pub fn send(&mut self, cmd: PreviewCommand) -> Result<(), PreviewCommand> {
        self.producer.push(cmd).map_err(|e| match e {
            rtrb::PushError::Full(value) => value,
        })
    }

    /// Check if there's space in the queue
    #[allow(dead_code)]
    pub fn has_space(&self) -> bool {
        self.producer.slots() > 0
    }
}

/// Lock-free atomics for UI to read audio state
pub struct PreviewAtomics {
    /// Current playback position in samples
    pub position: AtomicU64,
    /// Whether audio is currently playing
    pub playing: AtomicBool,
    /// Total track length in samples
    pub length: AtomicU64,
}

impl PreviewAtomics {
    fn new() -> Self {
        Self {
            position: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            length: AtomicU64::new(0),
        }
    }

    /// Get current playback position
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Get track length
    pub fn length(&self) -> u64 {
        self.length.load(Ordering::Relaxed)
    }
}

/// Audio playback state for UI interaction
///
/// Holds command sender and atomic state references
pub struct AudioState {
    /// Command sender for audio thread (None if disconnected)
    command_sender: Option<CommandSender>,
    /// Atomics for reading current state
    atomics: Arc<PreviewAtomics>,
}

impl AudioState {
    /// Create new audio state with command channel
    fn new(
        producer: rtrb::Producer<PreviewCommand>,
        atomics: Arc<PreviewAtomics>,
    ) -> Self {
        Self {
            command_sender: Some(CommandSender { producer }),
            atomics,
        }
    }

    /// Create a disconnected audio state (for when JACK is unavailable)
    pub fn disconnected() -> Self {
        Self {
            command_sender: None,
            atomics: Arc::new(PreviewAtomics::new()),
        }
    }

    /// Get current playback position
    pub fn position(&self) -> u64 {
        self.atomics.position()
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        self.atomics.is_playing()
    }

    /// Get track length
    #[allow(dead_code)]
    pub fn length(&self) -> u64 {
        self.atomics.length()
    }

    /// Seek to position
    pub fn seek(&mut self, position: u64) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(PreviewCommand::Seek(position));
        }
    }

    /// Start playback
    pub fn play(&mut self) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(PreviewCommand::Play);
        }
    }

    /// Pause playback
    pub fn pause(&mut self) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(PreviewCommand::Pause);
        }
    }

    /// Toggle play/pause
    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Set the current track for playback
    pub fn set_track(&mut self, stems: Shared<StemBuffers>, length: u64) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(PreviewCommand::LoadStems(Box::new(stems), length));
            log::info!("AudioState: Track load command sent, {} samples", length);
        }
    }

    /// Clear the current track
    #[allow(dead_code)]
    pub fn clear_track(&mut self) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(PreviewCommand::UnloadStems);
        }
    }
}

/// JACK process handler for audio preview
///
/// Owns all audio data exclusively - no sharing with UI thread
pub struct JackProcessor {
    /// Output ports
    left: Port<AudioOut>,
    right: Port<AudioOut>,
    /// Command receiver from UI
    command_rx: rtrb::Consumer<PreviewCommand>,
    /// Atomics for UI to read state
    atomics: Arc<PreviewAtomics>,
    /// Current stems (owned by audio thread)
    stems: Option<Shared<StemBuffers>>,
    /// Current playback position
    position: usize,
    /// Track length
    length: usize,
    /// Playing flag
    playing: bool,
}

impl JackProcessor {
    /// Process pending commands from UI
    fn process_commands(&mut self) {
        while let Ok(cmd) = self.command_rx.pop() {
            match cmd {
                PreviewCommand::LoadStems(stems, length) => {
                    self.stems = Some(*stems);
                    self.length = length as usize;
                    self.position = 0;
                    self.playing = false;
                    // Update atomics
                    self.atomics.length.store(length, Ordering::Relaxed);
                    self.atomics.position.store(0, Ordering::Relaxed);
                    self.atomics.playing.store(false, Ordering::Relaxed);
                }
                PreviewCommand::UnloadStems => {
                    self.stems = None;
                    self.length = 0;
                    self.position = 0;
                    self.playing = false;
                    self.atomics.length.store(0, Ordering::Relaxed);
                    self.atomics.position.store(0, Ordering::Relaxed);
                    self.atomics.playing.store(false, Ordering::Relaxed);
                }
                PreviewCommand::Play => {
                    if self.stems.is_some() && self.position < self.length {
                        self.playing = true;
                        self.atomics.playing.store(true, Ordering::Relaxed);
                    }
                }
                PreviewCommand::Pause => {
                    self.playing = false;
                    self.atomics.playing.store(false, Ordering::Relaxed);
                }
                PreviewCommand::Seek(pos) => {
                    self.position = (pos as usize).min(self.length);
                    self.atomics
                        .position
                        .store(self.position as u64, Ordering::Relaxed);
                }
            }
        }
    }
}

impl jack::ProcessHandler for JackProcessor {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        // Process commands first (lock-free)
        self.process_commands();

        let n_frames = ps.n_frames() as usize;
        let out_left = self.left.as_mut_slice(ps);
        let out_right = self.right.as_mut_slice(ps);

        // Check if playing and have stems
        if !self.playing {
            out_left.fill(0.0);
            out_right.fill(0.0);
            return Control::Continue;
        }

        let stems = match self.stems.as_ref() {
            Some(s) => s,
            None => {
                out_left.fill(0.0);
                out_right.fill(0.0);
                return Control::Continue;
            }
        };

        let len = stems.len();

        // Sum all 4 stems and output
        for i in 0..n_frames {
            let idx = self.position + i;
            if idx >= len {
                out_left[i] = 0.0;
                out_right[i] = 0.0;
            } else {
                // Sum all stems (vocals + drums + bass + other)
                let v = &stems.vocals[idx];
                let d = &stems.drums[idx];
                let b = &stems.bass[idx];
                let o = &stems.other[idx];
                out_left[i] = v.left + d.left + b.left + o.left;
                out_right[i] = v.right + d.right + b.right + o.right;
            }
        }

        // Advance position
        let new_pos = (self.position + n_frames).min(len);
        self.position = new_pos;
        self.atomics
            .position
            .store(new_pos as u64, Ordering::Relaxed);

        // Stop at end of track
        if new_pos >= len {
            self.playing = false;
            self.atomics.playing.store(false, Ordering::Relaxed);
        }

        Control::Continue
    }
}

/// JACK notification handler
struct JackNotifications;

impl jack::NotificationHandler for JackNotifications {
    unsafe fn shutdown(&mut self, _status: jack::ClientStatus, reason: &str) {
        log::warn!("JACK server shut down: {}", reason);
    }

    fn sample_rate(&mut self, _client: &Client, srate: jack::Frames) -> Control {
        log::info!("JACK sample rate: {}", srate);
        Control::Continue
    }

    fn xrun(&mut self, _client: &Client) -> Control {
        log::warn!("JACK xrun (audio dropout)");
        Control::Continue
    }
}

/// Error type for JACK operations
#[derive(Debug)]
pub enum JackError {
    /// Failed to create JACK client
    ClientCreation(String),
    /// Failed to register port
    PortRegistration(String),
    /// Failed to activate client
    Activation(String),
}

impl std::fmt::Display for JackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JackError::ClientCreation(msg) => write!(f, "Failed to create JACK client: {}", msg),
            JackError::PortRegistration(msg) => write!(f, "Failed to register port: {}", msg),
            JackError::Activation(msg) => write!(f, "Failed to activate client: {}", msg),
        }
    }
}

impl std::error::Error for JackError {}

/// Handle to the active JACK client
pub struct JackHandle {
    /// The async client (keeps JACK running until dropped)
    _async_client: jack::AsyncClient<JackNotifications, JackProcessor>,
}

/// Start the JACK audio client for preview playback
///
/// Returns the audio state for UI interaction and JACK handle to keep client alive.
/// Uses lock-free command queue - no Mutex in audio path.
pub fn start_jack_client() -> Result<(AudioState, JackHandle), JackError> {
    // Create JACK client (don't start server if not running)
    let (client, _status) = Client::new("mesh-cue", ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    log::info!(
        "JACK client 'mesh-cue' created (sample rate: {}, buffer size: {})",
        client.sample_rate(),
        client.buffer_size()
    );

    // Register stereo output ports
    let left = client
        .register_port("out_left", AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    let right = client
        .register_port("out_right", AudioOut::default())
        .map_err(|e| JackError::PortRegistration(e.to_string()))?;

    // Create lock-free command channel
    let (producer, consumer) = preview_command_channel();

    // Create shared atomics for state
    let atomics = Arc::new(PreviewAtomics::new());

    // Create processor with owned state
    let processor = JackProcessor {
        left,
        right,
        command_rx: consumer,
        atomics: atomics.clone(),
        stems: None,
        position: 0,
        length: 0,
        playing: false,
    };

    // Create audio state for UI
    let audio_state = AudioState::new(producer, atomics);

    // Activate the client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| JackError::Activation(e.to_string()))?;

    log::info!("JACK client activated - lock-free audio preview ready");

    // Try to auto-connect to system playback
    if let Err(e) = auto_connect_ports() {
        log::warn!("Could not auto-connect to system playback: {}", e);
    }

    Ok((
        audio_state,
        JackHandle {
            _async_client: async_client,
        },
    ))
}

/// Try to auto-connect mesh-cue outputs to system playback ports
fn auto_connect_ports() -> Result<(), JackError> {
    // Create a temporary client just for connecting
    let (client, _) = Client::new("mesh-cue_connect", ClientOptions::NO_START_SERVER)
        .map_err(|e| JackError::ClientCreation(e.to_string()))?;

    // Find system playback ports
    let playback_ports = client.ports(
        Some("system:playback_.*"),
        None,
        jack::PortFlags::IS_INPUT,
    );

    if playback_ports.len() >= 2 {
        // Connect outputs to system playback
        if let Err(e) = client.connect_ports_by_name("mesh-cue:out_left", &playback_ports[0]) {
            log::warn!("Could not connect left output: {}", e);
        }
        if let Err(e) = client.connect_ports_by_name("mesh-cue:out_right", &playback_ports[1]) {
            log::warn!("Could not connect right output: {}", e);
        }

        log::info!(
            "Connected outputs to {} and {}",
            playback_ports[0],
            playback_ports[1]
        );
    } else {
        log::warn!("No system playback ports found for auto-connect");
    }

    Ok(())
}
