//! USB device support for Mesh DJ software
//!
//! This module provides:
//! - USB storage device detection via sysinfo (cross-platform)
//! - Mount/unmount support
//! - Playlist storage backend for USB devices
//! - Efficient sync with metadata-based change detection (size + mtime)
//! - Background manager thread for non-blocking operations
//!
//! # Architecture
//!
//! All USB operations run in background threads to never block the UI:
//!
//! ```text
//! UI Thread (iced)
//!     │
//!     │ UsbCommand (non-blocking send)
//!     ▼
//! UsbManager Thread
//!     ├── udev monitor (device connect/disconnect)
//!     ├── mount operations (via udisks2)
//!     └── worker pool (hashing, copying, scanning)
//!            │
//!            │ UsbMessage (async results)
//!            ▼
//!        UI Thread
//! ```

pub mod config;
pub mod detection;
pub mod manager;
pub mod message;
pub mod mount;
pub mod storage;
pub mod sync;

// Re-export main types for convenience
pub use config::{
    ExportableConfig, ExportableAudioConfig, ExportableDisplayConfig, ExportableSlicerConfig,
};
pub use manager::{UsbCommand, UsbManager};
pub use message::UsbMessage;
pub use storage::{CachedTrackMetadata, UsbStorage};
pub use sync::{CollectionState, PlaylistLink, SyncPlan, TrackCopy, TrackInfo};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents a detected USB storage device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbDevice {
    /// Device path (e.g., "/dev/sdb1")
    pub device_path: PathBuf,

    /// User-friendly label (volume name or device name)
    pub label: String,

    /// Mount point when mounted (e.g., "/media/user/SANDISK")
    /// None if device is not mounted
    pub mount_point: Option<PathBuf>,

    /// Filesystem type
    pub filesystem: FilesystemType,

    /// Total capacity in bytes
    pub capacity_bytes: u64,

    /// Available space in bytes (0 if not mounted)
    pub available_bytes: u64,

    /// Whether this device has an existing mesh-collection directory
    pub has_mesh_collection: bool,
}

impl UsbDevice {
    /// Get the path to the mesh-collection root on this device
    ///
    /// Returns None if device is not mounted
    pub fn collection_root(&self) -> Option<PathBuf> {
        self.mount_point
            .as_ref()
            .map(|mp| mp.join("mesh-collection"))
    }

    /// Get the manifest file path
    pub fn manifest_path(&self) -> Option<PathBuf> {
        self.collection_root()
            .map(|root| root.join("mesh-manifest.yaml"))
    }

    /// Get the config file path
    pub fn config_path(&self) -> Option<PathBuf> {
        self.collection_root()
            .map(|root| root.join("player-config.yaml"))
    }

    /// Format device info for display (e.g., "SANDISK (32GB, 28GB free)")
    pub fn display_info(&self) -> String {
        if self.mount_point.is_some() {
            format!(
                "{} ({}, {} free)",
                self.label,
                format_bytes(self.capacity_bytes),
                format_bytes(self.available_bytes)
            )
        } else {
            format!("{} ({}, not mounted)", self.label, format_bytes(self.capacity_bytes))
        }
    }

    /// Check if this device supports symlinks
    pub fn supports_symlinks(&self) -> bool {
        matches!(self.filesystem, FilesystemType::Ext4)
    }
}

/// Format bytes as human-readable string (GB for >= 1GB, MB otherwise)
pub fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0}MB", bytes as f64 / MB as f64)
    }
}

/// Supported filesystem types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FilesystemType {
    /// Linux native filesystem - supports symlinks
    Ext4,
    /// Extended FAT - cross-platform, no symlinks
    ExFat,
    /// FAT32 - maximum compatibility, no symlinks, 4GB file limit
    Fat32,
    /// Unknown or unsupported filesystem
    #[default]
    Unknown,
}

impl FilesystemType {
    /// Parse filesystem type from string (as returned by udev/blkid)
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ext4" => FilesystemType::Ext4,
            "exfat" => FilesystemType::ExFat,
            "vfat" | "fat32" | "fat" => FilesystemType::Fat32,
            _ => FilesystemType::Unknown,
        }
    }

    /// Get display name
    pub fn display_name(&self) -> &'static str {
        match self {
            FilesystemType::Ext4 => "ext4",
            FilesystemType::ExFat => "exFAT",
            FilesystemType::Fat32 => "FAT32",
            FilesystemType::Unknown => "Unknown",
        }
    }

    /// Check if this filesystem supports symlinks
    pub fn supports_symlinks(&self) -> bool {
        matches!(self, FilesystemType::Ext4)
    }
}

