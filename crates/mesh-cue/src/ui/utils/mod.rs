//! Utility functions for mesh-cue UI
//!
//! Helper functions extracted from app.rs for better organization.

pub mod beat_grid;
pub mod tree;

// Re-export commonly used items
pub use beat_grid::{
    nudge_beat_grid, regenerate_beat_grid, snap_to_nearest_beat, update_waveform_beat_grid,
    BEAT_GRID_NUDGE_SAMPLES,
};
pub use tree::{build_tree_nodes, get_tracks_for_folder, tracks_to_rows};
