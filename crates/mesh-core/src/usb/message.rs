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
    /// Sync plan calculation started (scanning files)
    SyncPlanStarted,

    /// Progress during sync planning (scanning local files)
    SyncPlanProgress {
        /// Files scanned so far
        files_scanned: usize,
        /// Total files to scan
        total_files: usize,
    },

    /// Sync plan ready for user confirmation
    SyncPlanReady(SyncPlan),

    /// Sync plan failed
    SyncPlanFailed(UsbError),

    // ─────────────────────────────────────────────────────────────────
    // Export Operations (atomic per-track progress)
    // ─────────────────────────────────────────────────────────────────
    /// Export started
    ExportStarted {
        /// Total tracks to export
        total_tracks: usize,
        /// Total bytes to copy
        total_bytes: u64,
    },

    /// A track export started (WAV copy beginning)
    ExportTrackStarted {
        /// Filename of the track being exported
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
    },

    /// A track was fully exported (WAV copied + DB synced)
    ///
    /// This is the atomic completion signal - only sent after both
    /// the WAV file and all database metadata are written.
    ExportTrackComplete {
        /// Filename that was exported
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
        /// Total tracks in the export
        total_tracks: usize,
        /// Cumulative bytes exported so far
        bytes_complete: u64,
        /// Total bytes to export
        total_bytes: u64,
    },

    /// A track failed to export
    ExportTrackFailed {
        /// Filename that failed
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
        /// Error description
        error: String,
    },

    /// Export completed successfully
    ExportComplete {
        /// How long the export took
        duration: Duration,
        /// Number of tracks exported
        tracks_exported: usize,
        /// Any files that failed (with error messages)
        failed_files: Vec<(String, String)>,
    },

    /// Export failed with error
    ExportError(UsbError),

    /// Export was cancelled by user
    ExportCancelled,

    /// Playlist operations phase started (after all tracks are copied)
    ///
    /// This phase adds/removes tracks from playlists in the USB database.
    ExportPlaylistOpsStarted {
        /// Total number of playlist membership operations
        total_operations: usize,
    },

    /// A playlist operation completed
    ExportPlaylistOpComplete {
        /// Number of operations completed so far
        completed: usize,
        /// Total number of operations
        total: usize,
    },

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
            UsbMessage::SyncPlanProgress { files_scanned, total_files } => {
                format!("Scanning files: {}/{}", files_scanned, total_files)
            }
            UsbMessage::SyncPlanReady(plan) => {
                plan.summary()
            }
            UsbMessage::ExportStarted { total_tracks, .. } => {
                format!("Starting export of {} tracks", total_tracks)
            }
            UsbMessage::ExportTrackStarted { filename, .. } => {
                format!("Exporting: {}", filename)
            }
            UsbMessage::ExportTrackComplete { track_index, total_tracks, .. } => {
                format!("Exported: {}/{}", track_index + 1, total_tracks)
            }
            UsbMessage::ExportTrackFailed { filename, error, .. } => {
                format!("Failed: {} - {}", filename, error)
            }
            UsbMessage::ExportComplete { tracks_exported, duration, failed_files } => {
                if failed_files.is_empty() {
                    format!("Export complete: {} tracks in {:.1}s", tracks_exported, duration.as_secs_f64())
                } else {
                    format!("Export complete: {} tracks, {} failed in {:.1}s", tracks_exported, failed_files.len(), duration.as_secs_f64())
                }
            }
            UsbMessage::ExportError(e) => format!("Export failed: {}", e),
            UsbMessage::ExportCancelled => "Export cancelled".to_string(),
            UsbMessage::ExportPlaylistOpsStarted { total_operations } => {
                format!("Updating {} playlist entries...", total_operations)
            }
            UsbMessage::ExportPlaylistOpComplete { completed, total } => {
                format!("Playlist entries: {}/{}", completed, total)
            }
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
