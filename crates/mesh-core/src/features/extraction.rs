//! Audio feature extraction using Essentia algorithms
//!
//! Extracts a 16-dimensional feature vector for similarity search.

use crate::db::AudioFeatures;
use crate::types::SAMPLE_RATE;
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
        };

        let vec = features.to_vector();
        for (i, &v) in vec.iter().enumerate() {
            assert!(
                v >= 0.0 && v <= 1.0,
                "Feature {} out of range: {}",
                i,
                v
            );
        }
    }
}
