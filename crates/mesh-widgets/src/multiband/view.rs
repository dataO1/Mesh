//! View function for the multiband editor widget

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::message::MultibandEditorMessage;
use super::state::{BandUiState, EffectUiState, MacroUiState, MultibandEditorState};
use super::{format_freq, freq_to_position};

// ─────────────────────────────────────────────────────────────────────────────
// Colors
// ─────────────────────────────────────────────────────────────────────────────

const BG_DARK: Color = Color::from_rgb(0.12, 0.12, 0.14);
const BG_MEDIUM: Color = Color::from_rgb(0.18, 0.18, 0.20);
const BG_LIGHT: Color = Color::from_rgb(0.25, 0.25, 0.28);
const BORDER_COLOR: Color = Color::from_rgb(0.35, 0.35, 0.40);
const TEXT_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);
const TEXT_SECONDARY: Color = Color::from_rgb(0.6, 0.6, 0.65);
const ACCENT_COLOR: Color = Color::from_rgb(0.3, 0.7, 0.9);
const MUTE_COLOR: Color = Color::from_rgb(0.8, 0.3, 0.3);
const SOLO_COLOR: Color = Color::from_rgb(0.9, 0.8, 0.2);
const BYPASS_COLOR: Color = Color::from_rgb(0.5, 0.5, 0.5);

// ─────────────────────────────────────────────────────────────────────────────
// Horizontal divider helper
// ─────────────────────────────────────────────────────────────────────────────

fn divider<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new())
        .height(Length::Fixed(1.0))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(BORDER_COLOR.into()),
            ..Default::default()
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Main view function
// ─────────────────────────────────────────────────────────────────────────────

/// Render the multiband editor as a modal overlay
///
/// Returns None if the editor is closed.
pub fn multiband_editor<'a>(
    state: &'a MultibandEditorState,
) -> Option<Element<'a, MultibandEditorMessage>> {
    if !state.is_open {
        return None;
    }

    let content = column![
        // Header with preset controls and close button
        header_row(state),
        divider(),
        // Crossover visualization bar
        crossover_bar(state),
        divider(),
        // Band lanes (scrollable if many bands)
        scrollable(
            column(
                state
                    .bands
                    .iter()
                    .map(|band| band_lane(band, state.any_soloed).into())
                    .collect::<Vec<Element<'_, MultibandEditorMessage>>>(),
            )
            .spacing(1),
        )
        .height(Length::Fill),
        // Add band button
        add_band_button(state.bands.len()),
        divider(),
        // Macro knobs row
        macro_bar(&state.macros),
    ]
    .spacing(8)
    .padding(16);

    // Wrap in modal container
    let modal = container(content)
        .width(Length::Fixed(800.0))
        .height(Length::Fixed(600.0))
        .style(|_| container::Style {
            background: Some(BG_DARK.into()),
            border: iced::Border {
                color: BORDER_COLOR,
                width: 2.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        });

    // Center the modal
    let centered = container(modal)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.7).into()),
            ..Default::default()
        });

    Some(centered.into())
}

// ─────────────────────────────────────────────────────────────────────────────
// Header row
// ─────────────────────────────────────────────────────────────────────────────

