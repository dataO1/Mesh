//! Batch import state

use crate::batch_import::{self, ImportProgress, StemGroup, TrackImportResult};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

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
    /// Detected stem groups from scan
    pub detected_groups: Vec<StemGroup>,
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
            detected_groups: Vec::new(),
            phase: None,
            results: Vec::new(),
            show_results: false,
            progress_rx: None,
            cancel_flag: None,
        }
    }
}
