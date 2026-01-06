//! Context menu component for right-click actions
//!
//! Provides popup menus for tracks and playlists with appropriate actions.

use super::app::Message;
use iced::widget::{button, column, container, text};
use iced::{Background, Border, Element, Length, Point};
use mesh_core::playlist::NodeId;

/// What kind of item the context menu is for
#[derive(Debug, Clone)]
pub enum ContextMenuKind {
    /// Context menu for a track in the collection (permanent actions)
    CollectionTrack {
        track_id: NodeId,
        track_name: String,
    },
    /// Context menu for a track in a playlist (non-destructive)
    PlaylistTrack {
        track_id: NodeId,
        track_name: String,
    },
    /// Context menu for a playlist folder
    Playlist {
        playlist_id: NodeId,
        playlist_name: String,
    },
}

impl ContextMenuKind {
    /// Get the ID of the item
    pub fn id(&self) -> &NodeId {
        match self {
            ContextMenuKind::CollectionTrack { track_id, .. } => track_id,
            ContextMenuKind::PlaylistTrack { track_id, .. } => track_id,
            ContextMenuKind::Playlist { playlist_id, .. } => playlist_id,
        }
    }

    /// Get the display name
    pub fn name(&self) -> &str {
        match self {
            ContextMenuKind::CollectionTrack { track_name, .. } => track_name,
            ContextMenuKind::PlaylistTrack { track_name, .. } => track_name,
            ContextMenuKind::Playlist { playlist_name, .. } => playlist_name,
        }
    }
}

/// State for the context menu
#[derive(Debug, Clone, Default)]
pub struct ContextMenuState {
    /// Whether the menu is open
    pub is_open: bool,
    /// What kind of menu and for what item
    pub kind: Option<ContextMenuKind>,
    /// Position to display the menu (screen coordinates)
    pub position: Point,
}

impl ContextMenuState {
    /// Show a context menu
    pub fn show(&mut self, kind: ContextMenuKind, position: Point) {
        self.kind = Some(kind);
        self.position = position;
        self.is_open = true;
    }

    /// Close the context menu
    pub fn close(&mut self) {
        self.is_open = false;
        self.kind = None;
    }
}

/// Render a context menu at the given position
pub fn view(state: &ContextMenuState) -> Option<Element<'_, Message>> {
    if !state.is_open {
        return None;
    }

    let kind = state.kind.as_ref()?;

    // Build menu items based on kind
    let items: Vec<Element<'_, Message>> = match kind {
        ContextMenuKind::CollectionTrack { track_id, .. } => {
            vec![
                menu_item("Re-analyse", Message::ReanalyzeTrack(track_id.clone())),
                menu_separator(),
                menu_item_danger("Delete (Permanent)", Message::RequestDeleteById(track_id.clone())),
            ]
        }
        ContextMenuKind::PlaylistTrack { track_id, .. } => {
            vec![
                menu_item("Re-analyse", Message::ReanalyzeTrack(track_id.clone())),
                menu_separator(),
                menu_item("Remove from Playlist", Message::RequestDeleteById(track_id.clone())),
            ]
        }
        ContextMenuKind::Playlist { playlist_id, .. } => {
            vec![
                menu_item("Rename", Message::StartRenamePlaylist(playlist_id.clone())),
                menu_separator(),
                menu_item("Delete Playlist", Message::RequestDeletePlaylist(playlist_id.clone())),
            ]
        }
    };

    let menu = container(column(items).spacing(2).padding(4))
        .style(|theme: &iced::Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(Background::Color(palette.background.strong.color)),
                border: Border {
                    color: palette.background.weak.color,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        })
        .width(Length::Fixed(180.0));

    Some(menu.into())
}

/// Create a menu item button
fn menu_item<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .on_press(message)
        .width(Length::Fill)
        .padding([6, 12])
        .style(|theme: &iced::Theme, status| {
            let palette = theme.extended_palette();
            let bg = match status {
                button::Status::Hovered => palette.primary.weak.color,
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: palette.background.base.text,
                border: Border::default(),
                ..Default::default()
            }
        })
        .into()
}

/// Create a danger-styled menu item (for destructive actions)
fn menu_item_danger<'a>(label: &'a str, message: Message) -> Element<'a, Message> {
    button(text(label).size(13).color(iced::Color::from_rgb(0.9, 0.3, 0.3)))
        .on_press(message)
        .width(Length::Fill)
        .padding([6, 12])
        .style(|theme: &iced::Theme, status| {
            let palette = theme.extended_palette();
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(0.9, 0.3, 0.3, 0.2),
                _ => iced::Color::TRANSPARENT,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: iced::Color::from_rgb(0.9, 0.3, 0.3),
                border: Border::default(),
                ..Default::default()
            }
        })
        .into()
}

/// Create a separator line
fn menu_separator<'a>() -> Element<'a, Message> {
    container(iced::widget::rule::horizontal(1))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}
