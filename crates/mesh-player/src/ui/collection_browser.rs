//! Read-only collection browser for mesh-player
//!
//! A simplified playlist browser that allows:
//! - Navigating the collection tree
//! - Searching and sorting tracks
//! - Loading tracks to one of 4 decks
//!
//! Unlike mesh-cue's browser, this is READ-ONLY:
//! - No playlist creation/rename/delete
//! - No drag-drop between playlists
//! - No inline metadata editing

use iced::widget::{button, column, container, row, text};
use iced::{Element, Length};
use mesh_core::playlist::{FilesystemStorage, NodeId, NodeKind, PlaylistNode, PlaylistStorage};
use mesh_widgets::{
    playlist_browser, PlaylistBrowserMessage, PlaylistBrowserState, TrackRow,
    TrackTableMessage, TreeIcon, TreeMessage, TreeNode,
};
use std::path::PathBuf;

/// State for the collection browser
pub struct CollectionBrowserState {
    /// Playlist storage backend (shared with mesh-cue)
    pub storage: Option<Box<FilesystemStorage>>,
    /// Browser widget state (single browser, not dual)
    pub browser: PlaylistBrowserState<NodeId, NodeId>,
    /// Cached tree nodes for display
    pub tree_nodes: Vec<TreeNode<NodeId>>,
    /// Cached tracks for current folder
    pub tracks: Vec<TrackRow<NodeId>>,
    /// Currently selected track path (for deck load buttons)
    selected_track_path: Option<PathBuf>,
}

/// Messages from the collection browser
#[derive(Debug, Clone)]
pub enum CollectionBrowserMessage {
    /// Internal browser message (filtered for read-only)
    Browser(PlaylistBrowserMessage<NodeId, NodeId>),
    /// Load selected track to a specific deck
    LoadToDeck(usize),
    /// Refresh collection from disk
    Refresh,
    /// Scroll selection by delta (positive = down, negative = up)
    ScrollBy(i32),
    /// Select current item (enter folder or activate track)
    SelectCurrent,
}

impl CollectionBrowserState {
    /// Create new state, initializing storage at collection path
    pub fn new(collection_path: PathBuf) -> Self {
        let storage = match FilesystemStorage::new(collection_path) {
            Ok(s) => Some(Box::new(s)),
            Err(e) => {
                log::warn!("Failed to initialize collection storage: {}", e);
                None
            }
        };

        let tree_nodes = storage
            .as_ref()
            .map(|s| build_tree_nodes(s))
            .unwrap_or_default();

        Self {
            storage,
            browser: PlaylistBrowserState::new(),
            tree_nodes,
            tracks: Vec::new(),
            selected_track_path: None,
        }
    }

