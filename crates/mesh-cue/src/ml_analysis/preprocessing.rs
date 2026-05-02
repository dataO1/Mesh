//! Mel spectrogram preprocessing for MuQ-MuLan ML embedding inference.
//!
//! Reproduces the exact mel pipeline used to train MuQ-MuLan-large (the
//! `melspec_2048` feature in `muq.modules.features.MelSTFT`):
//!
//! * 24 kHz mono input (resample with anti-aliased low-pass first)
//! * `n_fft = 2048`, `hop_length = 240`, `win_length = 2048` (defaults)
//! * `power = 2.0` (squared magnitude — torchaudio's MelSpectrogram default)
//! * `center = True` with `pad_mode = "reflect"` (torchaudio default)
//! * `n_mels = 128`, HTK mel scale, `f_min = 0`, `f_max = sr/2 = 12000`,
//!   `norm = None` (no Slaney area normalization — torchaudio default)
//! * `amplitude_to_db` with default `top_db = 80` (set in MuQModel:
//!   `MelSTFT(... is_db=True)`)
//! * Trim the trailing frame (`out[key] = layer(x.float())[..., :-1]` in
//!   `MuQModel.preprocessing`) — keeps frame counts at exact multiples of
//!   the 10 s clip length.
//!
//! Per-clip normalization (`(x - mean) / std`) is applied by the caller in
//! `inference.rs` using stats loaded from the `*.norm.json` sidecar that
//! ships next to the ONNX. Keeping it out of this module lets the same
//! mel computation serve any future model that uses the same spectrogram
//! parameters.
//!
//! At 24 kHz, a 10 s clip produces `floor(240000 / 240) + 1 = 1001` frames
//! pre-trim, `1000` frames post-trim — matches the model's expected input
//! width.

use realfft::RealFftPlanner;
use serde::{Serialize, Deserialize};

// ─── tunables, in lock-step with MuQ's MelSTFT defaults ────────────────────
pub const MUQ_TARGET_SR: f32 = 24_000.0;
pub const MUQ_N_FFT: usize = 2048;
pub const MUQ_HOP: usize = 240;
pub const MUQ_N_MELS: usize = 128;
/// torchaudio's `AmplitudeToDB` default. Anything quieter than this in the
/// power spectrum gets clamped before the log so we don't hit `-inf`.
const AMP_TO_DB_AMIN_POWER: f32 = 1e-10;
/// torchaudio's `AmplitudeToDB(top_db=80.0)` default — the dB output is
/// floored at `(max - top_db)` so silent / near-silent frames don't pull
/// the model way out of its training distribution.
const AMP_TO_DB_TOP_DB: f32 = 80.0;

/// Mel spectrogram result for MuQ-MuLan ML inference.
///
/// Each frame is a 128-dimensional vector of dB-scale mel band energies
/// (post `AmplitudeToDB`, pre normalization). Frames are stored in time
/// order, ready to be sliced into 1000-frame clips for the encoder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MelSpectrogramResult {
    /// Mel spectrogram frames (each frame = 128 dB-scale mel band values).
    /// Frames are post-trim (the trailing edge frame is already dropped).
    pub frames: Vec<Vec<f32>>,
    /// Number of mel bands (always 128 for MuQ-MuLan).
    pub n_bands: usize,
    /// Sample rate used for computation (24000 Hz).
    pub sample_rate: u32,
}

