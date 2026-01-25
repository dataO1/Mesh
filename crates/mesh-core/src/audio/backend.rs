//! Audio backend trait for platform-specific implementations
//!
//! Defines a common interface for audio backends:
//! - **Linux**: Native JACK for pro-audio with port-level routing
//! - **Windows/macOS**: CPAL for cross-platform device support
//!
//! Both backends use the same lock-free architecture:
//! - UI sends commands via ringbuffer
//! - Audio thread owns the AudioEngine exclusively
//! - Atomics for lock-free state reads

use std::sync::Arc;

use crate::db::DatabaseService;
use crate::engine::{DeckAtomics, LinkedStemAtomics, SlicerAtomics};
use crate::loader::LinkedStemResultReceiver;
use crate::types::NUM_DECKS;

use super::config::AudioConfig;
use super::error::AudioResult;

/// Stereo output pair for audio routing
///
/// Represents a pair of channels that form a stereo output.
/// - On JACK: Specific port names like "system:playback_1" and "system:playback_2"
/// - On CPAL: Device identifier (left/right are implicit)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StereoPair {
    /// Human-readable label (e.g., "Outputs 1-2" or "[ALSA] hw:0,0")
    pub label: String,
    /// Left channel identifier (port name for JACK, device ID for CPAL)
    pub left: String,
    /// Right channel identifier (port name for JACK, same as left for CPAL)
    pub right: String,
}

impl std::fmt::Display for StereoPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label)
    }
}

/// Result of starting the audio system
///
/// Contains all the handles and communication channels needed by the UI.
pub struct AudioSystemResult {
    /// Handle to keep audio alive (drop to stop)
    pub handle: AudioHandle,
    /// Command sender for UI thread (lock-free)
    pub command_sender: CommandSender,
    /// Deck atomics for lock-free UI reads
    pub deck_atomics: [Arc<DeckAtomics>; NUM_DECKS],
    /// Slicer atomics for lock-free UI reads
    pub slicer_atomics: [Arc<SlicerAtomics>; NUM_DECKS],
    /// Linked stem atomics for lock-free UI reads
    pub linked_stem_atomics: [Arc<LinkedStemAtomics>; NUM_DECKS],
    /// Receiver for linked stem load results
    pub linked_stem_receiver: LinkedStemResultReceiver,
    /// Sample rate of the audio system
    pub sample_rate: u32,
    /// Actual buffer size in frames
    pub buffer_size: u32,
    /// Audio latency in milliseconds (one-way, output only)
    pub latency_ms: f32,
}

/// Handle to the active audio system
///
/// Keeps the audio streams/client alive. Drop this to stop audio.
pub enum AudioHandle {
    /// CPAL-based handle (Windows/macOS/Linux fallback)
    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    Cpal(super::cpal_backend::CpalAudioHandle),

    /// Native JACK handle (Linux with jack-backend feature)
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    Jack(super::jack_backend::JackAudioHandle),
}

impl AudioHandle {
    /// Get the sample rate of the audio system
    pub fn sample_rate(&self) -> u32 {
        match self {
            #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
            AudioHandle::Cpal(h) => h.sample_rate(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.sample_rate(),
        }
    }

    /// Get the actual buffer size in frames
    pub fn buffer_size(&self) -> u32 {
        match self {
            #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
            AudioHandle::Cpal(h) => h.buffer_size(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.buffer_size(),
        }
    }

    /// Get the audio latency in milliseconds
    pub fn latency_ms(&self) -> f32 {
        match self {
            #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
            AudioHandle::Cpal(h) => h.latency_ms(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.latency_ms(),
        }
    }
}

/// Command sender for the UI thread
///
/// Wraps the lock-free producer for sending EngineCommand to the audio thread.
/// All operations are non-blocking (~50ns per command).
pub struct CommandSender {
    pub(crate) producer: rtrb::Producer<crate::engine::EngineCommand>,
}

impl CommandSender {
    /// Send a command to the audio engine (non-blocking, ~50ns)
    ///
    /// Returns `Ok(())` if the command was queued successfully,
    /// or `Err(cmd)` if the queue is full (command is returned).
    pub fn send(
        &mut self,
        cmd: crate::engine::EngineCommand,
    ) -> Result<(), crate::engine::EngineCommand> {
        self.producer.push(cmd).map_err(|e| match e {
            rtrb::PushError::Full(value) => value,
        })
    }

    /// Check if the queue has space for more commands
    #[allow(dead_code)]
    pub fn has_space(&self) -> bool {
        self.producer.slots() > 0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Platform-specific audio system startup
// ═══════════════════════════════════════════════════════════════════════════════

/// Start the audio system with the given configuration
///
/// Automatically selects the appropriate backend:
/// - **Linux with jack-backend feature**: Native JACK for pro-audio routing
/// - **Other platforms**: CPAL for cross-platform support
///
/// # Arguments
/// * `config` - Audio configuration specifying output mode and devices
/// * `db_service` - Database service for the audio engine
///
/// # Returns
/// * `AudioSystemResult` containing handles, command sender, and atomics
pub fn start_audio_system(
    config: &AudioConfig,
    db_service: Arc<DatabaseService>,
) -> AudioResult<AudioSystemResult> {
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        super::jack_backend::start_audio_system(config, db_service)
    }

    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    {
        super::cpal_backend::start_audio_system(config, db_service)
    }
}

/// Get available stereo output pairs for UI dropdown
///
/// On JACK: Returns actual port pairs (e.g., "Scarlett 1-2", "Scarlett 3-4")
/// On CPAL: Returns devices as pseudo-pairs
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        super::jack_backend::get_available_stereo_pairs()
    }

    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    {
        super::cpal_backend::get_available_stereo_pairs()
    }
}

/// Connect audio outputs to specified stereo pairs
///
/// On JACK: Connects client ports to system playback ports
/// On CPAL: No-op (device selection happens at stream creation)
pub fn connect_ports(
    _client_name: &str,
    _master_pair: Option<usize>,
    _cue_pair: Option<usize>,
) -> AudioResult<()> {
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        super::jack_backend::connect_ports(_client_name, _master_pair, _cue_pair)
    }

    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    {
        // CPAL handles device selection at stream creation time
        Ok(())
    }
}

/// Reconnect audio outputs to different stereo pairs (hot-swap)
///
/// On JACK: Disconnects existing connections and reconnects to new pairs
/// On CPAL: Returns false (requires app restart for device changes)
///
/// Returns true if hot-swap succeeded, false if restart is required.
pub fn reconnect_ports(
    _client_name: &str,
    _master_pair: Option<usize>,
    _cue_pair: Option<usize>,
) -> bool {
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        match super::jack_backend::reconnect_ports(_client_name, _master_pair, _cue_pair) {
            Ok(()) => true,
            Err(e) => {
                log::error!("Failed to reconnect JACK ports: {}", e);
                false
            }
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    {
        // CPAL doesn't support hot-swapping devices
        false
    }
}
