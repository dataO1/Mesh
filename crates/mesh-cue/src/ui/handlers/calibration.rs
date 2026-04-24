//! Aggression calibration handler
//!
//! Manages the pairwise comparison flow: coverage detection, pair planning,
//! audio clip pre-loading, user choice processing, and weight learning.

use std::collections::HashMap;
use std::sync::Arc;

use iced::Task;

use mesh_core::audio_file::{AudioFileReader, LoadedTrack};
use mesh_core::suggestions::aggression;
use mesh_core::suggestions::UncoveredCommunity;

use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::calibration::{
    CalibrationSide, CalibrationTrackInfo, PreloadedPair,
};

impl MeshCueApp {
    /// Run coverage detection in the background.
    /// Called on startup (after graph state loads) and after import/PCA build.
    pub fn trigger_calibration_coverage_check(&self) -> Task<Message> {
        if self.calibration.prompted_this_session {
            return Task::none();
        }

        let db = self.domain.db_arc();

        // Need community assignments from graph state
        let community_assignments: HashMap<i64, i32> = self.collection.graph_state
            .as_ref()
            .map(|gs| {
                gs.clusters.iter()
                    .map(|(&k, &v)| (k, v))
                    .collect()
            })
            .unwrap_or_default();

        if community_assignments.is_empty() {
            return Task::none();
        }

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let pairs = db.get_all_calibration_pairs().unwrap_or_default();
                    let pair_tuples: Vec<(i64, i64, i32)> = pairs.iter()
                        .map(|&(_, a, b, c, _)| (a, b, c))
                        .collect();

                    // Load PCA vectors
                    let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
                    let pca_data: HashMap<i64, Vec<f32>> = all_pca.iter()
                        .filter_map(|(t, v)| Some((t.id?, v.clone())))
                        .collect();

                    // Load genre labels from ML analysis
                    let mut genre_labels: HashMap<i64, String> = HashMap::new();
                    for (track_id, _) in &pca_data {
                        if let Ok(Some(ml)) = db.get_ml_analysis(*track_id) {
                            if let Some(genre) = ml.top_genre {
                                genre_labels.insert(*track_id, genre);
                            }
                        }
                    }

                    aggression::detect_uncovered_communities(
                        &community_assignments,
                        &pair_tuples,
                        &pca_data,
                        &genre_labels,
                    )
                })
                .await
                .unwrap_or_default()
            },
            |uncovered| Message::CalibrationCoverageCheck(uncovered),
        )
    }

    /// Handle coverage check result — open modal if uncovered communities found.
    pub fn handle_calibration_coverage_check(&mut self, uncovered: Vec<UncoveredCommunity>) -> Task<Message> {
        if uncovered.is_empty() {
            return Task::none();
        }

        log::info!(
            "[CALIBRATION] {} uncovered communities detected ({} tracks)",
            uncovered.len(),
            uncovered.iter().map(|c| c.track_count).sum::<usize>(),
        );

        self.calibration.uncovered_communities = uncovered;
        self.calibration.prompted_this_session = true;

        // Plan pairs (need weights + PCA data)
        self.plan_calibration_pairs();

        // Auto-open the modal
        self.calibration.is_open = true;
        self.calibration.explanation_shown = true;

        // Start pre-loading pairs immediately (while user reads the explanation)
        let mut tasks = Vec::new();
        for _ in 0..3 {
            if let Some(task) = self.preload_next_calibration_pair() {
                tasks.push(task);
            }
        }
        log::info!("[CALIBRATION] Queued {} pairs for pre-loading during explanation screen", tasks.len());
        Task::batch(tasks)
    }

    /// Plan calibration pairs based on uncovered communities.
    fn plan_calibration_pairs(&mut self) {
        let db = self.domain.db_arc();

        // Load current weights (or zeros if none)
        let weights = db.get_aggression_weights()
            .ok()
            .flatten()
            .map(|(w, _)| w)
            .unwrap_or_default();

        // Load PCA data
        let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
        let pca_data: HashMap<i64, Vec<f32>> = all_pca.iter()
            .filter_map(|(t, v)| Some((t.id?, v.clone())))
            .collect();

        // Load existing pairs
        let existing_pairs = db.get_all_calibration_pairs().unwrap_or_default();
        let pair_tuples: Vec<(i64, i64, i32)> = existing_pairs.iter()
            .map(|&(_, a, b, c, _)| (a, b, c))
            .collect();

        // Get community assignments
        let community_assignments: HashMap<i64, i32> = self.collection.graph_state
            .as_ref()
            .map(|gs| gs.clusters.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default();

        let current_weights = if weights.is_empty() {
            // Create zero weights of the right dimensionality
            let dim = pca_data.values().next().map(|v| v.len()).unwrap_or(128);
            vec![0.0f32; dim]
        } else {
            weights.clone()
        };

        let (anchor, intra, boundary) = aggression::plan_calibration_pairs(
            &self.calibration.uncovered_communities,
            &community_assignments,
            &pca_data,
            &current_weights,
            &pair_tuples,
        );

        let total = anchor.len() + intra.len() + boundary.len();
        self.calibration.anchor_total = anchor.len();
        self.calibration.intra_total = intra.len();
        self.calibration.boundary_total = boundary.len();
        self.calibration.pending_anchor = anchor;
        self.calibration.pending_intra = intra;
        self.calibration.pending_boundary = boundary;
        self.calibration.total_pairs_planned = total;
        self.calibration.total_historical = existing_pairs.len();
        self.calibration.weights = current_weights;
    }

    /// Handle "Continue" from explanation screen — start comparison flow.
    pub fn handle_calibration_start(&mut self) -> Task<Message> {
        self.calibration.explanation_shown = false;

        // Pairs should already be pre-loading from the explanation screen.
        // Advance to first pair if one is ready, otherwise it will be shown
        // when CalibrationPairPreloaded arrives.
        self.calibration.advance_pair();

        // If pre-loading hasn't started yet (edge case), kick it off now
        let mut tasks = Vec::new();
        for _ in 0..3 {
            if let Some(task) = self.preload_next_calibration_pair() {
                tasks.push(task);
            }
        }
        Task::batch(tasks)
    }

    /// Handle user choice (left or right is more intense).
    pub fn handle_calibration_choice(&mut self, side: CalibrationSide) -> Task<Message> {
        let choice = match side {
            CalibrationSide::Left => 0,  // track_a more aggressive
            CalibrationSide::Right => 1, // track_b more aggressive
        };
        self.process_calibration_response(choice)
    }

    /// Handle "About the same" response.
    pub fn handle_calibration_equal(&mut self) -> Task<Message> {
        self.process_calibration_response(2) // equal
    }

    /// Handle "Skip" — discard pair and move on.
    pub fn handle_calibration_skip(&mut self) -> Task<Message> {
        self.stop_calibration_playback();

        // Advance to next pair
        if !self.calibration.advance_pair() {
            // No more pairs — finish
            return self.handle_calibration_finish();
        }

        // Pre-load another pair to keep buffer full
        self.preload_next_calibration_pair().unwrap_or(Task::none())
    }

    /// Process a user comparison response (0=A, 1=B, 2=equal).
    fn process_calibration_response(&mut self, choice: i32) -> Task<Message> {
        self.stop_calibration_playback();

        // Store the pair in DB
        if let Some(ref pair) = self.calibration.current_pair {
            let db = self.domain.db_arc();
            let track_a_id = pair.track_a.id;
            let track_b_id = pair.track_b.id;

            match db.store_calibration_pair(track_a_id, track_b_id, choice) {
                Ok(id) => {
                    let count = db.get_calibration_pair_count().unwrap_or(0);
                    log::info!("[CALIBRATION] Stored pair id={} ({} vs {}, choice={}), total={}", id, track_a_id, track_b_id, choice, count);
                }
                Err(e) => log::warn!("[CALIBRATION] Failed to store pair: {}", e),
            }

            // Online SGD step
            if !pair.pca_a.is_empty() && !pair.pca_b.is_empty() {
                aggression::sgd_step(
                    &mut self.calibration.weights,
                    &pair.pca_a,
                    &pair.pca_b,
                    choice,
                );
            }
        }

        // Batch retrain every 20 comparisons
        let total_done = self.calibration.completed_count + self.calibration.total_historical;
        if total_done > 0 && total_done % 20 == 0 {
            self.batch_retrain_weights();
        }

        // Advance to next pair
        if !self.calibration.advance_pair() {
            if self.calibration.remaining() == 0 {
                return self.handle_calibration_finish();
            }
            // Pairs are still preloading — will arrive via CalibrationPairPreloaded
        }

        // Pre-load another pair
        self.preload_next_calibration_pair().unwrap_or(Task::none())
    }

    /// Handle a pre-loaded pair arriving from the background thread.
    pub fn handle_calibration_pair_preloaded(&mut self, pair: Box<PreloadedPair>) -> Task<Message> {
        self.calibration.preloading_count = self.calibration.preloading_count.saturating_sub(1);
        log::info!("[CALIBRATION] Pair preloaded: '{}' vs '{}' (queue: {}, loading: {})",
            pair.track_a.title, pair.track_b.title,
            self.calibration.preloaded_pairs.len() + 1,
            self.calibration.preloading_count);
        self.calibration.preloaded_pairs.push_back(*pair);

        // If we don't have a current pair yet, advance
        if self.calibration.current_pair.is_none() && !self.calibration.explanation_shown {
            self.calibration.advance_pair();
        }

        Task::none()
    }

    /// Toggle audio preview for a side.
    pub fn handle_calibration_preview_toggle(&mut self, side: CalibrationSide) -> Task<Message> {
        if self.calibration.playing_side == Some(side) {
            // Stop playing — pause the engine
            self.calibration.playing_side = None;
            self.audio.pause();
        } else {
            // Start playing this side (stop the other if playing)
            self.calibration.playing_side = Some(side);

            if let Some(ref pair) = self.calibration.current_pair {
                let clip = match side {
                    CalibrationSide::Left => &pair.clip_a,
                    CalibrationSide::Right => &pair.clip_b,
                };
                let bpm = match side {
                    CalibrationSide::Left => pair.track_a.bpm.unwrap_or(174.0),
                    CalibrationSide::Right => pair.track_b.bpm.unwrap_or(174.0),
                };
                let lufs = match side {
                    CalibrationSide::Left => pair.track_a.lufs,
                    CalibrationSide::Right => pair.track_b.lufs,
                };

                // Build a LoadedTrack from the clip. Place mixed audio in "Other" stem
                // with remaining stems silent. Engine sums all stems.
                let sample_rate = 48000u32;
                let n_samples = clip.len() / 2; // interleaved stereo → mono sample count
                let other = mesh_core::types::StereoBuffer::from_interleaved(clip);
                let silence = mesh_core::types::StereoBuffer::silence(n_samples);
                let stems = mesh_core::audio_file::StemBuffers {
                    vocals: silence.clone(),
                    drums: silence.clone(),
                    bass: silence,
                    other,
                };
                let shared_stems = basedrop::Shared::new(
                    &mesh_core::engine::gc::gc_handle(),
                    stems,
                );

                // Set metadata BPM to match global BPM (no time-stretching)
                // and LUFS for loudness normalization (removes mastering bias)
                let mut metadata = mesh_core::audio_file::TrackMetadata::default();
                metadata.bpm = Some(bpm);
                metadata.lufs = lufs;
                let loaded = mesh_core::audio_file::LoadedTrack {
                    path: std::path::PathBuf::new(),
                    stems: shared_stems,
                    metadata,
                    duration_samples: n_samples,
                    duration_seconds: n_samples as f64 / sample_rate as f64,
                };

                self.audio.set_global_bpm(bpm);
                self.audio.load_track(loaded);
                self.audio.seek(0);
                self.audio.play();
            }
        }
        Task::none()
    }

    /// Handle "Finish Early" — save weights and close.
    pub fn handle_calibration_finish(&mut self) -> Task<Message> {
        // Final batch retrain on all data
        self.batch_retrain_weights();

        // Store final weights in DB
        let db = self.domain.db_arc();
        if !self.calibration.weights.is_empty() {
            if let Err(e) = db.store_aggression_weights(&self.calibration.weights, self.calibration.model_accuracy) {
                log::warn!("[CALIBRATION] Failed to store learned weights: {}", e);
            } else {
                log::info!(
                    "[CALIBRATION] Stored learned weights ({} dims, accuracy={:.1}%)",
                    self.calibration.weights.len(),
                    self.calibration.model_accuracy * 100.0,
                );
            }
        }

        // Invalidate graph state so suggestions use new weights
        self.collection.graph_state = None;
        self.collection.graph_edges = None;
        self.collection.graph_suggestion_rows.clear();

        self.calibration.close();
        Task::none()
    }

    /// Handle "Reset All" — clear all calibration data.
    pub fn handle_calibration_reset(&mut self) -> Task<Message> {
        let db = self.domain.db_arc();
        if let Err(e) = db.clear_calibration_pairs() {
            log::warn!("[CALIBRATION] Failed to clear pairs: {}", e);
        }
        self.calibration.close();
        Task::none()
    }

    /// Batch retrain weights from ALL stored pairs.
    fn batch_retrain_weights(&mut self) {
        let db = self.domain.db_arc();
        let all_pairs = db.get_all_calibration_pairs().unwrap_or_default();
        if all_pairs.is_empty() { return; }

        let pair_tuples: Vec<(i64, i64, i32)> = all_pairs.iter()
            .map(|&(_, a, b, c, _)| (a, b, c))
            .collect();

        // Load PCA vectors for all involved tracks
        let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
        let pca_data: HashMap<i64, Vec<f32>> = all_pca.iter()
            .filter_map(|(t, v)| Some((t.id?, v.clone())))
            .collect();

        if let Some((new_weights, accuracy)) = aggression::learn_weights_from_pairs(
            &pair_tuples,
            &pca_data,
            &self.calibration.weights,
        ) {
            log::info!("[CALIBRATION] Batch retrain: accuracy={:.1}%", accuracy * 100.0);
            self.calibration.weights = new_weights;
            self.calibration.model_accuracy = accuracy;
        }
    }

    /// Stop any calibration preview playback.
    fn stop_calibration_playback(&mut self) {
        if self.calibration.playing_side.is_some() {
            self.audio.pause();
            self.calibration.playing_side = None;
        }
    }

    /// Handle "Back" — undo the last comparison and return to it.
    pub fn handle_calibration_back(&mut self) -> Task<Message> {
        self.stop_calibration_playback();

        // Remove the last stored pair from DB
        let db = self.domain.db_arc();
        if let Some(last) = self.calibration.history.pop() {
            // Delete from DB by track pair
            if let Err(e) = db.delete_last_calibration_pair() {
                log::warn!("[CALIBRATION] Failed to delete last pair: {}", e);
            }

            // Push current pair back to preloaded queue front
            if let Some(current) = self.calibration.current_pair.take() {
                self.calibration.preloaded_pairs.push_front(current);
            }

            // Restore the previous pair as current
            self.calibration.current_pair = Some(last);
            self.calibration.completed_count = self.calibration.completed_count.saturating_sub(1);
            self.calibration.update_phase_public();
        }

        Task::none()
    }

    /// Pre-load the next pending pair in the background.
    pub fn preload_next_calibration_pair(&mut self) -> Option<Task<Message>> {
        // Don't pre-load more than 2 ahead
        if self.calibration.preloaded_pairs.len() + self.calibration.preloading_count >= 3 {
            return None;
        }

        let (track_a_id, track_b_id) = self.calibration.pop_next_pending()?;
        self.calibration.preloading_count += 1;

        let db = self.domain.db_arc();

        Some(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    preload_pair(db, track_a_id, track_b_id)
                })
                .await
                .ok()
                .flatten()
            },
            |result| {
                match result {
                    Some(pair) => Message::CalibrationPairPreloaded(Box::new(pair)),
                    None => Message::CalibrationPairPreloadFailed,
                }
            },
        ))
    }
}

