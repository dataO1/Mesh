//! CPAL audio backend implementation
//!
//! Provides the core audio streaming functionality using CPAL.
//! Supports both single-output (master only) and dual-output (master + cue) modes.
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
//! │   DeckAtomics    │◄────────────────────│  CPAL Audio Thread  │
//! │   (lock-free)    │     sync writes     │  (owns AudioEngine) │
//! └──────────────────┘                     └─────────────────────┘
//! ```

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize as CpalBufferSize, SampleFormat, Stream, StreamConfig};

use super::config::{AudioConfig, BufferSize, OutputMode, DEFAULT_BUFFER_SIZE, MAX_BUFFER_SIZE};
use super::device::{find_device_by_id, get_cpal_default_device};
use super::error::{AudioError, AudioResult};
use crate::db::DatabaseService;
use crate::engine::{
    command_channel, AudioEngine, DeckAtomics, EngineCommand, LinkedStemAtomics, SlicerAtomics,
};
use crate::loader::LinkedStemResultReceiver;
use crate::types::{StereoBuffer, StereoSample, NUM_DECKS};

/// Handle to the active audio system
///
/// Keeps the audio streams alive. Drop this to stop audio.
pub struct AudioHandle {
    /// Master output stream
    _master_stream: Stream,
    /// Cue output stream (only present in MasterAndCue mode)
    _cue_stream: Option<Stream>,
    /// Sample rate of the audio system
    sample_rate: u32,
    /// Actual buffer size in frames (as negotiated with the device)
    buffer_size: u32,
}

impl AudioHandle {
    /// Get the sample rate of the audio system
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Get the actual buffer size in frames
    pub fn buffer_size(&self) -> u32 {
        self.buffer_size
    }

    /// Get the audio latency in milliseconds (one-way, output only)
    pub fn latency_ms(&self) -> f32 {
        (self.buffer_size as f32 / self.sample_rate as f32) * 1000.0
    }
}

/// Command sender for the UI thread
///
/// Wraps the lock-free producer for sending EngineCommand to the audio thread.
/// All operations are non-blocking (~50ns per command).
pub struct CommandSender {
    producer: rtrb::Producer<EngineCommand>,
}

