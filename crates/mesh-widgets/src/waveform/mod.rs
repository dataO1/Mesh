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
mod peaks;
mod state;
mod view;

pub use peaks::{
    generate_peaks, generate_peaks_for_range, generate_waveform_preview, smooth_peaks,
    DEFAULT_WIDTH, PEAK_SMOOTHING_WINDOW,
};

pub use state::{
    CombinedState, OverviewState, ZoomedState,
    // Constants
    COMBINED_WAVEFORM_GAP, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

pub use view::{waveform_combined, waveform_overview, waveform_zoomed};

// Re-export canvas types for advanced usage (custom Program state)
pub use canvas::{CombinedInteraction, OverviewInteraction, ZoomedInteraction};

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
