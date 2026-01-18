//! USB storage backend implementing PlaylistStorage
//!
//! Reads playlists and track metadata from USB's mesh.db database.
//! Track audio files are stored in tracks/ but browsing is playlist-only.

use super::{ExportableConfig, UsbDevice};
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

        // Try to open the USB database
        let db_service = DatabaseService::new(&collection_root).ok();

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
    /// Playlists are direct children of root - no extra nesting.
    fn build_tree_from_db(&mut self) {
        self.nodes.clear();

        // Build playlists from database - they become direct children of root
        let mut playlist_children = Vec::new();

        if let Some(ref db_service) = self.db_service {
            // Get all playlists from database
            if let Ok(playlists) = PlaylistQuery::get_all(db_service.db()) {
                for playlist in playlists {
                    // Playlists are direct children of root (no "playlists/" prefix)
                    let playlist_id = NodeId(format!("playlist:{}", playlist.name));
                    playlist_children.push(playlist_id.clone());

                    // Get tracks in this playlist
                    let mut track_children = Vec::new();
                    if let Ok(tracks) = PlaylistQuery::get_tracks(db_service.db(), playlist.id) {
                        for track in tracks {
                            // Build track path pointing to tracks/ folder
                            let filename = PathBuf::from(&track.path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&track.name)
                                .to_string();

                            let track_id = playlist_id.child(&filename);
                            let track_path = self.collection_root.join("tracks").join(&filename);

                            let track_node = PlaylistNode {
                                id: track_id.clone(),
                                kind: NodeKind::Track,
                                name: track.name.clone(),
                                children: Vec::new(),
                                track_path: Some(track_path),
                            };
                            self.nodes.insert(track_id.clone(), track_node);
                            track_children.push(track_id);
                        }
                    }

                    // Create playlist node
                    let playlist_node = PlaylistNode {
                        id: playlist_id.clone(),
                        kind: NodeKind::Playlist,
                        name: playlist.name,
                        children: track_children,
                        track_path: None,
                    };
                    self.nodes.insert(playlist_id, playlist_node);
                }
            }
        }

        // Create root node with playlists as direct children
        let root = PlaylistNode {
            id: NodeId::root(),
            kind: NodeKind::Root,
            name: self.device.label.clone(),
            children: playlist_children,
            track_path: None,
        };
        self.nodes.insert(NodeId::root(), root);
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

        // Extract playlist name from folder_id (e.g., "playlist:My Set" -> "My Set")
        let playlist_name = folder_id.as_str().strip_prefix("playlist:").unwrap_or("");
        if playlist_name.is_empty() {
            return Vec::new();
        }

        // Find playlist by name
        let Ok(Some(playlist)) = db_service.get_playlist_by_name(playlist_name, None) else {
            return Vec::new();
        };

        // Get tracks from database
        let Ok(tracks) = PlaylistQuery::get_tracks(db_service.db(), playlist.id) else {
            return Vec::new();
        };

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
