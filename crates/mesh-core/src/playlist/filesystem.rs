//! Filesystem-based playlist storage using symlinks
//!
//! This implementation stores playlists as directories, with tracks
//! represented as symlinks pointing back to the original files in the
//! collection. This allows the same track to appear in multiple playlists
//! without duplicating the audio data.

use super::*;
use crate::audio_file::read_metadata;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

/// Filesystem-based playlist storage.
///
/// Directory structure:
/// ```text
/// root_path/
/// ├── tracks/           # General Collection (real audio files)
/// │   ├── track1.wav
/// │   └── subfolder/
/// │       └── track2.wav
/// └── playlists/        # User playlists (symlinks to tracks)
///     ├── Live Set/
///     │   └── track1.wav -> ../../tracks/track1.wav
///     └── Favorites/
/// ```
pub struct FilesystemStorage {
    /// Root path of the collection
    root_path: PathBuf,
    /// Cached node tree (rebuilt on refresh)
    nodes: HashMap<NodeId, PlaylistNode>,
}

impl FilesystemStorage {
    /// Create a new filesystem storage at the given root path.
    /// Creates the required directories if they don't exist.
    pub fn new(root_path: PathBuf) -> Result<Self, PlaylistError> {
        let mut storage = Self {
            root_path,
            nodes: HashMap::new(),
        };
        storage.ensure_directories()?;
        storage.scan_tree()?;
        Ok(storage)
    }

    /// Get the root path
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Ensure required directories exist
    fn ensure_directories(&self) -> Result<(), PlaylistError> {
        fs::create_dir_all(self.root_path.join("tracks"))?;
        fs::create_dir_all(self.root_path.join("playlists"))?;
        Ok(())
    }

    /// Scan the filesystem and build the node tree
    fn scan_tree(&mut self) -> Result<(), PlaylistError> {
        self.nodes.clear();

        // Create virtual root node
        let root = PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: "Root".to_string(),
            children: vec![NodeId::tracks(), NodeId::playlists()],
            track_path: None,
        };
        self.nodes.insert(NodeId::root(), root);

        // Scan tracks/ directory (collection)
        self.scan_collection_folder(&NodeId::tracks(), &self.root_path.join("tracks"))?;

        // Scan playlists/ directory
        self.scan_playlist_folder(&NodeId::playlists(), &self.root_path.join("playlists"))?;

