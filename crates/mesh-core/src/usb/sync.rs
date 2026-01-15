//! Sync engine with SHA256 content hashing
//!
//! This module provides efficient file synchronization by:
//! - Computing SHA256 hashes for change detection
//! - Building sync plans (what to copy, skip, delete)
//! - Managing the USB manifest for incremental sync

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::time::SystemTime;

/// SHA256 hash of a file's content
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileHash {
    /// SHA256 hash (32 bytes)
    #[serde(with = "hex_serde")]
    pub sha256: [u8; 32],
    /// File size in bytes
    pub size: u64,
}

impl FileHash {
    /// Compute hash from a file
    pub fn from_file(path: &PathBuf) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        let size = metadata.len();

        let mut reader = BufReader::with_capacity(65536, file); // 64KB buffer
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let result = hasher.finalize();
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&result);

        Ok(FileHash { sha256, size })
    }

    /// Format hash as hex string for display
    pub fn hex_string(&self) -> String {
        hex::encode(self.sha256)
    }
}

/// Manifest of exported files on USB device
///
/// Stored as mesh-manifest.yaml in the collection root
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbManifest {
    /// Manifest format version
    pub version: u32,
    /// When this export was created/updated
    pub exported_at: SystemTime,
    /// Map of relative path -> file hash
    pub files: HashMap<PathBuf, FileHash>,
}

impl Default for UsbManifest {
    fn default() -> Self {
        Self {
            version: 1,
            exported_at: SystemTime::now(),
            files: HashMap::new(),
        }
    }
}

impl UsbManifest {
    /// Load manifest from file
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = std::fs::read_to_string(path)?;
        let manifest: Self = serde_yaml::from_str(&content)?;
        Ok(manifest)
    }

    /// Save manifest to file
    pub fn save(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Check if a file needs to be updated
    pub fn needs_update(&self, relative_path: &PathBuf, hash: &FileHash) -> bool {
        match self.files.get(relative_path) {
            Some(existing) => existing != hash,
            None => true, // New file
        }
    }
}

/// A file to be synced
#[derive(Debug, Clone)]
pub struct SyncFile {
    /// Source path (local)
    pub source: PathBuf,
    /// Destination path (relative to USB collection root)
    pub destination: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Pre-computed hash
    pub hash: FileHash,
}

/// Result of comparing local vs USB collections
#[derive(Debug, Clone)]
pub struct SyncPlan {
    /// Files to copy (new or changed)
    pub to_copy: Vec<SyncFile>,
    /// Files to delete from USB (removed locally)
    pub to_delete: Vec<PathBuf>,
    /// Files unchanged (skip)
    pub unchanged: Vec<PathBuf>,
    /// Total bytes to transfer
    pub total_bytes: u64,
}

impl SyncPlan {
    /// Create an empty sync plan
    pub fn empty() -> Self {
        Self {
            to_copy: Vec::new(),
            to_delete: Vec::new(),
            unchanged: Vec::new(),
            total_bytes: 0,
        }
    }

    /// Check if there's anything to sync
    pub fn is_empty(&self) -> bool {
        self.to_copy.is_empty() && self.to_delete.is_empty()
    }

    /// Get summary for display
    pub fn summary(&self) -> String {
        let copy_mb = self.total_bytes as f64 / 1_000_000.0;
        format!(
            "{} to copy ({:.1}MB), {} unchanged, {} to delete",
            self.to_copy.len(),
            copy_mb,
            self.unchanged.len(),
            self.to_delete.len()
        )
    }

    /// Validate that USB has enough space
    pub fn validate_space(&self, available_bytes: u64) -> Result<(), super::UsbError> {
        if self.total_bytes > available_bytes {
            return Err(super::UsbError::InsufficientSpace {
                required: self.total_bytes,
                available: available_bytes,
            });
        }
        Ok(())
    }
}

