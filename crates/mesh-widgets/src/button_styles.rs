//! Material 3D button styling for mesh DJ applications
//!
//! Provides consistent button styling with raised/pressed effects:
//! - Press/release buttons: temporary visual change while held
//! - Toggle buttons: permanent "pressed in" look when active

use iced::widget::button::{Status, Style};
use iced::{Background, Border, Color, Shadow, Vector};

/// Default button background color
pub const DEFAULT_BG: Color = Color::from_rgb(0.25, 0.25, 0.25);

/// Active/enabled button color
pub const ACTIVE_BG: Color = Color::from_rgb(0.3, 0.6, 0.9);

/// Shadow offset for raised buttons
const SHADOW_OFFSET: Vector = Vector::new(2.0, 2.0);

/// Shadow blur for raised buttons
const SHADOW_BLUR: f32 = 3.0;

/// Lighten a color by a factor (0.0-1.0)
fn lighten(color: Color, factor: f32) -> Color {
    Color::from_rgb(
        (color.r + factor).min(1.0),
        (color.g + factor).min(1.0),
        (color.b + factor).min(1.0),
    )
}

/// Darken a color by a factor (0.0-1.0)
fn darken(color: Color, factor: f32) -> Color {
    Color::from_rgb(
        (color.r * (1.0 - factor)).max(0.0),
        (color.g * (1.0 - factor)).max(0.0),
        (color.b * (1.0 - factor)).max(0.0),
    )
}

/// Create a raised 3D button style (shadow on bottom-right)
fn raised_style(base_color: Color) -> Style {
    Style {
        background: Some(Background::Color(base_color)),
        text_color: Color::WHITE,
        border: Border {
            color: lighten(base_color, 0.1),
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.4),
            offset: SHADOW_OFFSET,
            blur_radius: SHADOW_BLUR,
        },
        snap: false,
    }
}

/// Create a pressed 3D button style (reduced shadow, slight offset)
fn pressed_style(base_color: Color) -> Style {
    Style {
        background: Some(Background::Color(darken(base_color, 0.15))),
        text_color: Color::WHITE,
        border: Border {
            color: darken(base_color, 0.2),
            width: 1.0,
            radius: 4.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.2),
            offset: Vector::new(0.5, 0.5),
            blur_radius: 1.0,
        },
        snap: false,
    }
}

/// Create a flat disabled button style
fn disabled_style() -> Style {
    Style {
        background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
        text_color: Color::from_rgb(0.5, 0.5, 0.5),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        shadow: Shadow::default(),
        snap: false,
    }
}

/// Style function for press/release buttons (momentary)
///
/// These buttons have:
/// - Raised appearance when not pressed
/// - "Pressed in" appearance while held
///
/// Use with `.style(|theme, status| press_release_style(status, base_color))`
pub fn press_release_style(status: Status, base_color: Color) -> Style {
    match status {
        Status::Active => raised_style(base_color),
        Status::Hovered => raised_style(lighten(base_color, 0.08)),
        Status::Pressed => pressed_style(base_color),
        Status::Disabled => disabled_style(),
    }
}

/// Style function for toggle buttons
///
/// These buttons have:
/// - Raised appearance when inactive
/// - "Pressed in" appearance when active (permanently until toggled again)
///
/// Use with `.style(|theme, status| toggle_style(status, is_active, active_color))`
pub fn toggle_style(status: Status, is_active: bool, active_color: Color) -> Style {
    if is_active {
        // Active state: pressed-in look with active color
        match status {
            Status::Active => pressed_style(active_color),
            Status::Hovered => pressed_style(lighten(active_color, 0.08)),
            Status::Pressed => pressed_style(darken(active_color, 0.1)),
            Status::Disabled => disabled_style(),
        }
    } else {
        // Inactive state: raised look with default color
        match status {
            Status::Active => raised_style(DEFAULT_BG),
            Status::Hovered => raised_style(lighten(DEFAULT_BG, 0.08)),
            Status::Pressed => pressed_style(DEFAULT_BG),
            Status::Disabled => disabled_style(),
        }
    }
}

/// Style function for colored buttons (like hot cues)
///
/// Similar to press_release_style but takes a custom color
pub fn colored_style(status: Status, color: Color) -> Style {
    match status {
        Status::Active => raised_style(color),
        Status::Hovered => raised_style(lighten(color, 0.15)),
        Status::Pressed => pressed_style(color),
        Status::Disabled => disabled_style(),
    }
}

/// Style function for colored toggle buttons
///
/// Inactive: default gray raised
/// Active: colored pressed-in look
pub fn colored_toggle_style(status: Status, is_active: bool, active_color: Color) -> Style {
    if is_active {
        match status {
            Status::Active => pressed_style(active_color),
            Status::Hovered => pressed_style(lighten(active_color, 0.1)),
            Status::Pressed => pressed_style(darken(active_color, 0.1)),
            Status::Disabled => disabled_style(),
        }
    } else {
        match status {
            Status::Active => raised_style(DEFAULT_BG),
            Status::Hovered => raised_style(lighten(DEFAULT_BG, 0.08)),
            Status::Pressed => pressed_style(DEFAULT_BG),
            Status::Disabled => disabled_style(),
        }
    }
}
