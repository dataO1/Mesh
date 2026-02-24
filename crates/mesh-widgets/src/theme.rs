//! Shared theme system for mesh UI components
//!
//! Themes are loaded from `theme.yaml` in the mesh collection folder.
//! Each theme defines UI colors (for iced widgets) and stem waveform colors.
//! Users can add custom themes by editing the YAML file.
//!
//! Default location: `~/Music/mesh-collection/theme.yaml`

use iced::Color;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::track_table::parse_hex_color;

// ── Runtime Types ──────────────────────────────────────────────────────────

/// A complete mesh theme with UI and stem colors
#[derive(Debug, Clone)]
pub struct MeshTheme {
    /// Display name shown in settings UI
    pub name: String,
    /// UI colors for iced widgets (background, text, accent, etc.)
    pub ui: UiColors,
    /// Waveform stem colors [Vocals, Drums, Bass, Other]
    pub stems: [Color; 4],
}

/// UI color palette — maps to iced's `theme::Palette`
#[derive(Debug, Clone)]
pub struct UiColors {
    /// App background color
    pub background: Color,
    /// Default text color
    pub text: Color,
    /// Primary accent color (buttons, highlights, selections)
    pub accent: Color,
    /// Success state color
    pub success: Color,
    /// Warning state color
    pub warning: Color,
    /// Danger/error state color
    pub danger: Color,
}

impl MeshTheme {
    /// Build an iced `Theme` from this mesh theme
    pub fn iced_theme(&self) -> iced::Theme {
        iced::Theme::custom(self.name.clone(), self.ui.to_palette())
    }

    /// Get stem colors array [Vocals, Drums, Bass, Other]
    pub fn stem_colors(&self) -> [Color; 4] {
        self.stems
    }
}

impl UiColors {
    /// Convert to iced's `theme::Palette`
    pub fn to_palette(&self) -> iced::theme::Palette {
        iced::theme::Palette {
            background: self.background,
            text: self.text,
            primary: self.accent,
            success: self.success,
            warning: self.warning,
            danger: self.danger,
        }
    }
}

// ── Serialization Types (hex strings for human-readable YAML) ──────────────

#[derive(Debug, Serialize, Deserialize)]
struct ThemeFile {
    themes: Vec<ThemeEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ThemeEntry {
    name: String,
    ui: UiColorsConfig,
    stems: StemColorsConfig,
}

#[derive(Debug, Serialize, Deserialize)]
struct UiColorsConfig {
    background: String,
    text: String,
    accent: String,
    success: String,
    warning: String,
    danger: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StemColorsConfig {
    vocals: String,
    drums: String,
    bass: String,
    other: String,
}

// ── Conversion ─────────────────────────────────────────────────────────────

fn hex(s: &str) -> Color {
    parse_hex_color(s).unwrap_or(Color::WHITE)
}

fn color_to_hex(c: Color) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    )
}

impl From<ThemeEntry> for MeshTheme {
    fn from(e: ThemeEntry) -> Self {
        MeshTheme {
            name: e.name,
            ui: UiColors {
                background: hex(&e.ui.background),
                text: hex(&e.ui.text),
                accent: hex(&e.ui.accent),
                success: hex(&e.ui.success),
                warning: hex(&e.ui.warning),
                danger: hex(&e.ui.danger),
            },
            stems: [
                hex(&e.stems.vocals),
                hex(&e.stems.drums),
                hex(&e.stems.bass),
                hex(&e.stems.other),
            ],
        }
    }
}

impl From<&MeshTheme> for ThemeEntry {
    fn from(t: &MeshTheme) -> Self {
        ThemeEntry {
            name: t.name.clone(),
            ui: UiColorsConfig {
                background: color_to_hex(t.ui.background),
                text: color_to_hex(t.ui.text),
                accent: color_to_hex(t.ui.accent),
                success: color_to_hex(t.ui.success),
                warning: color_to_hex(t.ui.warning),
                danger: color_to_hex(t.ui.danger),
            },
            stems: StemColorsConfig {
                vocals: color_to_hex(t.stems[0]),
                drums: color_to_hex(t.stems[1]),
                bass: color_to_hex(t.stems[2]),
                other: color_to_hex(t.stems[3]),
            },
        }
    }
}

