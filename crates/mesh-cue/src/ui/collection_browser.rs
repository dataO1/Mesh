//! Collection browser view with hierarchical playlist navigation

use super::app::{CollectionState, Message};
use super::editor;
use iced::widget::{column, container, row, rule, text, Space};
use iced::{Element, Length};
use mesh_widgets::playlist_browser;

/// Render the collection view (editor + dual browsers below)
pub fn view(state: &CollectionState) -> Element<Message> {
    let editor = view_editor(state);
    let browsers = view_browsers(state);

    column![
        editor,
        rule::horizontal(2),
        browsers,
    ]
    .spacing(10)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Track editor (top section)
fn view_editor(state: &CollectionState) -> Element<Message> {
    if let Some(ref loaded) = state.loaded_track {
        editor::view(loaded)
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
fn view_browsers(state: &CollectionState) -> Element<Message> {
    // Use cached tracks from state (updated when folder changes in message handlers)
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
