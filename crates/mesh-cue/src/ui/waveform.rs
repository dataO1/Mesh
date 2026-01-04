//! Waveform display component - thin wrapper over mesh_widgets
//!
//! This module re-exports the waveform types from mesh_widgets and provides
//! type aliases for backward compatibility during the migration.
//!
//! ## Architecture
//!
//! Following iced 0.14 patterns, waveform rendering is now handled by:
//! - **State structs** in mesh_widgets: Pure data (peaks, markers, positions)
//! - **View functions**: Take state + callbacks, return Elements
//! - **Canvas Programs**: Handle custom rendering
//!
//! See mesh_widgets::waveform for the full implementation.

use super::app::Message;
use iced::Element;

// Re-export all types from mesh_widgets for backward compatibility
pub use mesh_widgets::{
    // State structures (renamed for compatibility)
    OverviewState as WaveformView,
    ZoomedState as ZoomedWaveformView,
    CombinedState as CombinedWaveformView,
    // Other exports
    CueMarker,
    generate_peaks, generate_peaks_for_range, generate_waveform_preview, smooth_peaks,
    CUE_COLORS, STEM_COLORS, DEFAULT_WIDTH, PEAK_SMOOTHING_WINDOW,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, COMBINED_WAVEFORM_GAP,
    MIN_ZOOM_BARS, MAX_ZOOM_BARS, DEFAULT_ZOOM_BARS, ZOOM_PIXELS_PER_LEVEL,
};

// Import the view function for local use
use mesh_widgets::waveform_combined;

/// Create a combined waveform view element
///
/// This is a convenience function that wraps the mesh_widgets view function
/// with mesh-cue's specific Message type.
pub fn view_combined_waveform(state: &CombinedWaveformView, playhead: u64) -> Element<Message> {
    waveform_combined(
        state,
        playhead,
        Message::Seek,
        Message::SetZoomBars,
    )
}
