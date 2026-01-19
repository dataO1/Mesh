//! Browser message handlers
//!
//! Handles: BrowserLeft, BrowserRight, RefreshPlaylists, DragTrackStart, DragTrackEnd, DropTracksOnPlaylist
//!
//! Uses parameterized handlers to avoid duplication between left and right browsers.

use iced::Task;
use mesh_core::playlist::{NodeId, NodeKind};
use mesh_widgets::{PlaylistBrowserMessage, TrackTableMessage, TrackColumn, sort_tracks, TreeMessage};
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::{BrowserSide, CollectionState};

impl MeshCueApp {
    /// Handle BrowserLeft message - delegates to parameterized handler
    pub fn handle_browser_left(&mut self, browser_msg: PlaylistBrowserMessage<NodeId, NodeId>) -> Task<Message> {
        self.handle_browser(BrowserSide::Left, browser_msg)
    }

    /// Handle BrowserRight message - delegates to parameterized handler
    pub fn handle_browser_right(&mut self, browser_msg: PlaylistBrowserMessage<NodeId, NodeId>) -> Task<Message> {
        self.handle_browser(BrowserSide::Right, browser_msg)
    }

    /// Parameterized browser message handler
    ///
    /// Handles tree and table messages for either browser side,
    /// eliminating duplication between left and right handlers.
    fn handle_browser(&mut self, side: BrowserSide, browser_msg: PlaylistBrowserMessage<NodeId, NodeId>) -> Task<Message> {
        let side_name = CollectionState::side_name(side);

        match browser_msg {
            PlaylistBrowserMessage::Tree(ref tree_msg) => {
                match tree_msg {
                    TreeMessage::CreateChild(parent_id) => {
                        match self.domain.create_playlist_with_node(parent_id, "New Playlist") {
                            Ok(new_id) => {
                                log::info!("Created playlist: {:?}", new_id);
                                self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                                self.collection.browser_mut(side).tree_state.start_edit(
                                    new_id,
                                    "New Playlist".to_string(),
                                );
                            }
                            Err(e) => log::error!("Failed to create playlist: {:?}", e),
                        }
                    }
                    TreeMessage::StartEdit(id) => {
                        if let Some(node) = self.domain.get_node(id) {
                            self.collection.browser_mut(side).tree_state.start_edit(
                                id.clone(),
                                node.name.clone(),
                            );
                        }
                    }
                    TreeMessage::CommitEdit => {
                        if let Some((id, new_name)) = self.collection.browser_mut(side).tree_state.commit_edit() {
                            if let Err(e) = self.domain.rename_playlist_by_node(&id, &new_name) {
                                log::error!("Failed to rename playlist: {:?}", e);
                            }
                            self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                        }
                    }
                    TreeMessage::CancelEdit => {
                        self.collection.browser_mut(side).tree_state.cancel_edit();
                    }
                    TreeMessage::DropReceived(target_id) => {
                        log::debug!("{} tree: DropReceived on {:?}", side_name, target_id);
                        if let Some(ref drag) = self.collection.dragging_track {
                            log::debug!("  Currently dragging: {}", drag.display_text());
                            if let Some(target_node) = self.domain.get_node(target_id) {
                                log::debug!("  Target node kind: {:?}", target_node.kind);
                                if target_node.kind == NodeKind::Playlist
                                    || target_node.kind == NodeKind::PlaylistsRoot
                                {
                                    log::info!(
                                        "Drop on {} tree: {} -> {:?}",
                                        side_name,
                                        drag.display_text(),
                                        target_id
                                    );
                                    return self.update(Message::DropTracksOnPlaylist {
                                        track_ids: drag.track_ids.clone(),
                                        target_playlist: target_id.clone(),
                                    });
                                }
                            }
                        }
                        return self.update(Message::DragTrackEnd);
                    }
                    TreeMessage::RightClick(id, _widget_position) => {
                        let position = self.global_mouse_position;
                        log::info!("[{} TREE] RightClick received: id={:?}, global_position={:?}", side_name, id, position);
                        if let Some(node) = self.domain.get_node(id) {
                            log::info!("[{} TREE] Node found: kind={:?}, name={}", side_name, node.kind, node.name);
                            let menu_kind = if node.kind == NodeKind::Collection {
                                super::super::context_menu::ContextMenuKind::Collection
                            } else {
                                super::super::context_menu::ContextMenuKind::Playlist {
                                    playlist_id: id.clone(),
                                    playlist_name: node.name.clone(),
                                }
                            };
                            log::info!("[{} TREE] Showing context menu: {:?}", side_name, menu_kind);
                            return self.update(Message::ShowContextMenu(menu_kind, position));
                        } else {
                            log::warn!("[{} TREE] Node not found in storage: {:?}", side_name, id);
                        }
                    }
                    TreeMessage::MouseMoved(_) => {
                        // Widget-relative position, not used
                    }
                    _ => {
                        let folder_changed = self.collection.browser_mut(side).handle_tree_message(tree_msg);
                        if folder_changed {
                            let current_folder = self.collection.browser(side).current_folder.clone();
                            log::debug!("{} browser folder changed to {:?}", side_name, current_folder);
                            if let Some(ref folder) = current_folder {
                                let tracks = self.domain.get_tracks_for_display(folder);
                                *self.collection.tracks_mut(side) = tracks;
                            } else {
                                self.collection.tracks_mut(side).clear();
                            }
                        }
                    }
                }
            }
            PlaylistBrowserMessage::Table(table_msg) => {
                return self.handle_browser_table(side, table_msg);
            }
        }
        Task::none()
    }

