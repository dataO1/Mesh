//! Cross-platform mount utilities
//!
//! On modern desktop operating systems, USB drives are typically auto-mounted.
//! This module provides utilities for:
//! - Refreshing device info (mount status, available space)
//! - Initializing the mesh-collection directory structure
//!
//! Note: Explicit mount/unmount operations are OS-specific and typically
//! handled by the desktop environment. We rely on auto-mount.

use super::{UsbDevice, UsbError};
use std::path::PathBuf;
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
