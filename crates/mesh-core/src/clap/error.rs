//! Error types for CLAP plugin hosting
//!
//! Provides structured errors for CLAP operations including plugin loading,
//! activation, audio processing, and discovery.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during CLAP operations
#[derive(Debug, Error)]
pub enum ClapError {
    /// Failed to load plugin bundle
    #[error("Failed to load CLAP bundle '{path}': {reason}")]
    BundleLoadFailed { path: PathBuf, reason: String },

    /// Plugin not found in bundle
    #[error("Plugin '{plugin_id}' not found in bundle '{bundle_path}'")]
    PluginNotFound {
        plugin_id: String,
        bundle_path: PathBuf,
    },

    /// Failed to instantiate plugin
    #[error("Failed to instantiate plugin '{plugin_id}': {reason}")]
    InstantiationFailed { plugin_id: String, reason: String },

    /// Failed to activate plugin
    #[error("Failed to activate plugin '{plugin_id}': {reason}")]
    ActivationFailed { plugin_id: String, reason: String },

    /// Plugin is not activated
    #[error("Plugin '{plugin_id}' is not activated")]
    NotActivated { plugin_id: String },

    /// Failed to get plugin parameter info
    #[error("Failed to get parameter info for plugin '{plugin_id}': {reason}")]
    ParamInfoFailed { plugin_id: String, reason: String },

    /// Parameter index out of bounds
    #[error("Parameter index {index} out of bounds for plugin '{plugin_id}' (has {count} params)")]
    ParamIndexOutOfBounds {
        plugin_id: String,
        index: usize,
        count: usize,
    },

    /// Failed to set parameter value
    #[error("Failed to set parameter {param_id} on plugin '{plugin_id}': {reason}")]
    ParamSetFailed {
        plugin_id: String,
        param_id: u32,
        reason: String,
    },

    /// Audio processing error
    #[error("Audio processing error for plugin '{plugin_id}': {reason}")]
    ProcessingError { plugin_id: String, reason: String },

    /// Plugin discovery error
    #[error("Discovery error in '{path}': {reason}")]
    DiscoveryError { path: PathBuf, reason: String },

    /// No CLAP plugins found
    #[error("No CLAP plugins found in search paths")]
    NoPluginsFound,

    /// IO error during discovery or file operations
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Plugin state save/restore error
    #[error("State error for plugin '{plugin_id}': {reason}")]
    StateError { plugin_id: String, reason: String },

    /// Multiband configuration error
    #[error("Multiband configuration error: {0}")]
    MultibandConfigError(String),

    /// Crossover plugin not available
    #[error("Crossover plugin '{0}' not available - required for multiband processing")]
    CrossoverNotAvailable(String),

    /// Band index out of bounds
    #[error("Band index {index} out of bounds (max {max})")]
    BandIndexOutOfBounds { index: usize, max: usize },

    /// Thread safety violation
    #[error("CLAP operation called from wrong thread")]
    ThreadSafetyViolation,

    /// Lock acquisition failed (non-blocking)
    #[error("Failed to acquire lock for plugin '{plugin_id}' - skipping frame")]
    LockFailed { plugin_id: String },

    /// Plugin does not support GUI extension
    #[error("Plugin '{plugin_id}' does not support GUI")]
    GuiNotSupported { plugin_id: String },

    /// GUI API not supported by plugin
    #[error("Plugin '{plugin_id}' does not support GUI API '{api}'")]
    GuiApiNotSupported { plugin_id: String, api: String },

    /// Failed to create GUI
    #[error("Failed to create GUI for plugin '{plugin_id}': {reason}")]
    GuiCreationFailed { plugin_id: String, reason: String },

    /// Failed to set GUI parent window
    #[error("Failed to set GUI parent for plugin '{plugin_id}'")]
    GuiParentFailed { plugin_id: String },

    /// Failed to show GUI
    #[error("Failed to show GUI for plugin '{plugin_id}'")]
    GuiShowFailed { plugin_id: String },

    /// Failed to hide GUI
    #[error("Failed to hide GUI for plugin '{plugin_id}'")]
    GuiHideFailed { plugin_id: String },
}

/// Result type for CLAP operations
pub type ClapResult<T> = Result<T, ClapError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ClapError::PluginNotFound {
            plugin_id: "org.lsp-plug.compressor".to_string(),
            bundle_path: PathBuf::from("/usr/lib/clap/lsp.clap"),
        };
        assert!(err.to_string().contains("org.lsp-plug.compressor"));
        assert!(err.to_string().contains("lsp.clap"));

        let err = ClapError::BandIndexOutOfBounds { index: 5, max: 4 };
        assert!(err.to_string().contains("5"));
        assert!(err.to_string().contains("4"));
    }
}
