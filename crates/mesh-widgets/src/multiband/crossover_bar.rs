//! Interactive crossover bar widget using conventional iced components
//!
//! Displays frequency bands on a log scale (20Hz-20kHz) with draggable dividers.
//! No Canvas used to avoid conflicts with existing waveform canvas (iced bug #3040).

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::message::MultibandEditorMessage;
use super::state::MultibandEditorState;
use super::{format_freq, freq_to_position};

// ─────────────────────────────────────────────────────────────────────────────
// Colors
// ─────────────────────────────────────────────────────────────────────────────

const BAND_COLORS: [Color; 8] = [
    Color::from_rgb(0.2, 0.4, 0.6),   // Sub - deep blue
    Color::from_rgb(0.3, 0.5, 0.4),   // Bass - teal
    Color::from_rgb(0.4, 0.5, 0.3),   // Low-mid - olive
    Color::from_rgb(0.5, 0.5, 0.2),   // Mid - yellow-green
    Color::from_rgb(0.6, 0.4, 0.2),   // High-mid - orange
    Color::from_rgb(0.6, 0.3, 0.3),   // Presence - salmon
    Color::from_rgb(0.5, 0.3, 0.5),   // Air - purple
    Color::from_rgb(0.4, 0.4, 0.5),   // Extra - gray-blue
];

const DIVIDER_COLOR: Color = Color::from_rgb(0.9, 0.9, 0.9);
const DIVIDER_HOVER_COLOR: Color = Color::from_rgb(1.0, 0.8, 0.3);
const TEXT_COLOR: Color = Color::from_rgb(0.85, 0.85, 0.85);
const LABEL_BG: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.5);

/// Height of the crossover bar
pub const CROSSOVER_BAR_HEIGHT: f32 = 60.0;

/// Width of draggable divider hitbox
const DIVIDER_WIDTH: f32 = 12.0;

// ─────────────────────────────────────────────────────────────────────────────
// Crossover bar view
// ─────────────────────────────────────────────────────────────────────────────

/// Render the interactive crossover bar
pub fn crossover_bar(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let num_bands = state.bands.len();

    if num_bands == 1 {
        // Single band mode - just show a simple bar
        return single_band_bar();
    }

    // Multi-band mode - show bands with dividers
    multi_band_bar(state)
}

