//! Re-analysis state

use crate::analysis::{AnalysisType, ReanalysisProgress, ReanalysisScope};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// State for re-analysis operations
#[derive(Debug, Default)]
pub struct ReanalysisState {
    /// Whether re-analysis is in progress
    pub is_running: bool,
    /// Analysis type being performed
    pub analysis_type: Option<AnalysisType>,
    /// Total tracks to process
    pub total_tracks: usize,
    /// Tracks completed so far
    pub completed_tracks: usize,
    /// Currently processing track name
    pub current_track: Option<String>,
    /// Number of successful completions
    pub succeeded: usize,
    /// Number of failed completions
    pub failed: usize,
    /// Cancel flag (shared with worker thread)
    pub cancel_flag: Option<Arc<AtomicBool>>,
    /// Channel to receive progress updates from worker thread
    pub progress_rx: Option<Receiver<ReanalysisProgress>>,

    // Config modal state (for "Re-analyse Metadata..." modal)
    /// Whether the metadata config modal is open
    pub config_modal_open: bool,
    /// Scope for the pending metadata reanalysis
    pub config_scope: Option<ReanalysisScope>,
    /// Checkbox: re-parse name/artist from original filename
    pub config_name_artist: bool,
    /// Checkbox: LUFS loudness measurement
    pub config_loudness: bool,
    /// Checkbox: musical key detection
    pub config_key: bool,
    /// Checkbox: ML tags (genre, mood, etc.)
    pub config_tags: bool,
}
