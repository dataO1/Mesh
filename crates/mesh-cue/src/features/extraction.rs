//! Audio feature extraction using Essentia algorithms
//!
//! Extracts audio features (spectral, rhythm, energy) for intensity scoring.
//! The 16-dim HNSW vector is no longer stored — these features are only used
//! to seed IntensityComponents for composite scoring.

use mesh_core::types::SAMPLE_RATE;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during feature extraction
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum FeatureExtractionError {
    #[error("Essentia algorithm error: {0}")]
    Algorithm(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Subprocess error: {0}")]
    Subprocess(String),
    #[error("IO error: {0}")]
    Io(String),
}

/// Extracted audio features from Essentia analysis.
///
/// Used locally to seed IntensityComponents. No longer stored in the database
/// as a 16-dim HNSW vector — similarity search uses EffNet PCA embeddings instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFeatures {
    pub bpm_normalized: f32,
    pub bpm_confidence: f32,
    pub beat_strength: f32,
    pub rhythm_regularity: f32,
    pub key_x: f32,
    pub key_y: f32,
    pub mode: f32,
    pub harmonic_complexity: f32,
    pub lufs_normalized: f32,
    pub dynamic_range: f32,
    pub energy_mean: f32,
    pub energy_variance: f32,
    pub spectral_centroid: f32,
    pub spectral_bandwidth: f32,
    pub spectral_rolloff: f32,
    pub mfcc_flatness: f32,
    /// Psychoacoustic dissonance (Plomp-Levelt roughness). None if not computed.
    pub dissonance: Option<f32>,
}

