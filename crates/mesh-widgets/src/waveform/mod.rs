//! Waveform display components and utilities
//!
//! This module provides waveform visualization for displaying stem-based
//! audio waveforms with beat grids and cue markers.
//!
//! ## Architecture
//!
//! All waveform rendering uses GPU shader pipelines via the `shader` submodule:
//!
//! - **State structs** (`OverviewState`, `ZoomedState`, `CombinedState`): Pure data
//! - **SharedPeakBuffer**: Zero-copy peak data shared between loader and UI via `RwLock`
//! - **4-deck view** (`waveform_player_shader`): mesh-player's 2×2 grid with deck headers
//! - **Single-deck view** (`waveform_shader_combined`): mesh-cue's zoomed + overview column
//! - **Peak data** streamed incrementally into `SharedPeakBuffer`, read by UI on cache miss

mod peaks;
pub mod shader;
mod state;

pub use peaks::{
    allocate_empty_peaks, allocate_flat_peaks,
    compute_highres_width, generate_peaks, generate_peaks_for_range,
    smooth_peaks_gaussian, update_peaks_for_region, update_peaks_for_region_flat,
    DEFAULT_WIDTH, PEAK_REFERENCE_ZOOM_BARS, PEAK_SMOOTHING_WINDOW,
};

pub use state::{
    CombinedState, OverviewState, PlayerCanvasState, SharedPeakBuffer,
    ZoomedState, ZoomedViewMode,
    // Constants
    COMBINED_WAVEFORM_GAP, DECK_HEADER_HEIGHT, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

// GPU shader waveform rendering
pub use shader::{
    PeakBuffer, WaveformAction, SingleDeckAction, view_deck_header,
    waveform_player_shader, waveform_shader_overview, waveform_shader_zoomed,
    waveform_shader_combined, waveform_shader_single_overview, waveform_shader_single_zoomed,
    view_master_meter_horizontal,
};

// Slicer overlay — now handled by GPU shader, canvas functions removed from compilation
// pub use slicer_overlay::{draw_slicer_overlay, draw_slicer_overlay_zoomed};

use iced::Color;

/// Cue point marker for display
#[derive(Debug, Clone)]
pub struct CueMarker {
    /// Normalized position (0.0 to 1.0)
    pub position: f64,
    /// Cue label text
    pub label: String,
    /// Marker color
    pub color: Color,
    /// Cue number (0-7)
    pub index: u8,
}
