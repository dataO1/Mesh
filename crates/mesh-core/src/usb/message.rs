//! USB message types for async communication
//!
//! This module defines the message types used for communication between
//! the UI thread and the USB manager background thread.
//!
//! # Communication Pattern
//!
//! - UI sends `UsbCommand` to the manager thread (non-blocking)
//! - Manager sends `UsbMessage` back to UI via channel (polled by iced subscription)
//!
//! This ensures all USB operations are async and never block the UI.

use super::{CachedTrackMetadata, ExportableConfig, SyncPlan, UsbDevice, UsbError};
use crate::playlist::{NodeId, PlaylistNode};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Commands sent FROM UI TO USB manager thread
///
/// These are sent via a channel and processed asynchronously.
/// The UI never blocks waiting for results.
#[derive(Debug, Clone)]
pub enum UsbCommand {
    /// Request a refresh of the device list
    RefreshDevices,

    /// Mount a device by its device path
    Mount { device_path: PathBuf },

    /// Unmount a device
    Unmount { device_path: PathBuf },

    /// Scan playlists on a mounted device
    /// Sends `PlaylistScanComplete` when done
    ScanPlaylists { device_path: PathBuf },

    /// Build a sync plan for exporting playlists
    /// Sends `SyncPlanReady` when done
    BuildSyncPlan {
        device_path: PathBuf,
        /// Local playlists to export
        playlists: Vec<NodeId>,
        /// Local collection root for resolving track paths
        local_collection_root: PathBuf,
    },

    /// Start the export operation
    StartExport {
        device_path: PathBuf,
        /// The sync plan (from BuildSyncPlan result)
        plan: SyncPlan,
        /// Include config file in export
        include_config: bool,
        /// Config to export (if include_config is true)
        config: Option<ExportableConfig>,
    },

    /// Cancel the current export operation
    CancelExport,

    /// Preload track metadata for a device (runs in background for instant browsing)
    PreloadMetadata { device_path: PathBuf },

    /// Shutdown the manager thread
    Shutdown,
}

/// Messages sent FROM USB manager thread TO UI
///
/// These are received via iced subscription polling.
#[derive(Debug, Clone)]
pub enum UsbMessage {
    // ─────────────────────────────────────────────────────────────────
    // Device Detection
    // ─────────────────────────────────────────────────────────────────
    /// Full device list refreshed
    DevicesRefreshed(Vec<UsbDevice>),

    /// A new device was connected (hot-plug)
    DeviceConnected(UsbDevice),

    /// A device was disconnected
    DeviceDisconnected {
        /// The device path that was disconnected
        device_path: PathBuf,
    },

    // ─────────────────────────────────────────────────────────────────
    // Mount Operations
    // ─────────────────────────────────────────────────────────────────
    /// Mount operation started
    MountStarted {
        device_path: PathBuf,
    },

    /// Mount operation completed
    MountComplete {
        /// Updated device with mount point set (or error)
        result: Result<UsbDevice, UsbError>,
    },

    /// Unmount operation completed
    UnmountComplete {
        device_path: PathBuf,
        result: Result<(), UsbError>,
    },

    // ─────────────────────────────────────────────────────────────────
    // Playlist Scanning
    // ─────────────────────────────────────────────────────────────────
    /// Playlist scan started
    PlaylistScanStarted {
        device_path: PathBuf,
    },

    /// Playlist scan completed
    PlaylistScanComplete {
        device_path: PathBuf,
        /// The scanned playlist tree
        tree_nodes: HashMap<NodeId, PlaylistNode>,
        /// Config loaded from device (if present)
        config: Option<ExportableConfig>,
    },

    // ─────────────────────────────────────────────────────────────────
    // Metadata Preloading (for instant browsing)
    // ─────────────────────────────────────────────────────────────────
    /// Metadata preload progress
    MetadataPreloadProgress {
        device_path: PathBuf,
        /// Tracks loaded so far
        loaded: usize,
        /// Total tracks to load
        total: usize,
    },

    /// Metadata preload completed
    MetadataPreloaded {
        device_path: PathBuf,
        /// Cached metadata keyed by filename
        metadata: HashMap<String, CachedTrackMetadata>,
    },

    // ─────────────────────────────────────────────────────────────────
    // Sync Planning
    // ─────────────────────────────────────────────────────────────────
    /// Sync plan calculation started (hashing files)
    SyncPlanStarted,

