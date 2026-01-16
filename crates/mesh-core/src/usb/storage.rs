//! USB storage backend implementing PlaylistStorage
//!
//! This provides the same interface as FilesystemStorage but for USB devices.
//! Handles symlink vs copy behavior based on filesystem type.

use super::{ExportableConfig, UsbDevice};
use crate::playlist::{NodeId, NodeKind, PlaylistError, PlaylistNode, PlaylistStorage, TrackInfo};
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Normalize a path by resolving `.` and `..` components without requiring the path to exist.
///
/// This is used for symlink resolution where `canonicalize()` would fail because it
/// requires the target file to exist. This function handles paths like:
/// `/mount/usb/playlists/Detox/../../tracks/song.wav` → `/mount/usb/tracks/song.wav`
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Go up one level (pop last component, but not root)
                if !components.is_empty() {
                    // Don't pop root components (RootDir, Prefix)
                    if let Some(last) = components.last() {
                        if !matches!(last, Component::RootDir | Component::Prefix(_)) {
                            components.pop();
                        }
                    }
                }
            }
            Component::CurDir => {
                // Skip current dir references (.)
            }
            c => {
                components.push(c);
            }
        }
    }
    components.iter().collect()
}

/// Lightweight cached metadata for display (no waveforms or heavy data)
///
/// This is preloaded when a USB device connects to enable instant browsing.
#[derive(Debug, Clone)]
pub struct CachedTrackMetadata {
    /// Artist name
    pub artist: Option<String>,
    /// Beats per minute
    pub bpm: Option<f64>,
    /// Musical key
    pub key: Option<String>,
    /// Duration in seconds
    pub duration_seconds: Option<f64>,
    /// Number of cue points set
    pub cue_count: u8,
}

/// PlaylistStorage implementation for USB devices
///
/// By default, this is read-only for mesh-player. Write operations
/// are enabled for mesh-cue export.
pub struct UsbStorage {
    /// The USB device
    device: UsbDevice,
    /// Root path of mesh-collection on USB
    collection_root: PathBuf,
    /// Cached node tree
    nodes: HashMap<NodeId, PlaylistNode>,
    /// Whether write operations are allowed
    read_only: bool,
    /// Cached track metadata keyed by filename (for instant browsing)
    track_metadata_cache: HashMap<String, CachedTrackMetadata>,
}

impl UsbStorage {
    /// Create a new USB storage backend
    ///
    /// # Arguments
    /// * `device` - The USB device (must be mounted)
    /// * `read_only` - If true, all write operations will fail
    pub fn new(device: UsbDevice, read_only: bool) -> Result<Self, PlaylistError> {
        let mount_point = device
            .mount_point
            .as_ref()
            .ok_or_else(|| PlaylistError::InvalidOperation("Device not mounted".to_string()))?;

        let collection_root = mount_point.join("mesh-collection");

        let mut storage = Self {
            device,
            collection_root,
            nodes: HashMap::new(),
            read_only,
            track_metadata_cache: HashMap::new(),
        };

        // Scan the tree
        storage.refresh()?;

        Ok(storage)
    }

    /// Create for export (read-write mode)
    pub fn for_export(device: UsbDevice) -> Result<Self, PlaylistError> {
        Self::new(device, false)
    }

    /// Create for browsing (read-only mode)
    pub fn for_browsing(device: UsbDevice) -> Result<Self, PlaylistError> {
        Self::new(device, true)
    }

    /// Get the USB device
    pub fn device(&self) -> &UsbDevice {
        &self.device
    }

    /// Get the collection root path
    pub fn collection_root(&self) -> &PathBuf {
        &self.collection_root
    }

    /// Load config from USB if present
    pub fn load_config(&self) -> Option<ExportableConfig> {
        let config_path = self.collection_root.join("player-config.yaml");
        ExportableConfig::load(&config_path).ok()
    }

    /// Get all nodes (for export to mesh-player)
    pub fn all_nodes(&self) -> &HashMap<NodeId, PlaylistNode> {
        &self.nodes
    }

