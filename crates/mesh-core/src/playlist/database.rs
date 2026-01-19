//! Database-backed playlist storage using CozoDB
//!
//! This implementation provides O(1) track metadata access by reading from
//! the database instead of scanning WAV files. The database is populated
//! during import/analysis and kept in sync via file watchers.
//!
//! No caching is needed - CozoDB queries are fast enough for real-time use.

use super::{NodeId, NodeKind, PlaylistError, PlaylistNode, PlaylistStorage, TrackInfo};
use crate::db::{DatabaseService, TrackQuery, PlaylistQuery};
use std::path::PathBuf;
use std::sync::Arc;

/// Database-backed playlist storage
///
/// Uses CozoDB for track metadata, providing instant access without file I/O.
/// Queries the database directly - no caching needed.
pub struct DatabaseStorage {
    /// Shared database service
    service: Arc<DatabaseService>,
}

impl DatabaseStorage {
    /// Create a new database storage backed by the given DatabaseService
    pub fn new(service: Arc<DatabaseService>) -> Result<Self, PlaylistError> {
        Ok(Self { service })
    }

    /// Convert database Track to TrackInfo
    fn track_to_info(track: &crate::db::Track, folder_id: &NodeId, order: i32) -> TrackInfo {
        TrackInfo {
            id: NodeId(format!("{}/{}", folder_id.0, track.name)),
            name: track.name.clone(),
            path: PathBuf::from(&track.path),
            order,
            artist: track.artist.clone(),
            bpm: track.bpm,
            key: track.key.clone(),
            duration: Some(track.duration_seconds),
            lufs: track.lufs,
        }
    }

