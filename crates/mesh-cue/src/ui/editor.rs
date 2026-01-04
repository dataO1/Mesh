//! Track editor view

use super::app::{LoadedTrackState, Message};
use super::{cue_editor, transport};
use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length};

/// Render the track editor
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    let header = view_header(state);

    // Player controls (vertical, left of waveforms)
    let player_controls = transport::view(state);

    // Combined waveform canvas (zoomed detail view above overview)
    // Uses single canvas to work around iced bug #3040 where multiple Canvas widgets
    // don't render properly - only the first one shows.
    let waveforms = state.combined_waveform.view(state.playhead_position());

    // Layout: player controls on left, waveforms take remaining width
    let main_row = row![player_controls, waveforms]
        .spacing(10)
        .align_y(Alignment::Center);

    // Hot cue buttons (single row of 8)
    let cue_panel = cue_editor::view(state);

    let save_section = view_save_section(state);

    container(
        column![
            header,
            main_row,
            cue_panel,
            save_section,
        ]
        .spacing(15),
    )
    .padding(15)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Header with track info and editable BPM/key
fn view_header(state: &LoadedTrackState) -> Element<Message> {
    let track_name = state
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("Unknown Track"));

    let title = text(track_name).size(20);

    let bpm_label = text("BPM:");
    let bpm_input = text_input("BPM", &format!("{:.2}", state.bpm))
        .on_input(|s| {
            s.parse::<f64>()
                .map(Message::SetBpm)
                .unwrap_or(Message::SetBpm(state.bpm))
        })
        .width(Length::Fixed(80.0));

    let key_label = text("Key:");
    let key_input = text_input("Key", &state.key)
        .on_input(Message::SetKey)
        .width(Length::Fixed(60.0));

    let modified_indicator = if state.modified {
        text("*").size(20)
    } else {
        text("").size(20)
    };

    row![
        title,
        modified_indicator,
        Space::new().width(Length::Fill),
        bpm_label,
        bpm_input,
        key_label,
        key_input,
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// Save section
fn view_save_section(state: &LoadedTrackState) -> Element<Message> {
    let save_btn = button(text("Save Changes"))
        .on_press_maybe(state.modified.then_some(Message::SaveTrack));

    let status = if state.modified {
        text("Unsaved changes").size(14)
    } else {
        text("All changes saved").size(14)
    };

    row![save_btn, status]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}