    /// Set the cached track metadata (called after background preload)
    pub fn set_metadata_cache(&mut self, metadata: HashMap<String, CachedTrackMetadata>) {
        self.track_metadata_cache = metadata;
    }

    /// Get cached metadata for a track by filename (instant, no I/O)
    pub fn get_cached_metadata(&self, filename: &str) -> Option<&CachedTrackMetadata> {
        self.track_metadata_cache.get(filename)
    }

    /// Check if metadata has been preloaded
    pub fn has_metadata_cache(&self) -> bool {
        !self.track_metadata_cache.is_empty()
    }

    /// Convert node ID to filesystem path
    fn node_to_path(&self, id: &NodeId) -> PathBuf {
        if id.is_root() {
            self.collection_root.clone()
        } else {
            self.collection_root.join(id.as_str())
        }
    }

    /// Scan the filesystem and rebuild the node tree
    fn scan_tree(&mut self) -> Result<(), PlaylistError> {
        self.nodes.clear();

        // Create root node
        let root = PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: self.device.label.clone(),
            children: vec![NodeId::tracks(), NodeId::playlists()],
            track_path: None,
        };
        self.nodes.insert(NodeId::root(), root);

        // Scan tracks directory
        let tracks_dir = self.collection_root.join("tracks");
        if tracks_dir.exists() {
            self.scan_directory(&NodeId::tracks(), &tracks_dir, true)?;
        } else {
            // Create empty tracks node
            let tracks_node = PlaylistNode {
                id: NodeId::tracks(),
                kind: NodeKind::Collection,
                name: "Collection".to_string(),
                children: Vec::new(),
                track_path: None,
            };
            self.nodes.insert(NodeId::tracks(), tracks_node);
        }

        // Scan playlists directory
        let playlists_dir = self.collection_root.join("playlists");
        if playlists_dir.exists() {
            self.scan_directory(&NodeId::playlists(), &playlists_dir, false)?;
        } else {
            // Create empty playlists node
            let playlists_node = PlaylistNode {
                id: NodeId::playlists(),
                kind: NodeKind::PlaylistsRoot,
                name: "Playlists".to_string(),
                children: Vec::new(),
                track_path: None,
            };
            self.nodes.insert(NodeId::playlists(), playlists_node);
        }

        Ok(())
    }

    /// Recursively scan a directory
    fn scan_directory(
        &mut self,
        parent_id: &NodeId,
        path: &Path,
        is_collection: bool,
    ) -> Result<(), PlaylistError> {
        let mut children = Vec::new();

        // Determine node kind for this directory
        let dir_kind = if parent_id.is_root() {
            if is_collection {
                NodeKind::Collection
            } else {
                NodeKind::PlaylistsRoot
            }
        } else if is_collection {
            NodeKind::CollectionFolder
        } else {
            NodeKind::Playlist
        };

        // Read directory entries
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => {
                // Directory might not exist yet
                let node = PlaylistNode {
                    id: parent_id.clone(),
                    kind: dir_kind,
                    name: path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown")
                        .to_string(),
                    children: Vec::new(),
                    track_path: None,
                };
                self.nodes.insert(parent_id.clone(), node);
                return Ok(());
            }
        };

        // Sort entries for consistent ordering
        let mut sorted_entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        sorted_entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for entry in sorted_entries {
            let entry_path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            let child_id = parent_id.child(&name);

            if entry_path.is_dir() {
                // Recursively scan subdirectory
                self.scan_directory(&child_id, &entry_path, is_collection)?;
                children.push(child_id);
            } else if is_audio_file(&entry_path) {
                // Resolve symlinks to get actual track path
                // Note: We use normalize_path() instead of canonicalize() because
                // canonicalize() requires the file to exist, which may fail on USB
                // due to mount timing or filesystem differences
                let track_path = if entry_path.is_symlink() {
                    match fs::read_link(&entry_path) {
                        Ok(link) => {
                            if link.is_absolute() {
                                link
                            } else {
                                // Resolve relative symlink manually
                                entry_path
                                    .parent()
                                    .map(|parent| {
                                        let joined = parent.join(&link);
                                        normalize_path(&joined)
                                    })
                                    .unwrap_or_else(|| {
                                        log::warn!("Could not resolve symlink parent for {:?}", entry_path);
                                        entry_path.clone()
                                    })
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to read symlink {:?}: {}", entry_path, e);
                            entry_path.clone()
                        }
                    }
                } else {
                    entry_path.clone()
                };

                let track_node = PlaylistNode {
                    id: child_id.clone(),
                    kind: NodeKind::Track,
                    name: name.trim_end_matches(".wav").to_string(),
                    children: Vec::new(),
                    track_path: Some(track_path),
                };
                self.nodes.insert(child_id.clone(), track_node);
                children.push(child_id);
            }
        }

        // Create parent node
        let parent_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(if is_collection { "Collection" } else { "Playlists" })
            .to_string();

        let parent_node = PlaylistNode {
            id: parent_id.clone(),
            kind: dir_kind,
            name: parent_name,
            children,
            track_path: None,
        };
        self.nodes.insert(parent_id.clone(), parent_node);

        Ok(())
    }

    /// Create a symlink (ext4) or copy (FAT32/exFAT) for a track
    ///
    /// Uses cross-platform symlink crate for portability across Linux/macOS/Windows.
    fn link_or_copy_track(&self, source: &Path, dest: &Path) -> Result<(), PlaylistError> {
        // Ensure parent directory exists
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        if self.device.supports_symlinks() {
            // Create relative symlink using cross-platform crate
            let rel_path = pathdiff::diff_paths(source, dest.parent().unwrap())
                .ok_or_else(|| {
                    PlaylistError::InvalidOperation("Could not compute relative path".to_string())
                })?;
            symlink::symlink_file(&rel_path, dest)?;
        } else {
            // Copy the file (FAT32/exFAT don't support symlinks)
            fs::copy(source, dest)?;
        }

        Ok(())
    }
}

