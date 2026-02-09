//! Effects editor view

use iced::widget::{button, column, container, row, scrollable, stack, text, text_input, Space};
use iced::{Alignment, Background, Color, Element, Length};
use mesh_widgets::{multiband_editor_content, MultibandEditorMessage};

use super::state::{EffectsEditorState, PresetBrowserMode, SaveDialogMode};
use crate::ui::message::Message;

// ─────────────────────────────────────────────────────────────────────────────
// Colors
// ─────────────────────────────────────────────────────────────────────────────

const BG_DARK: Color = Color::from_rgb(0.10, 0.10, 0.12);
const BG_MEDIUM: Color = Color::from_rgb(0.15, 0.15, 0.18);
const BORDER_COLOR: Color = Color::from_rgb(0.30, 0.30, 0.35);
const TEXT_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);
const TEXT_SECONDARY: Color = Color::from_rgb(0.55, 0.55, 0.60);
const ACCENT_COLOR: Color = Color::from_rgb(0.40, 0.65, 0.90);

// ─────────────────────────────────────────────────────────────────────────────
// Main view
// ─────────────────────────────────────────────────────────────────────────────

/// Render the effects editor modal
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────────────────────────┐
/// │  FX PRESETS                                                  [×] │
/// ├──────────────────────────────────────────────────────────────────┤
/// │  Deck: "My Deck"  [New] [Load] [Save]            🔊 Preview     │
/// ├──────────────────────────────────────────────────────────────────┤
/// │  STEM [VOC●] [DRM] [BAS●] [OTH]  │  Stem: "reverb" [Load][Save]│
/// ├──────────────────────────────────────────────────────────────────┤
/// │  [editor content]                                                │
/// └──────────────────────────────────────────────────────────────────┘
/// ```
pub fn effects_editor_view(state: &EffectsEditorState) -> Option<Element<'_, Message>> {
    if !state.is_open {
        return None;
    }

    let header = header_view(state);
    let deck_toolbar = deck_toolbar_view(state);
    let stem_toolbar = stem_toolbar_view(state);

    let editor_content = multiband_editor_content(&state.editor)
        .map(Message::EffectsEditor);

    let status_bar: Element<'_, Message> = if !state.status.is_empty() {
        container(text(&state.status).size(11).color(TEXT_PRIMARY))
            .padding([4, 8])
            .into()
    } else {
        Space::new().height(0).into()
    };

    let content = column![
        header,
        deck_toolbar,
        stem_toolbar,
        container(editor_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(8),
        status_bar,
    ]
    .spacing(0)
    .width(Length::Fixed(1700.0))
    .height(Length::Fixed(950.0));

    let modal: Element<'_, Message> = container(content)
        .style(modal_container_style)
        .padding(0)
        .into();

    // Layer overlays based on preset_browser_mode and save_dialog_mode
    let final_view: Element<'_, Message> = match state.preset_browser_mode {
        PresetBrowserMode::Stem => {
            let overlay = stem_preset_browser_overlay(state);
            stack![modal, overlay].into()
        }
        PresetBrowserMode::Deck => {
            let overlay = deck_preset_browser_overlay(state);
            stack![modal, overlay].into()
        }
        PresetBrowserMode::None => match state.save_dialog_mode {
            SaveDialogMode::Stem => {
                let overlay = stem_save_dialog_overlay(state);
                stack![modal, overlay].into()
            }
            SaveDialogMode::Deck => {
                let overlay = deck_save_dialog_overlay(state);
                stack![modal, overlay].into()
            }
            SaveDialogMode::None => modal,
        },
    };

    Some(final_view)
}

// ─────────────────────────────────────────────────────────────────────────────
// Header
// ─────────────────────────────────────────────────────────────────────────────

