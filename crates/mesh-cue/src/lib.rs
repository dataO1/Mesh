//! Mesh Cue Software - Track preparation for mesh DJ software
//!
//! This application provides a two-stage workflow for preparing tracks:
//!
//! 1. **Staging Area**: Import pre-separated stems (Vocals, Drums, Bass, Other),
//!    run audio analysis (BPM, key, beat grid), and export to 8-channel WAV format.
//!
//! 2. **Collection Editor**: Browse converted tracks, edit cue points, adjust beat grid,
//!    and save changes back to the files.

pub mod analysis;
pub mod audio;
pub mod collection;
pub mod export;
pub mod import;
pub mod ui;
