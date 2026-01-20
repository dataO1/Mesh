//! USB export message handlers
//!
//! Handles: OpenExport, CloseExport, SelectExportDevice, ToggleExportPlaylist,
//! ToggleExportPlaylistExpand, ToggleExportConfig, BuildSyncPlan, StartExport,
//! CancelExport, UsbMessage, DismissExportResults

use iced::Task;
use mesh_core::usb::{UsbMessage as UsbMsg, ExportableConfig, ExportableAudioConfig, ExportableDisplayConfig, ExportableSlicerConfig};
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::ExportPhase;
use crate::analysis::AnalysisType;
use mesh_core::playlist::NodeId;

impl MeshCueApp {
    /// Handle OpenExport message
    pub fn handle_open_export(&mut self) -> Task<Message> {
        log::info!("Opening USB export modal");
        self.export_state.is_open = true;
        self.export_state.reset();
        // Request fresh device list from UsbManager
        self.domain.refresh_usb_devices();
        Task::none()
    }

    /// Handle CloseExport message
    pub fn handle_close_export(&mut self) -> Task<Message> {
        // Just close the modal - don't cancel export in progress
        self.export_state.is_open = false;
        Task::none()
    }

    /// Handle SelectExportDevice message
    pub fn handle_select_export_device(&mut self, idx: usize) -> Task<Message> {
        self.export_state.selected_device = Some(idx);
        // Invalidate cached sync plan when device changes
        self.export_state.sync_plan = None;
        // If the device isn't mounted yet, request mount
        if let Some(device) = self.export_state.devices.get(idx) {
            if device.mount_point.is_none() {
                self.export_state.phase = ExportPhase::Mounting {
                    device_label: device.label.clone(),
                };
                self.domain.mount_usb_device(device.device_path.clone());
            } else {
                // Device already mounted, trigger sync plan computation
                self.trigger_sync_plan_computation();
            }
        }
        Task::none()
    }

    /// Handle ToggleExportPlaylist message
    pub fn handle_toggle_export_playlist(&mut self, id: NodeId) -> Task<Message> {
        // Use recursive toggle to select/deselect all children
        self.export_state.toggle_playlist_recursive(id, &self.collection.tree_nodes);
        // Invalidate cached sync plan and trigger recomputation
        self.export_state.sync_plan = None;
        self.trigger_sync_plan_computation();
        Task::none()
    }

    /// Handle ToggleExportPlaylistExpand message
    pub fn handle_toggle_export_playlist_expand(&mut self, id: NodeId) -> Task<Message> {
        self.export_state.toggle_playlist_expanded(id);
        Task::none()
    }

    /// Handle ToggleExportConfig message
    pub fn handle_toggle_export_config(&mut self) -> Task<Message> {
        self.export_state.export_config = !self.export_state.export_config;
        Task::none()
    }

    /// Handle BuildSyncPlan message
    pub fn handle_build_sync_plan(&mut self) -> Task<Message> {
        // Legacy handler - sync plan is now computed automatically
        // This triggers a manual recomputation if needed
        self.trigger_sync_plan_computation();
        Task::none()
    }

    /// Handle StartExport message
    pub fn handle_start_export(&mut self) -> Task<Message> {
        log::info!("Starting USB export");
        if let Some(idx) = self.export_state.selected_device {
            if let Some(device) = self.export_state.devices.get(idx) {
                // Use the cached sync plan
                if let Some(ref plan) = self.export_state.sync_plan {
                    // Use pre-computed LUFS check from sync plan (already computed in background)
                    let tracks_missing_lufs = plan.tracks_missing_lufs.clone();

                    if !tracks_missing_lufs.is_empty() {
                        // Need to analyze LUFS first before export
                        log::info!(
                            "[LUFS] {} tracks missing LUFS, starting analysis before export",
                            tracks_missing_lufs.len()
                        );

                        // Don't start if reanalysis is already running
                        if self.reanalysis_state.is_running || self.domain.is_reanalyzing() {
                            log::warn!("Re-analysis already in progress, cannot analyze LUFS for export");
                            return Task::none();
                        }

                        // Set up UI reanalysis state
                        self.reanalysis_state.is_running = true;
                        self.reanalysis_state.analysis_type = Some(AnalysisType::Loudness);
                        self.reanalysis_state.total_tracks = tracks_missing_lufs.len();
                        self.reanalysis_state.completed_tracks = 0;
                        self.reanalysis_state.succeeded = 0;
                        self.reanalysis_state.failed = 0;
                        self.reanalysis_state.current_track = None;

                        // Mark that export should start after analysis
                        self.export_state.pending_lufs_analysis = true;

                        // Start reanalysis through domain (owns db_service, config)
                        if let Err(e) = self.domain.start_reanalysis(tracks_missing_lufs, AnalysisType::Loudness) {
                            log::error!("Failed to start LUFS analysis: {:?}", e);
                            self.reanalysis_state.is_running = false;
                        }

                        return Task::none();
                    }

                    // No tracks missing LUFS, proceed with export directly
                    let config = self.build_export_config();
                    self.domain.start_usb_export(
                        device.device_path.clone(),
                        plan.clone(),
                        self.export_state.export_config,
                        config,
                    );
                }
            }
        }
        Task::none()
    }

