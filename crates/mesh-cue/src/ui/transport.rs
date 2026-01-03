//! Transport controls component

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, container, row, text};
use iced::{Alignment, Element, Length};
use mesh_core::types::SAMPLE_RATE;

/// Render transport controls
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // TODO: Get actual position from audio state
    let position = 0u64;
    let duration = state.track.duration_samples as u64;

    let position_str = format_time(position);
    let duration_str = format_time(duration);

    let skip_back = button(text("◄◄")).on_press(Message::Seek(0.0));
    let play = button(text("▶")).on_press(Message::Play);
    let pause = button(text("▮▮")).on_press(Message::Pause);
    let skip_forward = button(text("►►")).on_press(Message::Seek(1.0));

    let time_display = text(format!("{} / {}", position_str, duration_str)).size(14);

    container(
        row![skip_back, play, pause, skip_forward, time_display,]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .padding(10)
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

/// Format sample position as time string (MM:SS.ms)
fn format_time(samples: u64) -> String {
    let seconds = samples as f64 / SAMPLE_RATE as f64;
    let minutes = (seconds / 60.0).floor() as u64;
    let secs = seconds % 60.0;
    format!("{}:{:05.2}", minutes, secs)
}
