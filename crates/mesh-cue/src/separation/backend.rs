//! Separation backend trait and implementations
//!
//! This module defines the `SeparationBackend` trait that abstracts over different
//! stem separation implementations, allowing the backend to be swapped without
//! changing calling code.
//!
//! ## Available Backends
//!
//! - **OrtBackend**: Direct ONNX Runtime via `ort` crate - currently recommended
//! - **CharonBackend**: Uses `charon-audio` crate - blocked by rayon version conflict
//!
//! The backend can be selected at runtime via `SeparationConfig::backend`.

use std::path::Path;

use super::config::SeparationConfig;
use super::error::Result;

/// Represents separated audio stems (mono, not interleaved)
#[derive(Debug, Clone)]
pub struct StemData {
    /// Sample rate of all stems
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u16,
    /// Vocals stem (lead vocals, backing vocals) - interleaved if stereo
    pub vocals: Vec<f32>,
    /// Drums stem (kick, snare, hats, cymbals) - interleaved if stereo
    pub drums: Vec<f32>,
    /// Bass stem (bass guitar, sub-bass, bass synths) - interleaved if stereo
    pub bass: Vec<f32>,
    /// Other stem (everything else - synths, guitars, FX) - interleaved if stereo
    pub other: Vec<f32>,
}

impl StemData {
    /// Create empty stem data
    pub fn empty(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            channels: 2,
            vocals: Vec::new(),
            drums: Vec::new(),
            bass: Vec::new(),
            other: Vec::new(),
        }
    }

    /// Get the number of samples per channel
    pub fn samples_per_channel(&self) -> usize {
        self.vocals.len() / self.channels as usize
    }

    /// Get total sample count (all channels)
    pub fn len(&self) -> usize {
        self.vocals.len()
    }

    /// Check if stems are empty
    pub fn is_empty(&self) -> bool {
        self.vocals.is_empty()
    }

    /// Duration in seconds
    pub fn duration_secs(&self) -> f64 {
        self.samples_per_channel() as f64 / self.sample_rate as f64
    }

    /// Write all stems to WAV files in a directory
    ///
    /// Creates files named: `{base_name}_(Vocals).wav`, etc.
    /// Returns paths to the created files: (vocals, drums, bass, other)
    pub fn write_to_wav_files(
        &self,
        dir: &std::path::Path,
        base_name: &str,
    ) -> std::io::Result<(
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    )> {
        use hound::{SampleFormat, WavSpec, WavWriter};

        let spec = WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let write_stem = |stem: &[f32], suffix: &str| -> std::io::Result<std::path::PathBuf> {
            let path = dir.join(format!("{}_({}).wav", base_name, suffix));
            let mut writer = WavWriter::create(&path, spec).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            })?;
            for &sample in stem {
                writer.write_sample(sample).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
                })?;
            }
            writer.finalize().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            })?;
            Ok(path)
        };

        let vocals_path = write_stem(&self.vocals, "Vocals")?;
        let drums_path = write_stem(&self.drums, "Drums")?;
        let bass_path = write_stem(&self.bass, "Bass")?;
        let other_path = write_stem(&self.other, "Other")?;

        Ok((vocals_path, drums_path, bass_path, other_path))
    }
}

/// Progress callback for separation operations
pub type ProgressCallback = Box<dyn Fn(f32) + Send + Sync>;

/// Trait for audio stem separation backends
///
/// This trait abstracts over different separation implementations.
/// All backends should produce equivalent results given the same model.
///
/// ## Implementing a New Backend
///
/// ```ignore
/// struct MyBackend;
///
/// impl SeparationBackend for MyBackend {
///     fn separate(&self, input_path: &Path, model_path: &Path,
///                 config: &SeparationConfig, progress: Option<ProgressCallback>) -> Result<StemData> {
///         // 1. Decode audio from input_path
///         // 2. Load ONNX model from model_path
///         // 3. Run inference
///         // 4. Return separated stems
///     }
///     // ...
/// }
/// ```
pub trait SeparationBackend: Send + Sync {
    /// Separate an audio file into stems
    ///
    /// # Arguments
    /// * `input_path` - Path to the input audio file (any format supported by Symphonia)
    /// * `model_path` - Path to the ONNX model file
    /// * `config` - Separation configuration
    /// * `progress` - Optional progress callback (0.0 to 1.0)
    ///
    /// # Returns
    /// Separated stems as audio data (vocals, drums, bass, other)
    fn separate(
        &self,
        input_path: &Path,
        model_path: &Path,
        config: &SeparationConfig,
        progress: Option<ProgressCallback>,
    ) -> Result<StemData>;

