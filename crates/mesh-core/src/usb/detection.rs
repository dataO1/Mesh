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

/// Get a user-friendly label for a disk device.
///
/// On Linux, `sysinfo::Disk::name()` returns the kernel device name (e.g. "sda")
/// rather than the volume label. We resolve a proper name via:
///   1. Filesystem label from /dev/disk/by-label/ symlinks
///   2. Hardware model name from /sys/block/{dev}/device/model
///   3. Fallback: /dev/{name}
///
/// On macOS/Windows, `disk.name()` already returns the volume label,
/// so we use it directly with a mount-point-basename fallback.
#[cfg(target_os = "linux")]
fn get_device_label(device_name: &str, _mount_point: &std::path::Path) -> String {
    // Try filesystem label from /dev/disk/by-label/
    if let Some(label) = get_fs_label(device_name) {
        return label;
    }
    // Try hardware model from sysfs
    if let Some(model) = get_device_model(device_name) {
        return model;
    }
    format!("/dev/{}", device_name)
}

#[cfg(not(target_os = "linux"))]
fn get_device_label(device_name: &str, mount_point: &std::path::Path) -> String {
    if !device_name.is_empty() {
        return device_name.to_string();
    }
    mount_point
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "USB Drive".to_string())
}

/// Look up filesystem label via /dev/disk/by-label/ symlinks.
///
/// Each entry is a symlink named after the label, pointing to the device node.
/// e.g. `/dev/disk/by-label/SANDISK` → `../../sda1`
#[cfg(target_os = "linux")]
fn get_fs_label(device_name: &str) -> Option<String> {
    let by_label = std::path::Path::new("/dev/disk/by-label");
    let entries = std::fs::read_dir(by_label).ok()?;
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path()) {
            let target_name = target.file_name()?.to_string_lossy().to_string();
            if target_name == device_name {
                return Some(entry.file_name().to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Read the hardware model name from sysfs.
///
/// The model is at `/sys/block/{parent_dev}/device/model`.
/// For partitions like `sda1`, strips the trailing digits to get the parent `sda`.
#[cfg(target_os = "linux")]
fn get_device_model(device_name: &str) -> Option<String> {
    // Strip partition number: sda1 → sda, sdb2 → sdb
    // (whole-disk devices like sda are unchanged)
    let parent = device_name.trim_end_matches(|c: char| c.is_ascii_digit());
    let model_path = format!("/sys/block/{}/device/model", parent);
    let model = std::fs::read_to_string(model_path).ok()?;
    let model = model.trim().to_string();
    if model.is_empty() { None } else { Some(model) }
}

/// Enumerate currently connected USB storage devices
///
/// Returns a list of mounted removable storage devices.
/// Only includes devices that are currently mounted and accessible.
pub fn enumerate_devices() -> Result<Vec<UsbDevice>, Box<dyn std::error::Error + Send + Sync>> {
    let disks = Disks::new_with_refreshed_list();
    let mut devices = Vec::new();

    log::debug!("Enumerating disks: {} total from sysinfo", disks.list().len());

    for disk in disks.list() {
        let mount = disk.mount_point();
        let fs_str = disk.file_system().to_string_lossy();
        let removable = disk.is_removable();

        log::debug!(
            "  disk: {:?} mount={} fs={} removable={} total={}",
            disk.name(), mount.display(), fs_str, removable, disk.total_space()
        );

        // Only include removable drives (USB sticks, external drives)
        if !removable {
            continue;
        }

        // Get filesystem type
        let filesystem = FilesystemType::from_str(&fs_str);

        // Skip unsupported filesystems
        if matches!(filesystem, FilesystemType::Unknown) {
            log::debug!("    skipped: unsupported filesystem '{}'", fs_str);
            continue;
        }

        let mount_point = disk.mount_point().to_path_buf();
        let has_mesh_collection = mount_point.join("mesh-collection").exists();

        // disk.name() may return full path ("/dev/sda") or just name ("sda")
        let raw_name = disk.name().to_string_lossy().to_string();
        let device_name = std::path::Path::new(&raw_name)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(raw_name);

        // Get label: filesystem label → device model → /dev/sdX (Linux)
        //            disk name → mount point basename (macOS/Windows)
        let label = get_device_label(&device_name, &mount_point);

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
