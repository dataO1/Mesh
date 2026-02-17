//! Slicer editor modal view

use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Background, Color, Element, Length};
use mesh_widgets::slice_editor;

use super::state::SlicerEditorState;
use crate::ui::app::LoadedTrackState;
use crate::ui::message::Message;

// ─────────────────────────────────────────────────────────────────────────────
// Colors (matching effects_editor palette)
// ─────────────────────────────────────────────────────────────────────────────

const BG_DARK: Color = Color::from_rgb(0.10, 0.10, 0.12);
const BG_MEDIUM: Color = Color::from_rgb(0.15, 0.15, 0.18);
const BORDER_COLOR: Color = Color::from_rgb(0.30, 0.30, 0.35);
const TEXT_PRIMARY: Color = Color::from_rgb(0.9, 0.9, 0.9);

// ─────────────────────────────────────────────────────────────────────────────
// Main view
// ─────────────────────────────────────────────────────────────────────────────

/// Render the slicer editor modal
///
/// Returns `None` if the modal is closed or no track is loaded.
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────┐
/// │  SLICER                                  [×] │
/// ├──────────────────────────────────────────────┤
/// │  [slice_editor widget]         [Save Presets]│
/// └──────────────────────────────────────────────┘
/// ```
pub fn slicer_editor_view<'a>(
    state: &SlicerEditorState,
    loaded_track: Option<&'a LoadedTrackState>,
) -> Option<Element<'a, Message>> {
    if !state.is_open {
        return None;
    }

    let track = loaded_track?;

    // Header: title + close button
    let title = text("SLICER").size(18).color(TEXT_PRIMARY);

    let close_btn = button(text("×").size(20))
        .on_press(Message::CloseSlicerEditor)
        .padding([4, 10])
        .style(close_button_style);

    let header = container(
        row![title, Space::new().width(Length::Fill), close_btn]
            .align_y(Alignment::Center),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(header_style);

    // Content: slice editor widget + save button
    let slice_editor_widget = slice_editor(
        &track.slice_editor,
        |step, slice| Message::SliceEditorCellToggle { step, slice },
        Message::SliceEditorMuteToggle,
        Message::SliceEditorStemClick,
        Message::SliceEditorPresetSelect,
    );

    let save_presets_btn = button(text("Save Presets").size(11))
        .padding([4, 8])
        .on_press(Message::SaveSlicerPresets);

    let body = container(
        row![slice_editor_widget, save_presets_btn]
            .spacing(8)
            .align_y(Alignment::End),
    )
    .padding(12);

    let content = column![header, body].spacing(0);

    let modal: Element<'a, Message> = container(content)
        .style(modal_style)
        .padding(0)
        .into();

    Some(modal)
}

// ─────────────────────────────────────────────────────────────────────────────
// Styles
// ─────────────────────────────────────────────────────────────────────────────

fn modal_style(_theme: &iced::Theme) -> container::Style {
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

fn header_style(_theme: &iced::Theme) -> container::Style {
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

fn close_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: TEXT_PRIMARY,
        border: iced::Border::default(),
        ..Default::default()
    }
}
