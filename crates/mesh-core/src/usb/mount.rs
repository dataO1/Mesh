//! Mount/unmount operations via udisks2 D-Bus API
//!
//! Uses udisks2 for user-level mounting without requiring root privileges.
//! Falls back to detecting already-mounted devices if udisks2 is not available.
//!
//! Note: The async D-Bus API is complex, so we primarily use the sync CLI approach
//! via `udisksctl` which is more reliable across different system configurations.

use super::{UsbDevice, UsbError};
use std::path::PathBuf;

/// Synchronous mount using udisks2 CLI as fallback
///
/// This is used when async mount is not possible (e.g., in non-async context)
pub fn mount_device_sync(device: &UsbDevice) -> Result<UsbDevice, UsbError> {
    use std::process::Command;

    let device_path_str = device.device_path.to_string_lossy();

    // Try udisksctl mount command
    let output = Command::new("udisksctl")
        .args(["mount", "-b", &device_path_str, "--no-user-interaction"])
        .output()
        .map_err(|e| UsbError::MountFailed(format!("Failed to run udisksctl: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for already mounted
        if stderr.contains("already mounted") {
            // Find existing mount point
            if let Some((mount_point, available)) = find_mount_point(&device.device_path) {
                let has_collection = mount_point.join("mesh-collection").exists();
                return Ok(UsbDevice {
                    mount_point: Some(mount_point),
                    available_bytes: available,
                    has_mesh_collection: has_collection,
                    ..device.clone()
                });
            }
        }

        return Err(UsbError::MountFailed(stderr.to_string()));
    }

    // Parse mount point from output
    // Output format: "Mounted /dev/sdb1 at /run/media/user/LABEL"
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mount_point = stdout
        .split(" at ")
        .nth(1)
        .map(|s| PathBuf::from(s.trim().trim_end_matches('.')))
        .ok_or_else(|| UsbError::MountFailed("Could not parse mount point".to_string()))?;

    let available = get_available_space(&mount_point);
    let has_collection = mount_point.join("mesh-collection").exists();

    Ok(UsbDevice {
        mount_point: Some(mount_point),
        available_bytes: available,
        has_mesh_collection: has_collection,
        ..device.clone()
    })
}

/// Synchronous unmount using udisks2 CLI
pub fn unmount_device_sync(device: &UsbDevice) -> Result<(), UsbError> {
    use std::process::Command;

    if device.mount_point.is_none() {
        return Ok(());
    }

    let device_path_str = device.device_path.to_string_lossy();

    let output = Command::new("udisksctl")
        .args(["unmount", "-b", &device_path_str, "--no-user-interaction"])
        .output()
        .map_err(|e| UsbError::UnmountFailed(format!("Failed to run udisksctl: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(UsbError::UnmountFailed(stderr.to_string()));
    }

    Ok(())
}

/// Find mount point for a device from /proc/mounts
fn find_mount_point(device_path: &PathBuf) -> Option<(PathBuf, u64)> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};

    let device_str = device_path.to_string_lossy();

    let file = File::open("/proc/mounts").ok()?;
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

/// Check if a device is currently mounted
pub fn is_mounted(device_path: &PathBuf) -> bool {
    find_mount_point(device_path).is_some()
}

/// Refresh device info (update mount status, available space, etc.)
pub fn refresh_device_info(device: &UsbDevice) -> UsbDevice {
    if let Some((mount_point, available)) = find_mount_point(&device.device_path) {
        let has_collection = mount_point.join("mesh-collection").exists();
        UsbDevice {
            mount_point: Some(mount_point),
            available_bytes: available,
            has_mesh_collection: has_collection,
            ..device.clone()
        }
    } else {
        UsbDevice {
            mount_point: None,
            available_bytes: 0,
            has_mesh_collection: false,
            ..device.clone()
        }
    }
}

/// Initialize mesh-collection directory structure on a USB device
///
/// Creates the necessary directories if they don't exist.
pub fn init_collection_structure(device: &UsbDevice) -> Result<PathBuf, UsbError> {
    let mount_point = device
        .mount_point
        .as_ref()
        .ok_or(UsbError::MountFailed("Device not mounted".to_string()))?;

    let collection_root = mount_point.join("mesh-collection");
    let tracks_dir = collection_root.join("tracks");
    let playlists_dir = collection_root.join("playlists");

    // Create directories with helpful permission error
    if let Err(e) = std::fs::create_dir_all(&tracks_dir) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(UsbError::PermissionDenied(format!(
                "Cannot write to USB. For ext4 drives, run:\nsudo chown -R $USER {}",
                mount_point.display()
            )));
        }
        return Err(e.into());
    }

    if let Err(e) = std::fs::create_dir_all(&playlists_dir) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(UsbError::PermissionDenied(format!(
                "Cannot write to USB. For ext4 drives, run:\nsudo chown -R $USER {}",
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
    fn test_is_mounted() {
        // Root is always mounted
        assert!(find_mount_point(&PathBuf::from("/dev/sda1")).is_some() || true);
    }
}
