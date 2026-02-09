//! Waveform display components and utilities
//!
//! This module provides waveform visualization for displaying stem-based
//! audio waveforms with beat grids and cue markers.
//!
//! ## Architecture (iced 0.14 patterns)
//!
//! Following idiomatic iced patterns, this module separates concerns:
//!
//! - **State structs** (`OverviewState`, `ZoomedState`, `CombinedState`): Pure data
//! - **View functions** (`waveform_overview`, `waveform_zoomed`, `waveform_combined`):
//!   Take state + callbacks, return `Element<Message>`
//! - **Canvas Programs**: Handle custom rendering and event-to-callback translation
//!
//! ## Usage
//!
//! ```ignore
//! // In your application's view function:
//! let waveform = waveform_combined(
//!     &self.waveform_state,
//!     self.playhead,
//!     |pos| Message::Seek(pos),
//!     |bars| Message::SetZoomBars(bars),
//! );
//! ```
//!
//! ## Features
//!
//! - Peak generation utilities for downsampling audio to display data
//! - CueMarker data structure for cue point visualization
//! - State structures for overview and zoomed waveform views
//! - View functions with callback closures for reusable waveform widgets

mod canvas;
mod peak_computation;
mod peaks;
mod peaks_computer;
mod slicer_overlay;
mod state;
mod view;

pub use peak_computation::{
    CacheInfo, WindowInfo,
    compute_effective_width, generate_peaks_for_cache, generate_peaks_with_padding,
    samples_per_bar, scale_peaks_by_gain, smooth_peaks as smooth_peaks_array,
};

pub use peaks::{
    generate_peaks, generate_peaks_for_range, generate_waveform_preview,
    generate_waveform_preview_with_gain, smooth_peaks, smooth_peaks_gaussian,
    DEFAULT_WIDTH, HIGHRES_WIDTH, PEAK_SMOOTHING_WINDOW,
};

pub use peaks_computer::{PeaksComputer, PeaksComputeRequest, PeaksComputeResult, PeaksResultReceiver};

pub use state::{
    CombinedState, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    // Constants
    COMBINED_WAVEFORM_GAP, DECK_HEADER_HEIGHT, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

pub use view::{waveform_combined, waveform_overview, waveform_player, waveform_zoomed};

// Re-export canvas types for advanced usage (custom Program state)
pub use canvas::{
    CombinedInteraction, OverviewInteraction, PlayerInteraction, ZoomedInteraction,
    // Player canvas layout constants
    DECK_CELL_HEIGHT, DECK_GRID_GAP, DECK_INTERNAL_GAP,
};

// Slicer overlay drawing utilities (reusable across mesh-player and mesh-cue)
pub use slicer_overlay::{draw_slicer_overlay, draw_slicer_overlay_zoomed};

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
