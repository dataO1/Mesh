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
/// Deep indigo-black background with cyan-blue accent, inspired by
/// professional DJ hardware (Pioneer CDJ/Denon screens). The purple
/// undertone makes warm stem colors (orange, magenta) pop.
pub fn fallback_theme() -> MeshTheme {
    MeshTheme {
        name: "Mesh".to_string(),
        ui: UiColors {
            background: hex("#16141F"), // Deep indigo-black
            text: hex("#E0DDE8"),       // Warm off-white (slight lavender)
            accent: hex("#47B5FF"),     // Electric cyan-blue
            success: hex("#5EEAA0"),    // Soft mint green
            warning: hex("#F0C060"),    // Warm amber gold
            danger: hex("#E85C6F"),     // Coral-red
        },
        stems: [
            hex("#4DE8B0"), // Vocals - Teal-mint
            hex("#6090F0"), // Drums - Periwinkle
            hex("#F09040"), // Bass - Warm orange
            hex("#D060D0"), // Other - Magenta
        ],
    }
}

/// All 5 built-in themes (written to theme.yaml on first run)
pub fn default_themes() -> Vec<MeshTheme> {
    vec![
        fallback_theme(),
        // Catppuccin Mocha — soothing pastels on blue-tinted dark background.
        // Community-standard palette (github.com/catppuccin) with WCAG AAA contrast.
        MeshTheme {
            name: "Catppuccin".to_string(),
            ui: UiColors {
                background: hex("#1E1E2E"), // Base
                text: hex("#CDD6F4"),       // Text (cool white)
                accent: hex("#CBA6F7"),     // Mauve
                success: hex("#A6E3A1"),    // Green
                warning: hex("#F9E2AF"),    // Yellow
                danger: hex("#F38BA8"),     // Red (soft coral)
            },
            stems: [
                hex("#A6E3A1"), // Vocals - Catppuccin Green
                hex("#89B4FA"), // Drums - Catppuccin Blue
                hex("#FAB387"), // Bass - Catppuccin Peach
                hex("#CBA6F7"), // Other - Catppuccin Mauve
            ],
        },
        // Rosé Pine Moon — minimalist, editorial palette with purple-blue undertones.
        // "Soho vibes" aesthetic (rosepinetheme.com), curated to just 6 accent colors.
        MeshTheme {
            name: "Rosé Pine".to_string(),
            ui: UiColors {
                background: hex("#232136"), // Base
                text: hex("#E0DEF4"),       // Text
                accent: hex("#C4A7E7"),     // Iris
                success: hex("#9CCFD8"),    // Foam (teal)
                warning: hex("#F6C177"),    // Gold
                danger: hex("#EB6F92"),     // Love (coral-pink)
            },
            stems: [
                hex("#9CCFD8"), // Vocals - Foam (teal)
                hex("#EA9A97"), // Drums - Rose (warm pink)
                hex("#F6C177"), // Bass - Gold (amber)
                hex("#C4A7E7"), // Other - Iris (purple)
            ],
        },
        MeshTheme {
            name: "Synthwave".to_string(),
            ui: UiColors {
                background: hex("#1A1025"), // Deep violet-black
                text: hex("#F0E6FF"),       // Pale lavender white
                accent: hex("#4DB3F2"),     // Neon blue
                success: hex("#66F299"),    // Neon green
                warning: hex("#F2D94D"),    // Electric yellow
                danger: hex("#F266B3"),     // Hot pink
            },
            stems: [
                hex("#66F299"), // Vocals - Neon mint
                hex("#4DB3F2"), // Drums - Electric blue
                hex("#F266B3"), // Bass - Hot pink
                hex("#F2D94D"), // Other - Electric yellow
            ],
        },
        MeshTheme {
            name: "Gruvbox".to_string(),
            ui: UiColors {
                background: hex("#282828"), // Gruvbox bg0
                text: hex("#EBDBB2"),       // Gruvbox fg
                accent: hex("#D79921"),     // Gruvbox yellow
                success: hex("#B8BB26"),    // Gruvbox green
                warning: hex("#FE8019"),    // Gruvbox orange
                danger: hex("#CC241D"),     // Gruvbox red
            },
            stems: [
                hex("#B8BB26"), // Vocals - Gruvbox green
                hex("#83A598"), // Drums - Gruvbox blue
                hex("#FE8019"), // Bass - Gruvbox orange
                hex("#D3869B"), // Other - Gruvbox pink
            ],
        },
    ]
}

// ── Legacy Constants (kept for backward compatibility) ──────────────────────

/// Legacy stem color palettes (deprecated — use MeshTheme from theme.yaml instead)
pub mod stem_palettes {
    use iced::Color;

    /// Mesh default: teal-mint, periwinkle, orange, magenta
    pub const MESH: [Color; 4] = [
        Color::from_rgb(0.302, 0.910, 0.690), // #4DE8B0
        Color::from_rgb(0.376, 0.565, 0.941), // #6090F0
        Color::from_rgb(0.941, 0.565, 0.251), // #F09040
        Color::from_rgb(0.816, 0.376, 0.816), // #D060D0
    ];

    pub const SYNTHWAVE: [Color; 4] = [
        Color::from_rgb(0.40, 0.95, 0.60),
        Color::from_rgb(0.30, 0.70, 0.95),
        Color::from_rgb(0.95, 0.40, 0.70),
        Color::from_rgb(0.95, 0.85, 0.30),
    ];

    pub const GRUVBOX: [Color; 4] = [
        Color::from_rgb(0.722, 0.733, 0.149),
        Color::from_rgb(0.996, 0.502, 0.098),
        Color::from_rgb(0.827, 0.537, 0.608),
        Color::from_rgb(0.557, 0.753, 0.486),
    ];
}

/// Default stem colors (Mesh palette — legacy, prefer MeshTheme)
pub const STEM_COLORS: [Color; 4] = stem_palettes::MESH;

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
