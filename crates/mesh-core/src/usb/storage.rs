//! USB storage backend implementing PlaylistStorage
//!
//! Reads playlists and track metadata from USB's mesh.db database.
//! Track audio files are stored in tracks/ but browsing is playlist-only.

use super::{ExportableConfig, UsbDevice, get_or_open_usb_database};
use crate::db::{DatabaseService, PlaylistQuery};
use crate::playlist::{NodeId, NodeKind, PlaylistError, PlaylistNode, PlaylistStorage, TrackInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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
    /// LUFS loudness value
    pub lufs: Option<f32>,
}

/// PlaylistStorage implementation for USB devices
///
/// Reads playlists from USB's mesh.db database. Only shows playlists,
/// not the raw tracks folder. This is read-only for mesh-player.
pub struct UsbStorage {
    /// The USB device
    device: UsbDevice,
    /// Root path of mesh-collection on USB
    collection_root: PathBuf,
    /// Database service for USB's mesh.db
    db_service: Option<Arc<DatabaseService>>,
    /// Cached node tree (built from database)
    nodes: HashMap<NodeId, PlaylistNode>,
    /// Whether write operations are allowed
    read_only: bool,
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

        // Get or open the USB database (uses centralized cache)
        let db_service = get_or_open_usb_database(&collection_root);

        let mut storage = Self {
            device,
            collection_root,
            db_service,
            nodes: HashMap::new(),
            read_only,
        };

        // Build the node tree from database
        storage.build_tree_from_db();

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

    /// Get the database service (for suggestion queries across all USB sources)
    pub fn db(&self) -> Option<&Arc<DatabaseService>> {
        self.db_service.as_ref()
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

    /// Build the node tree from USB's mesh.db database
    ///
    /// Respects playlist hierarchy (parent_id) for nested display.
    fn build_tree_from_db(&mut self) {
        self.nodes.clear();

        // Clone the Arc to avoid borrow conflict (db_service is Arc<DatabaseService>)
        let db_service = match self.db_service.clone() {
            Some(db) => db,
            None => {
                self.nodes.insert(NodeId::root(), PlaylistNode {
                    id: NodeId::root(),
                    kind: NodeKind::Root,
                    name: self.device.label.clone(),
                    children: Vec::new(),
                    track_path: None,
                });
                return;
            }
        };

        // Build root-level playlists, then recurse into children
        let root_playlists = PlaylistQuery::get_roots(db_service.db())
            .unwrap_or_default();

        let root_children = self.build_playlist_nodes(db_service.db(), &root_playlists, "playlist");

        let root = PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: self.device.label.clone(),
            children: root_children,
            track_path: None,
        };
        self.nodes.insert(NodeId::root(), root);
    }

