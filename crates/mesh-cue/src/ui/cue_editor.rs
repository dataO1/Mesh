//! Cue point editor component

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};
use mesh_core::types::SAMPLE_RATE;

/// Render the cue point editor
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    let title = text("Cue Points").size(16);

    let cue_list: Element<Message> = if state.cue_points.is_empty() {
        text("No cue points set").size(14).into()
    } else {
        let items: Vec<Element<Message>> = state
            .cue_points
            .iter()
            .enumerate()
            .map(|(i, cue)| view_cue_item(i, cue, state))
            .collect();

        scrollable(column(items).spacing(4))
            .height(Length::Fixed(120.0))
            .into()
    };

    // Add cue button (would add at current playhead position)
    let add_btn = button(text("+ Add Cue at Playhead")).on_press(Message::AddCuePoint(0)); // TODO: use actual position

    container(column![title, cue_list, add_btn,].spacing(10))
        .padding(10)
        .width(Length::Fill)
        .into()
}

/// Render a single cue point item
fn view_cue_item(
    index: usize,
    cue: &mesh_core::audio_file::CuePoint,
    _state: &LoadedTrackState,
) -> Element<'static, Message> {
    let time_str = format_time(cue.sample_position);

    let num = text(format!("{}.", index + 1)).size(14);

    let time = text(time_str).size(14);

    let label_input = text_input("Label", &cue.label)
        .on_input(move |s| Message::SetCueLabel(index, s))
        .width(Length::Fixed(100.0));

    let color_indicator = container(text("●").size(14))
        .padding(2);

    let delete_btn = button(text("🗑").size(12)).on_press(Message::DeleteCuePoint(index));

    row![num, time, label_input, color_indicator, delete_btn,]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// Format sample position as time string (MM:SS.ms)
fn format_time(samples: u64) -> String {
    let seconds = samples as f64 / SAMPLE_RATE as f64;
    let minutes = (seconds / 60.0).floor() as u64;
    let secs = seconds % 60.0;
    format!("{}:{:05.2}", minutes, secs)
}
