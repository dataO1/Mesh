//! ML-based audio analysis module.
//!
//! Computes a 512-dim joint-space audio embedding using the MuQ-MuLan-large
//! audio tower, exported to ONNX via `nix run .#convert-muq-mulan-model`.
//!
//! # Architecture
//!
//! - **Preprocessing** (`preprocessing.rs`): pure-Rust 24 kHz / 128-band
//!   dB-scale mel spectrogram matching MuQ's `MelSTFT`.
//! - **Model management** (`models.rs`): locates the ONNX + sidecar from
//!   the local conversion run (no remote download — the model is built
//!   in-tree, not fetched).
//! - **Inference** (`inference.rs`): per-thread `ort` session that runs
//!   the audio tower on each 10 s clip and averages the resulting 512-d
//!   embeddings.
//!
//! Genre / classification heads from the prior MAEST integration have
//! been removed — MuQ-MuLan emits embeddings only.

pub mod preprocessing;
pub mod models;
pub mod inference;

// Re-export key types
pub use inference::{MlAnalyzer, MlAnalysisResult, MUQ_MULAN_EMBEDDING_DIM};
pub use models::{MlModelManager, MlModelType};

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    /// Lazily-built per-rayon-worker MuQ-MuLan analyzer, keyed by model dir.
    ///
    /// **Why per-thread instead of `Arc<Mutex<MlAnalyzer>>`:** ORT 2.x's
    /// `Session::run` takes `&mut self`, so a single shared analyzer behind
    /// a Mutex serializes ALL inference across the rayon pool — 24 workers
    /// effectively act as 1. Per-thread sessions remove that contention;
    /// ORT mmaps the underlying ONNX so N sessions don't cost N× the memory.
    ///
    /// Used by every place that runs MuQ-MuLan inside a rayon worker
    /// (`reanalysis::run_batch_metadata_reanalysis`, `batch_import::*`).
    /// Keyed by the model directory so re-runs against a different cache
    /// rebuild correctly; today this never changes mid-process.
    static MUQ_MULAN_ANALYZER: RefCell<Option<(PathBuf, MlAnalyzer)>> =
        const { RefCell::new(None) };
}

/// Run a closure with a thread-local `MlAnalyzer` for `model_dir`, building
/// one on first use per worker thread. Use this from any rayon-parallel
/// per-track loop instead of sharing `Arc<Mutex<MlAnalyzer>>`.
///
/// Returns the closure's result, or an error if the per-thread analyzer
/// failed to initialize (e.g. ONNX file missing, sidecar mismatch).
pub fn with_thread_local_analyzer<R>(
    model_dir: &Path,
    f: impl FnOnce(&mut MlAnalyzer) -> R,
) -> Result<R, String> {
    MUQ_MULAN_ANALYZER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let needs_init = slot.as_ref().map_or(true, |(dir, _)| dir != model_dir);
        if needs_init {
            let analyzer = MlAnalyzer::new(model_dir)?;
            *slot = Some((model_dir.to_path_buf(), analyzer));
        }
        let (_, analyzer) = slot.as_mut().expect("just initialized");
        Ok(f(analyzer))
    })
}

/// Resolve the on-disk MuQ-MuLan model directory. Returns `None` if the
/// model + sidecar can't be located via any of the search paths in
/// `MlModelManager` — caller should fall back to skipping ML embeddings.
///
/// `progress` is kept for signature compatibility with the prior MAEST flow
/// but never invoked: there is no download step on the MuQ-MuLan path
/// (the ONNX is produced locally by `nix run .#convert-muq-mulan-model`).
///
/// On first call we also opportunistically copy a freshly-converted ONNX
/// from `models/` into the long-term cache, so future runs don't depend on
/// the build dir staying around.
pub fn ensure_ml_model_dir(
    progress: impl Fn(&'static str, u64, Option<u64>) + Send + Sync + 'static,
) -> Option<PathBuf> {
    let mgr = match MlModelManager::new() {
        Ok(m) => m,
        Err(e) => {
            log::error!("ensure_ml_model_dir: cannot determine model cache dir: {}", e);
            return None;
        }
    };
    if let Err(e) = mgr.ensure_all_models_with_progress(progress) {
        log::warn!(
            "ensure_ml_model_dir: model not available: {} \
             — run `nix run .#convert-muq-mulan-model` to produce it",
            e
        );
        return None;
    }
    // Pull from search dirs into the long-term cache so downstream paths can
    // rely on it. install_to_cache is a no-op when the cache copy already exists.
    let model_path = match mgr.install_to_cache(MlModelType::MuQMulanLarge) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("ensure_ml_model_dir: install_to_cache failed: {}", e);
            // Fall back to whatever the manager finds in any search dir.
            mgr.model_path(MlModelType::MuQMulanLarge)?
        }
    };
    let dir = model_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    log::info!("ensure_ml_model_dir: using MuQ-MuLan from {:?}", dir);
    Some(dir)
}