    /// Get database playlist ID from a NodeId like "playlists/MyPlaylist"
    fn get_playlist_db_id(&self, node_id: &NodeId) -> Result<i64, PlaylistError> {
        let playlist_name = node_id.0.strip_prefix("playlists/")
            .ok_or_else(|| PlaylistError::NotFound(node_id.0.clone()))?;

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

        let playlist = PlaylistQuery::get_by_name(self.service.db(), name, parent_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?
            .ok_or_else(|| PlaylistError::NotFound(node_id.0.clone()))?;

        Ok(playlist.id)
    }

    /// Get database track ID from a file path
    fn get_track_db_id(&self, track_path: &PathBuf) -> Result<i64, PlaylistError> {
        let track = TrackQuery::get_by_path(self.service.db(), track_path.to_str().unwrap_or(""))
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?
            .ok_or_else(|| PlaylistError::NotFound(track_path.display().to_string()))?;

        Ok(track.id)
    }

    /// Get folder children (subfolders only, not tracks)
    fn get_folder_children(&self, folder_path: &str) -> Vec<NodeId> {
        let all_folders = match TrackQuery::get_folders(self.service.db()) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let prefix = if folder_path.is_empty() || folder_path == "tracks" {
            "tracks/"
        } else {
            return Vec::new(); // Only tracks folder has subfolders for now
        };

        all_folders
            .into_iter()
            .filter(|f| {
                if let Some(rest) = f.strip_prefix(prefix) {
                    !rest.contains('/') // Direct children only
                } else {
                    false
                }
            })
            .map(NodeId)
            .collect()
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
        // Handle special root nodes
        if id.0 == "" || id.0 == "root" {
            return Some(self.root());
        }
        if id.0 == "tracks" {
            return Some(PlaylistNode {
                id: NodeId::tracks(),
                kind: NodeKind::Collection,
                name: "General Collection".into(),
                children: self.get_folder_children("tracks"),
                track_path: None,
            });
        }
        if id.0 == "playlists" {
            let playlists = PlaylistQuery::get_roots(self.service.db()).unwrap_or_default();
            let children: Vec<NodeId> = playlists
                .iter()
                .map(|p| NodeId(format!("playlists/{}", p.name)))
                .collect();
            return Some(PlaylistNode {
                id: NodeId::playlists(),
                kind: NodeKind::PlaylistsRoot,
                name: "Playlists".into(),
                children,
                track_path: None,
            });
        }

        // Handle collection folders (e.g., "tracks/Subfolder")
        if id.0.starts_with("tracks/") && !id.0.contains('/') {
            // This is a subfolder, not a track
            let name = id.0.strip_prefix("tracks/").unwrap_or(&id.0).to_string();
            return Some(PlaylistNode {
                id: id.clone(),
                kind: NodeKind::CollectionFolder,
                name,
                children: Vec::new(),
                track_path: None,
            });
        }

        // Handle tracks in collection (e.g., "tracks/trackname" or "tracks/Subfolder/trackname")
        if id.0.starts_with("tracks/") {
            // Try to find this track in the database by name
            let track_name = id.name();
            let folder_path = id.parent().map(|p| p.0.clone()).unwrap_or_else(|| "tracks".to_string());

            let tracks = TrackQuery::get_by_folder(self.service.db(), &folder_path).ok()?;
            let track = tracks.iter().find(|t| t.name == track_name)?;

            return Some(PlaylistNode {
                id: id.clone(),
                kind: NodeKind::Track,
                name: track.name.clone(),
                children: Vec::new(),
                track_path: Some(PathBuf::from(&track.path)),
            });
        }

        // Handle playlists (e.g., "playlists/MyPlaylist")
        if id.0.starts_with("playlists/") {
            let playlist_name = id.0.strip_prefix("playlists/")?;
            // Could be a playlist or a track in a playlist

            // First check if it's a playlist
            if let Ok(Some(_)) = PlaylistQuery::get_by_name(self.service.db(), playlist_name, None) {
                return Some(PlaylistNode {
                    id: id.clone(),
                    kind: NodeKind::Playlist,
                    name: playlist_name.to_string(),
                    children: Vec::new(),
                    track_path: None,
                });
            }

            // Otherwise it might be a track in a playlist
            let parent = id.parent()?;
            let playlist_db_id = self.get_playlist_db_id(&parent).ok()?;
            let tracks = PlaylistQuery::get_tracks(self.service.db(), playlist_db_id).ok()?;
            let track_name = id.name();
            let track = tracks.iter().find(|t| t.name == track_name)?;

            return Some(PlaylistNode {
                id: id.clone(),
                kind: NodeKind::Track,
                name: track.name.clone(),
                children: Vec::new(),
                track_path: Some(PathBuf::from(&track.path)),
            });
        }

        None
    }

    fn get_children(&self, id: &NodeId) -> Vec<PlaylistNode> {
        if let Some(node) = self.get_node(id) {
            node.children.iter()
                .filter_map(|child_id| self.get_node(child_id))
                .collect()
        } else {
            Vec::new()
        }
    }

    fn get_tracks(&self, folder_id: &NodeId) -> Vec<TrackInfo> {
        let folder_path = &folder_id.0;

        // Handle playlist tracks
        if folder_path.starts_with("playlists/") {
            log::debug!("get_tracks: looking up playlist {:?}", folder_id);
            match self.get_playlist_db_id(folder_id) {
                Ok(playlist_db_id) => {
                    log::debug!("get_tracks: found playlist db_id={}", playlist_db_id);
                    match PlaylistQuery::get_tracks(self.service.db(), playlist_db_id) {
                        Ok(tracks) => {
                            log::debug!("get_tracks: found {} tracks in playlist", tracks.len());
                            // Tracks are already ordered by sort_order from DB, use enumerate for display order
                            return tracks.iter()
                                .enumerate()
                                .map(|(i, track)| TrackInfo {
                                    id: NodeId(format!("{}/{}", folder_id.0, track.name)),
                                    name: track.name.clone(),
                                    path: PathBuf::from(&track.path),
                                    order: (i + 1) as i32, // 1-based for display
                                    artist: track.artist.clone(),
                                    bpm: track.bpm,
                                    key: track.key.clone(),
                                    duration: Some(track.duration_seconds),
                                    lufs: track.lufs,
                                })
                                .collect();
                        }
                        Err(e) => {
                            log::warn!("get_tracks: failed to get tracks for playlist {}: {}", playlist_db_id, e);
                        }
                    }
                }
                Err(e) => {
                    log::warn!("get_tracks: failed to get playlist db_id for {:?}: {:?}", folder_id, e);
                }
            }
            return Vec::new();
        }

        // Handle collection tracks - use DatabaseService for proper Track type
        let tracks = match self.service.get_tracks_in_folder(folder_path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("Failed to get tracks for folder {}: {}", folder_path, e);
                return Vec::new();
            }
        };

        // Use enumerate for collection tracks - order represents import order
        tracks.iter()
            .enumerate()
            .map(|(i, track)| Self::track_to_info(track, folder_id, (i + 1) as i32))
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

        Ok(NodeId(format!("{}/{}", parent.0, name)))
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

        Ok(())
    }

    fn delete_playlist(&mut self, id: &NodeId) -> Result<(), PlaylistError> {
        if !id.0.starts_with("playlists/") {
            return Err(PlaylistError::CannotModifyCollection);
        }

        let db_id = self.get_playlist_db_id(id)?;

        // Delete from database (also removes track associations)
        PlaylistQuery::delete(self.service.db(), db_id)
            .map_err(|e| PlaylistError::InvalidOperation(e.to_string()))?;

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
