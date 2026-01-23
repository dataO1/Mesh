//! Audio backend error types

use thiserror::Error;

/// Errors that can occur during audio operations
#[derive(Error, Debug)]
pub enum AudioError {
    /// No audio devices available
    #[error("No audio output devices found")]
    NoDevices,

    /// Failed to get default device
    #[error("Failed to get default audio device: {0}")]
    NoDefaultDevice(String),

    /// Device not found
    #[error("Audio device not found: {0}")]
    DeviceNotFound(String),

    /// Failed to get device configuration
    #[error("Failed to get device config: {0}")]
    ConfigError(String),

    /// Failed to build audio stream
    #[error("Failed to build audio stream: {0}")]
    StreamBuildError(String),

    /// Failed to start/play stream
    #[error("Failed to start audio stream: {0}")]
    StreamPlayError(String),

    /// Stream error during playback
    #[error("Audio stream error: {0}")]
    StreamError(String),

    /// Unsupported sample format
    #[error("Unsupported sample format: {0}")]
    UnsupportedFormat(String),

    /// Sample rate mismatch between devices
    #[error("Sample rate mismatch: master={master}Hz, cue={cue}Hz")]
    SampleRateMismatch { master: u32, cue: u32 },
}

/// Result type for audio operations
pub type AudioResult<T> = Result<T, AudioError>;
