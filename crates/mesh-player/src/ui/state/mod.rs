//! Application state modules for mesh-player
//!
//! Extracted from app.rs for better organization and maintainability.

use std::sync::{Arc, Mutex};

use crate::loader::TrackLoadResult;
use mesh_core::preset_loader::PresetLoadResult;

/// UI display mode - affects layout only, not engine behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Simplified layout: waveform canvas + browser only (for live performance)
    #[default]
    Performance,
    /// Full layout with deck controls and mixer (for MIDI mapping/configuration)
    Mapping,
}

/// State machine for linked stem selection workflow
///
/// Workflow:
/// 1. Shift+Stem → Enter Selecting (browser highlights with stem color)
/// 2. Encoder rotate → Navigate browser
/// 3. Encoder press → Load linked stem in background
/// 4. Load completes → Ready for toggle
/// 5. Shift+Stem again → Toggle between original/linked
#[derive(Debug, Clone, Default)]
pub enum StemLinkState {
    /// No linked stem operation in progress
    #[default]
    Idle,
    /// Shift+stem pressed, waiting for track selection from browser
    Selecting {
        /// Host deck that will receive the linked stem
        deck: usize,
        /// Which stem slot to link (0-3)
        stem: usize,
    },
    /// Track selected, loading linked stem in background
    Loading {
        /// Host deck that will receive the linked stem
        deck: usize,
        /// Which stem slot to link
        stem: usize,
        /// Path to the source track being loaded
        path: std::path::PathBuf,
    },
}

/// Wrapper for TrackLoadResult enabling use in Message enum
/// Uses Arc for cheap cloning, manual Debug impl for simplicity
#[derive(Clone)]
pub struct TrackLoadedMsg(pub Arc<TrackLoadResult>);

impl std::fmt::Debug for TrackLoadedMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackLoadedMsg")
            .field("deck_idx", &self.0.deck_idx)
            .finish_non_exhaustive()
    }
}

/// Wrapper for LinkedStemLoadResult enabling use in Message enum
/// Uses Arc for cheap cloning, manual Debug impl for simplicity
#[derive(Clone)]
pub struct LinkedStemLoadedMsg(pub Arc<mesh_core::loader::LinkedStemLoadResult>);

impl std::fmt::Debug for LinkedStemLoadedMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkedStemLoadedMsg")
            .field("deck_idx", &self.0.host_deck_idx)
            .field("stem_idx", &self.0.stem_idx)
            .finish_non_exhaustive()
    }
}

/// Wrapper for PresetLoadResult enabling use in Message enum.
///
/// Uses `Arc<Mutex<Option<T>>>` instead of plain `Arc<T>` because
/// `MultibandHost` contains `Box<dyn Effect>` which is not `Sync`.
/// The Mutex provides the Sync bound that Arc requires for Send,
/// and Option allows `take()` for zero-copy extraction.
#[derive(Clone)]
pub struct PresetLoadedMsg(pub Arc<Mutex<Option<PresetLoadResult>>>);

impl std::fmt::Debug for PresetLoadedMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresetLoadedMsg").finish_non_exhaustive()
    }
}
