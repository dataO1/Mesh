//! Collection browser state

use std::sync::Arc;
use crate::collection::Collection;
use mesh_core::db::MeshDb;
use mesh_core::playlist::{NodeId, PlaylistStorage};
use mesh_widgets::{PlaylistBrowserState, TrackRow, TreeNode};

use super::loaded_track::LoadedTrackState;

/// Which browser panel a drag operation originated from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSide {
    Left,
    Right,
}

/// State for an in-progress drag operation (supports multi-track drag)
#[derive(Debug, Clone)]
pub struct DragState {
    /// The tracks being dragged (supports multi-selection)
    pub track_ids: Vec<NodeId>,
    /// Display names of the tracks (for status display)
    pub track_names: Vec<String>,
    /// Which browser the drag started from
    pub source_browser: BrowserSide,
}

impl DragState {
    /// Create a new drag state for a single track
    pub fn single(track_id: NodeId, track_name: String, source_browser: BrowserSide) -> Self {
        Self {
            track_ids: vec![track_id],
            track_names: vec![track_name],
            source_browser,
        }
    }

    /// Create a new drag state for multiple tracks
    pub fn multiple(
        track_ids: Vec<NodeId>,
        track_names: Vec<String>,
        source_browser: BrowserSide,
    ) -> Self {
        Self { track_ids, track_names, source_browser }
    }

    /// Get display text for the drag operation
    pub fn display_text(&self) -> String {
        match self.track_names.len() {
            0 => String::new(),
            1 => self.track_names[0].clone(),
            n => format!("{} tracks", n),
        }
    }
}

/// State for the collection view
pub struct CollectionState {
    /// Collection manager (legacy - kept for track scanning)
    pub collection: Collection,
    /// Currently selected track index (legacy)
    pub selected_track: Option<usize>,
    /// Currently loaded track for editing
    pub loaded_track: Option<LoadedTrackState>,
    /// Playlist storage backend (FilesystemStorage or DatabaseStorage)
    pub playlist_storage: Option<Box<dyn PlaylistStorage>>,
    /// Database connection for metadata updates (shared with DatabaseStorage)
    pub db: Option<Arc<MeshDb>>,
    /// Left browser state
    pub browser_left: PlaylistBrowserState<NodeId, NodeId>,
    /// Right browser state
    pub browser_right: PlaylistBrowserState<NodeId, NodeId>,
    /// Cached tree nodes for display (rebuilt when storage changes)
    pub tree_nodes: Vec<TreeNode<NodeId>>,
    /// Cached tracks for left browser (updated when folder changes)
    pub left_tracks: Vec<TrackRow<NodeId>>,
    /// Cached tracks for right browser (updated when folder changes)
    pub right_tracks: Vec<TrackRow<NodeId>>,
    /// Track currently being dragged (if any)
    pub dragging_track: Option<DragState>,
}

impl std::fmt::Debug for CollectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionState")
            .field("collection", &self.collection)
            .field("selected_track", &self.selected_track)
            .field("loaded_track", &self.loaded_track)
            .field("has_playlist_storage", &self.playlist_storage.is_some())
            .field("has_db", &self.db.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for CollectionState {
    fn default() -> Self {
        Self {
            collection: Collection::default(),
            selected_track: None,
            loaded_track: None,
            playlist_storage: None,
            db: None,
            browser_left: PlaylistBrowserState::new(),
            browser_right: PlaylistBrowserState::new(),
            tree_nodes: Vec::new(),
            left_tracks: Vec::new(),
            right_tracks: Vec::new(),
            dragging_track: None,
        }
    }
}
