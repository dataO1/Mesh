//! Mel spectrogram preprocessing for EffNet models
//!
//! Computes 96-band mel spectrograms using Essentia's MelBands algorithm.
//! This runs inside the procspawn subprocess alongside existing BPM/key analysis
//! because Essentia's C++ FFI is not thread-safe.

use serde::{Serialize, Deserialize};

/// Mel spectrogram result to be passed from subprocess to worker thread.
///
/// Contains the 96-band mel spectrogram frames, ready for EffNet input.
/// Each frame is a 96-dimensional vector of log-compressed mel band energies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MelSpectrogramResult {
    /// Mel spectrogram frames (each frame = 96 mel band values)
    pub frames: Vec<Vec<f32>>,
    /// Number of mel bands (always 96 for EffNet)
    pub n_bands: usize,
    /// Sample rate used for computation (16000 Hz)
    pub sample_rate: u32,
}

/// Compute mel spectrogram for EffNet input.
///
/// Uses a pure Rust implementation of the mel spectrogram computation
/// matching Essentia's MelBands parameters:
/// - Resample to 16kHz
/// - 96 mel bands, frame_size=512, hop_size=256
/// - Log compression: log10(1 + 10000 * x)
///
/// # Arguments
/// * `samples` - Input mono audio samples
/// * `sample_rate` - Input sample rate (will be resampled to 16kHz)
///
/// # Returns
/// Mel spectrogram frames ready for EffNet processing
pub fn compute_mel_spectrogram(samples: &[f32], sample_rate: f32) -> Result<MelSpectrogramResult, String> {
    if samples.is_empty() {
        return Err("Empty input samples".to_string());
    }

    // Parameters matching Essentia's EffNet preprocessing
    const TARGET_SR: f32 = 16000.0;
    const N_BANDS: usize = 96;
    const FRAME_SIZE: usize = 512;
    const HOP_SIZE: usize = 256;

    // Step 1: Resample to 16kHz if needed
    let resampled = if (sample_rate - TARGET_SR).abs() < 1.0 {
        samples.to_vec()
    } else {
        resample_linear(samples, sample_rate, TARGET_SR)
    };

    if resampled.len() < FRAME_SIZE {
        return Err("Audio too short for mel spectrogram".to_string());
    }

    // Step 2: Compute mel filterbank
    let mel_filterbank = create_mel_filterbank(N_BANDS, FRAME_SIZE, TARGET_SR as f32);

    // Step 3: Frame-by-frame STFT + mel bands
    let n_frames = (resampled.len().saturating_sub(FRAME_SIZE)) / HOP_SIZE + 1;
    let mut frames = Vec::with_capacity(n_frames);

    let window = hann_window(FRAME_SIZE);

    for frame_idx in 0..n_frames {
        let start = frame_idx * HOP_SIZE;
        let end = (start + FRAME_SIZE).min(resampled.len());

        // Apply window
        let mut windowed = vec![0.0f32; FRAME_SIZE];
        for i in 0..(end - start) {
            windowed[i] = resampled[start + i] * window[i];
        }

        // Compute power spectrum via DFT (real input)
        let spectrum = compute_power_spectrum(&windowed);

        // Apply mel filterbank
        let mut mel_bands = vec![0.0f32; N_BANDS];
        for (band_idx, filter) in mel_filterbank.iter().enumerate() {
            let mut energy = 0.0f32;
            for (&coeff, &spec_val) in filter.iter().zip(spectrum.iter()) {
                energy += coeff * spec_val;
            }
            // Log compression: log10(1 + 10000 * x)
            mel_bands[band_idx] = (1.0 + 10000.0 * energy.max(0.0)).log10();
        }

        frames.push(mel_bands);
    }

    Ok(MelSpectrogramResult {
        frames,
        n_bands: N_BANDS,
        sample_rate: TARGET_SR as u32,
    })
}

