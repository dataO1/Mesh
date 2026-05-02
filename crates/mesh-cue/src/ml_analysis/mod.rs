//! ML-based audio analysis module
//!
//! Provides genre classification, mood/theme tagging, voice/instrumental detection,
//! and audio characteristics using Essentia-based preprocessing + EffNet ONNX models.
//!
//! # Architecture
//!
//! - **Preprocessing** (`preprocessing.rs`): Pure Rust mel spectrogram computation
//! - **Model management** (`models.rs`): Download + cache ONNX models from Essentia Hub
//! - **Inference** (`inference.rs`): ort-based EffNet embedding → classification heads
//!   (includes voice/instrumental classifier — replaces old RMS-based detection)

pub mod preprocessing;
pub mod models;
pub mod inference;

// Re-export key types
pub use inference::{MlAnalyzer, MlAnalysisResult};
pub use models::{MlModelManager, MlModelType};

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    /// Lazily-built per-rayon-worker MAEST analyzer, keyed by model dir.
    ///
    /// **Why per-thread instead of `Arc<Mutex<MlAnalyzer>>`:** ORT 2.x's
    /// `Session::run` takes `&mut self`, so a single shared analyzer behind
    /// a Mutex serializes ALL inference across the rayon pool — 24 workers
    /// effectively act as 1. With multiple MAEST forward passes per track at
    /// ~1.5 s each, the lock chokes throughput to ~10 % CPU on a 24-core
    /// machine. Per-thread sessions remove that contention; the underlying
    /// ONNX file is mmap'd by ORT so N sessions don't cost N× the memory.
    ///
    /// Used by every place that runs MAEST inside a rayon worker
    /// (`reanalysis::run_batch_metadata_reanalysis`, `batch_import::*`).
    /// The key is the model directory so re-runs against a different cache
    /// rebuild correctly; today this never changes mid-process.
    static MAEST_ANALYZER: RefCell<Option<(PathBuf, MlAnalyzer)>> =
        const { RefCell::new(None) };
}

/// Run a closure with a thread-local `MlAnalyzer` for `model_dir`, building
/// one on first use per worker thread. Use this from any rayon-parallel
/// per-track loop instead of sharing `Arc<Mutex<MlAnalyzer>>`.
///
/// Returns the closure's result, or an error if the per-thread analyzer
/// failed to initialize (e.g. ONNX file missing).
pub fn with_thread_local_analyzer<R>(
    model_dir: &Path,
    f: impl FnOnce(&mut MlAnalyzer) -> R,
) -> Result<R, String> {
    MAEST_ANALYZER.with(|cell| {
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

/// Resolve the on-disk MAEST model directory, downloading it (with progress)
/// if missing. Returns `None` if the model cache dir can't be determined or
/// the download failed — caller should fall back to skipping ML tags.
///
/// `progress` receives `(model_display_name, bytes_done, bytes_total)` ticks
/// at ~4 Hz during a download (see `MlModelManager::ensure_all_models_with_progress`).
/// Pass a no-op closure if you don't care.
///
/// Centralised so every batch entry point downloads the same way and feeds
/// the same UI footer progress channel — see `with_thread_local_analyzer`
/// for the per-worker session pattern that goes with it.
pub fn ensure_maest_model_dir(
    progress: impl Fn(&'static str, u64, Option<u64>) + Send + Sync + 'static,
) -> Option<PathBuf> {
    let mgr = match MlModelManager::new() {
        Ok(m) => m,
        Err(e) => {
            log::error!("ensure_maest_model_dir: cannot determine model cache dir: {}", e);
            return None;
        }
    };
    if let Err(e) = mgr.ensure_all_models_with_progress(progress) {
        log::warn!("ensure_maest_model_dir: model download failed: {}", e);
        // Fall through — if the file already existed from a prior run we
        // can still proceed; the path check below is the source of truth.
    }
    let model_path = mgr.model_path(MlModelType::MaestEmbedding519l);
    if !model_path.exists() {
        log::warn!(
            "ensure_maest_model_dir: MAEST model not present at {:?} — ML tags disabled",
            model_path
        );
        return None;
    }
    Some(
        model_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf(),
    )
}