    /// Progress during sync planning (hashing local files)
    SyncPlanProgress {
        /// Files hashed so far
        files_hashed: usize,
        /// Total files to hash
        total_files: usize,
    },

    /// Sync plan ready for user confirmation
    SyncPlanReady(SyncPlan),

    /// Sync plan failed
    SyncPlanFailed(UsbError),

    // ─────────────────────────────────────────────────────────────────
    // Export Operations
    // ─────────────────────────────────────────────────────────────────
    /// Export started
    ExportStarted {
        /// Total files to copy
        total_files: usize,
        /// Total bytes to copy
        total_bytes: u64,
    },

    /// Export progress update
    ExportProgress {
        /// Current file being copied
        current_file: String,
        /// Files completed so far
        files_complete: usize,
        /// Bytes copied so far
        bytes_complete: u64,
        /// Total files to copy
        total_files: usize,
        /// Total bytes to copy
        total_bytes: u64,
    },

    /// A single file was copied successfully
    ExportFileComplete {
        path: PathBuf,
    },

    /// A single file failed to copy
    ExportFileFailed {
        path: PathBuf,
        error: String,
    },

    /// Export completed successfully
    ExportComplete {
        /// How long the export took
        duration: Duration,
        /// Number of files exported
        files_exported: usize,
        /// Any files that failed (with error messages)
        failed_files: Vec<(PathBuf, String)>,
    },

    /// Export failed with error
    ExportError(UsbError),

    /// Export was cancelled by user
    ExportCancelled,

    // ─────────────────────────────────────────────────────────────────
    // General
    // ─────────────────────────────────────────────────────────────────
    /// Manager thread has shut down
    Shutdown,
}

impl UsbMessage {
    /// Check if this message indicates an error
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            UsbMessage::ExportError(_)
                | UsbMessage::SyncPlanFailed(_)
                | UsbMessage::MountComplete { result: Err(_) }
                | UsbMessage::UnmountComplete { result: Err(_), .. }
        )
    }

    /// Get a human-readable description of this message
    pub fn description(&self) -> String {
        match self {
            UsbMessage::DevicesRefreshed(devices) => {
                format!("Found {} USB device(s)", devices.len())
            }
            UsbMessage::DeviceConnected(device) => {
                format!("Device connected: {}", device.label)
            }
            UsbMessage::DeviceDisconnected { device_path } => {
                format!("Device disconnected: {}", device_path.display())
            }
            UsbMessage::MountStarted { .. } => "Mounting device...".to_string(),
            UsbMessage::MountComplete { result: Ok(device) } => {
                format!("Mounted {} at {}", device.label, device.mount_point.as_ref().map(|p| p.display().to_string()).unwrap_or_default())
            }
            UsbMessage::MountComplete { result: Err(e) } => {
                format!("Mount failed: {}", e)
            }
            UsbMessage::PlaylistScanStarted { .. } => "Scanning playlists...".to_string(),
            UsbMessage::PlaylistScanComplete { tree_nodes, .. } => {
                format!("Found {} items", tree_nodes.len())
            }
            UsbMessage::MetadataPreloadProgress { loaded, total, .. } => {
                format!("Preloading metadata: {}/{}", loaded, total)
            }
            UsbMessage::MetadataPreloaded { metadata, .. } => {
                format!("Preloaded {} tracks", metadata.len())
            }
            UsbMessage::SyncPlanStarted => "Calculating sync plan...".to_string(),
            UsbMessage::SyncPlanProgress { files_hashed, total_files } => {
                format!("Hashing files: {}/{}", files_hashed, total_files)
            }
            UsbMessage::SyncPlanReady(plan) => {
                plan.summary()
            }
            UsbMessage::ExportStarted { total_files, .. } => {
                format!("Starting export of {} files", total_files)
            }
            UsbMessage::ExportProgress { files_complete, total_files, .. } => {
                format!("Exporting: {}/{}", files_complete, total_files)
            }
            UsbMessage::ExportComplete { files_exported, duration, .. } => {
                format!("Export complete: {} files in {:.1}s", files_exported, duration.as_secs_f64())
            }
            UsbMessage::ExportError(e) => format!("Export failed: {}", e),
            UsbMessage::ExportCancelled => "Export cancelled".to_string(),
            _ => format!("{:?}", self),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_is_error() {
        let error_msg = UsbMessage::ExportError(UsbError::DeviceDisconnected);
        assert!(error_msg.is_error());

        let ok_msg = UsbMessage::DevicesRefreshed(vec![]);
        assert!(!ok_msg.is_error());
    }
}