    /// Handle CancelExport message
    pub fn handle_cancel_export(&mut self) -> Task<Message> {
        log::info!("Cancelling USB export");
        self.domain.cancel_usb_export();
        self.export_state.phase = ExportPhase::SelectDevice;
        Task::none()
    }

    /// Handle UsbMessage message
    pub fn handle_usb_message(&mut self, usb_msg: UsbMsg) -> Task<Message> {
        match usb_msg {
            UsbMsg::DevicesRefreshed(devices) => {
                self.export_state.devices = devices;
                // Auto-select first device if none selected and devices available
                if self.export_state.selected_device.is_none()
                    && !self.export_state.devices.is_empty()
                {
                    self.export_state.selected_device = Some(0);
                }
            }
            UsbMsg::DeviceConnected(device) => {
                log::info!("USB device connected: {}", device.label);
                self.export_state.devices.push(device);
            }
            UsbMsg::DeviceDisconnected { device_path } => {
                log::info!("USB device disconnected: {:?}", device_path);
                self.export_state.devices.retain(|d| d.device_path != device_path);
                // Clear selection if the disconnected device was selected
                if let Some(idx) = self.export_state.selected_device {
                    if self.export_state.devices.get(idx).map(|d| &d.device_path) == Some(&device_path) {
                        self.export_state.selected_device = None;
                    }
                }
            }
            UsbMsg::MountComplete { result } => {
                match result {
                    Ok(dev) => {
                        log::info!("Device mounted at {:?}", dev.mount_point);
                        // Update device in list
                        if let Some(existing) = self.export_state.devices.iter_mut()
                            .find(|d| d.device_path == dev.device_path)
                        {
                            *existing = dev;
                        }
                        // Stay in SelectDevice phase, trigger sync plan computation
                        self.export_state.phase = ExportPhase::SelectDevice;
                        self.trigger_sync_plan_computation();
                    }
                    Err(e) => {
                        log::error!("Mount failed: {}", e);
                        self.export_state.phase = ExportPhase::Error(e.to_string());
                    }
                }
            }
            UsbMsg::SyncPlanProgress { files_scanned: _, total_files: _ } => {
                // Sync plan computation in progress (background, don't change phase)
                self.export_state.sync_plan_computing = true;
            }
            UsbMsg::SyncPlanReady(plan) => {
                // Store the computed plan (don't change phase - stay in SelectDevice)
                self.export_state.sync_plan = Some(plan);
                self.export_state.sync_plan_computing = false;
            }
            UsbMsg::ExportStarted { total_tracks, total_bytes } => {
                self.export_state.phase = ExportPhase::Exporting {
                    current_track: String::new(),
                    tracks_complete: 0,
                    bytes_complete: 0,
                    total_tracks,
                    total_bytes,
                    start_time: std::time::Instant::now(),
                };
            }
            UsbMsg::ExportTrackStarted { filename, track_index: _ } => {
                // Update current track being exported
                if let ExportPhase::Exporting { current_track, .. } = &mut self.export_state.phase {
                    *current_track = filename;
                }
            }
            UsbMsg::ExportTrackComplete {
                filename: _,
                track_index: _, // Ignored - parallel processing means tracks complete out of order
                total_tracks,
                bytes_complete,
                total_bytes,
            } => {
                // Increment completion count (don't use track_index - that's array position, not completion order)
                if let ExportPhase::Exporting { tracks_complete, start_time, .. } = &self.export_state.phase {
                    let new_count = tracks_complete + 1;
                    let start = *start_time;
                    self.export_state.phase = ExportPhase::Exporting {
                        current_track: String::new(), // Will be updated by next TrackStarted
                        tracks_complete: new_count,
                        bytes_complete,
                        total_tracks,
                        total_bytes,
                        start_time: start,
                    };
                }
            }
            UsbMsg::ExportTrackFailed { filename, track_index: _, error } => {
                log::warn!("Track export failed: {} - {}", filename, error);
                // Don't change phase - let export continue with other tracks
            }
            UsbMsg::ExportComplete { duration, tracks_exported, failed_files } => {
                self.export_state.phase = ExportPhase::Complete {
                    duration,
                    tracks_exported,
                    failed_files,
                };
                self.export_state.show_results = true;
                // Re-open modal to show completion results (even if user closed it during export)
                self.export_state.is_open = true;
            }
            UsbMsg::ExportError(err) => {
                self.export_state.phase = ExportPhase::Error(err.to_string());
                // Re-open modal to show error (even if user closed it during export)
                self.export_state.is_open = true;
            }
            UsbMsg::ExportCancelled => {
                self.export_state.phase = ExportPhase::SelectDevice;
            }
            _ => {
                // Handle other messages as needed
            }
        }
        Task::none()
    }

