//! Database-backed playlist storage using CozoDB
//!
//! This implementation provides O(1) track metadata access by reading from
//! the database instead of scanning WAV files. The database is populated
//! during import/analysis and kept in sync via file watchers.

use super::{NodeId, NodeKind, PlaylistError, PlaylistNode, PlaylistStorage, TrackInfo};
use crate::db::{DatabaseService, TrackQuery, PlaylistQuery};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Database-backed playlist storage
///
/// Uses CozoDB for track metadata, providing instant access without file I/O.
/// The tree structure is cached in memory and rebuilt on refresh.
pub struct DatabaseStorage {
    /// Shared database service
    service: Arc<DatabaseService>,
    /// Cached tree structure (folder hierarchy)
    tree_cache: RwLock<TreeCache>,
}

/// Cached tree structure for fast navigation
struct TreeCache {
    /// All folder nodes indexed by ID
    folders: HashMap<String, CachedFolder>,
    /// Track count per folder (for lazy loading indicator)
    track_counts: HashMap<String, usize>,
    /// Whether cache needs refresh
    dirty: bool,
}

/// A cached folder node
struct CachedFolder {
    id: NodeId,
    kind: NodeKind,
    name: String,
    children: Vec<NodeId>,
}

impl DatabaseStorage {
    /// Create a new database storage backed by the given DatabaseService
    ///
    /// # Arguments
    /// * `service` - Shared database service
    pub fn new(service: Arc<DatabaseService>) -> Result<Self, PlaylistError> {
        let storage = Self {
            service,
            tree_cache: RwLock::new(TreeCache {
                folders: HashMap::new(),
                track_counts: HashMap::new(),
                dirty: true,
            }),
        };

        // Build initial cache
        storage.rebuild_tree_cache()?;

        Ok(storage)
    }

    /// Rebuild the tree cache from database
    fn rebuild_tree_cache(&self) -> Result<(), PlaylistError> {
        let mut cache = self.tree_cache.write()
            .map_err(|_| PlaylistError::InvalidOperation("Lock poisoned".into()))?;

        cache.folders.clear();
        cache.track_counts.clear();

        // Get all unique folder paths from tracks
        let folders = TrackQuery::get_folders(self.service.db())
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Build the root structure
        let root_id = NodeId::root();
        let tracks_id = NodeId::tracks();
        let playlists_id = NodeId::playlists();

        // Root node
        cache.folders.insert(root_id.0.clone(), CachedFolder {
            id: root_id.clone(),
            kind: NodeKind::Root,
            name: String::new(),
            children: vec![tracks_id.clone(), playlists_id.clone()],
        });

        // Tracks (collection) node
        let mut tracks_children = Vec::new();
        for folder_path in &folders {
            // Extract subfolder structure
            if let Some(subfolder) = folder_path.strip_prefix("tracks/") {
                if !subfolder.contains('/') {
                    // Direct child of tracks
                    tracks_children.push(NodeId(folder_path.clone()));
                }
            }
        }
        cache.folders.insert(tracks_id.0.clone(), CachedFolder {
            id: tracks_id,
            kind: NodeKind::Collection,
            name: "General Collection".into(),
            children: tracks_children,
        });

        // Playlists root node
        let playlists = PlaylistQuery::get_roots(self.service.db())
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;
        let playlist_children: Vec<NodeId> = playlists
            .iter()
            .map(|p| NodeId(format!("playlists/{}", p.name)))
            .collect();
        cache.folders.insert(playlists_id.0.clone(), CachedFolder {
            id: playlists_id,
            kind: NodeKind::PlaylistsRoot,
            name: "Playlists".into(),
            children: playlist_children.clone(),
        });

        // Add each playlist folder
        for playlist in playlists {
            let id = NodeId(format!("playlists/{}", playlist.name));
            cache.folders.insert(id.0.clone(), CachedFolder {
                id: id.clone(),
                kind: NodeKind::Playlist,
                name: playlist.name,
                children: Vec::new(), // Tracks loaded on demand
            });
        }

        // Add collection subfolders
        for folder_path in folders {
            if folder_path != "tracks" && folder_path.starts_with("tracks/") {
                let name = folder_path.rsplit_once('/')
                    .map(|(_, n)| n.to_string())
                    .unwrap_or_else(|| folder_path.clone());
                let id = NodeId(folder_path.clone());
                cache.folders.insert(folder_path.clone(), CachedFolder {
                    id,
                    kind: NodeKind::CollectionFolder,
                    name,
                    children: Vec::new(),
                });
            }
        }

        cache.dirty = false;
        Ok(())
    }

    /// Convert database Track to TrackInfo
    fn track_to_info(track: &crate::db::Track, folder_id: &NodeId) -> TrackInfo {
        TrackInfo {
            id: NodeId(format!("{}/{}", folder_id.0, track.name)),
            name: track.name.clone(),
            path: PathBuf::from(&track.path),
            artist: track.artist.clone(),
            bpm: track.bpm,
            key: track.key.clone(),
            duration: Some(track.duration_seconds),
            lufs: track.lufs,
        }
    }

