//! Unified peak computation for waveform displays
//!
//! This module provides a single source of truth for all waveform peak calculations.
//! It handles window calculation, resolution scaling, and peak generation with proper
//! boundary padding for track start/end.
//!
//! # Architecture
//!
//! The UI provides:
//! - Stem audio buffers
//! - Playhead position
//! - Zoom level / buffer bounds
//! - View mode (scrolling vs fixed)
//!
//! This module handles:
//! - Window calculation with boundary padding
//! - Cache sizing and margins
//! - Resolution scaling based on zoom/mode
//! - Peak generation with zero-padding for boundaries

use mesh_core::audio_file::StemBuffers;
use mesh_core::types::{StereoBuffer, SAMPLE_RATE};

use super::state::ZoomedViewMode;
use super::peaks::smooth_peaks_gaussian;

// =============================================================================
// Constants
// =============================================================================

/// Default zoom level in bars (8 bars = 32 beats)
pub const DEFAULT_ZOOM_BARS: u32 = 8;

/// Minimum zoom level (1 bar = very zoomed in)
pub const MIN_ZOOM_BARS: u32 = 1;

/// Maximum zoom level (64 bars = very zoomed out)
pub const MAX_ZOOM_BARS: u32 = 64;

// =============================================================================
// Window Information
// =============================================================================

/// Result of window calculation with boundary padding info
///
/// This struct captures the actual sample range plus any "virtual" padding
/// needed for track boundaries. When playhead is near position 0, `left_padding`
/// indicates how many samples of silence should be prepended to center the playhead.
#[derive(Debug, Clone, Copy)]
pub struct WindowInfo {
    /// Actual start sample (clamped to 0)
    pub start: u64,
    /// Actual end sample (may exceed track duration - handled as zeros)
    pub end: u64,
    /// Virtual samples before track start (for centering at position 0)
    pub left_padding: u64,
    /// Total window size in samples (always consistent regardless of boundaries)
    pub total_samples: u64,
}

impl WindowInfo {
    /// Create a window for scrolling mode (centered on playhead)
    ///
    /// The window is always `total_samples` wide in "virtual" space, but the
    /// actual audio range (`start` to `end`) may be smaller when near track
    /// boundaries:
    ///
    /// ```text
    /// At track start (playhead=0):
    ///   |<-- left_padding -->|<-- actual audio -->|
    ///   |----- total_samples (full window) -------|
    ///                        ^
    ///                    playhead (center)
    /// ```
    pub fn scrolling(playhead: u64, zoom_bars: u32, bpm: f64) -> Self {
        let window_samples = samples_per_bar(bpm) * zoom_bars as u64;
        let half_window = window_samples / 2;

        // Calculate virtual start/end (may be negative conceptually)
        let virtual_start = playhead as i64 - half_window as i64;
        let virtual_end = playhead as i64 + half_window as i64;

        // Clamp to valid sample range (actual audio data)
        let start = virtual_start.max(0) as u64;
        let end = virtual_end.max(0) as u64;

        // Padding needed for samples "before" position 0
        let left_padding = (-virtual_start).max(0) as u64;

        Self {
            start,
            end,
            left_padding,
            total_samples: window_samples,
        }
    }

    /// Create a window for fixed buffer mode (slicer)
    pub fn fixed_buffer(buffer_start: u64, buffer_end: u64) -> Self {
        let total_samples = buffer_end.saturating_sub(buffer_start);
        Self {
            start: buffer_start,
            end: buffer_end,
            left_padding: 0, // Fixed buffers don't need padding
            total_samples,
        }
    }

    /// Create a window based on view mode and parameters
    pub fn compute(
        playhead: u64,
        zoom_bars: u32,
        bpm: f64,
        view_mode: ZoomedViewMode,
        fixed_bounds: Option<(u64, u64)>,
    ) -> Self {
        match view_mode {
            ZoomedViewMode::Scrolling => Self::scrolling(playhead, zoom_bars, bpm),
            ZoomedViewMode::FixedBuffer => {
                if let Some((start, end)) = fixed_bounds {
                    Self::fixed_buffer(start, end)
                } else {
                    // Fall back to scrolling behavior if no bounds
                    Self::scrolling(playhead, zoom_bars, bpm)
                }
            }
        }
    }

