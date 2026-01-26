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

/// Demucs htdemucs model expects exactly this many samples per segment
/// This is segment_length (7.8 seconds) * sample_rate (44100) from the model config
const DEMUCS_SEGMENT_SAMPLES: usize = 343980;

/// Overlap between segments for smooth blending (25% of segment)
const DEMUCS_OVERLAP_SAMPLES: usize = DEMUCS_SEGMENT_SAMPLES / 4;

/// STFT parameters matching Demucs htdemucs model
const DEMUCS_NFFT: usize = 4096;
const DEMUCS_HOP_LENGTH: usize = DEMUCS_NFFT / 4; // 1024
/// Number of STFT frames for the fixed segment length
/// Calculated as: ceil(DEMUCS_SEGMENT_SAMPLES / DEMUCS_HOP_LENGTH) with proper padding
const DEMUCS_STFT_FRAMES: usize = 336;

/// Run Demucs ONNX model inference with chunked processing
fn run_demucs_inference(
    audio: &[f32],
    sample_rate: u32,
    channels: u16,
    model_path: &Path,
    _config: &SeparationConfig,
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

    // Calculate segments for chunked processing
    let step_size = DEMUCS_SEGMENT_SAMPLES - DEMUCS_OVERLAP_SAMPLES;
    let num_segments = if num_samples <= DEMUCS_SEGMENT_SAMPLES {
        1
    } else {
        (num_samples - DEMUCS_OVERLAP_SAMPLES + step_size - 1) / step_size
    };

    log::info!(
        "Processing {} samples in {} segments (segment={}, overlap={})",
        num_samples,
        num_segments,
        DEMUCS_SEGMENT_SAMPLES,
        DEMUCS_OVERLAP_SAMPLES
    );

    // Initialize output accumulators for overlap-add
    // 4 stems, each with stereo interleaved samples
    let mut stem_accum: [Vec<f32>; 4] = [
        vec![0.0; num_samples * 2],
        vec![0.0; num_samples * 2],
        vec![0.0; num_samples * 2],
        vec![0.0; num_samples * 2],
    ];
    let mut weight_accum = vec![0.0f32; num_samples];

    // Process each segment
    for seg_idx in 0..num_segments {
        let start_sample = seg_idx * step_size;
        let end_sample = (start_sample + DEMUCS_SEGMENT_SAMPLES).min(num_samples);
        let segment_len = end_sample - start_sample;

        // Extract left and right channels for this segment (with zero-padding)
        let mut left_channel = vec![0.0f32; DEMUCS_SEGMENT_SAMPLES];
        let mut right_channel = vec![0.0f32; DEMUCS_SEGMENT_SAMPLES];
        for i in 0..segment_len {
            let src_idx = start_sample + i;
            left_channel[i] = stereo_audio[src_idx * 2];
            right_channel[i] = stereo_audio[src_idx * 2 + 1];
        }

        // Create waveform input tensor [1, 2, samples]
        let mut input_array = Array3::<f32>::zeros((1, 2, DEMUCS_SEGMENT_SAMPLES));
        for i in 0..DEMUCS_SEGMENT_SAMPLES {
            input_array[[0, 0, i]] = left_channel[i];
            input_array[[0, 1, i]] = right_channel[i];
        }

        // Compute STFT spectrogram (required by Demucs hybrid model)
        let stft_array = compute_stft_for_demucs(&left_channel, &right_channel);

        // Create input tensors
        let input_tensor = Tensor::from_array(input_array).map_err(|e| {
            SeparationError::SeparationFailed(format!("Failed to create input tensor: {}", e))
        })?;
        let stft_tensor = Tensor::from_array(stft_array).map_err(|e| {
            SeparationError::SeparationFailed(format!("Failed to create STFT tensor: {}", e))
        })?;

        // Run inference with both waveform and STFT inputs
        let outputs = session
            .run(ort::inputs!["input" => input_tensor, "x" => stft_tensor])
            .map_err(|e| SeparationError::SeparationFailed(format!("Inference failed: {}", e)))?;

        // Extract time-domain output "add_67" [1, 4, 2, samples]
        // Model outputs: "output" (spectrogram) and "add_67" (waveform stems)
        let output = outputs
            .get("add_67")
            .ok_or_else(|| {
                SeparationError::SeparationFailed("Output tensor 'add_67' not found".to_string())
            })?;

        let (shape, data) = output.try_extract_tensor::<f32>().map_err(|e| {
            SeparationError::SeparationFailed(format!("Failed to extract output: {}", e))
        })?;

        // Log shape on first segment to verify expected [1, 4, 2, samples]
        if seg_idx == 0 {
            log::info!(
                "Output tensor shape: {:?}, total elements: {}",
                shape.as_ref(),
                data.len()
            );
        }

        // Overlap-add: accumulate with triangular window for smooth blending
        for i in 0..segment_len {
            let out_idx = start_sample + i;

            // Triangular window weight for overlap blending
            let weight = if i < DEMUCS_OVERLAP_SAMPLES && seg_idx > 0 {
                // Fade in at start (except first segment)
                i as f32 / DEMUCS_OVERLAP_SAMPLES as f32
            } else if i >= segment_len - DEMUCS_OVERLAP_SAMPLES && seg_idx < num_segments - 1 {
                // Fade out at end (except last segment)
                (segment_len - i) as f32 / DEMUCS_OVERLAP_SAMPLES as f32
            } else {
                1.0
            };

            weight_accum[out_idx] += weight;

            // Accumulate each stem (output shape: [1, 4, 2, samples])
            for stem in 0..4 {
                let left_idx = i + DEMUCS_SEGMENT_SAMPLES * (0 + 2 * stem);
                let right_idx = i + DEMUCS_SEGMENT_SAMPLES * (1 + 2 * stem);
                stem_accum[stem][out_idx * 2] += data[left_idx] * weight;
                stem_accum[stem][out_idx * 2 + 1] += data[right_idx] * weight;
            }
        }

        // Update progress
        if let Some(cb) = progress {
            let prog = 0.2 + 0.7 * (seg_idx + 1) as f32 / num_segments as f32;
            cb(prog);
        }
    }

    // Normalize by accumulated weights
    for i in 0..num_samples {
        let w = weight_accum[i];
        if w > 0.0 {
            for stem in 0..4 {
                stem_accum[stem][i * 2] /= w;
                stem_accum[stem][i * 2 + 1] /= w;
            }
        }
    }

    if let Some(cb) = progress {
        cb(0.95);
    }

    // htdemucs order: drums=0, bass=1, other=2, vocals=3
    let stems = StemData {
        sample_rate,
        channels: 2,
        drums: std::mem::take(&mut stem_accum[0]),
        bass: std::mem::take(&mut stem_accum[1]),
        other: std::mem::take(&mut stem_accum[2]),
        vocals: std::mem::take(&mut stem_accum[3]),
    };

    // Log RMS energy per stem for debugging stem order
    let rms = |samples: &[f32]| -> f32 {
        let sum: f32 = samples.iter().map(|s| s * s).sum();
        (sum / samples.len() as f32).sqrt()
    };
    log::info!(
        "Separation complete: {} samples per stem ({} segments processed)",
        stems.samples_per_channel(),
        num_segments
    );
    log::info!(
        "Stem RMS levels - drums: {:.4}, bass: {:.4}, other: {:.4}, vocals: {:.4}",
        rms(&stems.drums),
        rms(&stems.bass),
        rms(&stems.other),
        rms(&stems.vocals)
    );

    Ok(stems)
}