    /// Parameterized browser table message handler
    fn handle_browser_table(&mut self, side: BrowserSide, table_msg: TrackTableMessage<NodeId>) -> Task<Message> {
        let side_name = CollectionState::side_name(side);

        // Handle selection with CURRENT modifier state
        if let TrackTableMessage::Select(ref track_id) = table_msg {
            let modifiers = mesh_widgets::SelectModifiers {
                shift: self.shift_held,
                ctrl: self.ctrl_held,
            };
            let already_selected = self.collection.browser(side).table_state.is_selected(track_id);
            log::info!(
                "[{} SELECT] track_id={:?}, shift={}, ctrl={}, already_selected={}, current_selection={}",
                side_name, track_id, self.shift_held, self.ctrl_held, already_selected,
                self.collection.browser(side).table_state.selected.len()
            );

            if already_selected && !modifiers.shift && !modifiers.ctrl {
                log::info!("[{} SELECT] preserving multi-selection for drag", side_name);
            } else {
                let all_ids: Vec<NodeId> = self.collection.tracks(side).iter().map(|t| t.id.clone()).collect();
                self.collection.browser_mut(side).table_state.handle_select(track_id.clone(), modifiers, &all_ids);
                log::info!(
                    "[{} SELECT] after handle_select: selected={}",
                    side_name,
                    self.collection.browser(side).table_state.selected.len()
                );
            }
        }

        // Handle table message and check for edit commits
        if let Some((track_id, column, new_value)) =
            self.collection.browser_mut(side).handle_table_message(&table_msg)
        {
            if let Some(node) = self.domain.get_node(&track_id) {
                if let Some(ref path) = node.track_path {
                    let db_field = match column {
                        TrackColumn::Artist => "artist",
                        TrackColumn::Bpm => "bpm",
                        TrackColumn::Key => "key",
                        _ => return Task::none(),
                    };
                    let track_path = path.to_string_lossy();
                    match self.domain.update_track_field(&track_path, db_field, &new_value) {
                        Ok(_) => {
                            log::info!("Saved {:?} = '{}' to database for {:?}", column, new_value, track_id);
                            let current_folder = self.collection.browser(side).current_folder.clone();
                            if let Some(ref folder) = current_folder {
                                let tracks = self.domain.get_tracks_for_display(folder);
                                *self.collection.tracks_mut(side) = tracks;
                            }
                        }
                        Err(e) => log::error!("Failed to update track field: {:?}", e),
                    }
                }
            }
        }

        // Handle drag initiation
        if let TrackTableMessage::Select(_) = &table_msg {
            let selected_ids: Vec<NodeId> = self.collection.browser(side).table_state.selected.iter().cloned().collect();
            if !selected_ids.is_empty() {
                let track_names: Vec<String> = selected_ids.iter()
                    .filter_map(|id| self.domain.get_node(id).map(|n| n.name.clone()))
                    .collect();
                log::debug!("{} table: initiating drag for {} track(s)", side_name, selected_ids.len());
                return self.update(Message::DragTrackStart {
                    track_ids: selected_ids,
                    track_names,
                    browser: side,
                });
            }
        }

        // Handle double-click to load track
        if let TrackTableMessage::Activate(ref track_id) = table_msg {
            log::info!("{} browser: Track activated (double-click): {:?}", side_name, track_id);
            if let Some(node) = self.domain.get_node(track_id) {
                if let Some(path) = &node.track_path {
                    return self.update(Message::LoadTrackByPath(path.clone()));
                }
            }
        }

        // Handle drop on table
        if let TrackTableMessage::DropReceived(_) = table_msg {
            log::debug!("{} table: DropReceived", side_name);
            if let Some(ref drag) = self.collection.dragging_track {
                let current_folder = self.collection.browser(side).current_folder.clone();
                if let Some(ref folder) = current_folder {
                    if let Some(folder_node) = self.domain.get_node(folder) {
                        if folder_node.kind == NodeKind::Playlist || folder_node.kind == NodeKind::PlaylistsRoot {
                            log::info!("Drop on {} table: {} -> {:?}", side_name, drag.display_text(), folder);
                            return self.update(Message::DropTracksOnPlaylist {
                                track_ids: drag.track_ids.clone(),
                                target_playlist: folder.clone(),
                            });
                        }
                    }
                }
            }
            return self.update(Message::DragTrackEnd);
        }

        // Handle right-click on track
        if let TrackTableMessage::RightClick(ref track_id, _) = table_msg {
            let position = self.global_mouse_position;
            log::info!("[{} TABLE] RightClick received: track_id={:?}, global_position={:?}", side_name, track_id, position);
            if let Some(node) = self.domain.get_node(track_id) {
                log::info!("[{} TABLE] Track found: name={}", side_name, node.name);
                let current_folder = self.collection.browser(side).current_folder.clone();
                let is_playlist_view = current_folder.as_ref()
                    .and_then(|f| self.domain.get_node(f))
                    .map(|n| n.kind == NodeKind::Playlist || n.kind == NodeKind::PlaylistsRoot)
                    .unwrap_or(false);

                let selected_tracks: Vec<NodeId> = self.collection.browser(side).table_state.selected.iter()
                    .filter(|id| *id != track_id)
                    .cloned()
                    .collect();

                let menu_kind = if is_playlist_view {
                    super::super::context_menu::ContextMenuKind::PlaylistTrack {
                        track_id: track_id.clone(),
                        track_name: node.name.clone(),
                        selected_tracks,
                    }
                } else {
                    super::super::context_menu::ContextMenuKind::CollectionTrack {
                        track_id: track_id.clone(),
                        track_name: node.name.clone(),
                        selected_tracks,
                    }
                };
                log::info!("[{} TABLE] Showing context menu: is_playlist={}", side_name, is_playlist_view);
                return self.update(Message::ShowContextMenu(menu_kind, position));
            }
        }

        // Sort tracks
        if let TrackTableMessage::SortBy(_) = &table_msg {
            let state = &self.collection.browser(side).table_state;
            let sort_col = state.sort_column;
            let sort_asc = state.sort_ascending;
            sort_tracks(self.collection.tracks_mut(side), sort_col, sort_asc);
        }

        Task::none()
    }

