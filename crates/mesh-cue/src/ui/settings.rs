//! Settings modal UI
//!
//! Provides a modal dialog for editing application configuration.

use super::app::{Message, SettingsState};
use crate::config::BpmSource;
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length};

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let title = text("Settings").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Audio output section (at top)
    let audio_section = view_audio_output_section(state);

    // BPM Range section
    let bpm_section = view_bpm_section(state);

    // Display settings section
    let display_section = view_display_section(state);

    // Track name format section
    let format_section = view_track_name_format_section(state);

    // Status message (for save feedback)
    let status: Element<Message> = if !state.status.is_empty() {
        text(&state.status).size(14).into()
    } else {
        Space::new().height(20).into()
    };

    // Action buttons
    let cancel_btn = button(text("Cancel"))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let save_btn = button(text("Save"))
        .on_press(Message::SaveSettings)
        .style(button::primary);

    let actions = row![Space::new().width(Length::Fill), cancel_btn, save_btn]
        .spacing(10)
        .width(Length::Fill);

    // Scrollable content area
    let scrollable_content = scrollable(
        column![audio_section, bpm_section, display_section, format_section]
            .spacing(15)
    )
    .height(Length::Fixed(400.0));

    let content = column![header, scrollable_content, status, actions]
        .spacing(15)
        .width(Length::Fixed(450.0));

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Audio output settings section
fn view_audio_output_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Audio Output").size(18);
    let hint = text("Select the audio output for playback preview")
        .size(12);

    // Output device dropdown
    let output_label = text("Output:").size(14);
    let output_dropdown: Element<'_, Message> = if state.available_stereo_pairs.is_empty() {
        text("No JACK outputs available").size(12).into()
    } else {
        pick_list(
            state.available_stereo_pairs.clone(),
            state.available_stereo_pairs.get(state.selected_output_pair).cloned(),
            |pair| {
                // Find index of selected pair
                let idx = state.available_stereo_pairs.iter()
                    .position(|p| p == &pair)
                    .unwrap_or(0);
                Message::UpdateSettingsOutputPair(idx)
            },
        )
        .width(Length::Fixed(200.0))
        .into()
    };

    let output_row = row![output_label, Space::new().width(Length::Fill), output_dropdown]
        .spacing(10)
        .align_y(Alignment::Center);

    // Refresh button
    let refresh_btn = button(text("Refresh Ports").size(11))
        .on_press(Message::RefreshJackPorts)
        .style(button::secondary);

    container(
        column![
            section_title,
            hint,
            Space::new().height(5),
            output_row,
            Space::new().height(5),
            refresh_btn,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// BPM detection range settings
fn view_bpm_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Analysis").size(18);

    let subsection_title = text("BPM Detection Range").size(14);
    let hint = text("Set the expected BPM range for your music genre (e.g., DnB: 160-190)")
        .size(12);

    let min_label = text("Min Tempo:").size(14);
    let min_input = text_input("40", &state.draft_min_tempo)
        .on_input(Message::UpdateSettingsMinTempo)
        .width(Length::Fixed(80.0));
    let min_range = text("(40-180)").size(12);

    let min_row = row![min_label, min_input, min_range]
        .spacing(10)
        .align_y(Alignment::Center);

    let max_label = text("Max Tempo:").size(14);
    let max_input = text_input("208", &state.draft_max_tempo)
        .on_input(Message::UpdateSettingsMaxTempo)
        .width(Length::Fixed(80.0));
    let max_range = text("(60-250)").size(12);

    let max_row = row![max_label, max_input, max_range]
        .spacing(10)
        .align_y(Alignment::Center);

    // BPM Source subsection
    let source_title = text("BPM Analysis Source").size(14);
    let source_hint = text("Which audio to analyze for BPM detection (drums recommended)")
        .size(12);

    // Source selection buttons
    let source_options = [BpmSource::Drums, BpmSource::FullMix];
    let source_buttons: Vec<Element<Message>> = source_options
        .iter()
        .map(|&source| {
            let is_selected = state.draft_bpm_source == source;
            let btn = button(text(source.to_string()).size(12))
                .on_press(Message::UpdateSettingsBpmSource(source))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(90.0));
            btn.into()
        })
        .collect();

    let source_label = text("Source:").size(14);
    let source_row = row![
        source_label,
        row(source_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Parallel processes subsection
    let parallel_title = text("Parallel Analysis").size(14);
    let parallel_hint = text("Number of tracks to analyze simultaneously during batch import")
        .size(12);

    let parallel_label = text("Processes:").size(14);
    let parallel_input = text_input("4", &state.draft_parallel_processes)
        .on_input(Message::UpdateSettingsParallelProcesses)
        .width(Length::Fixed(80.0));
    let parallel_range = text("(1-16)").size(12);

    let parallel_row = row![parallel_label, parallel_input, parallel_range]
        .spacing(10)
        .align_y(Alignment::Center);

    container(
        column![
            section_title,
            subsection_title,
            hint,
            min_row,
            max_row,
            Space::new().height(10),
            source_title,
            source_hint,
            source_row,
            Space::new().height(10),
            parallel_title,
            parallel_hint,
            parallel_row,
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Display settings (waveform grid density)
fn view_display_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Display").size(18);

    let subsection_title = text("Overview Grid Density").size(14);
    let hint = text("Beat grid line spacing on the overview waveform (in bars)")
        .size(12);

    // Grid density buttons (4, 8, 16, 32 bars)
    let grid_sizes: [u32; 4] = [4, 8, 16, 32];
    let grid_buttons: Vec<Element<Message>> = grid_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_grid_bars == size;
            let btn = button(text(format!("{}", size)).size(12))
                .on_press(Message::UpdateSettingsGridBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(40.0));
            btn.into()
        })
        .collect();

    let grid_label = text("Grid (bars):").size(14);
    let grid_row = row![
        grid_label,
        row(grid_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Slicer buffer size section
    let slicer_title = text("Slicer Buffer Size").size(14);
    let slicer_hint = text("How many beats the 16 slices span (1 bar = 4 beats)")
        .size(12);

    // Slicer buffer buttons (1, 4, 8, 16 bars)
    let slicer_sizes: [u32; 4] = [1, 4, 8, 16];
    let slicer_labels = ["4 beats", "16 beats", "32 beats", "64 beats"];
    let slicer_buttons: Vec<Element<Message>> = slicer_sizes
        .iter()
        .zip(slicer_labels.iter())
        .map(|(&size, &label)| {
            let is_selected = state.draft_slicer_buffer_bars == size;
            let btn = button(text(label).size(12))
                .on_press(Message::UpdateSettingsSlicerBufferBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(70.0));
            btn.into()
        })
        .collect();

    let slicer_label = text("Buffer:").size(14);
    let slicer_row = row![
        slicer_label,
        row(slicer_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            subsection_title,
            hint,
            grid_row,
            Space::new().height(10),
            slicer_title,
            slicer_hint,
            slicer_row,
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Track name format template settings
fn view_track_name_format_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Import").size(18);

    let subsection_title = text("Track Name Format").size(14);
    let hint = text("Template for auto-filling track names from stem filenames")
        .size(12);

    let format_label = text("Format:").size(14);
    let format_input = text_input("{artist} - {name}", &state.draft_track_name_format)
        .on_input(Message::UpdateSettingsTrackNameFormat)
        .width(Length::Fixed(200.0));

    let format_row = row![format_label, format_input]
        .spacing(10)
        .align_y(Alignment::Center);

    let tags_hint = text("Tags: {artist}, {name}")
        .size(12);

    container(
        column![section_title, subsection_title, hint, format_row, tags_hint].spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