/// Simple linear interpolation resampling
fn resample_linear(samples: &[f32], from_sr: f32, to_sr: f32) -> Vec<f32> {
    let ratio = from_sr / to_sr;
    let output_len = (samples.len() as f32 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f32 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f32;

        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
        } else if idx < samples.len() {
            samples[idx]
        } else {
            0.0
        };
        output.push(sample);
    }

    output
}

/// Generate a Hann window of given size
fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect()
}

/// Compute power spectrum from windowed frame using real DFT
///
/// Returns N/2+1 power spectrum bins.
fn compute_power_spectrum(frame: &[f32]) -> Vec<f32> {
    let n = frame.len();
    let n_bins = n / 2 + 1;
    let mut spectrum = vec![0.0f32; n_bins];

    // Direct DFT computation (O(N^2) but N=512 is small enough)
    for k in 0..n_bins {
        let mut real = 0.0f32;
        let mut imag = 0.0f32;
        for (i, &sample) in frame.iter().enumerate() {
            let angle = -2.0 * std::f32::consts::PI * k as f32 * i as f32 / n as f32;
            real += sample * angle.cos();
            imag += sample * angle.sin();
        }
        spectrum[k] = (real * real + imag * imag) / (n as f32);
    }

    spectrum
}

/// Create mel filterbank matrix
///
/// Returns a Vec of N_BANDS filters, each with N/2+1 coefficients.
fn create_mel_filterbank(n_bands: usize, frame_size: usize, sample_rate: f32) -> Vec<Vec<f32>> {
    let n_bins = frame_size / 2 + 1;
    let f_max = sample_rate / 2.0;

    // Mel scale conversion
    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(f_max);

    // Create evenly-spaced mel points
    let n_points = n_bands + 2;
    let mel_points: Vec<f32> = (0..n_points)
        .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_points - 1) as f32)
        .collect();

    // Convert back to Hz and then to FFT bin indices
    let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_points: Vec<f32> = hz_points
        .iter()
        .map(|&hz| hz * frame_size as f32 / sample_rate)
        .collect();

    // Create triangular filters
    let mut filterbank = Vec::with_capacity(n_bands);
    for band in 0..n_bands {
        let mut filter = vec![0.0f32; n_bins];
        let left = bin_points[band];
        let center = bin_points[band + 1];
        let right = bin_points[band + 2];

        for bin in 0..n_bins {
            let bin_f = bin as f32;
            if bin_f >= left && bin_f <= center && (center - left) > 0.0 {
                filter[bin] = (bin_f - left) / (center - left);
            } else if bin_f > center && bin_f <= right && (right - center) > 0.0 {
                filter[bin] = (right - bin_f) / (right - center);
            }
        }
        filterbank.push(filter);
    }

    filterbank
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mel_hz_roundtrip() {
        let hz = 1000.0;
        let mel = hz_to_mel(hz);
        let back = mel_to_hz(mel);
        assert!((back - hz).abs() < 0.1, "Roundtrip: {} -> {} -> {}", hz, mel, back);
    }

    #[test]
    fn test_compute_mel_spectrogram_basic() {
        // Generate 2 seconds of 440 Hz sine at 44100
        let sr = 44100.0;
        let samples: Vec<f32> = (0..(sr as usize * 2))
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin() * 0.5)
            .collect();

        let result = compute_mel_spectrogram(&samples, sr).unwrap();
        assert_eq!(result.n_bands, 96);
        assert!(result.frames.len() > 100, "Should have many frames: {}", result.frames.len());
        assert_eq!(result.frames[0].len(), 96);
    }

    #[test]
    fn test_empty_input_fails() {
        assert!(compute_mel_spectrogram(&[], 44100.0).is_err());
    }

    #[test]
    fn test_too_short_input_fails() {
        let short = vec![0.0f32; 100]; // Less than one frame at 16kHz
        assert!(compute_mel_spectrogram(&short, 16000.0).is_err());
    }
}