// ── Loading & Saving ───────────────────────────────────────────────────────

/// Load themes from a YAML file.
///
/// If the file doesn't exist, creates it with the default themes.
/// If parsing fails, logs a warning and returns default themes.
pub fn load_themes(path: &Path) -> Vec<MeshTheme> {
    if !path.exists() {
        log::info!("Theme file not found at {:?}, creating defaults", path);
        let defaults = default_themes();
        if let Err(e) = save_themes(&defaults, path) {
            log::warn!("Failed to write default theme file: {}", e);
        }
        return defaults;
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<ThemeFile>(&contents) {
            Ok(file) => {
                let themes: Vec<MeshTheme> = file.themes.into_iter().map(Into::into).collect();
                if themes.is_empty() {
                    log::warn!("Theme file has no themes, using defaults");
                    return default_themes();
                }
                log::info!("Loaded {} themes from {:?}", themes.len(), path);
                themes
            }
            Err(e) => {
                log::warn!("Failed to parse theme file: {}", e);
                log::info!("Migrating old theme.yaml to new format with default themes");
                let defaults = default_themes();
                if let Err(e) = save_themes(&defaults, path) {
                    log::warn!("Failed to write migrated theme file: {}", e);
                }
                defaults
            }
        },
        Err(e) => {
            log::warn!("Failed to read theme file: {}, using defaults", e);
            default_themes()
        }
    }
}

/// Save themes to a YAML file
pub fn save_themes(themes: &[MeshTheme], path: &Path) -> Result<(), String> {
    let file = ThemeFile {
        themes: themes.iter().map(Into::into).collect(),
    };
    let yaml = serde_yaml::to_string(&file).map_err(|e| e.to_string())?;

    // Prepend comment header
    let content = format!(
        "# Mesh Theme Configuration\n\
         # Each theme has UI colors (buttons, backgrounds, text) and stem waveform colors.\n\
         # Add your own themes or modify existing ones.\n\
         \n\
         {yaml}"
    );

    std::fs::write(path, content).map_err(|e| e.to_string())
}

/// Find a theme by name, falling back to the first theme or the hardcoded fallback
pub fn find_theme<'a>(themes: &'a [MeshTheme], name: &str) -> &'a MeshTheme {
    themes
        .iter()
        .find(|t| t.name == name)
        .or_else(|| themes.first())
        .unwrap_or_else(|| {
            // This shouldn't happen since default_themes() always returns at least 1
            // but we provide a static fallback just in case
            static FALLBACK: std::sync::LazyLock<MeshTheme> =
                std::sync::LazyLock::new(fallback_theme);
            &FALLBACK
        })
}

// ── Default Themes ─────────────────────────────────────────────────────────

/// The hardcoded fallback theme (used when no themes can be loaded)
///
/// Based on the "Mesh" theme (ex-High Contrast) — maximum hue separation.
pub fn fallback_theme() -> MeshTheme {
    MeshTheme {
        name: "Mesh".to_string(),
        ui: UiColors {
            background: Color::from_rgb(0.11, 0.11, 0.13),
            text: Color::from_rgb(0.90, 0.90, 0.90),
            accent: Color::from_rgb(0.30, 0.90, 0.40),
            success: Color::from_rgb(0.30, 0.80, 0.40),
            warning: Color::from_rgb(0.90, 0.70, 0.20),
            danger: Color::from_rgb(0.85, 0.30, 0.25),
        },
        stems: [
            Color::from_rgb(0.30, 0.90, 0.40), // Vocals - Bright Green
            Color::from_rgb(0.20, 0.60, 0.90), // Drums - Sky Blue
            Color::from_rgb(0.90, 0.50, 0.10), // Bass - Orange
            Color::from_rgb(0.80, 0.30, 0.80), // Other - Magenta
        ],
    }
}

