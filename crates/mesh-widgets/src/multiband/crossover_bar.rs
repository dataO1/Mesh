//! Interactive crossover bar widget using conventional iced components
//!
//! Displays frequency bands on a log scale (20Hz-20kHz) with draggable dividers.
//! No Canvas used to avoid conflicts with existing waveform canvas (iced bug #3040).

use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{Alignment, Color, Element, Length, Point};

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

/// Single band display (clickable to add first split)
fn single_band_bar() -> Element<'static, MultibandEditorMessage> {
    // Calculate midpoint frequency for splitting (geometric mean of 20Hz-20kHz in log scale)
    let log_mid = (super::FREQ_MIN.log10() + super::FREQ_MAX.log10()) / 2.0;
    let mid_freq = 10.0_f32.powf(log_mid); // ~632 Hz

    let band_name = super::default_band_name(super::FREQ_MIN, super::FREQ_MAX);

    let content = container(
        column![
            text(band_name).size(12).color(TEXT_COLOR),
            text(format!(
                "{} - {}",
                format_freq(super::FREQ_MIN),
                format_freq(super::FREQ_MAX)
            ))
            .size(10)
            .color(Color::from_rgb(0.7, 0.7, 0.7)),
            text("click to split into bands")
                .size(9)
                .color(Color::from_rgba(1.0, 1.0, 1.0, 0.5)),
        ]
        .spacing(3)
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
    });

    // Make the entire bar clickable to add a band at midpoint
    mouse_area(content)
        .on_press(MultibandEditorMessage::AddBandAtFrequency(mid_freq))
        .into()
}

/// Multi-band display with colored sections and dividers
/// Wrapped in mouse_area for drag support
fn multi_band_bar(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let num_bands = state.bands.len();
    let dragging_index = state.dragging_crossover;

    // Can we add more bands?
    let can_add_band = num_bands < 3;

    // Build band segments
    let mut band_row_elements: Vec<Element<'_, MultibandEditorMessage>> = Vec::new();

    for (i, band) in state.bands.iter().enumerate() {
        // Calculate width proportion based on frequency range (log scale)
        let pos_start = freq_to_position(band.freq_low);
        let pos_end = freq_to_position(band.freq_high);
        let width_ratio = pos_end - pos_start;

        // Band segment - clickable to add a split at midpoint
        let segment = band_segment(i, band.freq_low, band.freq_high, width_ratio, can_add_band);
        band_row_elements.push(segment);

        // Add divider after each band except the last
        if i < num_bands - 1 {
            let crossover_freq = state.crossover_freqs.get(i).copied().unwrap_or(1000.0);
            let divider_is_dragging = state.dragging_crossover == Some(i);
            let divider = crossover_divider(i, crossover_freq, divider_is_dragging);
            band_row_elements.push(divider);
        }
    }

    // Frequency scale labels
    let scale_labels = frequency_scale_labels();

    let bar_content = container(
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
    // Fill available width
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(Color::from_rgb(0.15, 0.15, 0.17).into()),
        border: iced::Border {
            color: Color::from_rgb(0.3, 0.3, 0.35),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    });

    // Wrap in mouse_area for drag support
    let mut area = mouse_area(bar_content)
        .on_release(MultibandEditorMessage::EndDragCrossover);

    // Only track mouse movement when dragging
    if let Some(drag_idx) = dragging_index {
        // Get current frequency and last mouse X for relative calculation
        let current_freq = state.crossover_freqs.get(drag_idx).copied().unwrap_or(1000.0);
        let last_x = state.crossover_drag_last_x;

        area = area.on_move(move |point: Point| {
            // Use relative movement for precise drag tracking
            if let Some(prev_x) = last_x {
                // Calculate delta in pixels
                let delta_x = point.x - prev_x;

                // Apply logarithmic frequency change
                // Small movements for fine control, larger for coarse adjustment
                let log_freq = current_freq.log10();
                // Sensitivity scales with current position - more sensitive at lower frequencies
                let octave_scale = 0.001 * (1.0 + log_freq / 4.0);
                let new_log_freq = log_freq + delta_x * octave_scale;
                let new_freq = 10.0_f32.powf(new_log_freq).clamp(super::FREQ_MIN, super::FREQ_MAX);

                MultibandEditorMessage::DragCrossoverRelative {
                    new_freq,
                    mouse_x: point.x,
                }
            } else {
                // First move - just record position, use absolute calculation as fallback
                MultibandEditorMessage::DragCrossoverRelative {
                    new_freq: current_freq,
                    mouse_x: point.x,
                }
            }
        });
    }

    area.into()
}

/// A single band segment (clickable to add split at midpoint)
fn band_segment<'a>(
    index: usize,
    freq_low: f32,
    freq_high: f32,
    width_ratio: f32,
    can_add_band: bool,
) -> Element<'a, MultibandEditorMessage> {
    let color = BAND_COLORS[index % BAND_COLORS.len()];
    let band_name = super::default_band_name(freq_low, freq_high);

    // Use FillPortion for proportional sizing
    let portion = ((width_ratio * 1000.0) as u16).max(1);

    // Calculate midpoint frequency (log scale)
    let log_mid = (freq_low.log10() + freq_high.log10()) / 2.0;
    let mid_freq = 10.0_f32.powf(log_mid);

    let segment = container(
        column![
            text(band_name).size(10).color(TEXT_COLOR),
            text(format!("{} - {}", format_freq(freq_low), format_freq(freq_high)))
                .size(8)
                .color(Color::from_rgb(0.7, 0.7, 0.7)),
            if can_add_band {
                text("click to split").size(7).color(Color::from_rgba(1.0, 1.0, 1.0, 0.4))
            } else {
                text("").size(7)
            },
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
    });

    // Wrap in mouse_area if we can add bands
    if can_add_band {
        mouse_area(segment)
            .on_press(MultibandEditorMessage::AddBandAtFrequency(mid_freq))
            .into()
    } else {
        segment.into()
    }
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

    // Use mouse_area for immediate drag on mouse down (not button which needs click-release)
    mouse_area(divider_content)
        .on_press(MultibandEditorMessage::StartDragCrossover(index))
        .on_release(MultibandEditorMessage::EndDragCrossover)
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
