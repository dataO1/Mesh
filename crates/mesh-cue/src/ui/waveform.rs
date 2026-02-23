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
//! - **GPU Shader**: Renders waveforms via WGSL fragment shader
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
    generate_peaks, generate_peaks_for_range, smooth_peaks,
    CUE_COLORS, STEM_COLORS, DEFAULT_WIDTH, PEAK_SMOOTHING_WINDOW,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, COMBINED_WAVEFORM_GAP,
    MIN_ZOOM_BARS, MAX_ZOOM_BARS, DEFAULT_ZOOM_BARS, ZOOM_PIXELS_PER_LEVEL,
};

use mesh_widgets::{waveform_shader_combined, SingleDeckAction};

/// Create a combined waveform view element using GPU shader rendering.
///
/// This replaces the canvas-based `waveform_combined()` with GPU shader
/// rendering via `waveform_shader_combined()`. The zoomed view supports
/// vinyl scratch gestures (horizontal drag = scrub, vertical drag = zoom).
pub fn view_combined_waveform(state: &CombinedWaveformView, playhead: u64) -> Element<'_, Message> {
    waveform_shader_combined(state, playhead, STEM_COLORS, |action| match action {
        SingleDeckAction::Seek(pos) => Message::Seek(pos),
        SingleDeckAction::SetZoom(bars) => Message::SetZoomBars(bars),
        SingleDeckAction::ScratchStart => Message::ScratchStart,
        SingleDeckAction::ScratchMove(pos) => Message::ScratchMove(pos),
        SingleDeckAction::ScratchEnd => Message::ScratchEnd,
    })
}
