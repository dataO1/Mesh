//! Similarity index (PCA build) handler
//!
//! Handles the "Build Similarity Index" context menu action.
//! Loads all ML embeddings from the DB, computes a 128-dim PCA projection,
//! and stores the projected vectors back in `ml_pca_embeddings` for fast HNSW queries.

use iced::Task;
use crate::pca;
use super::super::app::MeshCueApp;
use super::super::message::Message;

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

                    // Aggression axis: pull the active text-tower-derived intensity
                    // axis from the model dir and store it as the weights vector.
                    // The old genre+mood Pearson fit is gone — its supervisor
                    // (compute_track_aggression(genre)) returned 0 for every track
                    // under MuQ-MuLan, so the fit had no signal source.
                    //
                    // The "correlation" field is repurposed as the calibration-pair
                    // agreement rate (eval-only — calibration UI no longer drives
                    // weight fitting). See documents/aggression-axis-text-tower-plan.md.
                    log::info!("[PCA] Loading text-tower intensity axis...");
                    let model_dir = match crate::ml_analysis::ensure_ml_model_dir(|_, _, _| {}) {
                        Some(d) => d,
                        None => {
                            log::warn!("[PCA] No MuQ-MuLan model dir — skipping intensity axis store");
                            return Ok(());
                        }
                    };
                    let axis_path = model_dir.join(
                        crate::ml_analysis::MlModelType::MuQMulanLarge.aggression_axis_filename()
                    );
                    if !axis_path.exists() {
                        log::warn!(
                            "[PCA] No intensity axis JSON at {:?} — intensity scoring inactive. \
                             Run `nix run .#derive-aggression-axes`, then copy a variant from \
                             models/aggression-axes/ as {}.",
                            axis_path,
                            crate::ml_analysis::MlModelType::MuQMulanLarge.aggression_axis_filename(),
                        );
                        return Ok(());
                    }
                    let axis = match crate::ml_analysis::IntensityAxis::load(&axis_path) {
                        Ok(a) => a,
                        Err(e) => {
                            log::warn!("[PCA] Failed to load intensity axis from {:?}: {}", axis_path, e);
                            return Ok(());
                        }
                    };
                    log::info!(
                        "[PCA] Active intensity axis: {} ({}) — formula: {}",
                        axis.variant_id, axis.name, axis.intensity_formula,
                    );

                    // Agreement rate against any stored calibration pairs (eval-only).
                    // Falls back to 1.0 when the user has no pairs yet — meaning
                    // "we have no contradicting evidence", not "perfect fit".
                    let pairs = db.get_all_calibration_pairs().unwrap_or_default();
                    let pair_triplets: Vec<(i64, i64, i32)> = pairs.iter()
                        .map(|(_id, a, b, choice, _ts)| (*a, *b, *choice))
                        .collect();
                    let agreement = mesh_core::suggestions::aggression::compute_pair_agreement(
                        &axis.intensity_axis_vec,
                        |id| db.get_ml_embedding_raw(id).ok().flatten(),
                        &pair_triplets,
                        0.02, // equal-band: ±0.02 cosine units around 0
                    );
                    let correlation_field = agreement.unwrap_or(1.0);
                    if let Some(rate) = agreement {
                        log::info!(
                            "[PCA] Intensity axis vs {} stored calibration pairs: {:.1}% agreement",
                            pair_triplets.len(), rate * 100.0,
                        );
                    } else {
                        log::info!(
                            "[PCA] No usable calibration pairs ({} stored) — storing axis with correlation=1.0",
                            pair_triplets.len(),
                        );
                    }

                    if let Err(e) = db.store_aggression_weights(&axis.intensity_axis_vec, correlation_field) {
                        log::warn!("[PCA] Failed to store aggression axis: {}", e);
                    } else {
                        log::info!(
                            "[PCA] Stored intensity axis ({} dims, agreement/correlation={:.4})",
                            axis.intensity_axis_vec.len(), correlation_field,
                        );
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