/// Single band display (no crossovers)
fn single_band_bar() -> Element<'static, MultibandEditorMessage> {
    container(
        column![
            text("Single Band Mode")
                .size(11)
                .color(TEXT_COLOR),
            text("Click '+ Add Band' to enable multiband processing")
                .size(10)
                .color(Color::from_rgb(0.6, 0.6, 0.6)),
        ]
        .spacing(4)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(CROSSOVER_BAR_HEIGHT))
    .center_x(Length::Fill)
    .center_y(Length::Fixed(CROSSOVER_BAR_HEIGHT))
    .style(|_| container::Style {
        background: Some(BAND_COLORS[0].into()),
        border: iced::Border {
            color: Color::from_rgb(0.3, 0.3, 0.35),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Multi-band display with colored sections and dividers
fn multi_band_bar(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let num_bands = state.bands.len();

    // Build band segments
    let mut band_row_elements: Vec<Element<'_, MultibandEditorMessage>> = Vec::new();

    for (i, band) in state.bands.iter().enumerate() {
        // Calculate width proportion based on frequency range (log scale)
        let pos_start = freq_to_position(band.freq_low);
        let pos_end = freq_to_position(band.freq_high);
        let width_ratio = pos_end - pos_start;

        // Band segment
        let band_segment = band_segment(i, band.freq_low, band.freq_high, width_ratio);
        band_row_elements.push(band_segment);

        // Add divider after each band except the last
        if i < num_bands - 1 {
            let crossover_freq = state.crossover_freqs.get(i).copied().unwrap_or(1000.0);
            let is_dragging = state.dragging_crossover == Some(i);
            let divider = crossover_divider(i, crossover_freq, is_dragging);
            band_row_elements.push(divider);
        }
    }

    // Frequency scale labels
    let scale_labels = frequency_scale_labels();

    container(
        column![
            // Band segments row
            row(band_row_elements)
                .height(Length::Fixed(CROSSOVER_BAR_HEIGHT - 20.0))
                .width(Length::Fill),
            // Frequency scale
            scale_labels,
        ]
        .spacing(2),
    )
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgb(0.15, 0.15, 0.17).into()),
        border: iced::Border {
            color: Color::from_rgb(0.3, 0.3, 0.35),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A single band segment
fn band_segment<'a>(
    index: usize,
    freq_low: f32,
    freq_high: f32,
    width_ratio: f32,
) -> Element<'a, MultibandEditorMessage> {
    let color = BAND_COLORS[index % BAND_COLORS.len()];
    let band_name = super::default_band_name(freq_low, freq_high);

    // Use FillPortion for proportional sizing
    let portion = ((width_ratio * 1000.0) as u16).max(1);

    container(
        column![
            text(band_name).size(10).color(TEXT_COLOR),
            text(format!("{} - {}", format_freq(freq_low), format_freq(freq_high)))
                .size(8)
                .color(Color::from_rgb(0.7, 0.7, 0.7)),
        ]
        .spacing(2)
        .align_x(Alignment::Center),
    )
    .width(Length::FillPortion(portion))
    .height(Length::Fill)
    .center_x(Length::FillPortion(portion))
    .center_y(Length::Fill)
    .style(move |_| container::Style {
        background: Some(color.into()),
        ..Default::default()
    })
    .into()
}

/// Draggable crossover divider
fn crossover_divider<'a>(
    index: usize,
    freq: f32,
    is_dragging: bool,
) -> Element<'a, MultibandEditorMessage> {
    let color = if is_dragging {
        DIVIDER_HOVER_COLOR
    } else {
        DIVIDER_COLOR
    };

    // The divider is a narrow clickable area with a frequency label
    let divider_content = container(
        column![
            // Vertical line indicator
            container(Space::new())
                .width(Length::Fixed(2.0))
                .height(Length::Fixed(20.0))
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    ..Default::default()
                }),
            // Frequency label
            container(text(format_freq(freq)).size(8).color(color))
                .padding([1, 3])
                .style(|_| container::Style {
                    background: Some(LABEL_BG.into()),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        ]
        .spacing(2)
        .align_x(Alignment::Center),
    )
    .width(Length::Fixed(DIVIDER_WIDTH))
    .height(Length::Fill)
    .center_x(Length::Fixed(DIVIDER_WIDTH))
    .center_y(Length::Fill);

    // Wrap in a button for click handling
    // Note: For proper drag support, we'd need mouse_area with position tracking
    // For now, use buttons that increment/decrement on click
    button(divider_content)
        .padding(0)
        .on_press(MultibandEditorMessage::StartDragCrossover(index))
        .style(move |_theme, _status| button::Style {
            background: Some(Color::TRANSPARENT.into()),
            border: iced::Border::default(),
            ..Default::default()
        })
        .into()
}

/// Frequency scale labels at the bottom
fn frequency_scale_labels() -> Element<'static, MultibandEditorMessage> {
    let scale_color = Color::from_rgb(0.5, 0.5, 0.55);

    // Simple evenly-spaced labels for key frequency markers
    row![
        text("20").size(8).color(scale_color),
        Space::new().width(Length::Fill),
        text("100").size(8).color(scale_color),
        Space::new().width(Length::Fill),
        text("1k").size(8).color(scale_color),
        Space::new().width(Length::Fill),
        text("10k").size(8).color(scale_color),
        Space::new().width(Length::Fill),
        text("20k").size(8).color(scale_color),
    ]
    .height(Length::Fixed(16.0))
    .padding([0, 4])
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Crossover adjustment buttons (alternative to dragging)
// ─────────────────────────────────────────────────────────────────────────────

/// Render crossover adjustment controls (for when dragging isn't practical)
pub fn crossover_controls(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    if state.crossover_freqs.is_empty() {
        return Space::new().into();
    }

    let controls: Vec<Element<'_, MultibandEditorMessage>> = state
        .crossover_freqs
        .iter()
        .enumerate()
        .map(|(i, &freq)| {
            row![
                text(format!("X{}: ", i + 1)).size(10).color(TEXT_COLOR),
                // Decrease button
                button(text("◀").size(10))
                    .padding([2, 6])
                    .on_press(MultibandEditorMessage::DragCrossover(freq * 0.9)),
                // Current value
                text(format_freq(freq)).size(10).color(DIVIDER_HOVER_COLOR),
                // Increase button
                button(text("▶").size(10))
                    .padding([2, 6])
                    .on_press(MultibandEditorMessage::DragCrossover(freq * 1.1)),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();

    row(controls).spacing(16).into()
}
