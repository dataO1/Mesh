//! Slicer Editor Modal for mesh-cue
//!
//! Provides a modal overlay for the 16x16 slice editor grid.
//! The actual slice editor data lives on `LoadedTrackState` (track-specific);
//! this module only manages modal visibility and rendering.

mod state;
mod view;

pub use state::SlicerEditorState;
pub use view::slicer_editor_view;
