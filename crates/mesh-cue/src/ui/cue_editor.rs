//! Hot cue buttons component
//!
//! CDJ-style 8 hot cue buttons in a single row:
//! - Click on set cue → Jump to that cue position
//! - Click on empty slot → Set cue at current playhead position
//! - Shift+Click on set cue → Clear/delete that cue point

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, container, mouse_area, row, text};
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

/// Render the hot cue buttons (single row of 8 action buttons)
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // Create all 8 hot cue buttons in a single row
    let buttons: Vec<Element<Message>> = (0..8)
        .map(|i| {
            let cue = state.cue_points.iter().find(|c| c.index == i as u8);
            create_hot_cue_button(i, cue)
        })
        .collect();

    let hot_cue_row = row(buttons).spacing(8).align_y(Alignment::Center);

    container(hot_cue_row)
        .padding(10)
        .width(Length::Fill)
        .center_x(Length::Fill)
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
        .width(Length::Fixed(60.0))
        .height(Length::Fixed(44.0));

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
