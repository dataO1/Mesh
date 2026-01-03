//! Waveform display component

use super::app::{LoadedTrackState, Message};
use iced::widget::{container, text};
use iced::{Element, Length};

/// Render the waveform display
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // TODO: Implement actual waveform rendering using iced canvas
    // For now, show a placeholder

    let duration = state.track.duration_seconds;
    let beat_count = state.track.beat_count();

    let info = text(format!(
        "Waveform placeholder - {:.1}s - {} beats",
        duration, beat_count
    ))
    .size(14);

    container(info)
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(150.0))
        .center_x(Length::Fill)
        .center_y(Length::Fixed(150.0))
        .into()
}
