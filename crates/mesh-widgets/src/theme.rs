//! Shared theme constants for mesh UI components
//!
//! Color schemes and visual constants used across waveforms, cue buttons,
//! and other audio visualization widgets.

use iced::Color;

/// Stem color palettes - different aesthetic options
///
/// Each palette is designed for different use cases:
/// - NATURAL: Soft, muted tones for extended viewing comfort
/// - COOL_WARM: Uses color temperature for natural depth perception
/// - HIGH_CONTRAST: Maximum hue separation for clarity
/// - SYNTHWAVE: Modern DJ aesthetic with neon-inspired colors
/// - GRUVBOX: Retro warm palette inspired by the Gruvbox colorscheme
pub mod stem_palettes {
    use iced::Color;

    /// Natural/Organic - soft, muted tones for extended viewing comfort (DEFAULT)
    pub const NATURAL: [Color; 4] = [
        Color::from_rgb(0.45, 0.8, 0.55),  // Vocals - Sage Green
        Color::from_rgb(0.4, 0.6, 0.75),   // Drums - Steel Blue
        Color::from_rgb(0.75, 0.55, 0.35), // Bass - Bronze
        Color::from_rgb(0.7, 0.6, 0.85),   // Other - Lavender
    ];

    /// Cool-to-Warm Depth - uses color temperature for natural depth perception
    pub const COOL_WARM: [Color; 4] = [
        Color::from_rgb(0.2, 0.85, 0.5),   // Vocals - Green
        Color::from_rgb(0.3, 0.5, 0.9),    // Drums - Blue
        Color::from_rgb(0.6, 0.3, 0.8),    // Bass - Purple
        Color::from_rgb(0.95, 0.7, 0.2),   // Other - Gold
    ];

    /// High Contrast - maximum hue separation for clarity
    pub const HIGH_CONTRAST: [Color; 4] = [
        Color::from_rgb(0.3, 0.9, 0.4),    // Vocals - Bright Green
        Color::from_rgb(0.2, 0.6, 0.9),    // Drums - Sky Blue
        Color::from_rgb(0.9, 0.5, 0.1),    // Bass - Orange
        Color::from_rgb(0.8, 0.3, 0.8),    // Other - Magenta
    ];

    /// Synthwave - modern DJ aesthetic with neon-inspired colors
    pub const SYNTHWAVE: [Color; 4] = [
        Color::from_rgb(0.4, 0.95, 0.6),   // Vocals - Mint
        Color::from_rgb(0.3, 0.7, 0.95),   // Drums - Electric Blue
        Color::from_rgb(0.95, 0.4, 0.7),   // Bass - Hot Pink
        Color::from_rgb(0.95, 0.85, 0.3),  // Other - Yellow
    ];

    /// Gruvbox - retro warm palette with earthy, vintage tones
    /// Inspired by the popular Gruvbox colorscheme
    pub const GRUVBOX: [Color; 4] = [
        Color::from_rgb(0.72, 0.73, 0.15), // Vocals - Gruvbox Green (#b8bb26)
        Color::from_rgb(0.99, 0.50, 0.10), // Drums - Gruvbox Orange (#fe8019)
        Color::from_rgb(0.83, 0.53, 0.61), // Bass - Gruvbox Purple (#d3869b)
        Color::from_rgb(0.56, 0.75, 0.49), // Other - Gruvbox Aqua (#8ec07c)
    ];
}

/// Active stem colors (currently using Natural palette)
///
/// Used for waveform display with 4 color-coded audio stems.
/// Colors are semi-transparent when overlapped in the waveform view.
pub const STEM_COLORS: [Color; 4] = stem_palettes::NATURAL;

/// Cue point colors (8 distinct colors for 8 hot cue buttons)
///
/// Used for hot cue buttons and cue markers on the waveform.
/// Matches CDJ-style color coding.
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
    /// Overview waveform height in pixels
    pub overview_height: f32,
    /// Zoomed waveform height in pixels
    pub zoomed_height: f32,
    /// Minimum zoom level in bars
    pub min_zoom_bars: u32,
    /// Maximum zoom level in bars
    pub max_zoom_bars: u32,
    /// Default zoom level in bars
    pub default_zoom_bars: u32,
    /// Pixels of drag movement per zoom level change
    pub zoom_pixels_per_level: f32,
    /// Smoothing window size for peaks (moving average)
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
