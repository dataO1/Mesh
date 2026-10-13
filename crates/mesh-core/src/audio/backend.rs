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

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;

use crate::db::DatabaseService;
use crate::engine::{DeckAtomics, LevelAtomics, LinkedStemAtomics, SlicerAtomics};
use crate::engine::{command_channel, AudioEngine, EngineCommand};
use crate::loader::LinkedStemResultReceiver;
use crate::types::{StereoBuffer, StereoSample, NUM_DECKS};

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

/// Real-time engine driven by an audio backend
///
/// The backend calls `process_block` once per audio callback, copies the
/// buffers to its outputs, then pushes the master block into every producer
/// in `recording_producers` (set recorder). Implementations drain their own
/// command rings inside `process_block` and must be RT-safe.
pub trait RtEngine: Send {
    /// Drain pending commands and render one block into `master` and `cue`.
    ///
    /// Both buffers already have the callback's frame count as their length.
    fn process_block(&mut self, master: &mut StereoBuffer, cue: &mut StereoBuffer);

    /// Active set-recording producers fed with the master output.
    fn recording_producers(&mut self) -> &mut Vec<rtrb::Producer<StereoSample>>;

    /// Atomic the backend writes the measured output latency (samples) into.
    ///
    /// Default: a fresh atomic nobody reads.
    fn output_latency_samples(&self) -> Arc<AtomicU64> {
        Arc::new(AtomicU64::new(0))
    }
}

/// Push one master block into every recording producer, dropping abandoned ones
///
/// RT-safe: drops samples instead of blocking when a ring is full.
pub(crate) fn push_to_recording<E: RtEngine>(engine: &mut E, master: &[StereoSample]) {
    let producers = engine.recording_producers();
    if !producers.is_empty() {
        producers.retain_mut(|producer| {
            for &sample in master {
                if producer.push(sample).is_err() {
                    break; // Buffer full — drop rather than block
                }
            }
            !producer.is_abandoned()
        });
    }
}

/// DJ engine adapter: `AudioEngine` plus its UI and direct-MIDI command rings
pub(crate) struct DjRtEngine {
    engine: AudioEngine,
    /// Command receiver (consumer side of lock-free queue)
    command_rx: rtrb::Consumer<EngineCommand>,
    /// Direct command receiver for timing-critical MIDI commands
    direct_command_rx: rtrb::Consumer<EngineCommand>,
}

impl RtEngine for DjRtEngine {
    #[inline]
    fn process_block(&mut self, master: &mut StereoBuffer, cue: &mut StereoBuffer) {
        // Process commands from UI (lock-free, ~50ns per command)
        self.engine.process_commands(&mut self.command_rx);
        // Process direct MIDI commands (timing-critical, bypasses iced tick)
        self.engine.process_commands(&mut self.direct_command_rx);
        // Process audio through the engine
        self.engine.process(master, cue);
    }

    #[inline]
    fn recording_producers(&mut self) -> &mut Vec<rtrb::Producer<StereoSample>> {
        &mut self.engine.recording_producers
    }

    fn output_latency_samples(&self) -> Arc<AtomicU64> {
        self.engine.output_latency_samples()
    }
}

/// UI-side handles of the DJ engine, produced by its factory
struct DjHandles {
    command_sender: CommandSender,
    deck_atomics: [Arc<DeckAtomics>; NUM_DECKS],
    slicer_atomics: [Arc<SlicerAtomics>; NUM_DECKS],
    linked_stem_atomics: [Arc<LinkedStemAtomics>; NUM_DECKS],
    linked_stem_receiver: LinkedStemResultReceiver,
    clip_indicator: Arc<AtomicBool>,
    level_atomics: Arc<LevelAtomics>,
    internal_latency_samples: Arc<AtomicU32>,
    direct_command_producer: rtrb::Producer<EngineCommand>,
}

