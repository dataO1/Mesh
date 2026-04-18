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
            Ok(()) => log::info!("[PCA] Similarity index build complete"),
            Err(e) => log::error!("[PCA] Similarity index build failed: {}", e),
        }
        Task::none()
    }
}