impl CommandSender {
    /// Send a command to the audio engine (non-blocking, ~50ns)
    ///
    /// Returns `Ok(())` if the command was queued successfully,
    /// or `Err(cmd)` if the queue is full (command is returned).
    pub fn send(&mut self, cmd: EngineCommand) -> Result<(), EngineCommand> {
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

/// Result of starting the audio system
pub struct AudioSystemResult {
    /// Handle to keep audio alive (drop to stop)
    pub handle: AudioHandle,
    /// Command sender for UI thread
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

/// Start the audio system with the given configuration
///
/// This creates and starts the audio streams based on the configuration.
/// In MasterOnly mode, a single stream is created for the master output.
/// In MasterAndCue mode, two streams are created (possibly on different devices).
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
    match config.output_mode {
        OutputMode::MasterOnly => start_master_only(config, db_service),
        OutputMode::MasterAndCue => start_master_and_cue(config, db_service),
    }
}

/// Start audio system in master-only mode (single stereo output)
fn start_master_only(
    config: &AudioConfig,
    db_service: Arc<DatabaseService>,
) -> AudioResult<AudioSystemResult> {
    // Get the master device
    let device = match &config.master_device {
        Some(id) => find_device_by_id(id)?,
        None => get_cpal_default_device()?,
    };

    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
    log::info!("Using audio device: {}", device_name);

    // Get supported config with buffer size preference
    let (supported_config, buffer_size) = get_output_config(&device, config)?;
    let sample_rate = supported_config.sample_rate().0;

    // Build the stream config with buffer size
    let stream_config = StreamConfig {
        channels: supported_config.channels(),
        sample_rate: supported_config.sample_rate(),
        buffer_size: buffer_size_to_cpal(buffer_size),
    };

    let latency_ms = (buffer_size as f32 / sample_rate as f32) * 1000.0;

    log::info!(
        "Audio config: {} channels, {}Hz, {} frames (~{:.1}ms latency)",
        stream_config.channels,
        sample_rate,
        buffer_size,
        latency_ms
    );

    // Create engine and extract atomics
    let engine = AudioEngine::new_with_sample_rate(sample_rate, db_service);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();
    let linked_stem_atomics = engine.linked_stem_atomics();
    let linked_stem_receiver = engine.linked_stem_result_receiver();

    // Create command channel
    let (command_tx, command_rx) = command_channel();

    // Create the audio callback state (lock-free triple buffer approach)
    let callback_state = AudioCallbackState::new(engine, command_rx, OutputMode::MasterOnly);
    let callback_state = Arc::new(std::sync::Mutex::new(callback_state));

    // Build the stream
    let stream = build_output_stream(&device, &stream_config, callback_state)?;
    stream
        .play()
        .map_err(|e| AudioError::StreamPlayError(e.to_string()))?;

    log::info!("Audio stream started (master-only mode)");

    Ok(AudioSystemResult {
        handle: AudioHandle {
            _master_stream: stream,
            _cue_stream: None,
            sample_rate,
            buffer_size,
        },
        command_sender: CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        linked_stem_atomics,
        linked_stem_receiver,
        sample_rate,
        buffer_size,
        latency_ms,
    })
}

/// Start audio system in master+cue mode (dual stereo outputs)
fn start_master_and_cue(
    config: &AudioConfig,
    db_service: Arc<DatabaseService>,
) -> AudioResult<AudioSystemResult> {
    // Get the master device
    let master_device = match &config.master_device {
        Some(id) => find_device_by_id(id)?,
        None => get_cpal_default_device()?,
    };

    // Get the cue device (can be same as master or different)
    let cue_device = match &config.cue_device {
        Some(id) => find_device_by_id(id)?,
        None => get_cpal_default_device()?,
    };

    let master_name = master_device.name().unwrap_or_else(|_| "Unknown".to_string());
    let cue_name = cue_device.name().unwrap_or_else(|_| "Unknown".to_string());
    log::info!("Master device: {}", master_name);
    log::info!("Cue device: {}", cue_name);

    // Get configs for both devices with buffer size preference
    let (master_supported, master_buffer_size) = get_output_config(&master_device, config)?;
    let (cue_supported, cue_buffer_size) = get_output_config(&cue_device, config)?;

    let master_sample_rate = master_supported.sample_rate().0;
    let cue_sample_rate = cue_supported.sample_rate().0;

    // For now, require same sample rate (could add resampling later)
    if master_sample_rate != cue_sample_rate {
        return Err(AudioError::SampleRateMismatch {
            master: master_sample_rate,
            cue: cue_sample_rate,
        });
    }

    let sample_rate = master_sample_rate;
    // Use the larger buffer size if they differ (for stability)
    let buffer_size = master_buffer_size.max(cue_buffer_size);

    let master_stream_config = StreamConfig {
        channels: master_supported.channels(),
        sample_rate: master_supported.sample_rate(),
        buffer_size: buffer_size_to_cpal(buffer_size),
    };
    let cue_stream_config = StreamConfig {
        channels: cue_supported.channels(),
        sample_rate: cue_supported.sample_rate(),
        buffer_size: buffer_size_to_cpal(buffer_size),
    };

    let latency_ms = (buffer_size as f32 / sample_rate as f32) * 1000.0;

    log::info!(
        "Master config: {} channels, {}Hz, {} frames (~{:.1}ms)",
        master_stream_config.channels,
        sample_rate,
        buffer_size,
        latency_ms
    );
    log::info!(
        "Cue config: {} channels, {}Hz, {} frames",
        cue_stream_config.channels,
        sample_rate,
        buffer_size
    );

    // Create engine and extract atomics
    let engine = AudioEngine::new_with_sample_rate(sample_rate, db_service);
    let deck_atomics = engine.deck_atomics();
    let slicer_atomics = engine.slicer_atomics();
    let linked_stem_atomics = engine.linked_stem_atomics();
    let linked_stem_receiver = engine.linked_stem_result_receiver();

    // Create command channel
    let (command_tx, command_rx) = command_channel();

    // Create shared callback state for both streams
    // Actual buffer size is tracked atomically for synchronization
    let actual_buffer_size = Arc::new(AtomicU32::new(buffer_size));
    let callback_state = AudioCallbackState::new_with_buffer_tracking(
        engine,
        command_rx,
        OutputMode::MasterAndCue,
        actual_buffer_size.clone(),
    );
    let callback_state = Arc::new(std::sync::Mutex::new(callback_state));

    // Build master stream
    let master_stream =
        build_output_stream(&master_device, &master_stream_config, callback_state.clone())?;

    // Build cue stream (shares state with master)
    let cue_stream = build_cue_stream(&cue_device, &cue_stream_config, callback_state)?;

    // Start both streams
    master_stream
        .play()
        .map_err(|e| AudioError::StreamPlayError(format!("Master: {}", e)))?;
    cue_stream
        .play()
        .map_err(|e| AudioError::StreamPlayError(format!("Cue: {}", e)))?;

    log::info!("Audio streams started (master+cue mode)");

    Ok(AudioSystemResult {
        handle: AudioHandle {
            _master_stream: master_stream,
            _cue_stream: Some(cue_stream),
            sample_rate,
            buffer_size,
        },
        command_sender: CommandSender { producer: command_tx },
        deck_atomics,
        slicer_atomics,
        linked_stem_atomics,
        linked_stem_receiver,
        sample_rate,
        buffer_size,
        latency_ms,
    })
}

/// State shared between audio callbacks
struct AudioCallbackState {
    /// The audio engine (owned exclusively by audio thread)
    engine: AudioEngine,
    /// Command receiver from UI
    command_rx: rtrb::Consumer<EngineCommand>,
    /// Pre-allocated master buffer
    master_buffer: StereoBuffer,
    /// Pre-allocated cue buffer
    cue_buffer: StereoBuffer,
    /// Output mode
    output_mode: OutputMode,
    /// Whether we've processed this frame (for dual-stream mode)
    frame_processed: bool,
    /// Actual buffer size tracking (for dual-stream synchronization)
    actual_buffer_size: Option<Arc<AtomicU32>>,
}

impl AudioCallbackState {
    fn new(
        engine: AudioEngine,
        command_rx: rtrb::Consumer<EngineCommand>,
        output_mode: OutputMode,
    ) -> Self {
        Self {
            engine,
            command_rx,
            master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
            cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
            output_mode,
            frame_processed: false,
            actual_buffer_size: None,
        }
    }

    fn new_with_buffer_tracking(
        engine: AudioEngine,
        command_rx: rtrb::Consumer<EngineCommand>,
        output_mode: OutputMode,
        actual_buffer_size: Arc<AtomicU32>,
    ) -> Self {
        Self {
            engine,
            command_rx,
            master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
            cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
            output_mode,
            frame_processed: false,
            actual_buffer_size: Some(actual_buffer_size),
        }
    }

    /// Process audio and fill buffers (called by master stream)
    fn process(&mut self, n_frames: usize) {
        // Update actual buffer size if tracking
        if let Some(ref size) = self.actual_buffer_size {
            size.store(n_frames as u32, Ordering::Relaxed);
        }

        // Set working buffer length (RT-safe: no allocation)
        self.master_buffer.set_len_from_capacity(n_frames);
        self.cue_buffer.set_len_from_capacity(n_frames);

        // Process commands from UI (lock-free)
        self.engine.process_commands(&mut self.command_rx);

        // Process audio through the engine
        self.engine
            .process(&mut self.master_buffer, &mut self.cue_buffer);

        self.frame_processed = true;
    }

    /// Get master output samples
    fn master_samples(&self) -> &[StereoSample] {
        self.master_buffer.as_slice()
    }

    /// Get cue output samples
    fn cue_samples(&self) -> &[StereoSample] {
        self.cue_buffer.as_slice()
    }
}

/// Convert our BufferSize to CPAL's BufferSize
fn buffer_size_to_cpal(frames: u32) -> CpalBufferSize {
    CpalBufferSize::Fixed(frames)
}

/// Get the best output configuration for a device
///
/// Returns (SupportedStreamConfig, actual_buffer_size_in_frames)
fn get_output_config(
    device: &cpal::Device,
    config: &AudioConfig,
) -> AudioResult<(cpal::SupportedStreamConfig, u32)> {
    let supported_configs: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| AudioError::ConfigError(e.to_string()))?
        .collect();

    if supported_configs.is_empty() {
        return Err(AudioError::ConfigError(
            "No supported output configurations".to_string(),
        ));
    }

    // Prefer f32 format, stereo, and the requested sample rate
    // Default to 48kHz to match stored track format (avoids resampling)
    let target_sample_rate = config.sample_rate.unwrap_or(super::config::DEFAULT_SAMPLE_RATE);

    // Find the best matching config
    let best_config = supported_configs
        .iter()
        // Prefer f32 format
        .filter(|c| c.sample_format() == SampleFormat::F32)
        // Prefer stereo
        .filter(|c| c.channels() >= 2)
        // Check if target sample rate is in range
        .filter(|c| {
            target_sample_rate >= c.min_sample_rate().0
                && target_sample_rate <= c.max_sample_rate().0
        })
        .next()
        .or_else(|| {
            // Fallback: any config with at least 2 channels
            supported_configs.iter().filter(|c| c.channels() >= 2).next()
        })
        .or_else(|| {
            // Last resort: any config
            supported_configs.first()
        })
        .ok_or_else(|| {
            AudioError::ConfigError("No suitable output configuration found".to_string())
        })?;

    // Create the final config with the desired sample rate
    let sample_rate = if target_sample_rate >= best_config.min_sample_rate().0
        && target_sample_rate <= best_config.max_sample_rate().0
    {
        cpal::SampleRate(target_sample_rate)
    } else {
        // Device doesn't support requested rate - use max supported rate
        let fallback = best_config.max_sample_rate();
        log::warn!(
            "Audio device doesn't support {}Hz, falling back to {}Hz (tracks will be resampled)",
            target_sample_rate,
            fallback.0
        );
        fallback
    };

    let stream_config = best_config.clone().with_sample_rate(sample_rate);

    // Determine buffer size based on configuration
    let buffer_size = match config.buffer_size {
        BufferSize::Default => {
            // Use a reasonable default for DJ applications
            DEFAULT_BUFFER_SIZE
        }
        BufferSize::Fixed(frames) => {
            // Use the requested size, clamped to reasonable bounds
            frames.clamp(64, MAX_BUFFER_SIZE as u32)
        }
        BufferSize::LowLatency => {
            // For low-latency mode, use a safe but responsive default
            // The actual latency detection would require runtime testing
            // which is complex to implement reliably, so we use a known-good value
            256
        }
    };

    log::debug!(
        "Selected buffer size: {} frames for {:?} mode",
        buffer_size,
        config.buffer_size
    );

    Ok((stream_config, buffer_size))
}

/// Build the master output stream
fn build_output_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    state: Arc<std::sync::Mutex<AudioCallbackState>>,
) -> AudioResult<Stream> {
    let channels = config.channels as usize;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let mut state = state.lock().unwrap();
                let n_frames = data.len() / channels;

                // Process audio
                state.process(n_frames);

                // Copy master output to buffer
                let samples = state.master_samples();
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    if i < samples.len() {
                        let sample = samples[i];
                        frame[0] = sample.left;
                        if channels > 1 {
                            frame[1] = sample.right;
                        }
                        // Fill additional channels with silence
                        for ch in frame.iter_mut().skip(2) {
                            *ch = 0.0;
                        }
                    } else {
                        // Fill with silence if we don't have enough samples
                        for ch in frame.iter_mut() {
                            *ch = 0.0;
                        }
                    }
                }
            },
            move |err| {
                log::error!("Master audio stream error: {}", err);
            },
            None, // No timeout (blocking)
        )
        .map_err(|e| AudioError::StreamBuildError(e.to_string()))?;

    Ok(stream)
}

/// Build the cue output stream (for dual-output mode)
fn build_cue_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    state: Arc<std::sync::Mutex<AudioCallbackState>>,
) -> AudioResult<Stream> {
    let channels = config.channels as usize;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let state = state.lock().unwrap();

                // Note: We rely on master stream having processed the audio
                // In practice, both streams run independently but read from shared buffers

                // Copy cue output to buffer
                let samples = state.cue_samples();
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    if i < samples.len() {
                        let sample = samples[i];
                        frame[0] = sample.left;
                        if channels > 1 {
                            frame[1] = sample.right;
                        }
                        for ch in frame.iter_mut().skip(2) {
                            *ch = 0.0;
                        }
                    } else {
                        for ch in frame.iter_mut() {
                            *ch = 0.0;
                        }
                    }
                }
            },
            move |err| {
                log::error!("Cue audio stream error: {}", err);
            },
            None,
        )
        .map_err(|e| AudioError::StreamBuildError(e.to_string()))?;

    Ok(stream)
}
