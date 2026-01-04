//! Transport controls component

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, container, row, text};
use iced::{Alignment, Element, Length};
use mesh_core::types::SAMPLE_RATE;

/// Render transport controls
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // TODO: Get actual position from audio state
    let position = 0u64;
    let duration = state.duration_samples;

    let position_str = format_time(position);
    let duration_str = if state.loading_audio {
        "Loading...".to_string()
    } else {
        format_time(duration)
    };

    // Disable controls while loading
    let controls_enabled = !state.loading_audio && state.stems.is_some();

    let skip_back = if controls_enabled {
        button(text("◄◄")).on_press(Message::Seek(0.0))
    } else {
        button(text("◄◄"))
    };
    let play = if controls_enabled {
        button(text("▶")).on_press(Message::Play)
    } else {
        button(text("▶"))
    };
    let pause = if controls_enabled {
        button(text("▮▮")).on_press(Message::Pause)
    } else {
        button(text("▮▮"))
    };
    let skip_forward = if controls_enabled {
        button(text("►►")).on_press(Message::Seek(1.0))
    } else {
        button(text("►►"))
    };

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