/// Background function: load track metadata, decode audio, extract clip.
fn preload_pair(
    db: Arc<mesh_core::db::DatabaseService>,
    track_a_id: i64,
    track_b_id: i64,
) -> Option<PreloadedPair> {
    let track_a = db.get_track(track_a_id).ok().flatten()?;
    let track_b = db.get_track(track_b_id).ok().flatten()?;

    // Load PCA vectors
    let pca_a = db.get_pca_embedding_raw(track_a_id).ok().flatten().unwrap_or_default();
    let pca_b = db.get_pca_embedding_raw(track_b_id).ok().flatten().unwrap_or_default();

    // Load genre labels from ML analysis
    let genre_a = db.get_ml_analysis(track_a_id).ok().flatten()
        .and_then(|ml| ml.top_genre)
        .unwrap_or_default();
    let genre_b = db.get_ml_analysis(track_b_id).ok().flatten()
        .and_then(|ml| ml.top_genre)
        .unwrap_or_default();

    // Decode audio and extract clips
    let (clip_a, drop_a) = extract_preview_clip(&track_a.path, track_a.bpm, track_a.drop_marker)?;
    let (clip_b, drop_b) = extract_preview_clip(&track_b.path, track_b.bpm, track_b.drop_marker)?;

    let info_a = CalibrationTrackInfo {
        id: track_a_id,
        title: track_a.title.clone(),
        artist: track_a.artist.clone().unwrap_or_default(),
        genre: genre_a.split("---").last().unwrap_or(&genre_a).to_string(),
        bpm: track_a.bpm,
        key: track_a.key.clone(),
        lufs: track_a.lufs,
        path: track_a.path.clone(),
    };

    let info_b = CalibrationTrackInfo {
        id: track_b_id,
        title: track_b.title.clone(),
        artist: track_b.artist.clone().unwrap_or_default(),
        genre: genre_b.split("---").last().unwrap_or(&genre_b).to_string(),
        bpm: track_b.bpm,
        key: track_b.key.clone(),
        lufs: track_b.lufs,
        path: track_b.path.clone(),
    };

    Some(PreloadedPair {
        track_a: info_a,
        track_b: info_b,
        clip_a,
        clip_b,
        drop_sample_a: drop_a,
        drop_sample_b: drop_b,
        pca_a,
        pca_b,
    })
}