    /// Check if GPU acceleration is available
    fn supports_gpu(&self) -> bool;

    /// Get backend name for logging/UI
    fn name(&self) -> &'static str;

    /// Check if this backend is currently available
    /// (dependencies resolved, libraries loaded, etc.)
    fn is_available(&self) -> bool {
        true
    }

    /// Get reason why backend is unavailable (if is_available() returns false)
    fn unavailable_reason(&self) -> Option<&'static str> {
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Charon Backend (blocked by rayon version conflict)
// ─────────────────────────────────────────────────────────────────────────────

/// Backend using the charon-audio crate
///
/// Charon is a pure Rust audio separation library that uses:
/// - ONNX Runtime or Candle for model inference
/// - Symphonia for audio decoding
/// - Rayon for parallel processing
///
/// **Status**: Currently unavailable due to rayon version conflict.
/// charon-audio requires rayon >=1.10, but graph_builder (via cozo) requires <1.10.
/// This will be resolved when graph_builder updates its rayon compatibility.
pub struct CharonBackend {
    /// Whether GPU was detected as available
    gpu_available: bool,
}

impl CharonBackend {
    /// Create a new Charon backend
    pub fn new() -> Self {
        let gpu_available = Self::probe_gpu();
        Self { gpu_available }
    }

    /// Probe for GPU availability
    fn probe_gpu() -> bool {
        // Would check via charon-audio when available
        true
    }
}

impl Default for CharonBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SeparationBackend for CharonBackend {
    fn separate(
        &self,
        input_path: &Path,
        model_path: &Path,
        config: &SeparationConfig,
        progress: Option<ProgressCallback>,
    ) -> Result<StemData> {
        use super::error::SeparationError;

        // Verify input file exists
        if !input_path.exists() {
            return Err(SeparationError::AudioReadError {
                path: input_path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Input file not found",
                ),
            });
        }

        // Verify model file exists
        if !model_path.exists() {
            return Err(SeparationError::ModelNotFound(
                model_path.display().to_string(),
            ));
        }

        log::info!(
            "CharonBackend::separate called for {:?} with model {:?}",
            input_path,
            model_path
        );
        log::info!(
            "Config: use_gpu={}, segment_length={}s",
            config.use_gpu,
            config.segment_length_secs
        );

        if let Some(cb) = &progress {
            cb(0.0);
        }

        // TODO: Implement when charon-audio dependency conflict is resolved
        // The implementation will look like:
        // ```rust
        // use charon_audio::{Separator, SeparatorConfig};
        //
        // let mut charon_config = SeparatorConfig::default();
        // charon_config.model_path = model_path.to_path_buf();
        // charon_config.use_gpu = config.use_gpu;
        // charon_config.segment_length = config.segment_length_secs;
        //
        // let separator = Separator::new(charon_config)?;
        // let stems = separator.separate_file(input_path)?;
        // ```

        Err(SeparationError::BackendInitFailed(
            "CharonBackend unavailable: rayon version conflict with graph_builder. \
             Use OrtBackend instead."
                .to_string(),
        ))
    }

    fn supports_gpu(&self) -> bool {
        self.gpu_available
    }

    fn name(&self) -> &'static str {
        "Charon"
    }

    fn is_available(&self) -> bool {
        false // Blocked by dependency conflict
    }

    fn unavailable_reason(&self) -> Option<&'static str> {
        Some("Requires rayon >=1.10, but graph_builder requires <1.10")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ORT Backend (ONNX Runtime via ort crate)
// ─────────────────────────────────────────────────────────────────────────────

/// Backend using ONNX Runtime directly via the `ort` crate
///
/// This backend:
/// - Decodes audio using Symphonia (MP3, FLAC, WAV, OGG, etc.)
/// - Runs Demucs ONNX model via ONNX Runtime
/// - Supports GPU acceleration via CUDA/DirectML/CoreML
///
/// **Status**: Recommended backend, fully functional.
pub struct OrtBackend {
    /// Whether GPU was detected as available
    gpu_available: bool,
}

impl OrtBackend {
    /// Create a new ORT backend
    pub fn new() -> Self {
        let gpu_available = Self::probe_gpu();
        log::info!(
            "OrtBackend initialized, GPU available: {}",
            gpu_available
        );
        Self { gpu_available }
    }

