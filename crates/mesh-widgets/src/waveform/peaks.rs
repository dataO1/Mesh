//! Peak generation utilities for waveform display
//!
//! These functions downsample audio data into min/max peak pairs
//! suitable for waveform visualization at various zoom levels.

use mesh_core::audio_file::{quantize_peak, StemBuffers, StemPeaks, WaveformPreview};

/// Default display width for peak computation
pub const DEFAULT_WIDTH: usize = 800;

/// Smoothing window size for peaks (moving average)
pub const PEAK_SMOOTHING_WINDOW: usize = 3;

/// Generate peak data for all stems across the full track
///
/// Downsamples the audio to one min/max pair per pixel column.
/// Returns 4 arrays of (min, max) pairs, one per stem (Vocals, Drums, Bass, Other).
pub fn generate_peaks(stems: &StemBuffers, width: usize) -> [Vec<(f32, f32)>; 4] {
    let len = stems.len();
    if len == 0 || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let samples_per_column = len / width;
    if samples_per_column == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    // Get references to each stem buffer
    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];

    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (stem_idx, stem_buffer) in stem_refs.iter().enumerate() {
        result[stem_idx] = (0..width)
            .map(|col| {
                let start = col * samples_per_column;
                let end = ((col + 1) * samples_per_column).min(len);

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in start..end {
                    // Convert stereo to mono by averaging
                    let sample = (stem_buffer[i].left + stem_buffer[i].right) / 2.0;
                    min = min.min(sample);
                    max = max.max(sample);
                }

                (min, max)
            })
            .collect();
    }

    result
}

/// Generate peak data for a specific sample range
///
/// Used for zoomed waveform views where only a portion of the track is visible.
/// Returns 4 arrays of (min, max) pairs for the specified range.
///
/// Handles ranges that extend beyond track bounds (at start or end) by treating
/// out-of-bounds samples as silence (zeros). This enables symmetric views at
/// track boundaries.
pub fn generate_peaks_for_range(
    stems: &StemBuffers,
    start_sample: u64,
    end_sample: u64,
    width: usize,
) -> [Vec<(f32, f32)>; 4] {
    let len = stems.len();

    // Calculate the total range (may extend beyond actual data)
    let range_len = end_sample.saturating_sub(start_sample) as usize;
    if range_len == 0 || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let samples_per_column = range_len / width;
    if samples_per_column == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (stem_idx, stem_buffer) in stem_refs.iter().enumerate() {
        result[stem_idx] = (0..width)
            .map(|col| {
                // Calculate column range in the virtual window (may be out of bounds)
                let col_start_virtual = start_sample as usize + col * samples_per_column;
                let col_end_virtual = start_sample as usize + (col + 1) * samples_per_column;

                // Clamp to actual data bounds
                let col_start = col_start_virtual.min(len);
                let col_end = col_end_virtual.min(len);

                // If entire column is out of bounds, return silence
                if col_start >= len || col_start >= col_end {
                    return (0.0, 0.0);
                }

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in col_start..col_end {
                    let sample = (stem_buffer[i].left + stem_buffer[i].right) / 2.0;
                    min = min.min(sample);
                    max = max.max(sample);
                }

                if min == f32::INFINITY {
                    (0.0, 0.0)
                } else {
                    (min, max)
                }
            })
            .collect();
    }

    result
}

/// Apply moving average smoothing to peaks
///
/// Reduces visual noise in the waveform by averaging adjacent peak values.
/// Note: This reduces the array length by (window_size - 1).
pub fn smooth_peaks(peaks: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if peaks.len() < PEAK_SMOOTHING_WINDOW {
        return peaks.to_vec();
    }

    peaks
        .windows(PEAK_SMOOTHING_WINDOW)
        .map(|w| {
            let min_avg = w.iter().map(|(m, _)| m).sum::<f32>() / PEAK_SMOOTHING_WINDOW as f32;
            let max_avg = w.iter().map(|(_, m)| m).sum::<f32>() / PEAK_SMOOTHING_WINDOW as f32;
            (min_avg, max_avg)
        })
        .collect()
}

/// Gaussian smoothing weights for 5-sample window
///
/// Weights are designed to sum to 1.0 for normalized output.
/// Center sample gets highest weight (0.40), with symmetric falloff.
const GAUSSIAN_WEIGHTS: [f32; 5] = [0.06, 0.24, 0.40, 0.24, 0.06];

/// Apply Gaussian-weighted smoothing to peaks
///
/// Produces smoother results than simple moving average by weighting
/// the center sample more heavily. This preserves peaks better while
/// still reducing noise in the waveform visualization.
///
/// Note: This reduces the array length by 4 (window_size - 1).
pub fn smooth_peaks_gaussian(peaks: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if peaks.len() < 5 {
        return peaks.to_vec();
    }

    peaks
        .windows(5)
        .map(|w| {
            let mut min_sum = 0.0f32;
            let mut max_sum = 0.0f32;
            for (i, (min, max)) in w.iter().enumerate() {
                min_sum += min * GAUSSIAN_WEIGHTS[i];
                max_sum += max * GAUSSIAN_WEIGHTS[i];
            }
            (min_sum, max_sum)
        })
        .collect()
}

/// Generate a waveform preview for storage in WAV file
///
/// Creates a quantized preview at the standard width (800 pixels)
/// that can be stored in the wvfm chunk for instant display on load.
pub fn generate_waveform_preview(stems: &StemBuffers) -> WaveformPreview {
    let width = WaveformPreview::STANDARD_WIDTH as usize;

    // Generate peaks like the regular generate_peaks function
    // Note: We skip smoothing here - at 800 pixels the resolution is already
    // low enough, and smooth_peaks() reduces array length by (window_size - 1),
    // causing a mismatch between the width field and actual data length.
    let peaks = generate_peaks(stems, width);

    // Convert to quantized StemPeaks
    let mut preview = WaveformPreview {
        width: width as u16,
        stems: Default::default(),
    };

    for (stem_idx, stem_peaks) in peaks.iter().enumerate() {
        let mut min_values = Vec::with_capacity(stem_peaks.len());
        let mut max_values = Vec::with_capacity(stem_peaks.len());

        for &(min, max) in stem_peaks {
            min_values.push(quantize_peak(min));
            max_values.push(quantize_peak(max));
        }

        preview.stems[stem_idx] = StemPeaks {
            min: min_values,
            max: max_values,
        };
    }

    log::debug!(
        "Generated waveform preview: {}px width, {} samples per stem",
        preview.width,
        preview.stems[0].min.len()
    );

    preview
}
