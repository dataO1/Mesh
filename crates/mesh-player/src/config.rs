//! Player configuration for mesh-player
//!
//! Configuration is stored as YAML in the mesh collection folder.
//! Default location: ~/Music/mesh-collection/player-config.yaml

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Re-export shared config utilities from mesh-core
pub use mesh_core::config::{load_config, save_config, LoudnessConfig};
pub use mesh_widgets::{AppFont, FontSize};

/// Key scoring model for harmonic compatibility
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyScoringModel {
    /// Camelot wheel distance with hand-tuned transition scores
    #[default]
    Camelot,
    /// Krumhansl-Kessler probe-tone profile correlations
    Krumhansl,
}

impl KeyScoringModel {
    pub const ALL: [KeyScoringModel; 2] = [
        KeyScoringModel::Camelot,
        KeyScoringModel::Krumhansl,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            KeyScoringModel::Camelot => "Camelot",
            KeyScoringModel::Krumhansl => "Krumhansl",
        }
    }
}

/// Waveform layout orientation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveformLayout {
    /// Traditional horizontal layout (time flows left-to-right, 2x2 grid)
    #[default]
    Horizontal,
    /// Vertical layout (time flows top-to-bottom, overviews centered)
    Vertical,
    /// Vertical layout with inverted Y axis (time flows bottom-to-top)
    VerticalInverted,
}

impl WaveformLayout {
    pub const ALL: [WaveformLayout; 3] = [
        WaveformLayout::Horizontal,
        WaveformLayout::Vertical,
        WaveformLayout::VerticalInverted,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            WaveformLayout::Horizontal => "Horizontal",
            WaveformLayout::Vertical => "Vertical",
            WaveformLayout::VerticalInverted => "Vert. Inverted",
        }
    }

    /// Whether this layout uses vertical orientation (either direction)
    pub fn is_vertical(&self) -> bool {
        matches!(self, WaveformLayout::Vertical | WaveformLayout::VerticalInverted)
    }

    /// Whether the Y axis is inverted (time flows bottom-to-top)
    pub fn is_inverted(&self) -> bool {
        matches!(self, WaveformLayout::VerticalInverted)
    }
}

/// Waveform abstraction level (controls grid-aligned subsampling intensity)
///
/// Higher abstraction = larger grid cells = smoother/more abstract appearance.
/// Each level scales the per-stem subsampling targets uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveformAbstraction {
    /// Minimal subsampling, sharpest detail
    Low,
    /// Balanced smoothing (default)
    Medium,
    /// Heavy subsampling, most abstract/smooth
    High,
}

impl Default for WaveformAbstraction {
    fn default() -> Self {
        WaveformAbstraction::Medium
    }
}

impl WaveformAbstraction {
    pub const ALL: [WaveformAbstraction; 3] = [
        WaveformAbstraction::Low,
        WaveformAbstraction::Medium,
        WaveformAbstraction::High,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            WaveformAbstraction::Low => "Low",
            WaveformAbstraction::Medium => "Medium",
            WaveformAbstraction::High => "High",
        }
    }

    /// Convert to u8 level for shader uniforms (avoids cross-crate type dependency)
    pub fn as_level(&self) -> u8 {
        match self {
            WaveformAbstraction::Low => 0,
            WaveformAbstraction::Medium => 1,
            WaveformAbstraction::High => 2,
        }
    }
}

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerConfig {
    /// Audio settings (global BPM, latency preferences)
    pub audio: AudioConfig,
    /// Display settings (waveform, zoom levels)
    pub display: DisplayConfig,
    /// Slicer settings (buffer size, queue algorithm)
    pub slicer: SlicerConfig,
    /// OTA update settings
    pub updates: UpdateConfig,
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
            slicer: SlicerConfig::default(),
            updates: UpdateConfig::default(),
            collection_path,
        }
    }
}

