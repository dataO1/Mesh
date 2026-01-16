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
    build_sync_plan, copy_with_verification, scan_local_collection, scan_usb_collection,
    CollectionState, SyncPlan,
};
use super::{ExportableConfig, UsbDevice, UsbError};
use crate::audio_file::read_metadata;
use crate::playlist::NodeId;
use walkdir::WalkDir;

// Re-export for convenience
pub use super::message::{UsbCommand, UsbMessage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
}

impl UsbManager {
    /// Spawn the USB manager background thread
    ///
    /// Returns a manager instance with channels for communication.
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = channel::<UsbCommand>();
        let (message_tx, message_rx) = channel::<UsbMessage>();

        // Clone message_tx for the udev monitor thread
        let monitor_message_tx = message_tx.clone();

        // Spawn the main manager thread
        let thread_handle = thread::Builder::new()
            .name("usb-manager".to_string())
            .spawn(move || {
                manager_thread_main(command_rx, message_tx);
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
fn manager_thread_main(command_rx: Receiver<UsbCommand>, message_tx: Sender<UsbMessage>) {
    log::info!("USB manager thread started");

    // Track known devices
    let mut devices: HashMap<PathBuf, UsbDevice> = HashMap::new();

    // Track export state
    let mut export_cancel_flag: Option<Arc<AtomicBool>> = None;

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
                            &message_tx,
                        );
                    }

                    UsbCommand::StartExport {
                        device_path,
                        plan,
                        include_config,
                        config,
                    } => {
                        let cancel_flag = Arc::new(AtomicBool::new(false));
                        export_cancel_flag = Some(cancel_flag.clone());

                        handle_start_export(
                            &devices,
                            &device_path,
                            plan,
                            include_config,
                            config,
                            cancel_flag,
                            &message_tx,
                        );

                        export_cancel_flag = None;
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
                        if let Some(flag) = &export_cancel_flag {
                            flag.store(true, Ordering::SeqCst);
                            let _ = message_tx.send(UsbMessage::ExportCancelled);
                        }
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
                files_hashed: current,
                total_files: total,
            });
        });

    // Scan local collection (parallel hashing)
    let local_state = match scan_local_collection(
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
                        files_hashed: current,
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
/// Executes the sync plan: copies tracks, creates/removes symlinks, cleans up.
/// Uses parallel file copying with limited thread pool for USB I/O.
fn handle_start_export(
    devices: &HashMap<PathBuf, UsbDevice>,
    device_path: &PathBuf,
    plan: SyncPlan,
    include_config: bool,
    config: Option<ExportableConfig>,
    cancel_flag: Arc<AtomicBool>,
    message_tx: &Sender<UsbMessage>,
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

    let _ = message_tx.send(UsbMessage::ExportStarted {
        total_files: plan.tracks_to_copy.len(),
        total_bytes: plan.total_bytes,
    });

    let start_time = Instant::now();
    let mut files_exported = 0usize;
    let mut bytes_complete = 0u64;
    let mut failed_files: Vec<(PathBuf, String)> = Vec::new();
    let mut copied_tracks: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Phase 1: Copy tracks (parallel with limited threads for USB I/O)
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4) // USB is sequential; too many threads = thrashing
        .build()
        .unwrap();

    let total_tracks = plan.tracks_to_copy.len();
    pool.install(|| {
        use rayon::prelude::*;
        use std::sync::atomic::AtomicU64;

        let files_complete = std::sync::atomic::AtomicUsize::new(0);
        let bytes_done = AtomicU64::new(0);
        let failed = std::sync::Mutex::new(Vec::new());
        let copied = std::sync::Mutex::new(std::collections::HashSet::new());

        plan.tracks_to_copy.par_iter().for_each(|track| {
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }

            let dest_path = collection_root.join(&track.destination);

            match copy_with_verification(&track.source, &dest_path, track.size, 3) {
                Ok(()) => {
                    let current_files = files_complete.fetch_add(1, Ordering::Relaxed) + 1;
                    let current_bytes =
                        bytes_done.fetch_add(track.size, Ordering::Relaxed) + track.size;

                    // Track which files were copied successfully
                    if let Some(filename) = track.destination.file_name() {
                        copied
                            .lock()
                            .unwrap()
                            .insert(filename.to_string_lossy().to_string());
                    }

                    let _ = message_tx.send(UsbMessage::ExportProgress {
                        current_file: track
                            .source
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                            .to_string(),
                        files_complete: current_files,
                        bytes_complete: current_bytes,
                        total_files: total_tracks,
                        total_bytes: plan.total_bytes,
                    });
                }
                Err(e) => {
                    failed
                        .lock()
                        .unwrap()
                        .push((track.destination.clone(), e.to_string()));
                }
            }
        });

        files_exported = files_complete.load(Ordering::Relaxed);
        bytes_complete = bytes_done.load(Ordering::Relaxed);
        failed_files = failed.into_inner().unwrap();
        copied_tracks = copied.into_inner().unwrap();
    });

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = message_tx.send(UsbMessage::ExportCancelled);
        return;
    }

    // Phase 2: Delete tracks that are no longer needed
    let tracks_dir = collection_root.join("tracks");
    for filename in &plan.tracks_to_delete {
        let track_path = tracks_dir.join(filename);
        if track_path.exists() {
            if let Err(e) = std::fs::remove_file(&track_path) {
                log::warn!("Failed to delete track {}: {}", track_path.display(), e);
            }
        }
    }

    // Phase 3: Create playlist directories
    let playlists_dir = collection_root.join("playlists");
    for playlist_name in &plan.dirs_to_create {
        let playlist_dir = playlists_dir.join(playlist_name);
        if let Err(e) = std::fs::create_dir_all(&playlist_dir) {
            log::error!(
                "Failed to create playlist dir {}: {}",
                playlist_dir.display(),
                e
            );
        }
    }

    // Phase 4: Remove symlinks that are no longer needed
    for link in &plan.symlinks_to_remove {
        let link_path = playlists_dir.join(&link.playlist).join(&link.track_filename);
        if link_path.exists() {
            if let Err(e) = std::fs::remove_file(&link_path) {
                log::warn!("Failed to remove symlink {}: {}", link_path.display(), e);
            }
        }
    }

    // Phase 5: Create new symlinks (or copies for FAT filesystems)
    for link in &plan.symlinks_to_add {
        let playlist_dir = playlists_dir.join(&link.playlist);

        // Ensure playlist directory exists
        if !playlist_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&playlist_dir) {
                log::error!(
                    "Failed to create playlist dir {}: {}",
                    playlist_dir.display(),
                    e
                );
                continue;
            }
        }

        let link_path = playlist_dir.join(&link.track_filename);

        // Skip if link already exists
        if link_path.exists() {
            continue;
        }

        if device.supports_symlinks() {
            // Calculate relative path depth based on playlist nesting
            // playlists/Foo -> ../../tracks (depth 1)
            // playlists/Foo/Bar -> ../../../tracks (depth 2)
            let depth = link.playlist.matches('/').count() + 1;
            let mut target = PathBuf::new();
            for _ in 0..=depth {
                target.push("..");
            }
            target.push("tracks");
            target.push(&link.track_filename);

            // Use cross-platform symlink crate
            if let Err(e) = symlink::symlink_file(&target, &link_path) {
                log::error!("Failed to create symlink {}: {}", link_path.display(), e);
            }
        } else {
            // For FAT/exFAT, copy the file (no symlink support)
            let track_path = tracks_dir.join(&link.track_filename);
            if track_path.exists() {
                if let Err(e) = std::fs::copy(&track_path, &link_path) {
                    log::error!("Failed to copy to playlist {}: {}", link_path.display(), e);
                }
            }
        }
    }

    // Phase 6: Delete empty playlist directories
    for playlist_name in &plan.dirs_to_delete {
        let playlist_dir = playlists_dir.join(playlist_name);
        if playlist_dir.exists() {
            // Only delete if empty
            if let Ok(mut entries) = std::fs::read_dir(&playlist_dir) {
                if entries.next().is_none() {
                    if let Err(e) = std::fs::remove_dir(&playlist_dir) {
                        log::warn!(
                            "Failed to remove playlist dir {}: {}",
                            playlist_dir.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    // Phase 7: Save config if requested
    if include_config {
        if let Some(cfg) = config {
            if let Some(config_path) = device.config_path() {
                if let Err(e) = cfg.save(&config_path) {
                    log::error!("Failed to save config: {}", e);
                }
            }
        }
    }

    // No manifest to save - USB filesystem is the source of truth

    let duration = start_time.elapsed();
    let _ = message_tx.send(UsbMessage::ExportComplete {
        duration,
        files_exported,
        failed_files,
    });
}

/// Handle preload metadata command (runs in background thread)
fn handle_preload_metadata(tracks_dir: PathBuf, device_path: PathBuf, tx: Sender<UsbMessage>) {
    log::info!(
        "Preloading track metadata from {}",
        tracks_dir.display()
    );

    // Collect all WAV files
    let entries: Vec<_> = if tracks_dir.exists() {
        WalkDir::new(&tracks_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("wav"))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        Vec::new()
    };

    let total = entries.len();
    let mut metadata = HashMap::new();

    for (i, entry) in entries.iter().enumerate() {
        let path = entry.path();
        if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
            // Read metadata from the WAV file
            if let Ok(meta) = read_metadata(path) {
                let cached = CachedTrackMetadata {
                    artist: meta.artist,
                    bpm: meta.bpm,
                    key: meta.key,
                    duration_seconds: meta.duration_seconds,
                    cue_count: meta.cue_points.len() as u8,
                };
                metadata.insert(filename.to_string(), cached);
            }
        }

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
        let _manager = UsbManager::spawn();
        // Manager will be dropped and threads will be detached
    }
}
