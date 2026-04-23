//! Aggression calibration state
//!
//! Tracks the state of the pairwise comparison calibration modal,
//! including pre-loaded audio clips, pair queue, and learned weights.

use std::collections::VecDeque;
use mesh_core::suggestions::UncoveredCommunity;

/// Which side of the comparison the user interacts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationSide {
    Left,
    Right,
}

/// Current phase of the calibration process.
#[derive(Debug, Clone)]
pub enum CalibrationPhase {
    /// Anchor comparisons: uncovered track vs well-known reference
    Anchor { current: usize, total: usize },
    /// Within-community pairs to resolve intra-genre variation
    IntraCommunity { current: usize, total: usize },
    /// Cross-community boundary pairs
    Boundary { current: usize, total: usize },
}

impl CalibrationPhase {
    pub fn label(&self) -> &'static str {
        match self {
            CalibrationPhase::Anchor { .. } => "Anchoring",
            CalibrationPhase::IntraCommunity { .. } => "Refining",
            CalibrationPhase::Boundary { .. } => "Boundaries",
        }
    }

    pub fn progress(&self) -> (usize, usize) {
        match self {
            CalibrationPhase::Anchor { current, total }
            | CalibrationPhase::IntraCommunity { current, total }
            | CalibrationPhase::Boundary { current, total } => (*current, *total),
        }
    }

    pub fn phase_number(&self) -> usize {
        match self {
            CalibrationPhase::Anchor { .. } => 1,
            CalibrationPhase::IntraCommunity { .. } => 2,
            CalibrationPhase::Boundary { .. } => 3,
        }
    }
}

/// Lightweight track metadata for display in calibration cards.
#[derive(Debug, Clone)]
pub struct CalibrationTrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub genre: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub lufs: Option<f32>,
    pub path: std::path::PathBuf,
}

/// A pre-loaded pair of tracks ready for comparison.
#[derive(Debug, Clone)]
pub struct PreloadedPair {
    pub track_a: CalibrationTrackInfo,
    pub track_b: CalibrationTrackInfo,
    /// Pre-decoded stereo PCM clip (interleaved, 48kHz) around the drop
    pub clip_a: Vec<f32>,
    pub clip_b: Vec<f32>,
    /// Drop sample position within the full track (mono samples)
    pub drop_sample_a: u64,
    pub drop_sample_b: u64,
    /// PCA vectors for online learning
    pub pca_a: Vec<f32>,
    pub pca_b: Vec<f32>,
}

/// A completed comparison stored for this session.
#[derive(Debug, Clone)]
pub struct StoredPair {
    pub track_a_id: i64,
    pub track_b_id: i64,
    pub choice: i32, // 0=A, 1=B, 2=equal
}

/// State for the aggression calibration modal.
#[derive(Debug)]
pub struct CalibrationState {
    pub is_open: bool,
    /// Show explanation screen before starting comparisons
    pub explanation_shown: bool,
    pub phase: CalibrationPhase,
    /// Currently displayed pair (moved from preloaded queue)
    pub current_pair: Option<PreloadedPair>,
    /// Pre-loaded pairs ready to display (target: 2 ahead)
    pub preloaded_pairs: VecDeque<PreloadedPair>,
    /// Planned pairs not yet pre-loaded, organized by phase
    pub pending_anchor: Vec<(i64, i64)>,
    pub pending_intra: Vec<(i64, i64)>,
    pub pending_boundary: Vec<(i64, i64)>,
    /// Initial phase sizes (frozen at plan time, used for progress display)
    pub anchor_total: usize,
    pub intra_total: usize,
    pub boundary_total: usize,
    /// How many pairs have been completed in this session
    pub completed_count: usize,
    /// Total pairs planned across all phases
    pub total_pairs_planned: usize,
    /// Total completed across all sessions (from DB)
    pub total_historical: usize,
    /// Current learned weight vector (live-updated via SGD)
    pub weights: Vec<f32>,
    /// Leave-one-out accuracy from last batch retrain
    pub model_accuracy: f32,
    /// Communities flagged for calibration
    pub uncovered_communities: Vec<UncoveredCommunity>,
    /// Previous pairs for undo (most recent last)
    pub history: Vec<PreloadedPair>,
    /// Which side is currently playing audio (None = nothing playing)
    pub playing_side: Option<CalibrationSide>,
    /// Number of pairs preloading in background
    pub preloading_count: usize,
    /// Whether the user has been shown the calibration prompt this session
    pub prompted_this_session: bool,
}

