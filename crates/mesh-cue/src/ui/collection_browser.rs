//! Collection browser view with hierarchical playlist navigation

use super::app::{CollectionState, ImportState, Message};
use super::editor;
use iced::widget::{button, column, container, row, rule, text, Space};
use iced::{Alignment, Element, Length};
use mesh_widgets::playlist_browser;

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

/// Header row above the browsers with Import button
fn view_browser_header() -> Element<'static, Message> {
    let import_btn = button(text("Import").size(14))
        .on_press(Message::OpenImport)
        .style(button::secondary)
        .padding([4, 12]);

    container(
        row![
            text("Playlists").size(16),
            Space::new().width(Length::Fill),
            import_btn,
        ]
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
    let left_browser = playlist_browser(
        &state.tree_nodes,
        &state.left_tracks,
        &state.browser_left,
        |msg| Message::BrowserLeft(msg),
    );

    let right_browser = playlist_browser(
        &state.tree_nodes,
        &state.right_tracks,
        &state.browser_right,
        |msg| Message::BrowserRight(msg),
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
    .height(Length::Fixed(300.0))
    .into()
}