/// OTA update configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// When true, include release candidates and beta versions in OTA checks
    pub prerelease_channel: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            prerelease_channel: false,
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
    /// Auto-cue: automatically route low-volume decks to headphone/cue output.
    /// Decks at ≤30% volume are fully sent to cue bus; 30–50% fades out linearly.
    /// Only effective when master and cue outputs are different devices.
    pub auto_cue: bool,
    /// Loudness normalization settings
    pub loudness: LoudnessConfig,
    /// Audio output device configuration
    pub outputs: AudioOutputConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            global_bpm: 128.0, // Standard house/techno BPM
            phase_sync: true,  // Automatic beat sync enabled by default
            auto_cue: true,    // Auto-cue enabled by default
            loudness: LoudnessConfig::default(),
            outputs: AudioOutputConfig::default(),
        }
    }
}

/// Audio output device configuration
///
/// Configures which audio devices are used for master (speakers)
/// and cue (headphones) output. Uses device indices from the available
/// devices list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AudioOutputConfig {
    /// Master output device index (0 = first device, 1 = second, etc.)
    /// None = use system default device
    pub master_device: Option<usize>,
    /// Cue/headphone output device index
    /// None = use system default (same device as master, different channels if possible)
    pub cue_device: Option<usize>,
    /// Preferred buffer size in frames (64, 128, 256, 512, 1024, etc.)
    /// None = use system default (~512 frames)
    pub buffer_size: Option<u32>,
}

// LoudnessConfig is re-exported from mesh_core::config

/// Display configuration section
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// Default loop length index (0-6 maps to 0.25, 0.5, 1, 2, 4, 8, 16 beats)
    pub default_loop_length_index: usize,
    /// Default zoom level for zoomed waveform (in bars)
    pub default_zoom_bars: u32,
    /// Overview waveform grid density in beats (8, 16, 32, or 64)
    pub grid_bars: u32,
    /// Active theme name (references a theme from theme.yaml)
    pub theme: String,
    /// Show local collection in browser (default: false for USB-only mode)
    /// When false, only USB devices appear in the collection browser
    pub show_local_collection: bool,
    /// Key scoring model for harmonic compatibility in smart suggestions
    pub key_scoring_model: KeyScoringModel,
    /// Waveform layout orientation (horizontal or vertical)
    pub waveform_layout: WaveformLayout,
    /// Waveform abstraction level (Low/Medium/High grid-aligned subsampling)
    pub waveform_abstraction: WaveformAbstraction,
    /// UI font (requires restart to apply)
    pub font: AppFont,
    /// Font size preset (Small / Medium / Big)
    pub font_size: FontSize,
    /// Keep browser overlay visible while browse mode is active (disable auto-hide timeout)
    pub persistent_browse: bool,
}

/// Loop length options in beats (matches mesh-core/deck.rs LOOP_LENGTHS)
/// Range: 1/8 beat to 64 bars (256 beats)
pub const LOOP_LENGTH_OPTIONS: [f64; 12] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_loop_length_index: 7, // Default to 16 beats (index 7 in LOOP_LENGTH_OPTIONS)
            default_zoom_bars: 8,         // Default zoomed waveform to 8 bars
            grid_bars: 32,                // Default grid density to 32 beats (8 bars)
            theme: "Mesh".to_string(), // Default theme (from theme.yaml)
            show_local_collection: false, // USB-only mode by default
            key_scoring_model: KeyScoringModel::default(), // Camelot wheel
            waveform_layout: WaveformLayout::default(),  // Horizontal
            waveform_abstraction: WaveformAbstraction::default(), // Medium
            font: AppFont::default(), // Exo
            font_size: FontSize::default(), // Small
            persistent_browse: false, // Auto-hide browser overlay by default
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

// Re-export shared slicer config types from mesh-widgets
pub use mesh_widgets::SlicerConfig;