    /// Recursively build playlist nodes with their tracks and child playlists
    fn build_playlist_nodes(
        &mut self,
        db: &crate::db::MeshDb,
        playlists: &[crate::db::Playlist],
        parent_prefix: &str,
    ) -> Vec<NodeId> {
        let mut children = Vec::new();

        for playlist in playlists {
            let playlist_id = NodeId(format!("{}:{}", parent_prefix, playlist.name));
            children.push(playlist_id.clone());

            // Build track nodes for this playlist
            let mut playlist_children = Vec::new();
            if let Ok(tracks) = PlaylistQuery::get_tracks(db, playlist.id) {
                for track in tracks {
                    let filename = PathBuf::from(&track.path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&track.name)
                        .to_string();

                    let track_id = playlist_id.child(&filename);
                    let track_path = self.collection_root.join("tracks").join(&filename);

                    self.nodes.insert(track_id.clone(), PlaylistNode {
                        id: track_id.clone(),
                        kind: NodeKind::Track,
                        name: track.name.clone(),
                        children: Vec::new(),
                        track_path: Some(track_path),
                    });
                    playlist_children.push(track_id);
                }
            }

            // Recursively build child playlists
            if let Ok(child_playlists) = PlaylistQuery::get_children(db, playlist.id) {
                if !child_playlists.is_empty() {
                    let child_prefix = format!("{}:{}", parent_prefix, playlist.name);
                    let nested = self.build_playlist_nodes(db, &child_playlists, &child_prefix);
                    playlist_children.extend(nested);
                }
            }

            self.nodes.insert(playlist_id.clone(), PlaylistNode {
                id: playlist_id,
                kind: NodeKind::Playlist,
                name: playlist.name.clone(),
                children: playlist_children,
                track_path: None,
            });
        }

        children
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
                children: vec![NodeId::playlists()],
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
        // For USB, we get track metadata from the database
        let Some(ref db_service) = self.db_service else {
            return Vec::new();
        };

        // Extract playlist path from folder_id (e.g., "playlist:Parent:Child" -> "Parent:Child")
        let playlist_path = folder_id.as_str().strip_prefix("playlist:").unwrap_or("");
        if playlist_path.is_empty() {
            return Vec::new();
        }

        // Walk the colon-separated path to resolve nested playlists.
        // e.g., "Diskroma:test" → find "Diskroma" (root), then "test" (child of Diskroma)
        let segments: Vec<&str> = playlist_path.split(':').collect();
        let mut parent_id: Option<i64> = None;
        let mut resolved_id = None;
        for segment in &segments {
            match db_service.get_playlist_by_name(segment, parent_id) {
                Ok(Some(pl)) => {
                    resolved_id = Some(pl.id);
                    parent_id = Some(pl.id);
                }
                _ => return Vec::new(),
            }
        }

        let Some(playlist_db_id) = resolved_id else {
            return Vec::new();
        };

        // Get tracks from database
        let Ok(tracks) = PlaylistQuery::get_tracks(db_service.db(), playlist_db_id) else {
            return Vec::new();
        };

        // Batch-load tags and cue counts for all tracks in this playlist
        let track_db_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        let tags_map = db_service.get_tags_batch(&track_db_ids).unwrap_or_default();
        let cue_counts = db_service.get_cue_counts_batch(&track_db_ids).unwrap_or_default();

        // Tracks are already ordered by sort_order from DB, use enumerate for display order
        tracks
            .into_iter()
            .enumerate()
            .map(|(i, track)| {
                let filename = PathBuf::from(&track.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&track.name)
                    .to_string();

                let track_path = self.collection_root.join("tracks").join(&filename);
                let track_id = folder_id.child(&filename);

                TrackInfo {
                    id: track_id,
                    name: track.name,
                    path: track_path,
                    order: (i + 1) as i32, // 1-based for display
                    artist: track.artist,
                    bpm: track.bpm,
                    key: track.key,
                    duration: Some(track.duration_seconds),
                    lufs: track.lufs,
                    tags: tags_map.get(&track.id).cloned().unwrap_or_default(),
                    cue_count: cue_counts.get(&track.id).copied().unwrap_or(0),
                }
            })
            .collect()
    }

    // USB is read-only for mesh-player - all write operations return errors

    fn create_playlist(&mut self, _parent: &NodeId, _name: &str) -> Result<NodeId, PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn rename_playlist(&mut self, _id: &NodeId, _new_name: &str) -> Result<(), PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn delete_playlist(&mut self, _id: &NodeId) -> Result<(), PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn add_track_to_playlist(
        &mut self,
        _track_path: &PathBuf,
        _playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn remove_track_from_playlist(&mut self, _track_id: &NodeId) -> Result<(), PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn move_track(
        &mut self,
        _track_id: &NodeId,
        _target_playlist: &NodeId,
    ) -> Result<NodeId, PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }

    fn delete_track_permanently(&mut self, _track_id: &NodeId) -> Result<PathBuf, PlaylistError> {
        Err(PlaylistError::CannotModifyCollection)
    }
}
