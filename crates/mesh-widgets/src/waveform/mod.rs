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
//! - **4-deck view** (`waveform_player_shader`): mesh-player's 2×2 grid with deck headers
//! - **Single-deck view** (`waveform_shader_combined`): mesh-cue's zoomed + overview column
//! - **Peak data** uploaded once to GPU storage buffer; only uniform buffers update per frame
//!
//! The old canvas-based renderers (`canvas/`, `view.rs`) are kept as source reference
//! but are not compiled.

// Canvas module — DEPRECATED (kept as source reference, not compiled).
// mesh-player uses shader::waveform_player_shader() for 4-deck GPU rendering.
// mesh-cue uses shader::waveform_shader_combined() for single-deck GPU rendering.
// mod canvas;
// mod view;

// Peak computation — DEPRECATED (kept as source reference, not compiled).
// The GPU shader reads peaks from PeakBuffer (uploaded once at track load).
// The old per-frame peak recomputation via PeaksComputer is no longer used.
// mod peak_computation;
// mod peaks_computer;

mod peaks;
pub mod shader;
// Slicer overlay — DEPRECATED (kept as source reference, not compiled).
// Slicer visualization is now handled entirely in the GPU shader (waveform.wgsl).
// mod slicer_overlay;
mod state;

pub use peaks::{
    allocate_empty_peaks, compute_highres_width, generate_peaks, generate_peaks_for_range,
    generate_waveform_preview, generate_waveform_preview_with_gain, smooth_peaks,
    smooth_peaks_gaussian, update_peaks_for_region, DEFAULT_WIDTH, PEAK_REFERENCE_ZOOM_BARS,
    PEAK_SMOOTHING_WINDOW,
};

pub use state::{
    CombinedState, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    // Constants
    COMBINED_WAVEFORM_GAP, DECK_HEADER_HEIGHT, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

// GPU shader waveform rendering
pub use shader::{
    PeakBuffer, WaveformAction, SingleDeckAction, view_deck_header,
    waveform_player_shader, waveform_shader_overview, waveform_shader_zoomed,
    waveform_shader_combined, waveform_shader_single_overview, waveform_shader_single_zoomed,
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