fn header_view(_state: &EffectsEditorState) -> Element<'_, Message> {
    let title = text("FX PRESETS")
        .size(18)
        .color(TEXT_PRIMARY);

    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseEffectsEditor)
        .padding([4, 10])
        .style(close_button_style);

    container(
        row![
            title,
            Space::new().width(Length::Fill),
            close_btn,
        ]
        .align_y(Alignment::Center)
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(header_container_style)
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Deck toolbar row
// ─────────────────────────────────────────────────────────────────────────────

/// Top toolbar row: deck preset name + New/Load/Save + preview toggle
fn deck_toolbar_view(state: &EffectsEditorState) -> Element<'_, Message> {
    let deck_label = if let Some(ref name) = state.deck_preset_name {
        text(format!("Deck: {}", name)).size(12).color(TEXT_PRIMARY)
    } else {
        text("Deck: (none)").size(12).color(TEXT_SECONDARY)
    };

    let new_btn = button(text("New").size(11))
        .on_press(Message::EffectsEditorNewPreset)
        .padding([4, 12])
        .style(toolbar_button_style);

    let load_btn = button(text("Load").size(11))
        .on_press(Message::EffectsEditorOpenDeckPresetBrowser)
        .padding([4, 12])
        .style(toolbar_button_style);

    let save_btn = button(text("Save").size(11))
        .on_press(Message::EffectsEditorOpenDeckSaveDialog)
        .padding([4, 12])
        .style(toolbar_button_style);

    // Audio preview toggle
    let preview_icon = if state.audio_preview_enabled { "🔊" } else { "🔇" };
    let preview_btn = button(text(format!("{} Preview", preview_icon)).size(11))
        .on_press(Message::EffectsEditorTogglePreview)
        .padding([4, 12])
        .style(if state.audio_preview_enabled {
            preview_active_button_style
        } else {
            toolbar_button_style
        });

    container(
        row![
            deck_label,
            Space::new().width(16),
            new_btn,
            load_btn,
            save_btn,
            Space::new().width(Length::Fill),
            preview_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(toolbar_container_style)
    .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Stem toolbar row (merged stem tabs + stem preset controls)
// ─────────────────────────────────────────────────────────────────────────────

/// Bottom toolbar row: stem tabs + stem preset name + Load/Save
fn stem_toolbar_view(state: &EffectsEditorState) -> Element<'_, Message> {
    const STEM_LABELS: [&str; 4] = ["VOC", "DRM", "BAS", "OTH"];

    let tabs: Vec<Element<'_, Message>> = (0..4)
        .map(|idx| {
            let is_active = state.active_stem == idx;
            let has_data = state.stem_data[idx].is_some()
                || (is_active && (!state.editor.pre_fx.is_empty()
                    || !state.editor.bands.is_empty()
                    || !state.editor.post_fx.is_empty()));

            let label = STEM_LABELS[idx].to_string();
            let label = if has_data && !is_active {
                format!("{} ●", label)
            } else {
                label
            };

            stem_tab_button(label, idx, is_active, has_data)
        })
        .collect();

    // Stem preset info
    let stem_label = if let Some(ref name) = state.stem_preset_names[state.active_stem] {
        text(format!("Stem: {}", name)).size(12).color(TEXT_PRIMARY)
    } else {
        text("Stem: (unsaved)").size(12).color(TEXT_SECONDARY)
    };

    let stem_load_btn = button(text("Load").size(11))
        .on_press(Message::EffectsEditorOpenStemPresetBrowser)
        .padding([4, 12])
        .style(toolbar_button_style);

    let stem_save_btn = button(text("Save").size(11))
        .on_press(Message::EffectsEditorOpenStemSaveDialog)
        .padding([4, 12])
        .style(toolbar_button_style);

    container(
        row![
            text("STEM").size(10).color(TEXT_SECONDARY),
            Space::new().width(8),
            row(tabs).spacing(2),
            Space::new().width(16),
            // Vertical separator
            container(Space::new().width(1).height(20))
                .style(separator_style),
            Space::new().width(16),
            stem_label,
            Space::new().width(8),
            stem_load_btn,
            stem_save_btn,
            Space::new().width(Length::Fill),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
    )
    .padding([6, 16])
    .width(Length::Fill)
    .style(stem_tab_bar_style)
    .into()
}

/// Create a single stem editing tab button
fn stem_tab_button(label: String, stem_idx: usize, is_active: bool, has_data: bool) -> Element<'static, Message> {
    button(text(label).size(11))
        .on_press(Message::EffectsEditor(MultibandEditorMessage::SwitchStem(stem_idx)))
        .padding([5, 14])
        .style(if is_active {
            stem_tab_active_style
        } else if has_data {
            stem_tab_with_data_style
        } else {
            stem_tab_inactive_style
        })
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset browser overlays
// ─────────────────────────────────────────────────────────────────────────────

/// Stem preset browser overlay — lists available stem presets
fn stem_preset_browser_overlay(state: &EffectsEditorState) -> Element<'_, Message> {
    preset_browser_overlay_impl(
        "Load Stem Preset",
        &state.available_stem_presets,
        |name| Message::EffectsEditorLoadStemPreset(name),
        |name| Message::EffectsEditorDeleteStemPreset(name),
        Message::EffectsEditorCloseStemPresetBrowser,
    )
}

/// Deck preset browser overlay — lists available deck presets
fn deck_preset_browser_overlay(state: &EffectsEditorState) -> Element<'_, Message> {
    preset_browser_overlay_impl(
        "Load Deck Preset",
        &state.available_deck_presets,
        |name| Message::EffectsEditorLoadDeckPreset(name),
        |name| Message::EffectsEditorDeleteDeckPreset(name),
        Message::EffectsEditorCloseDeckPresetBrowser,
    )
}

/// Generic preset browser overlay implementation
fn preset_browser_overlay_impl<'a>(
    title: &'a str,
    presets: &'a [String],
    on_load: impl Fn(String) -> Message + 'a,
    on_delete: impl Fn(String) -> Message + 'a,
    on_close: Message,
) -> Element<'a, Message> {
    let preset_list: Vec<Element<'a, Message>> = presets
        .iter()
        .map(|name| {
            let name_for_load = name.clone();
            let name_for_delete = name.clone();
            row![
                button(text(name.as_str()).size(14))
                    .padding([8, 16])
                    .width(Length::Fill)
                    .on_press(on_load(name_for_load)),
                button(text("×").size(14).color(TEXT_SECONDARY))
                    .padding([8, 8])
                    .on_press(on_delete(name_for_delete)),
            ]
            .spacing(4)
            .into()
        })
        .collect();

    let content: Element<'_, Message> = if preset_list.is_empty() {
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
                text(title).size(14).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(14))
                    .padding([4, 8])
                    .on_press(on_close),
            ]
            .align_y(Alignment::Center),
            overlay_divider(),
            content,
        ]
        .spacing(12)
        .padding(16),
    )
    .width(Length::Fixed(350.0))
    .style(overlay_dialog_style);

    container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(overlay_backdrop_style)
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Save dialog overlays
// ─────────────────────────────────────────────────────────────────────────────

