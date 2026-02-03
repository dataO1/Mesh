//! View function for the multiband editor widget

use iced::widget::{button, column, container, mouse_area, row, scrollable, slider, text, Space};
use iced::{Alignment, Color, Element, Length};

use super::crossover_bar::crossover_bar;
use super::message::MultibandEditorMessage;
use super::state::{BandUiState, EffectUiState, MacroUiState, MultibandEditorState};

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

    // Build band columns (scrollable horizontally if many bands)
    let dragging_macro = state.dragging_macro;
    let band_columns: Vec<Element<'_, MultibandEditorMessage>> = state
        .bands
        .iter()
        .map(|band| band_column(band, state.any_soloed, dragging_macro).into())
        .collect();

    let content = column![
        // Header with preset controls and close button
        header_row(state),
        divider(),
        // Crossover visualization bar
        crossover_bar(state),
        divider(),
        // Band columns (fill available width)
        row(band_columns)
            .spacing(4)
            .padding([0, 4])
            .width(Length::Fill)
            .height(Length::Fill),
        // Add band button
        add_band_button(state.bands.len()),
        divider(),
        // Macro knobs row (draggable for mapping)
        macro_bar(&state.macros, state.dragging_macro),
    ]
    .spacing(8)
    .padding(16);

    // Wrap in modal container (fills most of the screen)
    let modal = container(content)
        .width(Length::FillPortion(9))  // ~90% of available width
        .height(Length::FillPortion(9)) // ~90% of available height
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
// Band column
// ─────────────────────────────────────────────────────────────────────────────