fn header_row(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    row![
        // Preset controls
        button(text("Load").size(12))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::OpenPresetBrowser),
        button(text("Save").size(12))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::SavePreset("default".to_string())),
        Space::new().width(Length::Fill),
        // Title
        text(format!("Deck {} - {}", state.deck + 1, state.stem_name))
            .size(16)
            .color(TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        // Close button
        button(text("×").size(18))
            .padding([2, 8])
            .on_press(MultibandEditorMessage::Close),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Crossover bar
// ─────────────────────────────────────────────────────────────────────────────

fn crossover_bar(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    // Simple text-based crossover display for now
    // TODO: Replace with canvas-based draggable dividers

    let freq_labels: Vec<Element<'_, MultibandEditorMessage>> = state
        .crossover_freqs
        .iter()
        .enumerate()
        .map(|(_i, &freq)| {
            let pos = freq_to_position(freq);
            let label = text(format_freq(freq)).size(11).color(ACCENT_COLOR);

            container(label)
                .width(Length::FillPortion((pos * 100.0) as u16))
                .into()
        })
        .collect();

    let freq_row = if freq_labels.is_empty() {
        row![text("Single band (no crossover)")
            .size(11)
            .color(TEXT_SECONDARY)]
    } else {
        row![
            text("20Hz").size(10).color(TEXT_SECONDARY),
            row(freq_labels).width(Length::Fill),
            text("20kHz").size(10).color(TEXT_SECONDARY),
        ]
    };

    container(
        column![
            text("Crossover Frequencies")
                .size(11)
                .color(TEXT_SECONDARY),
            freq_row.spacing(4),
        ]
        .spacing(4),
    )
    .padding(8)
    .style(|_| container::Style {
        background: Some(BG_MEDIUM.into()),
        ..Default::default()
    })
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Band lane
// ─────────────────────────────────────────────────────────────────────────────

fn band_lane<'a>(band: &'a BandUiState, any_soloed: bool) -> Element<'a, MultibandEditorMessage> {
    let band_idx = band.index;

    // Band header: name, freq range, solo/mute buttons
    let header = row![
        text(format!("Band {}: {}", band_idx + 1, band.name()))
            .size(12)
            .color(TEXT_PRIMARY),
        text(band.freq_range_str()).size(10).color(TEXT_SECONDARY),
        Space::new().width(Length::Fill),
        // Solo button
        button(
            text("S")
                .size(10)
                .color(if band.soloed { SOLO_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandSolo {
            band: band_idx,
            soloed: !band.soloed,
        }),
        // Mute button
        button(
            text("M")
                .size(10)
                .color(if band.muted { MUTE_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandMute {
            band: band_idx,
            muted: !band.muted,
        }),
        // Remove band button (if more than 1 band)
        button(text("×").size(10))
            .padding([2, 6])
            .on_press(MultibandEditorMessage::RemoveBand(band_idx)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    // Effect cards
    let effect_cards: Vec<Element<'_, MultibandEditorMessage>> = band
        .effects
        .iter()
        .enumerate()
        .map(|(effect_idx, effect)| effect_card(band_idx, effect_idx, effect).into())
        .collect();

    let effects_row = row(effect_cards).spacing(8).push(
        // Add effect button
        button(text("+").size(16))
            .padding([8, 16])
            .on_press(MultibandEditorMessage::OpenEffectPicker(band_idx)),
    );

    // Dim if muted or not soloed (when something else is soloed)
    let is_active = !band.muted && (!any_soloed || band.soloed);
    let bg_color = if is_active { BG_MEDIUM } else { BG_DARK };

    container(column![header, effects_row].spacing(8))
        .padding(8)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(bg_color.into()),
            border: iced::Border {
                color: BORDER_COLOR,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Effect card
// ─────────────────────────────────────────────────────────────────────────────

fn effect_card<'a>(
    band_idx: usize,
    effect_idx: usize,
    effect: &'a EffectUiState,
) -> Element<'a, MultibandEditorMessage> {
    let name_color = if effect.bypassed {
        BYPASS_COLOR
    } else {
        TEXT_PRIMARY
    };

    // Effect header: name, bypass indicator, remove button
    let header = row![
        text(effect.short_name()).size(11).color(name_color),
        Space::new().width(Length::Fill),
        // Bypass toggle
        button(
            text(if effect.bypassed { "○" } else { "●" })
                .size(10)
                .color(name_color)
        )
        .padding([2, 4])
        .on_press(MultibandEditorMessage::ToggleEffectBypass {
            band: band_idx,
            effect: effect_idx,
        }),
        // Remove button
        button(text("×").size(10))
            .padding([2, 4])
            .on_press(MultibandEditorMessage::RemoveEffect {
                band: band_idx,
                effect: effect_idx,
            }),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    // Parameter display (simplified - show first 4 values)
    let param_count = effect.param_values.len().min(4);
    let params: Vec<Element<'_, MultibandEditorMessage>> = (0..param_count)
        .map(|i| {
            let value = effect.param_values.get(i).copied().unwrap_or(0.5);
            let name = effect
                .param_names
                .get(i)
                .map(|s| s.as_str())
                .unwrap_or("?");

            column![
                text(format!("{:.0}%", value * 100.0))
                    .size(9)
                    .color(TEXT_SECONDARY),
                text(name).size(8).color(TEXT_SECONDARY),
            ]
            .spacing(2)
            .align_x(Alignment::Center)
            .width(Length::Fixed(28.0))
            .into()
        })
        .collect();

    let params_row = row(params).spacing(2);

    container(column![header, params_row].spacing(4))
        .padding(6)
        .width(Length::Fixed(120.0))
        .style(|_| container::Style {
            background: Some(BG_LIGHT.into()),
            border: iced::Border {
                color: BORDER_COLOR,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Add band button
// ─────────────────────────────────────────────────────────────────────────────

fn add_band_button(current_bands: usize) -> Element<'static, MultibandEditorMessage> {
    if current_bands >= 8 {
        text("Maximum 8 bands")
            .size(11)
            .color(TEXT_SECONDARY)
            .into()
    } else {
        button(
            row![text("+").size(14), text("Add Band").size(12),]
                .spacing(4)
                .align_y(Alignment::Center),
        )
        .padding([6, 16])
        .on_press(MultibandEditorMessage::AddBand)
        .into()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Macro bar
// ─────────────────────────────────────────────────────────────────────────────

fn macro_bar(macros: &[MacroUiState]) -> Element<'_, MultibandEditorMessage> {
    let macro_widgets: Vec<Element<'_, MultibandEditorMessage>> = macros
        .iter()
        .map(|m| {
            column![
                // Simplified macro display
                text(format!("{:.0}%", m.value * 100.0))
                    .size(10)
                    .color(TEXT_SECONDARY),
                text(&m.name).size(9).color(if m.mapping_count > 0 {
                    ACCENT_COLOR
                } else {
                    TEXT_SECONDARY
                }),
            ]
            .spacing(2)
            .align_x(Alignment::Center)
            .width(Length::Fixed(80.0))
            .into()
        })
        .collect();

    container(
        column![
            text("Macros").size(11).color(TEXT_SECONDARY),
            row(macro_widgets).spacing(8),
        ]
        .spacing(4),
    )
    .padding(8)
    .style(|_| container::Style {
        background: Some(BG_MEDIUM.into()),
        ..Default::default()
    })
    .into()
}
