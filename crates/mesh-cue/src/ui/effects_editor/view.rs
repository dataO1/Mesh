//! Effects editor view

use iced::widget::{button, column, container, row, stack, text, Space};
use iced::{Alignment, Background, Color, Element, Length};
use mesh_widgets::{
    multiband_editor_content, preset_browser_overlay, save_dialog_overlay,
    MultibandEditorMessage,
};

use super::state::EffectsEditorState;
use crate::ui::message::Message;

// ─────────────────────────────────────────────────────────────────────────────
// Colors
// ─────────────────────────────────────────────────────────────────────────────

const BG_DARK: Color = Color::from_rgb(0.10, 0.10, 0.12);
const BG_MEDIUM: Color = Color::from_rgb(0.15, 0.15, 0.18);
const BORDER_COLOR: Color = Color::from_rgb(0.30, 0.30, 0.35);
const TEXT_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);

// ─────────────────────────────────────────────────────────────────────────────
// Main view
// ─────────────────────────────────────────────────────────────────────────────

/// Render the effects editor modal
///
/// Returns None if the editor is closed.
pub fn effects_editor_view(state: &EffectsEditorState) -> Option<Element<'_, Message>> {
    if !state.is_open {
        return None;
    }

    // Header with title and close button
    let header = header_view(state);

    // Toolbar with preset controls
    let toolbar = toolbar_view(state);

    // Stem editing tabs - switch which stem's effects are displayed
    let stem_tabs = stem_tab_bar(state);

    // Main editor content (from mesh-widgets) - using content-only version without header
    let editor_content = multiband_editor_content(&state.editor)
        .map(Message::EffectsEditor);

    // Status bar
    let status_bar: Element<'_, Message> = if !state.status.is_empty() {
        container(text(&state.status).size(11).color(TEXT_PRIMARY))
            .padding([4, 8])
            .into()
    } else {
        Space::new().height(0).into()
    };

    // Compose the modal - use reasonable fixed size
    let content = column![
        header,
        toolbar,
        stem_tabs,
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

    // Layer preset browser or save dialog on top if open
    let final_view: Element<'_, Message> = if state.editor.preset_browser_open {
        let overlay = preset_browser_overlay(&state.editor).map(Message::EffectsEditor);
        stack![modal, overlay].into()
    } else if state.editor.save_dialog_open {
        let overlay = save_dialog_overlay(&state.editor).map(Message::EffectsEditor);
        stack![modal, overlay].into()
    } else {
        modal
    };

    Some(final_view)
}

/// Render the header row
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

/// Render the toolbar with preset controls
fn toolbar_view(state: &EffectsEditorState) -> Element<'_, Message> {
    let preset_label = if let Some(ref name) = state.editing_preset {
        text(format!("Editing: {}", name)).size(12).color(TEXT_PRIMARY)
    } else {
        text("New Preset (unsaved)").size(12).color(Color::from_rgb(0.7, 0.7, 0.5))
    };

    let new_btn = button(text("New").size(11))
        .on_press(Message::EffectsEditorNewPreset)
        .padding([4, 12])
        .style(toolbar_button_style);

    let save_btn = button(text("Save").size(11))
        .on_press(Message::EffectsEditorOpenSaveDialog)
        .padding([4, 12])
        .style(toolbar_button_style);

    let load_btn = button(text("Load").size(11))
        .on_press(Message::EffectsEditor(MultibandEditorMessage::OpenPresetBrowser))
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
            preset_label,
            Space::new().width(16),
            preview_btn,
            Space::new().width(Length::Fill),
            new_btn,
            load_btn,
            save_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    )
    .padding([8, 16])
    .width(Length::Fill)
    .style(toolbar_container_style)
    .into()
}

/// Render the stem editing tab bar
///
/// Shows [VOC] [DRM] [BAS] [OTH] tabs for switching which stem's effects
/// are displayed in the editor. The active stem is highlighted.
fn stem_tab_bar(state: &EffectsEditorState) -> Element<'_, Message> {
    const STEM_LABELS: [&str; 4] = ["VOC", "DRM", "BAS", "OTH"];

    let tabs: Vec<Element<'_, Message>> = (0..4)
        .map(|idx| {
            let is_active = state.active_stem == idx;
            let has_data = state.stem_data[idx].is_some()
                || (is_active && (!state.editor.pre_fx.is_empty()
                    || !state.editor.bands.is_empty()
                    || !state.editor.post_fx.is_empty()));
            let label = if let Some(ref name) = state.stem_preset_names[idx] {
                format!("{} ({})", STEM_LABELS[idx], name)
            } else {
                STEM_LABELS[idx].to_string()
            };
            stem_tab_button(label, idx, is_active, has_data)
        })
        .collect();

    container(
        row![
            text("STEM").size(10).color(Color::from_rgb(0.5, 0.5, 0.55)),
            Space::new().width(8),
            row(tabs).spacing(2),
        ]
        .align_y(Alignment::Center)
    )
    .padding([6, 16])
    .width(Length::Fill)
    .style(stem_tab_bar_style)
    .into()
}

/// Create a single stem editing tab button
fn stem_tab_button(label: String, stem_idx: usize, is_active: bool, has_data: bool) -> Element<'static, Message> {
    let btn_text = if has_data && !is_active {
        // Dot indicator for stems with effects loaded
        text(format!("{} ●", label)).size(11)
    } else {
        text(label).size(11)
    };

    button(btn_text)
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
            radius: 8.0.into(), // Top corners only handled by clip
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
