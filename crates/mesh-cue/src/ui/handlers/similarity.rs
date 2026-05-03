//! Similarity index (PCA build) handler
//!
//! Handles the "Build Similarity Index" context menu action.
//! Loads all ML embeddings from the DB, computes a 128-dim PCA projection,
//! and stores the projected vectors back in `ml_pca_embeddings` for fast HNSW queries.

use iced::Task;
use std::sync::Arc;
use mesh_core::db::DatabaseService;
use crate::pca;
use super::super::app::MeshCueApp;
use super::super::message::Message;

/// Load the active text-tower intensity axis JSON and store it into
/// `pca_aggression_axis` if absent or wrong dim. Idempotent. Logs (but does
/// not return) errors. Called at app startup AND at the end of "Build
/// Similarity Index" — two independent paths to keep the axis fresh in the DB.
///
/// **Why not gate on calibration weights?** The calibration UI is currently
/// disabled (it could overwrite the axis with Pearson-fit weights, which is
/// the bug we hit on V11 rollout). When re-enabled in eval-only mode it will
/// no longer write weights, so this loader can stay simple: if the existing
/// row has the right dim AND the same first 8 floats as the on-disk axis,
/// skip; otherwise overwrite.
pub fn ensure_intensity_axis_in_db(db: &Arc<DatabaseService>) -> Result<(), String> {
    let model_dir = match crate::ml_analysis::ensure_ml_model_dir(|_, _, _| {}) {
        Some(d) => d,
        None => {
            log::warn!("[AXIS] No MuQ-MuLan model dir — skipping intensity axis store");
            return Ok(());
        }
    };
    let axis_path = model_dir.join(
        crate::ml_analysis::MlModelType::MuQMulanLarge.aggression_axis_filename()
    );
    if !axis_path.exists() {
        log::warn!(
            "[AXIS] No intensity axis JSON at {:?} — intensity scoring inactive. \
             Run `nix run .#derive-aggression-axes` then `scripts/select-active-axis.sh <id>`.",
            axis_path,
        );
        return Ok(());
    }
    let axis = crate::ml_analysis::IntensityAxis::load(&axis_path)
        .map_err(|e| format!("load axis from {:?}: {}", axis_path, e))?;
    log::info!(
        "[AXIS] Active intensity axis: {} ({}) — formula: {}",
        axis.variant_id, axis.name, axis.intensity_formula,
    );

    // Skip the write if the on-disk axis already matches what's stored —
    // checked via length + first 8 floats. Saves a write on every launch.
    if let Ok(Some((stored_weights, stored_corr))) = db.get_aggression_weights() {
        let same_len = stored_weights.len() == axis.intensity_axis_vec.len();
        let same_head = stored_weights.iter().take(8)
            .zip(axis.intensity_axis_vec.iter().take(8))
            .all(|(a, b)| (a - b).abs() < 1e-6);
        if same_len && same_head {
            log::info!(
                "[AXIS] DB already has matching axis ({} dims, correlation={:.4}) — skip write",
                stored_weights.len(), stored_corr,
            );
            return Ok(());
        }
    }

    // Agreement rate against any stored calibration pairs (eval-only signal).
    let pairs = db.get_all_calibration_pairs().unwrap_or_default();
    let pair_triplets: Vec<(i64, i64, i32)> = pairs.iter()
        .map(|(_id, a, b, choice, _ts)| (*a, *b, *choice))
        .collect();
    let agreement = mesh_core::suggestions::aggression::compute_pair_agreement(
        &axis.intensity_axis_vec,
        |id| db.get_ml_embedding_raw(id).ok().flatten(),
        &pair_triplets,
        0.02,
    );
    let correlation_field = agreement.unwrap_or(1.0);
    if let Some(rate) = agreement {
        log::info!(
            "[AXIS] vs {} stored calibration pairs: {:.1}% agreement",
            pair_triplets.len(), rate * 100.0,
        );
    } else {
        log::info!(
            "[AXIS] No usable calibration pairs ({} stored) — storing axis with correlation=1.0",
            pair_triplets.len(),
        );
    }

    db.store_aggression_weights(&axis.intensity_axis_vec, correlation_field)
        .map_err(|e| format!("store_aggression_weights: {e}"))?;
    log::info!(
        "[AXIS] Stored ({} dims, agreement/correlation={:.4})",
        axis.intensity_axis_vec.len(), correlation_field,
    );
    Ok(())
}

