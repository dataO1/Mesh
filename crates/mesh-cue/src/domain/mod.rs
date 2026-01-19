//! Domain layer for mesh-cue
//!
//! This module provides the core business logic and state management,
//! separating it from the UI layer. The domain layer:
//!
//! - Owns all services (DatabaseService, UsbManager, etc.) - **PRIVATE**
//! - Manages track metadata and editing state
//! - Coordinates background operations (import, reanalysis, export)
//! - Provides high-level operations for the UI (no direct service access)
//!
//! The UI layer should only handle display and user input, delegating
//! all business logic to this domain layer. The UI should NEVER access
//! database or storage services directly - only through domain methods.

mod state;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use mesh_core::db::{DatabaseService, Track, Playlist, CuePoint as DbCuePoint, SavedLoop as DbSavedLoop, StemLink as DbStemLink};
use mesh_core::audio_file::{CuePoint, SavedLoop, StemLinkReference};
use mesh_core::playlist::{DatabaseStorage, NodeId, NodeKind, PlaylistNode, PlaylistStorage};
use mesh_widgets::TrackRow;
use mesh_core::usb::{UsbManager, UsbCommand, UsbMessage, SyncPlan, ExportableConfig};
use mesh_widgets::TreeNode;

use crate::analysis::{AnalysisType, ReanalysisProgress};
use crate::audio::{AudioState, JackHandle, JackError, start_jack_client};
use crate::batch_import::{ImportProgress, StemGroup};
use crate::config::Config;
use crate::reanalysis::run_batch_reanalysis;

// Internal helper from UI utils (builds TreeNode from PlaylistStorage)
fn build_tree_nodes(storage: &dyn PlaylistStorage) -> Vec<TreeNode<NodeId>> {
    crate::ui::utils::build_tree_nodes(storage)
}

pub use state::LoadedTrackState;

// Re-export types that UI needs (domain types only, not service types)
pub use mesh_core::db::Track as DomainTrack;
pub use mesh_core::db::Playlist as DomainPlaylist;

/// Domain layer for mesh-cue application
///
/// This struct owns all business logic and services, providing a clean
/// interface for the UI layer to interact with.
pub struct MeshCueDomain {
    // ═══════════════════════════════════════════════════════════════════════
    // Database & Storage
    // ═══════════════════════════════════════════════════════════════════════

    /// Database service for track metadata
    db_service: Arc<DatabaseService>,

    /// Collection root path
    collection_root: PathBuf,

    /// Playlist storage backend
    playlist_storage: Box<dyn PlaylistStorage>,

    // ═══════════════════════════════════════════════════════════════════════
    // Loaded Track State
    // ═══════════════════════════════════════════════════════════════════════

    /// Currently loaded track metadata (domain-level state)
    loaded_track: Option<LoadedTrackState>,

    /// Whether the loaded track has unsaved changes
    modified: bool,

    // ═══════════════════════════════════════════════════════════════════════
    // Collection Browser
    // ═══════════════════════════════════════════════════════════════════════

    /// Cached tree nodes for playlist browser
    tree_nodes: Vec<TreeNode<NodeId>>,

    // ═══════════════════════════════════════════════════════════════════════
    // Background Services
    // ═══════════════════════════════════════════════════════════════════════

    /// USB device manager
    usb_manager: UsbManager,

    /// Import progress receiver (active during batch import)
    import_progress_rx: Option<Receiver<ImportProgress>>,

    /// Import cancellation flag
    import_cancel_flag: Option<Arc<AtomicBool>>,

    /// Reanalysis progress receiver (active during reanalysis)
    reanalysis_progress_rx: Option<Receiver<ReanalysisProgress>>,

    /// Reanalysis cancellation flag
    reanalysis_cancel_flag: Option<Arc<AtomicBool>>,

    // ═══════════════════════════════════════════════════════════════════════
    // Configuration
    // ═══════════════════════════════════════════════════════════════════════

    /// Application configuration
    config: Arc<Config>,

    /// Path to configuration file
    config_path: PathBuf,
}

impl MeshCueDomain {
    // ═══════════════════════════════════════════════════════════════════════
    // Construction
    // ═══════════════════════════════════════════════════════════════════════

