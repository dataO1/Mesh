//! Track editor view

use super::app::{LoadedTrackState, Message};
use super::waveform::view_combined_waveform;
use super::{cue_editor, saved_loop_buttons, transport};
use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length};

/// Render the track editor
///
/// # Arguments
/// * `state` - The loaded track state
/// * `stem_link_selection` - Which stem slot is being linked (if any)
pub fn view(state: &LoadedTrackState, stem_link_selection: Option<usize>) -> Element<Message> {
    let header = view_header(state);

    // Player controls (vertical, left of waveforms)
    let player_controls = transport::view(state);

    // Combined waveform canvas (zoomed detail view above overview)
    // Uses single canvas to work around iced bug #3040 where multiple Canvas widgets
    // don't render properly - only the first one shows.
    // Use interpolated position for smooth waveform animation during playback
    let waveforms = view_combined_waveform(&state.combined_waveform, state.interpolated_playhead_position());

    // Layout: player controls on left, waveforms take remaining width
    // Align to top (Start) so content doesn't float in the middle
    let main_row = row![player_controls, waveforms]
        .spacing(10)
        .align_y(Alignment::Start);

    // Hot cue buttons (single row of 8) - directly under waveforms
    let cue_panel = cue_editor::view(state);

    // Saved loop buttons (single row of 8) - below hot cues
    let loop_panel = saved_loop_buttons::view(state);

    // Stem link buttons (4 buttons for prepared mode) - below saved loops
    let stem_links_panel = cue_editor::view_stem_links(state, stem_link_selection);

    let save_section = view_save_section(state);

    container(
        column![
            header,
            Space::new().height(8.0),  // Explicit spacing after header
            main_row,
            cue_panel,        // Hot cues - directly under waveforms
            loop_panel,       // Saved loops - below hot cues
            stem_links_panel, // Stem links - below saved loops
            // Spacer pushes save section to bottom
            Space::new().height(Length::Fill),
            save_section,
        ],
        // No column spacing - use explicit Space widgets for control
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

    // Beat grid nudge controls
    let grid_label = text("Grid:").size(14);
    let nudge_left = button(text("<<").size(12))
        .padding([4, 8])
        .on_press(Message::NudgeBeatGridLeft);
    let nudge_right = button(text(">>").size(12))
        .padding([4, 8])
        .on_press(Message::NudgeBeatGridRight);

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
        grid_label,
        nudge_left,
        nudge_right,
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
