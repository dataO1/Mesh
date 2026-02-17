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
const TEXT_DIM: Color = Color::from_rgb(0.55, 0.55, 0.60);

// ─────────────────────────────────────────────────────────────────────────────
// Main view
// ─────────────────────────────────────────────────────────────────────────────

/// Render the slicer editor modal content
///
/// Always returns an element when called — the modal opens independently of
/// whether a track is loaded. When no track is loaded, a placeholder message
/// is shown instead of the editor grid.
///
/// Layout:
/// ```text
/// ┌──────────────────────────────────────────────┐
/// │  SLICER                                  [×] │
/// ├──────────────────────────────────────────────┤
/// │  Usage instructions                          │
/// ├──────────────────────────────────────────────┤
/// │  [slice_editor widget]         [Save Presets]│
/// └──────────────────────────────────────────────┘
/// ```
pub fn slicer_editor_view<'a>(
    _state: &SlicerEditorState,
    loaded_track: Option<&'a LoadedTrackState>,
) -> Element<'a, Message> {
    // Header: title + close button
    let title = text("SLICER").size(18).color(TEXT_PRIMARY);

    let close_btn = button(text("\u{00d7}").size(20))
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

    // Usage instructions
    let instructions = container(
        text(
            "Click cells to toggle slices at each step. \
             Use the stem buttons (V/D/B/O) on the left to assign rows to stems. \
             Mute buttons silence individual steps. \
             Switch between 8 preset banks with the tabs at the bottom."
        )
        .size(12)
        .color(TEXT_DIM),
    )
    .padding([8, 16])
    .width(Length::Fill);

    // Body: either the editor or a placeholder
    let body: Element<'a, Message> = if let Some(track) = loaded_track {
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

        container(
            row![slice_editor_widget, save_presets_btn]
                .spacing(8)
                .align_y(Alignment::End),
        )
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        container(
            text("Load a track to edit slicer presets")
                .size(14)
                .color(TEXT_DIM),
        )
        .padding(24)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    };

    let content = column![header, instructions, body].spacing(0);

    container(content)
        .style(modal_style)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .into()
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