    /// Get database playlist ID from a NodeId like "playlists/MyPlaylist"
    fn get_playlist_db_id(&self, node_id: &NodeId) -> Result<i64, PlaylistError> {
        // Extract the playlist name from the path
        let playlist_name = if node_id.0.starts_with("playlists/") {
            node_id.0.strip_prefix("playlists/").unwrap_or(&node_id.0)
        } else {
            return Err(PlaylistError::NotFound(node_id.0.clone()));
        };

        // Handle nested playlists: "playlists/Parent/Child" -> name="Child", parent="Parent"
        let (parent_name, name) = if let Some((parent_path, name)) = playlist_name.rsplit_once('/') {
            (Some(parent_path), name)
        } else {
            (None, playlist_name)
        };

        // Look up parent ID first if needed
        let parent_id = if let Some(pname) = parent_name {
            let parent = PlaylistQuery::get_by_name(self.service.db(), pname, None)
                .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?
                .ok_or_else(|| PlaylistError::NotFound(format!("Parent playlist: {}", pname)))?;
            Some(parent.id)
        } else {
            None
        };

        // Look up the playlist
        let playlist = PlaylistQuery::get_by_name(self.service.db(), name, parent_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?
            .ok_or_else(|| PlaylistError::NotFound(node_id.0.clone()))?;

        Ok(playlist.id)
    }

    /// Get database track ID from a path in the tracks folder
    fn get_track_db_id(&self, track_path: &PathBuf) -> Result<i64, PlaylistError> {
        // Search for track by path
        let track = TrackQuery::get_by_path(self.service.db(), track_path.to_str().unwrap_or(""))
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?
            .ok_or_else(|| PlaylistError::NotFound(track_path.display().to_string()))?;

        Ok(track.id)
    }
}

impl PlaylistStorage for DatabaseStorage {
    fn root(&self) -> PlaylistNode {
        PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: String::new(),
            children: vec![NodeId::tracks(), NodeId::playlists()],
            track_path: None,
        }
    }

    fn get_node(&self, id: &NodeId) -> Option<PlaylistNode> {
        let cache = self.tree_cache.read().ok()?;
        let folder = cache.folders.get(&id.0)?;

        Some(PlaylistNode {
            id: folder.id.clone(),
            kind: folder.kind,
            name: folder.name.clone(),
            children: folder.children.clone(),
            track_path: None,
        })
    }

    fn get_children(&self, id: &NodeId) -> Vec<PlaylistNode> {
        let cache = match self.tree_cache.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        let folder = match cache.folders.get(&id.0) {
            Some(f) => f,
            None => return Vec::new(),
        };

        folder.children.iter().filter_map(|child_id| {
            cache.folders.get(&child_id.0).map(|f| PlaylistNode {
                id: f.id.clone(),
                kind: f.kind,
                name: f.name.clone(),
                children: f.children.clone(),
                track_path: None,
            })
        }).collect()
    }

    fn get_tracks(&self, folder_id: &NodeId) -> Vec<TrackInfo> {
        // This is the key optimization - read from DB instead of files!
        let folder_path = &folder_id.0;

        let tracks = match TrackQuery::get_by_folder(self.service.db(), folder_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("Failed to get tracks for folder {}: {}", folder_path, e);
                return Vec::new();
            }
        };

