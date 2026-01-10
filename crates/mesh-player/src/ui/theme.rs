//! Theme configuration for mesh-player
//!
//! Provides configurable colors for stems, cues, and other visual elements.
//! Configuration is stored as YAML in the user's config directory.
//! Default location: ~/.config/mesh-player/theme.yaml

use iced::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Global theme instance (initialized once at startup)
static THEME: OnceLock<ThemeConfig> = OnceLock::new();

/// Root theme configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    /// Stem colors for waveform display
    pub stems: StemColors,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            stems: StemColors::default(),
        }
    }
}

/// Stem color configuration
///
/// Colors are specified as hex strings (e.g., "#33CC66")
/// Stem order: [Vocals, Drums, Bass, Other]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StemColors {
    /// Vocals stem color (default: green)
    pub vocals: String,
    /// Drums stem color (default: yellow/orange)
    pub drums: String,
    /// Bass stem color (default: red)
    pub bass: String,
    /// Other stem color (default: cyan)
    pub other: String,
}

impl Default for StemColors {
    fn default() -> Self {
        Self {
            vocals: "#33CC66".to_string(), // Green
            drums: "#CC3333".to_string(),  // Dark Red
            bass: "#E6604D".to_string(),   // Orange-Red
            other: "#00CCCC".to_string(),  // Cyan
        }
    }
}

impl StemColors {
    /// Get colors as array [Vocals, Drums, Bass, Other] for waveform rendering
    pub fn as_array(&self) -> [Color; 4] {
        [
            parse_hex_color(&self.vocals),
            parse_hex_color(&self.drums),
            parse_hex_color(&self.bass),
            parse_hex_color(&self.other),
        ]
    }
}

/// Parse a hex color string to an iced Color
///
/// Supports formats: "#RRGGBB" or "RRGGBB"
/// Returns white on parse failure
fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        log::warn!("Invalid hex color '{}', using white", hex);
        return Color::WHITE;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);

    Color::from_rgb8(r, g, b)
}

/// Default fallback stem colors (matches StemColors::default())
pub const DEFAULT_STEM_COLORS: [Color; 4] = [
    Color::from_rgb(0.2, 0.8, 0.4),   // Vocals - Green (#33CC66)
    Color::from_rgb(0.8, 0.2, 0.2),   // Drums - Dark Red (#CC3333)
    Color::from_rgb(0.9, 0.38, 0.3),  // Bass - Orange-Red (#E6604D)
    Color::from_rgb(0.0, 0.8, 0.8),   // Other - Cyan (#00CCCC)
];

/// Get the default theme file path
///
/// Returns: ~/.config/mesh-player/theme.yaml
pub fn default_theme_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("mesh-player")
        .join("theme.yaml")
}

/// Load theme configuration from a YAML file
///
/// If the file doesn't exist, returns default config.
/// If the file exists but is invalid, logs a warning and returns default config.
pub fn load_theme(path: &Path) -> ThemeConfig {
    log::info!("load_theme: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_theme: Theme file doesn't exist, using defaults");
        return ThemeConfig::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<ThemeConfig>(&contents) {
            Ok(config) => {
                log::info!(
                    "load_theme: Loaded theme - Vocals: {}, Drums: {}, Bass: {}, Other: {}",
                    config.stems.vocals,
                    config.stems.drums,
                    config.stems.bass,
                    config.stems.other
                );
                config
            }
            Err(e) => {
                log::warn!("load_theme: Failed to parse theme: {}, using defaults", e);
                ThemeConfig::default()
            }
        },
        Err(e) => {
            log::warn!(
                "load_theme: Failed to read theme file: {}, using defaults",
                e
            );
            ThemeConfig::default()
        }
    }
}

/// Initialize the global theme from config file (call once at startup)
pub fn init_theme() {
    let path = default_theme_path();
    let config = load_theme(&path);
    if THEME.set(config).is_err() {
        log::warn!("Theme already initialized");
    }
}

/// Get stem colors array [Vocals, Drums, Bass, Other]
///
/// Returns configured colors from theme.yaml, or defaults if not initialized.
pub fn stem_colors() -> [Color; 4] {
    THEME
        .get()
        .map(|t| t.stems.as_array())
        .unwrap_or(DEFAULT_STEM_COLORS)
}

/// Stem names for UI display
pub const STEM_NAMES: [&str; 4] = ["Vocals", "Drums", "Bass", "Other"];

/// Short stem names for compact display
pub const STEM_NAMES_SHORT: [&str; 4] = ["Vox", "Drm", "Bas", "Oth"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        let color = parse_hex_color("#FF0000");
        assert_eq!(color.r, 1.0);
        assert_eq!(color.g, 0.0);
        assert_eq!(color.b, 0.0);

        let color = parse_hex_color("00FF00");
        assert_eq!(color.r, 0.0);
        assert_eq!(color.g, 1.0);
        assert_eq!(color.b, 0.0);
    }

    #[test]
    fn test_default_stem_colors() {
        let config = ThemeConfig::default();
        let colors = config.stems.as_array();
        assert_eq!(colors.len(), 4);
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = ThemeConfig {
            stems: StemColors {
                vocals: "#00FF00".to_string(),
                drums: "#FFFF00".to_string(),
                bass: "#FF0000".to_string(),
                other: "#00FFFF".to_string(),
            },
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: ThemeConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.stems.vocals, "#00FF00");
        assert_eq!(parsed.stems.drums, "#FFFF00");
        assert_eq!(parsed.stems.bass, "#FF0000");
        assert_eq!(parsed.stems.other, "#00FFFF");
    }
}
