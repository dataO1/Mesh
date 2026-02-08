//! Effects editor view

use iced::widget::{button, column, container, row, stack, text, Space};
use iced::{Alignment, Background, Color, Element, Length};
use mesh_core::types::Stem;
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

    // Audio preview controls - stem selector and preview toggle
    let stem_buttons = row![
        stem_button("VOC", Stem::Vocals, state.preview_stem),
        stem_button("DRM", Stem::Drums, state.preview_stem),
        stem_button("BAS", Stem::Bass, state.preview_stem),
        stem_button("OTH", Stem::Other, state.preview_stem),
    ]
    .spacing(2);

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
            stem_buttons,
            Space::new().width(8),
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

/// Create a stem selector button
fn stem_button(label: &'static str, stem: Stem, selected: Stem) -> Element<'static, Message> {
    let is_selected = stem == selected;
    button(text(label).size(10))
        .on_press(Message::EffectsEditorSetPreviewStem(stem))
        .padding([3, 8])
        .style(if is_selected {
            stem_button_selected_style
        } else {
            stem_button_style
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

fn stem_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.20, 0.20, 0.24))),
        text_color: Color::from_rgb(0.6, 0.6, 0.6),
        border: iced::Border {
            color: BORDER_COLOR,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}

fn stem_button_selected_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::from_rgb(0.35, 0.55, 0.75))),
        text_color: TEXT_PRIMARY,
        border: iced::Border {
            color: Color::from_rgb(0.45, 0.65, 0.85),
            width: 1.0,
            radius: 3.0.into(),
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