impl std::fmt::Display for FilesystemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Errors that can occur during USB operations
#[derive(Debug, Clone)]
pub enum UsbError {
    /// Device not found
    DeviceNotFound(String),

    /// Mount operation failed
    MountFailed(String),

    /// Unmount operation failed
    UnmountFailed(String),

    /// Insufficient space on device
    InsufficientSpace {
        required: u64,
        available: u64,
    },

    /// Permission denied (mount requires root or udisks2)
    PermissionDenied(String),

    /// IO error during file operations
    IoError(String),

    /// Device was disconnected during operation
    DeviceDisconnected,

    /// File size verification failed after copy
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },

    /// Filesystem not supported
    UnsupportedFilesystem(String),

    /// Manifest parsing error
    ManifestError(String),

    /// Export was cancelled by user
    Cancelled,
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsbError::DeviceNotFound(path) => write!(f, "Device not found: {}", path),
            UsbError::MountFailed(msg) => write!(f, "Mount failed: {}", msg),
            UsbError::UnmountFailed(msg) => write!(f, "Unmount failed: {}", msg),
            UsbError::InsufficientSpace { required, available } => {
                write!(
                    f,
                    "Insufficient space: need {}, only {} available",
                    format_bytes(*required),
                    format_bytes(*available)
                )
            }
            UsbError::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            UsbError::IoError(msg) => write!(f, "IO error: {}", msg),
            UsbError::DeviceDisconnected => write!(f, "Device disconnected during operation"),
            UsbError::SizeMismatch { path, expected, actual } => {
                write!(f, "File verification failed: {} (expected {} bytes, got {})",
                    path.display(), expected, actual)
            }
            UsbError::UnsupportedFilesystem(fs) => write!(f, "Unsupported filesystem: {}", fs),
            UsbError::ManifestError(msg) => write!(f, "Manifest error: {}", msg),
            UsbError::Cancelled => write!(f, "Operation cancelled"),
        }
    }
}

impl std::error::Error for UsbError {}

impl From<std::io::Error> for UsbError {
    fn from(e: std::io::Error) -> Self {
        // Provide helpful message for permission denied errors
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            UsbError::PermissionDenied(
                "Cannot write to USB device. For ext4 drives, run: sudo chown -R $USER <mount_point>".to_string()
            )
        } else {
            UsbError::IoError(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filesystem_type_parsing() {
        assert_eq!(FilesystemType::from_str("ext4"), FilesystemType::Ext4);
        assert_eq!(FilesystemType::from_str("EXT4"), FilesystemType::Ext4);
        assert_eq!(FilesystemType::from_str("exfat"), FilesystemType::ExFat);
        assert_eq!(FilesystemType::from_str("vfat"), FilesystemType::Fat32);
        assert_eq!(FilesystemType::from_str("fat32"), FilesystemType::Fat32);
        assert_eq!(FilesystemType::from_str("ntfs"), FilesystemType::Unknown);
    }

    #[test]
    fn test_symlink_support() {
        assert!(FilesystemType::Ext4.supports_symlinks());
        assert!(!FilesystemType::ExFat.supports_symlinks());
        assert!(!FilesystemType::Fat32.supports_symlinks());
    }

    #[test]
    fn test_usb_device_display() {
        let device = UsbDevice {
            device_path: PathBuf::from("/dev/sdb1"),
            label: "SANDISK".to_string(),
            mount_point: Some(PathBuf::from("/media/user/SANDISK")),
            filesystem: FilesystemType::ExFat,
            capacity_bytes: 32_000_000_000,
            available_bytes: 28_000_000_000,
            has_mesh_collection: true,
        };

        assert!(device.display_info().contains("SANDISK"));
        assert!(device.display_info().contains("32.0GB"));
        assert!(device.display_info().contains("28.0GB"));
        assert!(device.display_info().contains("free"));
    }

    #[test]
    fn test_collection_paths() {
        let device = UsbDevice {
            device_path: PathBuf::from("/dev/sdb1"),
            label: "TEST".to_string(),
            mount_point: Some(PathBuf::from("/media/test")),
            filesystem: FilesystemType::Ext4,
            capacity_bytes: 0,
            available_bytes: 0,
            has_mesh_collection: false,
        };

        assert_eq!(
            device.collection_root(),
            Some(PathBuf::from("/media/test/mesh-collection"))
        );
        assert_eq!(
            device.manifest_path(),
            Some(PathBuf::from("/media/test/mesh-collection/mesh-manifest.yaml"))
        );
    }
}
