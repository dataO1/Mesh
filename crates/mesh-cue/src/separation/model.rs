//! Model management for stem separation
//!
//! Handles downloading, caching, and locating ONNX models for audio separation.
//! Models are downloaded on first use from Hugging Face and cached locally.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use super::config::ModelType;
use super::error::{Result, SeparationError};

/// Manages model downloads and caching
pub struct ModelManager {
    /// Directory where models are cached
    cache_dir: PathBuf,
}

impl ModelManager {
    /// Create a new ModelManager with the default cache directory
    ///
    /// Default location: `~/.cache/mesh-cue/models/`
    pub fn new() -> Result<Self> {
        let cache_dir = Self::default_cache_dir()?;
        Ok(Self { cache_dir })
    }

    /// Create a ModelManager with a custom cache directory
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Get the default cache directory
    fn default_cache_dir() -> Result<PathBuf> {
        let base = dirs::cache_dir().ok_or_else(|| {
            SeparationError::InvalidConfig("Could not determine cache directory".to_string())
        })?;
        Ok(base.join("mesh-cue").join("models"))
    }

    /// Get the path to a model, downloading if necessary
    ///
    /// Downloads both the .onnx file and accompanying .onnx.data file if the model
    /// uses external data storage (most large models do).
    ///
    /// # Arguments
    /// * `model` - The model type to get
    /// * `progress` - Optional progress callback (0.0 to 1.0)
    ///
    /// # Returns
    /// Path to the model file (.onnx)
    pub fn ensure_model(
        &self,
        model: ModelType,
        progress: Option<Box<dyn Fn(f32) + Send>>,
    ) -> Result<PathBuf> {
        let model_path = self.model_path(model);
        let data_path = self.data_path(model);
        let needs_data = model.has_external_data();

        // Check if all required files exist
        let model_exists = model_path.exists();
        let data_exists = !needs_data || data_path.exists();

        if model_exists && data_exists {
            log::info!("Model {} found at {:?}", model.display_name(), model_path);
            if let Some(cb) = &progress {
                cb(1.0);
            }
            return Ok(model_path);
        }

        // Determine what needs to be downloaded
        let download_model = !model_exists;
        let download_data = needs_data && !data_exists;

        // Download the .onnx file if needed (small, ~2-5MB)
        if download_model {
            log::info!(
                "Downloading model {} from {}",
                model.display_name(),
                model.download_url()
            );
            // For .onnx file, don't report progress (it's fast)
            self.download_file(model.download_url(), &model_path, None)?;

            // Report 2% progress after .onnx download
            if let Some(ref cb) = progress {
                if download_data {
                    cb(0.02);
                }
            }
        }

        // Download the .data file if needed (large, ~160MB)
        if download_data {
            log::info!(
                "Downloading model data from {}",
                model.data_download_url()
            );
            // Pass progress directly for the large .data file
            // Scale 0-100% of data download to 2-100% overall
            let data_progress: Option<Box<dyn Fn(f32) + Send>> = if download_model {
                // Both files: .data progress maps to 2%-100%
                progress.map(|cb| {
                    Box::new(move |p: f32| cb(0.02 + p * 0.98)) as Box<dyn Fn(f32) + Send>
                })
            } else {
                // Only .data file: full 0-100%
                progress
            };
            self.download_file(model.data_download_url(), &data_path, data_progress)?;
        } else if let Some(ref cb) = progress {
            cb(1.0);
        }

        Ok(model_path)
    }

    /// Get the local path for the external data file
    pub fn data_path(&self, model: ModelType) -> PathBuf {
        self.cache_dir.join(model.data_filename())
    }

    /// Get the local path where a model would be stored
    pub fn model_path(&self, model: ModelType) -> PathBuf {
        self.cache_dir.join(model.filename())
    }

    /// Check if a model is already downloaded (including external data file)
    pub fn is_model_available(&self, model: ModelType) -> bool {
        let model_exists = self.model_path(model).exists();
        let data_exists = !model.has_external_data() || self.data_path(model).exists();
        model_exists && data_exists
    }

