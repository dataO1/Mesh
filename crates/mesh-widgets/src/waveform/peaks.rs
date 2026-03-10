//! Peak generation utilities for waveform display
//!
//! These functions downsample audio data into min/max peak pairs
//! suitable for waveform visualization at various zoom levels.

use mesh_core::audio_file::StemBuffers;
use mesh_core::types::SAMPLE_RATE;

/// Default display width for peak computation (overview display)
pub const DEFAULT_WIDTH: usize = 800;

/// Reference zoom level for peak resolution targeting.
///
/// At this zoom level (in bars visible on screen), the target peaks-per-pixel
/// is exactly the quality level's base value (1/2/4/8). This should match the
/// closest practical zoom level — zooming closer than this provides oversampled
/// (sub-pixel) resolution for anti-aliasing, while zooming out doubles ppp each step.
pub const PEAK_REFERENCE_ZOOM_BARS: u32 = 4;

/// Compute the high-resolution peak width for exact peaks-per-pixel at the
/// reference zoom level ([`PEAK_REFERENCE_ZOOM_BARS`] = 4 bars).
///
/// Uses the track's original BPM and screen width to compute a peak count that
/// gives exactly `target_ppp` peaks per pixel at 4-bar zoom. Each quality level
/// doubles the target, and since zoom levels are powers of 2, peaks-per-pixel
/// is always a clean integer at every zoom level:
///
/// - 0 (Low):    1 pp/px at 4-bar, 2 at 8-bar, 4 at 16-bar, ...
/// - 1 (Medium): 2 pp/px at 4-bar, 4 at 8-bar, 8 at 16-bar, ...
/// - 2 (High):   4 pp/px at 4-bar, 8 at 8-bar, 16 at 16-bar, ...
/// - 3 (Ultra):  8 pp/px at 4-bar, 16 at 8-bar, 32 at 16-bar, ...
///
/// Below the reference zoom (2-bar, 1-bar), ppp goes sub-pixel (0.5, 0.25),
/// providing oversampled data for the shader's anti-aliasing.
///
/// The formula matches the shader's integer-truncated `samples_per_bar` so
/// peaks-per-pixel cancels exactly at render time.
///
/// Result is clamped to [1024, 8_388_608].
pub fn compute_highres_width(
    total_samples: usize,
    bpm: f64,
    screen_width: u32,
    quality_level: u8,
) -> usize {
    let target_ppp: usize = match quality_level {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => 2,
    };
    // Match the shader's integer truncation for samples_per_bar
    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as usize;
    let samples_per_bar = samples_per_beat * 4;
    if samples_per_bar == 0 {
        return 1024;
    }
    // Denominator includes reference zoom: at PEAK_REFERENCE_ZOOM_BARS bars visible,
    // the shader window spans (samples_per_bar * ref_zoom) samples, so we divide
    // by that to get exactly target_ppp peaks per pixel at that zoom level.
    let ref_zoom = PEAK_REFERENCE_ZOOM_BARS as f64;
    let raw = (target_ppp as f64 * total_samples as f64 * screen_width as f64
        / (samples_per_bar as f64 * ref_zoom))
        .round() as usize;
    raw.max(1024).min(8_388_608)
}

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

/// Gaussian smoothing weights for 5-sample window
///
/// Weights approximate a Gaussian distribution centered on middle sample.
/// Sum = 0.06 + 0.24 + 0.40 + 0.24 + 0.06 = 1.0 (no normalization needed)
const GAUSSIAN_WEIGHTS_5: [f32; 5] = [0.06, 0.24, 0.40, 0.24, 0.06];

/// Apply Gaussian-weighted smoothing to peaks (5-sample window)
///
/// Produces smoother results than simple moving average by weighting
/// the center sample more heavily. Uses 5-sample window for light smoothing.
///
/// Note: This reduces the array length by 4 (window_size - 1).
pub fn smooth_peaks_gaussian(peaks: &[(f32, f32)]) -> Vec<(f32, f32)> {
    const WINDOW_SIZE: usize = 5;

    if peaks.len() < WINDOW_SIZE {
        return peaks.to_vec();
    }

    peaks
        .windows(WINDOW_SIZE)
        .map(|w| {
            let mut min_sum = 0.0f32;
            let mut max_sum = 0.0f32;
            for (i, (min, max)) in w.iter().enumerate() {
                min_sum += min * GAUSSIAN_WEIGHTS_5[i];
                max_sum += max * GAUSSIAN_WEIGHTS_5[i];
            }
            (min_sum, max_sum)
        })
        .collect()
}

