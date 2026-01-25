//! Collection browser view with hierarchical playlist navigation

use super::app::{BrowserSide, CollectionState, ImportState, Message};
use super::editor;
use iced::widget::{button, column, container, row, rule, text, Space};
use iced::{Alignment, Element, Length};
use mesh_widgets::playlist_browser_with_drop_highlight;

/// Render the collection view (editor + dual browsers below)
/// Note: Progress bar moved to main app view (always visible at bottom of screen)
/// Note: Modifier key handling (Shift/Ctrl) is done in app's update() handler
pub fn view<'a>(
    state: &'a CollectionState,
    _import_state: &'a ImportState,
    stem_link_selection: Option<usize>,
) -> Element<'a, Message> {
    let editor = view_editor(state, stem_link_selection);
    let browser_header = view_browser_header();
    let browsers = view_browsers(state);

    column![
        editor,
        rule::horizontal(2),
        browser_header,
        browsers,
    ]
    .spacing(5)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Header row above the browsers with Import and Export buttons
fn view_browser_header() -> Element<'static, Message> {
    let import_btn = button(text("Import").size(14))
        .on_press(Message::OpenImport)
        .style(button::secondary)
        .padding([4, 12]);

    let export_btn = button(text("Export").size(14))
        .on_press(Message::OpenExport)
        .style(button::secondary)
        .padding([4, 12]);

    container(
        row![
            text("Playlists").size(16),
            Space::new().width(Length::Fill),
            import_btn,
            export_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([0, 8]),
    )
    .width(Length::Fill)
    .into()
}

/// Track editor (top section)
fn view_editor(state: &CollectionState, stem_link_selection: Option<usize>) -> Element<'_, Message> {
    if let Some(ref loaded) = state.loaded_track {
        editor::view(loaded, stem_link_selection)
    } else {
        container(
            column![
                text("No track loaded").size(18),
                Space::new().height(20.0),
                text("Select a track from the browser below to load it for editing.").size(14),
            ]
            .spacing(10),
        )
        .padding(15)
        .width(Length::Fill)
        .height(Length::FillPortion(2))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }
}

/// Dual playlist browsers (bottom section)
fn view_browsers(state: &CollectionState) -> Element<'_, Message> {
    // Use cached tracks from state (updated when folder changes in message handlers)
    // Note: Modifier key handling (Shift/Ctrl) is done in app's update() handler

    // Determine which browser is the drop target (opposite of drag source)
    let (left_is_drop_target, right_is_drop_target) = match &state.dragging_track {
        Some(drag) => match drag.source_browser {
            BrowserSide::Left => (false, true),   // Dragging from left, drop on right
            BrowserSide::Right => (true, false),  // Dragging from right, drop on left
        },
        None => (false, false),
    };

    let left_browser = playlist_browser_with_drop_highlight(
        &state.tree_nodes,
        &state.left_tracks,
        &state.browser_left,
        |msg| Message::BrowserLeft(msg),
        left_is_drop_target,
    );

    let right_browser = playlist_browser_with_drop_highlight(
        &state.tree_nodes,
        &state.right_tracks,
        &state.browser_right,
        |msg| Message::BrowserRight(msg),
        right_is_drop_target,
    );

    row![
        container(left_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
        rule::vertical(2),
        container(right_browser)
            .width(Length::FillPortion(1))
            .height(Length::Fill),
    ]
    .spacing(0)
    .height(Length::FillPortion(1))  // Take remaining space proportionally
    .into()
}
