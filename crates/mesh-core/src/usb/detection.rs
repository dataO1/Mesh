//! USB device detection using sysfs and /proc
//!
//! This module provides device enumeration and hot-plug monitoring
//! without requiring libudev. It uses:
//! - /sys/block for device enumeration
//! - /proc/mounts for mount status
//! - inotify for hot-plug events (watching /dev/disk/by-id)

use super::{FilesystemType, UsbDevice};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// Enumerate currently connected USB storage devices
///
/// Returns a list of USB mass storage devices with their properties.
/// Devices may or may not be mounted.
pub fn enumerate_devices() -> Result<Vec<UsbDevice>, Box<dyn std::error::Error + Send + Sync>> {
    let mut devices = Vec::new();

    // Read /proc/partitions to find block devices
    let partitions = fs::read_to_string("/proc/partitions")?;

    for line in partitions.lines().skip(2) {
        // Skip header lines
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }

        let name = parts[3];

        // Skip loop/ram/dm devices
        if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("dm-") {
            continue;
        }

        let device_path = PathBuf::from(format!("/dev/{}", name));

        // Check if this is a USB device by examining sysfs
        if !is_usb_device(&device_path) {
            continue;
        }

        // For whole-disk devices (no digit suffix like "sda"), check if they have partitions
        // This handles USB drives formatted without a partition table (filesystem directly on device)
        let is_partition = name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false);
        if !is_partition {
            // Get base device name for partition check
            let base_name: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
            // Whole-disk device - skip if it has partitions (we'll enumerate those instead)
            if has_partitions(&base_name) {
                continue;
            }
        }

        // Get device info (includes filesystem check - returns None for unsupported fs)
        if let Some(device) = get_device_info(&device_path) {
            devices.push(device);
        }
    }

    Ok(devices)
}

/// Check if a block device has partitions (e.g., sda has sda1, sda2, etc.)
///
/// Used to avoid listing both the base device and its partitions.
fn has_partitions(device_name: &str) -> bool {
    let sysfs_dir = PathBuf::from(format!("/sys/block/{}", device_name));
    if let Ok(entries) = fs::read_dir(&sysfs_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            // Partitions are named like "sda1", "sda2", etc.
            if name.starts_with(device_name) && name.len() > device_name.len() {
                return true;
            }
        }
    }
    false
}

/// Check if a device is a USB device by examining sysfs
fn is_usb_device(device_path: &PathBuf) -> bool {
    let name = match device_path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };

    // Get the base device name (e.g., "sdb" from "sdb1")
    let base_name: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();

    // Check sysfs for USB subsystem
    let sysfs_path = PathBuf::from(format!("/sys/block/{}/device", base_name));

    if !sysfs_path.exists() {
        return false;
    }

    // Follow the symlink and check for "usb" in the path
    if let Ok(resolved) = fs::canonicalize(&sysfs_path) {
        let path_str = resolved.to_string_lossy();
        return path_str.contains("/usb");
    }

    false
}

/// Get device information from sysfs and /proc
fn get_device_info(device_path: &PathBuf) -> Option<UsbDevice> {
    let name = device_path.file_name()?.to_str()?;
    let base_name: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();

    // Get filesystem type from blkid or fallback
    let fs_type = get_filesystem_type(device_path);

    // Skip unsupported filesystems
    if matches!(fs_type, FilesystemType::Unknown) {
        return None;
    }

    // Get label
    let label = get_device_label(device_path)
        .or_else(|| get_model_name(&base_name))
        .unwrap_or_else(|| name.to_string());

    // Get capacity from sysfs
    let capacity_bytes = get_device_capacity(&base_name, name);

    // Check mount status
    let mount_info = get_mount_info(device_path);

    Some(UsbDevice {
        device_path: device_path.clone(),
        label,
        mount_point: mount_info.as_ref().map(|(mp, _)| mp.clone()),
        filesystem: fs_type,
        capacity_bytes,
        available_bytes: mount_info.as_ref().map(|(_, avail)| *avail).unwrap_or(0),
        has_mesh_collection: mount_info
            .as_ref()
            .map(|(mp, _)| mp.join("mesh-collection").exists())
            .unwrap_or(false),
    })
}

/// Get filesystem type using blkid, /proc/mounts, or heuristics
fn get_filesystem_type(device_path: &PathBuf) -> FilesystemType {
    // Try blkid command first (most reliable when we have permission)
    if let Ok(output) = std::process::Command::new("blkid")
        .arg("-s")
        .arg("TYPE")
        .arg("-o")
        .arg("value")
        .arg(device_path)
        .output()
    {
        if output.status.success() {
            let fs_str = String::from_utf8_lossy(&output.stdout);
            let fs_type = FilesystemType::from_str(fs_str.trim());
            if !matches!(fs_type, FilesystemType::Unknown) {
                return fs_type;
            }
        }
    }

    // Fallback: check /proc/mounts if device is mounted (no root required)
    let device_str = device_path.to_string_lossy();
    if let Ok(content) = fs::read_to_string("/proc/mounts") {
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[0] == device_str {
                return FilesystemType::from_str(parts[2]);
            }
        }
    }

    // Fallback: check /dev/disk/by-id to confirm it's USB
    let device_name = device_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let by_id = PathBuf::from("/dev/disk/by-id");
    if by_id.exists() {
        if let Ok(entries) = fs::read_dir(&by_id) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Ok(target) = fs::read_link(entry.path()) {
                    if target.ends_with(device_name) {
                        let id_name = entry.file_name().to_string_lossy().to_lowercase();
                        if id_name.contains("usb") {
                            // It's a USB device but we can't determine the fs type
                            // Return ExFat as default (most common for USB)
                            return FilesystemType::ExFat;
                        }
                    }
                }
            }
        }
    }

    FilesystemType::Unknown
}

