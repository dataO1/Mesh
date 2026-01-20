//! USB export state
//!
//! Manages the state for the USB export modal and export process.

use mesh_core::playlist::NodeId;
use mesh_core::usb::{SyncPlan, UsbDevice, UsbMessage};
use mesh_widgets::TreeNode;
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
        files_scanned: usize,
        total_files: usize,
    },

    /// Showing sync plan, waiting for user confirmation
    ReadyToSync {
        plan: SyncPlan,
    },

    /// Exporting tracks
    Exporting {
        current_track: String,
        tracks_complete: usize,
        bytes_complete: u64,
        total_tracks: usize,
        total_bytes: u64,
        start_time: Instant,
    },

    /// Export complete
    Complete {
        duration: Duration,
        tracks_exported: usize,
        /// Failed files: (filename, error_message)
        failed_files: Vec<(String, String)>,
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
            ExportPhase::BuildingSyncPlan { files_scanned, total_files } => {
                format!("Calculating changes: {}/{} files", files_scanned, total_files)
            }
            ExportPhase::ReadyToSync { plan } => plan.summary(),
            ExportPhase::Exporting { tracks_complete, total_tracks, .. } => {
                format!("Exporting: {}/{} tracks", tracks_complete, total_tracks)
            }
            ExportPhase::Complete { tracks_exported, failed_files, .. } => {
                if failed_files.is_empty() {
                    format!("Export complete! {} tracks exported", tracks_exported)
                } else {
                    format!(
                        "Export complete with {} error(s). {} tracks exported",
                        failed_files.len(),
                        tracks_exported
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

    /// Expanded playlist nodes in the tree view
    pub expanded_playlists: HashSet<NodeId>,

    /// Current export phase
    pub phase: ExportPhase,

    /// Include config file in export
    pub export_config: bool,

    /// Channel to receive USB messages (from UsbManager)
    /// Note: This is set when export modal opens and manager is initialized
    pub usb_message_rx: Option<Receiver<UsbMessage>>,

    /// Show detailed results after completion
    pub show_results: bool,

    /// Cached sync plan (computed in background during selection)
    /// This is updated automatically when playlists/device selection changes
    pub sync_plan: Option<SyncPlan>,

    /// Whether a sync plan computation is in progress
    pub sync_plan_computing: bool,

    /// Whether export is pending LUFS analysis completion
    /// When true, export will auto-start after reanalysis finishes
    pub pending_lufs_analysis: bool,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            is_open: false,
            devices: Vec::new(),
            selected_device: None,
            selected_playlists: HashSet::new(),
            expanded_playlists: HashSet::new(),
            phase: ExportPhase::SelectDevice,
            export_config: true, // Default to including config
            usb_message_rx: None,
            show_results: false,
            sync_plan: None,
            sync_plan_computing: false,
            pending_lufs_analysis: false,
        }
    }
}

impl ExportState {
    /// Reset state when opening the modal
    pub fn reset(&mut self) {
        self.selected_device = None;
        self.selected_playlists.clear();
        self.expanded_playlists.clear();
        self.phase = ExportPhase::SelectDevice;
        self.show_results = false;
        self.sync_plan = None;
        self.sync_plan_computing = false;
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

    /// Toggle playlist selection with recursive child selection
    ///
    /// When toggling a parent playlist, all children are set to the same state.
    pub fn toggle_playlist_recursive(&mut self, id: NodeId, tree: &[TreeNode<NodeId>]) {
        let new_state = !self.selected_playlists.contains(&id);

        // Apply to this node and all descendants
        self.set_playlist_recursive(&id, new_state, tree);
    }

    /// Set a playlist and all its descendants to a specific selection state
    fn set_playlist_recursive(&mut self, id: &NodeId, selected: bool, tree: &[TreeNode<NodeId>]) {
        // Set the state of this node
        if selected {
            self.selected_playlists.insert(id.clone());
        } else {
            self.selected_playlists.remove(id);
        }

        // Find this node in the tree and recurse to children
        if let Some(node) = Self::find_node(id, tree) {
            for child in &node.children {
                self.set_playlist_recursive(&child.id, selected, tree);
            }
        }
    }

    /// Find a node by ID in the tree (recursive search)
    fn find_node<'a>(id: &NodeId, tree: &'a [TreeNode<NodeId>]) -> Option<&'a TreeNode<NodeId>> {
        for node in tree {
            if &node.id == id {
                return Some(node);
            }
            if let Some(found) = Self::find_node(id, &node.children) {
                return Some(found);
            }
        }
        None
    }

    /// Toggle expand/collapse state for a tree node
    pub fn toggle_playlist_expanded(&mut self, id: NodeId) {
        if self.expanded_playlists.contains(&id) {
            self.expanded_playlists.remove(&id);
        } else {
            self.expanded_playlists.insert(id);
        }
    }

    /// Check if a node is expanded
    pub fn is_playlist_expanded(&self, id: &NodeId) -> bool {
        self.expanded_playlists.contains(id)
    }

    /// Check if all children of a node are selected (for partial checkbox state)
    pub fn all_children_selected(&self, id: &NodeId, tree: &[TreeNode<NodeId>]) -> bool {
        if let Some(node) = Self::find_node(id, tree) {
            if node.children.is_empty() {
                return self.selected_playlists.contains(id);
            }
            node.children
                .iter()
                .all(|child| self.all_children_selected(&child.id, tree))
        } else {
            false
        }
    }

    /// Check if any children of a node are selected (for partial checkbox state)
    pub fn any_children_selected(&self, id: &NodeId, tree: &[TreeNode<NodeId>]) -> bool {
        if let Some(node) = Self::find_node(id, tree) {
            if node.children.is_empty() {
                return self.selected_playlists.contains(id);
            }
            node.children
                .iter()
                .any(|child| self.any_children_selected(&child.id, tree))
        } else {
            false
        }
    }

    /// Check if export can be started
    pub fn can_start_export(&self) -> bool {
        self.selected_device.is_some()
            && !self.selected_playlists.is_empty()
            && self.sync_plan.is_some()
            && !self.sync_plan_computing
            && matches!(self.phase, ExportPhase::SelectDevice)
    }

    /// Check if cancel is available
    pub fn can_cancel(&self) -> bool {
        self.phase.is_exporting()
    }

    /// Get export progress (0.0 - 1.0)
    pub fn progress(&self) -> Option<f32> {
        match &self.phase {
            ExportPhase::BuildingSyncPlan { files_scanned, total_files } => {
                if *total_files > 0 {
                    Some(*files_scanned as f32 / *total_files as f32)
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
            files_scanned: 5,
            total_files: 10,
        };
        assert!(phase.status_message().contains("5/10"));
    }

    #[test]
    fn test_exporting_phase_status() {
        let phase = ExportPhase::Exporting {
            current_track: "test.wav".to_string(),
            tracks_complete: 3,
            bytes_complete: 1000,
            total_tracks: 10,
            total_bytes: 5000,
            start_time: Instant::now(),
        };
        assert!(phase.status_message().contains("3/10"));
    }
}
