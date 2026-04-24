//! Aggression calibration handler
//!
//! Manages the pairwise comparison flow: coverage detection, pair planning,
//! audio clip pre-loading, user choice processing, and weight learning.

use std::collections::{HashMap, HashSet};
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
    ///
    /// Builds a candidate POOL of all interesting pairs (FPS-edge × FPS-edge
    /// across communities, intra-community, vs anchor refs). The pool is fixed
    /// for the session, but which pair to ask next is chosen dynamically by
    /// `next_calibration_pair` after each user response — using active learning
    /// (uncertainty sampling) plus transitive closure of already-known relations.
    fn plan_calibration_pairs(&mut self) {
        let db = self.domain.db_arc();

        let weights = db.get_aggression_weights()
            .ok()
            .flatten()
            .map(|(w, _)| w)
            .unwrap_or_default();

        let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
        let pca_data: HashMap<i64, Vec<f32>> = all_pca.iter()
            .filter_map(|(t, v)| Some((t.id?, v.clone())))
            .collect();

        let existing_pairs = db.get_all_calibration_pairs().unwrap_or_default();

        let community_assignments: HashMap<i64, i32> = self.collection.graph_state
            .as_ref()
            .map(|gs| gs.clusters.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default();

        let current_weights = if weights.is_empty() {
            let dim = pca_data.values().next().map(|v| v.len()).unwrap_or(128);
            vec![0.0f32; dim]
        } else {
            weights.clone()
        };

        // Pick anchor reference tracks: well-calibrated tracks spanning the
        // aggression range. Fall back to PCA-projection-spanning tracks for
        // first-time calibration.
        let mut calibrated_ids: HashSet<i64> = HashSet::new();
        for &(_, a, b, _, _) in &existing_pairs {
            calibrated_ids.insert(a);
            calibrated_ids.insert(b);
        }
        let mut covered_scored: Vec<(i64, f32)> = community_assignments.keys()
            .filter(|id| calibrated_ids.contains(id))
            .filter_map(|&id| pca_data.get(&id).map(|p| (id, aggression::project_aggression(p, &current_weights))))
            .collect();
        covered_scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let anchor_refs: Vec<i64> = if covered_scored.len() >= 3 {
            let n = covered_scored.len();
            vec![covered_scored[n / 4].0, covered_scored[n / 2].0, covered_scored[n * 3 / 4].0]
        } else {
            covered_scored.iter().map(|(id, _)| *id).collect()
        };

        let plan = aggression::build_calibration_plan(
            &self.calibration.uncovered_communities,
            &pca_data,
            &anchor_refs,
            0, // unused — fixed at 2 edges + 1 centroid per community internally
        );

        let n_communities = self.calibration.uncovered_communities.len();
        log::info!(
            "[CALIBRATION] Plan: {} phase-1 (deterministic) + {} phase-2 (active learning pool) from {} communities",
            plan.phase_1.len(),
            plan.phase_2.len(),
            n_communities,
        );

        // Phase 1 has an EXACT count (deterministic). Phase 2 is heuristic.
        // Estimate phase 2 needs ~num_communities cross-community pairs to
        // pin down the global axis (transitive closure does the rest).
        let phase_2_estimate = n_communities.max(5).min(plan.phase_2.len());
        let estimated_total = plan.phase_1.len() + phase_2_estimate;

        self.calibration.anchor_total = estimated_total;
        self.calibration.intra_total = 0;
        self.calibration.boundary_total = 0;
        self.calibration.phase_1_queue = plan.phase_1.iter().copied().collect();
        self.calibration.phase_1_total = plan.phase_1.len();
        self.calibration.candidate_pool = plan.phase_2;
        self.calibration.track_community = plan.track_community;
        self.calibration.recent_communities.clear();
        self.calibration.total_pairs_planned = estimated_total;
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

            // Track which communities this pair touched, so the diversity
            // heuristic can rotate to other communities in upcoming rounds.
            // Keep a sliding window of the last 4 community IDs.
            for &id in &[track_a_id, track_b_id] {
                if let Some(&community) = self.calibration.track_community.get(&id) {
                    self.calibration.recent_communities.push_back(community);
                    while self.calibration.recent_communities.len() > 4 {
                        self.calibration.recent_communities.pop_front();
                    }
                }
            }
        }

        // Batch retrain every 10 comparisons so the accuracy indicator
        // updates at a reasonable frequency and plateau detection has signal.
        let total_done = self.calibration.completed_count + self.calibration.total_historical;
        if total_done > 0 && total_done % 10 == 0 {
            self.batch_retrain_weights();
            // Auto-stop on plateau, but ONLY after phase 1 is fully answered.
            // Phase 1 is the deterministic bootstrap that guarantees every
            // uncovered community gets its 5 representative tracks queried —
            // cutting it short leaves entire communities at 0% coverage,
            // which causes the next session to re-prompt for those communities.
            // Phase 1 pairs are drained from the queue FIFO before phase 2
            // pairs are picked, so completed_count >= phase_1_total means
            // every phase 1 pair was answered.
            let phase_1_done = self.calibration.completed_count >= self.calibration.phase_1_total;
            if phase_1_done
                && self.calibration.has_plateaued()
                && !self.calibration.completion_shown
            {
                log::info!(
                    "[CALIBRATION] Plateau detected after {} comparisons (phase 1 complete) — auto-stopping",
                    total_done,
                );
                self.calibration.completion_shown = true;
                self.calibration.playing_side = None;
                self.audio.pause();
                // Persist the learned weights immediately
                if !self.calibration.weights.is_empty() {
                    let db = self.domain.db_arc();
                    let _ = db.store_aggression_weights(&self.calibration.weights, self.calibration.model_accuracy);
                }
                return Task::none();
            } else if self.calibration.has_plateaued() && !phase_1_done {
                log::debug!(
                    "[CALIBRATION] Plateau detected but phase 1 still has {} bootstrap pairs left — continuing",
                    self.calibration.phase_1_queue.len(),
                );
            }
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
        // Pair is no longer in flight — it's now in the queue
        self.calibration.in_flight_pair_ids.remove(&(pair.track_a.id, pair.track_b.id));
        self.calibration.in_flight_pair_ids.remove(&(pair.track_b.id, pair.track_a.id));
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

    /// Handle "Restart Aggression Calibration" from collection context menu.
    /// Opens a confirmation modal — actual work happens in
    /// `handle_confirm_restart_calibration` after user confirms.
    pub fn handle_restart_calibration(&mut self) -> Task<Message> {
        self.context_menu_state.close();
        let pair_count = self.domain.db_arc().get_calibration_pair_count().unwrap_or(0);
        let has_weights = self.domain.db_arc()
            .get_aggression_weights().ok().flatten().is_some();
        self.delete_state.show(super::super::delete_modal::DeleteTarget::Custom {
            title: "Restart Aggression Calibration".to_string(),
            description: format!(
                "Discard {} stored comparison{} and {}, then start fresh?",
                pair_count,
                if pair_count == 1 { "" } else { "s" },
                if has_weights { "the learned aggression scale" } else { "no learned scale yet" },
            ),
            warning: "⚠ This permanently clears your training data and the learned model. Cannot be undone.".to_string(),
            confirm_label: "Restart Calibration".to_string(),
            confirm_message: Box::new(Message::ConfirmRestartCalibration),
        });
        Task::none()
    }

    /// Confirmed restart — clears pairs + weights, resets in-memory state,
    /// triggers fresh coverage check.
    pub fn handle_confirm_restart_calibration(&mut self) -> Task<Message> {
        let db = self.domain.db_arc();
        if let Err(e) = db.clear_calibration_pairs() {
            log::warn!("[CALIBRATION] Failed to clear pairs: {}", e);
        }
        if let Err(e) = db.clear_aggression_weights() {
            log::warn!("[CALIBRATION] Failed to clear aggression weights: {}", e);
        }
        self.calibration.weights.clear();
        self.calibration.model_accuracy = 0.0;
        self.calibration.total_historical = 0;
        self.calibration.prompted_this_session = false;
        self.calibration.close();
        self.collection.graph_suggestion_rows.clear();
        log::info!("[CALIBRATION] Restarted from scratch — pairs and weights cleared");
        self.trigger_calibration_coverage_check()
    }

    /// Batch retrain weights from ALL stored pairs.
    pub fn batch_retrain_weights(&mut self) {
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
            self.calibration.weights = new_weights;
            self.calibration.push_accuracy(accuracy);
            let plateau_note = if self.calibration.has_plateaued() { " [PLATEAUED]" } else { "" };
            log::info!(
                "[CALIBRATION] Batch retrain: accuracy={:.1}% (history: {:?}){}",
                accuracy * 100.0,
                self.calibration.accuracy_history.iter().map(|a| format!("{:.1}%", a * 100.0)).collect::<Vec<_>>(),
                plateau_note,
            );
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

    /// Pre-load the next pair in the background.
    ///
    /// Two-phase strategy:
    /// - **Phase 1**: drain `phase_1_queue` FIFO (deterministic bootstrap pairs).
    /// - **Phase 2**: once phase 1 is empty, pick from `candidate_pool` via
    ///   active learning v2 (uncertainty + transitive closure + diversity).
    pub fn preload_next_calibration_pair(&mut self) -> Option<Task<Message>> {
        if self.calibration.preloaded_pairs.len() + self.calibration.preloading_count >= 3 {
            return None;
        }

        let db = self.domain.db_arc();

        // Build in-flight exclusion set
        let mut in_flight: HashSet<(i64, i64)> = HashSet::new();
        for &(a, b) in &self.calibration.in_flight_pair_ids {
            in_flight.insert((a, b));
            in_flight.insert((b, a));
        }
        for p in &self.calibration.preloaded_pairs {
            in_flight.insert((p.track_a.id, p.track_b.id));
            in_flight.insert((p.track_b.id, p.track_a.id));
        }
        if let Some(ref p) = self.calibration.current_pair {
            in_flight.insert((p.track_a.id, p.track_b.id));
            in_flight.insert((p.track_b.id, p.track_a.id));
        }

        // Phase 1 first: drain FIFO, skipping any pair already in flight.
        let phase_1_pick = {
            let mut found = None;
            let mut idx = 0;
            while idx < self.calibration.phase_1_queue.len() {
                let p = self.calibration.phase_1_queue[idx];
                if !in_flight.contains(&p) && !in_flight.contains(&(p.1, p.0)) {
                    found = Some(self.calibration.phase_1_queue.remove(idx).unwrap());
                    break;
                }
                idx += 1;
            }
            found
        };

        let (track_a_id, track_b_id) = if let Some(p) = phase_1_pick {
            p
        } else {
            // Phase 2: active learning over the pool.
            if self.calibration.candidate_pool.is_empty() {
                return None;
            }

            let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
            let pca_data: HashMap<i64, Vec<f32>> = all_pca.iter()
                .filter_map(|(t, v)| Some((t.id?, v.clone())))
                .collect();
            let asked: Vec<(i64, i64, i32)> = db.get_all_calibration_pairs()
                .unwrap_or_default()
                .into_iter()
                .map(|(_, a, b, c, _)| (a, b, c))
                .collect();

            let pool: Vec<(i64, i64)> = self.calibration.candidate_pool.iter()
                .filter(|p| !in_flight.contains(p) && !in_flight.contains(&(p.1, p.0)))
                .copied()
                .collect();

            let recent: Vec<i32> = self.calibration.recent_communities.iter().copied().collect();

            aggression::next_calibration_pair_v2(
                &pool,
                &asked,
                &pca_data,
                &self.calibration.weights,
                &self.calibration.track_community,
                &recent,
            )?
        };

        self.calibration.preloading_count += 1;
        self.calibration.in_flight_pair_ids.insert((track_a_id, track_b_id));

        Some(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    preload_pair(db, track_a_id, track_b_id)
                })
                .await
                .ok()
                .flatten()
            },
            move |result| {
                match result {
                    Some(pair) => Message::CalibrationPairPreloaded(Box::new(pair)),
                    None => Message::CalibrationPairPreloadFailed(track_a_id, track_b_id),
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
