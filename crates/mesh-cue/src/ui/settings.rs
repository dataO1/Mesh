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

    let content = column![header, bpm_section, status, actions]
        .spacing(20)
        .width(Length::Fixed(400.0));

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

    container(
        column![section_title, subsection_title, hint, min_row, max_row].spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