/// Extract audio features from samples
///
/// This function runs all Essentia algorithms to extract the 16-dimensional
/// feature vector. Must be called from an isolated subprocess due to Essentia's
/// thread-safety limitations.
///
/// # Arguments
/// * `samples` - Mono audio samples at the system sample rate (48kHz)
///
/// # Returns
/// Complete AudioFeatures vector ready for HNSW indexing
pub fn extract_audio_features(samples: &[f32]) -> Result<AudioFeatures, FeatureExtractionError> {
    use essentia::algorithm::loudness_dynamics::dynamic_complexity::DynamicComplexity;
    use essentia::algorithm::rhythm::danceability::Danceability;
    use essentia::algorithm::rhythm::rhythm_descriptors::RhythmDescriptors;
    use essentia::algorithm::spectral::roll_off::RollOff;
    use essentia::algorithm::spectral::spectral_centroid_time::SpectralCentroidTime;
    use essentia::algorithm::spectral::spectral_complexity::SpectralComplexity;
    use essentia::algorithm::spectral::spectrum::Spectrum;
    use essentia::algorithm::statistics::flatness::Flatness;
    use essentia::algorithm::tonal::key_extractor::KeyExtractor;
    use essentia::data::GetFromDataContainer;
    use essentia::essentia::Essentia;

    if samples.len() < 1024 {
        return Err(FeatureExtractionError::InvalidInput(
            "Audio too short for feature extraction".to_string(),
        ));
    }

    let essentia = Essentia::new();
    let sample_rate = SAMPLE_RATE as f32;

    // =========================================================================
    // RHYTHM FEATURES
    // =========================================================================

    // RhythmDescriptors gives us BPM, confidence, and first_peak_weight (regularity)
    let (bpm, bpm_confidence, rhythm_regularity) = {
        let mut rhythm = essentia
            .create::<RhythmDescriptors>()
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = rhythm
            .compute(samples)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let bpm: f32 = result
            .bpm()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        let confidence: f32 = result
            .confidence()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        let first_peak_weight: f32 = result
            .first_peak_weight()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        (bpm, confidence, first_peak_weight)
    };

    // Danceability provides beat_strength (0-3 scale, higher = more danceable)
    let beat_strength = {
        let mut dance = essentia
            .create::<Danceability>()
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = dance
            .compute(samples)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let danceability: f32 = result
            .danceability()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Normalize from 0-3 to 0-1
        (danceability / 3.0).clamp(0.0, 1.0)
    };

    // =========================================================================
    // HARMONY FEATURES
    // =========================================================================

    // KeyExtractor gives us key and mode
    let (key_x, key_y, mode) = {
        let mut key_algo = essentia
            .create::<KeyExtractor>()
            .profile_type("edma")
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = key_algo
            .compute(samples)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let key: String = result
            .key()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        let scale: String = result
            .scale()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Convert key to circular encoding (0-11 semitones → x,y on unit circle)
        let key_index = key_to_index(&key);
        let angle = (key_index as f32) * 2.0 * std::f32::consts::PI / 12.0;
        let key_x = angle.cos();
        let key_y = angle.sin();

        // Mode: 1.0 = major, 0.0 = minor
        let mode = if scale == "major" { 1.0 } else { 0.0 };

        (key_x, key_y, mode)
    };

    // =========================================================================
    // COMPUTE SPECTRUM FOR SPECTRAL FEATURES
    // =========================================================================

    // Use a representative segment from the middle of the track for spectral analysis
    let frame_size = 4096;
    let mid_start = samples.len().saturating_sub(frame_size) / 2;
    let frame = &samples[mid_start..mid_start + frame_size.min(samples.len() - mid_start)];

    let spectrum_data: Vec<f32> = {
        let mut spectrum_algo = essentia
            .create::<Spectrum>()
            .size(frame_size as i32)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = spectrum_algo
            .compute(frame)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        result
            .spectrum()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get()
    };

    // SpectralComplexity - used as proxy for harmonic complexity
    let harmonic_complexity = {
        let mut sc = essentia
            .create::<SpectralComplexity>()
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = sc
            .compute(spectrum_data.as_slice())
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let complexity: f32 = result
            .spectral_complexity()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Normalize (typical range 0-50 for EDM)
        (complexity / 50.0).clamp(0.0, 1.0)
    };

    // =========================================================================
    // ENERGY FEATURES
    // =========================================================================

    // LUFS measurement
    let lufs = measure_lufs_internal(&essentia, samples, sample_rate)?;

    // DynamicComplexity for dynamic range
    let dynamic_range = {
        let mut dc = essentia
            .create::<DynamicComplexity>()
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = dc
            .compute(samples)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let complexity: f32 = result
            .dynamic_complexity()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Normalize (typical range 0-20 dB)
        (complexity / 20.0).clamp(0.0, 1.0)
    };

    // Segment-based energy statistics
    let (energy_mean, energy_variance) = compute_energy_statistics(&essentia, samples)?;

    // =========================================================================
    // TIMBRE FEATURES
    // =========================================================================

    // SpectralCentroidTime
    let spectral_centroid = {
        let mut sc = essentia
            .create::<SpectralCentroidTime>()
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = sc
            .compute(samples)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let centroid: f32 = result
            .centroid()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Normalize by Nyquist frequency
        (centroid / (sample_rate / 2.0)).clamp(0.0, 1.0)
    };

    // RollOff (85% cutoff)
    let spectral_rolloff = {
        let mut ro = essentia
            .create::<RollOff>()
            .sample_rate(sample_rate)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .cutoff(0.85_f32)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = ro
            .compute(spectrum_data.as_slice())
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let rolloff: f32 = result
            .roll_off()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        // Normalize by Nyquist frequency
        (rolloff / (sample_rate / 2.0)).clamp(0.0, 1.0)
    };

    // Flatness (on spectrum) - indicates noisiness vs tonality
    let mfcc_flatness = {
        // Ensure no negative values (add small epsilon for stability)
        let positive_spectrum: Vec<f32> = spectrum_data.iter().map(|&x| x.max(1e-10)).collect();

        let mut flatness = essentia
            .create::<Flatness>()
            .configure()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let result = flatness
            .compute(positive_spectrum.as_slice())
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let flat: f32 = result
            .flatness()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        flat.clamp(0.0, 1.0)
    };

    // Compute spectral bandwidth from frequency bands
    let spectral_bandwidth = compute_spectral_bandwidth(&essentia, &spectrum_data, sample_rate)?;

    // =========================================================================
    // ASSEMBLE FEATURE VECTOR
    // =========================================================================

    // Psychoacoustic dissonance: SpectralPeaks → Dissonance (Plomp-Levelt roughness curves).
    // High values indicate intermodulation products from distortion/saturation (e.g. neuro DnB
    // growling basses). Stored separately in `track_dissonance`; not included in the HNSW vector.
    let dissonance = compute_dissonance(&essentia, &spectrum_data, sample_rate);

    Ok(AudioFeatures {
        // Rhythm
        bpm_normalized: ((bpm - 60.0) / 140.0).clamp(0.0, 1.0),
        bpm_confidence,
        beat_strength,
        rhythm_regularity,

        // Harmony
        key_x,
        key_y,
        mode,
        harmonic_complexity,

        // Energy
        lufs_normalized: ((-24.0 - lufs) / 24.0).clamp(0.0, 1.0),
        dynamic_range,
        energy_mean,
        energy_variance,

        // Timbre
        spectral_centroid,
        spectral_bandwidth,
        spectral_rolloff,
        mfcc_flatness,
        dissonance,
    })
}

