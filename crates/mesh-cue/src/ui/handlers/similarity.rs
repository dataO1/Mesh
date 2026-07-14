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

/// Backfill / re-project the per-track `intensity_score` scalars.
///
/// For every track that has a 1024-d intensity embedding but whose
/// `intensity_score` row is missing OR was projected with a different axis
/// version than the active one, project through the active axis and upsert.
///
/// This is what makes an axis upgrade land without re-analysis: ship a new
/// embedded axis in the binary, and the first mesh-cue start re-projects
/// the whole library from the stored hidden states (µs per track). USB
/// sticks pick the fresh scalars up on their next export.
///
/// Axis resolution matches the ANALYSIS path (model-dir override →
/// embedded default), so backfilled scalars and analysis-time scalars are
/// always produced by the same axis instance.
pub fn backfill_intensity_scores(db: &Arc<DatabaseService>) -> Result<usize, String> {
    let axis = match crate::ml_analysis::ensure_ml_model_dir(|_, _, _| {}) {
        Some(model_dir) => {
            let axis_path = model_dir.join(
                crate::ml_analysis::MlModelType::MuQMulanLarge.aggression_axis_filename()
            );
            if axis_path.exists() {
                let a = crate::ml_analysis::IntensityAxis::load(&axis_path)
                    .map_err(|e| format!("load axis from {:?}: {}", axis_path, e))?;
                log::info!("[INTENSITY] Loaded user override from {:?}", axis_path);
                a
            } else {
                mesh_core::intensity_axis::IntensityAxis::embedded_default()
                    .expect("embedded axis JSON must parse")
            }
        }
        None => mesh_core::intensity_axis::IntensityAxis::embedded_default()
            .expect("embedded axis JSON must parse"),
    };
    let active_version = axis.variant_id.clone();
    log::info!("[INTENSITY] Active intensity axis: {}", active_version);

    let existing: std::collections::HashMap<i64, String> = db
        .get_all_intensity_scores()
        .map_err(|e| format!("get_all_intensity_scores: {e}"))?
        .into_iter()
        .map(|(id, _, version)| (id, version))
        .collect();

    let with_vecs = db
        .get_tracks_with_intensity_embeddings()
        .map_err(|e| format!("get_tracks_with_intensity_embeddings: {e}"))?;

    let total_vecs = with_vecs.len();
    let todo: Vec<i64> = with_vecs
        .into_iter()
        .filter(|id| existing.get(id).map_or(true, |v| *v != active_version))
        .collect();

    if todo.is_empty() {
        log::info!(
            "[INTENSITY] Scalars up to date: {} tracks on axis '{}'",
            total_vecs, active_version,
        );
        return Ok(0);
    }

    log::info!(
        "[INTENSITY] Backfilling {} of {} tracks onto axis '{}'",
        todo.len(), total_vecs, active_version,
    );

    let mut written = 0usize;
    let mut failed = 0usize;
    for id in todo {
        let emb = match db.get_ml_intensity_embedding_raw(id) {
            Ok(Some(e)) => e,
            Ok(None) => continue,
            Err(e) => {
                log::debug!("[INTENSITY] read embedding for {id}: {e}");
                failed += 1;
                continue;
            }
        };
        if emb.len() != mesh_core::intensity_axis::EMBEDDING_DIM {
            log::debug!(
                "[INTENSITY] track {id}: embedding dim {} ≠ {} — skipped",
                emb.len(), mesh_core::intensity_axis::EMBEDDING_DIM,
            );
            failed += 1;
            continue;
        }
        let score = axis.project_normalised(&emb);
        match db.store_intensity_score(id, score, &active_version) {
            Ok(()) => written += 1,
            Err(e) => {
                log::warn!("[INTENSITY] store score for {id}: {e}");
                failed += 1;
            }
        }
    }
    log::info!(
        "[INTENSITY] Backfill done: {written} written, {failed} failed (axis '{active_version}')",
    );
    Ok(written)
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

                    // Refresh the intensity scalars too. Identical work to
                    // the startup-time backfill (which is the canonical
                    // path); kept here as a belt-and-suspenders so users who
                    // hit "Build Similarity Index" after swapping axis files
                    // via scripts/select-active-axis.sh get an immediate
                    // re-projection without restarting mesh-cue.
                    if let Err(e) = backfill_intensity_scores(&db) {
                        log::warn!("[PCA] backfill_intensity_scores failed: {e}");
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