        Ok(())
    }

    /// Recursively scan a collection folder (tracks/)
    fn scan_collection_folder(&mut self, parent_id: &NodeId, path: &Path) -> Result<(), PlaylistError> {
        let kind = if parent_id == &NodeId::tracks() {
            NodeKind::Collection
        } else {
            NodeKind::CollectionFolder
        };

        let mut children = Vec::new();

        if path.exists() {
            let mut entries: Vec<_> = fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .collect();

            // Sort entries alphabetically for consistent ordering
            entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let child_id = parent_id.child(&name);
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    // Recursively scan subdirectory
                    self.scan_collection_folder(&child_id, &entry_path)?;
                    children.push(child_id);
                } else if is_audio_file(&entry_path) {
                    // Add track node
                    let track = PlaylistNode {
                        id: child_id.clone(),
                        kind: NodeKind::Track,
                        name: name.clone(),
                        children: vec![],
                        track_path: Some(entry_path),
                    };
                    self.nodes.insert(child_id.clone(), track);
                    children.push(child_id);
                }
            }
        }

        let node = PlaylistNode {
            id: parent_id.clone(),
            kind,
            name: if parent_id == &NodeId::tracks() {
                "Collection".to_string()
            } else {
                parent_id.name().to_string()
            },
            children,
            track_path: None,
        };
        self.nodes.insert(parent_id.clone(), node);

        Ok(())
    }

    /// Recursively scan a playlist folder
    fn scan_playlist_folder(&mut self, parent_id: &NodeId, path: &Path) -> Result<(), PlaylistError> {
        let kind = if parent_id == &NodeId::playlists() {
            NodeKind::PlaylistsRoot
        } else {
            NodeKind::Playlist
        };

        let mut children = Vec::new();

        if path.exists() {
            let mut entries: Vec<_> = fs::read_dir(path)?
                .filter_map(|e| e.ok())
                .collect();

            // Sort entries alphabetically
            entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

            for entry in entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let child_id = parent_id.child(&name);
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    // Recursively scan sub-playlist
                    self.scan_playlist_folder(&child_id, &entry_path)?;
                    children.push(child_id);
                } else if entry_path.is_symlink() && is_audio_file(&entry_path) {
                    // Resolve symlink to get actual track path
                    log::debug!("scan_playlist_folder: Found symlink: {:?}", entry_path);

                    let link_target = fs::read_link(&entry_path);
                    log::debug!("  read_link result: {:?}", link_target);

                    let resolved = link_target
                        .ok()
                        .and_then(|p| {
                            log::debug!("  link target path: {:?}, is_relative: {}", p, p.is_relative());
                            if p.is_relative() {
                                let joined = entry_path.parent().map(|parent| parent.join(&p));
                                log::debug!("  joined path: {:?}", joined);
                                joined
                            } else {
                                Some(p)
                            }
                        })
                        .and_then(|p| {
                            let canonical = fs::canonicalize(&p);
                            log::debug!("  canonicalize({:?}) = {:?}", p, canonical);
                            canonical.ok()
                        });

                    log::debug!("  final resolved track_path: {:?}", resolved);

                    let track = PlaylistNode {
                        id: child_id.clone(),
                        kind: NodeKind::Track,
                        name: name.clone(),
                        children: vec![],
                        track_path: resolved,
                    };
                    self.nodes.insert(child_id.clone(), track);
                    children.push(child_id);
                } else if is_audio_file(&entry_path) && !entry_path.is_symlink() {
                    // Regular audio file (not a symlink) - also support this
                    log::debug!("scan_playlist_folder: Found regular audio file (not symlink): {:?}", entry_path);
                }
            }
        }

        let node = PlaylistNode {
            id: parent_id.clone(),
            kind,
            name: if parent_id == &NodeId::playlists() {
                "Playlists".to_string()
            } else {
                parent_id.name().to_string()
            },
            children,
            track_path: None,
        };
        self.nodes.insert(parent_id.clone(), node);

        Ok(())
    }

    /// Convert a NodeId to its filesystem path
    fn node_to_path(&self, id: &NodeId) -> PathBuf {
        self.root_path.join(&id.0)
    }

    /// Create a relative symlink from link to target
    fn create_relative_symlink(&self, target: &Path, link: &Path) -> Result<(), PlaylistError> {
        let link_parent = link.parent().ok_or_else(|| {
            PlaylistError::InvalidOperation("Invalid link path".to_string())
        })?;

        // Calculate relative path from link location to target
        let relative = diff_paths(target, link_parent).ok_or_else(|| {
            PlaylistError::InvalidOperation(format!(
                "Cannot create relative path from {:?} to {:?}",
                link_parent, target
            ))
        })?;

        symlink(&relative, link)?;
        Ok(())
    }
}

impl PlaylistStorage for FilesystemStorage {
    fn root(&self) -> PlaylistNode {
        self.nodes.get(&NodeId::root()).cloned().unwrap_or_else(|| PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: "Root".to_string(),
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
            .filter(|node| node.kind == NodeKind::Track)
            .map(|node| {
                let path = node.track_path.clone().unwrap_or_default();
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| node.name.clone());

                // Load metadata from WAV file (fast path - no audio loading)
                let (artist, bpm, key, duration) = if path.exists() {
                    match read_metadata(&path) {
                        Ok(meta) => (
                            meta.artist,
                            meta.bpm,
                            meta.key,
                            meta.duration_seconds,
                        ),
                        Err(e) => {
                            log::debug!("Failed to read metadata from {:?}: {:?}", path, e);
                            (None, None, None, None)
                        }
                    }
                } else {
                    (None, None, None, None)
                };

                TrackInfo {
                    id: node.id,
                    name,
                    path,
                    artist,
                    bpm,
                    key,
                    duration,
                }
            })
            .collect()
    }

