//! Combined playlist browser widget (tree + table)
//!
//! A two-panel widget with a collapsible tree on the left for navigation
//! and a track table on the right for displaying folder contents.
//!
//! ## Usage
//!
//! ```ignore
//! let browser = playlist_browser(
//!     &tree_nodes,
//!     &tracks,
//!     &browser_state,
//!     |msg| Message::Browser(msg),
//! );
//! ```

use crate::track_table::{
    track_table, TrackColumn, TrackRow, TrackTableMessage, TrackTableState,
};
use crate::tree::{tree_view, TreeMessage, TreeNode, TreeState};
use iced::widget::{container, row, rule};
use iced::{Background, Border, Color, Element, Length, Theme};
use std::hash::Hash;

/// State for the combined playlist browser widget
#[derive(Debug, Clone)]
pub struct PlaylistBrowserState<NodeId, TrackId>
where
    NodeId: Clone + Eq + Hash,
    TrackId: Clone + Eq + Hash,
{
    /// State for the tree view
    pub tree_state: TreeState<NodeId>,
    /// State for the track table
    pub table_state: TrackTableState<TrackId>,
    /// Currently selected folder in the tree (whose contents are shown in table)
    pub current_folder: Option<NodeId>,
}

impl<NodeId, TrackId> Default for PlaylistBrowserState<NodeId, TrackId>
where
    NodeId: Clone + Eq + Hash,
    TrackId: Clone + Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<NodeId, TrackId> PlaylistBrowserState<NodeId, TrackId>
where
    NodeId: Clone + Eq + Hash,
    TrackId: Clone + Eq + Hash,
{
    /// Create a new browser state
    pub fn new() -> Self {
        Self {
            tree_state: TreeState::new(),
            table_state: TrackTableState::new(),
            current_folder: None,
        }
    }

    /// Set the current folder (usually called when a tree node is selected)
    pub fn set_current_folder(&mut self, folder: NodeId) {
        self.current_folder = Some(folder.clone());
        self.tree_state.select(folder);
        // Clear table selection when folder changes
        self.table_state.clear_selection();
        self.table_state.search_query.clear();
    }

    /// Handle a tree message, returning true if the folder changed
    pub fn handle_tree_message(&mut self, msg: &TreeMessage<NodeId>) -> bool
    where
        NodeId: Clone,
    {
        match msg {
            TreeMessage::Toggle(id) => {
                self.tree_state.toggle_expanded(id);
                false
            }
            TreeMessage::Select(id) => {
                let folder_changed = self.current_folder.as_ref() != Some(id);
                self.set_current_folder(id.clone());
                folder_changed
            }
            TreeMessage::EditChanged(text) => {
                self.tree_state.edit_buffer = text.clone();
                false
            }
            TreeMessage::MouseMoved(point) => {
                self.tree_state.set_mouse_position(*point);
                false
            }
            // These are passed through to app layer for storage operations
            TreeMessage::CreateChild(_)
            | TreeMessage::StartEdit(_)
            | TreeMessage::CommitEdit
            | TreeMessage::CancelEdit
            | TreeMessage::DropReceived(_)
            | TreeMessage::RightClick(_, _) => false,
        }
    }

    /// Handle a table message
    ///
    /// Returns `Some((track_id, column, new_value))` if an edit was committed,
    /// so the parent can save the metadata to the file.
    ///
    /// Note: `Select` messages do NOT perform selection here. The app's update()
    /// handler should apply modifiers (Shift/Ctrl) with current keyboard state
    /// and call `table_state.handle_select()` directly.
    pub fn handle_table_message(
        &mut self,
        msg: &TrackTableMessage<TrackId>,
    ) -> Option<(TrackId, TrackColumn, String)> {
        match msg {
            TrackTableMessage::SearchChanged(query) => {
                self.table_state.set_search(query.clone());
                None
            }
            TrackTableMessage::Select(_id) => {
                // Cancel any in-progress edit when selecting
                self.table_state.cancel_edit();
                // Selection with modifiers is handled by the app's update() handler
                // where current keyboard state is available
                None
            }
            TrackTableMessage::Activate(_) => {
                // Handled by parent (load track into editor)
                None
            }
            TrackTableMessage::SortBy(column) => {
                self.table_state.set_sort(*column);
                None
            }
            TrackTableMessage::StartEdit(id, column, current_value) => {
                self.table_state
                    .start_edit(id.clone(), *column, current_value.clone());
                None
            }
            TrackTableMessage::EditChanged(text) => {
                self.table_state.edit_buffer = text.clone();
                None
            }
            TrackTableMessage::CommitEdit => {
                // Return the edit data so parent can save to file
                self.table_state.commit_edit()
            }
            TrackTableMessage::CancelEdit => {
                self.table_state.cancel_edit();
                None
            }
            TrackTableMessage::DropReceived(_) | TrackTableMessage::DropReceivedOnTable => {
                // Drop handling is done by the app layer (needs access to drag state)
                None
            }
            TrackTableMessage::MouseMoved(point) => {
                self.table_state.set_mouse_position(*point);
                None
            }
            TrackTableMessage::RightClick(_, _) => {
                // Right-click handling is done by the app layer (context menu)
                None
            }
        }
    }
}

