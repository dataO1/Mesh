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
/// The EffNet model produces both 1280-dim embeddings AND 400-class genre
/// predictions in a single forward pass, so no separate genre head is needed.
/// (The Essentia hub only has the genre head as TensorFlow `.pb`, not ONNX.)
///
/// Binary mood classifiers consume EffNet's 1280-dim embeddings and output
/// 2-class softmax probabilities. The positive class index varies per model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlModelType {
    /// EffNet model (~17 MB) — always required
    /// Outputs: [0] genre predictions [n,400], [1] embeddings [n,1280]
    EffNetEmbedding,
    /// Jamendo mood/theme classification head (~2.7 MB)
    /// 56-class sigmoid output over mood/theme tags
    JamendoMood,
    /// Voice/Instrumental classifier (~502 KB) — positive class ("voice") at index 1
    /// Classes: [instrumental, voice] — 2-class softmax on EffNet embeddings
    VoiceInstrumental,
    /// Timbre brightness (~501 KB) — classes: [bright, dark]
    /// Bright probability at index 0
    Timbre,
    /// Tonal/Atonal (~502 KB) — classes: [atonal, tonal]
    /// Tonal probability at index 1
    TonalAtonal,
    /// Mood: Acoustic (~502 KB) — classes: [acoustic, non_acoustic]
    /// Acoustic probability at index 0
    MoodAcoustic,
    /// Mood: Electronic (~502 KB) — classes: [electronic, non_electronic]
    /// Electronic probability at index 0
    MoodElectronic,
    /// Danceability (~502 KB) — classes: [danceable, not_danceable]
    /// Danceable probability at index 0
    Danceability,
    /// Approachability regression (~502 KB) — continuous output [0,1]
    /// Single float regression, NOT softmax
    ApproachabilityRegression,
    /// NSynth Reverb (~502 KB) — classes: [wet, dry]
    /// Wet (reverberant) probability at index 0
    /// NOTE: Requires .pb→.onnx conversion via `nix run .#convert-reverb-model`
    NsynthReverb,
}

impl MlModelType {
    /// Filename for caching
    pub fn filename(&self) -> &'static str {
        match self {
            // bsdynamic = dynamic batch size ONNX variant (bs64 is TF-only)
            MlModelType::EffNetEmbedding => "discogs-effnet-bsdynamic-1.onnx",
            MlModelType::JamendoMood => "mtg_jamendo_moodtheme-discogs-effnet-1.onnx",
            MlModelType::VoiceInstrumental => "voice_instrumental-discogs-effnet-1.onnx",
            MlModelType::Timbre => "timbre-discogs-effnet-1.onnx",
            MlModelType::TonalAtonal => "tonal_atonal-discogs-effnet-1.onnx",
            MlModelType::MoodAcoustic => "mood_acoustic-discogs-effnet-1.onnx",
            MlModelType::MoodElectronic => "mood_electronic-discogs-effnet-1.onnx",
            MlModelType::Danceability => "danceability-discogs-effnet-1.onnx",
            MlModelType::ApproachabilityRegression => "approachability_regression-discogs-effnet-1.onnx",
            MlModelType::NsynthReverb => "nsynth_reverb-discogs-effnet-1.onnx",
        }
    }

    /// Download URL from GitHub releases (mirrored from Essentia's model hub)
    pub fn download_url(&self) -> &'static str {
        match self {
            MlModelType::EffNetEmbedding => "https://github.com/dataO1/Mesh/releases/download/models/discogs-effnet-bsdynamic-1.onnx",
            MlModelType::JamendoMood => "https://github.com/dataO1/Mesh/releases/download/models/mtg_jamendo_moodtheme-discogs-effnet-1.onnx",
            MlModelType::VoiceInstrumental => "https://essentia.upf.edu/models/classification-heads/voice_instrumental/voice_instrumental-discogs-effnet-1.onnx",
            MlModelType::Timbre => "https://essentia.upf.edu/models/classification-heads/timbre/timbre-discogs-effnet-1.onnx",
            MlModelType::TonalAtonal => "https://essentia.upf.edu/models/classification-heads/tonal_atonal/tonal_atonal-discogs-effnet-1.onnx",
            MlModelType::MoodAcoustic => "https://essentia.upf.edu/models/classification-heads/mood_acoustic/mood_acoustic-discogs-effnet-1.onnx",
            MlModelType::MoodElectronic => "https://essentia.upf.edu/models/classification-heads/mood_electronic/mood_electronic-discogs-effnet-1.onnx",
            MlModelType::Danceability => "https://essentia.upf.edu/models/classification-heads/danceability/danceability-discogs-effnet-1.onnx",
            MlModelType::ApproachabilityRegression => "https://essentia.upf.edu/models/classification-heads/approachability/approachability_regression-discogs-effnet-1.onnx",
            // NSynth Reverb needs .pb→.onnx conversion; hosted on our GitHub releases
            MlModelType::NsynthReverb => "https://github.com/dataO1/Mesh/releases/download/models/nsynth_reverb-discogs-effnet-1.onnx",
        }
    }

    /// Human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            MlModelType::EffNetEmbedding => "EffNet (Genre + Embedding)",
            MlModelType::JamendoMood => "Jamendo Mood/Theme",
            MlModelType::VoiceInstrumental => "Voice/Instrumental",
            MlModelType::Timbre => "Timbre (Bright/Dark)",
            MlModelType::TonalAtonal => "Tonal/Atonal",
            MlModelType::MoodAcoustic => "Acoustic/Non-Acoustic",
            MlModelType::MoodElectronic => "Electronic/Non-Electronic",
            MlModelType::Danceability => "Danceability",
            MlModelType::ApproachabilityRegression => "Approachability",
            MlModelType::NsynthReverb => "Reverb (Wet/Dry)",
        }
    }

    /// Positive class index in the 2-class softmax output.
    ///
    /// Returns `None` for models that aren't binary classifiers.
    /// The index varies per model due to different training class orderings.
    /// Models required for ML analysis (genre + mood + voice + audio characteristics)
    pub fn base_models() -> &'static [MlModelType] {
        &[
            MlModelType::EffNetEmbedding,
            MlModelType::JamendoMood,
            MlModelType::VoiceInstrumental,
            MlModelType::Timbre,
            MlModelType::TonalAtonal,
            MlModelType::MoodAcoustic,
            MlModelType::MoodElectronic,
            MlModelType::Danceability,
            MlModelType::ApproachabilityRegression,
            MlModelType::NsynthReverb,
        ]
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
        assert!(mgr.model_path(MlModelType::JamendoMood).to_str().unwrap().contains("mtg_jamendo"));
    }

    #[test]
    fn test_base_models_list() {
        assert_eq!(MlModelType::base_models().len(), 10);
    }
}