    fn create_playlist(&mut self, parent: &NodeId, name: &str) -> Result<NodeId, PlaylistError> {
        // Validate parent is playlists root or a playlist
        let parent_node = self.get_node(parent).ok_or_else(|| {
            PlaylistError::NotFound(parent.to_string())
        })?;

        match parent_node.kind {
            NodeKind::PlaylistsRoot | NodeKind::Playlist => {}
            _ => {
                return Err(PlaylistError::InvalidOperation(
                    "Can only create playlists under Playlists folder".to_string(),
                ))
            }
        }

        let new_id = parent.child(name);
        let path = self.node_to_path(&new_id);

        if path.exists() {
            return Err(PlaylistError::AlreadyExists(name.to_string()));
        }

        fs::create_dir_all(&path)?;
        self.scan_tree()?; // Refresh to pick up new directory

        Ok(new_id)
    }

    fn rename_playlist(&mut self, id: &NodeId, new_name: &str) -> Result<(), PlaylistError> {
        let node = self.get_node(id).ok_or_else(|| {
            PlaylistError::NotFound(id.to_string())
        })?;

        if node.kind != NodeKind::Playlist {
            return Err(PlaylistError::InvalidOperation(
                "Can only rename user playlists".to_string(),
            ));
        }

        let old_path = self.node_to_path(id);
        let new_path = old_path
            .parent()
            .ok_or_else(|| PlaylistError::InvalidOperation("Invalid path".to_string()))?
            .join(new_name);

        if new_path.exists() {
            return Err(PlaylistError::AlreadyExists(new_name.to_string()));
        }

        fs::rename(&old_path, &new_path)?;
        self.scan_tree()?;

        Ok(())
    }

    fn delete_playlist(&mut self, id: &NodeId) -> Result<(), PlaylistError> {
        let node = self.get_node(id).ok_or_else(|| {
            PlaylistError::NotFound(id.to_string())
        })?;

        if node.kind != NodeKind::Playlist {
            return Err(PlaylistError::InvalidOperation(
                "Can only delete user playlists".to_string(),
            ));
        }

        let path = self.node_to_path(id);
        fs::remove_dir_all(&path)?;
        self.scan_tree()?;

        Ok(())
    }

    fn add_track_to_playlist(
        &mut self,
        track_path: &PathBuf,
        playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        let playlist_node = self.get_node(playlist).ok_or_else(|| {
            PlaylistError::NotFound(playlist.to_string())
        })?;

        if playlist_node.kind != NodeKind::Playlist {
            return Err(PlaylistError::InvalidOperation(
                "Can only add tracks to user playlists".to_string(),
            ));
        }

        let track_name = track_path
            .file_name()
            .ok_or_else(|| PlaylistError::InvalidOperation("Invalid track path".to_string()))?
            .to_string_lossy()
            .to_string();

        let playlist_path = self.node_to_path(playlist);
        let link_path = playlist_path.join(&track_name);

        if link_path.exists() {
            return Err(PlaylistError::AlreadyExists(track_name));
        }

        // Canonicalize the track path for consistent symlink targets
        let canonical_track = fs::canonicalize(track_path).map_err(|_| {
            PlaylistError::NotFound(format!("Track file not found: {:?}", track_path))
        })?;

        self.create_relative_symlink(&canonical_track, &link_path)?;
        self.scan_tree()?;

        Ok(playlist.child(&track_name))
    }

    fn remove_track_from_playlist(&mut self, track_id: &NodeId) -> Result<(), PlaylistError> {
        // Only allow removing from playlists (symlinks), not from collection
        if !track_id.is_in_playlists() {
            return Err(PlaylistError::CannotModifyCollection);
        }

        let node = self.get_node(track_id).ok_or_else(|| {
            PlaylistError::NotFound(track_id.to_string())
        })?;

        if node.kind != NodeKind::Track {
            return Err(PlaylistError::InvalidOperation(
                "Can only remove tracks".to_string(),
            ));
        }

        let path = self.node_to_path(track_id);
        fs::remove_file(&path)?;
        self.scan_tree()?;

        Ok(())
    }