/// Decode a track's audio and extract a 16-bar clip around the drop.
///
/// When `drop_marker` is known, uses region-based decoding to only decode
/// the ~16 bars needed instead of the entire track (10-20x faster).
/// Falls back to full decode when drop position must be estimated.
fn extract_preview_clip(
    path: &std::path::Path,
    bpm: Option<f64>,
    drop_marker: Option<i64>,
) -> Option<(Vec<f32>, u64)> {
    let sample_rate = 48000u32;
    let channels = 2u32;
    let effective_bpm = bpm.unwrap_or(174.0);
    let bar_frames = (60.0 / effective_bpm * 4.0 * sample_rate as f64) as usize;
    let bars_before = 4 * bar_frames;
    let bars_after = 12 * bar_frames;

    if let Some(marker) = drop_marker {
        // Fast path: decode only the clip region
        let drop_frame = marker as usize;
        let region_start = drop_frame.saturating_sub(bars_before);
        let region_end = drop_frame + bars_after;
        let region_len = region_end - region_start;

        let reader = AudioFileReader::open(path).ok()?;
        let stems = reader.decode_region(region_start, region_len).ok()?;

        let len = stems.vocals.len();
        let mut clip = Vec::with_capacity(len * 2);
        for i in 0..len {
            let v = &stems.vocals[i];
            let d = &stems.drums[i];
            let b = &stems.bass[i];
            let o = &stems.other[i];
            clip.push(v.left + d.left + b.left + o.left);
            clip.push(v.right + d.right + b.right + o.right);
        }

        apply_fade(&mut clip, sample_rate, channels);
        Some((clip, marker as u64))
    } else {
        // Slow path: full decode needed to estimate drop position
        let stems = LoadedTrack::load_stems(path).ok()?;

        let len = stems.vocals.len();
        let mut mixed = Vec::with_capacity(len * 2);
        for i in 0..len {
            let v = &stems.vocals[i];
            let d = &stems.drums[i];
            let b = &stems.bass[i];
            let o = &stems.other[i];
            mixed.push(v.left + d.left + b.left + o.left);
            mixed.push(v.right + d.right + b.right + o.right);
        }

        let drop_sample = aggression::estimate_drop_sample(&mixed, sample_rate, channels);

        let drop_interleaved = drop_sample as usize * channels as usize;
        let start = drop_interleaved.saturating_sub(bars_before * channels as usize);
        let end = (drop_interleaved + bars_after * channels as usize).min(mixed.len());

        if end <= start {
            return Some((mixed, drop_sample));
        }

        let mut clip = mixed[start..end].to_vec();
        apply_fade(&mut clip, sample_rate, channels);
        Some((clip, drop_sample))
    }
}

/// Apply 50ms fade-in and fade-out to an interleaved stereo buffer.
fn apply_fade(clip: &mut [f32], sample_rate: u32, channels: u32) {
    let fade_samples = (sample_rate as f32 * 0.05) as usize * channels as usize;
    let clip_len = clip.len();
    for i in 0..fade_samples.min(clip_len) {
        let gain = i as f32 / fade_samples as f32;
        clip[i] *= gain;
    }
    for i in 0..fade_samples.min(clip_len) {
        let idx = clip_len - 1 - i;
        let gain = i as f32 / fade_samples as f32;
        clip[idx] *= gain;
    }
}
