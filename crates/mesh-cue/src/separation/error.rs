//! Separation error types

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during audio separation
#[derive(Error, Debug)]
pub enum SeparationError {
    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Model download failed: {0}")]
    ModelDownloadFailed(String),

    #[error("Failed to read audio file: {path}")]
    AudioReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Unsupported audio format: {0}")]
    UnsupportedFormat(String),

    #[error("Separation failed: {0}")]
    SeparationFailed(String),

    #[error("Backend initialization failed: {0}")]
    BackendInitFailed(String),

    #[error("GPU not available, falling back to CPU")]
    GpuNotAvailable,

    #[error("Failed to write stem file: {path}")]
    StemWriteError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

pub type Result<T> = std::result::Result<T, SeparationError>;