/// Stem save dialog overlay
fn stem_save_dialog_overlay(state: &EffectsEditorState) -> Element<'_, Message> {
    save_dialog_overlay_impl(
        "Save Stem Preset",
        &state.stem_preset_name_input,
        |s| Message::EffectsEditorSetStemPresetNameInput(s),
        Message::EffectsEditorSaveStemPreset(state.stem_preset_name_input.trim().to_string()),
        Message::EffectsEditorCloseSaveDialog,
    )
}

/// Deck save dialog overlay
fn deck_save_dialog_overlay(state: &EffectsEditorState) -> Element<'_, Message> {
    save_dialog_overlay_impl(
        "Save Deck Preset",
        &state.deck_preset_name_input,
        |s| Message::EffectsEditorSetDeckPresetNameInput(s),
        Message::EffectsEditorSaveDeckPreset(state.deck_preset_name_input.trim().to_string()),
        Message::EffectsEditorCloseSaveDialog,
    )
}

/// Generic save dialog overlay implementation
fn save_dialog_overlay_impl<'a>(
    title: &'a str,
    name_input: &str,
    on_input: impl Fn(String) -> Message + 'a,
    on_save: Message,
    on_close: Message,
) -> Element<'a, Message> {
    let can_save = !name_input.trim().is_empty();

    let dialog = container(
        column![
            row![
                text(title).size(14).color(TEXT_PRIMARY),
                Space::new().width(Length::Fill),
                button(text("×").size(14))
                    .padding([4, 8])
                    .on_press(on_close.clone()),
            ]
            .align_y(Alignment::Center),
            overlay_divider(),
            text("Preset Name:").size(14).color(TEXT_SECONDARY),
            text_input("Enter preset name...", name_input)
                .on_input(on_input)
                .padding(8)
                .size(14),
            row![
                button(text("Cancel").size(14))
                    .padding([8, 16])
                    .on_press(on_close),
                Space::new().width(Length::Fill),
                if can_save {
                    button(text("Save").size(14).color(ACCENT_COLOR))
                        .padding([8, 16])
                        .on_press(on_save)
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
    .style(overlay_dialog_style);

    container(dialog)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(overlay_backdrop_style)
        .into()
}

// ─────────────────────────────────────────────────────────────────────────────
// Styles
// ─────────────────────────────────────────────────────────────────────────────

fn modal_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_DARK)),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn header_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_MEDIUM)),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 0.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn toolbar_container_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.12, 0.12, 0.14))),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn close_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: TEXT_PRIMARY,
        border: iced::Border::default(),
        ..Default::default()
    }
}

fn toolbar_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn preview_active_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.25, 0.55, 0.35))),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: Color::from_rgb(0.35, 0.65, 0.45),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn stem_tab_bar_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(0.13, 0.13, 0.15))),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn stem_tab_active_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.30, 0.55, 0.80))),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: Color::from_rgb(0.40, 0.65, 0.90),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn stem_tab_with_data_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.22, 0.22, 0.28))),
        text_color: Color::from_rgb(0.75, 0.75, 0.80),
        border: iced::Border {
            color: Color::from_rgb(0.40, 0.40, 0.50),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn stem_tab_inactive_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.17, 0.17, 0.20))),
        text_color: Color::from_rgb(0.5, 0.5, 0.55),
        border: iced::Border {
            color: Color::from_rgb(0.30, 0.30, 0.35),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

fn separator_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_COLOR)),
        ..Default::default()
    }
}

fn overlay_dialog_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_DARK)),
        border: iced::Border {
            color: ACCENT_COLOR,
            width: 2.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn overlay_backdrop_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
        ..Default::default()
    }
}

/// Horizontal divider line for overlays
fn overlay_divider<'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(BORDER_COLOR)),
            ..Default::default()
        })
        .into()
}
