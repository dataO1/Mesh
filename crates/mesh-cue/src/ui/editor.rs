//! Track editor view

use super::app::{LoadedTrackState, Message};
use super::waveform::view_combined_waveform;
use super::{cue_editor, transport};
use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length};
use mesh_widgets::slice_editor;

/// Render the track editor
///
/// # Arguments
/// * `state` - The loaded track state
/// * `stem_link_selection` - Which stem slot is being linked (if any)
pub fn view(state: &LoadedTrackState, stem_link_selection: Option<usize>) -> Element<'_, Message> {
    let header = view_header(state);

    // Player controls (vertical, left of waveforms)
    let player_controls = transport::view(state);

    // Combined waveform canvas (zoomed detail view above overview)
    // Uses single canvas to work around iced bug #3040 where multiple Canvas widgets
    // don't render properly - only the first one shows.
    // Use interpolated position for smooth waveform animation during playback
    let waveforms = view_combined_waveform(&state.combined_waveform, state.interpolated_playhead_position());

    // Stem link buttons as vertical column (right of waveforms)
    let stem_links_column = cue_editor::view_stem_links_column(state, stem_link_selection);

    // Layout: player controls on left, waveforms center, stem links right
    // Align to top (Start) so content doesn't float in the middle
    let main_row = row![player_controls, waveforms, stem_links_column]
        .spacing(6)
        .align_y(Alignment::Start);

    // Hot cue buttons (single row of 8) - directly under waveforms
    let cue_panel = cue_editor::view(state);

    // Slice editor - below hot cues
    let slice_editor_widget = slice_editor(
        &state.slice_editor,
        |step, slice| Message::SliceEditorCellToggle { step, slice },
        Message::SliceEditorMuteToggle,
        Message::SliceEditorStemClick,
        Message::SliceEditorPresetSelect,
    );

    // Save presets button (next to slice editor)
    let save_presets_btn = button(text("Save Presets").size(11))
        .padding([4, 8])
        .on_press(Message::SaveSlicerPresets);

    let slice_editor_view = row![slice_editor_widget, save_presets_btn]
        .spacing(8)
        .align_y(Alignment::End);

    let save_section = view_save_section(state);

    container(
        column![
            header,
            Space::new().height(8.0),  // Explicit spacing after header
            main_row,
            cue_panel,         // Hot cues - directly under waveforms
            slice_editor_view, // Slice editor - below hot cues
            // Spacer pushes save section to bottom
            Space::new().height(Length::Fill),
            save_section,
        ],
        // No column spacing - use explicit Space widgets for control
    )
    .padding(15)
    .width(Length::Fill)
    .height(Length::FillPortion(3))  // Editor gets 3/4 of space (vs 1/4 for browsers)
    .into()
}

/// Header with track info and editable BPM/key
fn view_header(state: &LoadedTrackState) -> Element<'_, Message> {
    let track_name = state
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("Unknown Track"));

    let title = text(track_name).size(20);

    let bpm_label = text("BPM:");
    let bpm_minus = button(text("-").size(12))
        .padding([4, 8])
        .on_press(Message::DecreaseBpm);
    let bpm_input = text_input("BPM", &format!("{:.2}", state.bpm))
        .on_input(|s| {
            s.parse::<f64>()
                .map(Message::SetBpm)
                .unwrap_or(Message::SetBpm(state.bpm))
        })
        .width(Length::Fixed(80.0));
    let bpm_plus = button(text("+").size(12))
        .padding([4, 8])
        .on_press(Message::IncreaseBpm);

    let key_label = text("Key:");
    let key_input = text_input("Key", &state.key)
        .on_input(Message::SetKey)
        .width(Length::Fixed(60.0));

    // Beat grid controls
    let grid_label = text("Grid:").size(14);
    let nudge_left = button(text("<<").size(12))
        .padding([4, 8])
        .on_press(Message::NudgeBeatGridLeft);
    let nudge_right = button(text(">>").size(12))
        .padding([4, 8])
        .on_press(Message::NudgeBeatGridRight);
    let align_grid = button(text("│").size(14))
        .padding([4, 10])
        .on_press(Message::AlignBeatGridToPlayhead);

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
        bpm_minus,
        bpm_input,
        bpm_plus,
        key_label,
        key_input,
        grid_label,
        nudge_left,
        nudge_right,
        align_grid,
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// Save section
fn view_save_section(state: &LoadedTrackState) -> Element<'_, Message> {
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