/// Get device label from /dev/disk/by-label or blkid
fn get_device_label(device_path: &PathBuf) -> Option<String> {
    // Try blkid first
    if let Ok(output) = std::process::Command::new("blkid")
        .arg("-s")
        .arg("LABEL")
        .arg("-o")
        .arg("value")
        .arg(device_path)
        .output()
    {
        if output.status.success() {
            let label = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !label.is_empty() {
                return Some(label);
            }
        }
    }

    // Try /dev/disk/by-label
    let device_name = device_path.file_name()?.to_str()?;
    let by_label = PathBuf::from("/dev/disk/by-label");

    if by_label.exists() {
        if let Ok(entries) = fs::read_dir(&by_label) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Ok(target) = fs::read_link(entry.path()) {
                    if target.file_name().and_then(|n| n.to_str()) == Some(device_name) {
                        return Some(entry.file_name().to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    None
}

/// Get model name from sysfs
fn get_model_name(base_name: &str) -> Option<String> {
    let model_path = PathBuf::from(format!("/sys/block/{}/device/model", base_name));
    fs::read_to_string(&model_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Get device capacity from sysfs
fn get_device_capacity(base_name: &str, partition_name: &str) -> u64 {
    // Try partition size first
    let size_path = PathBuf::from(format!("/sys/block/{}/{}/size", base_name, partition_name));
    if let Ok(size_str) = fs::read_to_string(&size_path) {
        if let Ok(sectors) = size_str.trim().parse::<u64>() {
            return sectors * 512; // Sector size is typically 512 bytes
        }
    }

    // Fall back to whole device size
    let size_path = PathBuf::from(format!("/sys/block/{}/size", base_name));
    if let Ok(size_str) = fs::read_to_string(&size_path) {
        if let Ok(sectors) = size_str.trim().parse::<u64>() {
            return sectors * 512;
        }
    }

    0
}

/// Get mount info for a device from /proc/mounts
fn get_mount_info(device_path: &PathBuf) -> Option<(PathBuf, u64)> {
    let device_str = device_path.to_string_lossy();

    let file = fs::File::open("/proc/mounts").ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().map_while(Result::ok) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && parts[0] == device_str {
            let mount_point = PathBuf::from(parts[1]);
            let available = get_available_space(&mount_point);
            return Some((mount_point, available));
        }
    }

    None
}

/// Get available space on a mounted filesystem
fn get_available_space(mount_point: &PathBuf) -> u64 {
    use std::ffi::CString;

    let path_cstr = match CString::new(mount_point.to_string_lossy().as_bytes()) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
            (stat.f_bavail as u64) * (stat.f_bsize as u64)
        } else {
            0
        }
    }
}

/// Monitor for USB device connect/disconnect events using inotify
///
/// This function blocks and should be run in a dedicated thread.
/// It watches /dev/disk/by-id for changes and calls the callback.
pub fn monitor_devices<F>(mut callback: F) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    F: FnMut(DeviceEvent) + Send,
{
    use inotify::{Inotify, WatchMask};

    let mut inotify = Inotify::init()?;

    // Watch /dev/disk/by-id for device changes
    let watch_path = PathBuf::from("/dev/disk/by-id");
    if watch_path.exists() {
        inotify.watches().add(
            &watch_path,
            WatchMask::CREATE | WatchMask::DELETE,
        )?;
    }

    // Also watch /dev for direct device nodes
    inotify.watches().add(
        "/dev",
        WatchMask::CREATE | WatchMask::DELETE,
    )?;

    let mut buffer = [0u8; 4096];
    let mut known_devices: HashSet<PathBuf> = enumerate_devices()
        .ok()
        .map(|devices| devices.into_iter().map(|d| d.device_path).collect())
        .unwrap_or_default();

    loop {
        // Read events with timeout
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => events,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };

        for event in events {
            // Skip non-USB related events
            let name = match event.name {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            // Filter for USB-related device names
            if !name.starts_with("usb") && !name.starts_with("sd") {
                continue;
            }

            // Re-enumerate devices to detect changes
            if let Ok(current_devices) = enumerate_devices() {
                let current_paths: HashSet<PathBuf> =
                    current_devices.iter().map(|d| d.device_path.clone()).collect();

                // Check for new devices
                for device in &current_devices {
                    if !known_devices.contains(&device.device_path) {
                        callback(DeviceEvent::Added(device.clone()));
                    }
                }

                // Check for removed devices
                for path in &known_devices {
                    if !current_paths.contains(path) {
                        callback(DeviceEvent::Removed(path.clone()));
                    }
                }

                known_devices = current_paths;
            }
        }
    }
}

/// Device event for hot-plug monitoring
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// A new USB device was connected
    Added(UsbDevice),
    /// A USB device was disconnected
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
        println!("Found {} USB devices", result.unwrap().len());
    }

    #[test]
    fn test_get_mount_info() {
        // Test with root partition (should always be mounted)
        let root = PathBuf::from("/dev/sda1");
        // Result depends on system, just check it doesn't panic
        let _ = get_mount_info(&root);
    }
}
