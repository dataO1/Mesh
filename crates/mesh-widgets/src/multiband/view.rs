//! View function for the multiband editor widget

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};

use super::crossover_bar::crossover_bar;
use super::message::MultibandEditorMessage;
use super::state::{BandUiState, EffectChainLocation, EffectUiState, MacroMappingRef, MultibandEditorState};

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
                state,
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
            state,
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
            state,
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
        macro_bar(state),
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

/// Render the multiband editor content without modal wrapper or header
///
/// Use this when embedding the editor in a custom modal (e.g., mesh-cue's effects editor).
/// Returns the crossover bar, processing area (pre-fx, bands, post-fx), and macro bar.
///
/// Note: Does NOT include the preset browser/save overlays - handle those in your wrapper.
pub fn multiband_editor_content(
    state: &MultibandEditorState,
) -> Element<'_, MultibandEditorMessage> {
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
                state,
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
            state,
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
            state,
        ),
    ]
    .spacing(8)
    .width(Length::Fill)
    .height(Length::Fill);

    // Content without header (header is provided by the wrapper)
    let content = column![
        // Crossover visualization bar
        crossover_bar(state),
        divider(),
        // Main processing area with pre-fx, bands, post-fx
        processing_area,
        divider(),
        // Macro knobs row
        macro_bar(state),
    ]
    .spacing(8)
    .width(Length::Fill)
    .height(Length::Fill);

    content.into()
}

