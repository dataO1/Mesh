//! ML model management for audio analysis
//!
//! Handles downloading, caching, and locating ONNX models from Essentia's model hub.
//! Models are downloaded on first use and cached in `~/.cache/mesh-cue/ml-models/`.
//!
//! Follows the same pattern as `separation/model.rs` (ModelManager for Demucs).
//!
//! Branch `embeddings-upgrade`: EffNet has been replaced wholesale by
//! MAEST (`discogs-maest-30s-pw-519l-2`). The classification heads
//! (timbre/tonal/danceability/...) were trained against EffNet's 1280-dim
//! embedding and *do not work* against MAEST's 2304-dim vector — they are
//! disabled on this branch. Heads will be retrained against MAEST as a
//! follow-up; see `documents/embedding-models-research.md`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Types of ML models for audio analysis.
///
/// MAEST is the only model on this branch — the legacy EffNet classification
/// heads (mood/voice/timbre/tonal/danceability/approachability/reverb) were
/// removed wholesale alongside the EffNet embedding swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlModelType {
    /// MAEST PaSST/AST embedding (~348 MB).
    /// MTG-UPF, trained on 4M tracks across 519 Discogs styles.
    /// Input: melspectrogram [1, 1876, 96] @ 16kHz.
    /// Outputs of interest:
    ///   * `PartitionedCall/Identity_7` — layer-7 token embeddings
    ///     `[1, n_tokens, 768]`, pooled to 2304-dim (CLS|DIST|mean(rest)).
    ///   * `PartitionedCall/Identity_13` — 519-class sigmoid genre predictions.
    /// See `documents/embedding-models-research.md`.
    MaestEmbedding519l,
}

impl MlModelType {
    /// Filename for caching
    pub fn filename(&self) -> &'static str {
        match self {
            MlModelType::MaestEmbedding519l => "discogs-maest-30s-pw-519l-2.onnx",
        }
    }

    /// Download URL
    pub fn download_url(&self) -> &'static str {
        match self {
            MlModelType::MaestEmbedding519l => "https://essentia.upf.edu/models/feature-extractors/maest/discogs-maest-30s-pw-519l-2.onnx",
        }
    }

    /// Human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            MlModelType::MaestEmbedding519l => "MAEST (519-style Embedding + Genre)",
        }
    }

    /// Models required for ML analysis on this branch.
    pub fn base_models() -> &'static [MlModelType] {
        &[MlModelType::MaestEmbedding519l]
    }
}

/// Manages ML model downloads and caching
pub struct MlModelManager {
    cache_dir: PathBuf,
}

impl MlModelManager {
    /// Create with default cache directory: `~/.cache/mesh-cue/ml-models/`
    pub fn new() -> Result<Self, String> {
        let base = dirs::cache_dir()
            .ok_or_else(|| "Could not determine cache directory".to_string())?;
        Ok(Self {
            cache_dir: base.join("mesh-cue").join("ml-models"),
        })
    }

    /// Create with a custom cache directory (for testing)
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Get the local path for a model
    pub fn model_path(&self, model: MlModelType) -> PathBuf {
        self.cache_dir.join(model.filename())
    }

    /// Check if a model is already downloaded
    pub fn is_available(&self, model: MlModelType) -> bool {
        self.model_path(model).exists()
    }

    /// Check if all required models are available
    pub fn are_base_models_available(&self) -> bool {
        MlModelType::base_models().iter().all(|m| self.is_available(*m))
    }

    /// Get model path, downloading if necessary
    ///
    /// # Arguments
    /// * `model` - The model type to ensure
    /// * `progress` - Optional progress callback (0.0 to 1.0)
    pub fn ensure_model(
        &self,
        model: MlModelType,
        progress: Option<Box<dyn Fn(f32) + Send>>,
    ) -> Result<PathBuf, String> {
        let model_path = self.model_path(model);

        if model_path.exists() {
            log::info!("ML model {} found at {:?}", model.display_name(), model_path);
            if let Some(cb) = &progress {
                cb(1.0);
            }
            return Ok(model_path);
        }

        log::info!("Downloading ML model {} from {}", model.display_name(), model.download_url());
        self.download_file(model.download_url(), &model_path, progress)?;
        Ok(model_path)
    }

    /// Ensure all models needed for ML analysis are available
    pub fn ensure_all_models(&self) -> Result<(), String> {
        for &model in MlModelType::base_models() {
            self.ensure_model(model, None)?;
        }
        Ok(())
    }

