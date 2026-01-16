//! Cross-platform sync engine with SHA256 content hashing
//!
//! This module provides efficient file synchronization by:
//! - Scanning local and USB collections to discover current state
//! - Computing SHA256 hashes for content-based change detection
//! - Building minimal sync plans (what to copy, delete, link)
//! - Supporting playlist-aware diffing (track symlinks)

use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// SHA256 hash of a file's content (32 bytes)
pub type FileHash = [u8; 32];

/// Information about a track file
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Filename only (e.g., "track.wav")
    pub filename: String,
    /// SHA256 hash of content
    pub hash: FileHash,
    /// File size in bytes
    pub size: u64,
}

/// A symlink in a playlist folder
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaylistLink {
    /// Playlist name (folder name under playlists/)
    pub playlist: String,
    /// Track filename (e.g., "track.wav")
    pub track_filename: String,
}

/// State of a collection (local or USB)
#[derive(Debug, Clone, Default)]
pub struct CollectionState {
    /// Map of filename -> TrackInfo for tracks in tracks/
    pub tracks: HashMap<String, TrackInfo>,
    /// Set of (playlist_name, track_filename) pairs
    pub playlist_links: HashSet<PlaylistLink>,
    /// Set of playlist names (directories under playlists/)
    pub playlist_names: HashSet<String>,
}

/// A track to copy during sync
#[derive(Debug, Clone)]
pub struct TrackCopy {
    /// Source path (local)
    pub source: PathBuf,
    /// Destination path (relative to USB collection root, e.g., "tracks/file.wav")
    pub destination: PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Expected hash for verification
    pub hash: FileHash,
}

/// Result of comparing local vs USB collections
#[derive(Debug, Clone, Default)]
pub struct SyncPlan {
    /// Tracks to copy (new or changed content)
    pub tracks_to_copy: Vec<TrackCopy>,
    /// Track filenames to delete from USB
    pub tracks_to_delete: Vec<String>,
    /// Playlist symlinks to add
    pub symlinks_to_add: Vec<PlaylistLink>,
    /// Playlist symlinks to remove
    pub symlinks_to_remove: Vec<PlaylistLink>,
    /// Playlist directories to create
    pub dirs_to_create: Vec<String>,
    /// Playlist directories to delete
    pub dirs_to_delete: Vec<String>,
    /// Total bytes to transfer
    pub total_bytes: u64,
}

impl SyncPlan {
    /// Check if there's anything to sync
    pub fn is_empty(&self) -> bool {
        self.tracks_to_copy.is_empty()
            && self.tracks_to_delete.is_empty()
            && self.symlinks_to_add.is_empty()
            && self.symlinks_to_remove.is_empty()
            && self.dirs_to_create.is_empty()
            && self.dirs_to_delete.is_empty()
    }