/// Extract features in an isolated subprocess
///
/// Essentia's C++ library has global state and is NOT thread-safe.
/// This spawns analysis in a separate process for isolation.
///
/// # Arguments
/// * `samples` - Audio samples (ownership transferred to avoid copy)
///
/// # Returns
/// AudioFeatures from subprocess
pub fn extract_audio_features_in_subprocess(
    samples: Vec<f32>,
) -> Result<AudioFeatures, FeatureExtractionError> {
    use std::io::{Read, Write};

    // Generate unique temp file path
    let temp_path = std::env::temp_dir().join(format!(
        "mesh_features_{}.bin",
        std::process::id()
            ^ (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u32)
    ));

    // Write samples to temp file (raw f32 bytes)
    {
        let mut file = std::fs::File::create(&temp_path)
            .map_err(|e| FeatureExtractionError::Io(e.to_string()))?;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                samples.as_ptr() as *const u8,
                samples.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes)
            .map_err(|e| FeatureExtractionError::Io(e.to_string()))?;
    }
    let sample_count = samples.len();
    drop(samples); // Free memory before spawning subprocess

    // Spawn subprocess with temp file path
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let handle = procspawn::spawn((temp_path_str.clone(), sample_count), |(path, count)| {
        // Read samples from temp file in subprocess
        let samples = (|| -> Result<Vec<f32>, String> {
            let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut bytes = vec![0u8; count * std::mem::size_of::<f32>()];
            file.read_exact(&mut bytes).map_err(|e| e.to_string())?;

            // Convert bytes back to f32
            let samples: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
            Ok(samples)
        })()
        .map_err(|e| FeatureExtractionError::Io(e))?;

        // Run extraction in isolated process
        extract_audio_features(&samples)
    });

    // Wait for result
    let result = handle
        .join()
        .map_err(|e| FeatureExtractionError::Subprocess(format!("{:?}", e)))?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    result
}

// ============================================================================
// Helper functions
// ============================================================================

/// Convert key name to semitone index (0-11)
fn key_to_index(key: &str) -> u8 {
    match key.to_uppercase().as_str() {
        "C" => 0,
        "C#" | "DB" => 1,
        "D" => 2,
        "D#" | "EB" => 3,
        "E" => 4,
        "F" => 5,
        "F#" | "GB" => 6,
        "G" => 7,
        "G#" | "AB" => 8,
        "A" => 9,
        "A#" | "BB" => 10,
        "B" => 11,
        _ => 0,
    }
}

