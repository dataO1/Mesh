//! Post-processing for stem separation
//!
//! Implements spectral post-processing techniques to reduce bleed between stems.
//!
//! ## Wiener Softmask Filtering
//!
//! The softmask technique refines separation by computing time-frequency masks
//! based on the ratio of each stem's magnitude to the sum of all stems:
//!
//! ```text
//! mask[stem] = |X[stem]|^power / (Σ|X[all]|^power + eps)
//! output[stem] = mask[stem] * mixture_stft
//! ```
//!
//! This ensures that the sum of all masked stems equals the original mixture
//! in the spectral domain, reducing inter-stem bleed.
//!
//! Reference: UVR5 filtering.py, Open-Unmix, Nugraha et al. (2016)

use super::backend::StemData;
use super::error::SeparationError;
use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

type Result<T> = std::result::Result<T, SeparationError>;

/// STFT parameters for postprocessing (can differ from model's internal STFT)
const POSTPROCESS_NFFT: usize = 2048;
const POSTPROCESS_HOP: usize = POSTPROCESS_NFFT / 4; // 512, 75% overlap

/// Minimum mask value to prevent complete suppression (reduces "musical noise")
const MASK_FLOOR: f32 = 0.02;

/// Temporal smoothing coefficient (0 = no smoothing, 0.9 = heavy smoothing)
/// Higher values reduce pumping but may smear transients
const TEMPORAL_SMOOTHING: f32 = 0.7;

/// Apply Wiener softmask filtering to reduce bleed between stems
///
/// # Algorithm
///
/// 1. Compute STFT of mixture and all 4 stems
/// 2. For each time-frequency bin, compute softmask:
///    `mask[stem] = |stem|^power / (Σ|all_stems|^power + eps)`
/// 3. Apply mask to mixture STFT (not stem STFT)
/// 4. Reconstruct stems via ISTFT
///
/// # Arguments
///
/// * `stems` - Mutable reference to stem data (will be modified in place)
/// * `mixture` - Original stereo mixture (interleaved L/R samples)
/// * `power` - Exponent for magnitude (2.0 = energy, 1.0 = amplitude)
///
/// # Returns
///
/// Result indicating success or failure
pub fn apply_softmask_wiener(
    stems: &mut StemData,
    mixture: &[f32],
    power: f32,
) -> Result<()> {
    let sample_rate = stems.sample_rate;
    let num_samples = stems.samples_per_channel();
    let channels = stems.channels as usize;

    if mixture.len() != num_samples * channels {
        return Err(SeparationError::SeparationFailed(format!(
            "Mixture length {} doesn't match stems {} * {}",
            mixture.len(),
            num_samples,
            channels
        )));
    }

    log::info!(
        "Applying Wiener softmask (power={}, n_fft={}, hop={})",
        power,
        POSTPROCESS_NFFT,
        POSTPROCESS_HOP
    );

    // Process each channel separately
    for ch in 0..channels {
        // Extract mono channel from mixture and stems
        let mix_mono: Vec<f32> = (0..num_samples)
            .map(|i| mixture[i * channels + ch])
            .collect();

        let drums_mono: Vec<f32> = (0..num_samples)
            .map(|i| stems.drums[i * channels + ch])
            .collect();
        let bass_mono: Vec<f32> = (0..num_samples)
            .map(|i| stems.bass[i * channels + ch])
            .collect();
        let other_mono: Vec<f32> = (0..num_samples)
            .map(|i| stems.other[i * channels + ch])
            .collect();
        let vocals_mono: Vec<f32> = (0..num_samples)
            .map(|i| stems.vocals[i * channels + ch])
            .collect();

        // Apply softmask and get filtered stems
        let (filtered_drums, filtered_bass, filtered_other, filtered_vocals) =
            apply_softmask_mono(
                &mix_mono,
                &drums_mono,
                &bass_mono,
                &other_mono,
                &vocals_mono,
                power,
            )?;

        // Write back to stereo interleaved format
        for i in 0..num_samples {
            stems.drums[i * channels + ch] = filtered_drums[i];
            stems.bass[i * channels + ch] = filtered_bass[i];
            stems.other[i * channels + ch] = filtered_other[i];
            stems.vocals[i * channels + ch] = filtered_vocals[i];
        }
    }

    log::info!("Wiener softmask filtering complete");
    Ok(())
}

