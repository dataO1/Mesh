//! Message handlers organized by feature domain
//!
//! This module splits the large update() function into logical groupings.
//! Each sub-module provides handler methods on MeshCueApp.

pub mod browser;
pub mod delete;
pub mod editing;
pub mod export;
pub mod import;
pub mod keyboard;
pub mod playback;
pub mod reanalysis;
pub mod settings;
pub mod slicer;
pub mod stem_links;
pub mod tick;
pub mod track_loading;