/// Build the DJ engine at the device sample rate
fn make_dj_engine(sample_rate: u32, db_service: Arc<DatabaseService>) -> (DjRtEngine, DjHandles) {
    let engine = AudioEngine::new_with_sample_rate(sample_rate, db_service);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();
    let linked_stem_atomics = engine.linked_stem_atomics();
    let linked_stem_receiver = engine.linked_stem_result_receiver();
    let clip_indicator = engine.clip_indicator();
    let level_atomics = engine.level_atomics();
    let internal_latency_samples = engine.internal_latency_samples();

    // Create lock-free command channel
    let (command_tx, command_rx) = command_channel();

    // Create direct command channel for timing-critical MIDI commands
    let (direct_tx, direct_rx) = rtrb::RingBuffer::<EngineCommand>::new(64);

    let handles = DjHandles {
        command_sender: CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        linked_stem_atomics,
        linked_stem_receiver,
        clip_indicator,
        level_atomics,
        internal_latency_samples,
        direct_command_producer: direct_tx,
    };
    let engine = DjRtEngine { engine, command_rx, direct_command_rx: direct_rx };
    (engine, handles)
}

/// Result of starting the audio system with a custom engine
///
/// `handles` is whatever the engine factory returned next to the engine.
pub struct EngineAudioSystem<H> {
    /// Actual JACK client name (may differ from requested if name was taken)
    pub client_name: String,
    /// Handle to keep audio alive (drop to stop)
    pub handle: AudioHandle,
    /// Sample rate of the audio system (the rate given to the factory)
    pub sample_rate: u32,
    /// Actual buffer size in frames
    pub buffer_size: u32,
    /// Audio latency in milliseconds (one-way, output only)
    pub latency_ms: f32,
    /// Real output pipeline latency in samples (from `RtEngine::output_latency_samples`)
    pub output_latency_samples: Arc<AtomicU64>,
    /// Engine-specific UI handles from the factory
    pub handles: H,
}

/// Result of starting the audio system
///
/// Contains all the handles and communication channels needed by the UI.
pub struct AudioSystemResult {
    /// Actual JACK client name (may differ from requested if name was taken)
    /// Used for port connection/reconnection.
    pub client_name: String,
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
    /// Master clipper clip indicator (true = clipping occurred)
    pub clip_indicator: Arc<AtomicBool>,
    /// Lock-free peak level atomics for UI metering
    pub level_atomics: Arc<LevelAtomics>,
    /// Sample rate of the audio system
    pub sample_rate: u32,
    /// Actual buffer size in frames
    pub buffer_size: u32,
    /// Audio latency in milliseconds (one-way, output only)
    pub latency_ms: f32,
    /// Real output pipeline latency in samples (measured from CPAL/JACK timestamps)
    pub output_latency_samples: Arc<AtomicU64>,
    /// Internal effect chain latency in samples (global max)
    pub internal_latency_samples: Arc<AtomicU32>,
    /// Direct command producer for timing-critical MIDI commands
    /// (bypasses iced tick loop, goes straight to audio thread)
    pub direct_command_producer: rtrb::Producer<crate::engine::EngineCommand>,
}

/// Handle to the active audio system
///
/// Keeps the audio streams/client alive. Drop this to stop audio.
pub enum AudioHandle {
    /// CPAL-based handle (cross-platform, also JACK fallback on Linux)
    Cpal(super::cpal_backend::CpalAudioHandle),

    /// Native JACK handle (Linux with jack-backend feature)
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    Jack(super::jack_backend::JackAudioHandle),
}

