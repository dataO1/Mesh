//! Cross-platform USB device detection using sysinfo
//!
//! This module provides device enumeration and hot-plug monitoring
//! that works on Linux, macOS, and Windows.
//!
//! Uses the `sysinfo` crate to enumerate mounted removable drives
//! and polling to detect device connect/disconnect events.

use super::{FilesystemType, UsbDevice};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use sysinfo::Disks;

/// Enumerate currently connected USB storage devices
///
/// Returns a list of mounted removable storage devices.
/// Only includes devices that are currently mounted and accessible.
pub fn enumerate_devices() -> Result<Vec<UsbDevice>, Box<dyn std::error::Error + Send + Sync>> {
    let disks = Disks::new_with_refreshed_list();
    let mut devices = Vec::new();

    for disk in disks.list() {
        // Only include removable drives (USB sticks, external drives)
        if !disk.is_removable() {
            continue;
        }

        // Get filesystem type
        let fs_str = disk.file_system().to_string_lossy();
        let filesystem = FilesystemType::from_str(&fs_str);

        // Skip unsupported filesystems
        if matches!(filesystem, FilesystemType::Unknown) {
            continue;
        }

        let mount_point = disk.mount_point().to_path_buf();
        let has_mesh_collection = mount_point.join("mesh-collection").exists();

        // Get label - use disk name, falling back to mount point name
        let label = {
            let name = disk.name().to_string_lossy().to_string();
            if name.is_empty() {
                mount_point
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "USB Drive".to_string())
            } else {
                name
            }
        };

        devices.push(UsbDevice {
            // On cross-platform, we use mount_point as the identifier
            // (device_path like /dev/sdb1 is Linux-specific)
            device_path: mount_point.clone(),
            label,
            mount_point: Some(mount_point),
            filesystem,
            capacity_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            has_mesh_collection,
        });
    }

    Ok(devices)
}

/// Monitor for USB device connect/disconnect events using polling
///
/// This function blocks and should be run in a dedicated thread.
/// It polls for device changes at the specified interval and calls the callback.
///
/// # Arguments
/// * `callback` - Called when devices are added or removed
/// * `poll_interval` - How often to check for changes (default: 2 seconds)
pub fn monitor_devices<F>(mut callback: F) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnMut(DeviceEvent) + Send,
{
    monitor_devices_with_interval(Duration::from_secs(2), &mut callback)
}

/// Monitor for USB device events with a custom poll interval
pub fn monitor_devices_with_interval<F>(
    poll_interval: Duration,
    callback: &mut F,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnMut(DeviceEvent) + Send,
{
    // Get initial device state
    let mut known_devices: HashSet<PathBuf> = enumerate_devices()
        .ok()
        .map(|devices| {
            devices
                .into_iter()
                .map(|d| d.mount_point.unwrap_or(d.device_path))
                .collect()
        })
        .unwrap_or_default();

    log::info!(
        "USB monitor started with {} initial devices",
        known_devices.len()
    );

    loop {
        std::thread::sleep(poll_interval);

        // Re-enumerate devices
        let current_devices = match enumerate_devices() {
            Ok(devices) => devices,
            Err(e) => {
                log::warn!("Failed to enumerate USB devices: {}", e);
                continue;
            }
        };

        let current_paths: HashSet<PathBuf> = current_devices
            .iter()
            .map(|d| d.mount_point.clone().unwrap_or(d.device_path.clone()))
            .collect();

        // Check for new devices
        for device in &current_devices {
            let path = device.mount_point.clone().unwrap_or(device.device_path.clone());
            if !known_devices.contains(&path) {
                log::info!("USB device connected: {}", device.label);
                callback(DeviceEvent::Added(device.clone()));
            }
        }

        // Check for removed devices
        for path in &known_devices {
            if !current_paths.contains(path) {
                log::info!("USB device disconnected: {}", path.display());
                callback(DeviceEvent::Removed(path.clone()));
            }
        }

        known_devices = current_paths;
    }
}

/// Device event for hot-plug monitoring
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// A new USB device was connected (mounted)
    Added(UsbDevice),
    /// A USB device was disconnected (unmounted)
    Removed(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_devices() {
        // This test just checks that enumeration doesn't panic
        let result = enumerate_devices();
        assert!(result.is_ok());
        let devices = result.unwrap();
        println!("Found {} removable USB devices", devices.len());
        for device in &devices {
            println!(
                "  - {} ({}) at {:?}",
                device.label,
                device.filesystem.display_name(),
                device.mount_point
            );
        }
    }

    #[test]
    fn test_get_mount_info() {
        // Test that we can get disk info without panicking
        let disks = Disks::new_with_refreshed_list();
        for disk in disks.list() {
            println!(
                "Disk: {:?}, removable: {}, fs: {:?}",
                disk.name(),
                disk.is_removable(),
                disk.file_system()
            );
        }
    }
}
