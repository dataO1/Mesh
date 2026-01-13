//! Player configuration for mesh-player
//!
//! Configuration is stored as YAML in the user's config directory.
//! Default location: ~/.config/mesh-player/config.yaml

use anyhow::{Context, Result};
use iced::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Stem color palette selection
///
/// Maps to predefined palettes in mesh-widgets/src/theme.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StemColorPalette {
    /// Natural/Organic - soft, muted tones for extended viewing comfort
    #[default]
    Natural,
    /// Cool-to-Warm Depth - uses color temperature for natural depth perception
    CoolWarm,
    /// High Contrast - maximum hue separation for clarity
    HighContrast,
    /// Synthwave - modern DJ aesthetic with neon-inspired colors
    Synthwave,
    /// Gruvbox - retro warm palette with earthy, vintage tones
    Gruvbox,
}

impl StemColorPalette {
    /// Get all available palettes for UI selection
    pub const ALL: [StemColorPalette; 5] = [
        StemColorPalette::Natural,
        StemColorPalette::CoolWarm,
        StemColorPalette::HighContrast,
        StemColorPalette::Synthwave,
        StemColorPalette::Gruvbox,
    ];

    /// Get the display name for this palette
    pub fn display_name(&self) -> &'static str {
        match self {
            StemColorPalette::Natural => "Natural",
            StemColorPalette::CoolWarm => "Cool-Warm",
            StemColorPalette::HighContrast => "High Contrast",
            StemColorPalette::Synthwave => "Synthwave",
            StemColorPalette::Gruvbox => "Gruvbox",
        }
    }

    /// Get the color array for this palette
    ///
    /// Order: [Vocals, Drums, Bass, Other]
    pub fn colors(&self) -> [Color; 4] {
        use mesh_widgets::theme::stem_palettes;
        match self {
            StemColorPalette::Natural => stem_palettes::NATURAL,
            StemColorPalette::CoolWarm => stem_palettes::COOL_WARM,
            StemColorPalette::HighContrast => stem_palettes::HIGH_CONTRAST,
            StemColorPalette::Synthwave => stem_palettes::SYNTHWAVE,
            StemColorPalette::Gruvbox => stem_palettes::GRUVBOX,
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
    /// Loudness normalization settings
    pub loudness: LoudnessConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            global_bpm: 128.0, // Standard house/techno BPM
            phase_sync: true,  // Automatic beat sync enabled by default
            loudness: LoudnessConfig::default(),
        }
    }
}

/// Loudness normalization configuration
///
/// Controls automatic gain compensation to normalize tracks to a target loudness.
/// Uses LUFS values measured during import (EBU R128 integrated loudness).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessConfig {
    /// Target loudness in LUFS
    /// Tracks are gain-compensated to reach this level.
    /// Default: -9.0 LUFS (balanced loudness)
    pub target_lufs: f32,
    /// Enable automatic gain compensation based on track LUFS
    pub auto_gain_enabled: bool,
    /// Maximum boost in dB (safety limit for very quiet tracks)
    /// Default: 12.0 dB
    pub max_gain_db: f32,
    /// Maximum cut in dB (safety limit for very loud tracks)
    /// Default: -24.0 dB
    pub min_gain_db: f32,
}

impl Default for LoudnessConfig {
    fn default() -> Self {
        Self {
            target_lufs: -9.0,      // Balanced loudness
            auto_gain_enabled: true,
            max_gain_db: 12.0,      // Safety: max boost
            min_gain_db: -24.0,     // Safety: max cut
        }
    }
}

impl LoudnessConfig {
    /// Calculate gain compensation in dB for a track
    ///
    /// Returns None if LUFS is not available or auto-gain is disabled.
    pub fn calculate_gain_db(&self, track_lufs: Option<f32>) -> Option<f32> {
        if !self.auto_gain_enabled {
            return None;
        }
        track_lufs.map(|lufs| {
            (self.target_lufs - lufs).clamp(self.min_gain_db, self.max_gain_db)
        })
    }

