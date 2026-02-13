//! ML model management for audio analysis
//!
//! Handles downloading, caching, and locating ONNX models from Essentia's model hub.
//! Models are downloaded on first use and cached in `~/.cache/mesh-cue/ml-models/`.
//!
//! Follows the same pattern as `separation/model.rs` (ModelManager for Demucs).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Types of ML models for audio analysis
///
/// All models use the EffNet (discogs-effnet) embedding pipeline.
/// Arousal/valence is derived from Jamendo mood predictions — no separate
/// DEAM model is used because no EffNet-compatible A/V head exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlModelType {
    /// EffNet embedding model (~17 MB) — always required
    /// Produces 1280-dim embeddings from mel spectrograms
    EffNetEmbedding,
    /// Genre Discogs400 classification head (~2 MB) — always loaded
    /// 400-class sigmoid output over Discogs genre taxonomy
    GenreDiscogs400,
    /// Jamendo mood/theme classification head (~500 KB) — experimental only
    /// 56-class sigmoid output over mood/theme tags
    JamendoMood,
}

impl MlModelType {
    /// Filename for caching
    pub fn filename(&self) -> &'static str {
        match self {
            // Note: bsdynamic = dynamic batch size ONNX variant (bs64 is TF-only)
            MlModelType::EffNetEmbedding => "discogs-effnet-bsdynamic-1.onnx",
            MlModelType::GenreDiscogs400 => "genre_discogs400-discogs-effnet-1.onnx",
            MlModelType::JamendoMood => "mtg_jamendo_moodtheme-discogs-effnet-1.onnx",
        }
    }

    /// Download URL from Essentia's model hub
    pub fn download_url(&self) -> &'static str {
        match self {
            MlModelType::EffNetEmbedding => "https://essentia.upf.edu/models/feature-extractors/discogs-effnet/discogs-effnet-bsdynamic-1.onnx",
            MlModelType::GenreDiscogs400 => "https://essentia.upf.edu/models/classification-heads/genre_discogs400/genre_discogs400-discogs-effnet-1.onnx",
            MlModelType::JamendoMood => "https://essentia.upf.edu/models/classification-heads/mtg_jamendo_moodtheme/mtg_jamendo_moodtheme-discogs-effnet-1.onnx",
        }
    }

    /// Human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            MlModelType::EffNetEmbedding => "EffNet Embedding",
            MlModelType::GenreDiscogs400 => "Genre Discogs400",
            MlModelType::JamendoMood => "Jamendo Mood/Theme",
        }
    }

    /// Models always required (even without experimental flag)
    pub fn base_models() -> &'static [MlModelType] {
        &[MlModelType::EffNetEmbedding, MlModelType::GenreDiscogs400]
    }

    /// Models only loaded when experimental ML is enabled
    pub fn experimental_models() -> &'static [MlModelType] {
        &[MlModelType::JamendoMood]
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

    /// Ensure all models needed for the given configuration
    pub fn ensure_all_models(
        &self,
        experimental: bool,
    ) -> Result<(), String> {
        for &model in MlModelType::base_models() {
            self.ensure_model(model, None)?;
        }

        if experimental {
            for &model in MlModelType::experimental_models() {
                self.ensure_model(model, None)?;
            }
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
        fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        let temp_path = target_path.with_extension("tmp");

        let response = ureq::get(url)
            .call()
            .map_err(|e| format!("Download failed for {}: {}", url, e))?;

        let content_length: Option<u64> = response
            .header("Content-Length")
            .and_then(|s| s.parse().ok());

        let mut file = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temp file: {}", e))?;

        let mut reader = response.into_reader();
        let mut buffer = [0u8; 8192];
        let mut downloaded: u64 = 0;

        loop {
            let bytes_read = reader.read(&mut buffer)
                .map_err(|e| format!("Read error: {}", e))?;
            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read])
                .map_err(|e| format!("Write error: {}", e))?;

            downloaded += bytes_read as u64;

            if let (Some(cb), Some(total)) = (&progress, content_length) {
                cb((downloaded as f32 / total as f32).min(0.99));
            }
        }

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

        if let Some(cb) = progress {
            cb(1.0);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_paths() {
        let mgr = MlModelManager::with_cache_dir("/tmp/test-ml".into());
        assert!(mgr.model_path(MlModelType::EffNetEmbedding).to_str().unwrap().contains("discogs-effnet"));
        assert!(mgr.model_path(MlModelType::GenreDiscogs400).to_str().unwrap().contains("genre_discogs400"));
    }

    #[test]
    fn test_base_models_list() {
        assert_eq!(MlModelType::base_models().len(), 2);
        assert_eq!(MlModelType::experimental_models().len(), 1);
    }
}
