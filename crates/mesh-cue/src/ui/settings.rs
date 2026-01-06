//! Settings modal UI
//!
//! Provides a modal dialog for editing application configuration.

use super::app::{Message, SettingsState};
use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Element, Length};

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<Message> {
    let title = text("Settings").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

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

    let content = column![header, bpm_section, display_section, format_section, status, actions]
        .spacing(20)
        .width(Length::Fixed(450.0));

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// BPM detection range settings
fn view_bpm_section(state: &SettingsState) -> Element<Message> {
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
fn view_display_section(state: &SettingsState) -> Element<Message> {
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

    container(
        column![section_title, subsection_title, hint, grid_row].spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Track name format template settings
fn view_track_name_format_section(state: &SettingsState) -> Element<Message> {
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