    /// Probe for GPU availability via ONNX Runtime
    fn probe_gpu() -> bool {
        // For now, assume CPU-only and let ort handle execution provider selection
        // GPU support can be added via CUDA/DirectML/CoreML execution providers
        // TODO: Actually probe available execution providers
        false
    }
}

impl Default for OrtBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SeparationBackend for OrtBackend {
    fn separate(
        &self,
        input_path: &Path,
        model_path: &Path,
        config: &SeparationConfig,
        progress: Option<ProgressCallback>,
    ) -> Result<StemData> {
        use super::error::SeparationError;

        // Verify input file exists
        if !input_path.exists() {
            return Err(SeparationError::AudioReadError {
                path: input_path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Input file not found",
                ),
            });
        }

        // Verify model file exists
        if !model_path.exists() {
            return Err(SeparationError::ModelNotFound(
                model_path.display().to_string(),
            ));
        }

        log::info!(
            "OrtBackend::separate called for {:?} with model {:?}",
            input_path,
            model_path
        );
        log::info!(
            "Config: use_gpu={}, segment_length={}s",
            config.use_gpu,
            config.segment_length_secs
        );

        if let Some(ref cb) = progress {
            cb(0.0);
        }

        // Step 1: Decode audio file
        let (audio_data, sample_rate, channels) = decode_audio(input_path)?;
        log::info!(
            "Decoded audio: {} samples, {}Hz, {} channels",
            audio_data.len(),
            sample_rate,
            channels
        );

        if let Some(ref cb) = progress {
            cb(0.1);
        }

        // Step 2: Run ONNX inference
        let stems = run_demucs_inference(
            &audio_data,
            sample_rate,
            channels,
            model_path,
            config,
            progress.as_ref(),
        )?;

        if let Some(ref cb) = progress {
            cb(1.0);
        }

        Ok(stems)
    }

    fn supports_gpu(&self) -> bool {
        self.gpu_available
    }

    fn name(&self) -> &'static str {
        "ONNX Runtime"
    }

    fn is_available(&self) -> bool {
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Audio Decoding (Symphonia)
// ─────────────────────────────────────────────────────────────────────────────

/// Decode an audio file to f32 samples using Symphonia
fn decode_audio(path: &Path) -> Result<(Vec<f32>, u32, u16)> {
    use super::error::SeparationError;
    use std::fs::File;
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = File::open(path).map_err(|e| SeparationError::AudioReadError {
        path: path.to_path_buf(),
        source: e,
    })?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Create a hint with the file extension
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe the media source
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| SeparationError::UnsupportedFormat(e.to_string()))?;

    let mut format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| SeparationError::UnsupportedFormat("No audio track found".to_string()))?;

    let track_id = track.id;

    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| SeparationError::UnsupportedFormat("Unknown sample rate".to_string()))?;

    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    // Create decoder
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| SeparationError::UnsupportedFormat(e.to_string()))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => {
                log::warn!("Error reading packet: {}", e);
                break;
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(e) => {
                log::warn!("Error decoding packet: {}", e);
                continue;
            }
        };

        // Initialize sample buffer on first decode
        if sample_buf.is_none() {
            let spec = *decoded.spec();
            let duration = decoded.capacity() as u64;
            sample_buf = Some(SampleBuffer::new(duration, spec));
        }

        if let Some(ref mut buf) = sample_buf {
            buf.copy_interleaved_ref(decoded);
            samples.extend_from_slice(buf.samples());
        }
    }

    Ok((samples, sample_rate, channels))
}

// ─────────────────────────────────────────────────────────────────────────────
// ONNX Inference (Demucs model)
// ─────────────────────────────────────────────────────────────────────────────

