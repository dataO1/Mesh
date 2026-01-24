//! CPAL audio backend implementation
//!
//! Provides the core audio streaming functionality using CPAL.
//! Supports both single-output (master only) and dual-output (master + cue) modes.
//!
//! # Architecture
//!
//! ## Single Output (Master Only)
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
//!
//! ## Dual Output (Master + Cue)
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        LOCK-FREE DESIGN                         │
//! │  Master and Cue streams run independently without blocking      │
//! └─────────────────────────────────────────────────────────────────┘
//!
//!                    ┌───────────────────────┐
//!   UI Commands ────►│   Master Stream       │
//!                    │  (owns AudioEngine)   │
//!                    │  processes audio      │
//!                    └───────────┬───────────┘
//!                                │
//!                    ┌───────────▼───────────┐
//!                    │  Cue Sample Queue     │  <── lock-free ring buffer
//!                    │  (SPSC, ~8K samples)  │      master produces, cue consumes
//!                    └───────────┬───────────┘
//!                                │
//!                    ┌───────────▼───────────┐
//!                    │    Cue Stream         │  <── reads from queue only
//!                    │  (independent thread) │      never blocks on master
//!                    └───────────────────────┘
//! ```

use std::sync::Arc;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize as CpalBufferSize, SampleFormat, Stream, StreamConfig};

use super::config::{AudioConfig, BufferSize, OutputMode, DEFAULT_BUFFER_SIZE, MAX_BUFFER_SIZE};
use super::device::{find_device_by_id, get_cpal_default_device};
use super::error::{AudioError, AudioResult};
use crate::db::DatabaseService;
use crate::engine::{command_channel, AudioEngine, EngineCommand};
use crate::types::{StereoBuffer, StereoSample};

use super::backend::{AudioHandle, AudioSystemResult, CommandSender, StereoPair};

/// CPAL-specific audio handle
///
/// Keeps the audio streams alive. Drop this to stop audio.
pub struct CpalAudioHandle {
    /// Master output stream
    _master_stream: Stream,
    /// Cue output stream (only present in MasterAndCue mode)
    _cue_stream: Option<Stream>,
    /// Sample rate of the audio system
    sample_rate: u32,
    /// Actual buffer size in frames (as negotiated with the device)
    buffer_size: u32,
}

impl CpalAudioHandle {
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

    let handle = CpalAudioHandle {
        _master_stream: stream,
        _cue_stream: None,
        sample_rate,
        buffer_size,
    };