/// Measure LUFS using Essentia's LoudnessEbur128
fn measure_lufs_internal(
    essentia: &essentia::essentia::Essentia,
    samples: &[f32],
    sample_rate: f32,
) -> Result<f32, FeatureExtractionError> {
    use essentia::algorithm::loudness_dynamics::loudness_ebur_128::LoudnessEbur128;
    use essentia::data::GetFromDataContainer;

    // LoudnessEbur128 expects stereo - convert mono to stereo
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct StereoSample {
        left: f32,
        right: f32,
    }

    let stereo_samples: Vec<StereoSample> = samples
        .iter()
        .map(|&s| StereoSample { left: s, right: s })
        .collect();

    let mut loudness = essentia
        .create::<LoudnessEbur128>()
        .sample_rate(sample_rate)
        .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
        .configure()
        .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

    // SAFETY: Our StereoSample has same layout as essentia_sys::ffi::StereoSample
    let ffi_samples: &[essentia_sys::ffi::StereoSample] = unsafe {
        std::slice::from_raw_parts(
            stereo_samples.as_ptr() as *const essentia_sys::ffi::StereoSample,
            stereo_samples.len(),
        )
    };

    let result = loudness
        .compute(ffi_samples)
        .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

    let lufs: f32 = result
        .integrated_loudness()
        .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
        .get();

    Ok(lufs)
}

/// Compute segment-based energy statistics
fn compute_energy_statistics(
    essentia: &essentia::essentia::Essentia,
    samples: &[f32],
) -> Result<(f32, f32), FeatureExtractionError> {
    use essentia::algorithm::statistics::energy::Energy;
    use essentia::data::GetFromDataContainer;

    // Split into segments (1 second each)
    let segment_size = SAMPLE_RATE as usize;
    let mut energies = Vec::new();

    let mut energy_algo = essentia
        .create::<Energy>()
        .configure()
        .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

    for chunk in samples.chunks(segment_size) {
        if chunk.len() < segment_size / 2 {
            continue; // Skip very short final segments
        }

        let result = energy_algo
            .compute(chunk)
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?;

        let energy: f32 = result
            .energy()
            .map_err(|e| FeatureExtractionError::Algorithm(e.to_string()))?
            .get();

        energies.push(energy);
    }

    if energies.is_empty() {
        return Ok((0.5, 0.0));
    }

    // Compute mean
    let mean = energies.iter().sum::<f32>() / energies.len() as f32;

    // Compute variance
    let variance = if energies.len() > 1 {
        energies.iter().map(|&e| (e - mean).powi(2)).sum::<f32>() / (energies.len() - 1) as f32
    } else {
        0.0
    };

    // Normalize (typical energy range is 0 to segment_size for normalized audio)
    let normalized_mean = (mean / (segment_size as f32)).sqrt().clamp(0.0, 1.0);
    let normalized_variance = (variance / (segment_size as f32).powi(2))
        .sqrt()
        .clamp(0.0, 1.0);

    Ok((normalized_mean, normalized_variance))
}

/// Compute psychoacoustic dissonance using spectral peaks and Plomp-Levelt roughness curves.
///
/// High values (~0.6–1.0) indicate dense intermodulation products from distorted/saturated
/// instruments — characteristic of neuro DnB growling basses, harsh synthesisers, and heavy
/// overdrive. Clean instruments (liquid DnB sub-bass, pads, vocals) produce low values.
///
/// Uses the same middle-frame spectrum already computed for other spectral features.
/// Returns `None` on algorithm error (non-fatal — dissonance is optional).
fn compute_dissonance(
    essentia: &essentia::essentia::Essentia,
    spectrum: &[f32],
    sample_rate: f32,
) -> Option<f32> {
    use essentia::algorithm::spectral::spectral_peaks::SpectralPeaks;
    use essentia::algorithm::tonal::dissonance::Dissonance;
    use essentia::data::GetFromDataContainer;

    let mut peaks_algo = essentia
        .create::<SpectralPeaks>()
        .sample_rate(sample_rate)
        .ok()?
        .max_peaks(50_i32)
        .ok()?
        .order_by("frequency")
        .ok()?
        .configure()
        .ok()?;

    let peaks = peaks_algo.compute(spectrum).ok()?;
    let freqs: Vec<f32> = peaks.frequencies().ok()?.get();
    let mags: Vec<f32>  = peaks.magnitudes().ok()?.get();

    if freqs.is_empty() {
        return Some(0.0);
    }

    let mut diss_algo = essentia
        .create::<Dissonance>()
        .configure()
        .ok()?;

    let result = diss_algo.compute(freqs.as_slice(), mags.as_slice()).ok()?;
    let d: f32 = result.dissonance().ok()?.get();
    Some(d.clamp(0.0, 1.0))
}

