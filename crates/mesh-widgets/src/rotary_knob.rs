//! Rotary knob widget for mesh DJ applications
//!
//! A widget-based knob control using styled containers and sliders.
//! Avoids Canvas to prevent conflicts with the waveform canvas (iced bug #3040).

use iced::widget::{column, container, slider, text};
use iced::{Background, Border, Color, Element, Length};

/// State for a rotary knob (minimal - no Canvas cache needed)
#[derive(Debug, Default, Clone)]
pub struct RotaryKnobState {
    // Placeholder for future drag state if needed
    _placeholder: (),
}

impl RotaryKnobState {
    /// Create a new rotary knob state
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear any cached state (no-op for widget-based implementation)
    pub fn clear_cache(&mut self) {
        // No cache in widget-based implementation
    }
}

/// Style function for the knob container
fn knob_container_style(value: f32) -> container::Style {
    // Color based on value: blue (low) to orange (high)
    let accent_color = if value < 0.5 {
        Color::from_rgb(0.3, 0.5, 0.7)
    } else {
        Color::from_rgb(0.7, 0.5, 0.3)
    };

    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.12))),
        border: Border {
            color: accent_color,
            width: 2.0,
            radius: 16.0.into(), // Rounded to appear circular
        },
        ..Default::default()
    }
}

/// Create a rotary knob element using widgets (no Canvas)
///
/// # Arguments
/// * `_state` - Reference to knob state (unused in widget implementation, kept for API compatibility)
/// * `value` - Current value (0.0-1.0)
/// * `size` - Size of the knob in pixels
/// * `label` - Optional label to display below the knob
/// * `on_change` - Callback when value changes
///
/// # Returns
/// An Element that produces messages via the on_change callback
pub fn rotary_knob<'a, Message: Clone + 'a>(
    _state: &'a RotaryKnobState,
    value: f32,
    size: f32,
    label: Option<&'a str>,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a, Message> {
    let value = value.clamp(0.0, 1.0);

    // Display value as percentage
    let value_text = text(format!("{:.0}", value * 100.0))
        .size((size * 0.35).max(10.0))
        .color(Color::from_rgb(0.8, 0.8, 0.8));

    // Knob container with circular appearance
    let knob_visual = container(value_text)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_theme| knob_container_style(value));

    // Vertical slider for interaction (styled minimally)
    let knob_slider = slider(0.0..=1.0, value, on_change)
        .step(0.01)
        .width(Length::Fixed(size));

    // Stack: visual knob on top, slider below for interaction
    if let Some(label_text) = label {
        column![
            knob_visual,
            knob_slider,
            text(label_text).size(9).color(Color::from_rgb(0.6, 0.6, 0.6)),
        ]
        .spacing(2)
        .align_x(iced::Center)
        .into()
    } else {
        column![knob_visual, knob_slider]
            .spacing(2)
            .align_x(iced::Center)
            .into()
    }
}
