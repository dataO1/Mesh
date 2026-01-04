//! Waveform view functions
//!
//! These functions create waveform UI elements using the proper iced 0.14 pattern:
//! plain functions that take state references and callback closures, returning Elements.
//!
//! ## Usage
//!
//! ```ignore
//! // In your application's view function:
//! fn view(&self) -> Element<Message> {
//!     let waveform = waveform_combined(
//!         &self.waveform_state,
//!         self.playhead,
//!         |pos| Message::Seek(pos),
//!         |bars| Message::SetZoomBars(bars),
//!     );
//!
//!     column![waveform, /* other widgets */].into()
//! }
//! ```

use super::canvas::{CombinedCanvas, OverviewCanvas, ZoomedCanvas};
use super::state::{
    CombinedState, OverviewState, ZoomedState,
    COMBINED_WAVEFORM_GAP, WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT,
};
use iced::widget::Canvas;
use iced::{Element, Length};

/// Create an overview waveform element with click-to-seek
///
/// # Arguments
///
/// * `state` - The overview waveform state containing peak data, markers, etc.
/// * `on_seek` - Callback closure called with normalized position (0.0 to 1.0) on click/drag
///
/// # Returns
///
/// An `Element` that renders the overview waveform canvas
///
/// # Example
///
/// ```ignore
/// let overview = waveform_overview(
///     &self.overview_state,
///     |pos| Message::Seek(pos),
/// );
/// ```
pub fn waveform_overview<'a, Message>(
    state: &'a OverviewState,
    on_seek: impl Fn(f64) -> Message + 'a,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Canvas::new(OverviewCanvas { state, on_seek })
        .width(Length::Fill)
        .height(Length::Fixed(WAVEFORM_HEIGHT))
        .into()
}

/// Create a zoomed waveform element with zoom gesture
///
/// # Arguments
///
/// * `state` - The zoomed waveform state containing cached peaks, zoom level, etc.
/// * `playhead` - Current playhead position in samples
/// * `on_zoom` - Callback closure called with new zoom level (in bars) on drag
///
/// # Returns
///
/// An `Element` that renders the zoomed waveform canvas
///
/// # Example
///
/// ```ignore
/// let zoomed = waveform_zoomed(
///     &self.zoomed_state,
///     self.playhead_samples,
///     |bars| Message::SetZoomBars(bars),
/// );
/// ```
pub fn waveform_zoomed<'a, Message>(
    state: &'a ZoomedState,
    playhead: u64,
    on_zoom: impl Fn(u32) -> Message + 'a,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    Canvas::new(ZoomedCanvas {
        state,
        playhead,
        on_zoom,
    })
    .width(Length::Fill)
    .height(Length::Fixed(ZOOMED_WAVEFORM_HEIGHT))
    .into()
}

/// Create a combined waveform element (zoomed + overview in single canvas)
///
/// This combines both waveform views into a single canvas widget as a workaround
/// for iced bug #3040 where multiple Canvas widgets don't render properly.
///
/// # Arguments
///
/// * `state` - Combined state containing both zoomed and overview waveform data
/// * `playhead` - Current playhead position in samples
/// * `on_seek` - Callback for seek operations (normalized position 0.0 to 1.0)
/// * `on_zoom` - Callback for zoom operations (new zoom level in bars)
///
/// # Returns
///
/// An `Element` that renders both waveforms in a single canvas
///
/// # Example
///
/// ```ignore
/// let waveform = waveform_combined(
///     &self.waveform_state,
///     self.playhead_samples,
///     |pos| Message::Seek(pos),
///     |bars| Message::SetZoomBars(bars),
/// );
/// ```
pub fn waveform_combined<'a, Message>(
    state: &'a CombinedState,
    playhead: u64,
    on_seek: impl Fn(f64) -> Message + 'a,
    on_zoom: impl Fn(u32) -> Message + 'a,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    let combined_height = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP + WAVEFORM_HEIGHT;

    Canvas::new(CombinedCanvas {
        state,
        playhead,
        on_seek,
        on_zoom,
    })
    .width(Length::Fill)
    .height(Length::Fixed(combined_height))
    .into()
}