/// Compute spectral bandwidth (frequency spread around centroid)
fn compute_spectral_bandwidth(
    _essentia: &essentia::essentia::Essentia,
    spectrum: &[f32],
    sample_rate: f32,
) -> Result<f32, FeatureExtractionError> {
    // Compute spectral bandwidth manually as weighted standard deviation
    // around the centroid frequency

    let nyquist = sample_rate / 2.0;
    let bin_width = nyquist / spectrum.len() as f32;

    // Compute centroid first
    let total_power: f32 = spectrum.iter().sum();
    if total_power < 1e-10 {
        return Ok(0.0);
    }

    let centroid: f32 = spectrum
        .iter()
        .enumerate()
        .map(|(i, &mag)| (i as f32 * bin_width) * mag)
        .sum::<f32>()
        / total_power;

    // Compute bandwidth (standard deviation around centroid)
    let variance: f32 = spectrum
        .iter()
        .enumerate()
        .map(|(i, &mag)| {
            let freq = i as f32 * bin_width;
            mag * (freq - centroid).powi(2)
        })
        .sum::<f32>()
        / total_power;

    let bandwidth = variance.sqrt();

    // Normalize by Nyquist
    Ok((bandwidth / nyquist).clamp(0.0, 1.0))
}

/// Compute intensity components using multi-frame analysis (pure Rust).
///
/// Analyzes N frames spread across the track and averages the per-frame values.
/// This avoids the single-frame reliability problem of the original 16-dim features.
///
/// Returns `IntensityComponents` with all values in [0, 1].
pub fn compute_intensity_components(samples: &[f32], sample_rate: f32) -> mesh_core::db::IntensityComponents {
    use realfft::RealFftPlanner;

    let frame_size = 4096;
    let hop_size = frame_size / 2;
    let n_bins = frame_size / 2 + 1;

    if samples.len() < frame_size * 2 {
        return mesh_core::db::IntensityComponents::default();
    }

    // Full-track analysis: process ALL frames, skipping first/last 5% to avoid silence.
    // Cost: <100ms for a 4-minute track (~4,650 frames × ~13µs/frame).
    let start = samples.len() / 20;
    let end = samples.len() - samples.len() / 20;
    let usable = end - start;
    let n_frames = usable / hop_size;
    if n_frames < 3 {
        return mesh_core::db::IntensityComponents::default();
    }

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(frame_size);
    let mut fft_input = vec![0.0f32; frame_size];
    let mut fft_output = vec![realfft::num_complex::Complex::new(0.0f32, 0.0); n_bins];

    // Hann window
    let window: Vec<f32> = (0..frame_size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (frame_size - 1) as f32).cos()))
        .collect();

    let mut flatness_values = Vec::with_capacity(n_frames);
    let mut dissonance_values = Vec::with_capacity(n_frames);
    let mut harmonic_complexity_values = Vec::with_capacity(n_frames);
    let mut rolloff_values = Vec::with_capacity(n_frames);
    let mut centroid_values = Vec::with_capacity(n_frames);
    let mut rms_values = Vec::with_capacity(n_frames);
    let mut prev_magnitude: Option<Vec<f32>> = None;
    let mut flux_values = Vec::with_capacity(n_frames);

    for frame_idx in 0..n_frames {
        let frame_start = start + frame_idx * hop_size;
        if frame_start + frame_size > samples.len() { break; }

        // Apply window and FFT
        for i in 0..frame_size {
            fft_input[i] = samples[frame_start + i] * window[i];
        }
        if fft.process(&mut fft_input, &mut fft_output).is_err() { continue; }

        // Magnitude spectrum
        let magnitude: Vec<f32> = fft_output.iter().map(|c| (c.re * c.re + c.im * c.im).sqrt()).collect();
        let total_energy: f32 = magnitude.iter().map(|m| m * m).sum();

        if total_energy < 1e-10 { continue; }

        let sum_mag: f32 = magnitude.iter().sum::<f32>();
        let arith_mean = sum_mag / n_bins as f32;

        // ── Spectral centroid (weighted average frequency, normalized by Nyquist) ──
        if sum_mag > 1e-10 {
            let weighted_sum: f32 = magnitude.iter().enumerate()
                .map(|(i, &m)| i as f32 * m)
                .sum();
            centroid_values.push((weighted_sum / sum_mag) / n_bins as f32);
        }

        // ── Per-frame RMS energy (for inter-frame energy variance) ──
        rms_values.push((total_energy / n_bins as f32).sqrt());

        // ── Spectral flatness (geometric mean / arithmetic mean) ──
        // Mathematically bounded [0, 1] — no artificial scaling needed.
        let log_sum: f32 = magnitude.iter().map(|&m| (m.max(1e-10)).ln()).sum::<f32>();
        let geom_mean = (log_sum / n_bins as f32).exp();
        let flatness = if arith_mean > 1e-10 { geom_mean / arith_mean } else { 0.0 };
        flatness_values.push(flatness);

        // ── Spectral rolloff (bin position / total bins) ──
        // Intrinsically [0, 1] — fraction of spectrum containing 85% of energy.
        let threshold = total_energy * 0.85;
        let mut cumulative = 0.0f32;
        let mut rolloff_bin = n_bins - 1;
        for (i, &m) in magnitude.iter().enumerate() {
            cumulative += m * m;
            if cumulative >= threshold {
                rolloff_bin = i;
                break;
            }
        }
        rolloff_values.push(rolloff_bin as f32 / n_bins as f32);

        // ── Harmonic complexity (spectral gradient / total magnitude) ──
        // Raw ratio — no arbitrary /50.0 scaling. Percentile-ranked at query time.
        let gradient: f32 = magnitude.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .sum();
        let complexity = if sum_mag > 1e-10 { gradient / sum_mag } else { 0.0 };
        harmonic_complexity_values.push(complexity);

        // ── Dissonance (Plomp-Levelt roughness per peak pair) ──
        // Raw average roughness — no arbitrary *100 scaling. Percentile-ranked at query time.
        let peak_threshold = arith_mean * 3.0;
        let mut peaks: Vec<(usize, f32)> = Vec::new();
        for i in 1..magnitude.len() - 1 {
            if magnitude[i] > magnitude[i-1] && magnitude[i] > magnitude[i+1] && magnitude[i] > peak_threshold {
                peaks.push((i, magnitude[i]));
            }
        }
        peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        peaks.truncate(30);

        let mut roughness = 0.0f32;
        let mut pair_count = 0u32;
        let freq_res = sample_rate / frame_size as f32;
        for i in 0..peaks.len() {
            for j in (i+1)..peaks.len() {
                let f1 = peaks[i].0 as f32 * freq_res;
                let f2 = peaks[j].0 as f32 * freq_res;
                let diff = (f2 - f1).abs();
                let s = 0.24 * (f1.min(f2) * 0.021 + 19.0); // critical bandwidth
                if diff < s && diff > 0.0 {
                    let d = diff / s;
                    roughness += (peaks[i].1.min(peaks[j].1)) * (d * (-3.5 * d).exp());
                    pair_count += 1;
                }
            }
        }
        let raw_roughness = if pair_count > 0 {
            roughness / pair_count as f32
        } else { 0.0 };
        dissonance_values.push(raw_roughness);

        // ── Spectral flux (energy-normalized L2 distance from previous frame) ──
        // Divided by frame energy for scale-independence. Raw value, no clamp.
        if let Some(ref prev) = prev_magnitude {
            let flux: f32 = magnitude.iter().zip(prev.iter())
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt();
            flux_values.push(flux / total_energy.sqrt().max(1e-6));
        }
        prev_magnitude = Some(magnitude);
    }

    // ── Crest factor (peak / RMS over the full track) ──
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let crest_db = if rms > 1e-10 { 20.0 * (peak / rms).log10() } else { 20.0 };
    // Normalize: 3 dB (heavily limited) → 1.0, 20 dB (very dynamic) → 0.0
    let crest_norm = (1.0 - (crest_db - 3.0) / 17.0).clamp(0.0, 1.0);

    // ── Averages and variances ──
    // All values stored raw — no artificial scaling or clamping.
    // Percentile-rank normalization at query time handles scale equalization.
    let avg = |vals: &[f32]| -> f32 {
        if vals.is_empty() { 0.0 } else { vals.iter().sum::<f32>() / vals.len() as f32 }
    };
    let variance_of = |vals: &[f32]| -> f32 {
        if vals.len() < 2 { return 0.0; }
        let mean = vals.iter().sum::<f32>() / vals.len() as f32;
        vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / vals.len() as f32
    };

    // Energy variance: coefficient of variation² (scale-independent, raw)
    let energy_variance = if rms_values.len() >= 2 {
        let mean_rms = avg(&rms_values);
        if mean_rms > 1e-10 {
            let var = rms_values.iter()
                .map(|r| (r - mean_rms).powi(2))
                .sum::<f32>() / rms_values.len() as f32;
            var / (mean_rms * mean_rms)
        } else { 0.0 }
    } else { 0.0 };

    mesh_core::db::IntensityComponents {
        spectral_flux: avg(&flux_values),
        flatness: avg(&flatness_values),
        spectral_centroid: avg(&centroid_values),
        dissonance: avg(&dissonance_values),
        crest_factor: crest_norm,
        energy_variance,
        harmonic_complexity: avg(&harmonic_complexity_values),
        spectral_rolloff: avg(&rolloff_values),
        centroid_variance: variance_of(&centroid_values),
        flux_variance: variance_of(&flux_values),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_to_index() {
        assert_eq!(key_to_index("C"), 0);
        assert_eq!(key_to_index("C#"), 1);
        assert_eq!(key_to_index("Db"), 1);
        assert_eq!(key_to_index("A"), 9);
        assert_eq!(key_to_index("B"), 11);
    }

    #[test]
    fn test_audio_features_normalization() {
        // Test that features are properly normalized to [0, 1] range
        let features = AudioFeatures {
            bpm_normalized: 0.5, // 130 BPM
            bpm_confidence: 0.9,
            beat_strength: 0.7,
            rhythm_regularity: 0.8,
            key_x: 0.5,
            key_y: 0.866,
            mode: 1.0,
            harmonic_complexity: 0.3,
            lufs_normalized: 0.6, // -10 LUFS
            dynamic_range: 0.4,
            energy_mean: 0.7,
            energy_variance: 0.2,
            spectral_centroid: 0.5,
            spectral_bandwidth: 0.4,
            spectral_rolloff: 0.6,
            mfcc_flatness: 0.3,
            dissonance: None,
        };

        // Verify all features are in [0, 1] range
        assert!(features.bpm_normalized >= 0.0 && features.bpm_normalized <= 1.0);
        assert!(features.spectral_centroid >= 0.0 && features.spectral_centroid <= 1.0);
        assert!(features.energy_variance >= 0.0 && features.energy_variance <= 1.0);
    }
}