impl AudioHandle {
    /// Get the sample rate of the audio system
    pub fn sample_rate(&self) -> u32 {
        match self {
            AudioHandle::Cpal(h) => h.sample_rate(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.sample_rate(),
        }
    }

    /// Get the actual buffer size in frames
    pub fn buffer_size(&self) -> u32 {
        match self {
            AudioHandle::Cpal(h) => h.buffer_size(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.buffer_size(),
        }
    }

    /// Get the audio latency in milliseconds
    pub fn latency_ms(&self) -> f32 {
        match self {
            AudioHandle::Cpal(h) => h.latency_ms(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.latency_ms(),
        }
    }

    /// Pause the audio stream to free CPU for batch operations
    pub fn pause(&self) {
        match self {
            AudioHandle::Cpal(h) => h.pause(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.pause(),
        }
    }

    /// Resume the audio stream for playback
    pub fn play(&self) {
        match self {
            AudioHandle::Cpal(h) => h.play(),
            #[cfg(all(target_os = "linux", feature = "jack-backend"))]
            AudioHandle::Jack(h) => h.play(),
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
/// - **Linux with jack-backend feature**: Try JACK first, fall back to CPAL
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
    let system = start_audio_system_with(config, |sample_rate| {
        make_dj_engine(sample_rate, db_service.clone())
    })?;
    let h = system.handles;
    Ok(AudioSystemResult {
        client_name: system.client_name,
        handle: system.handle,
        command_sender: h.command_sender,
        deck_atomics: h.deck_atomics,
        slicer_atomics: h.slicer_atomics,
        linked_stem_atomics: h.linked_stem_atomics,
        linked_stem_receiver: h.linked_stem_receiver,
        clip_indicator: h.clip_indicator,
        level_atomics: h.level_atomics,
        sample_rate: system.sample_rate,
        buffer_size: system.buffer_size,
        latency_ms: system.latency_ms,
        output_latency_samples: system.output_latency_samples,
        internal_latency_samples: h.internal_latency_samples,
        direct_command_producer: h.direct_command_producer,
    })
}

/// Start the audio system with an engine built by `make`
///
/// Same backend selection as [`start_audio_system`]. `make` receives the
/// device's actual sample rate and returns the RT engine (moved to the audio
/// thread) plus its UI-side handles. It is `FnMut` because a JACK failure
/// after engine creation falls back to CPAL, which builds a second engine.
pub fn start_audio_system_with<E, H, F>(
    config: &AudioConfig,
    mut make: F,
) -> AudioResult<EngineAudioSystem<H>>
where
    E: RtEngine + 'static,
    F: FnMut(u32) -> (E, H),
{
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        match super::jack_backend::start_audio_system(config, &mut make) {
            Ok(result) => Ok(result),
            Err(e) => {
                log::warn!("JACK backend failed: {} — falling back to CPAL", e);
                super::cpal_backend::start_audio_system(config, &mut make)
            }
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "jack-backend")))]
    {
        super::cpal_backend::start_audio_system(config, &mut make)
    }
}

/// Get available stereo output pairs for UI dropdown
///
/// On JACK: Returns actual port pairs (e.g., "Scarlett 1-2", "Scarlett 3-4")
/// On CPAL: Returns devices as pseudo-pairs
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    #[cfg(all(target_os = "linux", feature = "jack-backend"))]
    {
        let pairs = super::jack_backend::get_available_stereo_pairs();
        if pairs.is_empty() {
            // JACK not available — fall back to CPAL device list
            super::cpal_backend::get_available_stereo_pairs()
        } else {
            pairs
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal engine: writes a constant into master, nothing into cue
    struct ConstEngine {
        value: f32,
        producers: Vec<rtrb::Producer<StereoSample>>,
    }

    impl RtEngine for ConstEngine {
        fn process_block(&mut self, master: &mut StereoBuffer, _cue: &mut StereoBuffer) {
            for s in master.as_mut_slice() {
                *s = StereoSample::new(self.value, -self.value);
            }
        }

        fn recording_producers(&mut self) -> &mut Vec<rtrb::Producer<StereoSample>> {
            &mut self.producers
        }
    }

    #[test]
    fn push_to_recording_feeds_and_prunes_producers() {
        let (live_tx, mut live_rx) = rtrb::RingBuffer::<StereoSample>::new(16);
        let (dead_tx, dead_rx) = rtrb::RingBuffer::<StereoSample>::new(16);
        drop(dead_rx);
        let mut engine = ConstEngine { value: 0.5, producers: vec![live_tx, dead_tx] };

        let mut master = StereoBuffer::silence(4);
        let mut cue = StereoBuffer::silence(4);
        engine.process_block(&mut master, &mut cue);
        push_to_recording(&mut engine, master.as_slice());

        assert_eq!(engine.producers.len(), 1, "abandoned producer is dropped");
        assert_eq!(live_rx.slots(), 4);
        let s = live_rx.pop().unwrap();
        assert_eq!((s.left, s.right), (0.5, -0.5));
    }

    #[test]
    fn push_to_recording_drops_when_full() {
        let (tx, rx) = rtrb::RingBuffer::<StereoSample>::new(2);
        let mut engine = ConstEngine { value: 1.0, producers: vec![tx] };
        let master = StereoBuffer::silence(8);
        push_to_recording(&mut engine, master.as_slice());
        assert_eq!(rx.slots(), 2);
        assert_eq!(engine.producers.len(), 1);
    }
}