    /// Get the range for actual audio data (excluding padding)
    pub fn data_range(&self) -> (u64, u64) {
        (self.start, self.end)
    }
}

// =============================================================================
// Cache Information
// =============================================================================

/// Cache window sizing for background computation
#[derive(Debug, Clone, Copy)]
pub struct CacheInfo {
    /// Start of cache window
    pub start: u64,
    /// End of cache window
    pub end: u64,
    /// Left padding from original window
    pub left_padding: u64,
}

impl CacheInfo {
    /// Compute cache bounds for a visible window
    ///
    /// In scrolling mode, caches slightly more than visible window (5% padding each side)
    /// In fixed buffer mode, caches exactly the visible range
    pub fn from_window(window: &WindowInfo, view_mode: ZoomedViewMode) -> Self {
        match view_mode {
            ZoomedViewMode::Scrolling => {
                // EXPERIMENTAL: Minimal cache - just 5% padding on each side
                // This forces frequent recomputation for testing
                let padding = window.total_samples / 20; // 5% of visible window
                let cache_start = window.start.saturating_sub(padding);
                let cache_end = window.end + padding;

                Self {
                    start: cache_start,
                    end: cache_end,
                    left_padding: window.left_padding,
                }
            }
            ZoomedViewMode::FixedBuffer => {
                // Cache exactly the visible range
                Self {
                    start: window.start,
                    end: window.end,
                    left_padding: 0,
                }
            }
        }
    }

    /// Check if a window is within this cache (with margin)
    pub fn contains_with_margin(&self, window: &WindowInfo) -> bool {
        // With full-track caching, just check if window is within cache bounds
        // No margin needed since we cache the entire track
        window.start >= self.start && window.end <= self.end
    }
}

// =============================================================================
// Resolution Calculation
// =============================================================================

/// Calculate effective width (resolution) for peak generation
///
/// Resolution scaling depends on view mode:
/// - **Scrolling**: Aggressively reduces resolution when zoomed out to prevent
///   overlapping lines from causing visual jiggling
/// - **FixedBuffer**: Uses 80% of base width (slightly reduced for slicer)
pub fn compute_effective_width(base_width: usize, zoom_bars: u32, view_mode: ZoomedViewMode) -> usize {
    match view_mode {
        ZoomedViewMode::Scrolling => {
            // Linear resolution scaling based on zoom level:
            // - 1 bar (very zoomed in): 1280 pixels
            // - 64 bars (very zoomed out): 256 pixels
            // Fewer peaks when zoomed out = less overlapping lines = less jiggling
            const MAX_RES: f64 = 1280.0;
            const MIN_RES: f64 = 256.0;
            const MIN_ZOOM: f64 = 1.0;
            const MAX_ZOOM: f64 = 64.0;

            // Linear interpolation: lerp from MAX_RES to MIN_RES as zoom goes 1→64
            let t = ((zoom_bars as f64) - MIN_ZOOM) / (MAX_ZOOM - MIN_ZOOM);
            let t = t.clamp(0.0, 1.0);
            let effective = MAX_RES - t * (MAX_RES - MIN_RES);
            (effective as usize).max(256)
        }
        ZoomedViewMode::FixedBuffer => {
            // Slicer mode: use 80% of base width
            ((base_width as f64) * 0.8) as usize
        }
    }
}

// =============================================================================
// Peak Generation
// =============================================================================