/// All 5 built-in themes (written to theme.yaml on first run)
pub fn default_themes() -> Vec<MeshTheme> {
    vec![
        fallback_theme(),
        MeshTheme {
            name: "Natural".to_string(),
            ui: UiColors {
                background: Color::from_rgb(0.11, 0.11, 0.13),
                text: Color::from_rgb(0.90, 0.90, 0.90),
                accent: Color::from_rgb(0.45, 0.80, 0.55),
                success: Color::from_rgb(0.30, 0.80, 0.40),
                warning: Color::from_rgb(0.90, 0.70, 0.20),
                danger: Color::from_rgb(0.85, 0.30, 0.25),
            },
            stems: [
                Color::from_rgb(0.45, 0.80, 0.55), // Vocals - Sage Green
                Color::from_rgb(0.40, 0.60, 0.75), // Drums - Steel Blue
                Color::from_rgb(0.75, 0.55, 0.35), // Bass - Bronze
                Color::from_rgb(0.70, 0.60, 0.85), // Other - Lavender
            ],
        },
        MeshTheme {
            name: "Cool-Warm".to_string(),
            ui: UiColors {
                background: Color::from_rgb(0.11, 0.11, 0.13),
                text: Color::from_rgb(0.90, 0.90, 0.90),
                accent: Color::from_rgb(0.20, 0.85, 0.50),
                success: Color::from_rgb(0.30, 0.80, 0.40),
                warning: Color::from_rgb(0.90, 0.70, 0.20),
                danger: Color::from_rgb(0.85, 0.30, 0.25),
            },
            stems: [
                Color::from_rgb(0.20, 0.85, 0.50), // Vocals - Green
                Color::from_rgb(0.30, 0.50, 0.90), // Drums - Blue
                Color::from_rgb(0.60, 0.30, 0.80), // Bass - Purple
                Color::from_rgb(0.95, 0.70, 0.20), // Other - Gold
            ],
        },
        MeshTheme {
            name: "Synthwave".to_string(),
            ui: UiColors {
                background: Color::from_rgb(0.10, 0.06, 0.15),
                text: Color::from_rgb(0.94, 0.90, 1.00),
                accent: Color::from_rgb(0.30, 0.70, 0.95),
                success: Color::from_rgb(0.40, 0.95, 0.60),
                warning: Color::from_rgb(0.95, 0.85, 0.30),
                danger: Color::from_rgb(0.95, 0.40, 0.70),
            },
            stems: [
                Color::from_rgb(0.40, 0.95, 0.60), // Vocals - Mint
                Color::from_rgb(0.30, 0.70, 0.95), // Drums - Electric Blue
                Color::from_rgb(0.95, 0.40, 0.70), // Bass - Hot Pink
                Color::from_rgb(0.95, 0.85, 0.30), // Other - Yellow
            ],
        },
        MeshTheme {
            name: "Gruvbox".to_string(),
            ui: UiColors {
                background: Color::from_rgb(0.16, 0.16, 0.16),
                text: Color::from_rgb(0.92, 0.86, 0.70),
                accent: Color::from_rgb(0.85, 0.65, 0.13),
                success: Color::from_rgb(0.72, 0.73, 0.15),
                warning: Color::from_rgb(0.99, 0.50, 0.10),
                danger: Color::from_rgb(0.80, 0.14, 0.11),
            },
            stems: [
                Color::from_rgb(0.72, 0.73, 0.15), // Vocals - Gruvbox Green
                Color::from_rgb(0.99, 0.50, 0.10), // Drums - Gruvbox Orange
                Color::from_rgb(0.83, 0.53, 0.61), // Bass - Gruvbox Purple
                Color::from_rgb(0.56, 0.75, 0.49), // Other - Gruvbox Aqua
            ],
        },
    ]
}

