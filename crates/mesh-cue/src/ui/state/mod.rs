//! Application state modules
//!
//! Extracted from app.rs for better organization and maintainability.
//! Each submodule contains a cohesive set of state structures.

pub mod collection;
pub mod export;
pub mod import;
pub mod loaded_track;
pub mod reanalysis;
pub mod settings;

// Re-export all types for convenient access
pub use collection::{BrowserSide, CollectionState, DragState, PendingDragState, DRAG_THRESHOLD};
pub use export::{ExportPhase, ExportState};
pub use import::{ImportMode, ImportPhase, ImportState};
pub use loaded_track::{LinkedStemLoadedMsg, LoadedTrackState, PresetLoadedMsg, StemsLoadResult};
pub use reanalysis::ReanalysisState;
pub use settings::SettingsState;

// Re-export UsbDevice for export state
pub use mesh_core::usb::UsbDevice;

/// Current view in the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Collection browser and track editor (with batch import)
    #[default]
    Collection,
}
