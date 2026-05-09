//! Reanalysis message handlers
//!
//! Handles: StartBeatsReanalysis, OpenMetadataReanalysisConfig, ConfirmMetadataReanalysis,
//! toggle checkboxes, ReanalysisProgress, CancelReanalysis, StartRenamePlaylist

use std::path::PathBuf;
use iced::Task;
use mesh_core::playlist::NodeId;
use crate::analysis::{AnalysisType, MetadataOptions, ReanalysisProgress, ReanalysisScope};
use super::super::app::MeshCueApp;
use super::super::message::Message;

impl MeshCueApp {
    /// Resolve a ReanalysisScope to a list of file paths
    fn resolve_scope_to_paths(&self, scope: &ReanalysisScope) -> Vec<PathBuf> {
        match scope {
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
                self.domain.get_children(playlist_id)
                    .into_iter()
                    .filter_map(|node| node.track_path)
                    .collect()
            }
            ReanalysisScope::EntireCollection => {
                // get_children only returns sub-folder nodes (e.g. "tracks/Techno") —
                // it misses flat collections where all tracks sit directly in "tracks/".
                // Use get_tracks_for_display which queries by folder_path directly and
                // always returns the correct set regardless of nesting.
                self.domain.get_tracks_for_display(&NodeId::tracks())
                    .into_iter()
                    .filter_map(|row| row.track_path.map(PathBuf::from))
                    .collect()
            }
        }
    }

    /// Handle StartBeatsReanalysis message — fires immediately from context menu
    pub fn handle_start_beats_reanalysis(&mut self, scope: ReanalysisScope) -> Task<Message> {
        self.context_menu_state.close();

        if self.reanalysis_state.is_running {
            log::warn!("Re-analysis already in progress, ignoring request");
            return Task::none();
        }

        let tracks = self.resolve_scope_to_paths(&scope);
        if tracks.is_empty() {
            log::warn!("No tracks to re-analyze");
            return Task::none();
        }

        log::info!("Starting Beats re-analysis for {} tracks", tracks.len());

        // Pause audio stream to free CPU for analysis
        if let Some(ref handle) = self.audio_handle {
            handle.pause();
        }

        self.reanalysis_state.is_running = true;
        self.reanalysis_state.analysis_type = Some(AnalysisType::Beats);
        self.reanalysis_state.total_tracks = tracks.len();
        self.reanalysis_state.completed_tracks = 0;
        self.reanalysis_state.succeeded = 0;
        self.reanalysis_state.failed = 0;
        self.reanalysis_state.current_track = None;

        if let Err(e) = self.domain.start_reanalysis(tracks, AnalysisType::Beats, None) {
            log::error!("Failed to start beats reanalysis: {:?}", e);
            self.reanalysis_state.is_running = false;
        }
        Task::none()
    }

    /// Handle OpenMetadataReanalysisConfig — opens the config modal with all checkboxes ON
    pub fn handle_open_metadata_reanalysis_config(&mut self, scope: ReanalysisScope) -> Task<Message> {
        self.context_menu_state.close();

        self.reanalysis_state.config_modal_open = true;
        self.reanalysis_state.config_scope = Some(scope);
        self.reanalysis_state.config_name_artist = true;
        self.reanalysis_state.config_loudness = true;
        self.reanalysis_state.config_key = true;
        self.reanalysis_state.config_tags = true;
        Task::none()
    }

    /// Handle checkbox toggle: Name/Artist
    pub fn handle_toggle_reanalysis_name_artist(&mut self, value: bool) -> Task<Message> {
        self.reanalysis_state.config_name_artist = value;
        Task::none()
    }

    /// Handle checkbox toggle: Loudness
    pub fn handle_toggle_reanalysis_loudness(&mut self, value: bool) -> Task<Message> {
        self.reanalysis_state.config_loudness = value;
        Task::none()
    }

    /// Handle checkbox toggle: Key
    pub fn handle_toggle_reanalysis_key(&mut self, value: bool) -> Task<Message> {
        self.reanalysis_state.config_key = value;
        Task::none()
    }

    /// Handle checkbox toggle: Tags
    pub fn handle_toggle_reanalysis_tags(&mut self, value: bool) -> Task<Message> {
        self.reanalysis_state.config_tags = value;
        Task::none()
    }

    /// Handle CloseReanalysisConfig — close the modal without starting
    pub fn handle_close_reanalysis_config(&mut self) -> Task<Message> {
        self.reanalysis_state.config_modal_open = false;
        self.reanalysis_state.config_scope = None;
        Task::none()
    }

    /// Handle ConfirmMetadataReanalysis — start metadata reanalysis with selected options
    pub fn handle_confirm_metadata_reanalysis(&mut self) -> Task<Message> {
        self.reanalysis_state.config_modal_open = false;

        if self.reanalysis_state.is_running {
            log::warn!("Re-analysis already in progress, ignoring request");
            return Task::none();
        }

        let scope = match self.reanalysis_state.config_scope.take() {
            Some(s) => s,
            None => return Task::none(),
        };

        let options = MetadataOptions {
            name_artist: self.reanalysis_state.config_name_artist,
            loudness: self.reanalysis_state.config_loudness,
            key: self.reanalysis_state.config_key,
            tags: self.reanalysis_state.config_tags,
        };

        // At least one option must be ticked
        if !options.name_artist && !options.loudness && !options.key && !options.tags {
            log::warn!("No metadata options selected, ignoring");
            return Task::none();
        }

        let tracks = self.resolve_scope_to_paths(&scope);
        if tracks.is_empty() {
            log::warn!("No tracks to re-analyze");
            return Task::none();
        }

        log::info!("Starting Metadata re-analysis for {} tracks (name={}, loudness={}, key={}, tags={})",
            tracks.len(), options.name_artist, options.loudness, options.key, options.tags);

        // Pause audio stream to free CPU for analysis
        if let Some(ref handle) = self.audio_handle {
            handle.pause();
        }

        self.reanalysis_state.is_running = true;
        self.reanalysis_state.analysis_type = Some(AnalysisType::Metadata);
        self.reanalysis_state.total_tracks = tracks.len();
        self.reanalysis_state.completed_tracks = 0;
        self.reanalysis_state.succeeded = 0;
        self.reanalysis_state.failed = 0;
        self.reanalysis_state.current_track = None;

        if let Err(e) = self.domain.start_reanalysis(tracks, AnalysisType::Metadata, Some(options)) {
            log::error!("Failed to start metadata reanalysis: {:?}", e);
            self.reanalysis_state.is_running = false;
        }
        Task::none()
    }

    /// Handle ReanalysisProgress message
    pub fn handle_reanalysis_progress(&mut self, progress: ReanalysisProgress) -> Task<Message> {
        match progress {
            ReanalysisProgress::Started { total_tracks, analysis_type, .. } => {
                self.reanalysis_state.total_tracks = total_tracks;
                self.reanalysis_state.analysis_type = Some(analysis_type);
            }
            ReanalysisProgress::ModelDownload { model_name, bytes_done, bytes_total } => {
                use crate::ui::state::reanalysis::ModelDownloadStatus;
                let done = bytes_total.map_or(false, |t| t > 0 && bytes_done >= t);
                self.reanalysis_state.model_download = if done {
                    None
                } else {
                    Some(ModelDownloadStatus { model_name, bytes_done, bytes_total })
                };
            }
            ReanalysisProgress::TrackStarted { track_name, .. } => {
                // Only update the display name, not the counter
                // (counter is updated by TrackCompleted)
                self.reanalysis_state.current_track = Some(track_name);
                // First track starting → download phase is over.
                self.reanalysis_state.model_download = None;
            }
            ReanalysisProgress::TrackCompleted { success, .. } => {
                if success {
                    self.reanalysis_state.succeeded += 1;
                } else {
                    self.reanalysis_state.failed += 1;
                }
                self.reanalysis_state.completed_tracks += 1;

                // Refresh collection per-track so metadata updates are visible immediately
                if success {
                    return Task::perform(async {}, |_| Message::RefreshCollection);
                }
            }
            ReanalysisProgress::AllComplete { succeeded, failed, .. } => {
                self.reanalysis_state.is_running = false;
                self.reanalysis_state.succeeded = succeeded;
                self.reanalysis_state.failed = failed;
                self.reanalysis_state.current_track = None;
                self.reanalysis_state.model_download = None;

                // Resume audio if a track is loaded
                if self.collection.loaded_track.is_some() {
                    if let Some(ref handle) = self.audio_handle {
                        handle.play();
                    }
                }

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

                // Refresh collection and rebuild PCA index (ML embeddings may have changed)
                return Task::batch([
                    Task::perform(async {}, |_| Message::RefreshCollection),
                    Task::perform(async {}, |_| Message::BuildSimilarityIndex),
                ]);
            }
        }
        Task::none()
    }

    /// Handle CancelReanalysis message
    pub fn handle_cancel_reanalysis(&mut self) -> Task<Message> {
        // Cancel through domain (owns the cancel flag)
        self.domain.cancel_reanalysis();
        log::info!("Re-analysis cancellation requested");

        // Resume audio if a track is loaded
        if self.collection.loaded_track.is_some() {
            if let Some(ref handle) = self.audio_handle {
                handle.play();
            }
        }
        Task::none()
    }

    // ── Round-7.7 migration prompt ──────────────────────────────────────────

    /// Show the V18.X intensity-probe migration modal at startup.
    /// Reuses the DeleteTarget::Custom confirmation scaffolding so we don't
    /// have to build a separate Iced widget. On confirm: dispatch
    /// AcceptMigrationReanalysis. On cancel ("Later"): the modal closes via
    /// the existing CancelDelete handler, no further action.
    pub fn handle_show_migration_prompt(&mut self, n_tracks: usize) -> Task<Message> {
        if n_tracks == 0 {
            return Task::none();
        }
        // Skip the popup if a metadata reanalysis is already running — the user
        // is already on it (manually started before this prompt landed).
        if self.reanalysis_state.is_running {
            log::debug!("[migration] popup suppressed — re-analysis already in progress");
            return Task::none();
        }
        self.delete_state.show(super::super::delete_modal::DeleteTarget::Custom {
            title: "░▒▓ MESH JUST DROPPED A NEW INTENSITY MODEL ▓▒░".to_string(),
            description: format!(
                "V18.X is on the decks — sharper ear, deeper substrate. \
                 To make the new sound system work for {} of your tracks, give them \
                 a fresh re-analyse. Anything not re-analysed stays on the old ranking until you do.\n\n\
                 About a second per track on a hot CPU.",
                n_tracks,
            ),
            warning: String::new(),
            confirm_label: "Drop the beat".to_string(),
            confirm_message: Box::new(Message::AcceptMigrationReanalysis),
        });
        Task::none()
    }

    /// User clicked "Drop the beat" — kick off the existing batch metadata
    /// reanalysis flow with `tags: true` ONLY (everything else explicitly
    /// false), scope = EntireCollection.
    ///
    /// **Critical invariant:** this path must only re-write ML-related data
    /// (ml_analysis, ml_embeddings, ml_intensity_embeddings, ML-derived
    /// track tags, stem_energy). It must NOT touch title, artist, LUFS,
    /// key, BPM, beat grid, cue points, saved loops, or the audio file on
    /// disk. Audited 2026-05-09 against `reanalyze_metadata_track` in
    /// `crate::reanalysis` — `name_artist`/`loudness`/`key` branches all
    /// gated by their respective flags. The defensive assertion below
    /// guards against future accidental flag-flipping in this path.
    pub fn handle_accept_migration_reanalysis(&mut self) -> Task<Message> {
        // Configure the reanalysis state as if the user had ticked the
        // "ML Tags" checkbox in the existing config modal — and ONLY that.
        self.reanalysis_state.config_scope = Some(crate::analysis::ReanalysisScope::EntireCollection);
        self.reanalysis_state.config_name_artist = false;
        self.reanalysis_state.config_loudness = false;
        self.reanalysis_state.config_key = false;
        self.reanalysis_state.config_tags = true;

        // Defensive: hard-fail before dispatch if any non-tags flag is set.
        // Catches future bugs where another code path leaves stale state on
        // self.reanalysis_state that this handler would inadvertently inherit.
        assert!(
            !self.reanalysis_state.config_name_artist
                && !self.reanalysis_state.config_loudness
                && !self.reanalysis_state.config_key
                && self.reanalysis_state.config_tags,
            "migration popup must dispatch tags-only reanalysis (name={}, loudness={}, key={}, tags={})",
            self.reanalysis_state.config_name_artist,
            self.reanalysis_state.config_loudness,
            self.reanalysis_state.config_key,
            self.reanalysis_state.config_tags,
        );
        log::info!("[migration] AcceptMigrationReanalysis → tags-only reanalysis on EntireCollection");

        // Dispatch ConfirmMetadataReanalysis to reuse the existing path
        // (logging, worker pool sizing, progress handling all stay the same).
        Task::done(Message::ConfirmMetadataReanalysis)
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