/// Progress callback for sync plan building
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Build a sync plan by comparing local files with USB manifest
///
/// # Arguments
/// * `local_tracks` - List of (source_path, relative_dest_path) for tracks to export
/// * `usb_manifest` - Existing manifest from USB (or default if new)
/// * `progress` - Optional callback for progress updates
pub fn build_sync_plan(
    local_tracks: Vec<(PathBuf, PathBuf)>,
    usb_manifest: &UsbManifest,
    progress: Option<ProgressCallback>,
) -> Result<SyncPlan, Box<dyn std::error::Error + Send + Sync>> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let total_files = local_tracks.len();
    let progress_counter = AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    // Compute hashes in parallel
    let results: Vec<Result<(PathBuf, PathBuf, FileHash), std::io::Error>> = local_tracks
        .into_par_iter()
        .map(|(source, dest)| {
            let hash = FileHash::from_file(&source)?;

            // Update progress
            let current = progress_counter.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            Ok((source, dest, hash))
        })
        .collect();

    // Build sync plan
    let mut to_copy = Vec::new();
    let mut unchanged = Vec::new();
    let mut total_bytes = 0u64;

    for result in results {
        let (source, dest, hash) = result?;

        if usb_manifest.needs_update(&dest, &hash) {
            total_bytes += hash.size;
            to_copy.push(SyncFile {
                source,
                destination: dest,
                size: hash.size,
                hash,
            });
        } else {
            unchanged.push(dest);
        }
    }

    // Find files to delete (in manifest but not in local tracks)
    let local_dests: std::collections::HashSet<_> = to_copy
        .iter()
        .map(|f| &f.destination)
        .chain(unchanged.iter())
        .collect();

    let to_delete: Vec<PathBuf> = usb_manifest
        .files
        .keys()
        .filter(|path| !local_dests.contains(path))
        .cloned()
        .collect();

    Ok(SyncPlan {
        to_copy,
        to_delete,
        unchanged,
        total_bytes,
    })
}

/// Copy a file with hash verification
///
/// Returns the file hash on success, or error with retry info
pub fn copy_with_verification(
    source: &PathBuf,
    destination: &PathBuf,
    expected_hash: &FileHash,
    max_retries: usize,
) -> Result<(), super::UsbError> {
    // Ensure parent directory exists
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    for attempt in 1..=max_retries {
        // Copy the file
        std::fs::copy(source, destination)?;

        // Verify hash
        match FileHash::from_file(destination) {
            Ok(actual_hash) if actual_hash == *expected_hash => {
                return Ok(());
            }
            Ok(_) => {
                log::warn!(
                    "Hash mismatch on attempt {} for {}",
                    attempt,
                    destination.display()
                );
                if attempt == max_retries {
                    return Err(super::UsbError::HashMismatch {
                        path: destination.clone(),
                    });
                }
            }
            Err(e) => {
                log::warn!(
                    "Hash verification failed on attempt {} for {}: {}",
                    attempt,
                    destination.display(),
                    e
                );
                if attempt == max_retries {
                    return Err(super::UsbError::IoError(e.to_string()));
                }
            }
        }
    }

    Err(super::UsbError::HashMismatch {
        path: destination.clone(),
    })
}

/// Hex serialization for FileHash
mod hex_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Invalid hash length"));
        }
        let mut result = [0u8; 32];
        result.copy_from_slice(&bytes);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_file_hash() {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(b"Hello, World!").unwrap();
        temp.flush().unwrap();

        let hash = FileHash::from_file(&temp.path().to_path_buf()).unwrap();
        assert_eq!(hash.size, 13);
        assert!(!hash.hex_string().is_empty());
    }

    #[test]
    fn test_manifest_roundtrip() {
        let mut manifest = UsbManifest::default();
        manifest.files.insert(
            PathBuf::from("tracks/test.wav"),
            FileHash {
                sha256: [0u8; 32],
                size: 1000,
            },
        );

        let yaml = serde_yaml::to_string(&manifest).unwrap();
        let parsed: UsbManifest = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.files.len(), 1);
        assert!(parsed.files.contains_key(&PathBuf::from("tracks/test.wav")));
    }

    #[test]
    fn test_needs_update() {
        let mut manifest = UsbManifest::default();
        let hash = FileHash {
            sha256: [1u8; 32],
            size: 100,
        };
        manifest
            .files
            .insert(PathBuf::from("test.wav"), hash.clone());

        // Same hash - no update needed
        assert!(!manifest.needs_update(&PathBuf::from("test.wav"), &hash));

        // Different hash - update needed
        let new_hash = FileHash {
            sha256: [2u8; 32],
            size: 100,
        };
        assert!(manifest.needs_update(&PathBuf::from("test.wav"), &new_hash));

        // New file - update needed
        assert!(manifest.needs_update(&PathBuf::from("new.wav"), &hash));
    }

    #[test]
    fn test_sync_plan_summary() {
        let plan = SyncPlan {
            to_copy: vec![SyncFile {
                source: PathBuf::from("/tmp/test.wav"),
                destination: PathBuf::from("tracks/test.wav"),
                size: 10_000_000,
                hash: FileHash {
                    sha256: [0u8; 32],
                    size: 10_000_000,
                },
            }],
            to_delete: vec![],
            unchanged: vec![PathBuf::from("tracks/old.wav")],
            total_bytes: 10_000_000,
        };

        let summary = plan.summary();
        assert!(summary.contains("1 to copy"));
        assert!(summary.contains("10.0MB"));
        assert!(summary.contains("1 unchanged"));
    }
}
