//! Delete confirmation message handlers
//!
//! Handles: RequestDelete, CancelDelete, ConfirmDelete, RequestDeleteById, RequestDeletePlaylist

use iced::Task;
use mesh_core::playlist::NodeId;
use super::super::app::MeshCueApp;
use super::super::delete_modal::DeleteTarget;
use super::super::message::Message;
use super::super::state::BrowserSide;

impl MeshCueApp {
    /// Handle RequestDelete message
    pub fn handle_request_delete(&mut self, browser_side: BrowserSide) -> Task<Message> {
        // Get selected tracks from the appropriate browser
        let (selected_ids, current_folder) = match browser_side {
            BrowserSide::Left => (
                self.collection.browser_left.table_state.selected.iter().cloned().collect::<Vec<_>>(),
                self.collection.browser_left.current_folder.clone(),
            ),
            BrowserSide::Right => (
                self.collection.browser_right.table_state.selected.iter().cloned().collect::<Vec<_>>(),
                self.collection.browser_right.current_folder.clone(),
            ),
        };

        if selected_ids.is_empty() {
            log::debug!("Delete requested but no tracks selected");
            return Task::none();
        }

        // Get track names from domain
        let track_names: Vec<String> = selected_ids
            .iter()
            .filter_map(|id| self.domain.get_node(id).map(|n| n.name.clone()))
            .collect();

        // Determine delete target based on current folder
        // If in the collection root (tracks folder), it's a permanent delete
        // If in a playlist, it's just removing from playlist
        let target = if current_folder == Some(NodeId::tracks()) {
            // In collection - permanent deletion!
            DeleteTarget::CollectionTracks {
                track_names,
                track_ids: selected_ids,
            }
        } else if let Some(folder_id) = current_folder {
            // In a playlist - just remove from playlist
            let playlist_name = self.domain.get_node(&folder_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| folder_id.to_string());
            DeleteTarget::PlaylistTracks {
                playlist_name,
                track_ids: selected_ids,
                track_names,
            }
        } else {
            log::debug!("Delete requested but no folder selected");
            return Task::none();
        };

        log::info!("Showing delete confirmation for {:?}", target);
        self.delete_state.show(target);
        Task::none()
    }

    /// Handle CancelDelete message
    pub fn handle_cancel_delete(&mut self) -> Task<Message> {
        self.delete_state.cancel();
        Task::none()
    }

    /// Handle ConfirmDelete message
    pub fn handle_confirm_delete(&mut self) -> Task<Message> {
        if let Some(ref target) = self.delete_state.target {
            log::info!("Executing delete: {:?}", target);

            match target {
                DeleteTarget::PlaylistTracks { track_ids, .. } => {
                    // Remove tracks from playlist (not from collection)
                    for track_id in track_ids {
                        if let Err(e) = self.domain.remove_track_from_playlist_by_node(track_id) {
                            log::error!("Failed to remove track from playlist: {:?}", e);
                        }
                    }
                    // Refresh displays
                    self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                    if let Some(ref folder) = self.collection.browser_left.current_folder {
                        self.collection.left_tracks = self.domain.get_tracks_for_display(folder);
                    }
                    if let Some(ref folder) = self.collection.browser_right.current_folder {
                        self.collection.right_tracks = self.domain.get_tracks_for_display(folder);
                    }
                }
                DeleteTarget::CollectionTracks { track_ids, .. } => {
                    // PERMANENT deletion - delete files from disk!
                    for track_id in track_ids {
                        if let Err(e) = self.domain.delete_track_permanently_by_node(track_id) {
                            log::error!("Failed to delete track permanently: {:?}", e);
                        }
                    }
                    // Refresh displays
                    self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                    if let Some(ref folder) = self.collection.browser_left.current_folder {
                        self.collection.left_tracks = self.domain.get_tracks_for_display(folder);
                    }
                    if let Some(ref folder) = self.collection.browser_right.current_folder {
                        self.collection.right_tracks = self.domain.get_tracks_for_display(folder);
                    }
                }
                DeleteTarget::Playlist { playlist_id, .. } => {
                    // Delete playlist (tracks stay in collection)
                    if let Err(e) = self.domain.delete_playlist_by_node(playlist_id) {
                        log::error!("Failed to delete playlist: {:?}", e);
                    }
                    self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                }
            }

            // Clear selection after delete
            self.collection.browser_left.table_state.clear_selection();
            self.collection.browser_right.table_state.clear_selection();
        }

        self.delete_state.complete();
        Task::none()
    }

    /// Handle RequestDeleteById message (from context menu)
    pub fn handle_request_delete_by_id(&mut self, track_id: NodeId) -> Task<Message> {
        self.context_menu_state.close();

        // Determine if track is in collection or playlist
        if track_id.is_in_tracks() {
            // Collection track - permanent deletion
            let track_name = self.domain.get_node(&track_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| track_id.name().to_string());
            self.delete_state.show(DeleteTarget::CollectionTracks {
                track_names: vec![track_name],
                track_ids: vec![track_id],
            });
        } else {
            // Playlist track - just remove from playlist
            let track_name = self.domain.get_node(&track_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| track_id.name().to_string());
            let playlist_name = track_id
                .parent()
                .and_then(|p| self.domain.get_node(&p).map(|n| n.name.clone()))
                .unwrap_or_default();
            self.delete_state.show(DeleteTarget::PlaylistTracks {
                playlist_name,
                track_names: vec![track_name],
                track_ids: vec![track_id],
            });
        }
        Task::none()
    }

    /// Handle RequestDeletePlaylist message (from context menu)
    pub fn handle_request_delete_playlist(&mut self, playlist_id: NodeId) -> Task<Message> {
        self.context_menu_state.close();

        let playlist_name = self.domain.get_node(&playlist_id)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| playlist_id.name().to_string());

        self.delete_state.show(DeleteTarget::Playlist {
            playlist_name,
            playlist_id,
        });
        Task::none()
    }

    /// Handle ShowContextMenu message
    pub fn handle_show_context_menu(&mut self, kind: super::super::context_menu::ContextMenuKind, position: iced::Point) -> Task<Message> {
        log::info!("[CONTEXT MENU] ShowContextMenu called: position={:?}, is_open will be: true", position);
        self.context_menu_state.show(kind, position);
        log::info!("[CONTEXT MENU] After show: is_open={}, position={:?}", self.context_menu_state.is_open, self.context_menu_state.position);
        Task::none()
    }

    /// Handle CloseContextMenu message
    pub fn handle_close_context_menu(&mut self) -> Task<Message> {
        self.context_menu_state.close();
        Task::none()
    }
}
