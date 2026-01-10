//! Shared theme constants for mesh UI components
//!
//! Color schemes and visual constants used across waveforms, cue buttons,
//! and other audio visualization widgets.

use iced::Color;

/// Stem colors (Vocals, Drums, Bass, Other)
///
/// Used for waveform display with 4 color-coded audio stems.
/// Colors are semi-transparent when overlapped in the waveform view.
/// Configurable via ~/.config/mesh-player/theme.yaml in mesh-player.
pub const STEM_COLORS: [Color; 4] = [
    Color::from_rgb(0.2, 0.8, 0.4),   // Vocals - Green (#33CC66)
    Color::from_rgb(0.8, 0.2, 0.2),   // Drums - Dark Red (#CC3333)
    Color::from_rgb(0.9, 0.38, 0.3),  // Bass - Orange-Red (#E6604D)
    Color::from_rgb(0.0, 0.8, 0.8),   // Other - Cyan (#00CCCC)
];

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