/// Apply softmask to mono signals
fn apply_softmask_mono(
    mixture: &[f32],
    drums: &[f32],
    bass: &[f32],
    other: &[f32],
    vocals: &[f32],
    power: f32,
) -> Result<(Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>)> {
    let num_samples = mixture.len();
    let n_fft = POSTPROCESS_NFFT;
    let hop = POSTPROCESS_HOP;

    // Compute STFT for mixture and all stems
    let mix_stft = compute_stft(mixture, n_fft, hop)?;
    let drums_stft = compute_stft(drums, n_fft, hop)?;
    let bass_stft = compute_stft(bass, n_fft, hop)?;
    let other_stft = compute_stft(other, n_fft, hop)?;
    let vocals_stft = compute_stft(vocals, n_fft, hop)?;

    let num_frames = mix_stft.len();
    let num_bins = if num_frames > 0 { mix_stft[0].len() } else { 0 };

    // Compute softmasks with temporal smoothing to reduce pumping artifacts
    let mut drums_masked = vec![vec![Complex::new(0.0f32, 0.0); num_bins]; num_frames];
    let mut bass_masked = vec![vec![Complex::new(0.0f32, 0.0); num_bins]; num_frames];
    let mut other_masked = vec![vec![Complex::new(0.0f32, 0.0); num_bins]; num_frames];
    let mut vocals_masked = vec![vec![Complex::new(0.0f32, 0.0); num_bins]; num_frames];

    // Previous frame masks for temporal smoothing
    let mut prev_drums_mask = vec![0.25f32; num_bins]; // Initialize to equal split
    let mut prev_bass_mask = vec![0.25f32; num_bins];
    let mut prev_other_mask = vec![0.25f32; num_bins];
    let mut prev_vocals_mask = vec![0.25f32; num_bins];

    let eps = 1e-10f32;
    let alpha = TEMPORAL_SMOOTHING;
    let one_minus_alpha = 1.0 - alpha;

    for frame in 0..num_frames {
        for bin in 0..num_bins {
            // Compute magnitudes raised to power
            let drums_mag = drums_stft[frame][bin].norm().powf(power);
            let bass_mag = bass_stft[frame][bin].norm().powf(power);
            let other_mag = other_stft[frame][bin].norm().powf(power);
            let vocals_mag = vocals_stft[frame][bin].norm().powf(power);

            let total_mag = drums_mag + bass_mag + other_mag + vocals_mag + eps;

            // Compute raw softmasks
            let mut drums_mask = drums_mag / total_mag;
            let mut bass_mask = bass_mag / total_mag;
            let mut other_mask = other_mag / total_mag;
            let mut vocals_mask = vocals_mag / total_mag;

            // Apply temporal smoothing: mask = alpha * prev_mask + (1-alpha) * current_mask
            // This reduces rapid fluctuations that cause "pumping"
            drums_mask = alpha * prev_drums_mask[bin] + one_minus_alpha * drums_mask;
            bass_mask = alpha * prev_bass_mask[bin] + one_minus_alpha * bass_mask;
            other_mask = alpha * prev_other_mask[bin] + one_minus_alpha * other_mask;
            vocals_mask = alpha * prev_vocals_mask[bin] + one_minus_alpha * vocals_mask;

            // Apply mask floor to prevent complete suppression ("musical noise")
            drums_mask = drums_mask.max(MASK_FLOOR);
            bass_mask = bass_mask.max(MASK_FLOOR);
            other_mask = other_mask.max(MASK_FLOOR);
            vocals_mask = vocals_mask.max(MASK_FLOOR);

            // Renormalize masks to sum to 1 (or slightly more due to floor)
            let mask_sum = drums_mask + bass_mask + other_mask + vocals_mask;
            drums_mask /= mask_sum;
            bass_mask /= mask_sum;
            other_mask /= mask_sum;
            vocals_mask /= mask_sum;

            // Store for next frame's smoothing
            prev_drums_mask[bin] = drums_mask;
            prev_bass_mask[bin] = bass_mask;
            prev_other_mask[bin] = other_mask;
            prev_vocals_mask[bin] = vocals_mask;

            // Apply masks to mixture STFT (preserves phase from mixture)
            let mix_bin = mix_stft[frame][bin];
            drums_masked[frame][bin] = mix_bin * drums_mask;
            bass_masked[frame][bin] = mix_bin * bass_mask;
            other_masked[frame][bin] = mix_bin * other_mask;
            vocals_masked[frame][bin] = mix_bin * vocals_mask;
        }
    }

    // Reconstruct via ISTFT
    let drums_out = compute_istft(&drums_masked, n_fft, hop, num_samples)?;
    let bass_out = compute_istft(&bass_masked, n_fft, hop, num_samples)?;
    let other_out = compute_istft(&other_masked, n_fft, hop, num_samples)?;
    let vocals_out = compute_istft(&vocals_masked, n_fft, hop, num_samples)?;

    Ok((drums_out, bass_out, other_out, vocals_out))
}

