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
    /// Global BPM for playback (saved/restored between sessions)
    pub global_bpm: f64,
    /// Default loop length index (0-6 maps to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    pub default_loop_length_index: usize,
}

/// Loop length options in beats
pub const LOOP_LENGTH_OPTIONS: [f32; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            grid_bars: 8,       // Default to 8 bars between grid lines
            zoom_bars: 8,       // Default zoomed waveform to 8 bars
            global_bpm: 128.0,  // Standard house/techno BPM
            default_loop_length_index: 4,  // Default to 4 beats (index 4 in LOOP_LENGTH_OPTIONS)
        }
    }
}

impl DisplayConfig {
    /// Get the default loop length in beats
    pub fn default_loop_length_beats(&self) -> f32 {
        LOOP_LENGTH_OPTIONS.get(self.default_loop_length_index)
            .copied()
            .unwrap_or(4.0)
    }
}

/// Analysis configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisConfig {
    /// BPM detection settings
    pub bpm: BpmConfig,
    /// Loudness normalization settings (for export-time waveform scaling)
    pub loudness: LoudnessConfig,
    /// Number of parallel analysis processes (1-16)
    ///
    /// Each track is analyzed in a separate subprocess (procspawn) because
    /// Essentia's C++ library is not thread-safe. This controls how many
    /// subprocesses run concurrently during batch import.
    pub parallel_processes: u8,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            bpm: BpmConfig::default(),
            loudness: LoudnessConfig::default(),
            parallel_processes: 4,
        }
    }
}

impl AnalysisConfig {
    /// Validate and clamp parallel_processes to valid range (1-16)
    pub fn validate(&mut self) {
        self.parallel_processes = self.parallel_processes.clamp(1, 16);
        self.bpm.validate();
    }
}

/// Loudness normalization configuration
///
/// Controls automatic gain compensation for track normalization.
/// LUFS is measured during analysis, and gain is calculated at export/playback time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessConfig {
    /// Target loudness in LUFS for waveform preview scaling
    /// Default: -9.0 LUFS (balanced loudness)
    pub target_lufs: f32,
    /// Maximum boost in dB (safety limit for very quiet tracks)
    pub max_gain_db: f32,
    /// Maximum cut in dB (safety limit for very loud tracks)
    pub min_gain_db: f32,
}

impl Default for LoudnessConfig {
    fn default() -> Self {
        Self {
            target_lufs: -9.0,
            max_gain_db: 12.0,
            min_gain_db: -24.0,
        }
    }
}

impl LoudnessConfig {
    /// Calculate gain compensation in dB for a track
    pub fn calculate_gain_db(&self, track_lufs: f32) -> f32 {
        (self.target_lufs - track_lufs).clamp(self.min_gain_db, self.max_gain_db)
    }

    /// Calculate linear gain multiplier for a track
    pub fn calculate_gain_linear(&self, track_lufs: f32) -> f32 {
        let db = self.calculate_gain_db(track_lufs);
        10.0_f32.powf(db / 20.0)
    }
}

/// Source audio for BPM detection
///
/// Determines which audio is used for tempo analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BpmSource {
    /// Use drums stem only (clearest beat, recommended for most music)
    #[default]
    Drums,
    /// Use full mix (all stems combined)
    FullMix,
}

impl std::fmt::Display for BpmSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BpmSource::Drums => write!(f, "Drums Only"),
            BpmSource::FullMix => write!(f, "Full Mix"),
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
    /// Which audio source to use for BPM detection
    pub source: BpmSource,
}

impl Default for BpmConfig {
    fn default() -> Self {
        Self {
            min_tempo: 40,
            max_tempo: 208,
            source: BpmSource::default(),
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
            source: BpmSource::default(),
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
                config.analysis.validate();
                log::info!(
                    "load_config: Loaded config - BPM range: {}-{}, parallel: {}",
                    config.analysis.bpm.min_tempo,
                    config.analysis.bpm.max_tempo,
                    config.analysis.parallel_processes
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
            source: BpmSource::default(),
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
            source: BpmSource::default(),
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
                    source: BpmSource::Drums,
                },
                loudness: LoudnessConfig::default(),
                parallel_processes: 4,
            },
            display: DisplayConfig::default(),
            track_name_format: String::from("{artist} - {name}"),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.analysis.bpm.min_tempo, 160);
        assert_eq!(parsed.analysis.bpm.max_tempo, 190);
    }
}