    /// Handle a browser message (filters out write operations)
    /// Returns Some((deck_idx, path)) if a track should be loaded
    pub fn handle_message(&mut self, msg: CollectionBrowserMessage) -> Option<(usize, PathBuf)> {
        match msg {
            CollectionBrowserMessage::Browser(browser_msg) => {
                match browser_msg {
                    PlaylistBrowserMessage::Tree(ref tree_msg) => {
                        // Only handle read-only tree operations
                        match tree_msg {
                            TreeMessage::Toggle(_) | TreeMessage::Select(_) => {
                                let folder_changed = self.browser.handle_tree_message(tree_msg);
                                if folder_changed {
                                    if let Some(ref folder) = self.browser.current_folder {
                                        if let Some(ref storage) = self.storage {
                                            self.tracks = get_tracks_for_folder(storage, folder);
                                        }
                                    }
                                    // Clear track selection when folder changes
                                    self.selected_track_path = None;
                                }
                            }
                            // Ignore all write operations and context menu
                            TreeMessage::CreateChild(_)
                            | TreeMessage::StartEdit(_)
                            | TreeMessage::EditChanged(_)
                            | TreeMessage::CommitEdit
                            | TreeMessage::CancelEdit
                            | TreeMessage::DropReceived(_)
                            | TreeMessage::RightClick(_, _)
                            | TreeMessage::MouseMoved(_) => {
                                // Silently ignore write operations and context menu
                            }
                        }
                    }
                    PlaylistBrowserMessage::Table(ref table_msg) => {
                        // Only handle read-only table operations
                        match table_msg {
                            TrackTableMessage::SearchChanged(_) | TrackTableMessage::SortBy(_) => {
                                let _ = self.browser.handle_table_message(table_msg);
                            }
                            TrackTableMessage::Select(track_id) => {
                                // mesh-player uses simple single-selection (no Shift/Ctrl)
                                self.browser.table_state.select(track_id.clone());
                                // Update selected track path for load buttons
                                self.selected_track_path = self.get_track_path(track_id);
                            }
                            TrackTableMessage::Activate(track_id) => {
                                // Double-click loads to Deck 1
                                if let Some(path) = self.get_track_path(track_id) {
                                    return Some((0, path));
                                }
                            }
                            // Ignore all edit, drop, and context menu operations (mesh-player is read-only)
                            TrackTableMessage::StartEdit(_, _, _)
                            | TrackTableMessage::EditChanged(_)
                            | TrackTableMessage::CommitEdit
                            | TrackTableMessage::CancelEdit
                            | TrackTableMessage::DropReceived(_)
                            | TrackTableMessage::RightClick(_, _)
                            | TrackTableMessage::MouseMoved(_) => {
                                // Silently ignore edit, drop, and context menu operations
                            }
                        }
                    }
                }
                None
            }
            CollectionBrowserMessage::LoadToDeck(deck_idx) => {
                self.selected_track_path
                    .clone()
                    .map(|path| (deck_idx, path))
            }
            CollectionBrowserMessage::Refresh => {
                if let Some(ref mut storage) = self.storage {
                    let _ = storage.refresh();
                    self.tree_nodes = build_tree_nodes(storage);
                    if let Some(ref folder) = self.browser.current_folder {
                        self.tracks = get_tracks_for_folder(storage, folder);
                    }
                }
                None
            }
            CollectionBrowserMessage::ScrollBy(delta) => {
                // If no folder selected or tracks are empty, scroll through folders (tree)
                if self.tracks.is_empty() {
                    self.scroll_tree(delta);
                    return None;
                }

                // Otherwise, scroll through tracks in the current folder
                // Find current selection index
                let current_idx = self
                    .browser
                    .table_state
                    .selected
                    .iter()
                    .next()
                    .and_then(|selected| {
                        self.tracks
                            .iter()
                            .position(|t| &t.id == selected)
                    })
                    .unwrap_or(0);

                // Calculate new index with wrapping
                let new_idx = if delta > 0 {
                    (current_idx + delta as usize).min(self.tracks.len() - 1)
                } else {
                    current_idx.saturating_sub((-delta) as usize)
                };

                // Select the new track
                if let Some(track) = self.tracks.get(new_idx) {
                    self.browser.table_state.select(track.id.clone());
                    self.selected_track_path = self.get_track_path(&track.id);
                }
                None
            }
            CollectionBrowserMessage::SelectCurrent => {
                // If there's a selected track, activate it (load to deck 0)
                if let Some(path) = self.selected_track_path.clone() {
                    return Some((0, path));
                }
                // If no track selected but we have a selected folder in tree, enter it
                if let Some(ref folder_id) = self.browser.tree_state.selected.clone() {
                    // Expand the folder in the tree view
                    self.browser.tree_state.expanded.insert(folder_id.clone());
                    // Set as current folder and load its tracks
                    self.browser.current_folder = Some(folder_id.clone());
                    if let Some(ref storage) = self.storage {
                        self.tracks = get_tracks_for_folder(storage, folder_id);
                    }
                    // Select first track if any
                    if let Some(first_track) = self.tracks.first() {
                        self.browser.table_state.select(first_track.id.clone());
                        self.selected_track_path = self.get_track_path(&first_track.id);
                    }
                }
                None
            }
        }
    }

    /// Get track path by ID from storage
    fn get_track_path(&self, track_id: &NodeId) -> Option<PathBuf> {
        self.storage
            .as_ref()
            .and_then(|s| s.get_node(track_id))
            .and_then(|node| node.track_path)
    }

    /// Get the currently selected track path (for MIDI load functionality)
    pub fn get_selected_track_path(&self) -> Option<&PathBuf> {
        self.selected_track_path.as_ref()
    }

    /// Get the currently selected track index (for auto-scroll)
    /// Returns None if no track is selected or tracks list is empty
    pub fn get_selected_index(&self) -> Option<usize> {
        self.browser
            .table_state
            .selected
            .iter()
            .next()
            .and_then(|selected| self.tracks.iter().position(|t| &t.id == selected))
    }

