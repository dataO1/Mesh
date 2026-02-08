//! View function for the stem preset widget

use iced::widget::{button, column, container, row, scrollable, slider, text, Space};
use iced::{Alignment, Background, Color, Element, Length};

use super::message::StemPresetMessage;
use super::StemPresetState;

// ─────────────────────────────────────────────────────────────────────────────
// Colors
// ─────────────────────────────────────────────────────────────────────────────

const BG_DARK: Color = Color::from_rgb(0.12, 0.12, 0.14);
const BG_MEDIUM: Color = Color::from_rgb(0.18, 0.18, 0.20);
const BORDER_COLOR: Color = Color::from_rgb(0.35, 0.35, 0.40);
const TEXT_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);
const TEXT_SECONDARY: Color = Color::from_rgb(0.6, 0.6, 0.65);
const ACCENT_COLOR: Color = Color::from_rgb(0.3, 0.7, 0.9);

// ─────────────────────────────────────────────────────────────────────────────
// Main view function
// ─────────────────────────────────────────────────────────────────────────────

/// Render the stem preset selector widget
///
/// Layout:
/// ```text
/// ┌─────────────────────────────────────────────────────────────────┐
/// │  [Preset Name ▾]                                                │
/// │  ┌──────┬──────┬──────┬──────┬──────┬──────┬──────┬──────┐     │
/// │  │ M1   │ M2   │ M3   │ M4   │ M5   │ M6   │ M7   │ M8   │     │
/// │  │ ━━━━ │ ━━━━ │ ━━━━ │ ━━━━ │ ━━━━ │ ━━━━ │ ━━━━ │ ━━━━ │     │
/// │  └──────┴──────┴──────┴──────┴──────┴──────┴──────┴──────┘     │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
pub fn stem_preset_view(state: &StemPresetState) -> Element<'_, StemPresetMessage> {
    // Preset selector button/dropdown
    let preset_selector = preset_dropdown(state);

    // Macro knobs row
    let macros = macro_knobs_row(state);

    column![preset_selector, macros]
        .spacing(4)
        .width(Length::Fill)
        .into()
}

/// Render the preset dropdown button and picker
fn preset_dropdown(state: &StemPresetState) -> Element<'_, StemPresetMessage> {
    let label = state
        .loaded_preset
        .as_deref()
        .unwrap_or("No Preset");

    let dropdown_btn = button(
        row![
            text(label).size(10),
            Space::new().width(Length::Fill),
            text("▾").size(10),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .on_press(StemPresetMessage::TogglePicker)
    .padding([4, 8])
    .width(Length::Fill)
    .style(dropdown_button_style);

    if state.picker_open {
        // Show dropdown with preset list
        let preset_list = preset_picker_list(state);

        column![dropdown_btn, preset_list]
            .spacing(2)
            .width(Length::Fill)
            .into()
    } else {
        dropdown_btn.into()
    }
}

/// Render the preset picker list
fn preset_picker_list(state: &StemPresetState) -> Element<'_, StemPresetMessage> {
    let mut items: Vec<Element<'_, StemPresetMessage>> = Vec::new();

    // "No Preset" option (passthrough)
    let no_preset_style = if state.loaded_preset.is_none() {
        preset_item_selected_style
    } else {
        preset_item_style
    };

    items.push(
        button(text("(No Preset)").size(9))
            .on_press(StemPresetMessage::SelectPreset(None))
            .padding([3, 8])
            .width(Length::Fill)
            .style(no_preset_style)
            .into(),
    );

    // Available presets
    for preset_name in &state.available_presets {
        let is_selected = state.loaded_preset.as_ref() == Some(preset_name);
        let style = if is_selected {
            preset_item_selected_style
        } else {
            preset_item_style
        };

        let name = preset_name.clone();
        items.push(
            button(text(preset_name).size(9))
                .on_press(StemPresetMessage::SelectPreset(Some(name)))
                .padding([3, 8])
                .width(Length::Fill)
                .style(style)
                .into(),
        );
    }

    let list = scrollable(column(items).spacing(1).width(Length::Fill))
        .height(Length::Fixed(120.0));

    container(list)
        .padding(4)
        .width(Length::Fill)
        .style(picker_container_style)
        .into()
}

/// Render the macro knobs row
fn macro_knobs_row(state: &StemPresetState) -> Element<'_, StemPresetMessage> {
    let knobs: Vec<Element<'_, StemPresetMessage>> = (0..8)
        .map(|i| macro_knob(state, i))
        .collect();

    row(knobs)
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Render a single macro knob with label
fn macro_knob(state: &StemPresetState, index: usize) -> Element<'_, StemPresetMessage> {
    let value = state.macro_value(index);
    let name = state.macro_name(index);

    // Truncate name if too long
    let display_name = if name.len() > 6 {
        format!("{}…", &name[..5])
    } else {
        name.to_string()
    };

    column![
        text(display_name).size(7).color(TEXT_SECONDARY),
        slider(0.0..=1.0, value, move |v| StemPresetMessage::SetMacro {
            index,
            value: v,
        })
        .width(50),  // Wider for better control (was 28)
    ]
    .spacing(1)
    .align_x(Alignment::Center)
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Button styles
// ─────────────────────────────────────────────────────────────────────────────

fn dropdown_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(BG_MEDIUM)),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

fn preset_item_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(BG_DARK)),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn preset_item_selected_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(ACCENT_COLOR)),
        text_color: Color::WHITE,
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn picker_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_DARK)),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}
