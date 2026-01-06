//! Mesh Cue Software - Track preparation for mesh DJ software
//!
//! This application provides tools for preparing tracks:
//!
//! 1. **Batch Import**: Import pre-separated stems from the import folder,
//!    run audio analysis (BPM, key, beat grid), and export to 8-channel WAV format.
//!
//! 2. **Collection Editor**: Browse converted tracks, edit cue points, adjust beat grid,
//!    and save changes back to the files.

pub mod analysis;
pub mod audio;
pub mod batch_import;
pub mod collection;
pub mod config;
pub mod export;
pub mod import;
pub mod keybindings;
pub mod ui;