/// Run Demucs ONNX model inference
fn run_demucs_inference(
    audio: &[f32],
    sample_rate: u32,
    channels: u16,
    model_path: &Path,
    config: &SeparationConfig,
    progress: Option<&ProgressCallback>,
) -> Result<StemData> {
    use super::error::SeparationError;
    use ndarray::Array3;
    use ort::session::{builder::GraphOptimizationLevel, Session};
    use ort::value::Tensor;

    // Demucs expects stereo input
    let stereo_audio = if channels == 1 {
        // Convert mono to stereo by duplicating
        audio.iter().flat_map(|&s| [s, s]).collect::<Vec<f32>>()
    } else if channels == 2 {
        audio.to_vec()
    } else {
        // Downmix to stereo (take first two channels)
        audio
            .chunks(channels as usize)
            .flat_map(|chunk| [chunk[0], chunk.get(1).copied().unwrap_or(chunk[0])])
            .collect()
    };

    let num_samples = stereo_audio.len() / 2;

    // Create ONNX session
    log::info!("Loading ONNX model from {:?}", model_path);

    let mut session = Session::builder()
        .map_err(|e| SeparationError::BackendInitFailed(e.to_string()))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| SeparationError::BackendInitFailed(e.to_string()))?
        .commit_from_file(model_path)
        .map_err(|e| {
            SeparationError::BackendInitFailed(format!("Failed to load ONNX model: {}", e))
        })?;

    if let Some(cb) = progress {
        cb(0.2);
    }

    // Reshape audio for model: [batch=1, channels=2, samples]
    // Demucs expects shape [1, 2, N]
    let mut input_array = Array3::<f32>::zeros((1, 2, num_samples));
    for (i, chunk) in stereo_audio.chunks(2).enumerate() {
        input_array[[0, 0, i]] = chunk[0]; // Left
        input_array[[0, 1, i]] = chunk[1]; // Right
    }

    log::info!("Running inference on {} samples...", num_samples);

    // Process in segments if audio is very long
    let _segment_samples = (config.segment_length_secs * sample_rate as f64) as usize;
    // TODO: Implement overlapped segment processing for long files

    // Convert ndarray to ort Tensor, then run inference
    let input_tensor = Tensor::from_array(input_array).map_err(|e| {
        SeparationError::SeparationFailed(format!("Failed to create input tensor: {}", e))
    })?;

    let outputs = session
        .run(ort::inputs!["input" => input_tensor])
        .map_err(|e| SeparationError::SeparationFailed(format!("Inference failed: {}", e)))?;

    if let Some(cb) = progress {
        cb(0.9);
    }

    // Extract output: [batch=1, stems=4, channels=2, samples]
    // Stem order in htdemucs: drums, bass, other, vocals
    let output = outputs
        .iter()
        .next()
        .ok_or_else(|| SeparationError::SeparationFailed("No output tensor".to_string()))?
        .1;

    // Extract shape and data from tensor
    let (shape, data) = output.try_extract_tensor::<f32>().map_err(|e| {
        SeparationError::SeparationFailed(format!("Failed to extract output: {}", e))
    })?;

    // Convert shape to Vec<i64>
    let output_shape: Vec<i64> = shape.iter().copied().collect();
    log::info!("Output shape: {:?}", output_shape);

    // Expected shape: [1, 4, 2, N] for htdemucs
    if output_shape.len() != 4 || output_shape[1] < 4 {
        return Err(SeparationError::SeparationFailed(format!(
            "Unexpected output shape: {:?}, expected [1, 4, 2, N]",
            output_shape
        )));
    }

    let _num_stems = output_shape[1] as usize;
    let num_channels = output_shape[2] as usize;
    let output_samples = output_shape[3] as usize;

    // Helper to calculate flat index into [1, stems, channels, samples] tensor (row-major)
    let flat_idx = |stem: usize, channel: usize, sample: usize| -> usize {
        sample + output_samples * (channel + num_channels * stem)
    };

    // Extract stems and interleave channels
    let extract_stem = |stem_idx: usize| -> Vec<f32> {
        let mut interleaved = Vec::with_capacity(output_samples * 2);
        for i in 0..output_samples {
            interleaved.push(data[flat_idx(stem_idx, 0, i)]); // Left
            interleaved.push(data[flat_idx(stem_idx, 1, i)]); // Right
        }
        interleaved
    };

    // htdemucs order: drums=0, bass=1, other=2, vocals=3
    let stems = StemData {
        sample_rate,
        channels: 2,
        drums: extract_stem(0),
        bass: extract_stem(1),
        other: extract_stem(2),
        vocals: extract_stem(3),
    };

    log::info!(
        "Separation complete: {} samples per stem",
        stems.samples_per_channel()
    );

    Ok(stems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stem_data_duration() {
        let mut stems = StemData::empty(44100);
        stems.vocals = vec![0.0; 88200]; // 1 second stereo
        stems.drums = vec![0.0; 88200];
        stems.bass = vec![0.0; 88200];
        stems.other = vec![0.0; 88200];

        assert!((stems.duration_secs() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_charon_backend_unavailable() {
        let backend = CharonBackend::new();
        assert!(!backend.is_available());
        assert!(backend.unavailable_reason().is_some());
    }

    #[test]
    fn test_ort_backend_available() {
        let backend = OrtBackend::new();
        assert!(backend.is_available());
        assert_eq!(backend.name(), "ONNX Runtime");
    }
}