/// Ensure all effect parameter knobs exist in the state
///
/// Call this in update handlers before view is rendered, specifically:
/// - When the editor is opened
/// - After adding an effect to any chain (pre-fx, band, post-fx)
pub fn ensure_effect_knobs_exist(state: &mut MultibandEditorState) {
    use super::state::MAX_UI_KNOBS;

    // Pre-FX effects
    for (effect_idx, effect) in state.pre_fx.iter().enumerate() {
        for knob_idx in 0..MAX_UI_KNOBS {
            let key = (EffectChainLocation::PreFx, effect_idx, knob_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(40.0);
                knob.set_value(effect.knob_assignments[knob_idx].value);
                state.effect_knobs.insert(key, knob);
            }
        }
    }

    // Band effects
    for (band_idx, band) in state.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            for knob_idx in 0..MAX_UI_KNOBS {
                let key = (EffectChainLocation::Band(band_idx), effect_idx, knob_idx);
                if !state.effect_knobs.contains_key(&key) {
                    let mut knob = Knob::new(40.0);
                    knob.set_value(effect.knob_assignments[knob_idx].value);
                    state.effect_knobs.insert(key, knob);
                }
            }
        }
    }

    // Post-FX effects
    for (effect_idx, effect) in state.post_fx.iter().enumerate() {
        for knob_idx in 0..MAX_UI_KNOBS {
            let key = (EffectChainLocation::PostFx, effect_idx, knob_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(40.0);
                knob.set_value(effect.knob_assignments[knob_idx].value);
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
        button(text("Load").size(14))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::OpenPresetBrowser),
        button(text("Save").size(14))
            .padding([4, 8])
            .on_press(MultibandEditorMessage::OpenSaveDialog),
        Space::new().width(Length::Fill),
        text(format!("Deck {} - {}", state.deck + 1, state.stem_name))
            .size(14)
            .color(TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(text("×").size(14))
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
    editor_state: &'a MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    // Band header: name and freq range
    let header = column![
        text(format!("Band {}", band_idx + 1))
            .size(14)
            .color(TEXT_PRIMARY),
        text(band.name()).size(14).color(TEXT_SECONDARY),
        text(band.freq_range_str()).size(11).color(TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    // Control buttons row: Solo, Mute, Remove
    let controls = row![
        button(
            text("S")
                .size(14)
                .color(if band.soloed { SOLO_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandSolo {
            band: band_idx,
            soloed: !band.soloed,
        }),
        button(
            text("M")
                .size(14)
                .color(if band.muted { MUTE_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 6])
        .on_press(MultibandEditorMessage::SetBandMute {
            band: band_idx,
            muted: !band.muted,
        }),
        button(text("×").size(14))
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
                editor_state,
            )
        })
        .collect();

    let effects_column = column(effect_cards).spacing(4).push(
        button(text("+ Add Effect").size(14))
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
    editor_state: &'a MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    let header = column![
        text(title).size(14).color(TEXT_PRIMARY),
        text(if location == EffectChainLocation::PreFx {
            "Before split"
        } else {
            "After merge"
        })
        .size(11)
        .color(TEXT_SECONDARY),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    let effect_cards: Vec<Element<'_, MultibandEditorMessage>> = effects
        .iter()
        .enumerate()
        .map(|(effect_idx, effect)| {
            fx_effect_card(effect_idx, effect, location, dragging_macro, effect_knobs, learning_knob, editor_state)
        })
        .collect();

    let add_button = button(text("+ Add Effect").size(14))
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
    .width(Length::Fixed(220.0))
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
    editor_state: &'a MultibandEditorState,
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
            text(&effect.name).size(14).color(name_color),
            Space::new().width(Length::Fill),
            // GUI button for CLAP plugins (toggles open/close)
            button(text(gui_button_text).size(11).color(gui_button_color))
                .padding([1, 3])
                .on_press(gui_message),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(11)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(if location == EffectChainLocation::PreFx {
                MultibandEditorMessage::TogglePreFxBypass(effect_idx)
            } else {
                MultibandEditorMessage::TogglePostFxBypass(effect_idx)
            }),
            button(text("×").size(11))
                .padding([1, 3])
                .on_press(if location == EffectChainLocation::PreFx {
                    MultibandEditorMessage::RemovePreFxEffect(effect_idx)
                } else {
                    MultibandEditorMessage::RemovePostFxEffect(effect_idx)
                }),
        ]
    } else {
        row![
            text(&effect.name).size(14).color(name_color),
            Space::new().width(Length::Fill),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(11)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(if location == EffectChainLocation::PreFx {
                MultibandEditorMessage::TogglePreFxBypass(effect_idx)
            } else {
                MultibandEditorMessage::TogglePostFxBypass(effect_idx)
            }),
            button(text("×").size(11))
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

    // Parameter knobs (show 8 knobs in 2 rows of 4)
    let param_knobs: Vec<Element<'_, MultibandEditorMessage>> = (0..8)
        .map(|knob_idx| {
            let assignment = &effect.knob_assignments[knob_idx];

            // Check if this knob is in learning mode
            let is_learning = learning_knob == Some((location, effect_idx, knob_idx));

            // Check if this knob is highlighted (from hovering a modulation indicator)
            let is_highlighted = is_param_highlighted(editor_state, location, effect_idx, knob_idx);

            // Get param name from available_params via the assignment's param_index
            let param_name = assignment.param_index
                .and_then(|idx| effect.available_params.get(idx))
                .map(|p| p.name.as_str())
                .unwrap_or("[assign]");

            let mapped_macro = assignment.macro_mapping.as_ref().and_then(|m| m.macro_index);
            let is_mapped = mapped_macro.is_some();

            // Learning mode takes priority for color, then highlight
            let label_color = if is_learning {
                LEARNING_COLOR
            } else if is_highlighted {
                PARAM_HIGHLIGHT_COLOR
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

            // Get the current value for display
            let value_display = format!("{:.0}%", assignment.value * 100.0);

            // Build clickable label - for CLAP effects, right-click (or long-press) starts learning
            // Regular click opens param picker
            let label_button: Element<'_, MultibandEditorMessage> = if is_learning {
                // When learning, clicking cancels learning mode
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::CancelLearning)
                    .into()
            } else if is_clap {
                // For CLAP effects, clicking starts learning mode
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::StartLearning {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                    })
                    .into()
            } else {
                // For non-CLAP effects, clicking opens param picker
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::OpenParamPicker {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                    })
                    .into()
            };

            // Get knob from state
            let key = (location, effect_idx, knob_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: knob_idx,
                        event,
                    })
                } else {
                    Space::new().width(40.0).height(40.0).into()
                };

            // Value text element (pass owned String to avoid borrow issues)
            let value_text = text(value_display)
                .size(10)
                .color(TEXT_SECONDARY);

            // Build the knob column content
            let knob_content = column![knob_element, label_button, value_text]
                .spacing(1)
                .align_x(Alignment::Center);

            // Wrap in container with highlight border if needed
            let knob_container: Element<'_, MultibandEditorMessage> = if is_highlighted {
                container(knob_content)
                    .style(move |_| container::Style {
                        border: iced::Border {
                            color: PARAM_HIGHLIGHT_COLOR,
                            width: 2.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                knob_content.into()
            };

            // Wrap in mouse_area for macro drop target when dragging
            let knob_with_label: Element<'_, MultibandEditorMessage> =
                if let Some(macro_idx) = dragging_macro {
                    mouse_area(knob_container)
                        .on_release(MultibandEditorMessage::DropMacroOnParam {
                            macro_index: macro_idx,
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .into()
                } else if is_mapped {
                    mouse_area(knob_container)
                        .on_press(MultibandEditorMessage::RemoveParamMapping {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .into()
                } else {
                    knob_container
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
    editor_state: &'a MultibandEditorState,
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
            text(&effect.name).size(11).color(name_color),
            Space::new().width(Length::Fill),
            // GUI button for CLAP plugins (toggles open/close)
            button(text(gui_button_text).size(11).color(gui_button_color))
                .padding([1, 3])
                .on_press(gui_message),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(11)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(MultibandEditorMessage::ToggleEffectBypass {
                band: band_idx,
                effect: effect_idx,
            }),
            button(text("×").size(11))
                .padding([1, 3])
                .on_press(MultibandEditorMessage::RemoveEffect {
                    band: band_idx,
                    effect: effect_idx,
                }),
        ]
    } else {
        row![
            text(&effect.name).size(11).color(name_color),
            Space::new().width(Length::Fill),
            button(
                text(if effect.bypassed { "○" } else { "●" })
                    .size(11)
                    .color(name_color)
            )
            .padding([1, 3])
            .on_press(MultibandEditorMessage::ToggleEffectBypass {
                band: band_idx,
                effect: effect_idx,
            }),
            button(text("×").size(11))
                .padding([1, 3])
                .on_press(MultibandEditorMessage::RemoveEffect {
                    band: band_idx,
                    effect: effect_idx,
                }),
        ]
    }
    .spacing(2)
    .align_y(Alignment::Center);

    // Parameter knobs (show 8 knobs in 2 rows of 4)
    let param_knobs: Vec<Element<'_, MultibandEditorMessage>> = (0..8)
        .map(|knob_idx| {
            let assignment = &effect.knob_assignments[knob_idx];

            // Check if this knob is in learning mode
            let is_learning = learning_knob == Some((location, effect_idx, knob_idx));

            // Check if this knob is highlighted (from hovering a modulation indicator)
            let is_highlighted = is_param_highlighted(editor_state, location, effect_idx, knob_idx);

            // Get param name from available_params via the assignment's param_index
            let param_name = assignment.param_index
                .and_then(|idx| effect.available_params.get(idx))
                .map(|p| p.name.as_str())
                .unwrap_or("[assign]");

            let mapped_macro = assignment.macro_mapping.as_ref().and_then(|m| m.macro_index);
            let is_mapped = mapped_macro.is_some();

            // Learning mode takes priority for color, then highlight
            let label_color = if is_learning {
                LEARNING_COLOR
            } else if is_highlighted {
                PARAM_HIGHLIGHT_COLOR
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

            // Get the current value for display
            let value_display = format!("{:.0}%", assignment.value * 100.0);

            // Get knob from state
            let key = (location, effect_idx, knob_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: knob_idx,
                        event,
                    })
                } else {
                    Space::new().width(40.0).height(40.0).into()
                };

            // Build clickable label - for CLAP effects, clicking starts learning mode
            let label_button: Element<'_, MultibandEditorMessage> = if is_learning {
                // When learning, clicking cancels learning mode
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::CancelLearning)
                    .into()
            } else if is_clap {
                // For CLAP effects, clicking starts learning mode
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::StartLearning {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                    })
                    .into()
            } else {
                // For non-CLAP effects, clicking opens param picker
                mouse_area(text(label_text).size(11).color(label_color))
                    .on_press(MultibandEditorMessage::OpenParamPicker {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                    })
                    .into()
            };

            // Value text element (pass owned String to avoid borrow issues)
            let value_text = text(value_display)
                .size(10)
                .color(TEXT_SECONDARY);

            // Build the knob column content
            let knob_content = column![knob_element, label_button, value_text]
                .spacing(1)
                .align_x(Alignment::Center);

            // Wrap in container with highlight border if needed
            let knob_container: Element<'_, MultibandEditorMessage> = if is_highlighted {
                container(knob_content)
                    .style(move |_| container::Style {
                        border: iced::Border {
                            color: PARAM_HIGHLIGHT_COLOR,
                            width: 2.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                knob_content.into()
            };

            // Wrap in mouse_area for macro drop target when dragging
            let knob_with_label: Element<'_, MultibandEditorMessage> =
                if let Some(macro_idx) = dragging_macro {
                    mouse_area(knob_container)
                        .on_release(MultibandEditorMessage::DropMacroOnParam {
                            macro_index: macro_idx,
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .into()
                } else if is_mapped {
                    mouse_area(knob_container)
                        .on_press(MultibandEditorMessage::RemoveParamMapping {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .into()
                } else {
                    knob_container
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
            .size(11)
            .color(TEXT_SECONDARY)
            .into()
    } else {
        button(
            row![text("+").size(14), text("Add Band").size(14),]
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
    state: &'a MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    let macros = &state.macros;
    let macro_knobs = &state.macro_knobs;
    let dragging_macro = state.dragging_macro;

    let macro_widgets: Vec<Element<'_, MultibandEditorMessage>> = macros
        .iter()
        .zip(macro_knobs.iter())
        .enumerate()
        .map(|(index, (m, knob))| {
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

            // Build mini modulation indicator row
            let mod_indicators = mod_indicators_row(index, &state.macro_mappings_index[index], state);

            let macro_content = column![
                mod_indicators,
                text(format!("{:.0}%", knob.value() * 100.0))
                    .size(14)
                    .color(TEXT_SECONDARY),
                knob_widget,
                text(if m.mapping_count > 0 {
                    format!("{} ({})", m.name, m.mapping_count)
                } else {
                    m.name.clone()
                })
                .size(11)
                .color(name_color),
                text("drag to map")
                    .size(11)
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
                text("Macros").size(11).color(TEXT_SECONDARY),
                Space::new().width(Length::Fill),
                if dragging_macro.is_some() {
                    text("Drop on parameter to map")
                        .size(11)
                        .color(ACCENT_COLOR)
                } else {
                    text("").size(11)
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
// Modulation Range Indicators
// ─────────────────────────────────────────────────────────────────────────────

/// Color for modulation indicators
const MOD_INDICATOR_COLOR: Color = Color::from_rgb(0.9, 0.5, 0.2);
/// Color for inverted (negative) modulation indicators
const MOD_INDICATOR_INVERTED_COLOR: Color = Color::from_rgb(0.7, 0.4, 0.2);
/// Color for modulation indicator highlight
const MOD_INDICATOR_HIGHLIGHT_COLOR: Color = Color::from_rgb(1.0, 0.7, 0.3);
/// Color for highlighted parameter knobs (when hovering mod indicator)
const PARAM_HIGHLIGHT_COLOR: Color = Color::from_rgb(1.0, 0.6, 0.2);

/// Check if a specific effect parameter knob should be highlighted
///
/// Returns true if the user is hovering a modulation indicator that targets this knob.
fn is_param_highlighted(
    state: &MultibandEditorState,
    location: EffectChainLocation,
    effect_idx: usize,
    knob_idx: usize,
) -> bool {
    if let Some((macro_idx, map_idx)) = state.hovered_mapping {
        if let Some(mapping) = state.macro_mappings_index[macro_idx].get(map_idx) {
            return mapping.location == location
                && mapping.effect_idx == effect_idx
                && mapping.knob_idx == knob_idx;
        }
    }
    false
}

/// Render a row of mini modulation indicators above a macro knob
fn mod_indicators_row<'a>(
    macro_idx: usize,
    mappings: &[MacroMappingRef],
    state: &MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    if mappings.is_empty() {
        return Space::new()
            .width(Length::Fixed(0.0))
            .height(Length::Fixed(24.0))
            .into();
    }

    let indicators: Vec<Element<'_, MultibandEditorMessage>> = mappings
        .iter()
        .enumerate()
        .map(|(i, m)| mod_range_indicator(macro_idx, i, m, state))
        .collect();

    container(
        row(indicators)
            .spacing(2)
            .align_y(Alignment::Center),
    )
    .width(Length::Shrink)
    .height(Length::Fixed(24.0))
    .center_x(Length::Shrink)
    .into()
}

/// Render a single mini modulation range indicator (8px × 24px bipolar bar)
///
/// Visual design:
/// - Center line at 12px from top
/// - Positive offset: fills UP from center (orange)
/// - Negative offset: fills DOWN from center (darker orange)
fn mod_range_indicator<'a>(
    macro_idx: usize,
    mapping_idx: usize,
    mapping: &MacroMappingRef,
    state: &MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    let is_hovered = state.hovered_mapping == Some((macro_idx, mapping_idx));
    let is_dragging = state.dragging_mod_range
        .map(|d| d.macro_index == macro_idx && d.mapping_idx == mapping_idx)
        .unwrap_or(false);
    let offset_range = mapping.offset_range;

    // Determine fill color based on state
    let fill_color = if is_hovered || is_dragging {
        MOD_INDICATOR_HIGHLIGHT_COLOR
    } else if offset_range < 0.0 {
        MOD_INDICATOR_INVERTED_COLOR
    } else {
        MOD_INDICATOR_COLOR
    };

    // Calculate visual representation
    // offset_range is -1 to +1, we normalize to fill position
    let bar_height = 24.0_f32;
    let center_y = bar_height / 2.0;
    let max_fill = center_y - 2.0; // Leave 2px margin at top/bottom

    // Absolute fill amount (0 to max_fill)
    let fill_amount = (offset_range.abs() * max_fill).min(max_fill);

    // Calculate padding to position the fill correctly
    // For positive offset: fill goes UP from center (top padding = center - fill, bottom = center)
    // For negative offset: fill goes DOWN from center (top = center, bottom = center - fill)
    let top_pad = if offset_range >= 0.0 { center_y - fill_amount } else { center_y };
    let _bottom_pad = if offset_range < 0.0 { center_y - fill_amount } else { center_y };

    // Create the indicator visual using a container with a styled inner container
    let bar_visual: Element<'_, MultibandEditorMessage> = container(
        // Inner container that represents the fill
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fixed(fill_amount))
            .style(move |_| container::Style {
                background: Some(fill_color.into()),
                ..Default::default()
            }),
    )
    .width(Length::Fixed(8.0))
    .height(Length::Fixed(bar_height))
    .padding([top_pad as u16, 0])  // [vertical, horizontal]
    .style(move |_| container::Style {
        background: Some(BG_LIGHT.into()),
        border: iced::Border {
            color: if is_hovered || is_dragging { MOD_INDICATOR_HIGHLIGHT_COLOR } else { BORDER_COLOR },
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    })
    .into();

    // Wrap in mouse_area for drag and hover interactions
    mouse_area(bar_visual)
        .on_press(MultibandEditorMessage::StartDragModRange { macro_index: macro_idx, mapping_idx })
        .on_enter(MultibandEditorMessage::HoverModRange { macro_index: macro_idx, mapping_idx })
        .on_exit(MultibandEditorMessage::UnhoverModRange)
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset browser overlay
// ─────────────────────────────────────────────────────────────────────────────

/// Render the preset browser overlay for loading presets
///
/// Shows a scrollable list of available presets with load/delete buttons.
/// Use this when building a custom modal wrapper around `multiband_editor_content`.
pub fn preset_browser_overlay(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let preset_list: Vec<Element<'_, MultibandEditorMessage>> = state
        .available_presets
        .iter()
        .map(|name| {
            let name_clone = name.clone();
            let name_for_delete = name.clone();
            row![
                button(text(name).size(14))
                    .padding([8, 16])
                    .width(Length::Fill)
                    .on_press(MultibandEditorMessage::LoadPreset(name_clone)),
                button(text("×").size(14).color(MUTE_COLOR))
                    .padding([8, 8])
                    .on_press(MultibandEditorMessage::DeletePreset(name_for_delete)),
            ]
            .spacing(4)
            .into()
        })
        .collect();

    let content: Element<'_, MultibandEditorMessage> = if preset_list.is_empty() {
        column![
            text("No presets found").size(14).color(TEXT_SECONDARY),
            text("Save a preset first").size(11).color(TEXT_SECONDARY),
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
                text("Load Preset").size(14).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(14))
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

/// Render the save dialog overlay for saving presets
///
/// Shows a text input for preset name and save/cancel buttons.
/// Use this when building a custom modal wrapper around `multiband_editor_content`.
pub fn save_dialog_overlay(state: &MultibandEditorState) -> Element<'_, MultibandEditorMessage> {
    let can_save = !state.preset_name_input.trim().is_empty();

    let dialog = container(
        column![
            row![
                text("Save Preset").size(14).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(14))
                    .padding([4, 8])
                    .on_press(MultibandEditorMessage::CloseSaveDialog),
            ]
            .align_y(Alignment::Center),
            divider(),
            text("Preset Name:").size(14).color(TEXT_SECONDARY),
            text_input("Enter preset name...", &state.preset_name_input)
                .on_input(MultibandEditorMessage::SetPresetNameInput)
                .padding(8)
                .size(14),
            row![
                button(text("Cancel").size(14))
                    .padding([8, 16])
                    .on_press(MultibandEditorMessage::CloseSaveDialog),
                Space::new().width(Length::Fill),
                if can_save {
                    button(text("Save").size(14).color(ACCENT_COLOR))
                        .padding([8, 16])
                        .on_press(MultibandEditorMessage::SavePreset)
                } else {
                    button(text("Save").size(14).color(TEXT_SECONDARY)).padding([8, 16])
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
                            .size(14)
                            .color(text_color),
                        text(range_text).size(14).color(TEXT_SECONDARY),
                        Space::new().width(Length::Fill),
                        text(assigned_note).size(11).color(TEXT_SECONDARY),
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
            text("No parameters match").size(14).color(TEXT_SECONDARY),
            text("Try a different search term")
                .size(14)
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
                    .size(14)
                    .color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(14))
                    .padding([4, 8])
                    .on_press(MultibandEditorMessage::CloseParamPicker),
            ]
            .align_y(Alignment::Center),
            // Effect name
            text(format!("Effect: {}", effect.name))
                .size(11)
                .color(TEXT_SECONDARY),
            divider(),
            // Search input
            text_input("Search parameters...", &state.param_picker_search)
                .on_input(MultibandEditorMessage::SetParamPickerFilter)
                .padding(8)
                .size(14),
            // Parameter list
            param_list,
            divider(),
            // Action buttons
            row![
                button(text("Clear Assignment").size(11))
                    .padding([6, 12])
                    .on_press(MultibandEditorMessage::AssignParam {
                        location,
                        effect: effect_idx,
                        knob: knob_idx,
                        param_index: None,
                    }),
                Space::new().width(Length::Fill),
                button(text("Cancel").size(11))
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