/// Compute the MuQ-MuLan mel spectrogram from a mono signal at any rate.
///
/// Steps:
/// 1. Resample to 24 kHz mono (anti-aliased low-pass before decimation).
/// 2. Reflect-pad by `n_fft/2` on both sides (matches `center=True`).
/// 3. STFT at `n_fft=2048`, `hop=240`, Hann window, `power=2.0`.
/// 4. Apply 128-band HTK mel filterbank covering `[0, sr/2]` Hz.
/// 5. Convert to dB (`10 * log10(max(power, amin))`, top_db=80 floor).
/// 6. Drop the final frame to match the `[..., :-1]` slicing the model
///    was trained against.
///
/// The output is **not** normalized — apply the per-stat `(x - mean) / std`
/// from the model's `*.norm.json` sidecar before feeding the ONNX.
pub fn compute_mel_spectrogram(samples: &[f32], sample_rate: f32) -> Result<MelSpectrogramResult, String> {
    if samples.is_empty() {
        return Err("Empty input samples".to_string());
    }

    // Step 1: resample to 24 kHz with anti-aliasing for downsampling.
    let resampled = if (sample_rate - MUQ_TARGET_SR).abs() < 1.0 {
        samples.to_vec()
    } else {
        resample_with_anti_alias(samples, sample_rate, MUQ_TARGET_SR)
    };

    if resampled.len() < MUQ_N_FFT {
        return Err(format!(
            "Audio too short for MuQ-MuLan mel: got {} samples, need ≥ {} (one FFT frame)",
            resampled.len(), MUQ_N_FFT,
        ));
    }

    // Step 2: reflect-pad both ends by n_fft/2 to mimic `center=True`.
    // torchaudio's `center=True` shifts each frame so it's centered at
    // `i * hop` instead of starting there; reflect-padding by n_fft/2
    // makes the early/late frames align with the model's training-time
    // frame indexing.
    let pad = MUQ_N_FFT / 2;
    let padded = reflect_pad(&resampled, pad);

    // Frame count: matches torchaudio's `floor(N / hop) + 1` when center=True.
    // (N here is the *unpadded* signal length; the reflect padding is
    // implicit in `padded`.)
    let n_frames_pre_trim = resampled.len() / MUQ_HOP + 1;

    // Step 3 prep: mel filterbank, Hann window, FFT plan.
    let mel_filterbank = create_mel_filterbank(MUQ_N_MELS, MUQ_N_FFT, MUQ_TARGET_SR);
    let window = hann_window(MUQ_N_FFT);

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(MUQ_N_FFT);
    let mut scratch = fft.make_scratch_vec();

    // First pass: compute power-spectrum mel bands per frame.
    // Keep the raw amplitudes here so step-5 dB conversion can apply the
    // top_db floor relative to the *frame-max* exactly as torchaudio does.
    let mut power_frames: Vec<Vec<f32>> = Vec::with_capacity(n_frames_pre_trim);
    for frame_idx in 0..n_frames_pre_trim {
        let start = frame_idx * MUQ_HOP;
        let end = start + MUQ_N_FFT;
        if end > padded.len() {
            // Should be impossible given the padding; defensive only.
            break;
        }

        // Apply Hann window in place into a fresh buffer.
        let mut windowed = vec![0.0f32; MUQ_N_FFT];
        for (i, w) in window.iter().enumerate() {
            windowed[i] = padded[start + i] * w;
        }

        let spectrum_power = compute_power_spectrum(&windowed, fft.as_ref(), &mut scratch);

        // Apply mel filterbank → 128 raw mel powers (no dB yet).
        let mut mel_bands = vec![0.0f32; MUQ_N_MELS];
        for (band_idx, filter) in mel_filterbank.iter().enumerate() {
            let mut energy = 0.0f32;
            for (&coeff, &spec_val) in filter.iter().zip(spectrum_power.iter()) {
                energy += coeff * spec_val;
            }
            mel_bands[band_idx] = energy.max(0.0);
        }
        power_frames.push(mel_bands);
    }

    // Step 5: amplitude_to_db.
    // torchaudio applies a *global* top_db floor (relative to the running
    // max across the whole tensor it was given). We're computing one mel
    // tensor for the full track here and slicing later, so the global max
    // is over the whole track — same semantics as if torchaudio saw the
    // same tensor.
    let log_mul = 10.0f32; // power → dB uses multiplier 10
    let mut max_db = f32::NEG_INFINITY;
    let mut db_frames: Vec<Vec<f32>> = power_frames
        .into_iter()
        .map(|frame| {
            frame.into_iter()
                .map(|p| {
                    let db = log_mul * p.max(AMP_TO_DB_AMIN_POWER).log10();
                    if db > max_db {
                        max_db = db;
                    }
                    db
                })
                .collect()
        })
        .collect();
    if max_db.is_finite() {
        let floor = max_db - AMP_TO_DB_TOP_DB;
        for frame in &mut db_frames {
            for v in frame.iter_mut() {
                if *v < floor {
                    *v = floor;
                }
            }
        }
    }

    // Step 6: drop the trailing frame to match `[..., :-1]`.
    if !db_frames.is_empty() {
        db_frames.pop();
    }

    Ok(MelSpectrogramResult {
        frames: db_frames,
        n_bands: MUQ_N_MELS,
        sample_rate: MUQ_TARGET_SR as u32,
    })
}

// ─── helpers ───────────────────────────────────────────────────────────────

/// Reflect-pad `samples` by `pad` on both ends, matching numpy/torch's
/// `pad_mode="reflect"` (which excludes the boundary sample from the
/// reflection — element 0 is not duplicated).
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f32> {
    let n = samples.len();
    if pad == 0 {
        return samples.to_vec();
    }
    let mut out = Vec::with_capacity(n + 2 * pad);
    // Leading reflection: samples[1..=pad] reversed.
    for i in 0..pad {
        // Reflection index = pad - i (so it walks 1..=pad), but bounded by n.
        let src = (pad - i).min(n.saturating_sub(1));
        out.push(samples[src]);
    }
    out.extend_from_slice(samples);
    // Trailing reflection: samples[n-2 .. n-pad-1] (i.e., excluding the last sample).
    for i in 0..pad {
        let back = i + 2; // skip the final sample; first reflection is samples[n-2]
        let src = n.saturating_sub(back);
        out.push(samples[src]);
    }
    out
}

