//! JACK audio playback for mesh-cue
//!
//! Provides audio preview functionality for the collection editor.
//! Simpler than mesh-player: just plays back the loaded track with all stems summed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessScope};
use mesh_core::audio_file::StemBuffers;

/// Audio playback state shared between UI and audio thread
pub struct AudioState {
    /// Current playback position in samples
    pub position: Arc<AtomicU64>,
    /// Whether audio is currently playing
    pub playing: Arc<AtomicBool>,
    /// Total track length in samples
    pub length: u64,
    /// Current track stems for playback (shared with JACK thread)
    pub stems: Arc<Mutex<Option<Arc<StemBuffers>>>>,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            position: Arc::new(AtomicU64::new(0)),
            playing: Arc::new(AtomicBool::new(false)),
            length: 0,
            stems: Arc::new(Mutex::new(None)),
        }
    }
}

impl AudioState {
    /// Get current playback position
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// Set playback position (seek)
    pub fn seek(&self, position: u64) {
        self.position.store(position.min(self.length), Ordering::Relaxed);
    }

    /// Check if playing
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Start playback
    pub fn play(&self) {
        self.playing.store(true, Ordering::Relaxed);
    }

    /// Pause playback
    pub fn pause(&self) {
        self.playing.store(false, Ordering::Relaxed);
    }

    /// Toggle play/pause
    #[allow(dead_code)]
    pub fn toggle(&self) {
        let current = self.playing.load(Ordering::Relaxed);
        self.playing.store(!current, Ordering::Relaxed);
    }

    /// Set the current track for playback
    pub fn set_track(&mut self, stems: Arc<StemBuffers>, length: u64) {
        self.length = length;
        *self.stems.lock().unwrap() = Some(stems);
        self.seek(0);
        log::info!("AudioState: Track set, {} samples", length);
    }

    /// Clear the current track
    #[allow(dead_code)]
    pub fn clear_track(&mut self) {
        *self.stems.lock().unwrap() = None;
        self.length = 0;
        self.pause();
    }
}

/// JACK process handler for audio preview
pub struct JackProcessor {
    /// Output ports
    left: Port<AudioOut>,
    right: Port<AudioOut>,
    /// Playback position (shared with UI)
    position: Arc<AtomicU64>,
    /// Playing flag (shared with UI)
    playing: Arc<AtomicBool>,
    /// Track stems (shared with UI)
    stems: Arc<Mutex<Option<Arc<StemBuffers>>>>,
    /// Track length in samples
    length: Arc<AtomicU64>,
}

impl jack::ProcessHandler for JackProcessor {
    fn process(&mut self, _client: &Client, ps: &ProcessScope) -> Control {
        let n_frames = ps.n_frames() as usize;
        let out_left = self.left.as_mut_slice(ps);
        let out_right = self.right.as_mut_slice(ps);

        // Check if playing
        if !self.playing.load(Ordering::Relaxed) {
            // Output silence when paused
            out_left.fill(0.0);
            out_right.fill(0.0);
            return Control::Continue;
        }

        // Non-blocking lock to avoid priority inversion
        let stems_guard = match self.stems.try_lock() {
            Ok(g) => g,
            Err(_) => {
                out_left.fill(0.0);
                out_right.fill(0.0);
                return Control::Continue;
            }
        };

        let stems = match stems_guard.as_ref() {
            Some(s) => s,
            None => {
                out_left.fill(0.0);
                out_right.fill(0.0);
                return Control::Continue;
            }
        };

        let pos = self.position.load(Ordering::Relaxed) as usize;
        let len = stems.len();

        // Sum all 4 stems and output
        for i in 0..n_frames {
            let idx = pos + i;
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
        let new_pos = (pos + n_frames).min(len);
        self.position.store(new_pos as u64, Ordering::Relaxed);

        // Stop at end of track
        if new_pos >= len {
            self.playing.store(false, Ordering::Relaxed);
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
/// Connects to JACK and creates stereo output ports.
/// The audio state's position/playing/stems are shared with the process callback.
pub fn start_jack_client(audio_state: &AudioState) -> Result<JackHandle, JackError> {
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

    // Create processor with shared state
    let processor = JackProcessor {
        left,
        right,
        position: audio_state.position.clone(),
        playing: audio_state.playing.clone(),
        stems: audio_state.stems.clone(),
        length: Arc::new(AtomicU64::new(audio_state.length)),
    };

    // Activate the client
    let async_client = client
        .activate_async(JackNotifications, processor)
        .map_err(|e| JackError::Activation(e.to_string()))?;

    log::info!("JACK client activated - audio preview ready");

    // Try to auto-connect to system playback
    if let Err(e) = auto_connect_ports() {
        log::warn!("Could not auto-connect to system playback: {}", e);
    }

    Ok(JackHandle {
        _async_client: async_client,
    })
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