        tracks.iter()
            .map(|track| Self::track_to_info(track, folder_id))
            .collect()
    }

    fn create_playlist(&mut self, parent: &NodeId, name: &str) -> Result<NodeId, PlaylistError> {
        if !parent.0.starts_with("playlists") && parent.0 != "playlists" {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get parent playlist ID (if creating nested playlist)
        let parent_db_id = if parent.0 == "playlists" {
            None
        } else {
            Some(self.get_playlist_db_id(parent)?)
        };

        // Insert into database
        let _db_id = PlaylistQuery::create(self.service.db(), name, parent_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        let new_id = NodeId(format!("{}/{}", parent.0, name));

        // Invalidate and rebuild cache
        self.rebuild_tree_cache()?;

        Ok(new_id)
    }

    fn rename_playlist(&mut self, id: &NodeId, new_name: &str) -> Result<(), PlaylistError> {
        if !id.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get the database ID for this playlist
        let db_id = self.get_playlist_db_id(id)?;

        // Update in database
        PlaylistQuery::rename(self.service.db(), db_id, new_name)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Invalidate and rebuild cache
        self.rebuild_tree_cache()?;

        Ok(())
    }

    fn delete_playlist(&mut self, id: &NodeId) -> Result<(), PlaylistError> {
        if !id.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get the database ID for this playlist
        let db_id = self.get_playlist_db_id(id)?;

        // Delete from database (also removes track associations)
        PlaylistQuery::delete(self.service.db(), db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Invalidate and rebuild cache
        self.rebuild_tree_cache()?;

        Ok(())
    }

    fn add_track_to_playlist(
        &mut self,
        track_path: &PathBuf,
        playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        if !playlist.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get the database IDs
        let playlist_db_id = self.get_playlist_db_id(playlist)?;
        let track_db_id = self.get_track_db_id(track_path)?;

        // Get next sort order for the playlist
        let sort_order = PlaylistQuery::next_sort_order(self.service.db(), playlist_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Add track-playlist association
        PlaylistQuery::add_track(self.service.db(), playlist_db_id, track_db_id, sort_order)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        let track_name = track_path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        Ok(NodeId(format!("{}/{}", playlist.0, track_name)))
    }

    fn remove_track_from_playlist(&mut self, track_id: &NodeId) -> Result<(), PlaylistError> {
        if !track_id.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Extract playlist path and track name from the node ID
        // e.g., "playlists/MyPlaylist/track_name" -> playlist="playlists/MyPlaylist", track="track_name"
        let playlist_id = track_id.parent()
            .ok_or_else(|| PlaylistError::InvalidOperation("Invalid track ID".into()))?;

        let playlist_db_id = self.get_playlist_db_id(&playlist_id)?;

        // Find the track by name in the playlist to get its db ID
        let tracks = PlaylistQuery::get_tracks(self.service.db(), playlist_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        let track_name = track_id.name();
        let track = tracks.iter()
            .find(|t| t.name == track_name)
            .ok_or_else(|| PlaylistError::NotFound(track_id.0.clone()))?;

        // Remove track-playlist association
        PlaylistQuery::remove_track(self.service.db(), playlist_db_id, track.id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        Ok(())
    }

    fn move_track(
        &mut self,
        track_id: &NodeId,
        target_playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        if !target_playlist.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        // Get source playlist and track info
        let source_playlist_id = track_id.parent()
            .ok_or_else(|| PlaylistError::InvalidOperation("Invalid track ID".into()))?;

        let source_playlist_db_id = self.get_playlist_db_id(&source_playlist_id)?;
        let target_playlist_db_id = self.get_playlist_db_id(target_playlist)?;

        // Find the track by name
        let tracks = PlaylistQuery::get_tracks(self.service.db(), source_playlist_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        let track_name = track_id.name();
        let track = tracks.iter()
            .find(|t| t.name == track_name)
            .ok_or_else(|| PlaylistError::NotFound(track_id.0.clone()))?;

        let track_db_id = track.id;

        // Remove from source playlist
        PlaylistQuery::remove_track(self.service.db(), source_playlist_db_id, track_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Add to target playlist
        let sort_order = PlaylistQuery::next_sort_order(self.service.db(), target_playlist_db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        PlaylistQuery::add_track(self.service.db(), target_playlist_db_id, track_db_id, sort_order)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        Ok(NodeId(format!("{}/{}", target_playlist.0, track_name)))
    }

    fn refresh(&mut self) -> Result<(), PlaylistError> {
        self.rebuild_tree_cache()
    }

    fn delete_track_permanently(&mut self, track_id: &NodeId) -> Result<PathBuf, PlaylistError> {
        if !track_id.0.starts_with("tracks/") {
            return Err(PlaylistError::InvalidOperation(
                "Can only delete tracks from collection".into()
            ));
        }

        // Get track path from database
        let folder_path = track_id.parent()
            .map(|p| p.0.clone())
            .unwrap_or_default();

        let tracks = TrackQuery::get_by_folder(self.service.db(), &folder_path)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        let track_name = track_id.name();
        let track = tracks.iter()
            .find(|t| t.name == track_name)
            .ok_or_else(|| PlaylistError::NotFound(track_id.0.clone()))?;

        let path = PathBuf::from(&track.path);

        // Delete from database
        TrackQuery::delete(self.service.db(), track.id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

        // Delete actual file
        std::fs::remove_file(&path)?;

        // Invalidate cache
        if let Ok(mut cache) = self.tree_cache.write() {
            cache.dirty = true;
        }

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_database_storage_creation() {
        let temp_dir = TempDir::new().unwrap();
        let service = DatabaseService::in_memory(temp_dir.path()).unwrap();

        let storage = DatabaseStorage::new(service);
        assert!(storage.is_ok());
    }

    #[test]
    fn test_root_node() {
        let temp_dir = TempDir::new().unwrap();
        let service = DatabaseService::in_memory(temp_dir.path()).unwrap();
        let storage = DatabaseStorage::new(service).unwrap();

        let root = storage.root();
        assert_eq!(root.kind, NodeKind::Root);
        assert!(root.children.contains(&NodeId::tracks()));
        assert!(root.children.contains(&NodeId::playlists()));
    }

    #[test]
    fn test_get_tracks_empty_folder() {
        let temp_dir = TempDir::new().unwrap();
        let service = DatabaseService::in_memory(temp_dir.path()).unwrap();
        let storage = DatabaseStorage::new(service).unwrap();

        let tracks = storage.get_tracks(&NodeId::tracks());
        assert!(tracks.is_empty());
    }
}