// ── Legacy Constants (kept for backward compatibility) ──────────────────────

/// Legacy stem color palettes (deprecated — use MeshTheme from theme.yaml instead)
pub mod stem_palettes {
    use iced::Color;

    pub const NATURAL: [Color; 4] = [
        Color::from_rgb(0.45, 0.8, 0.55),
        Color::from_rgb(0.4, 0.6, 0.75),
        Color::from_rgb(0.75, 0.55, 0.35),
        Color::from_rgb(0.7, 0.6, 0.85),
    ];

    pub const COOL_WARM: [Color; 4] = [
        Color::from_rgb(0.2, 0.85, 0.5),
        Color::from_rgb(0.3, 0.5, 0.9),
        Color::from_rgb(0.6, 0.3, 0.8),
        Color::from_rgb(0.95, 0.7, 0.2),
    ];

    pub const HIGH_CONTRAST: [Color; 4] = [
        Color::from_rgb(0.3, 0.9, 0.4),
        Color::from_rgb(0.2, 0.6, 0.9),
        Color::from_rgb(0.9, 0.5, 0.1),
        Color::from_rgb(0.8, 0.3, 0.8),
    ];

    pub const SYNTHWAVE: [Color; 4] = [
        Color::from_rgb(0.4, 0.95, 0.6),
        Color::from_rgb(0.3, 0.7, 0.95),
        Color::from_rgb(0.95, 0.4, 0.7),
        Color::from_rgb(0.95, 0.85, 0.3),
    ];

    pub const GRUVBOX: [Color; 4] = [
        Color::from_rgb(0.72, 0.73, 0.15),
        Color::from_rgb(0.99, 0.50, 0.10),
        Color::from_rgb(0.83, 0.53, 0.61),
        Color::from_rgb(0.56, 0.75, 0.49),
    ];
}

/// Default stem colors (Natural palette — legacy, prefer MeshTheme)
pub const STEM_COLORS: [Color; 4] = stem_palettes::NATURAL;

/// Cue point colors (8 distinct colors for 8 hot cue buttons)
pub const CUE_COLORS: [Color; 8] = [
    Color::from_rgb(1.0, 0.3, 0.3), // Red
    Color::from_rgb(1.0, 0.6, 0.0), // Orange
    Color::from_rgb(1.0, 1.0, 0.0), // Yellow
    Color::from_rgb(0.3, 1.0, 0.3), // Green
    Color::from_rgb(0.0, 0.8, 0.8), // Cyan
    Color::from_rgb(0.3, 0.3, 1.0), // Blue
    Color::from_rgb(0.8, 0.3, 0.8), // Purple
    Color::from_rgb(1.0, 0.5, 0.8), // Pink
];

/// Stem names (full)
pub const STEM_NAMES: [&str; 4] = ["Vocals", "Drums", "Bass", "Other"];

/// Stem names (short, for compact UI)
pub const STEM_NAMES_SHORT: [&str; 4] = ["Vox", "Drm", "Bas", "Oth"];

/// Waveform display configuration
pub struct WaveformConfig {
    pub overview_height: f32,
    pub zoomed_height: f32,
    pub min_zoom_bars: u32,
    pub max_zoom_bars: u32,
    pub default_zoom_bars: u32,
    pub zoom_pixels_per_level: f32,
    pub peak_smoothing_window: usize,
}

impl Default for WaveformConfig {
    fn default() -> Self {
        Self {
            overview_height: 75.0,
            zoomed_height: 240.0,
            min_zoom_bars: 1,
            max_zoom_bars: 64,
            default_zoom_bars: 8,
            zoom_pixels_per_level: 20.0,
            peak_smoothing_window: 3,
        }
    }
}