/// Compute STFT of a mono signal
fn compute_stft(
    signal: &[f32],
    n_fft: usize,
    hop: usize,
) -> Result<Vec<Vec<Complex<f32>>>> {
    let num_samples = signal.len();

    // Pad signal for complete frames
    let padded_len = ((num_samples + n_fft - 1) / hop) * hop + n_fft;
    let mut padded = vec![0.0f32; padded_len];
    padded[..num_samples].copy_from_slice(signal);

    // Compute number of frames
    let num_frames = (padded_len - n_fft) / hop + 1;
    let num_bins = n_fft / 2 + 1;

    // Prepare FFT
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n_fft);

    // Pre-compute Hann window
    let window: Vec<f32> = (0..n_fft)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / n_fft as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect();

    let norm_factor = 1.0 / (n_fft as f32).sqrt();

    let mut stft = Vec::with_capacity(num_frames);
    let mut scratch = fft.make_scratch_vec();
    let mut frame_buf = vec![0.0f32; n_fft];
    let mut spectrum = fft.make_output_vec();

    for frame_idx in 0..num_frames {
        let start = frame_idx * hop;

        // Apply window
        for i in 0..n_fft {
            frame_buf[i] = padded[start + i] * window[i];
        }

        // Forward FFT
        fft.process_with_scratch(&mut frame_buf, &mut spectrum, &mut scratch)
            .map_err(|e| {
                SeparationError::SeparationFailed(format!("FFT failed: {:?}", e))
            })?;

        // Normalize and store
        let frame_spectrum: Vec<Complex<f32>> = spectrum
            .iter()
            .map(|c| Complex::new(c.re * norm_factor, c.im * norm_factor))
            .collect();

        stft.push(frame_spectrum);
    }

    Ok(stft)
}

/// Compute ISTFT to reconstruct mono signal
fn compute_istft(
    stft: &[Vec<Complex<f32>>],
    n_fft: usize,
    hop: usize,
    target_len: usize,
) -> Result<Vec<f32>> {
    let num_frames = stft.len();
    if num_frames == 0 {
        return Ok(vec![0.0f32; target_len]);
    }

    // Prepare IFFT
    let mut planner = RealFftPlanner::<f32>::new();
    let ifft = planner.plan_fft_inverse(n_fft);

    // Pre-compute Hann window
    let window: Vec<f32> = (0..n_fft)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / n_fft as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect();

    let norm_factor = (n_fft as f32).sqrt();

    // Output buffer and window sum for normalization
    let output_len = (num_frames - 1) * hop + n_fft;
    let mut output = vec![0.0f32; output_len];
    let mut window_sum = vec![0.0f32; output_len];

    let mut scratch = ifft.make_scratch_vec();
    let mut time_frame = vec![0.0f32; n_fft];

    for frame_idx in 0..num_frames {
        // Prepare complex spectrum for IFFT
        let mut spectrum = ifft.make_input_vec();
        for (i, s) in spectrum.iter_mut().enumerate() {
            if i < stft[frame_idx].len() {
                // Undo normalization before IFFT
                *s = Complex::new(
                    stft[frame_idx][i].re * norm_factor,
                    stft[frame_idx][i].im * norm_factor,
                );
            }
        }

        // Inverse FFT
        ifft.process_with_scratch(&mut spectrum, &mut time_frame, &mut scratch)
            .map_err(|e| {
                SeparationError::SeparationFailed(format!("IFFT failed: {:?}", e))
            })?;

        // Apply window and accumulate with overlap-add
        let start = frame_idx * hop;
        for i in 0..n_fft {
            if start + i < output.len() {
                output[start + i] += time_frame[i] * window[i] / n_fft as f32;
                window_sum[start + i] += window[i] * window[i];
            }
        }
    }

    // Normalize by window sum
    let eps = 1e-8f32;
    for i in 0..output.len() {
        if window_sum[i] > eps {
            output[i] /= window_sum[i];
        }
    }

    // Trim to target length
    output.truncate(target_len);
    if output.len() < target_len {
        output.resize(target_len, 0.0);
    }

    Ok(output)
}

// ─────────────────────────────────────────────────────────────────────────────
// High-Frequency Preservation
// ─────────────────────────────────────────────────────────────────────────────