    /// Handle DismissExportResults message
    pub fn handle_dismiss_export_results(&mut self) -> Task<Message> {
        self.export_state.phase = ExportPhase::SelectDevice;
        self.export_state.show_results = false;
        self.export_state.is_open = false;
        Task::none()
    }

    /// Trigger USB export after LUFS analysis completes
    ///
    /// Called from reanalysis completion handler when pending_lufs_analysis is set
    pub fn trigger_usb_export_after_lufs(&mut self) {
        if let Some(idx) = self.export_state.selected_device {
            if let Some(device) = self.export_state.devices.get(idx) {
                if let Some(ref plan) = self.export_state.sync_plan {
                    let config = self.build_export_config();
                    self.domain.start_usb_export(
                        device.device_path.clone(),
                        plan.clone(),
                        self.export_state.export_config,
                        config,
                    );
                }
            }
        }
    }

    /// Trigger background sync plan computation for USB export
    ///
    /// This is called automatically when device or playlist selection changes.
    /// The sync plan is computed in the background and stored in export_state.sync_plan.
    pub fn trigger_sync_plan_computation(&mut self) {
        // Only compute if we have a device selected and playlists selected
        if self.export_state.selected_playlists.is_empty() {
            self.export_state.sync_plan = None;
            self.export_state.sync_plan_computing = false;
            return;
        }

        if let Some(idx) = self.export_state.selected_device {
            if let Some(device) = self.export_state.devices.get(idx) {
                // Only compute if device is mounted
                if device.mount_point.is_some() {
                    let playlists: Vec<NodeId> = self.export_state.selected_playlists.iter().cloned().collect();
                    self.export_state.sync_plan_computing = true;
                    self.domain.build_usb_sync_plan(device.device_path.clone(), playlists);
                }
            }
        }
    }

    /// Build ExportableConfig from mesh-cue's Config
    fn build_export_config(&self) -> Option<ExportableConfig> {
        if self.export_state.export_config {
            Some(ExportableConfig {
                audio: ExportableAudioConfig {
                    global_bpm: self.domain.config().display.global_bpm,
                    phase_sync: true, // Default to true for mesh-cue
                    loudness: self.domain.config().analysis.loudness.clone(),
                },
                display: ExportableDisplayConfig {
                    default_loop_length_index: self.domain.config().display.default_loop_length_index,
                    default_zoom_bars: self.domain.config().display.zoom_bars,
                    grid_bars: self.domain.config().display.grid_bars,
                    stem_color_palette: "natural".to_string(),
                },
                slicer: ExportableSlicerConfig {
                    buffer_bars: self.domain.config().slicer.validated_buffer_bars(),
                    presets: Vec::new(), // Presets handled separately
                },
            })
        } else {
            None
        }
    }
}
