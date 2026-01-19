//! Reanalysis message handlers
//!
//! Handles: StartReanalysis, ReanalysisProgress, CancelReanalysis, StartRenamePlaylist

use std::path::PathBuf;
use iced::Task;
use mesh_core::playlist::NodeId;
use crate::analysis::{AnalysisType, ReanalysisProgress, ReanalysisScope};
use super::super::app::MeshCueApp;
use super::super::message::Message;

impl MeshCueApp {
    /// Handle StartReanalysis message
    pub fn handle_start_reanalysis(&mut self, analysis_type: AnalysisType, scope: ReanalysisScope) -> Task<Message> {
        self.context_menu_state.close();

        // Don't start if already running
        if self.reanalysis_state.is_running {
            log::warn!("Re-analysis already in progress, ignoring request");
            return Task::none();
        }

        // Resolve scope to list of file paths
        let tracks: Vec<PathBuf> = match &scope {
            ReanalysisScope::SingleTrack(track_id) => {
                self.domain.get_node(track_id)
                    .and_then(|n| n.track_path.clone())
                    .map(|p| vec![p])
                    .unwrap_or_default()
            }
            ReanalysisScope::SelectedTracks(track_ids) => {
                track_ids
                    .iter()
                    .filter_map(|id| {
                        self.domain.get_node(id)
                            .and_then(|n| n.track_path.clone())
                    })
                    .collect()
            }
            ReanalysisScope::PlaylistFolder(playlist_id) => {
                // Get all tracks in the playlist
                self.domain.get_children(playlist_id)
                    .into_iter()
                    .filter_map(|node| node.track_path)
                    .collect()
            }
            ReanalysisScope::EntireCollection => {
                // Get all tracks from database via domain
                // Get tracks from the root tracks folder
                self.domain.get_children(&NodeId::tracks())
                    .into_iter()
                    .flat_map(|folder| self.domain.get_children(&folder.id))
                    .filter_map(|node| node.track_path)
                    .collect()
            }
        };

        if tracks.is_empty() {
            log::warn!("No tracks to re-analyze");
            return Task::none();
        }

        log::info!(
            "Starting {} re-analysis for {} tracks",
            analysis_type.display_name(),
            tracks.len()
        );

        // Set up UI state
        self.reanalysis_state.is_running = true;
        self.reanalysis_state.analysis_type = Some(analysis_type);
        self.reanalysis_state.total_tracks = tracks.len();
        self.reanalysis_state.completed_tracks = 0;
        self.reanalysis_state.succeeded = 0;
        self.reanalysis_state.failed = 0;
        self.reanalysis_state.current_track = None;

        // Start reanalysis through domain (owns db_service, config, spawns thread)
        if let Err(e) = self.domain.start_reanalysis(tracks, analysis_type) {
            log::error!("Failed to start reanalysis: {:?}", e);
            self.reanalysis_state.is_running = false;
        }
        Task::none()
    }

    /// Handle ReanalysisProgress message
    pub fn handle_reanalysis_progress(&mut self, progress: ReanalysisProgress) -> Task<Message> {
        match progress {
            ReanalysisProgress::Started { total_tracks, analysis_type } => {
                self.reanalysis_state.total_tracks = total_tracks;
                self.reanalysis_state.analysis_type = Some(analysis_type);
            }
            ReanalysisProgress::TrackStarted { track_name, .. } => {
                // Only update the display name, not the counter
                // (counter is updated by TrackCompleted)
                self.reanalysis_state.current_track = Some(track_name);
            }
            ReanalysisProgress::TrackCompleted { success, .. } => {
                if success {
                    self.reanalysis_state.succeeded += 1;
                } else {
                    self.reanalysis_state.failed += 1;
                }
                self.reanalysis_state.completed_tracks += 1;
            }
            ReanalysisProgress::AllComplete { succeeded, failed, .. } => {
                self.reanalysis_state.is_running = false;
                self.reanalysis_state.succeeded = succeeded;
                self.reanalysis_state.failed = failed;
                self.reanalysis_state.current_track = None;

                // Clear domain reanalysis state
                self.domain.clear_reanalysis_state();

                log::info!(
                    "Re-analysis complete: {} succeeded, {} failed",
                    succeeded,
                    failed
                );

                // Check if export was pending LUFS analysis
                if self.export_state.pending_lufs_analysis {
                    self.export_state.pending_lufs_analysis = false;
                    log::info!("[LUFS] LUFS analysis complete, now starting USB export");

                    // Directly trigger export (can't return Task from here when called via Tick)
                    self.trigger_usb_export_after_lufs();
                }

                // Refresh collection to show updated metadata
                return Task::perform(async {}, |_| Message::RefreshCollection);
            }
        }
        Task::none()
    }

    /// Handle CancelReanalysis message
    pub fn handle_cancel_reanalysis(&mut self) -> Task<Message> {
        // Cancel through domain (owns the cancel flag)
        self.domain.cancel_reanalysis();
        log::info!("Re-analysis cancellation requested");
        Task::none()
    }

    /// Handle StartRenamePlaylist message
    pub fn handle_start_rename_playlist(&mut self, playlist_id: NodeId) -> Task<Message> {
        self.context_menu_state.close();

        // Start inline rename in the appropriate tree
        if let Some(node) = self.domain.get_node(&playlist_id) {
            // Try to find which browser has this playlist and start edit
            if self
                .collection
                .browser_left
                .tree_state
                .is_expanded(&playlist_id.parent().unwrap_or_else(NodeId::playlists))
            {
                self.collection
                    .browser_left
                    .tree_state
                    .start_edit(playlist_id, node.name.clone());
            } else {
                self.collection
                    .browser_right
                    .tree_state
                    .start_edit(playlist_id, node.name.clone());
            }
        }
        Task::none()
    }
}
