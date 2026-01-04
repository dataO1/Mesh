//! Shared UI widgets for mesh audio applications
//!
//! This crate provides reusable iced widgets and utilities for waveform display,
//! audio visualization, and DJ-style controls.
//!
//! ## Architecture (iced 0.14 patterns)
//!
//! Following idiomatic iced patterns:
//!
//! - **State structs**: Pure data (`OverviewState`, `ZoomedState`, `CombinedState`)
//! - **View functions**: Take state + callbacks, return `Element<Message>`
//! - **Canvas Programs**: Handle custom rendering and event-to-callback translation
//!
//! ## Current Features
//!
//! - **Theme constants**: Shared color schemes for stems and cue points
//! - **Peak generation**: Utilities for downsampling audio to waveform peaks
//! - **Waveform state**: Data structures for overview and zoomed waveform views
//! - **CueMarker**: Data structure for cue point display
//!
//! ## View Functions
//!
//! - `waveform_overview`: Overview waveform with click-to-seek
//! - `waveform_zoomed`: Zoomed detail view with drag-to-zoom
//! - `waveform_combined`: Both views in a single canvas (iced bug #3040 workaround)

pub mod theme;
pub mod traits;
pub mod waveform;

// Re-export commonly used items
pub use theme::{WaveformConfig, CUE_COLORS, STEM_COLORS, STEM_NAMES, STEM_NAMES_SHORT};

// Deprecated: Use callback closures with view functions instead
#[allow(deprecated)]
pub use traits::WaveformEvents;

// Peak generation utilities
pub use waveform::{
    generate_peaks, generate_peaks_for_range, generate_waveform_preview, smooth_peaks,
    DEFAULT_WIDTH, PEAK_SMOOTHING_WINDOW,
};

// Waveform state structures
pub use waveform::{
    CombinedState, CueMarker, OverviewState, ZoomedState,
    // Constants
    COMBINED_WAVEFORM_GAP, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

// Waveform view functions (idiomatic iced 0.14 pattern)
pub use waveform::{waveform_combined, waveform_overview, waveform_zoomed};

// Canvas interaction types for advanced usage
pub use waveform::{CombinedInteraction, OverviewInteraction, ZoomedInteraction};