    Ok(AudioSystemResult {
        handle: AudioHandle::Cpal(handle),
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
///
/// This uses a lock-free architecture where:
/// - Master stream owns the AudioEngine and processes audio
/// - Cue samples are sent to the cue stream via a lock-free ring buffer
/// - Cue stream never blocks waiting for master
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

    // Create lock-free ring buffer for cue samples
    // Capacity: 4x buffer size to handle timing jitter between streams
    // Using StereoSample directly for efficient transfer
    let cue_buffer_capacity = (buffer_size as usize) * 4;
    let (cue_producer, cue_consumer) = rtrb::RingBuffer::<StereoSample>::new(cue_buffer_capacity);
    log::debug!(
        "Cue sample ring buffer created with capacity {} samples",
        cue_buffer_capacity
    );

    // Create callback state for master stream (owns the engine)
    let callback_state = AudioCallbackState::new(engine, command_rx, OutputMode::MasterAndCue);
    let callback_state = Arc::new(std::sync::Mutex::new(callback_state));

    // Build master stream with cue producer
    let master_stream = build_master_stream_dual(
        &master_device,
        &master_stream_config,
        callback_state,
        cue_producer,
    )?;

    // Build cue stream with cue consumer (lock-free, no shared state)
    let cue_stream = build_cue_stream_lockfree(&cue_device, &cue_stream_config, cue_consumer)?;

    // Start both streams
    master_stream
        .play()
        .map_err(|e| AudioError::StreamPlayError(format!("Master: {}", e)))?;
    cue_stream
        .play()
        .map_err(|e| AudioError::StreamPlayError(format!("Cue: {}", e)))?;

    log::info!("Audio streams started (master+cue mode, lock-free)");

    let handle = CpalAudioHandle {
        _master_stream: master_stream,
        _cue_stream: Some(cue_stream),
        sample_rate,
        buffer_size,
    };

    Ok(AudioSystemResult {
        handle: AudioHandle::Cpal(handle),
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

/// State for audio callbacks
///
/// In single-output mode: owned exclusively by the master stream callback
/// In dual-output mode: owned by master stream, cue samples sent via ring buffer
struct AudioCallbackState {
    /// The audio engine (owned exclusively by audio thread)
    engine: AudioEngine,
    /// Command receiver from UI
    command_rx: rtrb::Consumer<EngineCommand>,
    /// Pre-allocated master buffer
    master_buffer: StereoBuffer,
    /// Pre-allocated cue buffer
    cue_buffer: StereoBuffer,
}

impl AudioCallbackState {
    fn new(
        engine: AudioEngine,
        command_rx: rtrb::Consumer<EngineCommand>,
        _output_mode: OutputMode,
    ) -> Self {
        Self {
            engine,
            command_rx,
            master_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
            cue_buffer: StereoBuffer::silence(MAX_BUFFER_SIZE),
        }
    }

    /// Process audio and fill buffers (called by master stream)
    fn process(&mut self, n_frames: usize) {
        // Set working buffer length (RT-safe: no allocation)
        self.master_buffer.set_len_from_capacity(n_frames);
        self.cue_buffer.set_len_from_capacity(n_frames);

        // Process commands from UI (lock-free)
        self.engine.process_commands(&mut self.command_rx);

        // Process audio through the engine
        self.engine
            .process(&mut self.master_buffer, &mut self.cue_buffer);
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

/// Build the cue output stream (for dual-output mode) - LEGACY, uses shared mutex
/// Deprecated: Use build_cue_stream_lockfree instead for better performance
#[allow(dead_code)]
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

/// Build the master output stream for dual-output mode
///
/// This version also pushes cue samples to the lock-free ring buffer
/// so the cue stream can read them without blocking.
fn build_master_stream_dual(
    device: &cpal::Device,
    config: &StreamConfig,
    state: Arc<std::sync::Mutex<AudioCallbackState>>,
    mut cue_producer: rtrb::Producer<StereoSample>,
) -> AudioResult<Stream> {
    let channels = config.channels as usize;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let mut state = state.lock().unwrap();
                let n_frames = data.len() / channels;

                // Process audio (fills both master and cue buffers)
                state.process(n_frames);

                // Copy master output to device buffer
                let master_samples = state.master_samples();
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    if i < master_samples.len() {
                        let sample = master_samples[i];
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

                // Push cue samples to the lock-free ring buffer
                // The cue stream will read these independently
                let cue_samples = state.cue_samples();
                for sample in cue_samples {
                    // If buffer is full, drop oldest samples (cue stream is behind)
                    // This is better than blocking or dropping new samples
                    if cue_producer.push(*sample).is_err() {
                        // Buffer full - could log this but it's RT-safe not to
                        // In practice, if cue is behind, it will catch up
                        break;
                    }
                }
            },
            move |err| {
                log::error!("Master audio stream error: {}", err);
            },
            None,
        )
        .map_err(|e| AudioError::StreamBuildError(e.to_string()))?;

    Ok(stream)
}

/// Build the cue output stream using lock-free ring buffer
///
/// This stream reads cue samples from a ring buffer without any blocking.
/// If samples aren't available (master hasn't produced them yet), plays silence.
fn build_cue_stream_lockfree(
    device: &cpal::Device,
    config: &StreamConfig,
    mut cue_consumer: rtrb::Consumer<StereoSample>,
) -> AudioResult<Stream> {
    let channels = config.channels as usize;

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let n_frames = data.len() / channels;

                // Read cue samples from ring buffer (non-blocking)
                for (i, frame) in data.chunks_mut(channels).enumerate() {
                    if i >= n_frames {
                        break;
                    }

                    // Try to pop a sample from the ring buffer
                    match cue_consumer.pop() {
                        Ok(sample) => {
                            frame[0] = sample.left;
                            if channels > 1 {
                                frame[1] = sample.right;
                            }
                            for ch in frame.iter_mut().skip(2) {
                                *ch = 0.0;
                            }
                        }
                        Err(_) => {
                            // No samples available - play silence
                            // This happens briefly at startup or if master is slow
                            for ch in frame.iter_mut() {
                                *ch = 0.0;
                            }
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

// ═══════════════════════════════════════════════════════════════════════════════
// Device Enumeration for UI
// ═══════════════════════════════════════════════════════════════════════════════

/// Get available stereo output pairs for UI dropdown
///
/// For CPAL, each device is treated as a stereo pair since CPAL handles
/// channel routing internally. The left/right fields contain the device ID.
pub fn get_available_stereo_pairs() -> Vec<StereoPair> {
    super::device::get_available_output_devices()
        .into_iter()
        .map(|d| {
            let device_id = d.id.display_label();
            StereoPair {
                label: format!("[{}] {}", d.host, d.name),
                left: device_id.clone(),
                right: device_id,
            }
        })
        .collect()
}
