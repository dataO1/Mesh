//! Application state modules
//!
//! Extracted from app.rs for better organization and maintainability.
//! Each submodule contains a cohesive set of state structures.

pub mod collection;
pub mod import;
pub mod loaded_track;
pub mod reanalysis;
pub mod settings;

// Re-export all types for convenient access
pub use collection::{BrowserSide, CollectionState, DragState};
pub use import::{ImportPhase, ImportState};
pub use loaded_track::{LinkedStemLoadedMsg, LoadedTrackState, StemsLoadResult};
pub use reanalysis::ReanalysisState;
pub use settings::SettingsState;

/// Current view in the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Collection browser and track editor (with batch import)
    #[default]
    Collection,
}