fn band_column<'a>(band: &'a BandUiState, any_soloed: bool, dragging_macro: Option<usize>) -> Element<'a, MultibandEditorMessage> {
    let band_idx = band.index;

    // Band header: name and freq range
    let header = column![
        text(format!("Band {}", band_idx + 1))
            .size(12)
            .color(TEXT_PRIMARY),
        text(band.name())
            .size(10)
            .color(TEXT_SECONDARY),
        text(band.freq_range_str())
            .size(9)
            .color(TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    // Control buttons row: Solo, Mute, Remove
    let controls = row![
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
        // Remove band button
        button(text("×").size(10))
            .padding([2, 6])
            .on_press(MultibandEditorMessage::RemoveBand(band_idx)),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    // Effect cards stacked vertically
    let effect_cards: Vec<Element<'_, MultibandEditorMessage>> = band
        .effects
        .iter()
        .enumerate()
        .map(|(effect_idx, effect)| effect_card(band_idx, effect_idx, effect, dragging_macro).into())
        .collect();

    let effects_column = column(effect_cards)
        .spacing(4)
        .push(
            // Add effect button at the bottom
            button(text("+ Add Effect").size(10))
                .padding([6, 12])
                .on_press(MultibandEditorMessage::OpenEffectPicker(band_idx)),
        );

    // Dim if muted or not soloed (when something else is soloed)
    let is_active = !band.muted && (!any_soloed || band.soloed);
    let bg_color = if is_active { BG_MEDIUM } else { BG_DARK };

    container(
        column![
            header,
            controls,
            scrollable(effects_column)
                .height(Length::Fill)
        ]
        .spacing(8)
        .align_x(Alignment::Center)
    )
    .padding(8)
    .width(Length::FillPortion(1))
    .height(Length::Fill)
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
// Effect card (compact for column layout)
// ─────────────────────────────────────────────────────────────────────────────

fn effect_card<'a>(
    band_idx: usize,
    effect_idx: usize,
    effect: &'a EffectUiState,
    dragging_macro: Option<usize>,
) -> Element<'a, MultibandEditorMessage> {
    let name_color = if effect.bypassed {
        BYPASS_COLOR
    } else {
        TEXT_PRIMARY
    };

    // Effect header: name and controls
    let header = row![
        text(&effect.name).size(11).color(name_color),
        Space::new().width(Length::Fill),
        // Bypass toggle
        button(
            text(if effect.bypassed { "○" } else { "●" })
                .size(9)
                .color(name_color)
        )
        .padding([1, 3])
        .on_press(MultibandEditorMessage::ToggleEffectBypass {
            band: band_idx,
            effect: effect_idx,
        }),
        // Remove button
        button(text("×").size(9))
            .padding([1, 3])
            .on_press(MultibandEditorMessage::RemoveEffect {
                band: band_idx,
                effect: effect_idx,
            }),
    ]
    .spacing(2)
    .align_y(Alignment::Center);

    // Parameter sliders (show first 8 params)
    let param_count = effect.param_values.len().min(8);
    let param_controls: Vec<Element<'_, MultibandEditorMessage>> = (0..param_count)
        .map(|param_idx| {
            let param_name = effect
                .param_names
                .get(param_idx)
                .map(|s| s.as_str())
                .unwrap_or("P");
            let param_value = effect.param_values.get(param_idx).copied().unwrap_or(0.5);

            // Check if this param has a macro mapping
            let mapping = effect.param_mappings.get(param_idx);
            let mapped_macro = mapping.and_then(|m| m.macro_index);
            let is_mapped = mapped_macro.is_some();

            // Highlight color: green if mapped, blue highlight if dragging over
            let (label_color, border_color) = if dragging_macro.is_some() {
                (ACCENT_COLOR, ACCENT_COLOR) // Highlight as drop target
            } else if is_mapped {
                (Color::from_rgb(0.4, 0.8, 0.4), Color::from_rgb(0.4, 0.8, 0.4)) // Green for mapped
            } else {
                (TEXT_SECONDARY, Color::TRANSPARENT)
            };

            // Label shows macro number if mapped
            let label_text = if let Some(macro_idx) = mapped_macro {
                format!("M{}:", macro_idx + 1)
            } else {
                format!("{}:", &param_name[..param_name.len().min(3)])
            };

            // Compact param row: label + slider
            let param_row = row![
                text(label_text)
                    .size(8)
                    .color(label_color)
                    .width(Length::Fixed(28.0)),
                slider(0.0..=1.0, param_value, move |v| {
                    MultibandEditorMessage::SetEffectParam {
                        band: band_idx,
                        effect: effect_idx,
                        param: param_idx,
                        value: v,
                    }
                })
                .step(0.01)
                .width(Length::Fill),
            ]
            .spacing(2)
            .align_y(Alignment::Center);

            // Wrap in mouse_area for drop target when dragging
            let content: Element<'_, MultibandEditorMessage> = if let Some(macro_idx) = dragging_macro {
                // Make this a drop target
                mouse_area(
                    container(param_row)
                        .style(move |_| container::Style {
                            border: iced::Border {
                                color: border_color,
                                width: 1.0,
                                radius: 2.0.into(),
                            },
                            ..Default::default()
                        })
                )
                .on_press(MultibandEditorMessage::DropMacroOnParam {
                    macro_index: macro_idx,
                    band: band_idx,
                    effect: effect_idx,
                    param: param_idx,
                })
                .into()
            } else if is_mapped {
                // Show remove mapping option on click
                mouse_area(
                    container(param_row)
                        .style(move |_| container::Style {
                            border: iced::Border {
                                color: border_color,
                                width: 1.0,
                                radius: 2.0.into(),
                            },
                            ..Default::default()
                        })
                )
                .on_press(MultibandEditorMessage::RemoveParamMapping {
                    band: band_idx,
                    effect: effect_idx,
                    param: param_idx,
                })
                .into()
            } else {
                param_row.into()
            };

            content
        })
        .collect();

    let params_column = column(param_controls).spacing(2);

    container(
        column![header, params_column].spacing(4)
    )
    .padding(6)
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(BG_LIGHT.into()),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 3.0.into(),
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

fn macro_bar(macros: &[MacroUiState], dragging_macro: Option<usize>) -> Element<'_, MultibandEditorMessage> {
    let macro_widgets: Vec<Element<'_, MultibandEditorMessage>> = macros
        .iter()
        .map(|m| {
            let index = m.index;
            let value = m.value;
            let is_dragging = dragging_macro == Some(index);

            let name_color = if is_dragging {
                Color::from_rgb(1.0, 0.8, 0.3) // Yellow when dragging
            } else if m.mapping_count > 0 {
                ACCENT_COLOR
            } else {
                TEXT_SECONDARY
            };

            let border_color = if is_dragging {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else {
                Color::TRANSPARENT
            };

            // Macro content
            let macro_content = column![
                // Value display
                text(format!("{:.0}%", value * 100.0))
                    .size(10)
                    .color(TEXT_SECONDARY),
                // Interactive slider
                slider(0.0..=1.0, value, move |v| {
                    MultibandEditorMessage::SetMacro { index, value: v }
                })
                .width(60)
                .height(16),
                // Macro name (shows mapping count)
                text(if m.mapping_count > 0 {
                    format!("{} ({})", m.name, m.mapping_count)
                } else {
                    m.name.clone()
                })
                .size(9)
                .color(name_color),
                // Drag hint
                text("drag to map")
                    .size(7)
                    .color(Color::from_rgb(0.4, 0.4, 0.45)),
            ]
            .spacing(2)
            .align_x(Alignment::Center);

            // Wrap in mouse_area for drag support
            let draggable: Element<'_, MultibandEditorMessage> = mouse_area(
                container(macro_content)
                    .padding(4)
                    .style(move |_| container::Style {
                        border: iced::Border {
                            color: border_color,
                            width: if is_dragging { 2.0 } else { 0.0 },
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    })
            )
            .on_press(MultibandEditorMessage::StartDragMacro(index))
            .on_release(MultibandEditorMessage::EndDragMacro)
            .into();

            container(draggable)
                .width(Length::Fixed(80.0))
                .into()
        })
        .collect();

    container(
        column![
            row![
                text("Macros").size(11).color(TEXT_SECONDARY),
                Space::new().width(Length::Fill),
                if dragging_macro.is_some() {
                    text("Drop on parameter to map").size(9).color(ACCENT_COLOR)
                } else {
                    text("").size(9)
                },
            ]
            .width(Length::Fill),
            row(macro_widgets).spacing(8),
        ]
        .spacing(6),
    )
    .padding(8)
    .style(|_| container::Style {
        background: Some(BG_MEDIUM.into()),
        ..Default::default()
    })
    .into()
}