    /// Handle RefreshPlaylists message
    pub fn handle_refresh_playlists(&mut self) -> Task<Message> {
        // Database queries are always fresh - just rebuild the UI views
        self.domain.refresh_tree();
        self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
        // Refresh track lists for both browsers
        if let Some(ref folder) = self.collection.browser_left.current_folder {
            self.collection.left_tracks = self.domain.get_tracks_for_display(folder);
        }
        if let Some(ref folder) = self.collection.browser_right.current_folder {
            self.collection.right_tracks = self.domain.get_tracks_for_display(folder);
        }
        Task::none()
    }

    /// Handle DragTrackStart message
    pub fn handle_drag_track_start(&mut self, track_ids: Vec<NodeId>, track_names: Vec<String>, browser: BrowserSide) -> Task<Message> {
        use super::super::state::DragState;
        self.collection.dragging_track = Some(DragState {
            track_ids,
            track_names,
            source_browser: browser,
        });
        Task::none()
    }

    /// Handle DragTrackEnd message
    pub fn handle_drag_track_end(&mut self) -> Task<Message> {
        self.collection.dragging_track = None;
        Task::none()
    }

    /// Handle DropTracksOnPlaylist message
    pub fn handle_drop_tracks_on_playlist(&mut self, track_ids: Vec<NodeId>, target_playlist: NodeId) -> Task<Message> {
        log::info!(
            "Drop {} track(s) onto playlist {:?}",
            track_ids.len(),
            target_playlist
        );
        // Use domain's add_tracks_to_playlist for batch adding
        match self.domain.add_tracks_to_playlist(&target_playlist, &track_ids) {
            Ok(success_count) => {
                if success_count > 0 {
                    log::info!("Added {}/{} tracks successfully", success_count, track_ids.len());
                    // Refresh tree and both browser track lists
                    self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                    if let Some(ref folder) = self.collection.browser_left.current_folder {
                        self.collection.left_tracks = self.domain.get_tracks_for_display(folder);
                    }
                    if let Some(ref folder) = self.collection.browser_right.current_folder {
                        self.collection.right_tracks = self.domain.get_tracks_for_display(folder);
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to add tracks to playlist: {:?}", e);
            }
        }
        // End drag
        self.collection.dragging_track = None;
        Task::none()
    }
}