    /// Get total track count (for scroll calculations)
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Build the view with deck load buttons at top (centered)
    pub fn view(&self) -> Element<CollectionBrowserMessage> {
        // mesh-player uses simple single-selection (no Shift/Ctrl modifier tracking)
        let browser_element = playlist_browser(
            &self.tree_nodes,
            &self.tracks,
            &self.browser,
            CollectionBrowserMessage::Browser,
        );

        // Show deck load buttons (centered row at top)
        let load_buttons: Element<CollectionBrowserMessage> = if self.selected_track_path.is_some()
        {
            row![
                button(text("1").size(12))
                    .on_press(CollectionBrowserMessage::LoadToDeck(0))
                    .padding([6, 16]),
                button(text("2").size(12))
                    .on_press(CollectionBrowserMessage::LoadToDeck(1))
                    .padding([6, 16]),
                button(text("3").size(12))
                    .on_press(CollectionBrowserMessage::LoadToDeck(2))
                    .padding([6, 16]),
                button(text("4").size(12))
                    .on_press(CollectionBrowserMessage::LoadToDeck(3))
                    .padding([6, 16]),
            ]
            .spacing(8)
            .into()
        } else {
            row![text("Select a track to load").size(11),].into()
        };

        let load_bar = container(load_buttons)
            .padding([6, 10])
            .center_x(Length::Fill);

        column![load_bar, browser_element]
            .spacing(0)
            .height(Length::Fill)
            .into()
    }

    /// Compact view without load buttons (for performance mode)
    pub fn view_compact(&self) -> Element<CollectionBrowserMessage> {
        playlist_browser(
            &self.tree_nodes,
            &self.tracks,
            &self.browser,
            CollectionBrowserMessage::Browser,
        )
    }

    /// Scroll through tree nodes (folders) when not viewing tracks
    fn scroll_tree(&mut self, delta: i32) {
        // Build flat list of visible tree nodes
        let visible_nodes = self.get_visible_tree_nodes();
        if visible_nodes.is_empty() {
            return;
        }

        // Find current selection index
        let current_idx = self
            .browser
            .tree_state
            .selected
            .as_ref()
            .and_then(|selected| visible_nodes.iter().position(|id| id == selected))
            .unwrap_or(0);

        // Calculate new index with clamping
        let new_idx = if delta > 0 {
            (current_idx + delta as usize).min(visible_nodes.len() - 1)
        } else {
            current_idx.saturating_sub((-delta) as usize)
        };

        // Select the new node
        if let Some(node_id) = visible_nodes.get(new_idx) {
            self.browser.tree_state.selected = Some(node_id.clone());
        }
    }

    /// Get flat list of visible tree node IDs (respecting expansion state)
    fn get_visible_tree_nodes(&self) -> Vec<NodeId> {
        let mut visible = Vec::new();
        self.collect_visible_nodes(&self.tree_nodes, &mut visible);
        visible
    }

    /// Recursively collect visible node IDs
    fn collect_visible_nodes(&self, nodes: &[TreeNode<NodeId>], visible: &mut Vec<NodeId>) {
        for node in nodes {
            visible.push(node.id.clone());
            // Only include children if this node is expanded
            if self.browser.tree_state.expanded.contains(&node.id) {
                self.collect_visible_nodes(&node.children, visible);
            }
        }
    }

}

/// Build tree nodes from storage (read-only: no create/rename allowed)
fn build_tree_nodes(storage: &FilesystemStorage) -> Vec<TreeNode<NodeId>> {
    let root = storage.root();
    build_node_children(storage, &root)
}

/// Recursively build tree node children (read-only version)
fn build_node_children(storage: &FilesystemStorage, parent: &PlaylistNode) -> Vec<TreeNode<NodeId>> {
    storage
        .get_children(&parent.id)
        .into_iter()
        .filter(|node| node.kind != NodeKind::Track) // Only folders in tree
        .map(|node| {
            let icon = match node.kind {
                NodeKind::Collection => TreeIcon::Collection,
                NodeKind::CollectionFolder => TreeIcon::Folder,
                NodeKind::PlaylistsRoot => TreeIcon::Folder,
                NodeKind::Playlist => TreeIcon::Playlist,
                _ => TreeIcon::Folder,
            };

            // READ-ONLY: Never allow create or rename
            TreeNode::with_children(
                node.id.clone(),
                node.name.clone(),
                icon,
                build_node_children(storage, &node),
            )
            .with_create_child(false)
            .with_rename(false)
        })
        .collect()
}

/// Get tracks for a folder as TrackRow items for display
fn get_tracks_for_folder(storage: &FilesystemStorage, folder_id: &NodeId) -> Vec<TrackRow<NodeId>> {
    storage
        .get_tracks(folder_id)
        .into_iter()
        .map(|info| {
            let mut row = TrackRow::new(info.id, info.name);
            if let Some(artist) = info.artist {
                row = row.with_artist(artist);
            }
            if let Some(bpm) = info.bpm {
                row = row.with_bpm(bpm);
            }
            if let Some(key) = info.key {
                row = row.with_key(key);
            }
            if let Some(duration) = info.duration {
                row = row.with_duration(duration);
            }
            row
        })
        .collect()
}
