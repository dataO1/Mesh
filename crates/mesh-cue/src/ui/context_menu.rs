//! Context menu component for right-click actions
//!
//! Provides popup menus for tracks and playlists with appropriate actions.

use super::app::Message;
use crate::analysis::{AnalysisType, ReanalysisScope};
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
        /// Other selected tracks (for batch operations)
        selected_tracks: Vec<NodeId>,
    },
    /// Context menu for a track in a playlist (non-destructive)
    PlaylistTrack {
        track_id: NodeId,
        track_name: String,
        /// Other selected tracks (for batch operations)
        selected_tracks: Vec<NodeId>,
    },
    /// Context menu for a playlist folder
    Playlist {
        playlist_id: NodeId,
        playlist_name: String,
    },
    /// Context menu for the entire collection
    Collection,
}

impl ContextMenuKind {
    /// Get the ID of the item (None for Collection)
    pub fn id(&self) -> Option<&NodeId> {
        match self {
            ContextMenuKind::CollectionTrack { track_id, .. } => Some(track_id),
            ContextMenuKind::PlaylistTrack { track_id, .. } => Some(track_id),
            ContextMenuKind::Playlist { playlist_id, .. } => Some(playlist_id),
            ContextMenuKind::Collection => None,
        }
    }

    /// Get the display name
    pub fn name(&self) -> &str {
        match self {
            ContextMenuKind::CollectionTrack { track_name, .. } => track_name,
            ContextMenuKind::PlaylistTrack { track_name, .. } => track_name,
            ContextMenuKind::Playlist { playlist_name, .. } => playlist_name,
            ContextMenuKind::Collection => "Collection",
        }
    }

    /// Get the list of track IDs to operate on (for batch re-analysis)
    ///
    /// Returns the right-clicked track plus any selected tracks (deduplicated).
    pub fn track_ids(&self) -> Vec<NodeId> {
        match self {
            ContextMenuKind::CollectionTrack { track_id, selected_tracks, .. }
            | ContextMenuKind::PlaylistTrack { track_id, selected_tracks, .. } => {
                let mut ids = vec![track_id.clone()];
                for id in selected_tracks {
                    if id != track_id && !ids.contains(id) {
                        ids.push(id.clone());
                    }
                }
                ids
            }
            _ => Vec::new(),
        }
    }

    /// Returns true if this is a batch operation (multiple tracks selected)
    pub fn is_batch(&self) -> bool {
        match self {
            ContextMenuKind::CollectionTrack { selected_tracks, .. }
            | ContextMenuKind::PlaylistTrack { selected_tracks, .. } => !selected_tracks.is_empty(),
            ContextMenuKind::Playlist { .. } => true,
            ContextMenuKind::Collection => true,
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
pub fn view(state: &ContextMenuState) -> Option<Element<'static, Message>> {
    if !state.is_open {
        return None;
    }

    let kind = state.kind.as_ref()?;

    // Build menu items based on kind
    let items: Vec<Element<'static, Message>> = match kind {
        ContextMenuKind::CollectionTrack { track_id, selected_tracks, .. } => {
            // Determine scope based on selection
            let scope = if selected_tracks.is_empty() {
                ReanalysisScope::SingleTrack(track_id.clone())
            } else {
                let mut ids = vec![track_id.clone()];
                ids.extend(selected_tracks.iter().filter(|id| *id != track_id).cloned());
                ReanalysisScope::SelectedTracks(ids)
            };
            let scope_label = if selected_tracks.is_empty() { "" } else { " (Selected)" };

            vec![
                menu_item(
                    &format!("Re-analyse Loudness{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Loudness,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    &format!("Re-analyse BPM{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Bpm,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    &format!("Re-analyse Key{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Key,
                        scope: scope.clone(),
                    },
                ),
                menu_separator(),
                menu_item(
                    &format!("Re-analyse All{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::All,
                        scope,
                    },
                ),
                menu_separator(),
                menu_item_danger("Delete (Permanent)", Message::RequestDeleteById(track_id.clone())),
            ]
        }
        ContextMenuKind::PlaylistTrack { track_id, selected_tracks, .. } => {
            // Determine scope based on selection
            let scope = if selected_tracks.is_empty() {
                ReanalysisScope::SingleTrack(track_id.clone())
            } else {
                let mut ids = vec![track_id.clone()];
                ids.extend(selected_tracks.iter().filter(|id| *id != track_id).cloned());
                ReanalysisScope::SelectedTracks(ids)
            };
            let scope_label = if selected_tracks.is_empty() { "" } else { " (Selected)" };

            vec![
                menu_item(
                    &format!("Re-analyse Loudness{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Loudness,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    &format!("Re-analyse BPM{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Bpm,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    &format!("Re-analyse Key{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Key,
                        scope: scope.clone(),
                    },
                ),
                menu_separator(),
                menu_item(
                    &format!("Re-analyse All{}", scope_label),
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::All,
                        scope,
                    },
                ),
                menu_separator(),
                menu_item("Remove from Playlist", Message::RequestDeleteById(track_id.clone())),
            ]
        }
        ContextMenuKind::Playlist { playlist_id, .. } => {
            let scope = ReanalysisScope::PlaylistFolder(playlist_id.clone());
            vec![
                menu_item(
                    "Re-analyse Loudness (Playlist)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Loudness,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    "Re-analyse BPM (Playlist)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Bpm,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    "Re-analyse Key (Playlist)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Key,
                        scope: scope.clone(),
                    },
                ),
                menu_separator(),
                menu_item(
                    "Re-analyse All (Playlist)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::All,
                        scope,
                    },
                ),
                menu_separator(),
                menu_item("Rename", Message::StartRenamePlaylist(playlist_id.clone())),
                menu_item("Delete Playlist", Message::RequestDeletePlaylist(playlist_id.clone())),
            ]
        }
        ContextMenuKind::Collection => {
            let scope = ReanalysisScope::EntireCollection;
            vec![
                menu_item(
                    "Re-analyse Loudness (All)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Loudness,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    "Re-analyse BPM (All)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Bpm,
                        scope: scope.clone(),
                    },
                ),
                menu_item(
                    "Re-analyse Key (All)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::Key,
                        scope: scope.clone(),
                    },
                ),
                menu_separator(),
                menu_item(
                    "Re-analyse All (Collection)",
                    Message::StartReanalysis {
                        analysis_type: AnalysisType::All,
                        scope,
                    },
                ),
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
fn menu_item(label: impl ToString, message: Message) -> Element<'static, Message> {
    button(text(label.to_string()).size(13))
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
fn menu_item_danger(label: impl ToString, message: Message) -> Element<'static, Message> {
    button(text(label.to_string()).size(13).color(iced::Color::from_rgb(0.9, 0.3, 0.3)))
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
fn menu_separator() -> Element<'static, Message> {
    container(iced::widget::rule::horizontal(1))
        .padding([4, 8])
        .width(Length::Fill)
        .into()
}
