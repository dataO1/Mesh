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

/// Maximum shift in samples for shift augmentation (0.5 seconds at 44.1kHz)
const DEMUCS_MAX_SHIFT: usize = 22050;

/// Run Demucs ONNX model inference with chunked processing
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

    // Number of shifts for shift augmentation (from config, clamped to 1-5)
    let num_shifts = (config.shifts as usize).clamp(1, 5);

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

    // Log shift augmentation settings
    if num_shifts > 1 {
        log::info!(
            "Shift augmentation enabled: {} shifts, max_shift={} samples ({:.2}s)",
            num_shifts,
            DEMUCS_MAX_SHIFT,
            DEMUCS_MAX_SHIFT as f64 / 44100.0
        );
    }

    // RNG for shift augmentation
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut rng_state: u64 = {
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        hasher.finish()
    };
    let next_random = |state: &mut u64| -> usize {
        // Simple LCG random number generator
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((*state >> 33) as usize) % (DEMUCS_MAX_SHIFT + 1)
    };

    // Process each segment
    for seg_idx in 0..num_segments {
        let start_sample = seg_idx * step_size;
        let end_sample = (start_sample + DEMUCS_SEGMENT_SAMPLES).min(num_samples);
        let segment_len = end_sample - start_sample;

        // For shift augmentation, we need a padded segment
        // Pad by max_shift on each side to allow shifting
        let padded_start = start_sample.saturating_sub(DEMUCS_MAX_SHIFT);
        let padded_end = (start_sample + DEMUCS_SEGMENT_SAMPLES + DEMUCS_MAX_SHIFT).min(num_samples);

        // Extract padded segment (with zero-padding at boundaries)
        let padded_len = DEMUCS_SEGMENT_SAMPLES + 2 * DEMUCS_MAX_SHIFT;
        let mut padded_left = vec![0.0f32; padded_len];
        let mut padded_right = vec![0.0f32; padded_len];

        // Calculate offset into padded buffer where actual audio starts
        let pad_offset = if start_sample < DEMUCS_MAX_SHIFT {
            DEMUCS_MAX_SHIFT - start_sample
        } else {
            0
        };

        // Copy available audio into padded buffer
        for i in padded_start..padded_end {
            let buf_idx = pad_offset + (i - padded_start);
            if buf_idx < padded_len {
                padded_left[buf_idx] = stereo_audio[i * 2];
                padded_right[buf_idx] = stereo_audio[i * 2 + 1];
            }
        }

        // Accumulator for shift-averaged results
        let combined_size = 4 * 2 * DEMUCS_SEGMENT_SAMPLES;
        let mut shift_accum = vec![0.0f32; combined_size];

        // Run inference for each shift
        for shift_idx in 0..num_shifts {
            // Generate random offset (0 for first shift if only 1 shift)
            let offset = if num_shifts == 1 {
                DEMUCS_MAX_SHIFT // No shift - use center
            } else {
                next_random(&mut rng_state)
            };

            // Extract shifted segment from padded buffer
            let shift_start = offset;
            let mut left_channel = vec![0.0f32; DEMUCS_SEGMENT_SAMPLES];
            let mut right_channel = vec![0.0f32; DEMUCS_SEGMENT_SAMPLES];
            for i in 0..DEMUCS_SEGMENT_SAMPLES {
                let src_idx = shift_start + i;
                if src_idx < padded_len {
                    left_channel[i] = padded_left[src_idx];
                    right_channel[i] = padded_right[src_idx];
                }
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

            // ═══════════════════════════════════════════════════════════════════════
            // HTDemucs HYBRID model outputs TWO branches:
            // 1. "output" - frequency branch (masked spectrogram in CaC format)
            // 2. "add_67" - time branch (direct waveform)
            // Final separation = time_branch + istft(frequency_branch)
            // Reference: sevagh/demucs.onnx model_inference.cpp
            // ═══════════════════════════════════════════════════════════════════════

            // Extract time-domain output "add_67" [1, 4, 2, samples]
            let time_output = outputs
                .get("add_67")
                .ok_or_else(|| {
                    SeparationError::SeparationFailed("Output tensor 'add_67' not found".to_string())
                })?;

            let (time_shape, time_data) = time_output.try_extract_tensor::<f32>().map_err(|e| {
                SeparationError::SeparationFailed(format!("Failed to extract time output: {}", e))
            })?;

            // Extract frequency-domain output "output" [1, 4*2*2, freq_bins, frames]
            let freq_output = outputs
                .get("output")
                .ok_or_else(|| {
                    SeparationError::SeparationFailed("Output tensor 'output' not found".to_string())
                })?;

            let (freq_shape, freq_data) = freq_output.try_extract_tensor::<f32>().map_err(|e| {
                SeparationError::SeparationFailed(format!("Failed to extract freq output: {}", e))
            })?;

            // Log shapes on first segment, first shift
            if seg_idx == 0 && shift_idx == 0 {
                log::info!(
                    "Time output shape: {:?}, Freq output shape: {:?}",
                    time_shape.as_ref(),
                    freq_shape.as_ref()
                );
            }

            // Convert frequency branch from CaC format to waveform via ISTFT
            // and combine with time branch
            let combined_stems = combine_hybrid_outputs(
                &time_data,
                &freq_data,
                freq_shape.as_ref(),
                DEMUCS_SEGMENT_SAMPLES,
            );

            // Accumulate shifted output with proper alignment
            // The output corresponds to the shifted input position. To align all
            // outputs to the "center" position (offset = MAX_SHIFT), we need to
            // skip the first (MAX_SHIFT - offset) samples of the output.
            //
            // Reference: demucs apply.py: out += shifted_out[..., max_shift - offset:]
            let align_skip = DEMUCS_MAX_SHIFT.saturating_sub(offset);
            let valid_samples = DEMUCS_SEGMENT_SAMPLES.saturating_sub(align_skip);

            // Accumulate each stem with proper alignment
            // Layout: [stems * channels * samples] where stems=4, channels=2
            for stem in 0..4 {
                for ch in 0..2 {
                    let stem_ch_offset = (stem * 2 + ch) * DEMUCS_SEGMENT_SAMPLES;
                    for i in 0..valid_samples {
                        let src_idx = stem_ch_offset + align_skip + i;
                        let dst_idx = stem_ch_offset + i;
                        if src_idx < combined_stems.len() && dst_idx < shift_accum.len() {
                            shift_accum[dst_idx] += combined_stems[src_idx];
                        }
                    }
                }
            }
        }

        // Average the accumulated shifts
        // Note: edge samples have fewer contributions, but for simplicity we divide by num_shifts
        // This may cause slight amplitude reduction at edges, but segment overlap-add compensates
        let shift_divisor = num_shifts as f32;
        for val in &mut shift_accum {
            *val /= shift_divisor;
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

            // Accumulate each stem from combined hybrid output
            // Layout: [stems * channels * samples] = [4 * 2 * segment_samples]
            for stem in 0..4 {
                let stem_offset = stem * 2 * DEMUCS_SEGMENT_SAMPLES;
                let left_idx = stem_offset + i;
                let right_idx = stem_offset + DEMUCS_SEGMENT_SAMPLES + i;
                let left_val = if left_idx < shift_accum.len() {
                    shift_accum[left_idx]
                } else {
                    0.0
                };
                let right_val = if right_idx < shift_accum.len() {
                    shift_accum[right_idx]
                } else {
                    0.0
                };
                stem_accum[stem][out_idx * 2] += left_val * weight;
                stem_accum[stem][out_idx * 2 + 1] += right_val * weight;
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

/// Combine time and frequency branch outputs from HTDemucs hybrid model
///
/// The HTDemucs model outputs:
/// - Time branch: direct waveform [1, stems, channels, samples]
/// - Frequency branch: masked spectrogram in CaC format [1, stems*channels*2, freq, frames]
///
/// Final output = time_branch + istft(frequency_branch)
///
/// Reference: sevagh/demucs.onnx model_inference.cpp
fn combine_hybrid_outputs(
    time_data: &[f32],
    freq_data: &[f32],
    freq_shape: &[i64],
    segment_samples: usize,
) -> Vec<f32> {
    use realfft::RealFftPlanner;

    let n_fft = DEMUCS_NFFT;
    let hop = DEMUCS_HOP_LENGTH;

    // Parse frequency output shape: [batch, stems, ch*2, freq_bins, frames]
    // Actual shape: [1, 4, 4, 2048, 336]
    let num_stems = if freq_shape.len() >= 2 {
        freq_shape[1] as usize
    } else {
        4
    };
    let num_channels = 2; // stereo
    let cac_channels = if freq_shape.len() >= 3 {
        freq_shape[2] as usize // Should be 4 (2 channels * 2 for real/imag)
    } else {
        4
    };
    let freq_bins = if freq_shape.len() >= 4 {
        freq_shape[3] as usize
    } else {
        n_fft / 2
    };
    let num_frames = if freq_shape.len() >= 5 {
        freq_shape[4] as usize
    } else {
        DEMUCS_STFT_FRAMES
    };

    log::debug!(
        "ISTFT params: stems={}, cac_channels={}, freq_bins={}, frames={}",
        num_stems, cac_channels, freq_bins, num_frames
    );

    // Prepare inverse FFT
    let mut planner = RealFftPlanner::<f32>::new();
    let ifft = planner.plan_fft_inverse(n_fft);

    // Pre-compute Hann window
    let window: Vec<f32> = (0..n_fft)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / n_fft as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect();

    // Normalization factor (matches torch.stft normalized=True)
    let norm_factor = (n_fft as f32).sqrt();

    // Output: [stems, channels, samples] flattened as [stems * channels * samples]
    let mut combined = vec![0.0f32; num_stems * num_channels * segment_samples];

    // Copy time branch data directly
    // Time branch shape: [1, 4, 2, samples] = [batch, stems, channels, samples]
    for i in 0..time_data.len().min(combined.len()) {
        combined[i] = time_data[i];
    }

    // Debug: Log RMS of time branch per stem
    if log::log_enabled!(log::Level::Debug) {
        for s in 0..num_stems.min(4) {
            let stem_start = s * num_channels * segment_samples;
            let stem_end = stem_start + num_channels * segment_samples;
            if stem_end <= time_data.len() {
                let rms: f32 = time_data[stem_start..stem_end]
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    / (num_channels * segment_samples) as f32;
                log::debug!("Time branch stem {} RMS: {:.6}", s, rms.sqrt());
            }
        }
    }

    // Process frequency branch: convert CaC to complex, apply ISTFT, add to time
    // Frequency output layout: [batch, stems, ch*2, freq, frames] = [1, 4, 4, 2048, 336]
    // For each stem s and channel c (0=left, 1=right):
    //   real at ch_idx = c*2, imag at ch_idx = c*2+1
    //   index = s * stem_stride + ch_idx * ch_stride + freq * frame_stride + frame

    // Strides for 5D tensor [1, stems, ch*2, freq, frames]
    let frame_stride = 1usize;
    let freq_stride_5d = num_frames;
    let ch_stride = freq_bins * num_frames;
    let stem_stride = cac_channels * ch_stride;

    for stem in 0..num_stems {
        for ch in 0..num_channels {
            // Calculate padded output length for ISTFT
            // We need to account for the 2-frame offset and padding
            let padded_len = (num_frames + 4) * hop + n_fft;
            let mut istft_output = vec![0.0f32; padded_len];
            let mut window_sum = vec![0.0f32; padded_len];

            // Channel indices in CaC format: [left_real, left_imag, right_real, right_imag]
            let real_ch_idx = ch * 2;       // 0 for left, 2 for right
            let imag_ch_idx = ch * 2 + 1;   // 1 for left, 3 for right

            // Prepare buffers for IFFT
            let mut scratch = ifft.make_scratch_vec();
            let mut complex_frame = ifft.make_input_vec();
            let mut time_frame = vec![0.0f32; n_fft];

            // Process each frame
            for frame in 0..num_frames {
                // Extract complex spectrum for this frame
                // Zero the first 2 and last 2 frequency bins as per sevagh's implementation
                // This removes DC/near-DC and near-Nyquist content that can cause artifacts
                for freq in 0..complex_frame.len() {
                    if freq < 2 || freq >= freq_bins - 2 {
                        // Zero: bins 0, 1 (DC area), bins 2046, 2047 (near-Nyquist), bin 2048 (Nyquist)
                        complex_frame[freq] = realfft::num_complex::Complex::new(0.0, 0.0);
                    } else {
                        // Index into 5D tensor [batch, stems, ch*2, freq, frames]
                        let real_idx = stem * stem_stride
                            + real_ch_idx * ch_stride
                            + freq * freq_stride_5d
                            + frame * frame_stride;
                        let imag_idx = stem * stem_stride
                            + imag_ch_idx * ch_stride
                            + freq * freq_stride_5d
                            + frame * frame_stride;

                        let re = if real_idx < freq_data.len() {
                            freq_data[real_idx]
                        } else {
                            0.0
                        };
                        let im = if imag_idx < freq_data.len() {
                            freq_data[imag_idx]
                        } else {
                            0.0
                        };

                        complex_frame[freq] = realfft::num_complex::Complex::new(re, im);
                    }
                }

                // Apply inverse FFT
                if ifft
                    .process_with_scratch(&mut complex_frame, &mut time_frame, &mut scratch)
                    .is_ok()
                {
                    // Apply window and accumulate with overlap-add
                    // No frame offset needed - the pad extraction handles alignment
                    let frame_start = frame * hop;

                    for i in 0..n_fft {
                        let out_idx = frame_start + i;
                        if out_idx < istft_output.len() {
                            // Apply window and normalize
                            let sample = time_frame[i] * window[i] * norm_factor / n_fft as f32;
                            istft_output[out_idx] += sample;
                            window_sum[out_idx] += window[i] * window[i];
                        }
                    }
                }
            }

            // Normalize by window sum and extract the valid region
            // The valid region starts after the demucs padding (hop/2 * 3 = 1536)
            let pad = hop / 2 * 3;
            let output_offset = stem * num_channels * segment_samples + ch * segment_samples;

            // Calculate expected window sum for full 75% overlap (4 frames contribute)
            // For Hann window with 75% overlap, the sum of squared windows ≈ 1.5
            let expected_window_sum: f32 = (0..n_fft)
                .map(|i| {
                    let w = window[i];
                    w * w
                })
                .sum::<f32>()
                / hop as f32; // Normalize per hop
            let min_window_sum = expected_window_sum * 0.5; // Use 50% of expected as minimum

            for i in 0..segment_samples {
                let src_idx = pad + i;
                if src_idx < istft_output.len() {
                    let w = window_sum[src_idx];
                    // Use minimum threshold to avoid amplifying noise at edges
                    let freq_sample = if w > min_window_sum {
                        istft_output[src_idx] / w
                    } else if w > 1e-8 {
                        // Partial coverage - scale down to avoid edge artifacts
                        istft_output[src_idx] / min_window_sum
                    } else {
                        0.0 // No window coverage - output silence
                    };
                    // Add frequency branch to time branch
                    combined[output_offset + i] += freq_sample;
                }
            }
        }
    }

    // Debug: Log RMS of combined output per stem
    if log::log_enabled!(log::Level::Debug) {
        for s in 0..num_stems.min(4) {
            let stem_start = s * num_channels * segment_samples;
            let stem_end = stem_start + num_channels * segment_samples;
            if stem_end <= combined.len() {
                let rms: f32 = combined[stem_start..stem_end]
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    / (num_channels * segment_samples) as f32;
                log::debug!("Combined stem {} RMS: {:.6}", s, rms.sqrt());
            }
        }
    }

    combined
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
