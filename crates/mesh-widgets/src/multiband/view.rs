//! View function for the multiband editor widget

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};

use super::crossover_bar::crossover_bar;
use super::message::{ChainTarget, MultibandEditorMessage};
use super::state::{BandUiState, EffectChainLocation, EffectUiState, MacroMappingRef, MultibandEditorState};
use crate::knob::KnobEvent;

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
/// Color for dry/wet knobs (cyan tint)
const DRY_WET_COLOR: Color = Color::from_rgb(0.3, 0.8, 0.9);
/// Color for drag source items (being dragged)
const DRAG_SOURCE_COLOR: Color = Color::from_rgb(0.4, 0.6, 0.8);
/// Color for drop target items (valid drop location)
const DROP_TARGET_COLOR: Color = Color::from_rgb(0.4, 0.8, 0.4);

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
// Effect drag overlay - floating card following the mouse during drag
// ─────────────────────────────────────────────────────────────────────────────

/// Render a floating effect card that follows the mouse during drag operations
fn effect_drag_overlay(state: &MultibandEditorState) -> Option<Element<'_, MultibandEditorMessage>> {
    // Only show when we have both a drag name and mouse position
    let effect_name = state.dragging_effect_name.as_ref()?;
    let (mouse_x, mouse_y) = state.effect_drag_mouse_pos?;

    // Check if we're over a drop target - if so, we could snap there
    // For now, always follow the mouse
    let _drop_target = state.effect_drop_target;

    // Card dimensions - compact floating card
    const CARD_WIDTH: f32 = 180.0;
    const CARD_HEIGHT: f32 = 40.0;
    const OFFSET_X: f32 = 10.0; // Offset from cursor
    const OFFSET_Y: f32 = 10.0;

    // Create a compact effect card - fully opaque for clear visibility
    let card_content = container(
        row![
            text("🎛").size(14).color(DRAG_SOURCE_COLOR), // Effect icon
            text(effect_name).size(13).color(TEXT_PRIMARY),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    )
    .padding([10, 14])
    .style(|_| container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.18, 0.18, 0.22))), // Fully opaque
        border: iced::Border {
            color: DRAG_SOURCE_COLOR,
            width: 2.0,
            radius: 6.0.into(),
        },
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.7),
            offset: iced::Vector::new(4.0, 6.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    });

    // Position the card at mouse location using row/column spacers
    // This creates a "virtual grid" where the card is placed at (mouse_x, mouse_y)
    let card_with_width = container(card_content)
        .width(Length::Fixed(CARD_WIDTH))
        .height(Length::Fixed(CARD_HEIGHT));

    let positioned = column![
        // Vertical spacer (top padding)
        Space::new().width(Length::Shrink).height(Length::Fixed(mouse_y + OFFSET_Y)),
        // Row with horizontal spacer and card
        row![
            // Horizontal spacer (left padding)
            Space::new().width(Length::Fixed(mouse_x + OFFSET_X)).height(Length::Shrink),
            card_with_width,
        ]
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    Some(positioned.into())
}

// ─────────────────────────────────────────────────────────────────────────────
// Latency formatting helper
// ─────────────────────────────────────────────────────────────────────────────

/// Format latency in samples to a readable string (samples or ms at 48kHz)
fn format_latency(samples: u32) -> String {
    if samples == 0 {
        return String::new();
    }
    // Approximate conversion at 48kHz
    let ms = samples as f32 / 48.0;
    if ms < 1.0 {
        format!("{}smp", samples)
    } else {
        format!("{:.1}ms", ms)
    }
}

/// Color for latency labels - subtle gray/cyan
const LATENCY_COLOR: Color = Color::from_rgb(0.5, 0.6, 0.65);

// ─────────────────────────────────────────────────────────────────────────────
// Dry/Wet knob helper
// ─────────────────────────────────────────────────────────────────────────────

/// Create a small dry/wet knob with label
fn dry_wet_knob_view<'a>(
    knob: &Knob,
    label: &'static str,
    on_event: impl Fn(KnobEvent) -> MultibandEditorMessage + 'a,
    is_drag_target: bool,
    is_mapped: bool,
) -> Element<'a, MultibandEditorMessage> {
    let knob_element = knob.view(on_event);
    let value_text = format!("{:.0}%", knob.value() * 100.0);

    // Highlight color when dragging macro over or when mapped
    let label_color = if is_drag_target {
        ACCENT_COLOR // Highlight as valid drop target
    } else if is_mapped {
        Color::from_rgb(0.4, 0.8, 0.4) // Green for mapped
    } else {
        DRY_WET_COLOR
    };

    column![
        text(label).size(10).color(label_color),
        knob_element,
        text(value_text).size(9).color(TEXT_SECONDARY),
    ]
    .spacing(1)
    .align_x(Alignment::Center)
    .into()
}

