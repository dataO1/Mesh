//! Exportable configuration for USB devices
//!
//! This module defines the subset of player configuration that can be
//! exported to USB devices. It specifically excludes MIDI mappings which
//! are device-specific and shouldn't be transferred.

use crate::config::LoudnessConfig;
use serde::{Deserialize, Serialize};

/// Configuration subset that can be exported to USB devices
///
/// This includes audio, display, and slicer settings but deliberately
/// excludes MIDI mappings (which are device-specific).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportableConfig {
    /// Audio settings (global BPM, phase sync, loudness)
    pub audio: ExportableAudioConfig,
    /// Display settings (zoom, grid, loop length, stem colors)
    pub display: ExportableDisplayConfig,
    /// Slicer configuration
    pub slicer: ExportableSlicerConfig,
}

impl Default for ExportableConfig {
    fn default() -> Self {
        Self {
            audio: ExportableAudioConfig::default(),
            display: ExportableDisplayConfig::default(),
            slicer: ExportableSlicerConfig::default(),
        }
    }
}

/// Audio configuration for export
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportableAudioConfig {
    /// Global BPM for time-stretching
    pub global_bpm: f64,
    /// Enable automatic inter-deck phase synchronization
    pub phase_sync: bool,
    /// Loudness normalization settings
    pub loudness: LoudnessConfig,
}

impl Default for ExportableAudioConfig {
    fn default() -> Self {
        Self {
            global_bpm: 128.0,
            phase_sync: true,
            loudness: LoudnessConfig::default(),
        }
    }
}

/// Display configuration for export
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportableDisplayConfig {
    /// Default loop length index
    pub default_loop_length_index: usize,
    /// Default zoom level in bars
    pub default_zoom_bars: u32,
    /// Grid density in bars
    pub grid_bars: u32,
    /// Stem color palette name
    pub stem_color_palette: String,
}

impl Default for ExportableDisplayConfig {
    fn default() -> Self {
        Self {
            default_loop_length_index: 2,
            default_zoom_bars: 8,
            grid_bars: 8,
            stem_color_palette: "natural".to_string(),
        }
    }
}

/// Slicer configuration for export
///
/// Contains buffer size and preset definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportableSlicerConfig {
    /// Buffer size in bars
    pub buffer_bars: u32,
    /// Preset definitions (serialized from mesh-widgets SlicerConfig)
    pub presets: Vec<ExportableSlicerPreset>,
}

impl Default for ExportableSlicerConfig {
    fn default() -> Self {
        Self {
            buffer_bars: 4,
            presets: Vec::new(),
        }
    }
}

/// A slicer preset for export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportableSlicerPreset {
    /// Preset name
    pub name: String,
    /// Stem sequences (4 stems, each with 8 steps)
    pub stems: Vec<Option<ExportableStepSequence>>,
}

/// Step sequence for a single stem
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportableStepSequence {
    /// 8 steps in the sequence
    pub steps: Vec<ExportableStep>,
}

/// A single step in a sequence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportableStep {
    /// Whether this step is muted
    pub muted: bool,
    /// Slice indices for layering
    pub slices: Vec<u8>,
}

impl ExportableConfig {
    /// Load from YAML file
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Save to YAML file
    pub fn save(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ExportableConfig::default();
        assert_eq!(config.audio.global_bpm, 128.0);
        assert!(config.audio.phase_sync);
        assert_eq!(config.display.default_zoom_bars, 8);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = ExportableConfig {
            audio: ExportableAudioConfig {
                global_bpm: 140.0,
                phase_sync: false,
                ..Default::default()
            },
            display: ExportableDisplayConfig {
                default_zoom_bars: 16,
                stem_color_palette: "synthwave".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ExportableConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.audio.global_bpm, 140.0);
        assert!(!parsed.audio.phase_sync);
        assert_eq!(parsed.display.default_zoom_bars, 16);
        assert_eq!(parsed.display.stem_color_palette, "synthwave");
    }
}