/// Generate peaks for a window with proper boundary handling
///
/// This function handles:
/// - Left padding: Prepends zeros for samples "before" track start
/// - Right padding: Treats samples beyond track duration as zeros
/// - Always returns exactly `width` peaks
pub fn generate_peaks_with_padding(
    stems: &StemBuffers,
    window: &WindowInfo,
    width: usize,
) -> [Vec<(f32, f32)>; 4] {
    if window.total_samples == 0 || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let stem_len = stems.len();
    // total_samples already represents the full virtual window size (including padding conceptually)
    let total_virtual_samples = window.total_samples as usize;

    // Early exit if samples < width (can't have sub-sample resolution)
    if total_virtual_samples < width {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (stem_idx, stem_buffer) in stem_refs.iter().enumerate() {
        result[stem_idx] = (0..width)
            .map(|col| {
                // Bresenham-style integer division: distributes remainder evenly
                // col * total / width gives deterministic boundaries with no lost samples
                let virtual_col_start = col * total_virtual_samples / width;
                let virtual_col_end = (col + 1) * total_virtual_samples / width;

                // If entirely in left padding region, return silence
                if virtual_col_end <= window.left_padding as usize {
                    return (0.0, 0.0);
                }

                // Calculate actual sample positions (subtract padding)
                let actual_col_start = virtual_col_start.saturating_sub(window.left_padding as usize);
                let actual_col_end = virtual_col_end.saturating_sub(window.left_padding as usize);

                // Map to stem buffer indices
                let data_start = (window.start as usize + actual_col_start).min(stem_len);
                let data_end = (window.start as usize + actual_col_end).min(stem_len);

                // If entirely beyond track data, return silence
                if data_start >= stem_len || data_start >= data_end {
                    return (0.0, 0.0);
                }

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in data_start..data_end {
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

/// Generate peaks for a cache window (used by background thread)
///
/// Similar to `generate_peaks_with_padding` but for larger cache regions.
pub fn generate_peaks_for_cache(
    stems: &StemBuffers,
    cache: &CacheInfo,
    width: usize,
) -> [Vec<(f32, f32)>; 4] {
    let window = WindowInfo {
        start: cache.start,
        end: cache.end,
        left_padding: cache.left_padding,
        total_samples: cache.end - cache.start,
    };

    generate_peaks_with_padding(stems, &window, width)
}

/// Apply Gaussian smoothing to computed peaks
pub fn smooth_peaks(peaks: &mut [Vec<(f32, f32)>; 4]) {
    for stem_idx in 0..4 {
        if peaks[stem_idx].len() >= 5 {
            peaks[stem_idx] = smooth_peaks_gaussian(&peaks[stem_idx]);
        }
    }
}

/// Generate peaks with support for linked stem buffers
///
/// For stems where `linked_active[i]` is true and `linked_stems[i]` is Some,
/// uses the linked buffer instead of the host buffer for peak generation.
/// This allows the zoomed waveform to display the currently playing audio
/// regardless of whether it's the original stem or a linked stem.
pub fn generate_peaks_with_padding_and_linked(
    stems: &StemBuffers,
    window: &WindowInfo,
    width: usize,
    linked_stems: &[Option<&StereoBuffer>; 4],
    linked_active: &[bool; 4],
) -> [Vec<(f32, f32)>; 4] {
    if window.total_samples == 0 || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let stem_len = stems.len();
    let total_virtual_samples = window.total_samples as usize;

    if total_virtual_samples < width {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let host_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for stem_idx in 0..4 {
        // Choose between linked and host buffer
        let (buffer, buffer_len) = if linked_active[stem_idx] {
            if let Some(linked_buffer) = linked_stems[stem_idx] {
                (linked_buffer.as_slice(), linked_buffer.len())
            } else {
                (host_refs[stem_idx].as_slice(), stem_len)
            }
        } else {
            (host_refs[stem_idx].as_slice(), stem_len)
        };

        result[stem_idx] = (0..width)
            .map(|col| {
                let virtual_col_start = col * total_virtual_samples / width;
                let virtual_col_end = (col + 1) * total_virtual_samples / width;

                // If entirely in left padding region, return silence
                if virtual_col_end <= window.left_padding as usize {
                    return (0.0, 0.0);
                }

                // Calculate actual sample positions (subtract padding)
                let actual_col_start = virtual_col_start.saturating_sub(window.left_padding as usize);
                let actual_col_end = virtual_col_end.saturating_sub(window.left_padding as usize);

                // Map to buffer indices
                let data_start = (window.start as usize + actual_col_start).min(buffer_len);
                let data_end = (window.start as usize + actual_col_end).min(buffer_len);

                // If entirely beyond track data, return silence
                if data_start >= buffer_len || data_start >= data_end {
                    return (0.0, 0.0);
                }

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in data_start..data_end {
                    let sample = (buffer[i].left + buffer[i].right) / 2.0;
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

// =============================================================================
// Helper Functions
// =============================================================================

/// Calculate samples per bar at the given BPM
pub fn samples_per_bar(bpm: f64) -> u64 {
    let beats_per_bar = 4;
    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
    samples_per_beat * beats_per_bar
}

/// Scale peaks by gain compensation (for LUFS-normalized zoomed waveform display)
///
/// Applies a linear gain multiplier to all peak values in the array.
/// Used at runtime to match the zoomed waveform display to the current target LUFS.
///
/// # Arguments
/// * `peaks` - Mutable peak arrays for all 4 stems [Vocals, Drums, Bass, Other]
/// * `gain_linear` - Linear gain multiplier (1.0 = unity, calculated from LUFS)
///
/// # Example
/// ```ignore
/// let gain = config.loudness.calculate_gain_linear(track_lufs);
/// scale_peaks_by_gain(&mut peaks, gain);
/// ```
pub fn scale_peaks_by_gain(peaks: &mut [Vec<(f32, f32)>; 4], gain_linear: f32) {
    // Skip if unity gain (avoid unnecessary multiplication)
    if (gain_linear - 1.0).abs() < 0.001 {
        return;
    }

    for stem_peaks in peaks.iter_mut() {
        for (min, max) in stem_peaks.iter_mut() {
            *min = (*min * gain_linear).clamp(-1.0, 1.0);
            *max = (*max * gain_linear).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_at_track_start() {
        // At playhead=0, window should have left_padding for centering
        // 8 bars at 120 BPM = 768000 samples total, half_window = 384000
        let window = WindowInfo::scrolling(0, 8, 120.0);

        // Verify the window structure at track start
        assert_eq!(window.start, 0, "Start should be clamped to 0");
        assert_eq!(window.left_padding, 384000, "Left padding should be half_window");
        assert_eq!(window.end, 384000, "End should be playhead + half_window");
        assert_eq!(window.total_samples, 768000, "Total samples should be full window");

        // Verify: left_padding + (end - start) = total_samples
        assert_eq!(
            window.left_padding + (window.end - window.start),
            window.total_samples,
            "Padding + actual data should equal total window"
        );
    }

    #[test]
    fn test_window_in_middle() {
        // In middle of track, no padding needed
        let window = WindowInfo::scrolling(1_000_000, 8, 120.0);

        assert_eq!(window.left_padding, 0, "No padding needed in middle of track");
        assert!(window.start > 0, "Start should be positive");
    }

    #[test]
    fn test_fixed_buffer_no_padding() {
        let window = WindowInfo::fixed_buffer(100_000, 500_000);

        assert_eq!(window.left_padding, 0, "Fixed buffer should never have padding");
        assert_eq!(window.start, 100_000);
        assert_eq!(window.end, 500_000);
    }

    #[test]
    fn test_cache_contains_margin() {
        // Test full-track caching: window should be contained if within cache bounds
        let window = WindowInfo::scrolling(5_000_000, 8, 120.0);

        // Simulate full-track cache (0 to 10M samples)
        let full_track_cache = CacheInfo {
            start: 0,
            end: 10_000_000,
            left_padding: 0,
        };

        // Window should be within full-track cache
        assert!(full_track_cache.contains_with_margin(&window),
            "Window should be within full-track cache");

        // Window outside cache should not be contained
        let small_cache = CacheInfo {
            start: 1_000_000,
            end: 2_000_000,
            left_padding: 0,
        };
        assert!(!small_cache.contains_with_margin(&window),
            "Window outside cache bounds should not be contained");
    }
}
