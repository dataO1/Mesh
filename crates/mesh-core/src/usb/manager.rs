//! USB Manager - Background thread for all USB operations
//!
//! This module provides a non-blocking interface to USB operations.
//! All heavy work (device detection, mounting, file copying, hashing)
//! is performed in background threads.
//!
//! # Architecture
//!
//! ```text
//! UI Thread
//!     │
//!     │ UsbCommand (mpsc channel)
//!     ▼
//! Manager Thread
//!     ├── udev monitor (device events)
//!     ├── mount operations
//!     └── rayon pool (hashing, copying)
//!            │
//!            │ UsbMessage (mpsc channel)
//!            ▼
//!        UI Thread (via subscription)
//! ```

use super::detection::{enumerate_devices, monitor_devices, DeviceEvent};
use super::mount::{init_collection_structure, refresh_device_info};
use super::storage::{CachedTrackMetadata, UsbStorage};
use super::sync::{
    build_sync_plan, scan_local_collection_from_db, scan_usb_collection,
    CollectionState, SyncPlan,
};
use crate::db::DatabaseService;
use crate::export::{ExportProgress, ExportService};
use super::{ExportableConfig, UsbDevice, UsbError};
use crate::playlist::NodeId;

// Re-export for convenience
pub use super::message::{UsbCommand, UsbMessage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Manages USB operations in a background thread
///
/// Create with `UsbManager::spawn()` and communicate via channels.
pub struct UsbManager {
    /// Send commands to the manager thread
    command_tx: Sender<UsbCommand>,
    /// Receive messages from the manager thread (wrapped for subscription use)
    message_rx: Arc<Mutex<Receiver<UsbMessage>>>,
    /// Handle to the manager thread (for shutdown)
    _thread_handle: JoinHandle<()>,
    /// Handle to the udev monitor thread
    _monitor_handle: JoinHandle<()>,
    /// Shared database service (optional - sync operations require it)
    /// Stored to keep Arc alive - actual usage is via clone passed to manager thread
    #[allow(dead_code)]
    db_service: Option<Arc<DatabaseService>>,
}

impl UsbManager {
    /// Spawn the USB manager background thread
    ///
    /// # Arguments
    /// * `db_service` - Optional shared database service for sync operations.
    ///                  If None, sync/export operations will fail gracefully.
    ///
    /// Returns a manager instance with channels for communication.
    pub fn spawn(db_service: Option<Arc<DatabaseService>>) -> Self {
        let (command_tx, command_rx) = channel::<UsbCommand>();
        let (message_tx, message_rx) = channel::<UsbMessage>();

        // Clone message_tx for the udev monitor thread
        let monitor_message_tx = message_tx.clone();

        // Clone db_service for the manager thread
        let thread_db_service = db_service.clone();

        // Spawn the main manager thread
        let thread_handle = thread::Builder::new()
            .name("usb-manager".to_string())
            .spawn(move || {
                manager_thread_main(command_rx, message_tx, thread_db_service);
            })
            .expect("Failed to spawn USB manager thread");

        // Spawn the udev monitor thread
        let monitor_handle = thread::Builder::new()
            .name("usb-monitor".to_string())
            .spawn(move || {
                udev_monitor_thread(monitor_message_tx);
            })
            .expect("Failed to spawn USB monitor thread");

        Self {
            command_tx,
            message_rx: Arc::new(Mutex::new(message_rx)),
            _thread_handle: thread_handle,
            _monitor_handle: monitor_handle,
            db_service,
        }
    }

    /// Send a command to the manager (non-blocking)
    pub fn send(&self, cmd: UsbCommand) -> Result<(), std::sync::mpsc::SendError<UsbCommand>> {
        self.command_tx.send(cmd)
    }

    /// Try to receive a message (non-blocking)
    pub fn try_recv(&self) -> Option<UsbMessage> {
        self.message_rx.lock().ok().and_then(|rx| rx.try_recv().ok())
    }

    /// Get the message receiver for use with iced subscriptions
    ///
    /// Returns an Arc<Mutex<Receiver>> that can be passed to `mpsc_subscription`.
    /// This enables event-driven message handling in iced apps.
    pub fn message_receiver(&self) -> Arc<Mutex<Receiver<UsbMessage>>> {
        Arc::clone(&self.message_rx)
    }

    /// Request a device list refresh
    pub fn refresh_devices(&self) {
        let _ = self.send(UsbCommand::RefreshDevices);
    }

    /// Request mounting a device
    pub fn mount(&self, device_path: PathBuf) {
        let _ = self.send(UsbCommand::Mount { device_path });
    }

    /// Request unmounting a device
    pub fn unmount(&self, device_path: PathBuf) {
        let _ = self.send(UsbCommand::Unmount { device_path });
    }

    /// Shutdown the manager
    pub fn shutdown(&self) {
        let _ = self.send(UsbCommand::Shutdown);
    }
}

/// Main manager thread function
fn manager_thread_main(
    command_rx: Receiver<UsbCommand>,
    message_tx: Sender<UsbMessage>,
    db_service: Option<Arc<DatabaseService>>,
) {
    log::info!("USB manager thread started");

    // Track known devices
    let mut devices: HashMap<PathBuf, UsbDevice> = HashMap::new();

    // Initial device enumeration
    if let Ok(initial_devices) = enumerate_devices() {
        for device in initial_devices {
            devices.insert(device.device_path.clone(), device);
        }
        let device_list: Vec<UsbDevice> = devices.values().cloned().collect();
        let _ = message_tx.send(UsbMessage::DevicesRefreshed(device_list));
    }

    loop {
        // Wait for commands with timeout (allows periodic tasks)
        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => {
                match command {
                    UsbCommand::RefreshDevices => {
                        handle_refresh_devices(&mut devices, &message_tx);
                    }

                    UsbCommand::Mount { device_path } => {
                        handle_mount(&mut devices, &device_path, &message_tx);
                    }

                    UsbCommand::Unmount { device_path } => {
                        handle_unmount(&mut devices, &device_path, &message_tx);
                    }

                    UsbCommand::ScanPlaylists { device_path } => {
                        handle_scan_playlists(&devices, &device_path, &message_tx);
                    }

                    UsbCommand::BuildSyncPlan {
                        device_path,
                        playlists,
                        local_collection_root,
                    } => {
                        handle_build_sync_plan(
                            &devices,
                            &device_path,
                            &playlists,
                            &local_collection_root,
                            db_service.as_ref(),
                            &message_tx,
                        );
                    }

                    UsbCommand::StartExport {
                        device_path,
                        plan,
                        include_config,
                        config,
                    } => {
                        // Note: ExportService runs in its own thread pool with internal cancellation.
                        // The handle_start_export call blocks while forwarding progress messages.
                        handle_start_export(
                            &devices,
                            &device_path,
                            plan,
                            include_config,
                            config,
                            &message_tx,
                            db_service.as_ref(),
                        );
                    }

                    UsbCommand::PreloadMetadata { device_path } => {
                        // Find the device and get mount point
                        if let Some(device) = devices.get(&device_path) {
                            if let Some(mount_point) = &device.mount_point {
                                let tracks_dir = mount_point.join("mesh-collection").join("tracks");
                                let tx = message_tx.clone();
                                let dp = device_path.clone();

                                // Spawn background thread for preloading
                                thread::spawn(move || {
                                    handle_preload_metadata(tracks_dir, dp, tx);
                                });
                            }
                        }
                    }

                    UsbCommand::CancelExport => {
                        // Note: Cancellation during export is not currently supported
                        // because handle_start_export blocks while forwarding messages.
                        // To implement proper cancellation, would need to:
                        // 1. Store a reference to ExportService
                        // 2. Call export_service.cancel() here
                        // 3. Run progress forwarding in a separate thread
                        log::warn!("CancelExport received but export may already be complete");
                        let _ = message_tx.send(UsbMessage::ExportCancelled);
                    }

                    UsbCommand::Shutdown => {
                        log::info!("USB manager shutting down");
                        let _ = message_tx.send(UsbMessage::Shutdown);
                        break;
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Periodic tasks can go here (e.g., refresh device info)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                log::info!("USB manager command channel disconnected");
                break;
            }
        }
    }

    log::info!("USB manager thread exiting");
}

/// Handle device refresh command
fn handle_refresh_devices(
    devices: &mut HashMap<PathBuf, UsbDevice>,
    message_tx: &Sender<UsbMessage>,
) {
    match enumerate_devices() {
        Ok(new_devices) => {
            devices.clear();
            for device in new_devices {
                devices.insert(device.device_path.clone(), device);
            }
            let device_list: Vec<UsbDevice> = devices.values().cloned().collect();
            let _ = message_tx.send(UsbMessage::DevicesRefreshed(device_list));
        }
        Err(e) => {
            log::error!("Failed to enumerate devices: {}", e);
        }
    }
}

/// Handle mount command
///
/// On cross-platform, we rely on OS auto-mount. This just refreshes device info
/// to check if the device is currently mounted.
fn handle_mount(
    devices: &mut HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    message_tx: &Sender<UsbMessage>,
) {
    let _ = message_tx.send(UsbMessage::MountStarted {
        device_path: device_path.clone(),
    });

    let device = match devices.get(device_path) {
        Some(d) => d.clone(),
        None => {
            let _ = message_tx.send(UsbMessage::MountComplete {
                result: Err(UsbError::DeviceNotFound(device_path.display().to_string())),
            });
            return;
        }
    };

    // Refresh device info - if OS has auto-mounted, we'll see it
    let refreshed = refresh_device_info(&device);
    if refreshed.mount_point.is_some() {
        devices.insert(device_path.clone(), refreshed.clone());
        let _ = message_tx.send(UsbMessage::MountComplete {
            result: Ok(refreshed),
        });
    } else {
        let _ = message_tx.send(UsbMessage::MountComplete {
            result: Err(UsbError::MountFailed(
                "Device not mounted. Please ensure the device is connected and mounted by your operating system.".to_string()
            )),
        });
    }
}

/// Handle unmount command
///
/// On cross-platform, unmounting is handled by the OS. We just refresh
/// device info to reflect current state.
fn handle_unmount(
    devices: &mut HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    message_tx: &Sender<UsbMessage>,
) {
    let device = match devices.get(device_path) {
        Some(d) => d.clone(),
        None => {
            let _ = message_tx.send(UsbMessage::UnmountComplete {
                device_path: device_path.clone(),
                result: Err(UsbError::DeviceNotFound(device_path.display().to_string())),
            });
            return;
        }
    };

    // Refresh device info - if it's been unmounted, mount_point will be None
    let refreshed = refresh_device_info(&device);
    devices.insert(device_path.clone(), refreshed);

    // Report success - the device state has been updated
    let _ = message_tx.send(UsbMessage::UnmountComplete {
        device_path: device_path.clone(),
        result: Ok(()),
    });
}

/// Handle playlist scan command
fn handle_scan_playlists(
    devices: &HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    message_tx: &Sender<UsbMessage>,
) {
    let _ = message_tx.send(UsbMessage::PlaylistScanStarted {
        device_path: device_path.clone(),
    });

    let device = match devices.get(device_path) {
        Some(d) => d.clone(),
        None => {
            log::error!("Device not found for playlist scan: {}", device_path.display());
            return;
        }
    };

    // Create storage and scan
    match UsbStorage::for_browsing(device) {
        Ok(storage) => {
            let tree_nodes = storage.all_nodes().clone();
            let config = storage.load_config();
            let _ = message_tx.send(UsbMessage::PlaylistScanComplete {
                device_path: device_path.clone(),
                tree_nodes,
                config,
            });
        }
        Err(e) => {
            log::error!("Failed to scan playlists: {}", e);
        }
    }
}

/// Handle sync plan building
///
/// Scans both local and USB collections, then computes the minimal diff.
/// Uses parallel hashing for performance.
fn handle_build_sync_plan(
    devices: &HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    playlists: &[NodeId],
    local_collection_root: &PathBuf,
    db_service: Option<&Arc<DatabaseService>>,
    message_tx: &Sender<UsbMessage>,
) {
    let _ = message_tx.send(UsbMessage::SyncPlanStarted);

    let device = match devices.get(device_path) {
        Some(d) => d.clone(),
        None => {
            let _ = message_tx.send(UsbMessage::SyncPlanFailed(UsbError::DeviceNotFound(
                device_path.display().to_string(),
            )));
            return;
        }
    };

    // Ensure we have a database service
    let db_service = match db_service {
        Some(s) => s,
        None => {
            let _ = message_tx.send(UsbMessage::SyncPlanFailed(UsbError::IoError(
                "Database service not available for sync".to_string()
            )));
            return;
        }
    };

    // Extract playlist names from NodeIds (e.g., "playlists/My Set" -> "My Set")
    let playlist_names: Vec<String> = playlists
        .iter()
        .filter_map(|id| {
            if id.is_in_playlists() {
                Some(
                    id.as_str()
                        .strip_prefix("playlists/")
                        .unwrap_or(id.as_str())
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect();

    // Progress callback for local scanning
    let tx_local = message_tx.clone();
    let local_progress: super::sync::ProgressCallback =
        Box::new(move |current: usize, total: usize| {
            let _ = tx_local.send(UsbMessage::SyncPlanProgress {
                files_scanned: current,
                total_files: total,
            });
        });

    // Scan local collection from database (reads playlist membership from DB)
    let local_state = match scan_local_collection_from_db(
        db_service.db(),
        local_collection_root,
        &playlist_names,
        Some(local_progress),
    ) {
        Ok(state) => state,
        Err(e) => {
            let _ = message_tx.send(UsbMessage::SyncPlanFailed(UsbError::IoError(e.to_string())));
            return;
        }
    };

    // Scan USB collection if it exists
    let usb_state = if let Some(usb_root) = device.collection_root() {
        if usb_root.exists() {
            // Progress callback for USB scanning
            let tx_usb = message_tx.clone();
            let usb_progress: super::sync::ProgressCallback =
                Box::new(move |current: usize, total: usize| {
                    let _ = tx_usb.send(UsbMessage::SyncPlanProgress {
                        files_scanned: current,
                        total_files: total,
                    });
                });

            match scan_usb_collection(&usb_root, Some(usb_progress)) {
                Ok(state) => state,
                Err(e) => {
                    log::warn!("Failed to scan USB collection: {}", e);
                    CollectionState::default()
                }
            }
        } else {
            CollectionState::default()
        }
    } else {
        CollectionState::default()
    };

    // Build the sync plan by comparing states
    let plan = build_sync_plan(&local_state, &usb_state);
    let _ = message_tx.send(UsbMessage::SyncPlanReady(plan));
}

/// Handle export start
///
/// Delegates to ExportService for atomic per-track exports.
/// Each track export: WAV copy + DB sync + progress callback.
fn handle_start_export(
    devices: &HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    plan: SyncPlan,
    include_config: bool,
    config: Option<ExportableConfig>,
    message_tx: &Sender<UsbMessage>,
    local_db: Option<&Arc<DatabaseService>>,
) {
    let device = match devices.get(device_path) {
        Some(d) => d.clone(),
        None => {
            let _ = message_tx.send(UsbMessage::ExportError(UsbError::DeviceNotFound(
                device_path.display().to_string(),
            )));
            return;
        }
    };

    let collection_root = match device.collection_root() {
        Some(root) => root,
        None => {
            let _ = message_tx.send(UsbMessage::ExportError(UsbError::MountFailed(
                "Device not mounted".to_string(),
            )));
            return;
        }
    };

    // Initialize collection structure
    if let Err(e) = init_collection_structure(&device) {
        let _ = message_tx.send(UsbMessage::ExportError(e));
        return;
    }

    // Get local database reference
    let local_db = match local_db {
        Some(db) => Arc::clone(db),
        None => {
            let _ = message_tx.send(UsbMessage::ExportError(UsbError::IoError(
                "Database service not available for export".to_string(),
            )));
            return;
        }
    };

    // Create export service and start export
    let export_service = ExportService::new();
    let progress_rx = export_service.start_export(plan, local_db, &collection_root);

    // Forward ExportProgress messages to UsbMessage
    // This loop runs until the export is complete or cancelled
    for progress in progress_rx {
        let usb_msg = match progress {
            ExportProgress::Started { total_tracks, total_bytes } => {
                UsbMessage::ExportStarted { total_tracks, total_bytes }
            }
            ExportProgress::TrackStarted { filename, track_index } => {
                UsbMessage::ExportTrackStarted { filename, track_index }
            }
            ExportProgress::TrackComplete {
                filename,
                track_index,
                total_tracks,
                bytes_complete,
                total_bytes,
            } => UsbMessage::ExportTrackComplete {
                filename,
                track_index,
                total_tracks,
                bytes_complete,
                total_bytes,
            },
            ExportProgress::TrackFailed {
                filename,
                track_index,
                error,
            } => UsbMessage::ExportTrackFailed {
                filename,
                track_index,
                error,
            },
            ExportProgress::Complete {
                duration,
                tracks_exported,
                failed_files,
            } => {
                // Save config after successful export
                if include_config {
                    if let Some(cfg) = &config {
                        if let Some(config_path) = device.config_path() {
                            if let Err(e) = cfg.save(&config_path) {
                                log::error!("Failed to save config: {}", e);
                            }
                        }
                    }
                }

                UsbMessage::ExportComplete {
                    duration,
                    tracks_exported,
                    failed_files,
                }
            }
            ExportProgress::Cancelled => UsbMessage::ExportCancelled,
            ExportProgress::PlaylistOpsStarted { total_operations } => {
                UsbMessage::ExportPlaylistOpsStarted { total_operations }
            }
            ExportProgress::PlaylistOpComplete { completed, total } => {
                UsbMessage::ExportPlaylistOpComplete { completed, total }
            }
        };

        if message_tx.send(usb_msg).is_err() {
            // Receiver dropped, stop forwarding
            break;
        }
    }
}

/// Handle preload metadata command (runs in background thread)
fn handle_preload_metadata(tracks_dir: PathBuf, device_path: PathBuf, tx: Sender<UsbMessage>) {
    use crate::db::{TrackQuery, CuePointQuery};
    use super::cache::get_or_open_usb_database;

    log::info!(
        "Preloading track metadata from {}",
        tracks_dir.display()
    );

    // Read metadata from USB's mesh.db (WAV files no longer contain metadata)
    let collection_root = device_path.join("mesh-collection");
    let mut metadata = HashMap::new();

    // Get or open database from centralized cache
    let db_service = match get_or_open_usb_database(&collection_root) {
        Some(db) => db,
        None => {
            log::warn!("Failed to open USB database at {:?}", collection_root);
            let _ = tx.send(UsbMessage::MetadataPreloaded {
                device_path,
                metadata,
            });
            return;
        }
    };

    // Get all tracks from USB database
    if let Ok(tracks) = TrackQuery::get_all(db_service.db()) {
        let total = tracks.len();

        for (i, track) in tracks.iter().enumerate() {
            // Extract filename from path
            let filename = std::path::Path::new(&track.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // Get cue point count for this track
            let cue_count = CuePointQuery::get_for_track(db_service.db(), track.id)
                .map(|cues| cues.len() as u8)
                .unwrap_or(0);

            let cached = CachedTrackMetadata {
                artist: track.artist.clone(),
                bpm: track.bpm,
                key: track.key.clone(),
                duration_seconds: Some(track.duration_seconds),
                cue_count,
                lufs: track.lufs,
            };
            metadata.insert(filename.to_string(), cached);

            // Send progress every 10 tracks (avoid flooding messages)
            if i % 10 == 0 || i == total - 1 {
                let _ = tx.send(UsbMessage::MetadataPreloadProgress {
                    device_path: device_path.clone(),
                    loaded: i + 1,
                    total,
                });
            }
        }

        log::info!(
            "Preloaded metadata for {} tracks from USB database",
            metadata.len()
        );
    } else {
        log::warn!("Failed to read tracks from USB database");
    }

    log::info!(
        "Preloaded metadata for {} tracks from {}",
        metadata.len(),
        device_path.display()
    );

    // Send completion message with all metadata
    let _ = tx.send(UsbMessage::MetadataPreloaded {
        device_path,
        metadata,
    });
}

/// udev monitor thread function
fn udev_monitor_thread(message_tx: Sender<UsbMessage>) {
    log::info!("USB udev monitor thread started");

    let result = monitor_devices(|event| {
        match event {
            DeviceEvent::Added(device) => {
                let _ = message_tx.send(UsbMessage::DeviceConnected(device));
            }
            DeviceEvent::Removed(path) => {
                let _ = message_tx.send(UsbMessage::DeviceDisconnected { device_path: path });
            }
        }
    });

    if let Err(e) = result {
        log::error!("udev monitor error: {}", e);
    }

    log::info!("USB udev monitor thread exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_spawn() {
        // Just test that we can spawn without panicking
        // Actual functionality requires USB hardware
        // Pass None for db_service - sync operations won't work but basic USB ops will
        let _manager = UsbManager::spawn(None);
        // Manager will be dropped and threads will be detached
    }
}
