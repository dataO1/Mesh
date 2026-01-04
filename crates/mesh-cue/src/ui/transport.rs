//! Transport controls component
//!
//! CDJ-style transport with:
//! - Play/Pause toggle button
//! - Cue button (snap to nearest beat grid)
//! - Beat jump buttons (<< / >>)

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, container, row, text};
use iced::{Alignment, Element, Length};
use mesh_core::types::SAMPLE_RATE;

/// Render transport controls
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    let position = state.playhead_position();
    let duration = state.duration_samples;
    let beat_jump_size = state.beat_jump_size();
    let is_playing = state.is_playing();

    let position_str = format_time(position);
    let duration_str = if state.loading_audio {
        "Loading...".to_string()
    } else {
        format_time(duration)
    };

    // Disable controls while loading
    let controls_enabled = !state.loading_audio && state.stems.is_some();

    // Beat jump backward (<<)
    let jump_back = if controls_enabled {
        button(text("◄◄")).on_press(Message::BeatJump(-beat_jump_size))
    } else {
        button(text("◄◄"))
    };

    // CDJ-style cue button (●)
    let cue = if controls_enabled {
        button(text("●")).on_press(Message::Cue)
    } else {
        button(text("●"))
    };

    // Play/Pause toggle
    let play_pause = if controls_enabled {
        if is_playing {
            button(text("▮▮")).on_press(Message::Pause)
        } else {
            button(text("▶")).on_press(Message::Play)
        }
    } else {
        button(text("▶"))
    };

    // Beat jump forward (>>)
    let jump_forward = if controls_enabled {
        button(text("►►")).on_press(Message::BeatJump(beat_jump_size))
    } else {
        button(text("►►"))
    };

    let time_display = text(format!("{} / {}", position_str, duration_str)).size(14);

    container(
        row![jump_back, cue, play_pause, jump_forward, time_display,]
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
