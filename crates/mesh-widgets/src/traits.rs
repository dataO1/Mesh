//! Traits for abstracting widget events
//!
//! **Note**: This module is deprecated. The iced 0.14 pattern uses callback
//! closures instead of traits for message abstraction. See `waveform_combined`,
//! `waveform_overview`, and `waveform_zoomed` functions for the recommended approach.
//!
//! ## Recommended Pattern (iced 0.14)
//!
//! ```ignore
//! // Pass closures directly to view functions:
//! let waveform = waveform_combined(
//!     &state,
//!     playhead,
//!     |pos| Message::Seek(pos),      // seek callback
//!     |bars| Message::SetZoomBars(bars),  // zoom callback
//! );
//! ```

/// Event handler trait for waveform widgets
///
/// **Deprecated**: Use callback closures with view functions instead.
/// This trait was part of an earlier design before adopting iced 0.14 patterns.
#[deprecated(
    since = "0.1.0",
    note = "Use callback closures with waveform_combined/waveform_overview/waveform_zoomed instead"
)]
pub trait WaveformEvents: Clone {
    /// The message type produced by waveform interactions
    type Message: Clone + std::fmt::Debug;

    /// Create a seek message for the given normalized position (0.0 to 1.0)
    fn on_seek(position: f64) -> Self::Message;

    /// Create a zoom level change message for the given bar count
    fn on_zoom(bars: u32) -> Self::Message;
}
