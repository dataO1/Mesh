//! Collection browser state
//!
//! UI-only state for the collection browser panels.
//! Database service and playlist storage are owned by the domain layer.

use std::path::PathBuf;
use mesh_core::playlist::NodeId;
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
///
/// This is pure UI state - browser panels, selections, and cached display data.
/// All business logic and service access goes through the domain layer.
pub struct CollectionState {
    /// Path to the collection root folder
    pub collection_path: PathBuf,
    /// Currently selected track index (legacy)
    pub selected_track: Option<usize>,
    /// Currently loaded track for editing (UI view state)
    pub loaded_track: Option<LoadedTrackState>,
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
            .field("collection_path", &self.collection_path)
            .field("selected_track", &self.selected_track)
            .field("loaded_track", &self.loaded_track)
            .field("tree_nodes_count", &self.tree_nodes.len())
            .field("left_tracks_count", &self.left_tracks.len())
            .field("right_tracks_count", &self.right_tracks.len())
            .finish_non_exhaustive()
    }
}

impl CollectionState {
    /// Get mutable reference to browser state for the given side
    pub fn browser_mut(&mut self, side: BrowserSide) -> &mut PlaylistBrowserState<NodeId, NodeId> {
        match side {
            BrowserSide::Left => &mut self.browser_left,
            BrowserSide::Right => &mut self.browser_right,
        }
    }

    /// Get reference to browser state for the given side
    pub fn browser(&self, side: BrowserSide) -> &PlaylistBrowserState<NodeId, NodeId> {
        match side {
            BrowserSide::Left => &self.browser_left,
            BrowserSide::Right => &self.browser_right,
        }
    }

    /// Get mutable reference to tracks for the given side
    pub fn tracks_mut(&mut self, side: BrowserSide) -> &mut Vec<TrackRow<NodeId>> {
        match side {
            BrowserSide::Left => &mut self.left_tracks,
            BrowserSide::Right => &mut self.right_tracks,
        }
    }

    /// Get reference to tracks for the given side
    pub fn tracks(&self, side: BrowserSide) -> &Vec<TrackRow<NodeId>> {
        match side {
            BrowserSide::Left => &self.left_tracks,
            BrowserSide::Right => &self.right_tracks,
        }
    }

    /// Get the side name for logging
    pub fn side_name(side: BrowserSide) -> &'static str {
        match side {
            BrowserSide::Left => "Left",
            BrowserSide::Right => "Right",
        }
    }
}

impl Default for CollectionState {
    fn default() -> Self {
        // Default to ~/Music/mesh-collection (cross-platform via dirs crate)
        let default_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection");

        Self {
            collection_path: default_path,
            selected_track: None,
            loaded_track: None,
            browser_left: PlaylistBrowserState::new(),
            browser_right: PlaylistBrowserState::new(),
            tree_nodes: Vec::new(),
            left_tracks: Vec::new(),
            right_tracks: Vec::new(),
            dragging_track: None,
        }
    }
}
