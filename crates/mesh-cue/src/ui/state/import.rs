//! Batch import state

use crate::batch_import::{self, ImportProgress, MixedAudioFile, StemGroup, TrackImportResult};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Import mode - determines what type of files to import
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    /// Import pre-separated stem files (Artist - Track_(Vocals).wav, etc.)
    #[default]
    Stems,
    /// Import mixed audio files and auto-separate into stems
    MixedAudio,
}

/// Phase of the batch import process
#[derive(Debug, Clone)]
pub enum ImportPhase {
    /// Scanning import folder for stems
    Scanning,
    /// Processing tracks in parallel
    Processing {
        /// Currently processing track name
        current_track: String,
        /// Number of completed tracks
        completed: usize,
        /// Total tracks to process
        total: usize,
        /// Time import started (for ETA calculation)
        start_time: std::time::Instant,
    },
    /// Import complete
    Complete {
        /// How long the import took
        duration: std::time::Duration,
    },
}

/// State for the batch import modal and progress
#[derive(Debug)]
pub struct ImportState {
    /// Whether the import modal is open
    pub is_open: bool,
    /// Path to the import folder
    pub import_folder: std::path::PathBuf,
    /// Current import mode (stems vs mixed audio)
    pub import_mode: ImportMode,
    /// Detected stem groups from scan (for Stems mode)
    pub detected_groups: Vec<StemGroup>,
    /// Detected mixed audio files from scan (for MixedAudio mode)
    pub detected_mixed_files: Vec<MixedAudioFile>,
    /// Current import phase (None if not importing)
    pub phase: Option<ImportPhase>,
    /// Results from completed import (for final popup)
    pub results: Vec<TrackImportResult>,
    /// Show results popup after completion
    pub show_results: bool,
    /// Channel to receive progress updates from import thread
    pub progress_rx: Option<Receiver<ImportProgress>>,
    /// Atomic flag to signal cancellation to import thread
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

impl Default for ImportState {
    fn default() -> Self {
        Self {
            is_open: false,
            import_folder: batch_import::default_import_folder(),
            import_mode: ImportMode::default(),
            detected_groups: Vec::new(),
            detected_mixed_files: Vec::new(),
            phase: None,
            results: Vec::new(),
            show_results: false,
            progress_rx: None,
            cancel_flag: None,
        }
    }
}