/// Preserve high frequencies in a stem by blending in original mix above cutoff
///
/// Neural network-based separation models often attenuate high frequencies
/// (>14-16kHz) due to STFT resolution limits and training data characteristics.
/// For drums, this results in "dull" sounding hihats and cymbals.
///
/// This function blends the high-frequency content from the original mix
/// into the stem using a smooth crossfade in the frequency domain.
///
/// # Arguments
///
/// * `stem` - Mutable reference to stem samples (stereo interleaved)
/// * `mixture` - Original stereo mixture (interleaved L/R samples)
/// * `cutoff_hz` - Frequency above which to blend (e.g., 14000.0)
/// * `blend_width_hz` - Width of crossfade region (e.g., 2000.0)
/// * `sample_rate` - Sample rate in Hz
///
/// # Algorithm
///
/// 1. Compute STFT of both stem and mixture
/// 2. For each frequency bin:
///    - Below cutoff: use stem
///    - Above cutoff + width: use mixture
///    - In between: smooth crossfade
/// 3. Reconstruct via ISTFT
pub fn preserve_high_frequencies(
    stem: &mut [f32],
    mixture: &[f32],
    cutoff_hz: f32,
    blend_width_hz: f32,
    sample_rate: u32,
) -> Result<()> {
    let num_samples_total = stem.len();
    let channels = 2usize;
    let num_samples = num_samples_total / channels;

    if mixture.len() != num_samples_total {
        return Err(SeparationError::SeparationFailed(format!(
            "Mixture length {} doesn't match stem {}",
            mixture.len(),
            num_samples_total
        )));
    }

    log::info!(
        "Preserving high frequencies: cutoff={:.0}Hz, blend_width={:.0}Hz",
        cutoff_hz,
        blend_width_hz
    );

    let n_fft = POSTPROCESS_NFFT;
    let hop = POSTPROCESS_HOP;
    let num_bins = n_fft / 2 + 1;

    // Calculate frequency bin boundaries for crossfade
    let hz_per_bin = sample_rate as f32 / n_fft as f32;
    let cutoff_bin = (cutoff_hz / hz_per_bin) as usize;
    let blend_end_bin = ((cutoff_hz + blend_width_hz) / hz_per_bin) as usize;

    log::debug!(
        "HF preservation: bins {}-{} (of {}), hz_per_bin={:.1}",
        cutoff_bin,
        blend_end_bin,
        num_bins,
        hz_per_bin
    );

    // Process each channel separately
    for ch in 0..channels {
        // Extract mono channel from stem and mixture
        let stem_mono: Vec<f32> = (0..num_samples)
            .map(|i| stem[i * channels + ch])
            .collect();
        let mix_mono: Vec<f32> = (0..num_samples)
            .map(|i| mixture[i * channels + ch])
            .collect();

        // Compute STFT for both
        let stem_stft = compute_stft(&stem_mono, n_fft, hop)?;
        let mix_stft = compute_stft(&mix_mono, n_fft, hop)?;

        let num_frames = stem_stft.len();

        // Apply frequency-domain crossfade
        let mut blended_stft = vec![vec![Complex::new(0.0f32, 0.0); num_bins]; num_frames];

        for frame in 0..num_frames {
            for bin in 0..num_bins {
                let stem_bin = stem_stft[frame][bin];
                let mix_bin = if frame < mix_stft.len() && bin < mix_stft[frame].len() {
                    mix_stft[frame][bin]
                } else {
                    Complex::new(0.0, 0.0)
                };

                // Calculate blend factor (0 = stem only, 1 = mix only)
                let blend = if bin <= cutoff_bin {
                    0.0 // Below cutoff: use stem
                } else if bin >= blend_end_bin {
                    1.0 // Above blend end: use mix
                } else {
                    // Smooth crossfade using raised cosine
                    let t = (bin - cutoff_bin) as f32 / (blend_end_bin - cutoff_bin) as f32;
                    0.5 * (1.0 - (t * std::f32::consts::PI).cos())
                };

                // Blend: (1-blend)*stem + blend*mix
                blended_stft[frame][bin] = Complex::new(
                    (1.0 - blend) * stem_bin.re + blend * mix_bin.re,
                    (1.0 - blend) * stem_bin.im + blend * mix_bin.im,
                );
            }
        }

        // Reconstruct via ISTFT
        let reconstructed = compute_istft(&blended_stft, n_fft, hop, num_samples)?;

        // Write back to stereo interleaved format
        for i in 0..num_samples {
            stem[i * channels + ch] = reconstructed[i];
        }
    }

    log::info!("High-frequency preservation complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stft_istft_roundtrip() {
        // Generate test signal
        let num_samples = 4096;
        let signal: Vec<f32> = (0..num_samples)
            .map(|i| (i as f32 * 0.1).sin())
            .collect();

        // STFT
        let stft = compute_stft(&signal, POSTPROCESS_NFFT, POSTPROCESS_HOP).unwrap();

        // ISTFT
        let reconstructed = compute_istft(&stft, POSTPROCESS_NFFT, POSTPROCESS_HOP, num_samples).unwrap();

        // Check reconstruction (should be close, not exact due to windowing)
        let mut max_diff = 0.0f32;
        for i in POSTPROCESS_NFFT..num_samples - POSTPROCESS_NFFT {
            let diff = (signal[i] - reconstructed[i]).abs();
            max_diff = max_diff.max(diff);
        }

        assert!(
            max_diff < 0.01,
            "STFT/ISTFT roundtrip error too high: {}",
            max_diff
        );
    }
}
