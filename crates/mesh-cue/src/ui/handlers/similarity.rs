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

                    // Compute PCA aggression axis from genre + mood tags
                    log::info!("[PCA] Computing aggression axis from genre + mood tags...");
                    let all_pca = db.get_all_pca_with_tracks().unwrap_or_default();
                    let pca_data: Vec<(i64, Vec<f32>)> = all_pca.iter()
                        .filter_map(|(t, v)| Some((t.id?, v.clone())))
                        .collect();

                    // Build aggression estimates from ML analysis (genre + mood)
                    let mut aggression_estimates = std::collections::HashMap::new();
                    for (track_id, _) in &pca_data {
                        if let Ok(Some(ml)) = db.get_ml_analysis(*track_id) {
                            let genre = ml.top_genre.as_deref().unwrap_or("");
                            let aggr = mesh_core::suggestions::aggression::compute_track_aggression(
                                genre, ml.mood_themes.as_ref(),
                            );
                            aggression_estimates.insert(*track_id, aggr);
                        }
                    }

                    if let Some((weights, combined_r)) = mesh_core::suggestions::aggression::compute_aggression_weights(
                        &pca_data, &aggression_estimates,
                    ) {
                        let n_nonzero = weights.iter().filter(|w| w.abs() > 0.01).count();
                        log::info!("[PCA] Aggression weights: {} dims, {} significant, combined r={:.4}",
                            weights.len(), n_nonzero, combined_r);
                        if let Err(e) = db.store_aggression_weights(&weights, combined_r) {
                            log::warn!("[PCA] Failed to store aggression weights: {}", e);
                        }
                    } else {
                        log::warn!("[PCA] Could not compute aggression weights (insufficient genre/mood data)");
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
                // Invalidate graph state so next Graph tab visit rebuilds
                // with the new PCA embeddings (possibly different dimensionality)
                self.collection.graph_state = None;
                self.collection.graph_edges = None;
                self.collection.graph_suggestion_rows.clear();
            }
            Err(e) => log::error!("[PCA] Similarity index build failed: {}", e),
        }
        Task::none()
    }
}
