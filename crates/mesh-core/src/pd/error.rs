//! Error types for Pure Data integration
//!
//! Provides structured errors for PD operations including initialization,
//! patch loading, audio processing, and effect discovery.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during PD operations
#[derive(Debug, Error)]
pub enum PdError {
    /// Failed to initialize libpd
    #[error("Failed to initialize libpd: {0}")]
    InitializationFailed(String),

    /// Failed to configure libpd audio
    #[error("Failed to configure audio: {channels} channels @ {sample_rate}Hz - {reason}")]
    AudioConfigFailed {
        channels: i32,
        sample_rate: i32,
        reason: String,
    },

    /// Failed to open a PD patch file
    #[error("Failed to open patch '{path}': {reason}")]
    PatchOpenFailed { path: PathBuf, reason: String },

    /// Failed to close a PD patch
    #[error("Failed to close patch: {0}")]
    PatchCloseFailed(String),

    /// Patch file not found
    #[error("Patch file not found: {0}")]
    PatchNotFound(PathBuf),

    /// Failed to send message to PD
    #[error("Failed to send {msg_type} to receiver '{receiver}': {reason}")]
    SendFailed {
        msg_type: String,
        receiver: String,
        reason: String,
    },

    /// Invalid effect metadata
    #[error("Invalid metadata for effect '{effect_id}': {reason}")]
    InvalidMetadata { effect_id: String, reason: String },

    /// Missing required external
    #[error("Effect '{effect_id}' requires external '{external}' which was not found")]
    MissingExternal { effect_id: String, external: String },

    /// Effect not found in discovery
    #[error("Effect '{0}' not found")]
    EffectNotFound(String),

    /// IO error during discovery or file operations
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// JSON parsing error for metadata
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Instance not initialized for deck
    #[error("PD instance not initialized for deck {0}")]
    InstanceNotInitialized(usize),

    /// Thread safety violation
    #[error("PD operation called from wrong thread")]
    ThreadSafetyViolation,
}

/// Result type for PD operations
pub type PdResult<T> = Result<T, PdError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PdError::PatchNotFound(PathBuf::from("/foo/bar.pd"));
        assert!(err.to_string().contains("/foo/bar.pd"));

        let err = PdError::MissingExternal {
            effect_id: "rave".to_string(),
            external: "nn~".to_string(),
        };
        assert!(err.to_string().contains("nn~"));
    }
}
