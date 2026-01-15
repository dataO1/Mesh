//! USB export state
//!
//! Manages the state for the USB export modal and export process.

use mesh_core::playlist::NodeId;
use mesh_core::usb::{SyncPlan, UsbDevice, UsbMessage};
use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Phase of the export process
#[derive(Debug, Clone)]
pub enum ExportPhase {
    /// Initial state - selecting device and playlists
    SelectDevice,

    /// Mounting the selected device
    Mounting {
        device_label: String,
    },

    /// Scanning playlists on USB to compare
    ScanningUsb,

    /// Building sync plan (hashing local files)
    BuildingSyncPlan {
        files_hashed: usize,
        total_files: usize,
    },

    /// Showing sync plan, waiting for user confirmation
    ReadyToSync {
        plan: SyncPlan,
    },

    /// Exporting files
    Exporting {
        current_file: String,
        files_complete: usize,
        bytes_complete: u64,
        total_files: usize,
        total_bytes: u64,
        start_time: Instant,
    },

    /// Export complete
    Complete {
        duration: Duration,
        files_exported: usize,
        failed_files: Vec<(std::path::PathBuf, String)>,
    },

    /// Error state
    Error(String),
}

impl ExportPhase {
    /// Check if this phase allows interaction (device/playlist selection)
    pub fn allows_selection(&self) -> bool {
        matches!(self, ExportPhase::SelectDevice | ExportPhase::ReadyToSync { .. })
    }

    /// Check if export is in progress
    pub fn is_exporting(&self) -> bool {
        matches!(
            self,
            ExportPhase::Mounting { .. }
                | ExportPhase::ScanningUsb
                | ExportPhase::BuildingSyncPlan { .. }
                | ExportPhase::Exporting { .. }
        )
    }

    /// Get a human-readable status message
    pub fn status_message(&self) -> String {
        match self {
            ExportPhase::SelectDevice => "Select a device and playlists to export".to_string(),
            ExportPhase::Mounting { device_label } => format!("Mounting {}...", device_label),
            ExportPhase::ScanningUsb => "Scanning USB playlists...".to_string(),
            ExportPhase::BuildingSyncPlan { files_hashed, total_files } => {
                format!("Calculating changes: {}/{} files", files_hashed, total_files)
            }
            ExportPhase::ReadyToSync { plan } => plan.summary(),
            ExportPhase::Exporting { files_complete, total_files, .. } => {
                format!("Exporting: {}/{} files", files_complete, total_files)
            }
            ExportPhase::Complete { files_exported, failed_files, .. } => {
                if failed_files.is_empty() {
                    format!("Export complete! {} files exported", files_exported)
                } else {
                    format!(
                        "Export complete with {} error(s). {} files exported",
                        failed_files.len(),
                        files_exported
                    )
                }
            }
            ExportPhase::Error(msg) => format!("Error: {}", msg),
        }
    }
}

/// State for the USB export modal
#[derive(Debug)]
pub struct ExportState {
    /// Whether the export modal is open
    pub is_open: bool,

    /// Detected USB devices
    pub devices: Vec<UsbDevice>,

    /// Currently selected device index
    pub selected_device: Option<usize>,

    /// Selected playlists for export (by NodeId)
    pub selected_playlists: HashSet<NodeId>,

    /// Current export phase
    pub phase: ExportPhase,

    /// Include config file in export
    pub export_config: bool,

    /// Channel to receive USB messages (from UsbManager)
    /// Note: This is set when export modal opens and manager is initialized
    pub usb_message_rx: Option<Receiver<UsbMessage>>,

    /// Show detailed results after completion
    pub show_results: bool,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            is_open: false,
            devices: Vec::new(),
            selected_device: None,
            selected_playlists: HashSet::new(),
            phase: ExportPhase::SelectDevice,
            export_config: true, // Default to including config
            usb_message_rx: None,
            show_results: false,
        }
    }
}

impl ExportState {
    /// Reset state when opening the modal
    pub fn reset(&mut self) {
        self.selected_device = None;
        self.selected_playlists.clear();
        self.phase = ExportPhase::SelectDevice;
        self.show_results = false;
    }

    /// Get the currently selected device
    pub fn selected_device(&self) -> Option<&UsbDevice> {
        self.selected_device
            .and_then(|idx| self.devices.get(idx))
    }

    /// Check if a playlist is selected for export
    pub fn is_playlist_selected(&self, id: &NodeId) -> bool {
        self.selected_playlists.contains(id)
    }

    /// Toggle playlist selection
    pub fn toggle_playlist(&mut self, id: NodeId) {
        if self.selected_playlists.contains(&id) {
            self.selected_playlists.remove(&id);
        } else {
            self.selected_playlists.insert(id);
        }
    }

    /// Check if export can be started
    pub fn can_start_export(&self) -> bool {
        self.selected_device.is_some()
            && !self.selected_playlists.is_empty()
            && matches!(self.phase, ExportPhase::SelectDevice | ExportPhase::ReadyToSync { .. })
    }

    /// Check if cancel is available
    pub fn can_cancel(&self) -> bool {
        self.phase.is_exporting()
    }

    /// Get export progress (0.0 - 1.0)
    pub fn progress(&self) -> Option<f32> {
        match &self.phase {
            ExportPhase::BuildingSyncPlan { files_hashed, total_files } => {
                if *total_files > 0 {
                    Some(*files_hashed as f32 / *total_files as f32)
                } else {
                    None
                }
            }
            ExportPhase::Exporting { bytes_complete, total_bytes, .. } => {
                if *total_bytes > 0 {
                    Some(*bytes_complete as f32 / *total_bytes as f32)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get ETA string for export
    pub fn eta_string(&self) -> Option<String> {
        if let ExportPhase::Exporting {
            bytes_complete,
            total_bytes,
            start_time,
            ..
        } = &self.phase
        {
            if *bytes_complete > 0 {
                let elapsed = start_time.elapsed();
                let rate = *bytes_complete as f64 / elapsed.as_secs_f64();
                let remaining_bytes = total_bytes - bytes_complete;
                let eta_secs = remaining_bytes as f64 / rate;

                if eta_secs < 60.0 {
                    Some(format!("ETA: {:.0}s", eta_secs))
                } else {
                    let mins = (eta_secs / 60.0).floor();
                    let secs = (eta_secs % 60.0).floor();
                    Some(format!("ETA: {}m {}s", mins as u32, secs as u32))
                }
            } else {
                Some("Calculating...".to_string())
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let state = ExportState::default();
        assert!(!state.is_open);
        assert!(state.devices.is_empty());
        assert!(state.selected_playlists.is_empty());
        assert!(state.export_config);
    }

    #[test]
    fn test_playlist_selection() {
        let mut state = ExportState::default();
        let id = NodeId("playlists/Test".to_string());

        assert!(!state.is_playlist_selected(&id));
        state.toggle_playlist(id.clone());
        assert!(state.is_playlist_selected(&id));
        state.toggle_playlist(id.clone());
        assert!(!state.is_playlist_selected(&id));
    }

    #[test]
    fn test_phase_status() {
        let phase = ExportPhase::BuildingSyncPlan {
            files_hashed: 5,
            total_files: 10,
        };
        assert!(phase.status_message().contains("5/10"));
    }
}