/// Allocate peak arrays pre-filled with (0.0, 0.0) for incremental loading.
///
/// Returns 4 arrays (one per stem) of the specified width, ready to be
/// incrementally updated via `update_peaks_for_region()`.
pub fn allocate_empty_peaks(width: usize) -> [Vec<(f32, f32)>; 4] {
    [
        vec![(0.0, 0.0); width],
        vec![(0.0, 0.0); width],
        vec![(0.0, 0.0); width],
        vec![(0.0, 0.0); width],
    ]
}

/// Allocate a flat peak buffer pre-filled with 0.0 for incremental loading.
///
/// Layout: stem-major interleaved min/max — `[s0_min0, s0_max0, s0_min1, ..., s1_min0, ...]`
/// Total size: `width * 4 stems * 2 (min+max)` f32 values.
pub fn allocate_flat_peaks(width: usize) -> Vec<f32> {
    vec![0.0; width * 4 * 2]
}

/// Update a flat peak buffer for a specific sample range.
///
/// Same logic as [`update_peaks_for_region`] but writes directly into the flat
/// stem-major layout used by the GPU shader, avoiding the intermediate tuple
/// format and the subsequent `from_stem_peaks()` conversion.
///
/// Layout: `data[(stem * peaks_per_stem + col) * 2]` = min,
///         `data[(stem * peaks_per_stem + col) * 2 + 1]` = max
pub fn update_peaks_for_region_flat(
    stems: &StemBuffers,
    data: &mut [f32],
    peaks_per_stem: u32,
    sample_start: usize,
    sample_end: usize,
    total_duration: usize,
) {
    let pps = peaks_per_stem as usize;
    if total_duration == 0 || pps == 0 {
        return;
    }

    let samples_per_col = total_duration / pps;
    if samples_per_col == 0 {
        return;
    }

    let col_start = sample_start / samples_per_col;
    let col_end = ((sample_end + samples_per_col - 1) / samples_per_col).min(pps);

    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let stem_len = stems.len();

    for (stem_idx, stem_buf) in stem_refs.iter().enumerate() {
        let stem_offset = stem_idx * pps * 2;

        for col in col_start..col_end {
            let s = col * samples_per_col;
            let e = ((col + 1) * samples_per_col).min(stem_len);

            if s >= stem_len || s >= e {
                continue;
            }

            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;

            for i in s..e {
                let sample = (stem_buf[i].left + stem_buf[i].right) / 2.0;
                min = min.min(sample);
                max = max.max(sample);
            }

            let offset = stem_offset + col * 2;
            if min == f32::INFINITY {
                data[offset] = 0.0;
                data[offset + 1] = 0.0;
            } else {
                data[offset] = min;
                data[offset + 1] = max;
            }
        }
    }
}

/// Update pre-allocated peak arrays for a specific sample range.
///
/// Given a sample range `[sample_start, sample_end)`, computes which peak
/// columns are affected and recalculates min/max for those columns from the
/// stem data. Unloaded columns remain at `(0.0, 0.0)` — rendered as flat/silent.
///
/// This is designed for incremental loading: call after reading each batch of
/// audio data into the stem buffer. Only processes the affected columns, so
/// a 1.5M-sample batch across 4 stems completes in <5 ms.
///
/// # Arguments
/// * `stems` - The stem buffers (may be partially filled with audio data)
/// * `peaks` - Pre-allocated peak arrays to update in-place
/// * `sample_start` - Start of the loaded sample range (inclusive)
/// * `sample_end` - End of the loaded sample range (exclusive)
/// * `total_duration` - Total track duration in samples
/// * `total_width` - Width of the peak arrays (e.g., 800 for overview, 65536 for highres)
pub fn update_peaks_for_region(
    stems: &StemBuffers,
    peaks: &mut [Vec<(f32, f32)>; 4],
    sample_start: usize,
    sample_end: usize,
    total_duration: usize,
    total_width: usize,
) {
    if total_duration == 0 || total_width == 0 {
        return;
    }

    let samples_per_col = total_duration / total_width;
    if samples_per_col == 0 {
        return;
    }

    // Determine which columns are affected by this sample range
    let col_start = sample_start / samples_per_col;
    let col_end = ((sample_end + samples_per_col - 1) / samples_per_col).min(total_width);

    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let stem_len = stems.len();

    for (stem_idx, stem_buf) in stem_refs.iter().enumerate() {
        if peaks[stem_idx].len() < total_width {
            continue; // Skip if peaks array not properly allocated
        }

        for col in col_start..col_end {
            let s = col * samples_per_col;
            let e = ((col + 1) * samples_per_col).min(stem_len);

            if s >= stem_len || s >= e {
                continue;
            }

            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;

            for i in s..e {
                let sample = (stem_buf[i].left + stem_buf[i].right) / 2.0;
                min = min.min(sample);
                max = max.max(sample);
            }

            if min == f32::INFINITY {
                peaks[stem_idx][col] = (0.0, 0.0);
            } else {
                peaks[stem_idx][col] = (min, max);
            }
        }
    }
}
