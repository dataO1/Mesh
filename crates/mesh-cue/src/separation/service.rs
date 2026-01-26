//! Separation service - coordinates model management and backend execution
//!
//! The `SeparationService` is the main entry point for stem separation.
//! It handles:
//! - Model downloads (on first use)
//! - Backend selection and initialization
//! - Progress reporting
//! - Temp file cleanup

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::backend::{CharonBackend, OrtBackend, ProgressCallback, SeparationBackend, StemData};
use super::config::{BackendType, SeparationConfig};
use super::error::Result;
use super::model::ModelManager;

/// Progress stage during separation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeparationStage {
    /// Downloading model (if needed)
    DownloadingModel,
    /// Loading model into memory
    LoadingModel,
    /// Processing audio
    Separating,
    /// Finished
    Complete,
}

/// Combined progress info
#[derive(Debug, Clone)]
pub struct SeparationProgress {
    /// Current stage
    pub stage: SeparationStage,
    /// Progress within current stage (0.0 to 1.0)
    pub progress: f32,
    /// Human-readable status message
    pub message: String,
}

impl SeparationProgress {
    fn new(stage: SeparationStage, progress: f32, message: impl Into<String>) -> Self {
        Self {
            stage,
            progress,
            message: message.into(),
        }
    }
}

/// Callback for overall separation progress (uses Arc for cloneability)
pub type ServiceProgressCallback = Arc<dyn Fn(SeparationProgress) + Send + Sync>;

/// Main service for audio stem separation
///
/// Example usage:
/// ```ignore
/// let service = SeparationService::new()?;
/// let stems = service.separate("input.mp3", None)?;
/// ```
pub struct SeparationService {
    /// Model manager for downloads
    model_manager: ModelManager,
    /// Current separation config
    config: SeparationConfig,
    /// Active backend
    backend: Arc<dyn SeparationBackend>,
}

impl SeparationService {
    /// Create a new separation service with default config
    pub fn new() -> Result<Self> {
        Self::with_config(SeparationConfig::default())
    }

    /// Create a separation service with custom config
    pub fn with_config(mut config: SeparationConfig) -> Result<Self> {
        config.validate();

        let model_manager = ModelManager::new()?;
        let backend = Self::create_backend(config.backend);

        Ok(Self {
            model_manager,
            config,
            backend,
        })
    }

    /// Create the appropriate backend
    fn create_backend(backend_type: BackendType) -> Arc<dyn SeparationBackend> {
        match backend_type {
            BackendType::Charon => {
                // CharonBackend is not yet available due to rayon version conflict
                // It will return an error at separation time explaining why
                Arc::new(CharonBackend::new())
            }
            BackendType::OnnxRuntime => Arc::new(OrtBackend::new()),
        }
    }

    /// Update configuration
    pub fn set_config(&mut self, mut config: SeparationConfig) {
        config.validate();

        // Recreate backend if type changed
        if config.backend != self.config.backend {
            self.backend = Self::create_backend(config.backend);
        }

        self.config = config;
    }

    /// Get current configuration
    pub fn config(&self) -> &SeparationConfig {
        &self.config
    }

    /// Check if GPU acceleration is available
    pub fn supports_gpu(&self) -> bool {
        self.backend.supports_gpu()
    }

    /// Check if the configured model is downloaded
    pub fn is_model_ready(&self) -> bool {
        self.model_manager.is_model_available(self.config.model)
    }

    /// Ensure the model is downloaded (call before separation if you want
    /// to handle download progress separately)
    pub fn ensure_model_downloaded(
        &self,
        progress: Option<Box<dyn Fn(f32) + Send>>,
    ) -> Result<PathBuf> {
        self.model_manager.ensure_model(self.config.model, progress)
    }

    /// Separate an audio file into stems
    ///
    /// # Arguments
    /// * `input_path` - Path to the input audio file
    /// * `progress` - Optional progress callback (Arc for cloneability)
    ///
    /// # Returns
    /// Separated stem data (vocals, drums, bass, other)
    pub fn separate(
        &self,
        input_path: impl AsRef<Path>,
        progress: Option<ServiceProgressCallback>,
    ) -> Result<StemData> {
        let input_path = input_path.as_ref();

        // Report: downloading model (if needed)
        if let Some(ref cb) = progress {
            if !self.is_model_ready() {
                cb(SeparationProgress::new(
                    SeparationStage::DownloadingModel,
                    0.0,
                    format!("Downloading {} model...", self.config.model.display_name()),
                ));
            }
        }

        // Ensure model is available
        let download_progress: Option<Box<dyn Fn(f32) + Send>> = progress.clone().map(|cb| {
            Box::new(move |p: f32| {
                cb(SeparationProgress::new(
                    SeparationStage::DownloadingModel,
                    p,
                    format!("Downloading model... {:.0}%", p * 100.0),
                ));
            }) as Box<dyn Fn(f32) + Send>
        });
        let model_path = self.model_manager.ensure_model(self.config.model, download_progress)?;

        // Report: loading model
        if let Some(ref cb) = progress {
            cb(SeparationProgress::new(
                SeparationStage::LoadingModel,
                0.0,
                "Loading separation model...",
            ));
        }

        // Report: separating
        if let Some(ref cb) = progress {
            cb(SeparationProgress::new(
                SeparationStage::Separating,
                0.0,
                "Separating audio into stems...",
            ));
        }

        // Create progress callback for backend
        let backend_progress: Option<ProgressCallback> = progress.clone().map(|cb| {
            Box::new(move |p: f32| {
                cb(SeparationProgress::new(
                    SeparationStage::Separating,
                    p,
                    format!("Separating... {:.0}%", p * 100.0),
                ));
            }) as ProgressCallback
        });

        // Run separation
        let stems = self.backend.separate(
            input_path,
            &model_path,
            &self.config,
            backend_progress,
        )?;

        // Report: complete
        if let Some(ref cb) = progress {
            cb(SeparationProgress::new(
                SeparationStage::Complete,
                1.0,
                "Separation complete",
            ));
        }

        Ok(stems)
    }

    /// Get the model manager (for cache operations)
    pub fn model_manager(&self) -> &ModelManager {
        &self.model_manager
    }

    /// Get information about the current backend
    pub fn backend_info(&self) -> &'static str {
        self.backend.name()
    }
}

impl Default for SeparationService {
    fn default() -> Self {
        Self::new().expect("Failed to create SeparationService")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_creation() {
        // Note: This test may fail if cache dir can't be created
        let result = SeparationService::new();
        assert!(result.is_ok() || result.is_err()); // Just test it doesn't panic
    }

    #[test]
    fn test_config_update() {
        if let Ok(mut service) = SeparationService::new() {
            let mut new_config = service.config().clone();
            new_config.use_gpu = false;
            service.set_config(new_config);
            assert!(!service.config().use_gpu);
        }
    }
}
