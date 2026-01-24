//! Audio backend configuration
//!
//! Defines configuration for the audio system including output mode,
//! device selection, and buffer settings.

use serde::{Deserialize, Serialize};

/// Maximum buffer size to pre-allocate (covers typical configurations)
/// Common values: 64, 128, 256, 512, 1024, 2048, 4096 frames
pub const MAX_BUFFER_SIZE: usize = 8192;

/// Common low-latency buffer sizes to try, in order of preference (frames)
/// These translate to approximately:
/// - 64 frames @ 44.1kHz = ~1.5ms
/// - 128 frames @ 44.1kHz = ~2.9ms
/// - 256 frames @ 44.1kHz = ~5.8ms
/// - 512 frames @ 44.1kHz = ~11.6ms (safe default for most systems)
pub const LOW_LATENCY_BUFFER_SIZES: [u32; 4] = [64, 128, 256, 512];

/// Default buffer size when no preference is specified (frames)
/// 512 frames is a safe default that works on most systems
pub const DEFAULT_BUFFER_SIZE: u32 = 512;

/// Default sample rate for the audio system (48kHz)
/// This matches the rate at which tracks are stored, avoiding unnecessary resampling.
/// If the audio device doesn't support 48kHz, the system will fall back to the
/// device's maximum supported rate and resample tracks during loading.
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Output mode for the audio system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputMode {
    /// Single stereo output (master only)
    /// Used by mesh-cue for preview playback
    #[default]
    MasterOnly,

    /// Dual stereo outputs (master + cue/headphones)
    /// Used by mesh-player for DJ mixing with separate headphone cue
    MasterAndCue,
}

/// Preferred buffer size for audio streams
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BufferSize {
    /// Let the system choose the default buffer size
    Default,
    /// Request a specific buffer size in frames (may be adjusted by the system)
    Fixed(u32),
    /// Automatically detect the lowest stable latency
    /// Tries progressively larger buffers until one works without xruns
    LowLatency,
}

impl Default for BufferSize {
    fn default() -> Self {
        Self::Default
    }
}

impl BufferSize {
    /// Get the buffer size in frames, or None for system default
    pub fn as_frames(&self) -> Option<u32> {
        match self {
            BufferSize::Default => None,
            BufferSize::Fixed(frames) => Some(*frames),
            BufferSize::LowLatency => Some(DEFAULT_BUFFER_SIZE), // Start with safe default
        }
    }

    /// Calculate latency in milliseconds for a given sample rate
    pub fn latency_ms(&self, sample_rate: u32) -> Option<f32> {
        self.as_frames().map(|frames| {
            (frames as f32 / sample_rate as f32) * 1000.0
        })
    }
}

/// Audio device identifier
///
/// Includes both the device name and the host backend (JACK, ALSA, etc.)
/// This allows selecting devices from different hosts on systems with multiple
/// audio backends available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    /// Device name as reported by the system
    pub name: String,
    /// Audio host identifier (e.g., "Jack", "Alsa", "CoreAudio")
    /// If None, uses the default/preferred host
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl DeviceId {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            host: None,
        }
    }

    pub fn with_host(name: &str, host: &str) -> Self {
        Self {
            name: name.to_string(),
            host: Some(host.to_string()),
        }
    }

    /// Get a display label that includes the host if available
    pub fn display_label(&self) -> String {
        match &self.host {
            Some(host) => format!("[{}] {}", host, self.name),
            None => self.name.clone(),
        }
    }
}

/// Configuration for the audio backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Output mode (master-only or master+cue)
    pub output_mode: OutputMode,

    /// Master output device (None = use system default)
    /// Used by CPAL backend
    pub master_device: Option<DeviceId>,

    /// Cue/headphone output device (only used if output_mode is MasterAndCue)
    /// None = use system default (different from master if available)
    /// Used by CPAL backend
    pub cue_device: Option<DeviceId>,

    /// Master stereo pair index for JACK backend
    /// 0 = first available pair, 1 = second, etc.
    #[serde(default)]
    pub master_pair_index: Option<usize>,

    /// Cue stereo pair index for JACK backend
    /// None = auto-detect (uses second pair if available)
    #[serde(default)]
    pub cue_pair_index: Option<usize>,

    /// Preferred buffer size
    pub buffer_size: BufferSize,

    /// Preferred sample rate (None = use device default, typically 44100 or 48000)
    pub sample_rate: Option<u32>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            output_mode: OutputMode::default(),
            master_device: None,
            cue_device: None,
            master_pair_index: None,
            cue_pair_index: None,
            buffer_size: BufferSize::default(),
            sample_rate: None,
        }
    }
}

impl AudioConfig {
    /// Create config for mesh-cue (master-only, single stereo output)
    pub fn master_only() -> Self {
        Self {
            output_mode: OutputMode::MasterOnly,
            ..Default::default()
        }
    }

    /// Create config for mesh-player (master + cue outputs)
    pub fn master_and_cue() -> Self {
        Self {
            output_mode: OutputMode::MasterAndCue,
            ..Default::default()
        }
    }

    /// Create config optimized for low latency
    ///
    /// Uses automatic low-latency detection to find the smallest
    /// stable buffer size for the system.
    pub fn low_latency() -> Self {
        Self {
            buffer_size: BufferSize::LowLatency,
            ..Default::default()
        }
    }

    /// Set the master device
    pub fn with_master_device(mut self, device: DeviceId) -> Self {
        self.master_device = Some(device);
        self
    }

    /// Set the cue device (only effective in MasterAndCue mode)
    pub fn with_cue_device(mut self, device: DeviceId) -> Self {
        self.cue_device = Some(device);
        self
    }

    /// Set the preferred buffer size
    pub fn with_buffer_size(mut self, size: BufferSize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Set a fixed buffer size in frames
    pub fn with_buffer_frames(mut self, frames: u32) -> Self {
        self.buffer_size = BufferSize::Fixed(frames);
        self
    }

    /// Set the preferred sample rate
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = Some(rate);
        self
    }

    /// Enable low-latency mode
    pub fn with_low_latency(mut self) -> Self {
        self.buffer_size = BufferSize::LowLatency;
        self
    }
}