impl Default for CalibrationState {
    fn default() -> Self {
        Self {
            is_open: false,
            explanation_shown: true,
            phase: CalibrationPhase::Anchor { current: 0, total: 0 },
            current_pair: None,
            preloaded_pairs: VecDeque::new(),
            pending_anchor: Vec::new(),
            pending_intra: Vec::new(),
            pending_boundary: Vec::new(),
            anchor_total: 0,
            intra_total: 0,
            boundary_total: 0,
            completed_count: 0,
            total_pairs_planned: 0,
            total_historical: 0,
            weights: Vec::new(),
            model_accuracy: 0.0,
            uncovered_communities: Vec::new(),
            history: Vec::new(),
            playing_side: None,
            preloading_count: 0,
            prompted_this_session: false,
        }
    }
}

impl CalibrationState {
    /// Open the calibration modal with detected uncovered communities.
    pub fn open(&mut self, uncovered: Vec<UncoveredCommunity>) {
        self.is_open = true;
        self.explanation_shown = true;
        self.uncovered_communities = uncovered;
        self.completed_count = 0;
        self.current_pair = None;
        self.preloaded_pairs.clear();
        self.playing_side = None;
        self.preloading_count = 0;
    }

    /// Close the calibration modal and stop any playback.
    pub fn close(&mut self) {
        self.is_open = false;
        self.playing_side = None;
        self.current_pair = None;
        self.preloaded_pairs.clear();
        self.history.clear();
        self.pending_anchor.clear();
        self.pending_intra.clear();
        self.pending_boundary.clear();
        self.preloading_count = 0;
    }

    /// Advance to the next pair. Returns true if a pair was available.
    pub fn advance_pair(&mut self) -> bool {
        // Save current pair to history for undo
        if let Some(prev) = self.current_pair.take() {
            self.history.push(prev);
        }
        if let Some(next) = self.preloaded_pairs.pop_front() {
            self.current_pair = Some(next);
            self.playing_side = None;
            self.completed_count += 1;
            self.update_phase();
            true
        } else {
            false
        }
    }

    /// Public wrapper for update_phase (used by back handler).
    pub fn update_phase_public(&mut self) {
        self.update_phase();
    }

    /// Update the phase enum based on completed count and frozen phase totals.
    fn update_phase(&mut self) {
        if self.completed_count <= self.anchor_total {
            self.phase = CalibrationPhase::Anchor {
                current: self.completed_count,
                total: self.anchor_total,
            };
        } else if self.completed_count <= self.anchor_total + self.intra_total {
            self.phase = CalibrationPhase::IntraCommunity {
                current: self.completed_count - self.anchor_total,
                total: self.intra_total,
            };
        } else {
            self.phase = CalibrationPhase::Boundary {
                current: self.completed_count - self.anchor_total - self.intra_total,
                total: self.boundary_total,
            };
        }
    }

    /// Whether "Finish Early" should be available (minimum 30 comparisons total).
    pub fn can_finish_early(&self) -> bool {
        self.completed_count + self.total_historical >= 30
    }

    /// Pop the next pending pair to pre-load, cycling through phases in order.
    pub fn pop_next_pending(&mut self) -> Option<(i64, i64)> {
        if let Some(pair) = self.pending_anchor.pop() {
            Some(pair)
        } else if let Some(pair) = self.pending_intra.pop() {
            Some(pair)
        } else {
            self.pending_boundary.pop()
        }
    }

    /// Total remaining pairs (pending + preloaded but not yet shown).
    pub fn remaining(&self) -> usize {
        self.pending_anchor.len()
            + self.pending_intra.len()
            + self.pending_boundary.len()
            + self.preloaded_pairs.len()
    }

    /// Estimate minutes remaining based on ~5 seconds per comparison.
    pub fn estimated_minutes(&self) -> f32 {
        self.remaining() as f32 * 5.0 / 60.0
    }

    /// Summary of uncovered communities for the explanation screen.
    pub fn community_summary(&self) -> (usize, usize) {
        let count = self.uncovered_communities.len();
        let total_tracks: usize = self.uncovered_communities.iter().map(|c| c.track_count).sum();
        (count, total_tracks)
    }
}
