//! Player configuration for mesh-player
//!
//! Configuration is stored as YAML in the user's config directory.
//! Default location: ~/.config/mesh-player/config.yaml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerConfig {
    /// Audio settings (global BPM, latency preferences)
    pub audio: AudioConfig,
    /// Display settings (waveform, zoom levels)
    pub display: DisplayConfig,
    /// Path to the mesh collection folder (shared with mesh-cue)
    /// Default: ~/Music/mesh-collection
    pub collection_path: PathBuf,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        // Default collection path matches mesh-cue
        let collection_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection");

        Self {
            audio: AudioConfig::default(),
            display: DisplayConfig::default(),
            collection_path,
        }
    }
}

/// Audio configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Global BPM for time-stretching (saved/restored between sessions)
    pub global_bpm: f64,
    /// Enable automatic inter-deck phase synchronization
    /// When enabled, pressing play or hot cues automatically aligns
    /// to the master deck's beat phase
    pub phase_sync: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            global_bpm: 128.0, // Standard house/techno BPM
            phase_sync: true,  // Automatic beat sync enabled by default
        }
    }
}

/// Display configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Default loop length index (0-6 maps to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    pub default_loop_length_index: usize,
    /// Default zoom level for zoomed waveform (in bars)
    pub default_zoom_bars: u32,
    /// Overview waveform grid density (4, 8, 16, or 32 bars)
    pub grid_bars: u32,
}

/// Loop length options in beats (matches mesh-core/deck.rs LOOP_LENGTHS)
pub const LOOP_LENGTH_OPTIONS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_loop_length_index: 4, // Default to 4 beats (index 4 in LOOP_LENGTH_OPTIONS)
            default_zoom_bars: 8,         // Default zoomed waveform to 8 bars
            grid_bars: 8,                 // Default grid density to 8 bars
        }
    }
}

impl DisplayConfig {
    /// Get the default loop length in beats
    pub fn default_loop_length_beats(&self) -> f64 {
        LOOP_LENGTH_OPTIONS
            .get(self.default_loop_length_index)
            .copied()
            .unwrap_or(4.0)
    }
}

/// Get the default config file path
///
/// Returns: ~/.config/mesh-player/config.yaml
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("mesh-player")
        .join("config.yaml")
}

/// Load configuration from a YAML file
///
/// If the file doesn't exist, returns default config.
/// If the file exists but is invalid, logs a warning and returns default config.
pub fn load_config(path: &Path) -> PlayerConfig {
    log::info!("load_config: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_config: Config file doesn't exist, using defaults");
        return PlayerConfig::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<PlayerConfig>(&contents) {
            Ok(config) => {
                log::info!(
                    "load_config: Loaded config - Global BPM: {:.1}, Phase sync: {}, Loop length idx: {}",
                    config.audio.global_bpm,
                    config.audio.phase_sync,
                    config.display.default_loop_length_index
                );
                config
            }
            Err(e) => {
                log::warn!("load_config: Failed to parse config: {}, using defaults", e);
                PlayerConfig::default()
            }
        },
        Err(e) => {
            log::warn!(
                "load_config: Failed to read config file: {}, using defaults",
                e
            );
            PlayerConfig::default()
        }
    }
}

/// Save configuration to a YAML file
///
/// Creates parent directories if they don't exist.
pub fn save_config(config: &PlayerConfig, path: &Path) -> Result<()> {
    log::info!("save_config: Saving to {:?}", path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }

    // Serialize to YAML
    let yaml =
        serde_yaml::to_string(config).context("Failed to serialize config to YAML")?;

    // Write to file
    std::fs::write(path, yaml)
        .with_context(|| format!("Failed to write config file: {:?}", path))?;

    log::info!("save_config: Config saved successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PlayerConfig::default();
        assert_eq!(config.audio.global_bpm, 128.0);
        assert!(config.audio.phase_sync);
        assert_eq!(config.display.default_loop_length_index, 4);
    }

    #[test]
    fn test_default_loop_length() {
        let display = DisplayConfig::default();
        assert_eq!(display.default_loop_length_beats(), 4.0);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = PlayerConfig {
            audio: AudioConfig {
                global_bpm: 140.0,
                phase_sync: false,
            },
            display: DisplayConfig {
                default_loop_length_index: 5, // 8 beats
                default_zoom_bars: 4,
                grid_bars: 16,
            },
            collection_path: PathBuf::from("/tmp/test-collection"),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: PlayerConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.audio.global_bpm, 140.0);
        assert!(!parsed.audio.phase_sync);
        assert_eq!(parsed.display.default_loop_length_index, 5);
        assert_eq!(parsed.display.default_zoom_bars, 4);
    }
}
