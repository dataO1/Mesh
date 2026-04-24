//! Aggression calibration state
//!
//! Tracks the state of the pairwise comparison calibration modal,
//! including pre-loaded audio clips, pair queue, and learned weights.

use std::collections::{HashSet, VecDeque};
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
    /// Phase 1 (deterministic, asked first FIFO): bootstrap pairs that
    /// guarantee broad coverage — one intra per community + a few anchor
    /// pairs across the global aggression range.
    pub phase_1_queue: VecDeque<(i64, i64)>,
    /// Original phase 1 size (frozen at plan time). Used so estimated_remaining
    /// can compute "phase 2 done = completed - phase_1_original" without
    /// losing track as the queue drains.
    pub phase_1_total: usize,
    /// Phase 2 pool (active learning picks from here once phase 1 is exhausted).
    /// All cross-community + remaining intra + edge-vs-anchor combinations.
    pub candidate_pool: Vec<(i64, i64)>,
    /// Map: track_id → community_id. Used by the diversity heuristic so the
    /// active learner doesn't keep picking pairs from the same community.
    pub track_community: std::collections::HashMap<i64, i32>,
    /// Map: track_id → full Discogs EffNet label ("Super---Sub").
    /// Populated at plan time from each track's ml_analysis.genre_scores[0].
    /// Used by the active learner to weight pair scores by prior aggression
    /// gap, and by the answer handler to detect contradicting answers.
    pub genre_labels: std::collections::HashMap<i64, String>,
    /// Communities asked about in the last few rounds (for diversity rotation).
    pub recent_communities: VecDeque<i32>,
    /// Initial phase sizes (frozen at plan time, used for progress display).
    /// Kept for backward compat with phase enum; with active learning, all pairs
    /// are treated uniformly so we put the total in anchor_total.
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
    /// History of accuracies from recent batch retrains (most recent last).
    /// Used for plateau detection — when the last 3 entries are within a small
    /// threshold, the model has converged and we nudge the user to finish.
    pub accuracy_history: Vec<f32>,
    /// Communities flagged for calibration
    pub uncovered_communities: Vec<UncoveredCommunity>,
    /// Previous pairs for undo (most recent last)
    pub history: Vec<PreloadedPair>,
    /// Which side is currently playing audio (None = nothing playing)
    pub playing_side: Option<CalibrationSide>,
    /// Number of pairs preloading in background
    pub preloading_count: usize,
    /// Pair IDs currently being preloaded (in flight). Used by the active
    /// learner to avoid scheduling the same pair multiple times when several
    /// preload tasks run concurrently.
    pub in_flight_pair_ids: HashSet<(i64, i64)>,
    /// Whether the user has been shown the calibration prompt this session
    pub prompted_this_session: bool,
    /// Set true once auto-stop fires (plateau detected). Switches the modal
    /// into a "completion" state — comparison UI hidden, summary shown,
    /// single "Done" button persists weights and closes.
    pub completion_shown: bool,
}

impl Default for CalibrationState {
    fn default() -> Self {
        Self {
            is_open: false,
            explanation_shown: true,
            phase: CalibrationPhase::Anchor { current: 0, total: 0 },
            current_pair: None,
            preloaded_pairs: VecDeque::new(),
            phase_1_queue: VecDeque::new(),
            phase_1_total: 0,
            candidate_pool: Vec::new(),
            track_community: std::collections::HashMap::new(),
            genre_labels: std::collections::HashMap::new(),
            recent_communities: VecDeque::new(),
            anchor_total: 0,
            intra_total: 0,
            boundary_total: 0,
            completed_count: 0,
            total_pairs_planned: 0,
            total_historical: 0,
            weights: Vec::new(),
            model_accuracy: 0.0,
            accuracy_history: Vec::new(),
            uncovered_communities: Vec::new(),
            history: Vec::new(),
            playing_side: None,
            preloading_count: 0,
            in_flight_pair_ids: HashSet::new(),
            prompted_this_session: false,
            completion_shown: false,
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
        self.in_flight_pair_ids.clear();
        self.accuracy_history.clear();
        self.model_accuracy = 0.0;
        self.completion_shown = false;
    }

    /// Close the calibration modal and stop any playback.
    pub fn close(&mut self) {
        self.is_open = false;
        self.playing_side = None;
        self.current_pair = None;
        self.preloaded_pairs.clear();
        self.history.clear();
        self.phase_1_queue.clear();
        self.phase_1_total = 0;
        self.candidate_pool.clear();
        self.track_community.clear();
        self.recent_communities.clear();
        self.preloading_count = 0;
        self.in_flight_pair_ids.clear();
        self.accuracy_history.clear();
        self.model_accuracy = 0.0;
        self.completion_shown = false;
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

    /// Total remaining pairs to potentially ask. Phase 1 has an exact count
    /// (queue size); phase 2 is the candidate pool (active learning will pick
    /// far fewer than this — see `estimated_remaining` for the realistic count).
    pub fn remaining(&self) -> usize {
        self.phase_1_queue.len() + self.candidate_pool.len() + self.preloaded_pairs.len()
    }

    /// Record a new accuracy measurement from a batch retrain. Keeps the last
    /// 10 entries — enough lookback for plateau detection (need both a
    /// "running max" baseline and a "recent" window).
    pub fn push_accuracy(&mut self, accuracy: f32) {
        self.model_accuracy = accuracy;
        self.accuracy_history.push(accuracy);
        if self.accuracy_history.len() > 10 {
            self.accuracy_history.remove(0);
        }
    }

    /// True when the model has stopped improving for several retrains.
    ///
    /// Definition: the last 3 retrain accuracies have not exceeded the running
    /// maximum by more than 1.5%. This is robust to per-retrain noise (LOO
    /// accuracy can swing several percent between batches even when the model
    /// has stabilised) — what matters is whether NEW answers are pushing the
    /// model to higher accuracy or not.
    pub fn has_plateaued(&self) -> bool {
        let n = self.accuracy_history.len();
        if n < 4 { return false; }
        // Running max over all but the last 3 entries
        let prior_max = self.accuracy_history[..n - 3]
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        // None of the last 3 exceeded prior_max by 1.5%
        let recent = &self.accuracy_history[n - 3..];
        recent.iter().all(|&a| a <= prior_max + 0.015)
    }

    /// Estimated total comparisons (phase 1 exact + phase 2 heuristic),
    /// independent of progress. Used for the initial modal estimate.
    pub fn estimated_total(&self) -> usize {
        let n_communities = self.uncovered_communities.len();
        let phase_2_heuristic = n_communities.max(5).min(self.candidate_pool.len());
        self.phase_1_total + phase_2_heuristic
    }

    /// Estimated remaining comparisons after the user's current progress.
    /// = estimated_total - completed_count, clamped to >= 0.
    pub fn estimated_remaining(&self) -> usize {
        self.estimated_total().saturating_sub(self.completed_count)
    }

    /// Estimate minutes remaining based on ~5 seconds per comparison.
    pub fn estimated_minutes(&self) -> f32 {
        self.estimated_remaining() as f32 * 5.0 / 60.0
    }

    /// Summary of uncovered communities for the explanation screen.
    pub fn community_summary(&self) -> (usize, usize) {
        let count = self.uncovered_communities.len();
        let total_tracks: usize = self.uncovered_communities.iter().map(|c| c.track_count).sum();
        (count, total_tracks)
    }
}