/// Messages emitted by the playlist browser widget
#[derive(Debug, Clone)]
pub enum PlaylistBrowserMessage<NodeId, TrackId> {
    /// Message from the tree view
    Tree(TreeMessage<NodeId>),
    /// Message from the track table
    Table(TrackTableMessage<TrackId>),
}

/// Width of the tree panel in pixels
pub const TREE_PANEL_WIDTH: f32 = 200.0;

/// Build a combined playlist browser (tree + table)
///
/// # Arguments
///
/// * `tree_nodes` - Root-level tree nodes (folders/playlists)
/// * `tracks` - Tracks in the currently selected folder
/// * `state` - Current browser state
/// * `on_message` - Callback to convert browser messages to your message type
///
/// Note: Modifier key handling (Shift/Ctrl for multi-select) is NOT done here.
/// The app's `update()` handler should apply modifiers when processing `Select` messages.
///
/// # Example
///
/// ```ignore
/// playlist_browser(
///     &tree_nodes,
///     &current_tracks,
///     &browser_state,
///     |msg| Message::LeftBrowser(msg),
/// )
/// ```
pub fn playlist_browser<'a, NodeId, TrackId, Message>(
    tree_nodes: &'a [TreeNode<NodeId>],
    tracks: &'a [TrackRow<TrackId>],
    state: &'a PlaylistBrowserState<NodeId, TrackId>,
    on_message: impl Fn(PlaylistBrowserMessage<NodeId, TrackId>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    NodeId: Clone + Eq + Hash + 'a,
    TrackId: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    playlist_browser_with_drop_highlight(tree_nodes, tracks, state, on_message, false)
}

/// Build a combined playlist browser with optional drop zone highlighting
///
/// Same as `playlist_browser`, but with an `is_drop_target` flag to highlight
/// the table area when tracks are being dragged over it.
pub fn playlist_browser_with_drop_highlight<'a, NodeId, TrackId, Message>(
    tree_nodes: &'a [TreeNode<NodeId>],
    tracks: &'a [TrackRow<TrackId>],
    state: &'a PlaylistBrowserState<NodeId, TrackId>,
    on_message: impl Fn(PlaylistBrowserMessage<NodeId, TrackId>) -> Message + 'a + Clone,
    is_drop_target: bool,
) -> Element<'a, Message>
where
    NodeId: Clone + Eq + Hash + 'a,
    TrackId: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    let on_msg_tree = on_message.clone();
    let tree = tree_view(tree_nodes, &state.tree_state, move |msg| {
        on_msg_tree(PlaylistBrowserMessage::Tree(msg))
    });

    let on_msg_table = on_message.clone();
    let table = track_table(tracks, &state.table_state, move |msg| {
        on_msg_table(PlaylistBrowserMessage::Table(msg))
    });

    let tree_container = container(tree)
        .width(Length::Fixed(TREE_PANEL_WIDTH))
        .height(Length::Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(
                theme.extended_palette().background.base.color,
            )),
            ..Default::default()
        });

    let table_container = container(table)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme: &Theme| {
            let bg = theme.extended_palette().background.base.color;
            if is_drop_target {
                // Highlight with a subtle teal/cyan border when dragging
                container::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color: Color::from_rgb(0.3, 0.6, 0.7),
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            } else {
                container::Style {
                    background: Some(Background::Color(bg)),
                    ..Default::default()
                }
            }
        });

    row![tree_container, rule::vertical(1), table_container,]
        .spacing(0)
        .height(Length::Fill)
        .into()
}

/// Build a minimal tree-only browser (for compact layouts)
pub fn tree_browser<'a, NodeId, Message>(
    tree_nodes: &'a [TreeNode<NodeId>],
    state: &'a TreeState<NodeId>,
    on_message: impl Fn(TreeMessage<NodeId>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    NodeId: Clone + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    container(tree_view(tree_nodes, state, on_message))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(
                theme.extended_palette().background.base.color,
            )),
            ..Default::default()
        })
        .into()
}

/// Build a minimal table-only browser (for compact layouts)
pub fn table_browser<'a, TrackId, Message>(
    tracks: &'a [TrackRow<TrackId>],
    state: &'a TrackTableState<TrackId>,
    on_message: impl Fn(TrackTableMessage<TrackId>) -> Message + 'a + Clone,
) -> Element<'a, Message>
where
    TrackId: Clone + PartialEq + Eq + Hash + 'a,
    Message: Clone + 'a,
{
    container(track_table(tracks, state, on_message))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(
                theme.extended_palette().background.base.color,
            )),
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_state() {
        let mut state: PlaylistBrowserState<String, String> = PlaylistBrowserState::new();

        // Set folder
        state.set_current_folder("playlists/my-set".to_string());
        assert_eq!(
            state.current_folder,
            Some("playlists/my-set".to_string())
        );
        assert!(state.tree_state.is_selected(&"playlists/my-set".to_string()));

        // Handle tree toggle
        let msg = TreeMessage::Toggle("playlists".to_string());
        let changed = state.handle_tree_message(&msg);
        assert!(!changed); // Toggle doesn't change folder
        assert!(state.tree_state.is_expanded(&"playlists".to_string()));

        // Handle tree select
        let msg = TreeMessage::Select("playlists/other".to_string());
        let changed = state.handle_tree_message(&msg);
        assert!(changed); // Select changes folder
        assert_eq!(state.current_folder, Some("playlists/other".to_string()));
    }
}