/// Compute STFT for Demucs model input
///
/// Matches the preprocessing in Demucs' `standalone_spec` and `standalone_magnitude`:
/// - n_fft = 4096, hop_length = 1024
/// - torch.stft with normalized=True, center=True, Hann window
/// - Demucs adds extra padding and crops frames
/// - Returns [batch=1, channels*2=4, freq_bins=2048, time_frames=336]
fn compute_stft_for_demucs(left: &[f32], right: &[f32]) -> ndarray::Array4<f32> {
    use ndarray::Array4;
    use realfft::RealFftPlanner;

    let n_fft = DEMUCS_NFFT; // 4096
    let hop = DEMUCS_HOP_LENGTH; // 1024
    let freq_bins = n_fft / 2; // 2048 (excludes DC and Nyquist effectively)

    // Demucs standalone_spec padding:
    // le = ceil(input_len / hop)
    // pad = hop // 2 * 3 = 1536
    // Then pads: (pad, pad + le * hop - input_len) with reflect
    // After STFT, crops to frames [2 : 2 + le] (removes first 2 and last frames)
    let input_len = left.len();
    let le = (input_len + hop - 1) / hop; // ceil division
    let demucs_pad = hop / 2 * 3; // 1536

    // torch.stft center=True adds n_fft//2 padding on each side
    let center_pad = n_fft / 2; // 2048

    // Total padding for our manual STFT (combining both)
    let total_left_pad = demucs_pad + center_pad;
    let total_right_pad = demucs_pad + le * hop - input_len + center_pad;

    // Use exactly DEMUCS_STFT_FRAMES frames (336) to match model expectation
    let target_frames = DEMUCS_STFT_FRAMES;

    // Create FFT planner
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);

    // Pre-compute periodic Hann window (matches torch.hann_window default)
    // w[n] = 0.5 * (1 - cos(2*pi*n/N)) for n in 0..N
    let window: Vec<f32> = (0..n_fft)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / n_fft as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect();

    // Normalization factor for torch.stft normalized=True
    let norm_factor = 1.0 / (n_fft as f32).sqrt();

    // Output array: [batch=1, channels*2=4, freq_bins=2048, frames=336]
    let mut stft_output = Array4::<f32>::zeros((1, 4, freq_bins, target_frames));

    // Process each channel
    for (ch_idx, channel) in [left, right].iter().enumerate() {
        // Create padded signal with reflection padding
        let padded_len = total_left_pad + input_len + total_right_pad;
        let mut padded = vec![0.0f32; padded_len];

        // Left reflection padding
        for i in 0..total_left_pad {
            let reflect_idx = total_left_pad - 1 - i;
            let src_idx = reflect_idx % (2 * input_len);
            let src_idx = if src_idx < input_len {
                src_idx
            } else {
                2 * input_len - 1 - src_idx
            };
            if src_idx < input_len {
                padded[i] = channel[src_idx];
            }
        }

        // Copy original signal
        for i in 0..input_len {
            padded[total_left_pad + i] = channel[i];
        }

        // Right reflection padding
        for i in 0..total_right_pad {
            let reflect_idx = i;
            let src_idx = input_len - 1 - (reflect_idx % input_len);
            if src_idx < input_len {
                padded[total_left_pad + input_len + i] = channel[src_idx];
            }
        }

        // Compute STFT frames
        let mut scratch = fft.make_scratch_vec();
        let mut frame_input = vec![0.0f32; n_fft];
        let mut frame_output = fft.make_output_vec();

        // Total frames before cropping
        let total_frames = (padded_len - n_fft) / hop + 1;

        // Demucs crops: z[..., 2: 2 + le] - skip first 2 frames
        let frame_offset = 2;

        for out_frame_idx in 0..target_frames {
            let frame_idx = frame_offset + out_frame_idx;
            if frame_idx >= total_frames {
                break;
            }

            let start = frame_idx * hop;

            // Extract frame with windowing
            for i in 0..n_fft {
                let sample_idx = start + i;
                frame_input[i] = if sample_idx < padded_len {
                    padded[sample_idx] * window[i]
                } else {
                    0.0
                };
            }

            // Compute FFT
            fft.process_with_scratch(&mut frame_input, &mut frame_output, &mut scratch)
                .ok();

            // Store real and imaginary parts with normalization
            // Layout: [left_real, left_imag, right_real, right_imag] in channel dimension
            let real_ch = ch_idx * 2;
            let imag_ch = ch_idx * 2 + 1;

            for freq in 0..freq_bins {
                stft_output[[0, real_ch, freq, out_frame_idx]] = frame_output[freq].re * norm_factor;
                stft_output[[0, imag_ch, freq, out_frame_idx]] = frame_output[freq].im * norm_factor;
            }
        }
    }

    stft_output
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
