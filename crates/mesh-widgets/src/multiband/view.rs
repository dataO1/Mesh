//! View function for the multiband editor widget

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};

use super::crossover_bar::crossover_bar;
use super::message::MultibandEditorMessage;
use super::state::{BandUiState, EffectChainLocation, EffectUiState, MacroUiState, MultibandEditorState};

use crate::knob::{Knob, ModulationRange};

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
/// Color for knobs in learning mode (bright magenta for visibility)
const LEARNING_COLOR: Color = Color::from_rgb(1.0, 0.3, 0.8);

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
/// Note: Ensure all effect knobs exist before calling this (via `ensure_effect_knobs_exist`
/// in your update handler).
pub fn multiband_editor(
    state: &MultibandEditorState,
) -> Option<Element<'_, MultibandEditorMessage>> {
    if !state.is_open {
        return None;
    }

    let dragging_macro = state.dragging_macro;
    let learning_knob = state.learning_knob;

    // Build band columns
    let band_columns: Vec<Element<'_, MultibandEditorMessage>> = state
        .bands
        .iter()
        .enumerate()
        .map(|(band_idx, band)| {
            band_column(
                band,
                band_idx,
                state.any_soloed,
                dragging_macro,
                &state.effect_knobs,
                learning_knob,
            )
        })
        .collect();

    // Main processing area: Pre-FX | Bands | Post-FX
    let processing_area = row![
        // Pre-FX chain (left side)
        fx_chain_column(
            "Pre-FX",
            &state.pre_fx,
            EffectChainLocation::PreFx,
            dragging_macro,
            &state.effect_knobs,
            learning_knob,
        ),
        // Band columns (center, fill available space)
        column![
            row(band_columns)
                .spacing(4)
                .width(Length::Fill)
                .height(Length::Fill),
            add_band_button(state.bands.len()),
        ]
        .spacing(4)
        .width(Length::Fill)
        .height(Length::Fill),
        // Post-FX chain (right side)
        fx_chain_column(
            "Post-FX",
            &state.post_fx,
            EffectChainLocation::PostFx,
            dragging_macro,
            &state.effect_knobs,
            learning_knob,
        ),
    ]
    .spacing(8)
    .width(Length::Fill)
    .height(Length::Fill);

    let content = column![
        // Header with preset controls and close button
        header_row(state),
        divider(),
        // Crossover visualization bar
        crossover_bar(state),
        divider(),
        // Main processing area with pre-fx, bands, post-fx
        processing_area,
        divider(),
        // Macro knobs row
        macro_bar(&state.macros, &state.macro_knobs, state.dragging_macro),
    ]
    .spacing(8)
    .padding(16);

    // Wrap in modal container (80% of the screen)
    let modal = container(content)
        .width(Length::FillPortion(4))
        .height(Length::FillPortion(4))
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

    // Layer preset browser, save dialog, or param picker on top if open
    let final_view: Element<'_, MultibandEditorMessage> = if state.preset_browser_open {
        iced::widget::stack![centered, preset_browser_overlay(state),].into()
    } else if state.save_dialog_open {
        iced::widget::stack![centered, save_dialog_overlay(state),].into()
    } else if state.param_picker_open.is_some() {
        iced::widget::stack![centered, param_picker_overlay(state),].into()
    } else {
        centered.into()
    };

    Some(final_view)
}

