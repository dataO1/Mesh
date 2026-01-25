//! Global configuration for mesh-cue
//!
//! Configuration is stored as YAML alongside the collection folder.
//! Default location: ~/Music/mesh-collection/config.yaml

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// Re-export shared config utilities from mesh-core
// Note: load_config is NOT re-exported - we have a local wrapper that validates
pub use mesh_core::config::{
    default_collection_path, save_config,
    LoudnessConfig as CoreLoudnessConfig,
};

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Analysis settings (BPM, key detection, etc.)
    pub analysis: AnalysisConfig,
    /// Display settings (waveform, grid, etc.)
    pub display: DisplayConfig,
    /// Audio output settings
    pub audio: AudioConfig,
    /// Track name format template
    ///
    /// Supports placeholders:
    /// - {artist}: Artist name parsed from filename
    /// - {name}: Track name parsed from filename
    ///
    /// Example: "{artist} - {name}" produces "Daft Punk - One More Time"
    pub track_name_format: String,
    /// Slicer presets (8 presets, each with 4 stem patterns)
    pub slicer: SlicerConfig,
}

/// Audio output configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AudioConfig {
    /// Output device index (0 = first device, 1 = second, etc.)
    /// None = use system default device
    pub output_device: Option<usize>,
    /// Scratch interpolation method for waveform scrubbing
    /// Linear = fast, acceptable quality; Cubic = better quality, more CPU
    pub scratch_interpolation: mesh_core::engine::InterpolationMethod,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            analysis: AnalysisConfig::default(),
            display: DisplayConfig::default(),
            audio: AudioConfig::default(),
            track_name_format: String::from("{artist} - {name}"),
            slicer: SlicerConfig::default(),
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

// ─────────────────────────────────────────────────────────────────────────────
// Slicer Configuration (re-exported from mesh-widgets)
// ─────────────────────────────────────────────────────────────────────────────

pub use mesh_widgets::{SlicerConfig, SlicerPresetConfig, SlicerSequenceConfig, SlicerStepConfig};

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

/// Loudness normalization configuration (re-exported from mesh-core)
///
/// Controls automatic gain compensation for track normalization.
/// LUFS is measured during analysis, and gain is calculated at export/playback time.
///
/// Note: Use `calculate_gain_db_direct(lufs)` for non-optional gain calculation.
pub type LoudnessConfig = CoreLoudnessConfig;

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

/// Config filename for mesh-cue
pub const CONFIG_FILENAME: &str = "config.yaml";

/// Get the default config file path
///
/// Returns: ~/Music/mesh-collection/config.yaml
pub fn default_config_path() -> PathBuf {
    mesh_core::config::default_config_path(CONFIG_FILENAME)
}

/// Load configuration from a YAML file with validation
///
/// Uses the generic loader from mesh-core, then validates analysis settings.
/// If the file doesn't exist or is invalid, returns default config.
pub fn load_config(path: &Path) -> Config {
    let mut config: Config = mesh_core::config::load_config(path);
    config.analysis.validate();
    log::info!(
        "load_config: Loaded config - BPM range: {}-{}, parallel: {}",
        config.analysis.bpm.min_tempo,
        config.analysis.bpm.max_tempo,
        config.analysis.parallel_processes
    );
    config
}

// save_config is re-exported from mesh_core::config

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
            audio: AudioConfig::default(),
            track_name_format: String::from("{artist} - {name}"),
            slicer: SlicerConfig::default(),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.analysis.bpm.min_tempo, 160);
        assert_eq!(parsed.analysis.bpm.max_tempo, 190);
    }
}
