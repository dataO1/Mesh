//! Cross-platform mount utilities
//!
//! On modern desktop operating systems, USB drives are typically auto-mounted.
//! This module provides utilities for:
//! - Refreshing device info (mount status, available space)
//! - Initializing the mesh-collection directory structure
//!
//! Note: Explicit mount/unmount operations are OS-specific and typically
//! handled by the desktop environment. We rely on auto-mount.

use super::{FilesystemType, UsbDevice, UsbError};
use std::path::{Path, PathBuf};
use sysinfo::Disks;

/// Refresh device info using sysinfo
///
/// Updates mount status, available space, and mesh-collection presence.
/// This is cross-platform and works on Linux, macOS, and Windows.
pub fn refresh_device_info(device: &UsbDevice) -> UsbDevice {
    let disks = Disks::new_with_refreshed_list();

    // Find the disk by mount point
    for disk in disks.list() {
        let mount_point = disk.mount_point().to_path_buf();

        // Match by mount point (our primary identifier in cross-platform mode)
        if Some(&mount_point) == device.mount_point.as_ref()
            || mount_point == device.device_path
        {
            let has_collection = mount_point.join("mesh-collection").exists();
            return UsbDevice {
                mount_point: Some(mount_point),
                available_bytes: disk.available_space(),
                has_mesh_collection: has_collection,
                ..device.clone()
            };
        }
    }

    // Device not found - might have been unmounted
    UsbDevice {
        mount_point: None,
        available_bytes: 0,
        has_mesh_collection: false,
        ..device.clone()
    }
}

/// Check if a device is currently mounted
pub fn is_mounted(device: &UsbDevice) -> bool {
    let disks = Disks::new_with_refreshed_list();

    for disk in disks.list() {
        let mount_point = disk.mount_point().to_path_buf();
        if Some(&mount_point) == device.mount_point.as_ref()
            || mount_point == device.device_path
        {
            return true;
        }
    }

    false
}

/// Initialize mesh-collection directory structure on a USB device
///
/// Creates the necessary directories if they don't exist:
/// - mesh-collection/
/// - mesh-collection/tracks/
///
/// Playlists are stored in mesh.db (same as local collection).
pub fn init_collection_structure(device: &UsbDevice) -> Result<PathBuf, UsbError> {
    let mount_point = device
        .mount_point
        .as_ref()
        .ok_or(UsbError::MountFailed("Device not mounted".to_string()))?;

    let collection_root = mount_point.join("mesh-collection");
    let tracks_dir = collection_root.join("tracks");

    // Create directories with helpful permission error
    if let Err(e) = std::fs::create_dir_all(&tracks_dir) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(UsbError::PermissionDenied(format!(
                "Cannot write to USB device at {}",
                mount_point.display()
            )));
        }
        return Err(e.into());
    }

    Ok(collection_root)
}

/// Resolve the actual block device path (e.g. `/dev/sda1`) from a mount point.
///
/// Reads `/proc/mounts` to find which block device is mounted at the given path.
/// This is needed because `UsbDevice.device_path` is set to the mount point,
/// not the real block device, and tools like `e2label`/`fatlabel` need `/dev/sdX`.
#[cfg(target_os = "linux")]
pub fn resolve_block_device(mount_point: &Path) -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let mount_str = mount_point.to_string_lossy();
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let dev = parts.next()?;
        let mount = parts.next()?;
        if mount == mount_str.as_ref() && dev.starts_with("/dev/") {
            return Some(PathBuf::from(dev));
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn resolve_block_device(_mount_point: &Path) -> Option<PathBuf> {
    None
}

/// Set the filesystem label on a USB device.
///
/// Uses the appropriate tool for each filesystem type:
/// - ext4: `e2label` (max 16 chars)
/// - FAT32: `fatlabel` (max 11 chars, uppercase)
/// - exFAT: `exfatlabel` (max 15 chars)
///
/// Runs via `sudo` since label-setting requires root. On the embedded
/// NixOS image the mesh user has passwordless sudo.
#[cfg(target_os = "linux")]
pub fn set_filesystem_label(
    mount_point: &Path,
    label: &str,
    filesystem: FilesystemType,
) -> Result<(), UsbError> {
    let block_dev = resolve_block_device(mount_point).ok_or_else(|| {
        UsbError::IoError(format!(
            "Cannot resolve block device for {}",
            mount_point.display()
        ))
    })?;
    let block_dev_str = block_dev.to_string_lossy();

    let (cmd, truncated) = match filesystem {
        FilesystemType::Ext4 => {
            let max = label.len().min(16);
            ("e2label", label[..max].to_string())
        }
        FilesystemType::Fat32 => {
            let upper = label.to_uppercase();
            let max = upper.len().min(11);
            ("fatlabel", upper[..max].to_string())
        }
        FilesystemType::ExFat => {
            let max = label.len().min(15);
            ("exfatlabel", label[..max].to_string())
        }
        FilesystemType::Unknown => {
            return Err(UsbError::IoError(
                "Unsupported filesystem for labeling".to_string(),
            ));
        }
    };

    log::info!(
        "Setting {} label on {} to '{}'",
        cmd,
        block_dev_str,
        truncated
    );

    let output = std::process::Command::new("sudo")
        .arg(cmd)
        .arg(block_dev_str.as_ref())
        .arg(&truncated)
        .output()
        .map_err(|e| UsbError::IoError(format!("Failed to run {}: {}", cmd, e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(UsbError::IoError(format!(
            "{} failed: {}",
            cmd,
            stderr.trim()
        )));
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_filesystem_label(
    _mount_point: &Path,
    _label: &str,
    _filesystem: FilesystemType,
) -> Result<(), UsbError> {
    log::warn!("Filesystem label setting not supported on this platform");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_finds_disks() {
        // Just verify we can enumerate disks without panicking
        let disks = Disks::new_with_refreshed_list();
        println!("Found {} disks", disks.list().len());
        for disk in disks.list() {
            println!(
                "  - {:?} at {:?} ({} bytes available)",
                disk.name(),
                disk.mount_point(),
                disk.available_space()
            );
        }
    }
}
