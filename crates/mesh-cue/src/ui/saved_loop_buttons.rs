//! Saved loop buttons component
//!
//! CDJ-style 8 saved loop buttons in a single row (below hot cues):
//! - Click on empty slot (when loop active) → Save current loop
//! - Click on saved loop → Jump to and activate that loop
//! - Shift+Click on saved loop → Clear/delete that loop

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, container, row, text};
use iced::{Alignment, Color, Element, Length, Theme};
use mesh_core::audio_file::SavedLoop;
use mesh_core::types::SAMPLE_RATE;

/// Colors for saved loop buttons (distinct from hot cue colors)
const LOOP_COLORS: [Color; 8] = [
    Color::from_rgb(0.2, 0.6, 0.8),  // Teal
    Color::from_rgb(0.3, 0.7, 0.5),  // Green
    Color::from_rgb(0.5, 0.5, 0.8),  // Purple
    Color::from_rgb(0.7, 0.5, 0.3),  // Orange
    Color::from_rgb(0.6, 0.4, 0.6),  // Magenta
    Color::from_rgb(0.4, 0.6, 0.4),  // Olive
    Color::from_rgb(0.6, 0.6, 0.3),  // Yellow
    Color::from_rgb(0.5, 0.4, 0.5),  // Gray-purple
];

/// Render the saved loop buttons (single row of 8 action buttons)
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // Check if loop is currently active (for save action)
    let loop_active = state.deck.as_ref().map_or(false, |d| d.is_loop_active());

    // Create all 8 saved loop buttons in a single row
    let buttons: Vec<Element<Message>> = (0..8)
        .map(|i| {
            let saved_loop = state.saved_loops.iter().find(|l| l.index == i as u8);
            create_loop_button(i, saved_loop, loop_active)
        })
        .collect();

    let loop_row = row(buttons).spacing(8).align_y(Alignment::Center);

    // Small label and buttons row
    let label = text("Loops:").size(11).color(Color::from_rgb(0.6, 0.6, 0.6));

    container(row![label, loop_row].spacing(8).align_y(Alignment::Center))
        .padding([4, 10])
        .width(Length::Fill)
        .into()
}

/// Create a single saved loop button
fn create_loop_button(
    index: usize,
    saved_loop: Option<&SavedLoop>,
    loop_active: bool,
) -> Element<'static, Message> {
    // Loop icon (↻) matches CDJ loop button aesthetic
    let loop_icon = "\u{21BB}";

    let label_text = if let Some(loop_) = saved_loop {
        // Show loop icon, number, and length
        let length_secs = (loop_.end_sample.saturating_sub(loop_.start_sample)) as f64 / SAMPLE_RATE as f64;
        if length_secs < 1.0 {
            format!("{}{}\n{:.0}ms", loop_icon, index + 1, length_secs * 1000.0)
        } else {
            format!("{}{}\n{:.1}s", loop_icon, index + 1, length_secs)
        }
    } else {
        // Empty slot - show icon and number
        format!("{}{}", loop_icon, index + 1)
    };

    // Match hot cue button dimensions: dynamic width, 44px height
    let btn = button(text(label_text).size(11).center())
        .width(Length::Fill)
        .height(Length::Fixed(44.0));

    if saved_loop.is_some() {
        // Has saved loop - click to jump to it
        btn.on_press(Message::JumpToSavedLoop(index))
            .style(move |theme: &Theme, status| {
                colored_button_style(theme, status, LOOP_COLORS[index])
            })
            .into()
    } else if loop_active {
        // Empty slot but loop is active - click to save current loop
        btn.on_press(Message::SaveLoop(index))
            .style(|theme: &Theme, status| {
                // Subtle highlighted style to indicate "can save here"
                save_available_style(theme, status)
            })
            .into()
    } else {
        // Empty slot, no loop active - disabled look
        btn.style(iced::widget::button::secondary)
            .into()
    }
}

/// Create a colored button style for saved loops
fn colored_button_style(
    _theme: &Theme,
    status: iced::widget::button::Status,
    color: Color,
) -> iced::widget::button::Style {
    let (bg_color, text_color) = match status {
        iced::widget::button::Status::Active => (color, Color::WHITE),
        iced::widget::button::Status::Hovered => {
            // Lighten on hover
            (
                Color::from_rgb(
                    (color.r + 0.15).min(1.0),
                    (color.g + 0.15).min(1.0),
                    (color.b + 0.15).min(1.0),
                ),
                Color::WHITE,
            )
        }
        iced::widget::button::Status::Pressed => {
            // Darken on press
            (
                Color::from_rgb(color.r * 0.8, color.g * 0.8, color.b * 0.8),
                Color::WHITE,
            )
        }
        iced::widget::button::Status::Disabled => (Color::from_rgb(0.25, 0.25, 0.25), Color::from_rgb(0.5, 0.5, 0.5)),
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

/// Style for empty slots when loop is active (can save)
fn save_available_style(
    _theme: &Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let (bg_color, text_color, border_color) = match status {
        iced::widget::button::Status::Active => (
            Color::from_rgb(0.2, 0.2, 0.2),
            Color::from_rgb(0.7, 0.7, 0.7),
            Color::from_rgb(0.4, 0.6, 0.4),  // Green border hint
        ),
        iced::widget::button::Status::Hovered => (
            Color::from_rgb(0.25, 0.3, 0.25),  // Slight green tint
            Color::WHITE,
            Color::from_rgb(0.5, 0.7, 0.5),
        ),
        iced::widget::button::Status::Pressed => (
            Color::from_rgb(0.3, 0.4, 0.3),
            Color::WHITE,
            Color::from_rgb(0.4, 0.6, 0.4),
        ),
        iced::widget::button::Status::Disabled => (
            Color::from_rgb(0.2, 0.2, 0.2),
            Color::from_rgb(0.5, 0.5, 0.5),
            Color::TRANSPARENT,
        ),
    };

    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg_color)),
        text_color,
        border: iced::Border {
            color: border_color,
            width: 1.5,
            radius: 4.0.into(),
        },
        shadow: iced::Shadow::default(),
        snap: false,
    }
}
