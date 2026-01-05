//! Global configuration for mesh-cue
//!
//! Configuration is stored as YAML alongside the collection folder.
//! Default location: ~/Music/mesh-collection/config.yaml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Analysis settings (BPM, key detection, etc.)
    pub analysis: AnalysisConfig,
    /// Display settings (waveform, grid, etc.)
    pub display: DisplayConfig,
    /// Track name format template
    ///
    /// Supports placeholders:
    /// - {artist}: Artist name parsed from filename
    /// - {name}: Track name parsed from filename
    ///
    /// Example: "{artist} - {name}" produces "Daft Punk - One More Time"
    pub track_name_format: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            analysis: AnalysisConfig::default(),
            display: DisplayConfig::default(),
            track_name_format: String::from("{artist} - {name}"),
        }
    }
}

/// Display configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Default beat grid density for overview waveform (4, 8, 16, or 32 bars)
    pub grid_bars: u32,
    /// Zoomed waveform zoom level (1-64 bars)
    pub zoom_bars: u32,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            grid_bars: 8,  // Default to 8 bars between grid lines
            zoom_bars: 8,  // Default zoomed waveform to 8 bars
        }
    }
}

/// Analysis configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisConfig {
    /// BPM detection settings
    pub bpm: BpmConfig,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            bpm: BpmConfig::default(),
        }
    }
}

/// BPM detection configuration
///
/// These values map directly to Essentia's RhythmExtractor2013 parameters:
/// - min_tempo: minimum expected BPM (40-180)
/// - max_tempo: maximum expected BPM (60-250)
///
/// Note: Essentia expects i32 for these parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BpmConfig {
    /// Minimum expected tempo in BPM (Essentia range: 40-180)
    pub min_tempo: i32,
    /// Maximum expected tempo in BPM (Essentia range: 60-250)
    pub max_tempo: i32,
}

impl Default for BpmConfig {
    fn default() -> Self {
        Self {
            min_tempo: 40,
            max_tempo: 208,
        }
    }
}

impl BpmConfig {
    /// Validate and clamp values to Essentia's supported ranges
    pub fn validate(&mut self) {
        // Essentia constraints: min_tempo in [40, 180], max_tempo in [60, 250]
        self.min_tempo = self.min_tempo.clamp(40, 180);
        self.max_tempo = self.max_tempo.clamp(60, 250);

        // Ensure min < max with at least 20 BPM gap
        if self.min_tempo >= self.max_tempo {
            self.max_tempo = (self.min_tempo + 20).min(250);
        }
    }

    /// Create config for a specific genre (e.g., DnB: 160-190)
    pub fn for_range(min: i32, max: i32) -> Self {
        let mut config = Self {
            min_tempo: min,
            max_tempo: max,
        };
        config.validate();
        config
    }
}

/// Get the default config file path
///
/// Returns: ~/Music/mesh-collection/config.yaml
pub fn default_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
        .join("config.yaml")
}

/// Load configuration from a YAML file
///
/// If the file doesn't exist, returns default config.
/// If the file exists but is invalid, logs a warning and returns default config.
pub fn load_config(path: &Path) -> Config {
    log::info!("load_config: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_config: Config file doesn't exist, using defaults");
        return Config::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<Config>(&contents) {
            Ok(mut config) => {
                config.analysis.bpm.validate();
                log::info!(
                    "load_config: Loaded config - BPM range: {}-{}",
                    config.analysis.bpm.min_tempo,
                    config.analysis.bpm.max_tempo
                );
                config
            }
            Err(e) => {
                log::warn!("load_config: Failed to parse config: {}, using defaults", e);
                Config::default()
            }
        },
        Err(e) => {
            log::warn!("load_config: Failed to read config file: {}, using defaults", e);
            Config::default()
        }
    }
}

/// Save configuration to a YAML file
///
/// Creates parent directories if they don't exist.
pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    log::info!("save_config: Saving to {:?}", path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }

    // Serialize to YAML
    let yaml = serde_yaml::to_string(config)
        .context("Failed to serialize config to YAML")?;

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
        let config = Config::default();
        assert_eq!(config.analysis.bpm.min_tempo, 40);
        assert_eq!(config.analysis.bpm.max_tempo, 208);
    }

    #[test]
    fn test_bpm_validation_clamps_values() {
        let mut bpm = BpmConfig {
            min_tempo: 30, // Below minimum
            max_tempo: 300, // Above maximum
        };
        bpm.validate();
        assert_eq!(bpm.min_tempo, 40);
        assert_eq!(bpm.max_tempo, 250);
    }

    #[test]
    fn test_bpm_validation_min_max_order() {
        let mut bpm = BpmConfig {
            min_tempo: 180,
            max_tempo: 100, // Less than min
        };
        bpm.validate();
        assert!(bpm.max_tempo > bpm.min_tempo);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = Config {
            analysis: AnalysisConfig {
                bpm: BpmConfig {
                    min_tempo: 160,
                    max_tempo: 190,
                },
            },
            track_name_format: String::from("{artist} - {name}"),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.analysis.bpm.min_tempo, 160);
        assert_eq!(parsed.analysis.bpm.max_tempo, 190);
    }
}
