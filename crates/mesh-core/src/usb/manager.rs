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
use super::mount::{init_collection_structure, mount_device_sync, refresh_device_info};
use super::storage::UsbStorage;
use super::sync::{build_sync_plan, copy_with_verification, SyncPlan, UsbManifest};
use super::{ExportableConfig, UsbDevice, UsbError};
use crate::playlist::NodeId;

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

    match mount_device_sync(&device) {
        Ok(mounted_device) => {
            devices.insert(device_path.clone(), mounted_device.clone());
            let _ = message_tx.send(UsbMessage::MountComplete {
                result: Ok(mounted_device),
            });
        }
        Err(e) => {
            // Try refreshing device info in case it's already mounted
            let refreshed = refresh_device_info(&device);
            if refreshed.mount_point.is_some() {
                devices.insert(device_path.clone(), refreshed.clone());
                let _ = message_tx.send(UsbMessage::MountComplete {
                    result: Ok(refreshed),
                });
            } else {
                let _ = message_tx.send(UsbMessage::MountComplete { result: Err(e) });
            }
        }
    }
}

/// Handle unmount command
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

    match super::mount::unmount_device_sync(&device) {
        Ok(()) => {
            // Update device to unmounted state
            let unmounted = UsbDevice {
                mount_point: None,
                available_bytes: 0,
                has_mesh_collection: false,
                ..device
            };
            devices.insert(device_path.clone(), unmounted);
            let _ = message_tx.send(UsbMessage::UnmountComplete {
                device_path: device_path.clone(),
                result: Ok(()),
            });
        }
        Err(e) => {
            let _ = message_tx.send(UsbMessage::UnmountComplete {
                device_path: device_path.clone(),
                result: Err(e),
            });
        }
    }
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

    // Load existing manifest from USB
    let manifest = device
        .manifest_path()
        .and_then(|p| UsbManifest::load(&p).ok())
        .unwrap_or_default();

    // Build list of tracks to export from selected playlists
    let mut local_tracks: Vec<(PathBuf, PathBuf)> = Vec::new();

    // Read local collection to get track paths
    let _local_playlists_dir = local_collection_root.join("playlists");
    let _local_tracks_dir = local_collection_root.join("tracks");

    for playlist_id in playlists {
        // Get playlist directory
        let playlist_path = if playlist_id.is_in_playlists() {
            local_collection_root.join(playlist_id.as_str())
        } else {
            continue;
        };

        // Scan playlist for tracks
        if let Ok(entries) = std::fs::read_dir(&playlist_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let entry_path = entry.path();
                if entry_path.extension().and_then(|e| e.to_str()) == Some("wav") {
                    // Resolve symlink to get actual track path
                    let track_path = if entry_path.is_symlink() {
                        std::fs::read_link(&entry_path)
                            .ok()
                            .map(|link| {
                                if link.is_absolute() {
                                    link
                                } else {
                                    entry_path.parent().unwrap().join(&link)
                                }
                            })
                            .and_then(|p| p.canonicalize().ok())
                            .unwrap_or(entry_path.clone())
                    } else {
                        entry_path.clone()
                    };

                    // Compute destination path (relative to USB collection)
                    let file_name = entry_path.file_name().unwrap();
                    let dest_tracks = PathBuf::from("tracks").join(file_name);
                    let _dest_playlist = PathBuf::from(playlist_id.as_str()).join(file_name);

                    // Add track to collection
                    if !local_tracks.iter().any(|(_, d)| d == &dest_tracks) {
                        local_tracks.push((track_path.clone(), dest_tracks));
                    }
                }
            }
        }
    }

    // Build sync plan with progress callback
    let tx = message_tx.clone();
    let progress_callback = Box::new(move |current: usize, total: usize| {
        let _ = tx.send(UsbMessage::SyncPlanProgress {
            files_hashed: current,
            total_files: total,
        });
    });

    match build_sync_plan(local_tracks, &manifest, Some(progress_callback)) {
        Ok(plan) => {
            let _ = message_tx.send(UsbMessage::SyncPlanReady(plan));
        }
        Err(e) => {
            let _ = message_tx.send(UsbMessage::SyncPlanFailed(UsbError::IoError(e.to_string())));
        }
    }
}

/// Handle export start
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
        total_files: plan.to_copy.len(),
        total_bytes: plan.total_bytes,
    });

    let start_time = Instant::now();
    let mut files_exported = 0usize;
    let mut bytes_complete = 0u64;
    let mut failed_files: Vec<(PathBuf, String)> = Vec::new();

    // Use rayon for parallel copying (but limit threads to not saturate USB)
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();

    pool.install(|| {
        use rayon::prelude::*;
        use std::sync::atomic::AtomicU64;

        let files_complete = std::sync::atomic::AtomicUsize::new(0);
        let bytes_done = AtomicU64::new(0);
        let failed = std::sync::Mutex::new(Vec::new());

        plan.to_copy.par_iter().for_each(|file| {
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }

            let dest_path = collection_root.join(&file.destination);

            match copy_with_verification(&file.source, &dest_path, &file.hash, 3) {
                Ok(()) => {
                    let current_files = files_complete.fetch_add(1, Ordering::Relaxed) + 1;
                    let current_bytes = bytes_done.fetch_add(file.size, Ordering::Relaxed) + file.size;

                    let _ = message_tx.send(UsbMessage::ExportProgress {
                        current_file: file.source.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                            .to_string(),
                        files_complete: current_files,
                        bytes_complete: current_bytes,
                        total_files: plan.to_copy.len(),
                        total_bytes: plan.total_bytes,
                    });
                }
                Err(e) => {
                    failed.lock().unwrap().push((file.destination.clone(), e.to_string()));
                }
            }
        });

        files_exported = files_complete.load(Ordering::Relaxed);
        bytes_complete = bytes_done.load(Ordering::Relaxed);
        failed_files = failed.into_inner().unwrap();
    });

    if cancel_flag.load(Ordering::Relaxed) {
        let _ = message_tx.send(UsbMessage::ExportCancelled);
        return;
    }

    // Save config if requested
    if include_config {
        if let Some(cfg) = config {
            if let Some(config_path) = device.config_path() {
                if let Err(e) = cfg.save(&config_path) {
                    log::error!("Failed to save config: {}", e);
                }
            }
        }
    }

    // Update manifest
    let mut manifest = device
        .manifest_path()
        .and_then(|p| UsbManifest::load(&p).ok())
        .unwrap_or_default();

    for file in &plan.to_copy {
        if !failed_files.iter().any(|(p, _)| p == &file.destination) {
            manifest.files.insert(file.destination.clone(), file.hash.clone());
        }
    }

    manifest.exported_at = std::time::SystemTime::now();

    if let Some(manifest_path) = device.manifest_path() {
        if let Err(e) = manifest.save(&manifest_path) {
            log::error!("Failed to save manifest: {}", e);
        }
    }

    let duration = start_time.elapsed();
    let _ = message_tx.send(UsbMessage::ExportComplete {
        duration,
        files_exported,
        failed_files,
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