    /// Create a new domain layer
    ///
    /// Initializes the database service, playlist storage, and USB manager.
    pub fn new(
        collection_root: PathBuf,
        config: Arc<Config>,
        config_path: PathBuf,
    ) -> Result<Self> {
        // Initialize database service
        let db_service = DatabaseService::new(&collection_root)
            .map_err(|e| anyhow!("Failed to initialize database: {}", e))?;

        // Create playlist storage backed by database
        let storage = DatabaseStorage::new(db_service.clone())
            .map_err(|e| anyhow!("Failed to create playlist storage: {}", e))?;
        let playlist_storage: Box<dyn PlaylistStorage> = Box::new(storage);

        // Build initial tree
        let tree_nodes = build_tree_nodes(&*playlist_storage);

        // Initialize USB manager (spawns background thread)
        let usb_manager = UsbManager::spawn(Some(db_service.clone()));

        Ok(Self {
            db_service,
            collection_root,
            playlist_storage,
            loaded_track: None,
            modified: false,
            tree_nodes,
            usb_manager,
            import_progress_rx: None,
            import_cancel_flag: None,
            reanalysis_progress_rx: None,
            reanalysis_cancel_flag: None,
            config,
            config_path,
        })
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Public Accessors (read-only domain state)
    // ═══════════════════════════════════════════════════════════════════════

    /// Get the collection root path
    pub fn collection_root(&self) -> &Path {
        &self.collection_root
    }

    /// Get the configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get mutable access to configuration
    pub fn config_mut(&mut self) -> &mut Config {
        Arc::make_mut(&mut self.config)
    }

    /// Get the config path
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Get the tree nodes for playlist browser
    pub fn tree_nodes(&self) -> &[TreeNode<NodeId>] {
        &self.tree_nodes
    }

    /// Get the loaded track state (if any)
    pub fn loaded_track(&self) -> Option<&LoadedTrackState> {
        self.loaded_track.as_ref()
    }

    /// Get mutable loaded track state (if any)
    pub fn loaded_track_mut(&mut self) -> Option<&mut LoadedTrackState> {
        self.loaded_track.as_mut()
    }

    /// Check if there's a loaded track
    pub fn has_loaded_track(&self) -> bool {
        self.loaded_track.is_some()
    }

    /// Check if the loaded track has unsaved changes
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Track Queries (high-level database access)
    // ═══════════════════════════════════════════════════════════════════════

    /// Get a track by its file path
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<Track>> {
        self.db_service.get_track_by_path(path)
            .map_err(|e| anyhow!("Failed to get track: {}", e))
    }

    /// Get a track by its database ID
    pub fn get_track_by_id(&self, id: i64) -> Result<Option<Track>> {
        self.db_service.get_track(id)
            .map_err(|e| anyhow!("Failed to get track: {}", e))
    }

    /// Get all tracks in a folder
    pub fn get_tracks_in_folder(&self, folder_path: &str) -> Result<Vec<Track>> {
        self.db_service.get_tracks_in_folder(folder_path)
            .map_err(|e| anyhow!("Failed to get tracks: {}", e))
    }

    /// Get tracks for a specific playlist
    pub fn get_playlist_tracks(&self, playlist_id: i64) -> Result<Vec<Track>> {
        self.db_service.get_playlist_tracks(playlist_id)
            .map_err(|e| anyhow!("Failed to get playlist tracks: {}", e))
    }

    /// Search tracks by query string
    pub fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<Track>> {
        self.db_service.search_tracks(query, limit)
            .map_err(|e| anyhow!("Failed to search tracks: {}", e))
    }

    /// Get total track count
    pub fn track_count(&self) -> Result<usize> {
        self.db_service.track_count()
            .map_err(|e| anyhow!("Failed to get track count: {}", e))
    }

    /// Delete a track from database
    pub fn delete_track(&self, track_id: i64) -> Result<()> {
        self.db_service.delete_track(track_id)
            .map_err(|e| anyhow!("Failed to delete track: {}", e))
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Playlist Queries (high-level database access)
    // ═══════════════════════════════════════════════════════════════════════

    /// Get a playlist by name
    pub fn get_playlist_by_name(&self, name: &str, parent_id: Option<i64>) -> Result<Option<Playlist>> {
        self.db_service.get_playlist_by_name(name, parent_id)
            .map_err(|e| anyhow!("Failed to get playlist: {}", e))
    }

    /// Get root-level playlists (no parent)
    pub fn get_root_playlists(&self) -> Result<Vec<Playlist>> {
        self.db_service.get_root_playlists()
            .map_err(|e| anyhow!("Failed to get playlists: {}", e))
    }

    /// Get child playlists of a parent
    pub fn get_child_playlists(&self, parent_id: i64) -> Result<Vec<Playlist>> {
        self.db_service.get_child_playlists(parent_id)
            .map_err(|e| anyhow!("Failed to get child playlists: {}", e))
    }

    /// Get next sort order for a playlist
    pub fn next_playlist_sort_order(&self, playlist_id: i64) -> Result<i32> {
        self.db_service.next_playlist_sort_order(playlist_id)
            .map_err(|e| anyhow!("Failed to get sort order: {}", e))
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Audio System
    // ═══════════════════════════════════════════════════════════════════════

    /// Initialize audio preview system (JACK)
    ///
    /// Returns AudioState and JackHandle for UI to store.
    /// Domain owns the db_service internally.
    pub fn init_audio_preview(&self) -> Result<(AudioState, JackHandle), JackError> {
        start_jack_client(self.db_service.clone())
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Track Operations
    // ═══════════════════════════════════════════════════════════════════════

    /// Load track metadata from database
    ///
    /// This loads the track's metadata (BPM, key, cue points, etc.) but NOT
    /// the audio data. Audio loading is handled separately by the UI layer.
    pub fn load_track_metadata(&mut self, path: &Path) -> Result<LoadedTrackState> {
        let path_str = path.to_string_lossy();

        let track = self.db_service.get_track_by_path(&path_str)?
            .ok_or_else(|| anyhow!("Track not found in database: {}", path_str))?;

        let state = LoadedTrackState::from_db_track(track);
        self.loaded_track = Some(state.clone());
        self.modified = false;

        log::info!("Domain: Loaded track metadata for {:?}", path);
        Ok(state)
    }

    /// Save the current track's metadata to database
    ///
    /// This saves all modifications (BPM, key, cue points, loops, stem links)
    /// back to the database.
    pub fn save_current_track(&mut self) -> Result<()> {
        let state = self.loaded_track.as_ref()
            .ok_or_else(|| anyhow!("No track loaded"))?;

        let path_str = state.path.to_string_lossy();

        // Load the existing track to update it
        let mut track = self.db_service.get_track_by_path(&path_str)?
            .ok_or_else(|| anyhow!("Track not found in database: {}", path_str))?;

        // Update from state
        track.bpm = Some(state.bpm);
        track.original_bpm = Some(state.original_bpm);
        track.key = Some(state.key.clone());
        track.drop_marker = state.drop_marker.map(|s| s as i64);
        track.first_beat_sample = state.first_beat_sample as i64;

        // Convert cue points (only non-empty ones)
        track.cue_points = state.cue_points.iter()
            .filter(|c| c.sample_position > 0)
            .map(|cue| {
                DbCuePoint {
                    track_id: track.id.unwrap_or(0),
                    index: cue.index,
                    sample_position: cue.sample_position as i64,
                    label: if cue.label.is_empty() { None } else { Some(cue.label.clone()) },
                    color: cue.color.clone(),
                }
            }).collect();

        // Convert saved loops (only non-empty ones)
        track.saved_loops = state.saved_loops.iter()
            .filter(|l| l.start_sample > 0 || l.end_sample > 0)
            .map(|loop_| {
                DbSavedLoop {
                    track_id: track.id.unwrap_or(0),
                    index: loop_.index,
                    start_sample: loop_.start_sample as i64,
                    end_sample: loop_.end_sample as i64,
                    label: if loop_.label.is_empty() { None } else { Some(loop_.label.clone()) },
                    color: loop_.color.clone(),
                }
            }).collect();

        // Convert stem links
        track.stem_links = state.stem_links.iter().map(|link| {
            // Look up source track by path to get ID
            let source_track_id = self.db_service
                .get_track_by_path(&link.source_path.to_string_lossy())
                .ok()
                .flatten()
                .and_then(|t| t.id)
                .unwrap_or(0);

            DbStemLink {
                track_id: track.id.unwrap_or(0),
                stem_index: link.stem_index,
                source_track_id,
                source_stem: link.source_stem,
            }
        }).collect();

        // Save to database
        self.db_service.save_track(&track)?;
        self.modified = false;

        log::info!("Domain: Saved track metadata for {:?}", state.path);
        Ok(())
    }

    /// Mark the loaded track as modified
    pub fn mark_modified(&mut self) {
        self.modified = true;
    }

    /// Clear the loaded track
    pub fn clear_loaded_track(&mut self) {
        self.loaded_track = None;
        self.modified = false;
    }

    /// Save track metadata from UI editor state
    ///
    /// Called when UI has modified track metadata (BPM, key, cue points, etc.)
    /// This is used for auto-save when switching tracks.
    pub fn save_track_metadata(
        &self,
        path: &Path,
        bpm: f64,
        key: &str,
        drop_marker: Option<u64>,
        first_beat_sample: u64,
        cue_points: &[CuePoint],
        saved_loops: &[SavedLoop],
    ) -> Result<()> {
        let path_str = path.to_string_lossy();

        // Load existing track from database to preserve fields we don't modify
        let mut track = self.db_service.get_track_by_path(&path_str)?
            .ok_or_else(|| anyhow!("Track not found in database: {}", path_str))?;

        // Update track fields
        track.bpm = Some(bpm);
        track.key = Some(key.to_string());
        track.drop_marker = drop_marker.map(|d| d as i64);
        track.first_beat_sample = first_beat_sample as i64;

        // Convert cue points to database format
        let track_id = track.id.unwrap_or(0);
        track.cue_points = cue_points.iter().map(|c| DbCuePoint {
            track_id,
            index: c.index,
            sample_position: c.sample_position as i64,
            label: if c.label.is_empty() { None } else { Some(c.label.clone()) },
            color: c.color.clone(),
        }).collect();

        // Convert saved loops to database format
        track.saved_loops = saved_loops.iter().map(|l| DbSavedLoop {
            track_id,
            index: l.index,
            start_sample: l.start_sample as i64,
            end_sample: l.end_sample as i64,
            label: if l.label.is_empty() { None } else { Some(l.label.clone()) },
            color: l.color.clone(),
        }).collect();

        // Save everything at once
        self.db_service.save_track(&track)?;

        log::info!("Domain: Saved track metadata for {:?}", path);
        Ok(())
    }

    /// Update a single field on a track (for inline table editing)
    ///
    /// Field names: "bpm", "key", "title", "artist"
    pub fn update_track_field(&self, track_path: &str, field: &str, value: &str) -> Result<()> {
        // Look up track to get ID
        let track = self.db_service.get_track_by_path(track_path)?
            .ok_or_else(|| anyhow!("Track not found in database: {}", track_path))?;

        let track_id = track.id.unwrap_or(0);
        self.db_service.update_track_field(track_id, field, value)
            .map_err(|e| anyhow!("Failed to update track field: {}", e))
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Track Metadata Updates
    // ═══════════════════════════════════════════════════════════════════════

    /// Set BPM on the loaded track
    pub fn set_bpm(&mut self, bpm: f64) {
        if let Some(ref mut state) = self.loaded_track {
            state.bpm = bpm;
            // Regenerate beat grid
            state.regenerate_beat_grid();
            self.modified = true;
        }
    }

    /// Set key on the loaded track
    pub fn set_key(&mut self, key: String) {
        if let Some(ref mut state) = self.loaded_track {
            state.key = key;
            self.modified = true;
        }
    }

    /// Set drop marker on the loaded track
    pub fn set_drop_marker(&mut self, sample: Option<u64>) {
        if let Some(ref mut state) = self.loaded_track {
            state.drop_marker = sample;
            self.modified = true;
        }
    }

    /// Set a cue point on the loaded track
    pub fn set_cue_point(&mut self, index: u8, sample: u64, label: String, color: Option<String>) {
        if let Some(ref mut state) = self.loaded_track {
            // Ensure we have enough slots
            while state.cue_points.len() <= index as usize {
                state.cue_points.push(CuePoint {
                    index: state.cue_points.len() as u8,
                    sample_position: 0,
                    label: String::new(),
                    color: None,
                });
            }
            state.cue_points[index as usize] = CuePoint {
                index,
                sample_position: sample,
                label,
                color,
            };
            self.modified = true;
        }
    }

    /// Delete a cue point from the loaded track
    pub fn delete_cue_point(&mut self, index: u8) {
        if let Some(ref mut state) = self.loaded_track {
            if (index as usize) < state.cue_points.len() {
                // Reset to empty rather than removing to preserve indices
                state.cue_points[index as usize] = CuePoint {
                    index,
                    sample_position: 0,
                    label: String::new(),
                    color: None,
                };
                self.modified = true;
            }
        }
    }

    /// Set a saved loop on the loaded track
    pub fn set_saved_loop(&mut self, index: u8, start: u64, end: u64, label: String, color: Option<String>) {
        if let Some(ref mut state) = self.loaded_track {
            while state.saved_loops.len() <= index as usize {
                state.saved_loops.push(SavedLoop {
                    index: state.saved_loops.len() as u8,
                    start_sample: 0,
                    end_sample: 0,
                    label: String::new(),
                    color: None,
                });
            }
            state.saved_loops[index as usize] = SavedLoop {
                index,
                start_sample: start,
                end_sample: end,
                label,
                color,
            };
            self.modified = true;
        }
    }

    /// Delete a saved loop from the loaded track
    pub fn delete_saved_loop(&mut self, index: u8) {
        if let Some(ref mut state) = self.loaded_track {
            if (index as usize) < state.saved_loops.len() {
                state.saved_loops[index as usize] = SavedLoop {
                    index,
                    start_sample: 0,
                    end_sample: 0,
                    label: String::new(),
                    color: None,
                };
                self.modified = true;
            }
        }
    }

    /// Set a stem link on the loaded track
    pub fn set_stem_link(&mut self, stem_index: u8, source_path: PathBuf, source_stem: u8, source_drop_marker: u64) {
        if let Some(ref mut state) = self.loaded_track {
            // Remove existing link for this stem if any
            state.stem_links.retain(|l| l.stem_index != stem_index);

            // Add new link
            state.stem_links.push(StemLinkReference {
                stem_index,
                source_path,
                source_stem,
                source_drop_marker,
            });
            self.modified = true;
        }
    }

    /// Delete a stem link from the loaded track
    pub fn delete_stem_link(&mut self, stem_index: u8) {
        if let Some(ref mut state) = self.loaded_track {
            state.stem_links.retain(|l| l.stem_index != stem_index);
            self.modified = true;
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Collection Browser
    // ═══════════════════════════════════════════════════════════════════════

    /// Refresh the playlist tree from storage
    pub fn refresh_tree(&mut self) {
        self.tree_nodes = build_tree_nodes(&*self.playlist_storage);
    }

    /// Get tracks for a playlist/folder node
    ///
    /// NodeId uses path-like strings:
    /// - "tracks" or "tracks/subfolder" -> Collection folder
    /// - "playlists/My Playlist" -> Playlist
    pub fn get_tracks_for_node(&self, node_id: &NodeId) -> Result<Vec<Track>> {
        let path = node_id.as_str();

        if path.is_empty() {
            // Root node - return all tracks
            self.db_service.get_tracks_in_folder("")
                .map_err(|e| anyhow!("Failed to get all tracks: {}", e))
        } else if node_id.is_in_tracks() {
            // Collection folder - extract relative path after "tracks/"
            let folder_path = if path == "tracks" {
                ""
            } else {
                path.strip_prefix("tracks/").unwrap_or("")
            };
            self.db_service.get_tracks_in_folder(folder_path)
                .map_err(|e| anyhow!("Failed to get tracks: {}", e))
        } else if node_id.is_in_playlists() {
            // Playlist node - look up playlist by name
            // For now, we use the playlist name from the path
            let playlist_name = node_id.name();
            // Search for playlist with this name (no specific parent)
            match self.db_service.get_playlist_by_name(playlist_name, None) {
                Ok(Some(playlist)) => {
                    self.db_service.get_playlist_tracks(playlist.id)
                        .map_err(|e| anyhow!("Failed to get playlist tracks: {}", e))
                }
                Ok(None) => Ok(Vec::new()),
                Err(e) => Err(anyhow!("Failed to get playlist: {}", e)),
            }
        } else {
            // Unknown node type
            Ok(Vec::new())
        }
    }

    /// Create a new playlist
    pub fn create_playlist(&mut self, name: &str, parent_id: Option<i64>) -> Result<i64> {
        let id = self.db_service.create_playlist(name, parent_id)
            .map_err(|e| anyhow!("Failed to create playlist: {}", e))?;
        self.refresh_tree();
        Ok(id)
    }

    /// Delete a playlist
    pub fn delete_playlist(&mut self, id: i64) -> Result<()> {
        self.db_service.delete_playlist(id)
            .map_err(|e| anyhow!("Failed to delete playlist: {}", e))?;
        self.refresh_tree();
        Ok(())
    }

    /// Rename a playlist
    pub fn rename_playlist(&mut self, id: i64, new_name: &str) -> Result<()> {
        self.db_service.rename_playlist(id, new_name)
            .map_err(|e| anyhow!("Failed to rename playlist: {}", e))?;
        self.refresh_tree();
        Ok(())
    }

    /// Add a track to a playlist
    pub fn add_track_to_playlist(&mut self, playlist_id: i64, track_id: i64) -> Result<()> {
        let sort_order = self.db_service.next_playlist_sort_order(playlist_id)
            .map_err(|e| anyhow!("Failed to get sort order: {}", e))?;
        self.db_service.add_track_to_playlist(playlist_id, track_id, sort_order)
            .map_err(|e| anyhow!("Failed to add track to playlist: {}", e))?;
        Ok(())
    }

    /// Remove a track from a playlist
    pub fn remove_track_from_playlist(&mut self, playlist_id: i64, track_id: i64) -> Result<()> {
        self.db_service.remove_track_from_playlist(playlist_id, track_id)
            .map_err(|e| anyhow!("Failed to remove track from playlist: {}", e))?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Playlist Storage Operations (NodeId-based)
    // ═══════════════════════════════════════════════════════════════════════
    // These methods provide NodeId-based access to playlist operations,
    // delegating to the PlaylistStorage trait for seamless tree navigation.

    /// Get a node by its ID
    pub fn get_node(&self, id: &NodeId) -> Option<PlaylistNode> {
        self.playlist_storage.get_node(id)
    }

    /// Get children of a node
    pub fn get_children(&self, id: &NodeId) -> Vec<PlaylistNode> {
        self.playlist_storage.get_children(id)
    }

    /// Get tracks for display in a folder/playlist
    ///
    /// This returns TrackRow items ready for display in the track table.
    pub fn get_tracks_for_display(&self, folder_id: &NodeId) -> Vec<TrackRow<NodeId>> {
        crate::ui::utils::get_tracks_for_folder(&*self.playlist_storage, folder_id)
    }

    /// Create a new playlist with NodeId-based parent
    ///
    /// Returns the NodeId of the newly created playlist.
    /// Parent should be `NodeId::playlists()` for root playlists, or an existing playlist's NodeId.
    pub fn create_playlist_with_node(&mut self, parent_id: &NodeId, name: &str) -> Result<NodeId> {
        let new_id = self.playlist_storage.create_playlist(parent_id, name)
            .map_err(|e| anyhow!("Failed to create playlist: {}", e))?;
        self.refresh_tree();
        Ok(new_id)
    }

    /// Rename a playlist by NodeId
    pub fn rename_playlist_by_node(&mut self, id: &NodeId, new_name: &str) -> Result<()> {
        self.playlist_storage.rename_playlist(id, new_name)
            .map_err(|e| anyhow!("Failed to rename playlist: {}", e))?;
        self.refresh_tree();
        Ok(())
    }

    /// Delete a playlist by NodeId
    pub fn delete_playlist_by_node(&mut self, id: &NodeId) -> Result<()> {
        self.playlist_storage.delete_playlist(id)
            .map_err(|e| anyhow!("Failed to delete playlist: {}", e))?;
        self.refresh_tree();
        Ok(())
    }

    /// Remove a track from its playlist (track NodeId)
    ///
    /// The track's NodeId encodes which playlist it belongs to.
    pub fn remove_track_from_playlist_by_node(&mut self, track_id: &NodeId) -> Result<()> {
        self.playlist_storage.remove_track_from_playlist(track_id)
            .map_err(|e| anyhow!("Failed to remove track from playlist: {}", e))?;
        Ok(())
    }

    /// Delete a track permanently (from database and filesystem)
    pub fn delete_track_permanently_by_node(&mut self, track_id: &NodeId) -> Result<()> {
        self.playlist_storage.delete_track_permanently(track_id)
            .map_err(|e| anyhow!("Failed to delete track permanently: {}", e))?;
        self.refresh_tree();
        Ok(())
    }

    /// Add tracks to a playlist
    ///
    /// This copies tracks from collection to a playlist.
    pub fn add_tracks_to_playlist(&mut self, target_playlist_id: &NodeId, track_ids: &[NodeId]) -> Result<usize> {
        let mut success_count = 0;
        for track_id in track_ids {
            // Check if source is collection (tracks/...) or playlist (playlists/...)
            let is_from_collection = track_id.as_str().starts_with("tracks/");

            if is_from_collection {
                // Get the track path from the node
                if let Some(node) = self.playlist_storage.get_node(track_id) {
                    if let Some(ref path) = node.track_path {
                        // Look up target playlist ID from NodeId
                        let playlist_name = target_playlist_id.name();
                        let path_str = path.to_string_lossy();
                        if let Ok(Some(playlist)) = self.db_service.get_playlist_by_name(playlist_name, None) {
                            // Look up track ID from path
                            if let Ok(Some(track)) = self.db_service.get_track_by_path(&path_str) {
                                if let Some(track_db_id) = track.id {
                                    let sort_order = self.db_service.next_playlist_sort_order(playlist.id)
                                        .unwrap_or(0);
                                    if self.db_service.add_track_to_playlist(playlist.id, track_db_id, sort_order).is_ok() {
                                        success_count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(success_count)
    }

    /// Get all playlist paths (for export sync)
    pub fn get_all_playlist_paths(&self) -> Vec<(NodeId, String)> {
        let mut results = Vec::new();
        self.collect_playlists_recursive(&NodeId::playlists(), &mut results);
        results
    }

    /// Helper to collect playlists recursively
    fn collect_playlists_recursive(&self, parent: &NodeId, results: &mut Vec<(NodeId, String)>) {
        for node in self.playlist_storage.get_children(parent) {
            if node.kind == NodeKind::Playlist {
                results.push((node.id.clone(), node.name.clone()));
                // Recurse into nested playlists
                self.collect_playlists_recursive(&node.id, results);
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Batch Import
    // ═══════════════════════════════════════════════════════════════════════

    /// Scan a folder for stem groups to import
    pub fn scan_import_folder(&self, folder: &Path) -> Result<Vec<StemGroup>> {
        crate::batch_import::scan_and_group_stems(folder)
            .map_err(|e| anyhow!("Failed to scan import folder: {}", e))
    }

    /// Start batch import in background thread
    pub fn start_batch_import(
        &mut self,
        groups: Vec<StemGroup>,
        import_folder: PathBuf,
    ) -> Result<()> {
        use crate::batch_import::ImportConfig;

        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Build ImportConfig from domain state
        let import_config = ImportConfig {
            import_folder,
            collection_path: self.collection_root.clone(),
            db_service: self.db_service.clone(),
            bpm_config: self.config.analysis.bpm.clone(),
            loudness_config: self.config.analysis.loudness.clone(),
            parallel_processes: self.config.analysis.parallel_processes,
        };

        let cancel = cancel_flag.clone();

        std::thread::spawn(move || {
            crate::batch_import::run_batch_import(
                groups,
                import_config,
                progress_tx,
                cancel,
            );
        });

        self.import_progress_rx = Some(progress_rx);
        self.import_cancel_flag = Some(cancel_flag);

        Ok(())
    }

    /// Cancel an in-progress import
    pub fn cancel_import(&mut self) {
        if let Some(ref flag) = self.import_cancel_flag {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Get import progress receiver (for subscription)
    pub fn import_progress_receiver(&self) -> Option<&Receiver<ImportProgress>> {
        self.import_progress_rx.as_ref()
    }

    /// Clear import state after completion
    pub fn clear_import_state(&mut self) {
        self.import_progress_rx = None;
        self.import_cancel_flag = None;
    }

    /// Check if import is in progress
    pub fn is_importing(&self) -> bool {
        self.import_progress_rx.is_some()
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Reanalysis
    // ═══════════════════════════════════════════════════════════════════════

    /// Start reanalysis in background thread
    pub fn start_reanalysis(
        &mut self,
        tracks: Vec<PathBuf>,
        analysis_type: AnalysisType,
    ) -> Result<()> {
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let bpm_config = self.config.analysis.bpm.clone();
        let loudness_config = self.config.analysis.loudness.clone();
        let parallel = self.config.analysis.parallel_processes;
        let db = self.db_service.clone();
        let cancel = cancel_flag.clone();

        std::thread::spawn(move || {
            run_batch_reanalysis(
                tracks,
                analysis_type,
                bpm_config,
                loudness_config,
                parallel,
                progress_tx,
                cancel,
                Some(db),
            );
        });

        self.reanalysis_progress_rx = Some(progress_rx);
        self.reanalysis_cancel_flag = Some(cancel_flag);

        Ok(())
    }

    /// Cancel an in-progress reanalysis
    pub fn cancel_reanalysis(&mut self) {
        if let Some(ref flag) = self.reanalysis_cancel_flag {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Get reanalysis progress receiver (for subscription)
    pub fn reanalysis_progress_receiver(&self) -> Option<&Receiver<ReanalysisProgress>> {
        self.reanalysis_progress_rx.as_ref()
    }

    /// Clear reanalysis state after completion
    pub fn clear_reanalysis_state(&mut self) {
        self.reanalysis_progress_rx = None;
        self.reanalysis_cancel_flag = None;
    }

    /// Check if reanalysis is in progress
    pub fn is_reanalyzing(&self) -> bool {
        self.reanalysis_progress_rx.is_some()
    }

    // ═══════════════════════════════════════════════════════════════════════
    // USB Export (high-level operations only)
    // ═══════════════════════════════════════════════════════════════════════

    /// Refresh USB device list
    pub fn refresh_usb_devices(&self) {
        self.usb_manager.refresh_devices();
    }

    /// Get USB message receiver for subscriptions
    ///
    /// Use this with `mpsc_subscription` to receive USB events in the UI.
    pub fn usb_message_receiver(&self) -> Arc<Mutex<Receiver<UsbMessage>>> {
        self.usb_manager.message_receiver()
    }

    /// Mount a USB device by path
    pub fn mount_usb_device(&self, device_path: PathBuf) {
        self.usb_manager.mount(device_path);
    }

    /// Unmount a USB device by path
    pub fn unmount_usb_device(&self, device_path: PathBuf) {
        self.usb_manager.unmount(device_path);
    }

    /// Build a sync plan for USB export
    ///
    /// Asynchronously builds a plan comparing local playlists with USB content.
    /// Result is delivered via `UsbMessage::SyncPlanReady`.
    pub fn build_usb_sync_plan(&self, device_path: PathBuf, playlists: Vec<NodeId>) {
        let _ = self.usb_manager.send(UsbCommand::BuildSyncPlan {
            device_path,
            playlists,
            local_collection_root: self.collection_root.clone(),
        });
    }

    /// Start USB export with a sync plan
    ///
    /// Progress updates delivered via `UsbMessage::ExportProgress`.
    /// Completion via `UsbMessage::ExportComplete`.
    pub fn start_usb_export(&self, device_path: PathBuf, plan: SyncPlan, include_config: bool, export_config: Option<ExportableConfig>) {
        let _ = self.usb_manager.send(UsbCommand::StartExport {
            device_path,
            plan,
            include_config,
            config: export_config,
        });
    }

    /// Cancel an in-progress USB export
    pub fn cancel_usb_export(&self) {
        let _ = self.usb_manager.send(UsbCommand::CancelExport);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Configuration
    // ═══════════════════════════════════════════════════════════════════════

    /// Save configuration to file
    pub fn save_config(&self) -> Result<()> {
        crate::config::save_config(&*self.config, &self.config_path)
            .map_err(|e| anyhow!("Failed to save config: {}", e))
    }

    /// Update configuration with a closure
    pub fn update_config<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut Config),
    {
        f(Arc::make_mut(&mut self.config));
        self.save_config()
    }
}

impl std::fmt::Debug for MeshCueDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshCueDomain")
            .field("collection_root", &self.collection_root)
            .field("has_loaded_track", &self.loaded_track.is_some())
            .field("modified", &self.modified)
            .field("tree_nodes_count", &self.tree_nodes.len())
            .field("is_importing", &self.is_importing())
            .field("is_reanalyzing", &self.is_reanalyzing())
            .finish_non_exhaustive()
    }
}
