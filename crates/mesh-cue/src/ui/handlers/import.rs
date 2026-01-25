//! Batch import message handlers
//!
//! Handles: OpenImport, CloseImport, ScanImportFolder, ImportFolderScanned,
//! StartBatchImport, ImportProgressUpdate, CancelImport, DismissImportResults

use iced::Task;
use crate::batch_import::{self, ImportProgress};
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::{ImportPhase, ImportState};

impl MeshCueApp {
    /// Handle OpenImport message
    pub fn handle_open_import(&mut self) -> Task<Message> {
        // If import is already running, just open the modal (don't rescan)
        if self.import_state.phase.is_some() {
            self.import_state.is_open = true;
            return Task::none();
        }

        // Not running - reset state and trigger folder scan
        self.import_state = ImportState::default();
        self.import_state.is_open = true;
        self.update(Message::ScanImportFolder)
    }

    /// Handle CloseImport message
    pub fn handle_close_import(&mut self) -> Task<Message> {
        // Just close the modal - DON'T cancel the import!
        // Import continues in background, progress visible via status bar at bottom of screen
        // Only Message::CancelImport (explicit cancel button) should stop the import
        self.import_state.is_open = false;
        Task::none()
    }

    /// Handle ScanImportFolder message
    pub fn handle_scan_import_folder(&mut self) -> Task<Message> {
        self.import_state.phase = Some(ImportPhase::Scanning);
        let import_folder = self.import_state.import_folder.clone();
        Task::perform(
            async move {
                batch_import::scan_and_group_stems(&import_folder)
                    .unwrap_or_else(|e| {
                        log::error!("Failed to scan import folder: {}", e);
                        Vec::new()
                    })
            },
            Message::ImportFolderScanned,
        )
    }

    /// Handle ImportFolderScanned message
    pub fn handle_import_folder_scanned(&mut self, groups: Vec<batch_import::StemGroup>) -> Task<Message> {
        log::info!("Import folder scanned: {} groups found", groups.len());
        self.import_state.detected_groups = groups;
        self.import_state.phase = None;
        Task::none()
    }

    /// Handle StartBatchImport message
    pub fn handle_start_batch_import(&mut self) -> Task<Message> {
        let complete_groups: Vec<_> = self
            .import_state
            .detected_groups
            .iter()
            .filter(|g| g.is_complete())
            .cloned()
            .collect();

        if complete_groups.is_empty() {
            log::warn!("No complete stem groups to import");
            return Task::none();
        }

        log::info!("Starting batch import of {} tracks", complete_groups.len());

        // Set initial UI phase
        self.import_state.results.clear();
        self.import_state.phase = Some(ImportPhase::Processing {
            current_track: String::new(),
            completed: 0,
            total: complete_groups.len(),
            start_time: std::time::Instant::now(),
        });

        // Start import through domain (owns db_service, config, spawns thread)
        let import_folder = self.import_state.import_folder.clone();
        if let Err(e) = self.domain.start_batch_import(complete_groups, import_folder) {
            log::error!("Failed to start batch import: {:?}", e);
            self.import_state.phase = None;
        }
        Task::none()
    }

    /// Handle ImportProgressUpdate message
    pub fn handle_import_progress_update(&mut self, progress: ImportProgress) -> Task<Message> {
        match progress {
            ImportProgress::Started { total } => {
                log::info!("Import started: {} tracks", total);
                self.import_state.phase = Some(ImportPhase::Processing {
                    current_track: String::new(),
                    completed: 0,
                    total,
                    start_time: std::time::Instant::now(),
                });
            }
            ImportProgress::TrackStarted { base_name, index, total } => {
                log::info!("Processing track {}/{}: {}", index + 1, total, base_name);
                if let Some(ImportPhase::Processing { ref mut current_track, .. }) =
                    self.import_state.phase
                {
                    *current_track = base_name;
                }
            }
            ImportProgress::TrackCompleted(result) => {
                log::info!(
                    "Track completed: {} (success={})",
                    result.base_name,
                    result.success
                );
                let was_success = result.success;
                if let Some(ImportPhase::Processing { ref mut completed, .. }) =
                    self.import_state.phase
                {
                    *completed += 1;
                }
                self.import_state.results.push(result);

                // Refresh collection immediately when track imports successfully
                // so user sees new tracks appear in browser as they complete
                if was_success {
                    // Ensure left browser has a folder selected (default to tracks)
                    if self.collection.browser_left.current_folder.is_none() {
                        self.collection.browser_left.set_current_folder(mesh_core::playlist::NodeId::tracks());
                    }
                    // Refresh both tree and track lists
                    return Task::perform(async {}, |_| Message::RefreshCollection);
                }
            }
            ImportProgress::AllComplete { results } => {
                log::info!("Import complete: {} tracks processed", results.len());
                // Calculate duration from start_time if available
                let duration = if let Some(ImportPhase::Processing { start_time, .. }) =
                    self.import_state.phase
                {
                    start_time.elapsed()
                } else {
                    std::time::Duration::ZERO
                };

                self.import_state.phase = Some(ImportPhase::Complete { duration });
                self.import_state.results = results;
                self.import_state.show_results = true;

                // Clear domain import state
                self.domain.clear_import_state();

                // Refresh collection to show newly imported tracks
                // Need both: RefreshCollection scans for tracks, RefreshPlaylists updates tree
                return Task::batch([
                    Task::perform(async {}, |_| Message::RefreshCollection),
                    Task::perform(async {}, |_| Message::RefreshPlaylists),
                ]);
            }
        }
        Task::none()
    }

    /// Handle CancelImport message
    pub fn handle_cancel_import(&mut self) -> Task<Message> {
        log::info!("Cancelling import");
        // Cancel through domain (owns the cancel flag)
        self.domain.cancel_import();
        self.domain.clear_import_state();
        self.import_state.phase = None;
        Task::none()
    }

    /// Handle DismissImportResults message
    pub fn handle_dismiss_import_results(&mut self) -> Task<Message> {
        self.import_state.phase = None;
        self.import_state.show_results = false;
        self.import_state.is_open = false;
        Task::none()
    }
}