/// Create a chain dry/wet section with label and knob
fn chain_dry_wet_section<'a>(
    label: &'static str,
    knob: &Knob,
    on_event: impl Fn(KnobEvent) -> MultibandEditorMessage + 'a,
    dragging_macro: Option<usize>,
    chain_target: ChainTarget,
    is_mapped: bool,
) -> Element<'a, MultibandEditorMessage> {
    let knob_element = knob.view(on_event);
    let value_text = format!("{:.0}%", knob.value() * 100.0);

    // Highlight color when dragging macro or when mapped
    let label_color = if dragging_macro.is_some() {
        ACCENT_COLOR // Highlight as valid drop target
    } else if is_mapped {
        Color::from_rgb(0.4, 0.8, 0.4) // Green for mapped
    } else {
        TEXT_SECONDARY
    };

    let content = row![
        text(label).size(11).color(label_color),
        Space::new().width(Length::Fill),
        column![
            knob_element,
            text(value_text).size(9).color(TEXT_SECONDARY),
        ]
        .spacing(1)
        .align_x(Alignment::Center),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    // Make it a drop target for macros
    let content: Element<'_, MultibandEditorMessage> = if let Some(macro_idx) = dragging_macro {
        mouse_area(content)
            .on_release(MultibandEditorMessage::DropMacroOnChainDryWet {
                macro_index: macro_idx,
                chain: chain_target,
            })
            .into()
    } else {
        content.into()
    };

    container(content)
        .padding([4, 8])
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.2, 0.3, 0.35, 0.3).into()),
            border: iced::Border {
                color: DRY_WET_COLOR.scale_alpha(0.3),
                width: 1.0,
                radius: 3.0.into(),
            },
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
        // Click on crossover bar to add bands instead of using a button
        row(band_columns)
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
        // Crossover visualization bar (flush with processing area)
        crossover_bar(state),
        // Main processing area with pre-fx, bands, post-fx (no gap)
        processing_area,
        // Macro knobs row (flush with processing area)
        macro_bar(state),
    ]
    .spacing(0)
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
    let base_view: Element<'_, MultibandEditorMessage> = if state.preset_browser_open {
        iced::widget::stack![centered, preset_browser_overlay(&state.available_presets),].into()
    } else if state.save_dialog_open {
        iced::widget::stack![centered, save_dialog_overlay(&state.preset_name_input),].into()
    } else if state.param_picker_open.is_some() {
        iced::widget::stack![centered, param_picker_overlay(state),].into()
    } else {
        centered.into()
    };

    // Layer effect drag overlay on top if dragging an effect
    let final_view = if let Some(drag_overlay) = effect_drag_overlay(state) {
        iced::widget::stack![base_view, drag_overlay].into()
    } else {
        base_view
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
        // Click on crossover bar to add bands instead of using a button
        row(band_columns)
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
    // No dividers or gaps between crossover bar, processing area, and macro bar
    let content: Element<'_, MultibandEditorMessage> = column![
        // Crossover visualization bar (flush with processing area)
        crossover_bar(state),
        // Main processing area with pre-fx, bands, post-fx (no gap)
        processing_area,
        // Macro knobs row (flush with processing area)
        macro_bar(state),
    ]
    .spacing(0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    // Layer effect drag overlay on top if dragging an effect
    if let Some(drag_overlay) = effect_drag_overlay(state) {
        iced::widget::stack![content, drag_overlay].into()
    } else {
        content
    }
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
        // Ensure dry/wet knob exists
        let dw_key = (EffectChainLocation::PreFx, effect_idx);
        if !state.effect_dry_wet_knobs.contains_key(&dw_key) {
            let mut knob = Knob::new(36.0);
            knob.set_value(effect.dry_wet);
            state.effect_dry_wet_knobs.insert(dw_key, knob);
        }

        // Ensure parameter knobs exist
        for knob_idx in 0..MAX_UI_KNOBS {
            let key = (EffectChainLocation::PreFx, effect_idx, knob_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(48.0);
                knob.set_value(effect.knob_assignments[knob_idx].value);
                state.effect_knobs.insert(key, knob);
            }
        }
    }

    // Band effects
    for (band_idx, band) in state.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            // Ensure dry/wet knob exists
            let dw_key = (EffectChainLocation::Band(band_idx), effect_idx);
            if !state.effect_dry_wet_knobs.contains_key(&dw_key) {
                let mut knob = Knob::new(36.0);
                knob.set_value(effect.dry_wet);
                state.effect_dry_wet_knobs.insert(dw_key, knob);
            }

            // Ensure parameter knobs exist
            for knob_idx in 0..MAX_UI_KNOBS {
                let key = (EffectChainLocation::Band(band_idx), effect_idx, knob_idx);
                if !state.effect_knobs.contains_key(&key) {
                    let mut knob = Knob::new(48.0);
                    knob.set_value(effect.knob_assignments[knob_idx].value);
                    state.effect_knobs.insert(key, knob);
                }
            }
        }
    }

    // Post-FX effects
    for (effect_idx, effect) in state.post_fx.iter().enumerate() {
        // Ensure dry/wet knob exists
        let dw_key = (EffectChainLocation::PostFx, effect_idx);
        if !state.effect_dry_wet_knobs.contains_key(&dw_key) {
            let mut knob = Knob::new(36.0);
            knob.set_value(effect.dry_wet);
            state.effect_dry_wet_knobs.insert(dw_key, knob);
        }

        // Ensure parameter knobs exist
        for knob_idx in 0..MAX_UI_KNOBS {
            let key = (EffectChainLocation::PostFx, effect_idx, knob_idx);
            if !state.effect_knobs.contains_key(&key) {
                let mut knob = Knob::new(48.0);
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
    // Check drag-and-drop state for visual feedback
    let is_being_dragged = editor_state.dragging_band == Some(band_idx);
    let is_drop_target = editor_state.band_drop_target == Some(band_idx);
    let another_band_is_dragging = editor_state.dragging_band.is_some() && !is_being_dragged;

    // Band header: name prominently with controls on right
    // Make the name/freq section draggable for band reordering
    let header_left = column![
        text(band.name()).size(14).color(if is_being_dragged {
            DRAG_SOURCE_COLOR
        } else {
            TEXT_PRIMARY
        }),
        text(band.freq_range_str()).size(10).color(TEXT_SECONDARY),
        if is_being_dragged {
            text("dragging...").size(8).color(DRAG_SOURCE_COLOR)
        } else if another_band_is_dragging {
            text("drop here to swap").size(8).color(DROP_TARGET_COLOR)
        } else {
            text("drag to swap").size(8).color(Color::from_rgba(0.5, 0.5, 0.5, 0.6))
        },
    ]
    .spacing(1);

    // Wrap the left side in mouse_area for drag initiation
    let header_left_draggable: Element<'_, MultibandEditorMessage> = mouse_area(header_left)
        .on_press(MultibandEditorMessage::StartDragBand(band_idx))
        .into();

    let header = row![
        header_left_draggable,
        Space::new().width(Length::Fill),
        // Control buttons on right: Solo, Mute, Remove
        button(
            text("S")
                .size(12)
                .color(if band.soloed { SOLO_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 4])
        .on_press(MultibandEditorMessage::SetBandSolo {
            band: band_idx,
            soloed: !band.soloed,
        }),
        button(
            text("M")
                .size(12)
                .color(if band.muted { MUTE_COLOR } else { TEXT_SECONDARY })
        )
        .padding([2, 4])
        .on_press(MultibandEditorMessage::SetBandMute {
            band: band_idx,
            muted: !band.muted,
        }),
        button(text("×").size(12))
            .padding([2, 4])
            .on_press(MultibandEditorMessage::RemoveBand(band_idx)),
    ]
    .spacing(2)
    .align_y(Alignment::Center);

    // Build effect cards with drop indicators for effect drag-and-drop
    let location = EffectChainLocation::Band(band_idx);
    let effects_with_drops = build_effect_list_with_drops(
        &band.effects,
        location,
        band_idx,
        dragging_macro,
        effect_knobs,
        learning_knob,
        editor_state,
    );

    let effects_column = column(effects_with_drops).spacing(2).push(
        button(text("+ Add Effect").size(14))
            .padding([6, 12])
            .on_press(MultibandEditorMessage::OpenEffectPicker(band_idx)),
    );

    // Dim if muted or not soloed (when something else is soloed)
    let is_active = !band.muted && (!any_soloed || band.soloed);

    // Visual feedback for drag-and-drop state
    let (bg_color, border_color, border_width) = if is_drop_target {
        (Color::from_rgba(0.3, 0.5, 0.3, 0.8), DROP_TARGET_COLOR, 3.0)
    } else if is_being_dragged {
        (Color::from_rgba(0.3, 0.4, 0.5, 0.6), DRAG_SOURCE_COLOR, 2.0)
    } else if is_active {
        (BG_MEDIUM, BORDER_COLOR, 1.0)
    } else {
        (BG_DARK, BORDER_COLOR, 1.0)
    };

    // Chain dry/wet section at the bottom
    let band_knob = editor_state.band_chain_dry_wet_knobs.get(band_idx);
    let chain_dw_section = if let Some(knob) = band_knob {
        // Clone and add modulation ranges if mapped
        let mut display_knob = knob.clone();
        if let Some(ref mapping) = band.chain_dry_wet_macro_mapping {
            if let Some(macro_idx) = mapping.macro_index {
                let macro_value = editor_state.macro_value(macro_idx);
                let (mod_min, mod_max) = mapping.modulation_bounds(band.chain_dry_wet);
                display_knob.set_modulations(vec![ModulationRange::new(
                    mod_min,
                    mod_max,
                    Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                )]);
                let modulated_value = mapping.modulate(band.chain_dry_wet, macro_value);
                display_knob.set_display_value(Some(modulated_value));
            }
        }
        chain_dry_wet_section(
            "Chain D/W",
            &display_knob,
            move |event| MultibandEditorMessage::BandChainDryWetKnob { band: band_idx, event },
            dragging_macro,
            ChainTarget::Band(band_idx),
            band.chain_dry_wet_macro_mapping.is_some(),
        )
    } else {
        // Fallback if knob doesn't exist yet
        text("D/W").size(11).color(TEXT_SECONDARY).into()
    };

    let band_content = container(
        column![
            header,
            scrollable(
                container(effects_column)
                    .padding(iced::Padding { top: 0.0, right: 12.0, bottom: 0.0, left: 0.0 })
            ).height(Length::Fill),
            chain_dw_section,
        ]
        .spacing(8)
        .align_x(Alignment::Center)
        .width(Length::Fill),
    )
    .padding(8)
    .width(Length::FillPortion(1))
    .height(Length::Fill)
    .center_x(Length::FillPortion(1))
    .style(move |_| container::Style {
        background: Some(bg_color.into()),
        border: iced::Border {
            color: border_color,
            width: border_width,
            radius: 4.0.into(),
        },
        ..Default::default()
    });

    // If another band is being dragged, make this a drop target
    if another_band_is_dragging {
        mouse_area(band_content)
            .on_enter(MultibandEditorMessage::SetBandDropTarget(Some(band_idx)))
            .on_exit(MultibandEditorMessage::SetBandDropTarget(None))
            .on_release(MultibandEditorMessage::DropBandAt(band_idx))
            .into()
    } else if is_being_dragged {
        // When dragging, on_release ends the drag
        mouse_area(band_content)
            .on_release(MultibandEditorMessage::EndDragBand)
            .into()
    } else {
        band_content.into()
    }
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

    // Build effect cards with drop indicators for effect drag-and-drop
    // Use 0 for band_idx_opt since this is pre/post-fx (not band effects)
    let effects_with_drops = build_effect_list_with_drops(
        effects,
        location,
        0, // not used for pre/post-fx
        dragging_macro,
        effect_knobs,
        learning_knob,
        editor_state,
    );

    let add_button = button(text("+ Add Effect").size(14))
        .padding([6, 12])
        .on_press(if location == EffectChainLocation::PreFx {
            MultibandEditorMessage::OpenPreFxEffectPicker
        } else {
            MultibandEditorMessage::OpenPostFxEffectPicker
        });

    let effects_column = column(effects_with_drops).spacing(2).push(add_button);

    // Chain dry/wet section at the bottom
    let (chain_knob, chain_target, chain_dw_mapped, chain_dw_value, chain_dw_mapping) = if location == EffectChainLocation::PreFx {
        (
            &editor_state.pre_fx_chain_dry_wet_knob,
            ChainTarget::PreFx,
            editor_state.pre_fx_chain_dry_wet_macro_mapping.is_some(),
            editor_state.pre_fx_chain_dry_wet,
            &editor_state.pre_fx_chain_dry_wet_macro_mapping,
        )
    } else {
        (
            &editor_state.post_fx_chain_dry_wet_knob,
            ChainTarget::PostFx,
            editor_state.post_fx_chain_dry_wet_macro_mapping.is_some(),
            editor_state.post_fx_chain_dry_wet,
            &editor_state.post_fx_chain_dry_wet_macro_mapping,
        )
    };

    // Clone and add modulation ranges if mapped
    let mut display_knob = chain_knob.clone();
    if let Some(ref mapping) = chain_dw_mapping {
        if let Some(macro_idx) = mapping.macro_index {
            let macro_value = editor_state.macro_value(macro_idx);
            let (mod_min, mod_max) = mapping.modulation_bounds(chain_dw_value);
            display_knob.set_modulations(vec![ModulationRange::new(
                mod_min,
                mod_max,
                Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
            )]);
            let modulated_value = mapping.modulate(chain_dw_value, macro_value);
            display_knob.set_display_value(Some(modulated_value));
        }
    }

    let chain_dry_wet_section = chain_dry_wet_section(
        "Chain D/W",
        &display_knob,
        move |event| {
            if location == EffectChainLocation::PreFx {
                MultibandEditorMessage::PreFxChainDryWetKnob(event)
            } else {
                MultibandEditorMessage::PostFxChainDryWetKnob(event)
            }
        },
        dragging_macro,
        chain_target,
        chain_dw_mapped,
    );

    container(
        column![
            header,
            scrollable(
                container(effects_column)
                    .padding(iced::Padding { top: 0.0, right: 12.0, bottom: 0.0, left: 0.0 })
            ).height(Length::Fill),
            chain_dry_wet_section,
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fixed(300.0))
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

    // Build header with optional settings button for CLAP effects
    let header = if is_clap {
        // Settings button toggles open/close plugin GUI
        let settings_icon = if effect.gui_open { "✕" } else { "⚙" };
        let settings_color = if effect.gui_open { Color::from_rgb(0.9, 0.5, 0.5) } else { ACCENT_COLOR };
        let gui_message = if effect.gui_open {
            MultibandEditorMessage::ClosePluginGui { location, effect: effect_idx }
        } else {
            MultibandEditorMessage::OpenPluginGui { location, effect: effect_idx }
        };

        // Build name with optional latency
        let latency_text = format_latency(effect.latency_samples);
        let name_with_latency: Element<'_, MultibandEditorMessage> = if latency_text.is_empty() {
            text(&effect.name).size(14).color(name_color).into()
        } else {
            row![
                text(&effect.name).size(14).color(name_color),
                text(format!(" ({})", latency_text)).size(10).color(LATENCY_COLOR),
            ].spacing(0).into()
        };

        row![
            name_with_latency,
            Space::new().width(Length::Fill),
            // Settings button for CLAP plugins (toggles open/close)
            button(text(settings_icon).size(11).color(settings_color))
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
        // Build name with optional latency (non-CLAP)
        let latency_text = format_latency(effect.latency_samples);
        let name_with_latency: Element<'_, MultibandEditorMessage> = if latency_text.is_empty() {
            text(&effect.name).size(14).color(name_color).into()
        } else {
            row![
                text(&effect.name).size(14).color(name_color),
                text(format!(" ({})", latency_text)).size(10).color(LATENCY_COLOR),
            ].spacing(0).into()
        };

        row![
            name_with_latency,
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

            // Learning mode shows "LEARN" label, otherwise show truncated param name
            // Mapped params are indicated by green color, not "M1" label
            let label_text = if is_learning {
                "LEARN".to_string()
            } else {
                // Truncate param name to 4 chars max
                param_name[..param_name.len().min(4)].to_string()
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

            // Get knob from state and apply modulation visualization if mapped
            let key = (location, effect_idx, knob_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    // Check if this knob has a macro mapping
                    let mut display_knob = knob.clone();
                    if let Some(ref mapping) = assignment.macro_mapping {
                        if let Some(macro_idx) = mapping.macro_index {
                            // Get current macro value
                            let macro_value = editor_state.macro_value(macro_idx);
                            // Calculate modulation bounds based on base value
                            let (mod_min, mod_max) = mapping.modulation_bounds(assignment.value);
                            // Set modulation range indicator
                            display_knob.set_modulations(vec![ModulationRange::new(
                                mod_min,
                                mod_max,
                                Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                            )]);
                            // Calculate and set the actual modulated value for the indicator dot
                            let modulated_value = mapping.modulate(assignment.value, macro_value);
                            display_knob.set_display_value(Some(modulated_value));
                        }
                    }
                    display_knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: knob_idx,
                        event,
                    })
                } else {
                    Space::new().width(48.0).height(48.0).into()
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
                    // Mapped params: click to remove, hover to highlight macro button
                    mouse_area(knob_container)
                        .on_press(MultibandEditorMessage::RemoveParamMapping {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .on_enter(MultibandEditorMessage::HoverParam {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .on_exit(MultibandEditorMessage::UnhoverParam)
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

    // Per-effect dry/wet knob on the left
    let is_dw_mapped = effect.dry_wet_macro_mapping.is_some();
    let dw_key = (location, effect_idx);
    let dry_wet_element: Element<'_, MultibandEditorMessage> = if let Some(knob) = editor_state.effect_dry_wet_knobs.get(&dw_key) {
        // Clone and add modulation ranges if mapped
        let mut display_knob = knob.clone();
        if let Some(ref mapping) = effect.dry_wet_macro_mapping {
            if let Some(macro_idx) = mapping.macro_index {
                let macro_value = editor_state.macro_value(macro_idx);
                let (mod_min, mod_max) = mapping.modulation_bounds(effect.dry_wet);
                display_knob.set_modulations(vec![ModulationRange::new(
                    mod_min,
                    mod_max,
                    Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                )]);
                let modulated_value = mapping.modulate(effect.dry_wet, macro_value);
                display_knob.set_display_value(Some(modulated_value));
            }
        }
        let dry_wet_knob = dry_wet_knob_view(
            &display_knob,
            "D/W",
            move |event| MultibandEditorMessage::EffectDryWetKnob {
                location,
                effect: effect_idx,
                event,
            },
            dragging_macro.is_some(),
            is_dw_mapped,
        );

        // Wrap dry/wet in macro drop target
        if let Some(macro_idx) = dragging_macro {
            mouse_area(dry_wet_knob)
                .on_release(MultibandEditorMessage::DropMacroOnEffectDryWet {
                    macro_index: macro_idx,
                    location,
                    effect: effect_idx,
                })
                .into()
        } else {
            dry_wet_knob
        }
    } else {
        // Fallback if knob doesn't exist yet
        text("D/W").size(10).color(TEXT_SECONDARY).into()
    };

    // Combine dry/wet knob with param knobs
    let knobs_with_dry_wet = row![
        dry_wet_element,
        container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
            .style(|_| container::Style {
                background: Some(BORDER_COLOR.into()),
                ..Default::default()
            }),
        knob_rows,
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    container(column![header, knobs_with_dry_wet].spacing(4).align_x(Alignment::Center))
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
// Effect list with drop indicators (for drag-and-drop)
// ─────────────────────────────────────────────────────────────────────────────

/// Build a list of effect cards with drop indicators between them for drag-and-drop
fn build_effect_list_with_drops<'a>(
    effects: &'a [EffectUiState],
    location: EffectChainLocation,
    band_idx_opt: usize, // Used for band effects, ignored for pre/post fx
    dragging_macro: Option<usize>,
    effect_knobs: &'a std::collections::HashMap<super::state::EffectKnobKey, Knob>,
    learning_knob: Option<(EffectChainLocation, usize, usize)>,
    editor_state: &'a MultibandEditorState,
) -> Vec<Element<'a, MultibandEditorMessage>> {
    let dragging_effect = editor_state.dragging_effect;
    let effect_drop_target = editor_state.effect_drop_target;
    let _is_dragging_in_this_location = dragging_effect.map(|(loc, _)| loc == location).unwrap_or(false);
    let is_any_effect_dragging = dragging_effect.is_some();

    let mut elements: Vec<Element<'a, MultibandEditorMessage>> = Vec::new();

    // Add drop indicator at position 0 (before first effect)
    if is_any_effect_dragging {
        elements.push(effect_drop_indicator(location, 0, effect_drop_target));
    }

    for (effect_idx, effect) in effects.iter().enumerate() {
        let is_being_dragged = dragging_effect == Some((location, effect_idx));

        // Create the effect card
        let card = match location {
            EffectChainLocation::Band(_) => effect_card(
                band_idx_opt,
                effect_idx,
                effect,
                dragging_macro,
                effect_knobs,
                learning_knob,
                editor_state,
            ),
            _ => fx_effect_card(
                effect_idx,
                effect,
                location,
                dragging_macro,
                effect_knobs,
                learning_knob,
                editor_state,
            ),
        };

        // Wrap card in mouse_area for drag support
        let card_with_drag: Element<'a, MultibandEditorMessage> = if is_being_dragged {
            // Being dragged - show with drag styling
            let styled_card = container(card)
                .style(move |_| container::Style {
                    border: iced::Border {
                        color: DRAG_SOURCE_COLOR,
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                });
            mouse_area(styled_card)
                .on_release(MultibandEditorMessage::EndDragEffect)
                .into()
        } else if is_any_effect_dragging {
            // Another effect is being dragged - make this a potential drop target (inserts after this effect)
            // Hovering/dropping here would place the dragged effect after this one
            card
        } else {
            // No drag in progress - make this card draggable
            mouse_area(card)
                .on_press(MultibandEditorMessage::StartDragEffect {
                    location,
                    effect: effect_idx,
                })
                .into()
        };

        elements.push(card_with_drag);

        // Add drop indicator after each effect (for inserting after this effect)
        if is_any_effect_dragging && !is_being_dragged {
            elements.push(effect_drop_indicator(location, effect_idx + 1, effect_drop_target));
        }
    }

    elements
}

/// Create a drop indicator element for effect drag-and-drop
fn effect_drop_indicator<'a>(
    location: EffectChainLocation,
    position: usize,
    current_target: Option<(EffectChainLocation, usize)>,
) -> Element<'a, MultibandEditorMessage> {
    let is_target = current_target == Some((location, position));

    let indicator = container(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(if is_target { 8.0 } else { 4.0 }))
        .style(move |_| container::Style {
            background: Some(if is_target {
                DROP_TARGET_COLOR.into()
            } else {
                Color::from_rgba(0.4, 0.8, 0.4, 0.2).into()
            }),
            border: iced::Border {
                radius: 2.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    mouse_area(indicator)
        .on_enter(MultibandEditorMessage::SetEffectDropTarget(Some((location, position))))
        .on_exit(MultibandEditorMessage::SetEffectDropTarget(None))
        .on_release(MultibandEditorMessage::DropEffectAt { location, position })
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

    // Build header with optional settings button for CLAP effects
    let header = if is_clap {
        // Settings button toggles open/close plugin GUI
        let settings_icon = if effect.gui_open { "✕" } else { "⚙" };
        let settings_color = if effect.gui_open { Color::from_rgb(0.9, 0.5, 0.5) } else { ACCENT_COLOR };
        let gui_message = if effect.gui_open {
            MultibandEditorMessage::ClosePluginGui { location, effect: effect_idx }
        } else {
            MultibandEditorMessage::OpenPluginGui { location, effect: effect_idx }
        };

        // Build name with optional latency
        let latency_text = format_latency(effect.latency_samples);
        let name_with_latency: Element<'_, MultibandEditorMessage> = if latency_text.is_empty() {
            text(&effect.name).size(11).color(name_color).into()
        } else {
            row![
                text(&effect.name).size(11).color(name_color),
                text(format!(" ({})", latency_text)).size(9).color(LATENCY_COLOR),
            ].spacing(0).into()
        };

        row![
            name_with_latency,
            Space::new().width(Length::Fill),
            // Settings button for CLAP plugins (toggles open/close)
            button(text(settings_icon).size(11).color(settings_color))
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
        // Build name with optional latency (non-CLAP)
        let latency_text = format_latency(effect.latency_samples);
        let name_with_latency: Element<'_, MultibandEditorMessage> = if latency_text.is_empty() {
            text(&effect.name).size(11).color(name_color).into()
        } else {
            row![
                text(&effect.name).size(11).color(name_color),
                text(format!(" ({})", latency_text)).size(9).color(LATENCY_COLOR),
            ].spacing(0).into()
        };

        row![
            name_with_latency,
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

            // Learning mode shows "LEARN" label, otherwise show truncated param name
            // Mapped params are indicated by green color, not "M1" label
            let label_text = if is_learning {
                "LEARN".to_string()
            } else {
                // Truncate param name to 4 chars max
                param_name[..param_name.len().min(4)].to_string()
            };

            // Get the current value for display
            let value_display = format!("{:.0}%", assignment.value * 100.0);

            // Get knob from state and apply modulation visualization if mapped
            let key = (location, effect_idx, knob_idx);
            let knob_element: Element<'_, MultibandEditorMessage> =
                if let Some(knob) = effect_knobs.get(&key) {
                    // Check if this knob has a macro mapping
                    let mut display_knob = knob.clone();
                    if let Some(ref mapping) = assignment.macro_mapping {
                        if let Some(macro_idx) = mapping.macro_index {
                            // Get current macro value
                            let macro_value = editor_state.macro_value(macro_idx);
                            // Calculate modulation bounds based on base value
                            let (mod_min, mod_max) = mapping.modulation_bounds(assignment.value);
                            // Set modulation range indicator
                            display_knob.set_modulations(vec![ModulationRange::new(
                                mod_min,
                                mod_max,
                                Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                            )]);
                            // Calculate and set the actual modulated value for the indicator dot
                            let modulated_value = mapping.modulate(assignment.value, macro_value);
                            display_knob.set_display_value(Some(modulated_value));
                        }
                    }
                    display_knob.view(move |event| MultibandEditorMessage::EffectKnob {
                        location,
                        effect: effect_idx,
                        param: knob_idx,
                        event,
                    })
                } else {
                    Space::new().width(48.0).height(48.0).into()
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
                    // Mapped params: click to remove, hover to highlight macro button
                    mouse_area(knob_container)
                        .on_press(MultibandEditorMessage::RemoveParamMapping {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .on_enter(MultibandEditorMessage::HoverParam {
                            location,
                            effect: effect_idx,
                            param: knob_idx,
                        })
                        .on_exit(MultibandEditorMessage::UnhoverParam)
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

    // Per-effect dry/wet knob on the left
    let is_dw_mapped = effect.dry_wet_macro_mapping.is_some();
    let dw_key = (location, effect_idx);
    let dry_wet_element: Element<'_, MultibandEditorMessage> = if let Some(knob) = editor_state.effect_dry_wet_knobs.get(&dw_key) {
        // Clone and add modulation ranges if mapped
        let mut display_knob = knob.clone();
        if let Some(ref mapping) = effect.dry_wet_macro_mapping {
            if let Some(macro_idx) = mapping.macro_index {
                let macro_value = editor_state.macro_value(macro_idx);
                let (mod_min, mod_max) = mapping.modulation_bounds(effect.dry_wet);
                display_knob.set_modulations(vec![ModulationRange::new(
                    mod_min,
                    mod_max,
                    Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                )]);
                let modulated_value = mapping.modulate(effect.dry_wet, macro_value);
                display_knob.set_display_value(Some(modulated_value));
            }
        }
        let dry_wet_knob = dry_wet_knob_view(
            &display_knob,
            "D/W",
            move |event| MultibandEditorMessage::EffectDryWetKnob {
                location,
                effect: effect_idx,
                event,
            },
            dragging_macro.is_some(),
            is_dw_mapped,
        );

        // Wrap dry/wet in macro drop target
        if let Some(macro_idx) = dragging_macro {
            mouse_area(dry_wet_knob)
                .on_release(MultibandEditorMessage::DropMacroOnEffectDryWet {
                    macro_index: macro_idx,
                    location,
                    effect: effect_idx,
                })
                .into()
        } else {
            dry_wet_knob
        }
    } else {
        // Fallback if knob doesn't exist yet
        text("D/W").size(10).color(TEXT_SECONDARY).into()
    };

    // Combine dry/wet knob with param knobs
    let knobs_with_dry_wet = row![
        dry_wet_element,
        container(Space::new()).width(Length::Fixed(1.0)).height(Length::Fill)
            .style(|_| container::Style {
                background: Some(BORDER_COLOR.into()),
                ..Default::default()
            }),
        knob_rows,
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    container(column![header, knobs_with_dry_wet].spacing(4).align_x(Alignment::Center))
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
            let is_highlighted = is_macro_highlighted(state, index);

            let name_color = if is_mapping_drag || is_highlighted {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else if m.mapping_count > 0 {
                ACCENT_COLOR
            } else {
                TEXT_SECONDARY
            };

            // Show border when dragging or when a mapped param is hovered
            let border_color = if is_mapping_drag || is_highlighted {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else {
                Color::TRANSPARENT
            };

            let border_width = if is_mapping_drag || is_highlighted { 2.0 } else { 0.0 };

            // Macro knobs don't show modulation indicators - they ARE the modulation source
            // Only target knobs (effect params, dry/wet) should show modulation ranges
            let knob_widget = knob.view(move |event| MultibandEditorMessage::MacroKnob {
                index,
                event,
            });

            // Build vertical modulation indicator column (on the side of the knob)
            let mod_indicators = mod_indicators_column(index, &state.macro_mappings_index[index], state);

            // Check if we're editing this macro's name
            let is_editing = state.editing_macro_name == Some(index);

            // Name display/editor - click to edit directly
            let name_element: Element<'_, MultibandEditorMessage> = if is_editing {
                // Show text input when editing
                text_input("Name...", &m.name)
                    .on_input(move |new_name| MultibandEditorMessage::RenameMacro {
                        index,
                        name: new_name,
                    })
                    .on_submit(MultibandEditorMessage::EndEditMacroName)
                    .size(11)
                    .width(Length::Fixed(80.0))
                    .into()
            } else {
                // Show text - click to edit
                let name_text = if m.mapping_count > 0 {
                    format!("{} ({})", m.name, m.mapping_count)
                } else {
                    m.name.clone()
                };
                mouse_area(
                    text(name_text).size(11).color(name_color)
                )
                .on_press(MultibandEditorMessage::StartEditMacroName(index))
                .into()
            };

            // Drag handle (9-dot grid icon) - only this triggers drag-to-map
            let drag_handle_color = if is_mapping_drag {
                Color::from_rgb(1.0, 0.8, 0.3)
            } else {
                Color::from_rgb(0.5, 0.5, 0.55)
            };
            let drag_handle: Element<'_, MultibandEditorMessage> = mouse_area(
                container(
                    text("⠿").size(16).color(drag_handle_color) // 9-dot braille pattern
                )
                .padding([2, 4])
                .style(move |_| container::Style {
                    background: Some(Color::from_rgba(0.3, 0.3, 0.35, 0.5).into()),
                    border: iced::Border {
                        radius: 3.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
            )
            .on_press(MultibandEditorMessage::StartDragMacro(index))
            .on_release(MultibandEditorMessage::EndDragMacro)
            .into();

            // Name row with drag handle on the left
            let name_row = row![
                drag_handle,
                name_element,
            ]
            .spacing(4)
            .align_y(Alignment::Center);

            // Main content: mod indicators on left, knob + name in center
            let knob_column = column![
                text(format!("{:.0}%", knob.value() * 100.0))
                    .size(12)
                    .color(TEXT_SECONDARY),
                knob_widget,
                name_row,
            ]
            .spacing(2)
            .align_x(Alignment::Center);

            let macro_content = row![
                mod_indicators,
                knob_column,
            ]
            .spacing(4)
            .align_y(Alignment::Center);

            // Container with highlight border (no drag from container itself)
            let styled_content: Element<'_, MultibandEditorMessage> = container(macro_content)
                .padding(4)
                .style(move |_| container::Style {
                    border: iced::Border {
                        color: border_color,
                        width: border_width,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                })
                .into();

            container(styled_content).width(Length::Fixed(130.0)).into()
        })
        .collect();

    // Global dry/wet control
    let global_dry_wet = state.global_dry_wet;
    let global_dw_mapped = state.global_dry_wet_macro_mapping.is_some();
    let global_dw_knob = {
        // Clone and add modulation ranges if mapped
        let mut display_knob = state.global_dry_wet_knob.clone();
        if let Some(ref mapping) = state.global_dry_wet_macro_mapping {
            if let Some(macro_idx) = mapping.macro_index {
                let macro_value = state.macro_value(macro_idx);
                let (mod_min, mod_max) = mapping.modulation_bounds(global_dry_wet);
                display_knob.set_modulations(vec![ModulationRange::new(
                    mod_min,
                    mod_max,
                    Color::from_rgb(0.9, 0.5, 0.2), // Orange for mod range
                )]);
                let modulated_value = mapping.modulate(global_dry_wet, macro_value);
                display_knob.set_display_value(Some(modulated_value));
            }
        }
        let knob_element = display_knob.view(|event| MultibandEditorMessage::GlobalDryWetKnob(event));

        let value_text = format!("{:.0}%", global_dry_wet * 100.0);

        // Highlight color when dragging macro or when mapped
        let label_color = if dragging_macro.is_some() {
            ACCENT_COLOR // Highlight as valid drop target
        } else if global_dw_mapped {
            Color::from_rgb(0.4, 0.8, 0.4) // Green for mapped
        } else {
            DRY_WET_COLOR
        };

        let content = column![
            text("Global").size(10).color(label_color),
            text("D/W").size(10).color(label_color),
            knob_element,
            text(value_text).size(10).color(TEXT_SECONDARY),
        ]
        .spacing(1)
        .align_x(Alignment::Center);

        // Wrap in macro drop target
        let content: Element<'_, MultibandEditorMessage> = if let Some(macro_idx) = dragging_macro {
            mouse_area(content)
                .on_release(MultibandEditorMessage::DropMacroOnGlobalDryWet { macro_index: macro_idx })
                .into()
        } else {
            content.into()
        };

        container(content)
            .padding(4)
            .style(|_| container::Style {
                background: Some(Color::from_rgba(0.2, 0.3, 0.35, 0.3).into()),
                border: iced::Border {
                    color: DRY_WET_COLOR.scale_alpha(0.3),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
    };

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
            row![
                row(macro_widgets).spacing(16).padding([0, 8]), // More spacing between macro knobs
                Space::new().width(Length::Fill),
                global_dw_knob,
            ]
            .spacing(16)
            .align_y(Alignment::Center),
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

/// Check if a macro button should be highlighted because a mapped param is hovered
fn is_macro_highlighted(state: &MultibandEditorState, macro_idx: usize) -> bool {
    use super::state::MappingTarget;

    if let Some((location, effect_idx, knob_idx)) = state.hovered_param {
        // Check if this macro is mapped to the hovered param
        let target = MappingTarget::Param { location, effect_idx, knob_idx };
        for mapping in &state.macro_mappings_index[macro_idx] {
            if mapping.target == target {
                return true;
            }
        }
    }
    false
}

/// Check if a specific effect parameter knob should be highlighted
///
/// Returns true if the user is hovering OR dragging a modulation indicator that targets this knob.
fn is_param_highlighted(
    state: &MultibandEditorState,
    location: EffectChainLocation,
    effect_idx: usize,
    knob_idx: usize,
) -> bool {
    use super::state::MappingTarget;

    let target = MappingTarget::Param { location, effect_idx, knob_idx };

    // Check if hovering a mod indicator that targets this param
    if let Some((macro_idx, map_idx)) = state.hovered_mapping {
        if let Some(mapping) = state.macro_mappings_index[macro_idx].get(map_idx) {
            if mapping.target == target {
                return true;
            }
        }
    }
    // Check if dragging a mod range indicator that targets this param
    if let Some(drag) = state.dragging_mod_range {
        if let Some(mapping) = state.macro_mappings_index[drag.macro_index].get(drag.mapping_idx) {
            if mapping.target == target {
                return true;
            }
        }
    }
    false
}

/// Render a dynamic column of modulation indicators to the side of a macro knob
/// Shows all mapped parameters, growing the column as needed
fn mod_indicators_column<'a>(
    macro_idx: usize,
    mappings: &[MacroMappingRef],
    state: &MultibandEditorState,
) -> Element<'a, MultibandEditorMessage> {
    // Fixed width for the indicator column (accommodates 16px wide indicators)
    const INDICATOR_COLUMN_WIDTH: f32 = 20.0;
    // Smaller indicators when there are many mappings
    const COMPACT_THRESHOLD: usize = 3;

    if mappings.is_empty() {
        // Empty placeholder with consistent width
        return container(Space::new())
            .width(Length::Fixed(INDICATOR_COLUMN_WIDTH))
            .height(Length::Fixed(64.0)) // Match knob height
            .into();
    }

    // Show ALL indicators - dynamically size based on count
    let indicators: Vec<Element<'_, MultibandEditorMessage>> = mappings
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if mappings.len() > COMPACT_THRESHOLD {
                // Compact mode for many mappings
                mod_range_indicator_compact(macro_idx, i, m, state)
            } else {
                mod_range_indicator(macro_idx, i, m, state)
            }
        })
        .collect();

    let col = column(indicators).spacing(1).align_x(Alignment::Center);

    container(col)
        .width(Length::Fixed(INDICATOR_COLUMN_WIDTH))
        .center_y(Length::Shrink)
        .into()
}

/// Render a modulation range indicator (16px × 28px bipolar bar)
///
/// Visual design:
/// - Center line at middle
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

    // Calculate visual representation (doubled from original 8x24 to 16x28)
    // offset_range is -1 to +1, representing full modulation range
    // The bar shows this as a fill from center: up for positive, down for negative
    let bar_width = 16.0_f32;
    let bar_height = 28.0_f32;
    let center_y = bar_height / 2.0;

    // Use full center_y as max fill so 100% offset fills from center to edge
    let max_fill = center_y;

    // Absolute fill amount (0 to max_fill), with minimum 3px for visibility
    let fill_amount = if offset_range.abs() > 0.01 {
        (offset_range.abs() * max_fill).max(3.0).min(max_fill)
    } else {
        0.0
    };

    // Calculate asymmetric padding to position the fill correctly
    // For positive offset: fill goes UP from center (top padding = center - fill)
    // For negative offset: fill goes DOWN from center (top padding = center)
    let top_pad = if offset_range >= 0.0 { center_y - fill_amount } else { center_y };

    // Create the indicator visual using a container with asymmetric padding
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
    .width(Length::Fixed(bar_width))
    .height(Length::Fixed(bar_height))
    .padding(iced::Padding::default().top(top_pad))  // Asymmetric: only top padding
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

/// Render a compact modulation range indicator (12px × 16px) for when there are many mappings
fn mod_range_indicator_compact<'a>(
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

    // Compact dimensions (smaller than regular)
    let bar_width = 12.0_f32;
    let bar_height = 16.0_f32;
    let center_y = bar_height / 2.0;
    let max_fill = center_y;

    let fill_amount = if offset_range.abs() > 0.01 {
        (offset_range.abs() * max_fill).max(2.0).min(max_fill)
    } else {
        0.0
    };

    let top_pad = if offset_range >= 0.0 { center_y - fill_amount } else { center_y };

    let bar_visual: Element<'_, MultibandEditorMessage> = container(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fixed(fill_amount))
            .style(move |_| container::Style {
                background: Some(fill_color.into()),
                ..Default::default()
            }),
    )
    .width(Length::Fixed(bar_width))
    .height(Length::Fixed(bar_height))
    .padding(iced::Padding::default().top(top_pad))
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
///
/// `available_presets` is the list of preset names to display.
pub fn preset_browser_overlay(available_presets: &[String]) -> Element<'_, MultibandEditorMessage> {
    let preset_list: Vec<Element<'_, MultibandEditorMessage>> = available_presets
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
///
/// `preset_name_input` is the current text in the name input field.
pub fn save_dialog_overlay(preset_name_input: &str) -> Element<'_, MultibandEditorMessage> {
    let can_save = !preset_name_input.trim().is_empty();

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
            text_input("Enter preset name...", preset_name_input)
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