    /// Ensure all models, reporting per-model byte-level download progress.
    ///
    /// `progress` is invoked with `(model_display_name, bytes_done, bytes_total)`
    /// at most every ~250 ms during a download (and once at completion).
    /// Skipped entirely for models already cached on disk.
    pub fn ensure_all_models_with_progress(
        &self,
        progress: impl Fn(&'static str, u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let progress = std::sync::Arc::new(progress);
        for &model in MlModelType::base_models() {
            let model_path = self.model_path(model);
            if model_path.exists() {
                continue;
            }
            log::info!(
                "Downloading ML model {} from {}",
                model.display_name(),
                model.download_url()
            );
            let cb_progress = std::sync::Arc::clone(&progress);
            self.download_file_with_bytes(
                model.download_url(),
                &model_path,
                Box::new(move |done, total| {
                    cb_progress(model.display_name(), done, total);
                }),
            )?;
        }
        Ok(())
    }

    /// Download a file from URL to target path with atomic rename
    fn download_file(
        &self,
        url: &str,
        target_path: &Path,
        progress: Option<Box<dyn Fn(f32) + Send>>,
    ) -> Result<(), String> {
        // Adapt the legacy fractional-progress callback to the byte-level helper.
        let bytes_cb: Box<dyn Fn(u64, Option<u64>) + Send> = if let Some(cb) = progress {
            Box::new(move |done, total| {
                if let Some(total) = total {
                    if total > 0 {
                        cb((done as f32 / total as f32).min(0.99));
                    }
                }
            })
        } else {
            Box::new(|_, _| {})
        };
        self.download_file_with_bytes(url, target_path, bytes_cb)
    }

    /// Download with byte-level progress reporting (done, total) and
    /// periodic logging — caller-supplied callback is invoked at most every
    /// ~250 ms while bytes are streaming.
    fn download_file_with_bytes(
        &self,
        url: &str,
        target_path: &Path,
        progress: Box<dyn Fn(u64, Option<u64>) + Send>,
    ) -> Result<(), String> {
        fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        let temp_path = target_path.with_extension("tmp");

        let response = ureq::get(url)
            .call()
            .map_err(|e| format!("Download failed for {}: {}", url, e))?;

        let content_length: Option<u64> = response
            .header("Content-Length")
            .and_then(|s| s.parse().ok());

        let total_mb = content_length.map(|b| b as f64 / 1_048_576.0);
        let display_name = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("model");
        match total_mb {
            Some(mb) => log::info!("download_file: {} ({:.1} MB)", display_name, mb),
            None => log::info!("download_file: {} (size unknown)", display_name),
        }

        let mut file = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temp file: {}", e))?;

        let mut reader = response.into_reader();
        let mut buffer = [0u8; 8192];
        let mut downloaded: u64 = 0;
        let mut last_callback = std::time::Instant::now();
        let mut last_log = std::time::Instant::now();
        let started_at = std::time::Instant::now();
        // Always emit the initial 0-byte progress so the UI can switch to the
        // download bar before the first chunk arrives (helpful on slow links).
        progress(0, content_length);

        loop {
            let bytes_read = reader.read(&mut buffer)
                .map_err(|e| format!("Read error: {}", e))?;
            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read])
                .map_err(|e| format!("Write error: {}", e))?;

            downloaded += bytes_read as u64;

            // Throttle UI updates to ~4 Hz to avoid flooding the channel.
            if last_callback.elapsed() >= std::time::Duration::from_millis(250) {
                progress(downloaded, content_length);
                last_callback = std::time::Instant::now();
            }
            // Throttle log lines to ~1 every 5 s so long downloads aren't silent.
            if last_log.elapsed() >= std::time::Duration::from_secs(5) {
                let elapsed_s = started_at.elapsed().as_secs_f64().max(0.001);
                let mb_done = downloaded as f64 / 1_048_576.0;
                let rate_mbps = mb_done / elapsed_s;
                match content_length {
                    Some(total) => {
                        let pct = (downloaded as f64 / total as f64 * 100.0).min(100.0);
                        log::info!(
                            "download_file: {} {:.1}% ({:.1}/{:.1} MB, {:.2} MB/s)",
                            display_name, pct, mb_done, total as f64 / 1_048_576.0, rate_mbps
                        );
                    }
                    None => log::info!(
                        "download_file: {} {:.1} MB ({:.2} MB/s)",
                        display_name, mb_done, rate_mbps
                    ),
                }
                last_log = std::time::Instant::now();
            }
        }
        // Final progress tick at 100%.
        progress(downloaded, content_length);

        file.flush().map_err(|e| format!("Flush error: {}", e))?;
        drop(file);

        // Verify size
        let actual_size = fs::metadata(&temp_path)
            .map_err(|e| format!("Metadata error: {}", e))?
            .len();

        if let Some(expected) = content_length {
            if actual_size != expected {
                fs::remove_file(&temp_path).ok();
                return Err(format!(
                    "Download incomplete: expected {} bytes, got {}",
                    expected, actual_size
                ));
            }
        }

        // Atomic rename
        fs::rename(&temp_path, target_path)
            .map_err(|e| format!("Rename failed: {}", e))?;

        log::info!("Downloaded ML model {:?} ({} bytes)", target_path.file_name().unwrap_or_default(), actual_size);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_paths() {
        let mgr = MlModelManager::with_cache_dir("/tmp/test-ml".into());
        assert!(mgr.model_path(MlModelType::MaestEmbedding519l).to_str().unwrap().contains("discogs-maest-30s-pw-519l"));
        assert!(mgr.model_path(MlModelType::MaestEmbedding519l).to_str().unwrap().contains("discogs-maest-30s-pw-519l"));
    }

    #[test]
    fn test_base_models_list() {
        // embeddings-upgrade: only MAEST is required at runtime; heads disabled.
        assert_eq!(MlModelType::base_models().len(), 1);
    }
}
