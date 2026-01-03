//! Collection browser view

use super::app::{CollectionState, Message};
use super::editor;
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Element, Length};

/// Render the collection view (browser + editor)
pub fn view(state: &CollectionState) -> Element<Message> {
    let browser = view_browser(state);
    let editor = view_editor(state);

    row![browser, editor]
        .spacing(20)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Collection browser (left panel)
fn view_browser(state: &CollectionState) -> Element<Message> {
    let title = text("Collection").size(18);

    let path_text = text(state.collection.path().display().to_string()).size(12);

    let refresh_btn = button(text("Refresh")).on_press(Message::RefreshCollection);

    let track_list: Element<Message> = if state.collection.tracks().is_empty() {
        text("No tracks in collection").size(14).into()
    } else {
        let items: Vec<Element<Message>> = state
            .collection
            .tracks()
            .iter()
            .enumerate()
            .map(|(i, track)| {
                let is_selected = state.selected_track == Some(i);

                let info = format!(
                    "{} - {:.1} BPM - {}",
                    track.name,
                    track.bpm,
                    track.key
                );

                button(text(info).size(14))
                    .on_press(Message::SelectTrack(i))
                    .style(if is_selected { button::primary } else { button::secondary })
                    .width(Length::Fill)
                    .into()
            })
            .collect();

        scrollable(column(items).spacing(4))
            .height(Length::Fill)
            .into()
    };

    let load_btn = button(text("Load Selected"))
        .on_press_maybe(state.selected_track.map(Message::LoadTrack));

    container(
        column![title, path_text, refresh_btn, track_list, load_btn,].spacing(10),
    )
    .padding(15)
    .width(Length::Fixed(300.0))
    .height(Length::Fill)
    .into()
}

/// Track editor (right panel)
fn view_editor(state: &CollectionState) -> Element<Message> {
    if let Some(ref loaded) = state.loaded_track {
        editor::view(loaded)
    } else {
        container(
            column![
                text("No track loaded").size(18),
                Space::new().height(20.0),
                text("Double-click a track in the collection to load it for editing.").size(14),
            ]
            .spacing(10),
        )
        .padding(15)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    }
}