/// Ensure all effect parameter knobs exist in the state
///
/// Call this in update handlers before view is rendered, specifically:
/// - When the editor is opened
/// - After adding an effect to any chain (pre-fx, band, post-fx)
pub fn ensure_effect_knobs_exist(state: &mut MultibandEditorState) {
    // Pre-FX effects
    for (effect_idx, effect) in state.pre_fx.iter().enumerate() {
        for param_idx in 0..effect.param_values.len() {
            let key = (EffectChainLocation::PreFx, effect_idx, param_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(40.0);
                knob.set_value(effect.param_values[param_idx]);
                state.effect_knobs.insert(key, knob);
            }
        }
    }

    // Band effects
    for (band_idx, band) in state.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            for param_idx in 0..effect.param_values.len() {
                let key = (EffectChainLocation::Band(band_idx), effect_idx, param_idx);
                if !state.effect_knobs.contains_key(&key) {
                    let mut knob = Knob::new(40.0);
                    knob.set_value(effect.param_values[param_idx]);
                    state.effect_knobs.insert(key, knob);
                }
            }
        }
    }

    // Post-FX effects
    for (effect_idx, effect) in state.post_fx.iter().enumerate() {
        for param_idx in 0..effect.param_values.len() {
            let key = (EffectChainLocation::PostFx, effect_idx, param_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(40.0);
                knob.set_value(effect.param_values[param_idx]);
                state.effect_knobs.insert(key, knob);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Header row
// ─────────────────────────────────────────────────────────────────────────────

fn header_row(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    row![
        button(text("Load").size(20))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::OpenPresetBrowser),
        button(text("Save").size(20))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::OpenSaveDialog),
        Space::new().width(Length::Fill),
        text(format!("Deck {} - {}", state.deck + 1, state.stem_name))
            .size(20)
            .color(TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(text("×").size(20))
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

fn band_column<'a>(
    band: &'a BandUiState,
    band_idx: usize,
    any_soloed: bool,
    dragging_macro: Option<usize>,
    effect_knobs: &'a std::collections::HashMap<super::state::EffectKnobKey, Knob>,
    learning_knob: Option<(EffectChainLocation, usize, usize)>,
) -> Element<'a, MultibandEditorMessage> {
    // Band header: name and freq range
    let header = column![
        text(format!("Band {}", band_idx + 1))
            .size(20)
            .color(TEXT_PRIMARY),
        text(band.name()).size(20).color(TEXT_SECONDARY),
        text(band.freq_range_str()).size(13).color(TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    // Control buttons row: Solo, Mute, Remove
    let controls = row![
        button(
            text("S")
                .size(20)
                .color(if band.soloed { SOLO_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandSolo {
            band: band_idx,
            soloed: !band.soloed,
        }),
        button(
            text("M")
                .size(20)
                .color(if band.muted { MUTE_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandMute {
            band: band_idx,
            muted: !band.muted,
        }),
        button(text("×").size(20))
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
        .map(|(effect_idx, effect)| {
            effect_card(
                band_idx,
                effect_idx,
                effect,
                dragging_macro,
                effect_knobs,
                learning_knob,
            )
        })
        .collect();

    let effects_column = column(effect_cards).spacing(4).push(
        button(text("+ Add Effect").size(20))
            .padding([6, 12])
            .on_press(MultibandEditorMessage::OpenEffectPicker(band_idx)),
    );

    // Dim if muted or not soloed (when something else is soloed)
    let is_active = !band.muted && (!any_soloed || band.soloed);
    let bg_color = if is_active { BG_MEDIUM } else { BG_DARK };

    container(
        column![header, controls, scrollable(effects_column).height(Length::Fill)]
            .spacing(8)
            .align_x(Alignment::Center),
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
// Pre-FX / Post-FX chain column
// ─────────────────────────────────────────────────────────────────────────────

fn fx_chain_column<'a>(
    title: &'static str,
    effects: &'a [EffectUiState],
    location: EffectChainLocation,
    dragging_macro: Option<usize>,
    effect_knobs: &'a std::collections::HashMap<super::state::EffectKnobKey, Knob>,
    learning_knob: Option<(EffectChainLocation, usize, usize)>,
) -> Element<'a, MultibandEditorMessage> {
    let header = column![
        text(title).size(20).color(TEXT_PRIMARY),
        text(if location == EffectChainLocation::PreFx {
            "Before split"
        } else {
            "After merge"
        })
        .size(13)
        .color(TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    let effect_cards: Vec<Element<'_, MultibandEditorMessage>> = effects
        .iter()
        .enumerate()
        .map(|(effect_idx, effect)| {
            fx_effect_card(effect_idx, effect, location, dragging_macro, effect_knobs, learning_knob)
        })
        .collect();

    let add_button = button(text("+ Add Effect").size(20))
        .padding([6, 12])
        .on_press(if location == EffectChainLocation::PreFx {
            MultibandEditorMessage::OpenPreFxEffectPicker
        } else {
            MultibandEditorMessage::OpenPostFxEffectPicker
        });

    let effects_column = column(effect_cards).spacing(4).push(add_button);

    container(
        column![header, scrollable(effects_column).height(Length::Fill)]
            .spacing(8)
            .align_x(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fixed(180.0))
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(BG_MEDIUM.into()),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Renders an effect card for pre-fx or post-fx chains
fn fx_effect_card<'a>(
    effect_idx: usize,
    effect: &'a EffectUiState,
    location: EffectChainLocation,
    dragging_macro: Option<usize>,
    effect_knobs: &'a std::collections::HashMap<super::state::EffectKnobKey, Knob>,
    learning_knob: Option<(EffectChainLocation, usize, usize)>,
) -> Element<'a, MultibandEditorMessage> {
    use super::state::EffectSourceType;

    let name_color = if effect.bypassed {
        BYPASS_COLOR
    } else {
        TEXT_PRIMARY
    };

    // Check if this is a CLAP effect (can open plugin GUI)
    let is_clap = effect.source == EffectSourceType::Clap;

    // Build header with optional GUI button for CLAP effects
    let header = if is_clap {
        // GUI button toggles open/close
        let gui_button_text = if effect.gui_open { "✕ GUI" } else { "GUI" };
        let gui_button_color = if effect.gui_open { Color::from_rgb(0.9, 0.5, 0.5) } else { ACCENT_COLOR };
        let gui_message = if effect.gui_open {
            MultibandEditorMessage::ClosePluginGui { location, effect: effect_idx }
        } else {
            MultibandEditorMessage::OpenPluginGui { location, effect: effect_idx }
        };

        row![
            text(&effect.name).size(20).color(name_color),
            Space::new().width(Length::Fill),
            // GUI button for CLAP plugins (toggles open/close)
            button(text(gui_button_text).size(11).color(gui_button_color))
                .padding([1, 3])
                .on_press(gui_message),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(13)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(if location == EffectChainLocation::PreFx {
                MultibandEditorMessage::TogglePreFxBypass(effect_idx)
            } else {
                MultibandEditorMessage::TogglePostFxBypass(effect_idx)
            }),
            button(text("×").size(13))
                .padding([1, 3])
                .on_press(if location == EffectChainLocation::PreFx {
                    MultibandEditorMessage::RemovePreFxEffect(effect_idx)
                } else {
                    MultibandEditorMessage::RemovePostFxEffect(effect_idx)
                }),
        ]
    } else {
        row![
            text(&effect.name).size(20).color(name_color),
            Space::new().width(Length::Fill),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(13)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(if location == EffectChainLocation::PreFx {
                MultibandEditorMessage::TogglePreFxBypass(effect_idx)
            } else {
                MultibandEditorMessage::TogglePostFxBypass(effect_idx)
            }),
            button(text("×").size(13))
                .padding([1, 3])
                .on_press(if location == EffectChainLocation::PreFx {
                    MultibandEditorMessage::RemovePreFxEffect(effect_idx)
                } else {
                    MultibandEditorMessage::RemovePostFxEffect(effect_idx)
                }),
        ]
    }
    .spacing(2)
    .align_y(Alignment::Center);

    // Parameter knobs (show first 6 params in 2 rows of 3)
    let param_count = effect.param_values.len().min(6);
    let param_knobs: Vec<Element<'_, MultibandEditorMessage>> = (0..param_count)
        .map(|param_idx| {
            // Check if this knob is in learning mode
            let is_learning = learning_knob == Some((location, effect_idx, param_idx));

            let param_name = effect
                .param_names
                .get(param_idx)
                .map(|s| s.as_str())
                .unwrap_or("P");

            let mapping = effect.param_mappings.get(param_idx);
            let mapped_macro = mapping.and_then(|m| m.macro_index);
            let is_mapped = mapped_macro.is_some();

            // Learning mode takes priority for color
            let label_color = if is_learning {
                LEARNING_COLOR
            } else if dragging_macro.is_some() {
                ACCENT_COLOR
            } else if is_mapped {
                Color::from_rgb(0.4, 0.8, 0.4)
            } else {
                TEXT_SECONDARY
            };

            // Learning mode shows "LEARN" label
            let label_text = if is_learning {
                "LEARN".to_string()
            } else if let Some(macro_idx) = mapped_macro {
                format!("M{}", macro_idx + 1)
            } else {
                param_name[..param_name.len().min(3)].to_string()
            };

            // Build clickable label - for CLAP effects, right-click (or long-press) starts learning
            // Regular click opens param picker
            let label_button: Element<'_, MultibandEditorMessage> = if is_learning {
                // When learning, clicking cancels learning mode
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::CancelLearning)
                    .into()
            } else if is_clap {
                // For CLAP effects, clicking starts learning mode
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::StartLearning {
                        location,
                        effect: effect_idx,
                        knob: param_idx,
                    })
                    .into()
            } else {
                // For non-CLAP effects, clicking opens param picker
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::OpenParamPicker {
                        location,
                        effect: effect_idx,
                        knob: param_idx,
                    })
                    .into()
            };

            // Get knob from state
            let key = (location, effect_idx, param_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: param_idx,
                        event,
                    })
                } else {
                    Space::new().width(40.0).height(40.0).into()
                };

            // Wrap in mouse_area for macro drop target when dragging
            let knob_with_label: Element<'_, MultibandEditorMessage> =
                if let Some(macro_idx) = dragging_macro {
                    mouse_area(
                        column![knob_element, label_button]
                            .spacing(1)
                            .align_x(Alignment::Center),
                    )
                    .on_release(MultibandEditorMessage::DropMacroOnParam {
                        macro_index: macro_idx,
                        location,
                        effect: effect_idx,
                        param: param_idx,
                    })
                    .into()
                } else if is_mapped {
                    mouse_area(
                        column![knob_element, label_button]
                            .spacing(1)
                            .align_x(Alignment::Center),
                    )
                    .on_press(MultibandEditorMessage::RemoveParamMapping {
                        location,
                        effect: effect_idx,
                        param: param_idx,
                    })
                    .into()
                } else {
                    column![knob_element, label_button]
                        .spacing(1)
                        .align_x(Alignment::Center)
                        .into()
                };

            knob_with_label
        })
        .collect();

    // Arrange knobs in rows of 3
    let knob_rows: Element<'_, MultibandEditorMessage> = {
        let mut knobs_iter = param_knobs.into_iter();
        let first_row: Vec<_> = knobs_iter.by_ref().take(3).collect();
        let second_row: Vec<_> = knobs_iter.collect();

        if second_row.is_empty() {
            row(first_row).spacing(4).into()
        } else {
            column![row(first_row).spacing(4), row(second_row).spacing(4),]
                .spacing(4)
                .into()
        }
    };

    container(column![header, knob_rows].spacing(4).align_x(Alignment::Center))
        .padding(4)
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
// Effect card (for band effects)
// ─────────────────────────────────────────────────────────────────────────────

fn effect_card<'a>(
    band_idx: usize,
    effect_idx: usize,
    effect: &'a EffectUiState,
    dragging_macro: Option<usize>,
    effect_knobs: &'a std::collections::HashMap<super::state::EffectKnobKey, Knob>,
    learning_knob: Option<(EffectChainLocation, usize, usize)>,
) -> Element<'a, MultibandEditorMessage> {
    use super::state::EffectSourceType;

    let location = EffectChainLocation::Band(band_idx);

    let name_color = if effect.bypassed {
        BYPASS_COLOR
    } else {
        TEXT_PRIMARY
    };

    // Check if this is a CLAP effect (can open plugin GUI)
    let is_clap = effect.source == EffectSourceType::Clap;

    // Build header with optional GUI button for CLAP effects
    let header = if is_clap {
        // GUI button toggles open/close
        let gui_button_text = if effect.gui_open { "✕ GUI" } else { "GUI" };
        let gui_button_color = if effect.gui_open { Color::from_rgb(0.9, 0.5, 0.5) } else { ACCENT_COLOR };
        let gui_message = if effect.gui_open {
            MultibandEditorMessage::ClosePluginGui { location, effect: effect_idx }
        } else {
            MultibandEditorMessage::OpenPluginGui { location, effect: effect_idx }
        };

        row![
            text(&effect.name).size(13).color(name_color),
            Space::new().width(Length::Fill),
            // GUI button for CLAP plugins (toggles open/close)
            button(text(gui_button_text).size(11).color(gui_button_color))
                .padding([1, 3])
                .on_press(gui_message),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(13)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(MultibandEditorMessage::ToggleEffectBypass {
                band: band_idx,
                effect: effect_idx,
            }),
            button(text("×").size(13))
                .padding([1, 3])
                .on_press(MultibandEditorMessage::RemoveEffect {
                    band: band_idx,
                    effect: effect_idx,
                }),
        ]
    } else {
        row![
            text(&effect.name).size(13).color(name_color),
            Space::new().width(Length::Fill),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(13)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(MultibandEditorMessage::ToggleEffectBypass {
                band: band_idx,
                effect: effect_idx,
            }),
            button(text("×").size(13))
                .padding([1, 3])
                .on_press(MultibandEditorMessage::RemoveEffect {
                    band: band_idx,
                    effect: effect_idx,
                }),
        ]
    }
    .spacing(2)
    .align_y(Alignment::Center);

    // Parameter knobs (show first 8 params in 2 rows of 4)
    let param_count = effect.param_values.len().min(8);
    let param_knobs: Vec<Element<'_, MultibandEditorMessage>> = (0..param_count)
        .map(|param_idx| {
            // Check if this knob is in learning mode
            let is_learning = learning_knob == Some((location, effect_idx, param_idx));

            let param_name = effect
                .param_names
                .get(param_idx)
                .map(|s| s.as_str())
                .unwrap_or("P");

            let mapping = effect.param_mappings.get(param_idx);
            let mapped_macro = mapping.and_then(|m| m.macro_index);
            let is_mapped = mapped_macro.is_some();

            // Learning mode takes priority for color
            let label_color = if is_learning {
                LEARNING_COLOR
            } else if dragging_macro.is_some() {
                ACCENT_COLOR
            } else if is_mapped {
                Color::from_rgb(0.4, 0.8, 0.4)
            } else {
                TEXT_SECONDARY
            };

            // Learning mode shows "LEARN" label
            let label_text = if is_learning {
                "LEARN".to_string()
            } else if let Some(macro_idx) = mapped_macro {
                format!("M{}", macro_idx + 1)
            } else {
                param_name[..param_name.len().min(3)].to_string()
            };

            // Get knob from state
            let key = (location, effect_idx, param_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: param_idx,
                        event,
                    })
                } else {
                    Space::new().width(40.0).height(40.0).into()
                };

            // Build clickable label - for CLAP effects, clicking starts learning mode
            let label_button: Element<'_, MultibandEditorMessage> = if is_learning {
                // When learning, clicking cancels learning mode
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::CancelLearning)
                    .into()
            } else if is_clap {
                // For CLAP effects, clicking starts learning mode
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::StartLearning {
                        location,
                        effect: effect_idx,
                        knob: param_idx,
                    })
                    .into()
            } else {
                // For non-CLAP effects, clicking opens param picker
                mouse_area(text(label_text).size(13).color(label_color))
                    .on_press(MultibandEditorMessage::OpenParamPicker {
                        location,
                        effect: effect_idx,
                        knob: param_idx,
                    })
                    .into()
            };

            // Wrap in mouse_area for macro drop target when dragging
            let knob_with_label: Element<'_, MultibandEditorMessage> =
                if let Some(macro_idx) = dragging_macro {
                    mouse_area(
                        column![knob_element, label_button]
                            .spacing(1)
                            .align_x(Alignment::Center),
                    )
                    .on_release(MultibandEditorMessage::DropMacroOnParam {
                        macro_index: macro_idx,
                        location,
                        effect: effect_idx,
                        param: param_idx,
                    })
                    .into()
                } else if is_mapped {
                    mouse_area(
                        column![knob_element, label_button]
                            .spacing(1)
                            .align_x(Alignment::Center),
                    )
                    .on_press(MultibandEditorMessage::RemoveParamMapping {
                        location,
                        effect: effect_idx,
                        param: param_idx,
                    })
                    .into()
                } else {
                    column![knob_element, label_button]
                        .spacing(1)
                        .align_x(Alignment::Center)
                        .into()
                };

            knob_with_label
        })
        .collect();

    // Arrange knobs in rows of 4
    let knob_rows: Element<'_, MultibandEditorMessage> = {
        let mut knobs_iter = param_knobs.into_iter();
        let first_row: Vec<_> = knobs_iter.by_ref().take(4).collect();
        let second_row: Vec<_> = knobs_iter.collect();

        if second_row.is_empty() {
            row(first_row).spacing(4).into()
        } else {
            column![row(first_row).spacing(4), row(second_row).spacing(4),]
                .spacing(4)
                .into()
        }
    };

    container(column![header, knob_rows].spacing(4).align_x(Alignment::Center))
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
            .size(13)
            .color(TEXT_SECONDARY)
            .into()
    } else {
        button(
            row![text("+").size(20), text("Add Band").size(20),]
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

fn macro_bar<'a>(
    macros: &'a [MacroUiState],
    macro_knobs: &'a [Knob],
    dragging_macro: Option<usize>,
) -> Element<'a, MultibandEditorMessage> {
    let macro_widgets: Vec<Element<'_, MultibandEditorMessage>> = macros
        .iter()
        .zip(macro_knobs.iter())
        .map(|(m, knob)| {
            let index = m.index;
            let is_mapping_drag = dragging_macro == Some(index);

            let name_color = if is_mapping_drag {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else if m.mapping_count > 0 {
                ACCENT_COLOR
            } else {
                TEXT_SECONDARY
            };

            let border_color = if is_mapping_drag {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else {
                Color::TRANSPARENT
            };

            // Build modulation indicators if this macro has mappings
            let mut display_knob = knob.clone();
            if m.mapping_count > 0 {
                let value = knob.value();
                display_knob.set_modulations(vec![ModulationRange::new(
                    (value - 0.1).max(0.0),
                    (value + 0.1).min(1.0),
                    Color::from_rgb(0.9, 0.5, 0.2),
                )]);
            }

            let knob_widget = display_knob.view(move |event| MultibandEditorMessage::MacroKnob {
                index,
                event,
            });

            let macro_content = column![
                text(format!("{:.0}%", knob.value() * 100.0))
                    .size(20)
                    .color(TEXT_SECONDARY),
                knob_widget,
                text(if m.mapping_count > 0 {
                    format!("{} ({})", m.name, m.mapping_count)
                } else {
                    m.name.clone()
                })
                .size(13)
                .color(name_color),
                text("drag to map")
                    .size(13)
                    .color(Color::from_rgb(0.4, 0.4, 0.45)),
            ]
            .spacing(2)
            .align_x(Alignment::Center);

            // Wrap in mouse_area for mapping drag support
            let draggable: Element<'_, MultibandEditorMessage> = mouse_area(
                container(macro_content)
                    .padding(4)
                    .style(move |_| container::Style {
                        border: iced::Border {
                            color: border_color,
                            width: if is_mapping_drag { 2.0 } else { 0.0 },
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .on_press(MultibandEditorMessage::StartDragMacro(index))
            .on_release(MultibandEditorMessage::EndDragMacro)
            .into();

            container(draggable).width(Length::Fixed(100.0)).into()
        })
        .collect();

    container(
        column![
            row![
                text("Macros").size(13).color(TEXT_SECONDARY),
                Space::new().width(Length::Fill),
                if dragging_macro.is_some() {
                    text("Drop on parameter to map")
                        .size(13)
                        .color(ACCENT_COLOR)
                } else {
                    text("").size(13)
                },
            ]
            .width(Length::Fill),
            row(macro_widgets).spacing(8).padding([0, 8]), // Add horizontal padding for knob circles
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

// ─────────────────────────────────────────────────────────────────────────────
// Preset browser overlay
// ─────────────────────────────────────────────────────────────────────────────

fn preset_browser_overlay(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let preset_list: Vec<Element<'_, MultibandEditorMessage>> = state
        .available_presets
        .iter()
        .map(|name| {
            let name_clone = name.clone();
            let name_for_delete = name.clone();
            row![
                button(text(name).size(20))
                    .padding([8, 16])
                    .width(Length::Fill)
                    .on_press(MultibandEditorMessage::LoadPreset(name_clone)),
                button(text("×").size(20).color(MUTE_COLOR))
                    .padding([8, 8])
                    .on_press(MultibandEditorMessage::DeletePreset(name_for_delete)),
            ]
            .spacing(4)
            .into()
        })
        .collect();

    let content: Element<'_, MultibandEditorMessage> = if preset_list.is_empty() {
        column![
            text("No presets found").size(20).color(TEXT_SECONDARY),
            text("Save a preset first").size(13).color(TEXT_SECONDARY),
        ]
        .spacing(8)
        .align_x(Alignment::Center)
        .into()
    } else {
        scrollable(column(preset_list).spacing(4).width(Length::Fill))
            .height(Length::Fixed(300.0))
            .into()
    };

    let dialog = container(
        column![
            row![
                text("Load Preset").size(20).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(20))
                    .padding([4, 8])
                    .on_press(MultibandEditorMessage::ClosePresetBrowser),
            ]
            .align_y(Alignment::Center),
            divider(),
            content,
        ]
        .spacing(12)
        .padding(16),
    )
    .width(Length::Fixed(350.0))
    .style(|_| container::Style {
        background: Some(BG_DARK.into()),
        border: iced::Border {
            color: ACCENT_COLOR,
            width: 2.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    });

    container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
            ..Default::default()
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Save dialog overlay
// ─────────────────────────────────────────────────────────────────────────────

fn save_dialog_overlay(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let can_save = !state.preset_name_input.trim().is_empty();

    let dialog = container(
        column![
            row![
                text("Save Preset").size(20).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(20))
                    .padding([4, 8])
                    .on_press(MultibandEditorMessage::CloseSaveDialog),
            ]
            .align_y(Alignment::Center),
            divider(),
            text("Preset Name:").size(20).color(TEXT_SECONDARY),
            text_input("Enter preset name...", &state.preset_name_input)
                .on_input(MultibandEditorMessage::SetPresetNameInput)
                .padding(8)
                .size(20),
            row![
                button(text("Cancel").size(20))
                    .padding([8, 16])
                    .on_press(MultibandEditorMessage::CloseSaveDialog),
                Space::new().width(Length::Fill),
                if can_save {
                    button(text("Save").size(20).color(ACCENT_COLOR))
                        .padding([8, 16])
                        .on_press(MultibandEditorMessage::SavePreset)
                } else {
                    button(text("Save").size(20).color(TEXT_SECONDARY)).padding([8, 16])
                },
            ]
            .spacing(8),
        ]
        .spacing(12)
        .padding(16),
    )
    .width(Length::Fixed(350.0))
    .style(|_| container::Style {
        background: Some(BG_DARK.into()),
        border: iced::Border {
            color: ACCENT_COLOR,
            width: 2.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    });

    container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
            ..Default::default()
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Parameter picker overlay
// ─────────────────────────────────────────────────────────────────────────────

fn param_picker_overlay(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let (location, effect_idx, knob_idx) = match state.param_picker_open {
        Some(info) => info,
        None => {
            return Space::new().width(0.0).height(0.0).into();
        }
    };

    // Get the effect state
    let effect = match location {
        EffectChainLocation::PreFx => state.pre_fx.get(effect_idx),
        EffectChainLocation::Band(band_idx) => state
            .bands
            .get(band_idx)
            .and_then(|b| b.effects.get(effect_idx)),
        EffectChainLocation::PostFx => state.post_fx.get(effect_idx),
    };

    let effect = match effect {
        Some(e) => e,
        None => {
            return Space::new().width(0.0).height(0.0).into();
        }
    };

    // Get current assignment for this knob
    let current_param_idx = effect
        .knob_assignments
        .get(knob_idx)
        .and_then(|a| a.param_index);

    // Filter params by search term
    let search_lower = state.param_picker_search.to_lowercase();
    let filtered_params: Vec<(usize, &super::state::AvailableParam)> = effect
        .available_params
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            search_lower.is_empty() || p.name.to_lowercase().contains(&search_lower)
        })
        .collect();

    // Build parameter list
    let param_items: Vec<Element<'_, MultibandEditorMessage>> = filtered_params
        .iter()
        .map(|(idx, param)| {
            let is_current = current_param_idx == Some(*idx);

            // Check if this param is assigned to another knob
            let assigned_to_other = effect
                .knob_assignments
                .iter()
                .enumerate()
                .any(|(k, a)| k != knob_idx && a.param_index == Some(*idx));

            let text_color = if is_current {
                ACCENT_COLOR
            } else if assigned_to_other {
                Color::from_rgb(0.5, 0.5, 0.55) // Dimmed
            } else {
                TEXT_PRIMARY
            };

            let idx_copy = *idx;
            let range_text = if param.min == 0.0 && param.max == 1.0 {
                String::new()
            } else {
                let unit_str = if param.unit.is_empty() {
                    String::new()
                } else {
                    format!(" {}", param.unit)
                };
                format!(" ({:.1} - {:.1}{})", param.min, param.max, unit_str)
            };

            let indicator = if is_current { "★ " } else { "  " };
            let assigned_note = if assigned_to_other { " (on another knob)" } else { "" };

            mouse_area(
                container(
                    row![
                        text(format!("{}{}", indicator, param.name))
                            .size(20)
                            .color(text_color),
                        text(range_text).size(20).color(TEXT_SECONDARY),
                        Space::new().width(Length::Fill),
                        text(assigned_note).size(13).color(TEXT_SECONDARY),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([6, 12])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: if is_current {
                        Some(Color::from_rgb(0.2, 0.3, 0.4).into())
                    } else {
                        None
                    },
                    ..Default::default()
                }),
            )
            .on_press(MultibandEditorMessage::AssignParam {
                location,
                effect: effect_idx,
                knob: knob_idx,
                param_index: Some(idx_copy),
            })
            .into()
        })
        .collect();

    let param_list: Element<'_, MultibandEditorMessage> = if param_items.is_empty() {
        column![
            text("No parameters match").size(20).color(TEXT_SECONDARY),
            text("Try a different search term")
                .size(20)
                .color(TEXT_SECONDARY),
        ]
        .spacing(4)
        .align_x(Alignment::Center)
        .into()
    } else {
        scrollable(column(param_items).spacing(2).width(Length::Fill))
            .height(Length::Fixed(300.0))
            .into()
    };

    let dialog = container(
        column![
            // Header
            row![
                text(format!("Select Parameter for Knob {}", knob_idx + 1))
                    .size(20)
                    .color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(20))
                    .padding([4, 8])
                    .on_press(MultibandEditorMessage::CloseParamPicker),
            ]
            .align_y(Alignment::Center),
            // Effect name
            text(format!("Effect: {}", effect.name))
                .size(13)
                .color(TEXT_SECONDARY),
            divider(),
            // Search input
            text_input("Search parameters...", &state.param_picker_search)
                .on_input(MultibandEditorMessage::SetParamPickerFilter)
                .padding(8)
                .size(20),
            // Parameter list
            param_list,
            divider(),
            // Action buttons
            row![
                button(text("Clear Assignment").size(13))
                    .padding([6, 12])
                    .on_press(MultibandEditorMessage::AssignParam {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                        param_index: None,
                    }),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(13))
                    .padding([6, 12])
                    .on_press(MultibandEditorMessage::CloseParamPicker),
            ]
            .spacing(8),
        ]
        .spacing(10)
        .padding(16),
    )
    .width(Length::Fixed(400.0))
    .style(|_| container::Style {
        background: Some(BG_DARK.into()),
        border: iced::Border {
            color: ACCENT_COLOR,
            width: 2.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    });

    container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
            ..Default::default()
        })
        .into()
}