impl MeshCueApp {
    /// Kick off the background PCA build from all ML embeddings in the library.
    pub fn handle_build_similarity_index(&mut self) -> Task<Message> {
        // Guard against double-start
        if self.pca_build_progress.is_some() {
            return Task::none();
        }

        self.context_menu_state.close();
        let db = self.domain.db_arc();

        let (tx, rx) = std::sync::mpsc::channel();
        self.pca_progress_rx = Some(rx);
        self.pca_build_progress = Some((0, 0));

        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    log::info!("[PCA] Loading ML embeddings for similarity index build...");

                    let embeddings = db.get_all_ml_embeddings()
                        .map_err(|e| format!("Failed to load embeddings: {e}"))?;

                    let total = embeddings.len();
                    log::info!("[PCA] Starting build: {} tracks with ML embeddings", total);
                    let _ = tx.send((0, total));

                    if total < 10 {
                        return Err(format!(
                            "Not enough tracks with ML embeddings ({total}) — analyse at least 10 first"
                        ));
                    }

                    // Compute PCA projection (CPU-intensive)
                    let projection = pca::compute_pca_projection(&embeddings, None)
                        .map_err(|e| format!("PCA computation failed: {e}"))?;

                    log::info!("[PCA] Projection built. Storing {}-dim vectors...", projection.n_components);

                    // Wipe stale PCA rows before inserting fresh ones. Otherwise
                    // tracks without a current ML embedding (here: 56/910 after
                    // a partial reanalysis) keep their old-dim PCA vectors,
                    // and later code that reads the relation panics when it
                    // sees mixed dimensions in the same table.
                    if let Err(e) = db.clear_all_pca_embeddings() {
                        log::warn!("[PCA] Failed to clear stale PCA rows before rebuild: {e}");
                    }

                    // Store projected vectors with progress updates
                    let mut stored = 0usize;
                    for (i, (track_id, raw_vec)) in embeddings.iter().enumerate() {
                        let pca_vec = projection.project(raw_vec);
                        if let Err(e) = db.store_pca_embedding(*track_id, &pca_vec) {
                            log::warn!("[PCA] Failed to store embedding for track {}: {}", track_id, e);
                        } else {
                            stored += 1;
                        }
                        // Send progress every 10 tracks to avoid channel spam
                        if (i + 1) % 10 == 0 || i + 1 == total {
                            let _ = tx.send((i + 1, total));
                        }
                    }

                    log::info!("[PCA] Build complete: {} PCA embeddings stored (of {} total)", stored, total);

                    // Refresh the intensity axis too. Identical work to the
                    // startup-time loader (which is the canonical path); kept
                    // here as a belt-and-suspenders so users who hit "Build
                    // Similarity Index" after swapping variant files via
                    // scripts/select-active-axis.sh get an immediate refresh
                    // without restarting mesh-cue.
                    if let Err(e) = ensure_intensity_axis_in_db(&db) {
                        log::warn!("[PCA] ensure_intensity_axis_in_db failed: {e}");
                    }

                    Ok(())
                })
                .await
                .map_err(|e| format!("Task panicked: {e}"))?
            },
            Message::SimilarityIndexComplete,
        )
    }

    /// Handle PCA build result
    pub fn handle_similarity_index_complete(&mut self, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => {
                log::info!("[PCA] Similarity index build complete");
                // Invalidate graph state and rebuild immediately so the user
                // doesn't land on a "No PCA embeddings found" screen while
                // sitting on the Graph tab — the old behavior waited for a
                // tab-switch to pick up the new embeddings.
                self.collection.graph_state = None;
                self.collection.graph_edges = None;
                self.collection.graph_suggestion_rows.clear();
                return self.handle_build_graph_edges();
            }
            Err(e) => log::error!("[PCA] Similarity index build failed: {}", e),
        }
        Task::none()
    }
}