impl PlaylistStorage for UsbStorage {
    fn root(&self) -> PlaylistNode {
        self.nodes
            .get(&NodeId::root())
            .cloned()
            .unwrap_or_else(|| PlaylistNode {
                id: NodeId::root(),
                kind: NodeKind::Root,
                name: self.device.label.clone(),
                children: vec![NodeId::tracks(), NodeId::playlists()],
                track_path: None,
            })
    }

    fn get_node(&self, id: &NodeId) -> Option<PlaylistNode> {
        self.nodes.get(id).cloned()
    }

    fn get_children(&self, id: &NodeId) -> Vec<PlaylistNode> {
        self.nodes
            .get(id)
            .map(|node| {
                node.children
                    .iter()
                    .filter_map(|child_id| self.nodes.get(child_id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn get_tracks(&self, folder_id: &NodeId) -> Vec<TrackInfo> {
        self.get_children(folder_id)
            .into_iter()
            .filter(|node| node.is_track())
            .map(|node| {
                let path = node.track_path.clone().unwrap_or_default();

                // Try cached metadata first (instant, no disk I/O)
                let filename = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                if let Some(cached) = self.track_metadata_cache.get(filename) {
                    // Use preloaded cache - instant!
                    return TrackInfo {
                        id: node.id,
                        name: node.name,
                        path,
                        artist: cached.artist.clone(),
                        bpm: cached.bpm,
                        key: cached.key.clone(),
                        duration: cached.duration_seconds,
                    };
                }

                // Fall back to reading from file (slow path, only if not preloaded)
                let metadata = crate::audio_file::read_metadata(&path).ok();

                TrackInfo {
                    id: node.id,
                    name: node.name,
                    path,
                    artist: metadata.as_ref().and_then(|m| m.artist.clone()),
                    bpm: metadata.as_ref().and_then(|m| m.bpm),
                    key: metadata.as_ref().and_then(|m| m.key.clone()),
                    duration: metadata.as_ref().and_then(|m| m.duration_seconds),
                }
            })
            .collect()
    }

    fn create_playlist(&mut self, parent: &NodeId, name: &str) -> Result<NodeId, PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Only allow creating playlists under playlists root
        if !parent.is_in_playlists() && *parent != NodeId::playlists() {
            return Err(PlaylistError::InvalidOperation(
                "Can only create playlists under Playlists".to_string(),
            ));
        }

        let playlist_id = parent.child(name);
        let playlist_path = self.node_to_path(&playlist_id);

        // Check if already exists
        if playlist_path.exists() {
            return Err(PlaylistError::AlreadyExists(name.to_string()));
        }

        fs::create_dir_all(&playlist_path)?;
        self.refresh()?;

        Ok(playlist_id)
    }

    fn rename_playlist(&mut self, id: &NodeId, new_name: &str) -> Result<(), PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        if !id.is_in_playlists() {
            return Err(PlaylistError::CannotModifyCollection);
        }

        let old_path = self.node_to_path(id);
        let new_id = id.parent().map(|p| p.child(new_name)).unwrap_or_else(|| NodeId(new_name.to_string()));
        let new_path = self.node_to_path(&new_id);

        if new_path.exists() {
            return Err(PlaylistError::AlreadyExists(new_name.to_string()));
        }

        fs::rename(&old_path, &new_path)?;
        self.refresh()?;

        Ok(())
    }

    fn delete_playlist(&mut self, id: &NodeId) -> Result<(), PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        if !id.is_in_playlists() {
            return Err(PlaylistError::CannotModifyCollection);
        }

        let path = self.node_to_path(id);
        if !path.exists() {
            return Err(PlaylistError::NotFound(id.to_string()));
        }

        fs::remove_dir_all(&path)?;
        self.refresh()?;

        Ok(())
    }

    fn add_track_to_playlist(
        &mut self,
        track_path: &PathBuf,
        playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        if !playlist.is_in_playlists() {
            return Err(PlaylistError::InvalidOperation(
                "Can only add tracks to playlists".to_string(),
            ));
        }

        let file_name = track_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| PlaylistError::InvalidOperation("Invalid track path".to_string()))?;

        let track_id = playlist.child(file_name);
        let dest_path = self.node_to_path(&track_id);

        self.link_or_copy_track(track_path, &dest_path)?;
        self.refresh()?;

        Ok(track_id)
    }

    fn remove_track_from_playlist(&mut self, track_id: &NodeId) -> Result<(), PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        if !track_id.is_in_playlists() {
            return Err(PlaylistError::InvalidOperation(
                "Can only remove tracks from playlists".to_string(),
            ));
        }

        let path = self.node_to_path(track_id);
        if !path.exists() {
            return Err(PlaylistError::NotFound(track_id.to_string()));
        }

        fs::remove_file(&path)?;
        self.refresh()?;

        Ok(())
    }

