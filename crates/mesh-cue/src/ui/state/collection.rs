//! Collection browser state
//!
//! UI-only state for the collection browser panels.
//! Database service and playlist storage are owned by the domain layer.

use std::path::PathBuf;
use std::sync::Arc;
use iced::{Color, Point};
use mesh_core::playlist::NodeId;
use mesh_widgets::{
    EnergyArcState, GraphViewState, PlaylistBrowserState, TrackColumn, TrackRow, TreeNode,
};
use mesh_widgets::track_table::sort_tracks;

use super::loaded_track::LoadedTrackState;

/// Minimum distance (in pixels) mouse must move before drag starts
pub const DRAG_THRESHOLD: f32 = 8.0;

/// Which browser panel a drag operation originated from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserSide {
    Left,
    Right,
}

/// State for a pending drag (click happened, waiting for mouse movement)
#[derive(Debug, Clone)]
pub struct PendingDragState {
    /// Mouse position when click occurred
    pub start_position: Point,
    /// The tracks that would be dragged
    pub track_ids: Vec<NodeId>,
    /// Display names of the tracks
    pub track_names: Vec<String>,
    /// Which browser the click happened in
    pub source_browser: BrowserSide,
    /// Whether Shift or Ctrl was held when the click occurred.
    /// Used to prevent collapsing a modifier-built selection on release.
    pub had_modifiers: bool,
}

impl PendingDragState {
    /// Check if mouse has moved past the drag threshold
    pub fn should_start_drag(&self, current_position: Point) -> bool {
        let dx = current_position.x - self.start_position.x;
        let dy = current_position.y - self.start_position.y;
        let distance = (dx * dx + dy * dy).sqrt();
        distance >= DRAG_THRESHOLD
    }

    /// Convert to active drag state
    pub fn into_drag_state(self) -> DragState {
        DragState {
            track_ids: self.track_ids,
            track_names: self.track_names,
            source_browser: self.source_browser,
        }
    }
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
    /// Single track: shows track name
    /// Multiple tracks: shows "first track name..."
    pub fn display_text(&self) -> String {
        match self.track_names.len() {
            0 => String::new(),
            1 => self.track_names[0].clone(),
            _ => format!("{}...", self.track_names[0]),
        }
    }
}

/// Browser tab selection (List = dual playlist browsers, Graph = suggestion graph)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserTab {
    #[default]
    List,
    Graph,
}

/// Re-export GraphEdge from mesh-core for the suggestion graph.
pub use mesh_core::suggestions::query::GraphEdge;

/// State for the collection view
///
/// This is pure UI state - browser panels, selections, and cached display data.
/// All business logic and service access goes through the domain layer.
pub struct CollectionState {
    /// Path to the collection root folder
    pub collection_path: PathBuf,
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
    /// Pending drag state (click happened, waiting for threshold movement)
    pub pending_drag: Option<PendingDragState>,
    /// Which browser the mouse is currently hovering over (during drag)
    pub drag_hover_browser: Option<BrowserSide>,
    /// Pending cell edit (deferred until mouse release to avoid double-click race)
    /// Set by CellClicked on an already-selected row, cleared by Activate (double-click).
    /// Executed on DropReceived (mouse release) if still present.
    pub pending_cell_edit: Option<(NodeId, TrackColumn)>,
    /// Active theme stem colors (for waveform rendering)
    pub stem_colors: [Color; 4],
    /// Active browser tab
    pub active_tab: BrowserTab,
    /// Graph view state (None until first Graph tab open)
    pub graph_state: Option<GraphViewState>,
    /// Precomputed graph edges (None until built)
    pub graph_edges: Option<Arc<Vec<GraphEdge>>>,
    /// True while graph edges are being built in background
    pub graph_building: bool,
    /// Whether to L2-normalize PCA vectors (persists across graph rebuilds)
    pub graph_normalize_vectors: bool,
    /// Suggestion tracks for graph left panel (populated on seed select)
    pub graph_suggestion_rows: Vec<TrackRow<NodeId>>,
    /// Table state for the graph suggestion list
    pub graph_table_state: mesh_widgets::TrackTableState<NodeId>,
    /// PCA cosine distances between consecutive left browser tracks (for energy arc)
    pub consecutive_similarities: Vec<f32>,
    /// Cached energy arc state for the left browser (rebuilt when tracks change)
    pub energy_arc: Option<EnergyArcState>,
}

impl std::fmt::Debug for CollectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollectionState")
            .field("collection_path", &self.collection_path)
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

    /// Replace track list for a browser side and re-apply the current sort order.
    ///
    /// This must be used instead of direct assignment whenever tracks are refreshed
    /// from the domain (e.g. after delete, edit, or playlist drop) so that index-based
    /// selection and scroll restoration operate on correctly sorted data.
    pub fn refresh_tracks(&mut self, side: BrowserSide, tracks: Vec<TrackRow<NodeId>>) {
        *self.tracks_mut(side) = tracks;
        let state = &self.browser(side).table_state;
        let sort_col = state.sort_column;
        let sort_asc = state.sort_ascending;
        sort_tracks(self.tracks_mut(side), sort_col, sort_asc);
        // Rebuild the energy arc when the left browser tracks change
        if matches!(side, BrowserSide::Left) {
            self.rebuild_energy_arc();
        }
    }

    /// Rebuild the cached energy arc from the current left browser tracks
    /// and selected track position.
    pub fn rebuild_energy_arc(&mut self) {
        use crate::ui::collection_browser::build_energy_arc;
        let current_idx = self
            .browser_left
            .table_state
            .selected_ids()
            .iter()
            .next()
            .and_then(|sel_id| self.left_tracks.iter().position(|t| &t.id == sel_id))
            .unwrap_or(0);
        self.energy_arc = build_energy_arc(&self.left_tracks, current_idx, &self.consecutive_similarities, self.stem_colors);
    }
}

impl Default for CollectionState {
    fn default() -> Self {
        // Default to ~/Music/mesh-collection (cross-platform via dirs crate)
        let default_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Music")
            .join("mesh-collection");

        let editable_cols = &[TrackColumn::Name, TrackColumn::Artist, TrackColumn::Bpm, TrackColumn::Key];
        let mut browser_left = PlaylistBrowserState::new();
        browser_left.table_state.set_editable_columns(editable_cols);
        let mut browser_right = PlaylistBrowserState::new();
        browser_right.table_state.set_editable_columns(editable_cols);

        Self {
            collection_path: default_path,
            loaded_track: None,
            browser_left,
            browser_right,
            tree_nodes: Vec::new(),
            left_tracks: Vec::new(),
            right_tracks: Vec::new(),
            dragging_track: None,
            pending_drag: None,
            drag_hover_browser: None,
            pending_cell_edit: None,
            stem_colors: mesh_widgets::STEM_COLORS,
            active_tab: BrowserTab::List,
            graph_state: None,
            graph_edges: None,
            graph_building: false,
            graph_normalize_vectors: false,
            graph_suggestion_rows: Vec::new(),
            graph_table_state: {
                let mut ts = mesh_widgets::TrackTableState::new();
                ts.display_columns = Some(mesh_widgets::TrackColumn::graph_analysis());
                ts
            },
            consecutive_similarities: Vec::new(),
            energy_arc: None,
        }
    }
}
