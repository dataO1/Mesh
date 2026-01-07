//! Settings modal UI for mesh-player
//!
//! Provides a modal dialog for editing player configuration.

use super::app::Message;
use crate::config::LOOP_LENGTH_OPTIONS;
use iced::widget::{button, column, container, row, text, toggler, Space};
use iced::{Alignment, Element, Length};

/// Settings state for the modal
#[derive(Debug, Clone)]
pub struct SettingsState {
    /// Whether the settings modal is open
    pub is_open: bool,
    /// Draft loop length index (0-6)
    pub draft_loop_length_index: usize,
    /// Draft zoom bars
    pub draft_zoom_bars: u32,
    /// Draft grid bars
    pub draft_grid_bars: u32,
    /// Draft phase sync enabled
    pub draft_phase_sync: bool,
    /// Status message (for save feedback)
    pub status: String,
}

impl SettingsState {
    /// Create settings state from current config
    pub fn from_config(config: &crate::config::PlayerConfig) -> Self {
        Self {
            is_open: false,
            draft_loop_length_index: config.display.default_loop_length_index,
            draft_zoom_bars: config.display.default_zoom_bars,
            draft_grid_bars: config.display.grid_bars,
            draft_phase_sync: config.audio.phase_sync,
            status: String::new(),
        }
    }

    /// Create default settings state
    pub fn new() -> Self {
        Self {
            is_open: false,
            draft_loop_length_index: 2, // 4 beats (index 2 in new array)
            draft_zoom_bars: 8,
            draft_grid_bars: 8,
            draft_phase_sync: true, // Enabled by default
            status: String::new(),
        }
    }
}

impl Default for SettingsState {
    fn default() -> Self {
        Self::new()
    }
}

/// Render the settings modal content
pub fn view(state: &SettingsState) -> Element<'_, Message> {
    let title = text("Settings").size(24);
    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseSettings)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    // Loop length section
    let loop_section = view_loop_section(state);

    // Display settings section
    let display_section = view_display_section(state);

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

    let content = column![header, loop_section, display_section, status, actions]
        .spacing(20)
        .width(Length::Fixed(450.0));

    container(content)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Playback settings (loop length, phase sync)
fn view_loop_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Playback").size(18);

    // Phase sync toggle
    let phase_sync_label = text("Automatic Beat Sync").size(14);
    let phase_sync_hint = text("Automatically align beats when starting playback or hot cues")
        .size(12);
    let phase_sync_toggle = toggler(state.draft_phase_sync)
        .on_toggle(Message::UpdateSettingsPhaseSync);
    let phase_sync_row = row![
        column![phase_sync_label, phase_sync_hint].spacing(4),
        Space::new().width(Length::Fill),
        phase_sync_toggle,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Loop length section
    let subsection_title = text("Default Loop/Beat Jump Length").size(14);
    let hint = text("Loop length also controls beat jump distance")
        .size(12);

    // Loop length buttons (0.25, 0.5, 1, 2, 4, 8, 16 beats)
    let loop_buttons: Vec<Element<Message>> = LOOP_LENGTH_OPTIONS
        .iter()
        .enumerate()
        .map(|(idx, &beats)| {
            let is_selected = state.draft_loop_length_index == idx;
            let label = if beats < 1.0 {
                format!("{:.2}", beats)
            } else {
                format!("{:.0}", beats)
            };
            let btn = button(text(label).size(11))
                .on_press(Message::UpdateSettingsLoopLength(idx))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let loop_label = text("Beats:").size(14);
    let loop_row = row![
        loop_label,
        row(loop_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            phase_sync_row,
            Space::new().height(10),
            subsection_title,
            hint,
            loop_row
        ]
        .spacing(10),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}

/// Display settings (waveform zoom and grid)
fn view_display_section(state: &SettingsState) -> Element<'_, Message> {
    let section_title = text("Display").size(18);

    // Zoom level section
    let zoom_subsection = text("Default Zoomed Waveform Level").size(14);
    let zoom_hint = text("Number of bars visible in zoomed waveform view")
        .size(12);

    let zoom_sizes: [u32; 6] = [2, 4, 8, 16, 32, 64];
    let zoom_buttons: Vec<Element<Message>> = zoom_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_zoom_bars == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press(Message::UpdateSettingsZoomBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let zoom_label = text("Bars:").size(14);
    let zoom_row = row![
        zoom_label,
        row(zoom_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // Grid density section
    let grid_subsection = text("Overview Grid Density").size(14);
    let grid_hint = text("Beat grid line spacing on the overview waveform")
        .size(12);

    let grid_sizes: [u32; 4] = [4, 8, 16, 32];
    let grid_buttons: Vec<Element<Message>> = grid_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.draft_grid_bars == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press(Message::UpdateSettingsGridBars(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(36.0));
            btn.into()
        })
        .collect();

    let grid_label = text("Bars:").size(14);
    let grid_row = row![
        grid_label,
        row(grid_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    container(
        column![
            section_title,
            zoom_subsection,
            zoom_hint,
            zoom_row,
            Space::new().height(10),
            grid_subsection,
            grid_hint,
            grid_row,
        ]
        .spacing(8),
    )
    .padding(15)
    .width(Length::Fill)
    .into()
}