    /// Calculate linear gain multiplier for a track
    ///
    /// Returns 1.0 (unity gain) if LUFS is not available or auto-gain is disabled.
    pub fn calculate_gain_linear(&self, track_lufs: Option<f32>) -> f32 {
        self.calculate_gain_db(track_lufs)
            .map(|db| 10.0_f32.powf(db / 20.0))
            .unwrap_or(1.0)
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
    /// Stem color palette for waveform display
    pub stem_color_palette: StemColorPalette,
}

/// Loop length options in beats (matches mesh-core/deck.rs LOOP_LENGTHS)
/// Range: 1 beat to 64 bars (256 beats)
pub const LOOP_LENGTH_OPTIONS: [f64; 9] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            default_loop_length_index: 2, // Default to 4 beats (index 2 in LOOP_LENGTH_OPTIONS)
            default_zoom_bars: 8,         // Default zoomed waveform to 8 bars
            grid_bars: 8,                 // Default grid density to 8 bars
            stem_color_palette: StemColorPalette::default(), // Natural palette
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

/// Slicer configuration section
///
/// Controls the stem slicer feature that allows real-time remixing
/// by rearranging slice playback order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlicerConfig {
    /// Default buffer size in bars (1, 4, 8, or 16)
    /// Smaller = more responsive, larger = more material to work with
    pub default_buffer_bars: u32,
    /// Which stems are affected by the slicer [Vocals, Drums, Bass, Other]
    /// Default: only Drums enabled
    pub affected_stems: [bool; 4],
    /// 8 preset patterns for breakbeat manipulation (sorted sparse → busy)
    /// Button 0-7 loads preset. Each pattern has 16 steps (slice indices 0-15).
    pub presets: [[u8; 16]; 8],
}

impl Default for SlicerConfig {
    fn default() -> Self {
        Self {
            default_buffer_bars: 4, // 4 bars = 16 slices (one per 16th note)
            affected_stems: [false, true, false, false], // Only Drums by default
            // Breakbeat presets: all use all 16 slices for full variety
            // Each preset is a permutation ensuring all beats are heard
            presets: [
                // Preset 1: Sequential (default/reset)
                [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                // Preset 2: Bar swap (play bar 3-4 then bar 1-2)
                [8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7],
                // Preset 3: Reverse full
                [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
                // Preset 4: Reverse per bar
                [3, 2, 1, 0, 7, 6, 5, 4, 11, 10, 9, 8, 15, 14, 13, 12],
                // Preset 5: Interleave (zip bar1+bar3, bar2+bar4)
                [0, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15],
                // Preset 6: Funky shuffle
                [0, 5, 2, 7, 4, 1, 6, 3, 8, 13, 10, 15, 12, 9, 14, 11],
                // Preset 7: Evens then odds (skip shuffle)
                [0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15],
                // Preset 8: Adjacent swap (pair flip)
                [1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14],
            ],
        }
    }
}

impl SlicerConfig {
    /// Get buffer bars clamped to valid range
    pub fn buffer_bars(&self) -> u32 {
        match self.default_buffer_bars {
            1 | 4 | 8 | 16 => self.default_buffer_bars,
            _ => 4, // Default to 4 if invalid
        }
    }

    /// Get a preset pattern by index (0-7)
    pub fn preset(&self, index: usize) -> Option<[u8; 16]> {
        self.presets.get(index).copied()
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
        assert_eq!(config.display.default_loop_length_index, 2);
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
                ..Default::default()
            },
            display: DisplayConfig {
                default_loop_length_index: 5, // 8 beats
                default_zoom_bars: 4,
                grid_bars: 16,
                ..Default::default()
            },
            slicer: SlicerConfig {
                default_buffer_bars: 8,
                ..Default::default()
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