    /// Download a file from a URL to the cache directory
    fn download_file(
        &self,
        url: &str,
        target_path: &std::path::Path,
        progress: Option<Box<dyn Fn(f32) + Send>>,
    ) -> Result<()> {
        // Ensure cache directory exists
        fs::create_dir_all(&self.cache_dir).map_err(SeparationError::Io)?;

        // Create temp file with unique suffix
        let temp_path = target_path.with_extension("tmp");

        log::info!("Downloading {} to {:?}", url, target_path);

        // Use ureq for HTTP requests (blocking, simple)
        let response = ureq::get(url)
            .call()
            .map_err(|e| SeparationError::ModelDownloadFailed(e.to_string()))?;

        // Get content length for progress
        let content_length: Option<u64> = response
            .header("Content-Length")
            .and_then(|s| s.parse().ok());

        // Create temp file
        let mut file = fs::File::create(&temp_path).map_err(SeparationError::Io)?;

        // Read response body with progress updates
        let mut reader = response.into_reader();
        let mut buffer = [0u8; 8192];
        let mut downloaded: u64 = 0;

        loop {
            let bytes_read = reader.read(&mut buffer).map_err(SeparationError::Io)?;
            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read])
                .map_err(SeparationError::Io)?;

            downloaded += bytes_read as u64;

            // Report progress
            if let (Some(cb), Some(total)) = (&progress, content_length) {
                let pct = downloaded as f32 / total as f32;
                cb(pct.min(0.99)); // Cap at 99% until verification
            }
        }

        file.flush().map_err(SeparationError::Io)?;
        drop(file);

        // Verify download size
        let actual_size = fs::metadata(&temp_path)
            .map_err(SeparationError::Io)?
            .len();

        if let Some(expected) = content_length {
            if actual_size != expected {
                fs::remove_file(&temp_path).ok();
                return Err(SeparationError::ModelDownloadFailed(format!(
                    "Download incomplete: expected {} bytes, got {}",
                    expected, actual_size
                )));
            }
        }

        // Rename temp to final
        fs::rename(&temp_path, target_path).map_err(SeparationError::Io)?;

        log::info!(
            "Successfully downloaded {:?} ({} bytes)",
            target_path.file_name().unwrap_or_default(),
            actual_size
        );

        if let Some(cb) = progress {
            cb(1.0);
        }

        Ok(())
    }

    /// Delete a cached model (including external data file)
    pub fn delete_model(&self, model: ModelType) -> Result<()> {
        let model_path = self.model_path(model);
        if model_path.exists() {
            fs::remove_file(&model_path).map_err(SeparationError::Io)?;
            log::info!("Deleted cached model: {:?}", model_path);
        }

        // Also delete external data file if it exists
        if model.has_external_data() {
            let data_path = self.data_path(model);
            if data_path.exists() {
                fs::remove_file(&data_path).map_err(SeparationError::Io)?;
                log::info!("Deleted model data: {:?}", data_path);
            }
        }
        Ok(())
    }

    /// Get total size of all cached models (including external data files)
    pub fn cache_size(&self) -> u64 {
        ModelType::all()
            .iter()
            .map(|model| {
                let model_size = fs::metadata(self.model_path(*model))
                    .ok()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let data_size = if model.has_external_data() {
                    fs::metadata(self.data_path(*model))
                        .ok()
                        .map(|m| m.len())
                        .unwrap_or(0)
                } else {
                    0
                };
                model_size + data_size
            })
            .sum()
    }

    /// Clear all cached models
    pub fn clear_cache(&self) -> Result<()> {
        for model in ModelType::all() {
            self.delete_model(*model)?;
        }
        Ok(())
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new().expect("Failed to create ModelManager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn test_model_path() {
        let cache_dir = temp_dir().join("mesh-test-models");
        let manager = ModelManager::with_cache_dir(cache_dir.clone());

        let path = manager.model_path(ModelType::Demucs4Stems);
        assert_eq!(path, cache_dir.join("htdemucs.onnx"));
    }

    #[test]
    fn test_is_model_available_false() {
        let cache_dir = temp_dir().join("mesh-test-models-nonexistent");
        let manager = ModelManager::with_cache_dir(cache_dir);

        assert!(!manager.is_model_available(ModelType::Demucs4Stems));
    }
}