/// Resample with anti-aliasing low-pass for downsampling, linear interp for
/// upsampling. Identical algorithm to the legacy MAEST 16 kHz pipeline so
/// behaviour stays predictable across the codebase.
fn resample_with_anti_alias(samples: &[f32], from_sr: f32, to_sr: f32) -> Vec<f32> {
    let ratio = from_sr / to_sr;

    if ratio <= 1.01 {
        return resample_linear(samples, from_sr, to_sr);
    }

    let cutoff_freq = to_sr * 0.475 / from_sr;
    let filter_len = ((32.0 * ratio) as usize) | 1;
    let half = filter_len / 2;

    let mut filter: Vec<f32> = (0..filter_len)
        .map(|i| {
            let n = i as f32 - half as f32;
            let sinc = if n.abs() < 1e-6 {
                2.0 * cutoff_freq
            } else {
                (2.0 * std::f32::consts::PI * cutoff_freq * n).sin()
                    / (std::f32::consts::PI * n)
            };
            let w = 0.5
                * (1.0
                    - (2.0 * std::f32::consts::PI * i as f32 / (filter_len - 1) as f32).cos());
            sinc * w
        })
        .collect();

    let sum: f32 = filter.iter().sum();
    if sum.abs() > 1e-10 {
        for v in &mut filter {
            *v /= sum;
        }
    }

    let filtered: Vec<f32> = (0..samples.len())
        .map(|i| {
            let mut acc = 0.0f32;
            for (j, &coeff) in filter.iter().enumerate() {
                let idx = i as isize + j as isize - half as isize;
                if idx >= 0 && (idx as usize) < samples.len() {
                    acc += samples[idx as usize] * coeff;
                }
            }
            acc
        })
        .collect();

    resample_linear(&filtered, from_sr, to_sr)
}

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

fn hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| {
            let phase = 2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32;
            0.5 * (1.0 - phase.cos())
        })
        .collect()
}

/// Power spectrum (|X[k]|²) — matches torchaudio with `power=2.0` and
/// `normalized=False` (the MuQ MelSpectrogram defaults).
fn compute_power_spectrum(
    frame: &[f32],
    fft: &dyn realfft::RealToComplex<f32>,
    scratch: &mut [realfft::num_complex::Complex<f32>],
) -> Vec<f32> {
    let n = frame.len();
    let n_bins = n / 2 + 1;

    let mut input = frame.to_vec();
    let mut output = vec![realfft::num_complex::Complex::new(0.0f32, 0.0f32); n_bins];

    fft.process_with_scratch(&mut input, &mut output, scratch).ok();

    output
        .iter()
        .map(|c| c.re * c.re + c.im * c.im)
        .collect()
}

/// HTK mel filterbank — torchaudio's `MelScale(mel_scale="htk", norm=None)`.
fn create_mel_filterbank(n_bands: usize, frame_size: usize, sample_rate: f32) -> Vec<Vec<f32>> {
    let n_bins = frame_size / 2 + 1;
    let f_max = sample_rate / 2.0;

    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(f_max);

    let n_points = n_bands + 2;
    let mel_points: Vec<f32> = (0..n_points)
        .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (n_points - 1) as f32)
        .collect();

    let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_points: Vec<f32> = hz_points
        .iter()
        .map(|&hz| hz * frame_size as f32 / sample_rate)
        .collect();

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
        // 2 seconds of 440 Hz at 44.1 kHz → resampled to 24 kHz → ~200 frames.
        let sr = 44100.0;
        let samples: Vec<f32> = (0..(sr as usize * 2))
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin() * 0.5)
            .collect();

        let result = compute_mel_spectrogram(&samples, sr).unwrap();
        assert_eq!(result.n_bands, MUQ_N_MELS);
        assert_eq!(result.sample_rate, MUQ_TARGET_SR as u32);
        assert!(result.frames.len() > 100, "Should have many frames: {}", result.frames.len());
        assert_eq!(result.frames[0].len(), MUQ_N_MELS);
    }

    #[test]
    fn test_clip_frame_count_at_24khz() {
        // Exactly 10 seconds @ 24 kHz: 240000 samples → 1001 frames pre-trim,
        // 1000 frames post-trim. Matches the model's expected clip width.
        let sr = MUQ_TARGET_SR;
        let samples = vec![0.0_f32; (sr as usize) * 10];
        let result = compute_mel_spectrogram(&samples, sr).unwrap();
        assert_eq!(result.frames.len(), 1000, "10 s @ 24 kHz should produce 1000 mel frames");
    }

    #[test]
    fn test_empty_input_fails() {
        assert!(compute_mel_spectrogram(&[], 44100.0).is_err());
    }

    #[test]
    fn test_too_short_input_fails() {
        // Less than one FFT frame at 24 kHz.
        let short = vec![0.0f32; MUQ_N_FFT - 1];
        assert!(compute_mel_spectrogram(&short, MUQ_TARGET_SR).is_err());
    }

    #[test]
    fn test_reflect_pad_basic() {
        let s = vec![1.0_f32, 2.0, 3.0, 4.0, 5.0];
        let p = reflect_pad(&s, 2);
        // Leading: samples[1..=2] reversed → [3, 2]; then original; then [4, 3] (skipping last sample).
        assert_eq!(p, vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0]);
    }
}
