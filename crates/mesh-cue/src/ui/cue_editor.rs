//! Hot cue buttons component
//!
//! CDJ-style 8 hot cue action buttons:
//! - Click on set cue → Jump to that cue position
//! - Click on empty slot → Set cue at current playhead position
//! - Shift+Click on set cue → Clear/delete that cue point

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Alignment, Color, Element, Length, Theme};
use mesh_core::types::SAMPLE_RATE;

/// Cue button colors (matching waveform.rs CUE_COLORS)
const CUE_COLORS: [Color; 8] = [
    Color::from_rgb(1.0, 0.3, 0.3), // Red
    Color::from_rgb(1.0, 0.6, 0.0), // Orange
    Color::from_rgb(1.0, 1.0, 0.0), // Yellow
    Color::from_rgb(0.3, 1.0, 0.3), // Green
    Color::from_rgb(0.0, 0.8, 0.8), // Cyan
    Color::from_rgb(0.3, 0.3, 1.0), // Blue
    Color::from_rgb(0.8, 0.3, 0.8), // Purple
    Color::from_rgb(1.0, 0.5, 0.8), // Pink
];

/// Render the hot cue buttons (8 action buttons)
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    let title = text("Hot Cues").size(16);

    // Create top row buttons (1-4)
    let top_buttons: Vec<Element<Message>> = (0..4)
        .map(|i| {
            let cue = state.cue_points.iter().find(|c| c.index == i as u8);
            create_hot_cue_button(i, cue)
        })
        .collect();

    // Create bottom row buttons (5-8)
    let bottom_buttons: Vec<Element<Message>> = (4..8)
        .map(|i| {
            let cue = state.cue_points.iter().find(|c| c.index == i as u8);
            create_hot_cue_button(i, cue)
        })
        .collect();

    // Display in two rows of 4
    let top_row = row(top_buttons).spacing(8).align_y(Alignment::Center);
    let bottom_row = row(bottom_buttons).spacing(8).align_y(Alignment::Center);

    // Beat jump size selector
    let jump_sizes = [1, 4, 8, 16, 32];
    let jump_buttons: Vec<Element<Message>> = jump_sizes
        .iter()
        .map(|&size| {
            let is_selected = state.beat_jump_size() == size;
            let btn = button(text(format!("{}", size)).size(12))
                .on_press(Message::SetBeatJumpSize(size))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                });
            btn.into()
        })
        .collect();

    let jump_label = text("Beat Jump:").size(12);
    let jump_row = row![
        jump_label,
        row(jump_buttons).spacing(4).align_y(Alignment::Center),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // Shift+click hint
    let hint = text("Click to jump/set • Shift+click to clear").size(10);

    container(
        column![title, top_row, bottom_row, jump_row, hint,].spacing(8),
    )
    .padding(10)
    .width(Length::Fill)
    .into()
}

/// Create a single hot cue button
fn create_hot_cue_button(
    index: usize,
    cue: Option<&mesh_core::audio_file::CuePoint>,
) -> Element<'static, Message> {
    let label_text = if let Some(cue) = cue {
        // Show cue number and time
        let time = format_time_short(cue.sample_position);
        format!("{}\n{}", index + 1, time)
    } else {
        // Empty slot
        format!("{}", index + 1)
    };

    let btn = button(text(label_text).size(11).center())
        .width(Length::Fixed(55.0))
        .height(Length::Fixed(40.0));

    // If cue exists, use CDJ-style preview (hold to play, release to return)
    // Otherwise, click sets a new cue point
    if cue.is_some() {
        // Wrap in mouse_area for press/release detection (CDJ-style preview)
        let styled_btn = btn.style(move |theme: &Theme, status| {
            let color = CUE_COLORS[index];
            colored_button_style(theme, status, color)
        });

        mouse_area(styled_btn)
            .on_press(Message::HotCuePressed(index))
            .on_release(Message::HotCueReleased(index))
            .into()
    } else {
        // Empty slot - just set cue on click
        btn.on_press(Message::SetCuePoint(index))
            .style(iced::widget::button::secondary)
            .into()
    }
}

/// Create a colored button style
fn colored_button_style(
    _theme: &Theme,
    status: iced::widget::button::Status,
    color: Color,
) -> iced::widget::button::Style {
    let (bg_color, text_color) = match status {
        iced::widget::button::Status::Active => (color, Color::BLACK),
        iced::widget::button::Status::Hovered => {
            // Lighten on hover
            (
                Color::from_rgb(
                    (color.r + 0.2).min(1.0),
                    (color.g + 0.2).min(1.0),
                    (color.b + 0.2).min(1.0),
                ),
                Color::BLACK,
            )
        }
        iced::widget::button::Status::Pressed => {
            // Darken on press
            (
                Color::from_rgb(color.r * 0.8, color.g * 0.8, color.b * 0.8),
                Color::WHITE,
            )
        }
        iced::widget::button::Status::Disabled => (Color::from_rgb(0.3, 0.3, 0.3), Color::WHITE),
    };

    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg_color)),
        text_color,
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        shadow: iced::Shadow::default(),
        snap: false,
    }
}

/// Format sample position as short time string (S.ms)
fn format_time_short(samples: u64) -> String {
    let seconds = samples as f64 / SAMPLE_RATE as f64;
    if seconds < 60.0 {
        format!("{:.1}s", seconds)
    } else {
        let minutes = (seconds / 60.0).floor() as u64;
        let secs = (seconds % 60.0).floor() as u64;
        format!("{}:{:02}", minutes, secs)
    }
}