/// Convert mesh-widgets SlicerConfig to engine SlicerPreset array
///
/// This converts the rich per-stem pattern format (with muting, layering)
/// into the engine's runtime format.
pub fn slicer_config_to_engine_presets(
    config: &SlicerConfig,
) -> [mesh_core::engine::SlicerPreset; 8] {
    use mesh_core::engine::{SliceStep, SlicerPreset, StepSequence, MAX_SLICE_LAYERS, MUTED_SLICE};

    std::array::from_fn(|preset_idx| {
        let preset_config = &config.presets[preset_idx];
        SlicerPreset {
            stems: std::array::from_fn(|stem_idx| {
                preset_config.stems[stem_idx].as_ref().map(|seq_config| {
                    StepSequence {
                        steps: std::array::from_fn(|step_idx| {
                            let step_config = &seq_config.steps[step_idx];

                            if step_config.muted || step_config.slices.is_empty() {
                                // Muted step: all layers silent
                                SliceStep {
                                    slices: [MUTED_SLICE; MAX_SLICE_LAYERS],
                                    velocities: [0.0; MAX_SLICE_LAYERS],
                                }
                            } else {
                                // Active step: convert Vec<u8> to fixed array
                                let mut slices = [MUTED_SLICE; MAX_SLICE_LAYERS];
                                let mut velocities = [0.0; MAX_SLICE_LAYERS];

                                for (layer, &slice_idx) in
                                    step_config.slices.iter().take(MAX_SLICE_LAYERS).enumerate()
                                {
                                    slices[layer] = slice_idx;
                                    velocities[layer] = 1.0; // Full velocity
                                }

                                SliceStep { slices, velocities }
                            }
                        }),
                    }
                })
            }),
        }
    })
}

// default_collection_path is re-exported from mesh_core::config

/// Config filename for mesh-player
pub const CONFIG_FILENAME: &str = "player-config.yaml";

/// Get the default config file path
///
/// Returns: ~/Music/mesh-collection/player-config.yaml
pub fn default_config_path() -> PathBuf {
    mesh_core::config::default_config_path(CONFIG_FILENAME)
}

// load_config and save_config are re-exported from mesh_core::config
// Usage: load_config::<PlayerConfig>(&path) and save_config(&config, &path)

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PlayerConfig::default();
        assert_eq!(config.audio.global_bpm, 128.0);
        assert!(config.audio.phase_sync);
        assert_eq!(config.display.default_loop_length_index, 7);
    }

    #[test]
    fn test_default_loop_length() {
        let display = DisplayConfig::default();
        assert_eq!(display.default_loop_length_beats(), 16.0);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = PlayerConfig {
            audio: AudioConfig {
                global_bpm: 140.0,
                phase_sync: false,
                ..Default::default()
            },
            display: DisplayConfig {
                default_loop_length_index: 5, // 8 beats
                default_zoom_bars: 4,
                grid_bars: 64,
                ..Default::default()
            },
            slicer: SlicerConfig {
                buffer_bars: 8,
                ..Default::default()
            },
            updates: UpdateConfig::default(),
            collection_path: PathBuf::from("/tmp/test-collection"),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: PlayerConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.audio.global_bpm, 140.0);
        assert!(!parsed.audio.phase_sync);
        assert_eq!(parsed.display.default_loop_length_index, 5);
        assert_eq!(parsed.display.default_zoom_bars, 4);
    }

    #[test]
    fn test_loudness_config() {
        let config = LoudnessConfig::default();
        assert_eq!(config.target_lufs, -9.0);
        assert!(config.auto_gain_enabled);

        // Test gain calculation
        // Track at -12 LUFS, target -9 LUFS = +3 dB boost
        let gain_db = config.calculate_gain_db(Some(-12.0)).unwrap();
        assert!((gain_db - 3.0).abs() < 0.001);

        // Track at -4 LUFS, target -9 LUFS = -5 dB cut
        let gain_db = config.calculate_gain_db(Some(-4.0)).unwrap();
        assert!((gain_db - (-5.0)).abs() < 0.001);

        // No LUFS = no gain
        assert!(config.calculate_gain_db(None).is_none());
        assert_eq!(config.calculate_gain_linear(None), 1.0);
    }
}