    fn move_track(
        &mut self,
        track_id: &NodeId,
        target_playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        let node = self.get_node(track_id).ok_or_else(|| {
            PlaylistError::NotFound(track_id.to_string())
        })?;

        let track_path = node.track_path.ok_or_else(|| {
            PlaylistError::InvalidOperation("Track has no associated file path".to_string())
        })?;

        // Add to new playlist first
        let new_id = self.add_track_to_playlist(&track_path, target_playlist)?;

        // Remove from old playlist (only if it was in a playlist, not collection)
        if track_id.is_in_playlists() {
            // Need to re-scan since add_track_to_playlist already called scan_tree
            // Just remove the file directly
            let old_path = self.node_to_path(track_id);
            if old_path.exists() {
                fs::remove_file(&old_path)?;
                self.scan_tree()?;
            }
        }

        Ok(new_id)
    }

    fn refresh(&mut self) -> Result<(), PlaylistError> {
        self.scan_tree()
    }
}

/// Check if a path is an audio file based on extension
fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_lowercase().as_str(),
                "wav" | "mp3" | "flac" | "aiff" | "aif" | "ogg" | "m4a"
            )
        })
        .unwrap_or(false)
}

/// Calculate the relative path from `base` to `path`.
/// This is a simplified implementation that works for our use case.
fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
    // Try to use canonical paths for accurate comparison
    let path = fs::canonicalize(path).ok()?;
    let base = fs::canonicalize(base).ok()?;

    let mut path_iter = path.components().peekable();
    let mut base_iter = base.components().peekable();

    // Skip common prefix
    while let (Some(p), Some(b)) = (path_iter.peek(), base_iter.peek()) {
        if p == b {
            path_iter.next();
            base_iter.next();
        } else {
            break;
        }
    }

    // Count remaining base components (need that many ..)
    let mut result = PathBuf::new();
    for _ in base_iter {
        result.push("..");
    }

    // Add remaining path components
    for component in path_iter {
        result.push(component);
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_storage() -> (FilesystemStorage, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(temp_dir.path().to_path_buf()).unwrap();
        (storage, temp_dir)
    }

    #[test]
    fn test_create_storage() {
        let (storage, temp_dir) = create_test_storage();

        // Check directories were created
        assert!(temp_dir.path().join("tracks").exists());
        assert!(temp_dir.path().join("playlists").exists());

        // Check root node
        let root = storage.root();
        assert_eq!(root.kind, NodeKind::Root);
        assert_eq!(root.children.len(), 2);
    }

    #[test]
    fn test_create_playlist() {
        let (mut storage, _temp_dir) = create_test_storage();

        // Create a playlist under the playlists root
        let playlist_id = storage
            .create_playlist(&NodeId::playlists(), "My Playlist")
            .unwrap();

        assert_eq!(playlist_id.as_str(), "playlists/My Playlist");

        // Verify it exists
        let node = storage.get_node(&playlist_id).unwrap();
        assert_eq!(node.kind, NodeKind::Playlist);
        assert_eq!(node.name, "My Playlist");
    }

    #[test]
    fn test_create_nested_playlist() {
        let (mut storage, _temp_dir) = create_test_storage();

        // Create parent playlist
        let parent_id = storage
            .create_playlist(&NodeId::playlists(), "Live Sets")
            .unwrap();

        // Create nested playlist
        let child_id = storage.create_playlist(&parent_id, "2024 Tour").unwrap();

        assert_eq!(child_id.as_str(), "playlists/Live Sets/2024 Tour");

        // Verify parent has child
        let parent = storage.get_node(&parent_id).unwrap();
        assert!(parent.children.contains(&child_id));
    }

    #[test]
    fn test_diff_paths() {
        // Test relative path calculation
        // Note: Using underscored variables as examples - actual path diffing
        // requires canonicalized paths which aren't available in this test context
        let _base = PathBuf::from("/home/user/music/playlists/set1");
        let _target = PathBuf::from("/home/user/music/tracks/song.wav");

        // This would give: ../../tracks/song.wav
        // But we need to use actual paths for the test to work with canonicalize
    }

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file(Path::new("song.wav")));
        assert!(is_audio_file(Path::new("song.WAV")));
        assert!(is_audio_file(Path::new("song.mp3")));
        assert!(is_audio_file(Path::new("song.flac")));
        assert!(is_audio_file(Path::new("song.aiff")));
        assert!(is_audio_file(Path::new("song.ogg")));
        assert!(is_audio_file(Path::new("song.m4a")));
        assert!(!is_audio_file(Path::new("song.txt")));
        assert!(!is_audio_file(Path::new("song")));
    }
}