    /// Get summary for display
    pub fn summary(&self) -> String {
        let copy_mb = self.total_bytes as f64 / 1_000_000.0;
        format!(
            "{} tracks to copy ({:.1}MB), {} to delete, {} symlinks to add, {} to remove",
            self.tracks_to_copy.len(),
            copy_mb,
            self.tracks_to_delete.len(),
            self.symlinks_to_add.len(),
            self.symlinks_to_remove.len(),
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

/// Progress callback for scanning/hashing
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Compute SHA256 hash of a file
pub fn compute_hash(path: &Path) -> std::io::Result<FileHash> {
    let file = File::open(path)?;
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
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    Ok(hash)
}

/// Scan a local collection directory to discover tracks and playlist membership
///
/// # Arguments
/// * `collection_root` - Path to the local collection (contains tracks/ and playlists/)
/// * `selected_playlists` - List of playlist names to include (e.g., ["My Playlist", "Set 1"])
/// * `progress` - Optional callback for progress updates
pub fn scan_local_collection(
    collection_root: &Path,
    selected_playlists: &[String],
    progress: Option<ProgressCallback>,
) -> Result<CollectionState, Box<dyn std::error::Error + Send + Sync>> {
    let mut state = CollectionState::default();
    let playlists_dir = collection_root.join("playlists");

    // Collect all track paths and their playlist membership
    let mut track_paths: HashMap<String, PathBuf> = HashMap::new(); // filename -> resolved path
    let mut track_playlists: HashMap<String, HashSet<String>> = HashMap::new(); // filename -> playlist names

    for playlist_name in selected_playlists {
        let playlist_path = playlists_dir.join(playlist_name);
        if !playlist_path.exists() {
            continue;
        }

        state.playlist_names.insert(playlist_name.clone());

        // Scan playlist directory for tracks
        if let Ok(entries) = std::fs::read_dir(&playlist_path) {
            for entry in entries.filter_map(|e| e.ok()) {
                let entry_path = entry.path();

                // Only process .wav files
                if entry_path.extension().and_then(|e| e.to_str()) != Some("wav") {
                    continue;
                }

                let filename = match entry_path.file_name().and_then(|n| n.to_str()) {
                    Some(f) => f.to_string(),
                    None => continue,
                };

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
                        .unwrap_or_else(|| entry_path.clone())
                } else {
                    entry_path.clone()
                };

                // Track the file path (use first occurrence if duplicate filename)
                track_paths.entry(filename.clone()).or_insert(track_path);

                // Track playlist membership
                track_playlists
                    .entry(filename.clone())
                    .or_default()
                    .insert(playlist_name.clone());

                // Add playlist link
                state.playlist_links.insert(PlaylistLink {
                    playlist: playlist_name.clone(),
                    track_filename: filename,
                });
            }
        }
    }

    // Now hash all unique tracks in parallel
    let track_list: Vec<(String, PathBuf)> = track_paths.into_iter().collect();
    let total_files = track_list.len();
    let progress_counter = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    let track_infos: Vec<Result<TrackInfo, std::io::Error>> = track_list
        .into_par_iter()
        .map(|(filename, path)| {
            let metadata = std::fs::metadata(&path)?;
            let hash = compute_hash(&path)?;

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            Ok(TrackInfo {
                path,
                filename,
                hash,
                size: metadata.len(),
            })
        })
        .collect();

    // Collect results
    for result in track_infos {
        let info = result?;
        state.tracks.insert(info.filename.clone(), info);
    }

    Ok(state)
}

/// Scan a USB collection to discover what's already there
///
/// # Arguments
/// * `collection_root` - Path to mesh-collection/ on USB
/// * `progress` - Optional callback for progress updates
pub fn scan_usb_collection(
    collection_root: &Path,
    progress: Option<ProgressCallback>,
) -> Result<CollectionState, Box<dyn std::error::Error + Send + Sync>> {
    let mut state = CollectionState::default();

    let tracks_dir = collection_root.join("tracks");
    let playlists_dir = collection_root.join("playlists");

    // Collect track paths first
    let track_paths: Vec<PathBuf> = if tracks_dir.exists() {
        WalkDir::new(&tracks_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wav"))
            .map(|e| e.path().to_path_buf())
            .collect()
    } else {
        Vec::new()
    };

    // Hash tracks in parallel
    let total_files = track_paths.len();
    let progress_counter = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    let track_infos: Vec<Result<TrackInfo, std::io::Error>> = track_paths
        .into_par_iter()
        .map(|path| {
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let metadata = std::fs::metadata(&path)?;
            let hash = compute_hash(&path)?;

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            Ok(TrackInfo {
                path,
                filename,
                hash,
                size: metadata.len(),
            })
        })
        .collect();

    // Collect track results
    for result in track_infos {
        if let Ok(info) = result {
            state.tracks.insert(info.filename.clone(), info);
        }
    }

    // Scan playlist directories
    if playlists_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&playlists_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let playlist_name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                state.playlist_names.insert(playlist_name.clone());

                // Scan playlist for links (symlinks or copies)
                if let Ok(playlist_entries) = std::fs::read_dir(&path) {
                    for pentry in playlist_entries.filter_map(|e| e.ok()) {
                        let ppath = pentry.path();
                        if ppath.extension().and_then(|x| x.to_str()) == Some("wav") {
                            if let Some(filename) = ppath.file_name().and_then(|n| n.to_str()) {
                                state.playlist_links.insert(PlaylistLink {
                                    playlist: playlist_name.clone(),
                                    track_filename: filename.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(state)
}

/// Build a sync plan by comparing local state with USB state
pub fn build_sync_plan(local: &CollectionState, usb: &CollectionState) -> SyncPlan {
    let mut plan = SyncPlan::default();

    // Tracks to copy: in local but not in USB, or hash differs
    for (filename, local_info) in &local.tracks {
        let needs_copy = match usb.tracks.get(filename) {
            Some(usb_info) => {
                // Quick check: different size means different content
                if local_info.size != usb_info.size {
                    true
                } else {
                    // Same size, check hash
                    local_info.hash != usb_info.hash
                }
            }
            None => true, // New file
        };

        if needs_copy {
            plan.total_bytes += local_info.size;
            plan.tracks_to_copy.push(TrackCopy {
                source: local_info.path.clone(),
                destination: PathBuf::from("tracks").join(filename),
                size: local_info.size,
                hash: local_info.hash,
            });
        }
    }

    // Tracks to delete: in USB but not in local
    for filename in usb.tracks.keys() {
        if !local.tracks.contains_key(filename) {
            plan.tracks_to_delete.push(filename.clone());
        }
    }

    // Symlinks to add: in local but not in USB
    for link in &local.playlist_links {
        if !usb.playlist_links.contains(link) {
            plan.symlinks_to_add.push(link.clone());
        }
    }

    // Symlinks to remove: in USB but not in local
    for link in &usb.playlist_links {
        if !local.playlist_links.contains(link) {
            plan.symlinks_to_remove.push(link.clone());
        }
    }

    // Directories to create: in local but not in USB
    for name in &local.playlist_names {
        if !usb.playlist_names.contains(name) {
            plan.dirs_to_create.push(name.clone());
        }
    }

    // Directories to delete: in USB but not in local
    for name in &usb.playlist_names {
        if !local.playlist_names.contains(name) {
            plan.dirs_to_delete.push(name.clone());
        }
    }

    plan
}

/// Copy a file with hash verification
///
/// Returns Ok(()) on success, or error with retry info
pub fn copy_with_verification(
    source: &Path,
    destination: &Path,
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
        match compute_hash(destination) {
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
                        path: destination.to_path_buf(),
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
        path: destination.to_path_buf(),
    })
}

/// Format hash as hex string for display
pub fn hash_to_hex(hash: &FileHash) -> String {
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_compute_hash() {
        let mut temp = NamedTempFile::new().unwrap();
        temp.write_all(b"Hello, World!").unwrap();
        temp.flush().unwrap();

        let hash = compute_hash(temp.path()).unwrap();
        assert!(!hash_to_hex(&hash).is_empty());
    }

    #[test]
    fn test_sync_plan_summary() {
        let plan = SyncPlan {
            tracks_to_copy: vec![TrackCopy {
                source: PathBuf::from("/tmp/test.wav"),
                destination: PathBuf::from("tracks/test.wav"),
                size: 10_000_000,
                hash: [0u8; 32],
            }],
            tracks_to_delete: vec![],
            symlinks_to_add: vec![PlaylistLink {
                playlist: "My Playlist".to_string(),
                track_filename: "test.wav".to_string(),
            }],
            symlinks_to_remove: vec![],
            dirs_to_create: vec!["My Playlist".to_string()],
            dirs_to_delete: vec![],
            total_bytes: 10_000_000,
        };

        let summary = plan.summary();
        assert!(summary.contains("1 tracks to copy"));
        assert!(summary.contains("10.0MB"));
    }

    #[test]
    fn test_build_sync_plan_empty() {
        let local = CollectionState::default();
        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);
        assert!(plan.is_empty());
    }

    #[test]
    fn test_build_sync_plan_new_track() {
        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            TrackInfo {
                path: PathBuf::from("/tmp/test.wav"),
                filename: "test.wav".to_string(),
                hash: [1u8; 32],
                size: 1000,
            },
        );

        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);

        assert_eq!(plan.tracks_to_copy.len(), 1);
        assert!(plan.tracks_to_delete.is_empty());
    }

    #[test]
    fn test_build_sync_plan_unchanged_track() {
        let track = TrackInfo {
            path: PathBuf::from("/tmp/test.wav"),
            filename: "test.wav".to_string(),
            hash: [1u8; 32],
            size: 1000,
        };

        let mut local = CollectionState::default();
        local.tracks.insert("test.wav".to_string(), track.clone());

        let mut usb = CollectionState::default();
        usb.tracks.insert("test.wav".to_string(), track);

        let plan = build_sync_plan(&local, &usb);
        assert!(plan.tracks_to_copy.is_empty());
    }
}