    fn move_track(
        &mut self,
        track_id: &NodeId,
        target_playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get source track path
        let node = self
            .get_node(track_id)
            .ok_or_else(|| PlaylistError::NotFound(track_id.to_string()))?;

        let track_path = node
            .track_path
            .ok_or_else(|| PlaylistError::InvalidOperation("Not a track".to_string()))?;

        // Remove from source
        self.remove_track_from_playlist(track_id)?;

        // Add to target
        self.add_track_to_playlist(&track_path, target_playlist)
    }

    fn refresh(&mut self) -> Result<(), PlaylistError> {
        self.scan_tree()
    }

    fn delete_track_permanently(&mut self, track_id: &NodeId) -> Result<PathBuf, PlaylistError> {
        if self.read_only {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Only allow deleting from collection (not playlists)
        if !track_id.is_in_tracks() {
            return Err(PlaylistError::InvalidOperation(
                "Can only permanently delete tracks from Collection".to_string(),
            ));
        }

        let path = self.node_to_path(track_id);
        if !path.exists() {
            return Err(PlaylistError::NotFound(track_id.to_string()));
        }

        let result_path = path.clone();
        fs::remove_file(&path)?;
        self.refresh()?;

        Ok(result_path)
    }
}

/// Check if a path is an audio file we support
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file(Path::new("test.wav")));
        assert!(is_audio_file(Path::new("test.WAV")));
        assert!(!is_audio_file(Path::new("test.mp3")));
        assert!(!is_audio_file(Path::new("test")));
    }
}
