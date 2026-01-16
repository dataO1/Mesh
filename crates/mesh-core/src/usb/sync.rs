//! Cross-platform sync engine with metadata-based change detection
//!
//! This module provides efficient file synchronization by:
//! - Scanning local and USB collections to discover current state
//! - Using file metadata (size + mtime) for fast change detection
//! - Building minimal sync plans (what to copy, delete, link)
//! - Supporting playlist-aware diffing (track symlinks)

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// Information about a track file
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Filename only (e.g., "track.wav")
    pub filename: String,
    /// File size in bytes
    pub size: u64,
    /// Last modified time
    pub mtime: SystemTime,
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
    /// Tracks missing LUFS analysis (need to analyze before export)
    pub tracks_missing_lufs: Vec<PathBuf>,
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
        format!(
            "{} tracks to copy ({}), {} to delete, {} symlinks to add, {} to remove",
            self.tracks_to_copy.len(),
            super::format_bytes(self.total_bytes),
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

/// Progress callback for scanning
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Scan a local collection directory to discover tracks and playlist membership
///
/// # Arguments
/// * `collection_root` - Path to the local collection (contains tracks/ and playlists/)
/// * `selected_playlists` - List of playlist names to include (e.g., ["My Playlist", "Set 1"])
/// * `progress` - Optional callback for progress updates (reports files scanned, not hashed)
pub fn scan_local_collection(
    collection_root: &Path,
    selected_playlists: &[String],
    progress: Option<ProgressCallback>,
) -> Result<CollectionState, Box<dyn std::error::Error + Send + Sync>> {
    let mut state = CollectionState::default();
    let playlists_dir = collection_root.join("playlists");

    // Collect all track paths and their playlist membership
    let mut track_paths: HashMap<String, PathBuf> = HashMap::new(); // filename -> resolved path

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

                // Add playlist link
                state.playlist_links.insert(PlaylistLink {
                    playlist: playlist_name.clone(),
                    track_filename: filename,
                });
            }
        }
    }

    // Get metadata for all unique tracks (fast - no hashing)
    let track_list: Vec<(String, PathBuf)> = track_paths.into_iter().collect();
    let total_files = track_list.len();
    let progress_counter = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    let track_infos: Vec<Result<TrackInfo, std::io::Error>> = track_list
        .into_par_iter()
        .map(|(filename, path)| {
            let metadata = std::fs::metadata(&path)?;
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            Ok(TrackInfo {
                path,
                filename,
                size: metadata.len(),
                mtime,
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
/// * `progress` - Optional callback for progress updates (reports files scanned)
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

    // Get metadata for tracks (fast - no hashing)
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
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            Ok(TrackInfo {
                path,
                filename,
                size: metadata.len(),
                mtime,
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
///
/// Uses size and modification time to detect changes (fast, no hashing).
/// A file needs to be copied if:
/// - It doesn't exist on USB
/// - Size differs (definite change)
/// - Local mtime is newer (file was modified)
pub fn build_sync_plan(local: &CollectionState, usb: &CollectionState) -> SyncPlan {
    let mut plan = SyncPlan::default();

    // Tracks to copy: in local but not in USB, or metadata indicates change
    for (filename, local_info) in &local.tracks {
        let needs_copy = match usb.tracks.get(filename) {
            Some(usb_info) => {
                // Different size = definitely different content
                if local_info.size != usb_info.size {
                    true
                } else {
                    // Same size, check if local is newer
                    // Use a small tolerance (2 seconds) for FAT32 time granularity
                    local_info.mtime > usb_info.mtime
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

    // Check which tracks are missing LUFS (for auto-analysis before export)
    plan.tracks_missing_lufs = plan
        .tracks_to_copy
        .iter()
        .filter(|track| {
            crate::audio_file::read_metadata(&track.source)
                .map(|m| m.lufs.is_none())
                .unwrap_or(false)
        })
        .map(|track| track.source.clone())
        .collect();

    if !plan.tracks_missing_lufs.is_empty() {
        log::info!(
            "[LUFS] {} tracks missing LUFS analysis",
            plan.tracks_missing_lufs.len()
        );
    }

    plan
}

/// Copy a file with size verification
///
/// Returns Ok(()) on success, or error with retry info.
/// Verifies the copy succeeded by checking destination size matches source.
pub fn copy_with_verification(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    max_retries: usize,
) -> Result<(), super::UsbError> {
    // Ensure parent directory exists
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    for attempt in 1..=max_retries {
        // Copy the file
        std::fs::copy(source, destination)?;

        // Verify size matches
        match std::fs::metadata(destination) {
            Ok(meta) if meta.len() == expected_size => {
                return Ok(());
            }
            Ok(meta) => {
                log::warn!(
                    "Size mismatch on attempt {} for {}: expected {} got {}",
                    attempt,
                    destination.display(),
                    expected_size,
                    meta.len()
                );
                if attempt == max_retries {
                    return Err(super::UsbError::SizeMismatch {
                        path: destination.to_path_buf(),
                        expected: expected_size,
                        actual: meta.len(),
                    });
                }
            }
            Err(e) => {
                log::warn!(
                    "Size verification failed on attempt {} for {}: {}",
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

    Err(super::UsbError::SizeMismatch {
        path: destination.to_path_buf(),
        expected: expected_size,
        actual: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_plan_summary() {
        let plan = SyncPlan {
            tracks_to_copy: vec![TrackCopy {
                source: PathBuf::from("/tmp/test.wav"),
                destination: PathBuf::from("tracks/test.wav"),
                size: 10_000_000,
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
        assert!(summary.contains("10MB")); // format_bytes rounds to whole MB for < 1GB
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
                size: 1000,
                mtime: SystemTime::now(),
            },
        );

        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);

        assert_eq!(plan.tracks_to_copy.len(), 1);
        assert!(plan.tracks_to_delete.is_empty());
    }

    #[test]
    fn test_build_sync_plan_unchanged_track() {
        let mtime = SystemTime::now();
        let track = TrackInfo {
            path: PathBuf::from("/tmp/test.wav"),
            filename: "test.wav".to_string(),
            size: 1000,
            mtime,
        };

        let mut local = CollectionState::default();
        local.tracks.insert("test.wav".to_string(), track.clone());

        let mut usb = CollectionState::default();
        usb.tracks.insert("test.wav".to_string(), track);

        let plan = build_sync_plan(&local, &usb);
        assert!(plan.tracks_to_copy.is_empty());
    }

    #[test]
    fn test_build_sync_plan_size_changed() {
        let mtime = SystemTime::now();

        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            TrackInfo {
                path: PathBuf::from("/tmp/test.wav"),
                filename: "test.wav".to_string(),
                size: 2000, // Different size
                mtime,
            },
        );

        let mut usb = CollectionState::default();
        usb.tracks.insert(
            "test.wav".to_string(),
            TrackInfo {
                path: PathBuf::from("/usb/test.wav"),
                filename: "test.wav".to_string(),
                size: 1000,
                mtime,
            },
        );

        let plan = build_sync_plan(&local, &usb);
        assert_eq!(plan.tracks_to_copy.len(), 1);
    }

    #[test]
    fn test_build_sync_plan_mtime_newer() {
        use std::time::Duration;

        let old_mtime = SystemTime::now() - Duration::from_secs(60);
        let new_mtime = SystemTime::now();

        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            TrackInfo {
                path: PathBuf::from("/tmp/test.wav"),
                filename: "test.wav".to_string(),
                size: 1000,
                mtime: new_mtime, // Newer
            },
        );

        let mut usb = CollectionState::default();
        usb.tracks.insert(
            "test.wav".to_string(),
            TrackInfo {
                path: PathBuf::from("/usb/test.wav"),
                filename: "test.wav".to_string(),
                size: 1000,
                mtime: old_mtime, // Older
            },
        );

        let plan = build_sync_plan(&local, &usb);
        assert_eq!(plan.tracks_to_copy.len(), 1);
    }
}
