//! Waveform state structures for iced canvas-based waveform widgets
//!
//! These structures hold the data for waveform visualization, separate from
//! rendering logic. Following iced 0.14 patterns, state lives at the application
//! level while view functions consume references to generate UI elements.

use super::CueMarker;
use crate::CUE_COLORS;
use mesh_core::audio_file::{dequantize_peak, CuePoint, LoadedTrack, StemBuffers, WaveformPreview};
use mesh_core::types::SAMPLE_RATE;
use std::sync::Arc;

use super::{generate_peaks, generate_peaks_for_range, smooth_peaks, DEFAULT_WIDTH, PEAK_SMOOTHING_WINDOW};

// =============================================================================
// Configuration Constants
// =============================================================================

/// Overview waveform height in pixels (compact)
pub const WAVEFORM_HEIGHT: f32 = 75.0;

/// Zoomed waveform height in pixels (detailed, larger)
pub const ZOOMED_WAVEFORM_HEIGHT: f32 = 240.0;

/// Gap between zoomed and overview waveforms in combined view
pub const COMBINED_WAVEFORM_GAP: f32 = 10.0;

/// Minimum zoom level in bars
pub const MIN_ZOOM_BARS: u32 = 1;

/// Maximum zoom level in bars
pub const MAX_ZOOM_BARS: u32 = 64;

/// Default zoom level in bars
pub const DEFAULT_ZOOM_BARS: u32 = 8;

/// Pixels of drag movement per zoom level change
pub const ZOOM_PIXELS_PER_LEVEL: f32 = 20.0;

// =============================================================================
// Overview Waveform State
// =============================================================================

/// Overview waveform state (full track view)
///
/// Contains all data needed to render an overview waveform:
/// - Pre-computed stem peaks for fast rendering
/// - Beat grid markers
/// - Cue point markers
/// - Playhead position
///
/// This is pure data with builder methods - rendering is handled by view functions.
#[derive(Debug, Clone)]
pub struct OverviewState {
    /// Cached waveform data per stem (min/max pairs per column)
    pub stem_waveforms: [Vec<(f32, f32)>; 4],
    /// Current playhead position (0.0 to 1.0)
    pub position: f64,
    /// Current main cue point position (0.0 to 1.0), None if not set
    pub cue_position: Option<f64>,
    /// Beat grid positions (normalized 0.0 to 1.0)
    pub beat_markers: Vec<f64>,
    /// Cue point markers
    pub cue_markers: Vec<CueMarker>,
    /// Track duration in samples
    pub duration_samples: u64,
    /// Track loaded
    pub has_track: bool,
    /// Audio is loading (show placeholder)
    pub loading: bool,
    /// Missing preview message (when no waveform data available)
    pub missing_preview_message: Option<String>,
    /// Grid density for overview (bars between major grid lines: 4, 8, 16, 32)
    pub grid_bars: u32,
}

impl OverviewState {
    /// Create a new empty waveform state
    pub fn new() -> Self {
        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            has_track: false,
            loading: false,
            missing_preview_message: None,
            grid_bars: 8, // Default: show grid every 8 bars
        }
    }

    /// Set the main cue point position (normalized 0.0 to 1.0)
    pub fn set_cue_position(&mut self, position: Option<f64>) {
        self.cue_position = position;
    }

    /// Set the grid density (bars between major grid lines)
    pub fn set_grid_bars(&mut self, bars: u32) {
        self.grid_bars = bars.clamp(4, 32);
    }

    /// Create from a cached waveform preview
    ///
    /// This provides instant waveform display without recomputing from stems.
    pub fn from_preview(
        preview: &WaveformPreview,
        beat_grid: &[u64],
        cue_points: &[CuePoint],
        duration_samples: u64,
    ) -> Self {
        // Dequantize the preview data back to f32 peaks
        let mut stem_waveforms: [Vec<(f32, f32)>; 4] =
            [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

        for (stem_idx, stem_peaks) in preview.stems.iter().enumerate() {
            let peaks: Vec<(f32, f32)> = stem_peaks
                .min
                .iter()
                .zip(stem_peaks.max.iter())
                .map(|(&min, &max)| (dequantize_peak(min), dequantize_peak(max)))
                .collect();
            stem_waveforms[stem_idx] = peaks;
        }

        // Convert beat grid to normalized positions
        let beat_markers: Vec<f64> = if duration_samples > 0 {
            beat_grid
                .iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect()
        } else {
            Vec::new()
        };

        // Convert cue points to markers
        let cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);

        log::debug!(
            "Created OverviewState from preview: {} peaks, {} cue markers",
            stem_waveforms[0].len(),
            cue_markers.len()
        );

        Self {
            stem_waveforms,
            position: 0.0,
            cue_position: None,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: None,
            grid_bars: 8,
        }
    }

    /// Create an empty state with a message explaining why data is missing
    ///
    /// Used when a track doesn't have a cached waveform preview (needs re-analysis).
    pub fn empty_with_message(message: &str, cue_points: &[CuePoint], duration_samples: u64) -> Self {
        let cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);

        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers: Vec::new(),
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: Some(message.to_string()),
            grid_bars: 8,
        }
    }

    /// Create a placeholder from metadata only (no audio data yet)
    ///
    /// Shows cue markers while audio loads in background.
    pub fn from_metadata(metadata: &mesh_core::audio_file::TrackMetadata) -> Self {
        let cue_markers: Vec<CueMarker> = metadata
            .cue_points
            .iter()
            .map(|cue| {
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position: 0.0, // Will be normalized when duration is known
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();

        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers: Vec::new(),
            cue_markers,
            duration_samples: 0,
            has_track: true,
            loading: true,
            missing_preview_message: None,
            grid_bars: 8,
        }
    }

    /// Set stems and generate waveform data (called when audio finishes loading)
    pub fn set_stems(
        &mut self,
        stems: &StemBuffers,
        cue_points: &[CuePoint],
        beat_grid: &[u64],
    ) {
        let duration_samples = stems.len() as u64;
        self.duration_samples = duration_samples;
        self.loading = false;

        // Generate peak data for each stem
        self.stem_waveforms = generate_peaks(stems, DEFAULT_WIDTH);

        // Convert beat grid to normalized positions
        if duration_samples > 0 {
            self.beat_markers = beat_grid
                .iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect();

            // Update cue markers with correct normalized positions
            self.cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);
        }
    }

    /// Create from a loaded track
    pub fn from_track(track: &Arc<LoadedTrack>, cue_points: &[CuePoint]) -> Self {
        let duration_samples = track.duration_samples as u64;

        // Generate peak data for each stem
        let stem_waveforms = generate_peaks(&track.stems, DEFAULT_WIDTH);

        // Convert beat grid to normalized positions
        let beat_markers: Vec<f64> = track
            .metadata
            .beat_grid
            .beats
            .iter()
            .map(|&pos| pos as f64 / duration_samples as f64)
            .collect();

        let cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);

        Self {
            stem_waveforms,
            position: 0.0,
            cue_position: None,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: None,
            grid_bars: 8,
        }
    }

    /// Update playhead position
    pub fn set_position(&mut self, position: f64) {
        self.position = position.clamp(0.0, 1.0);
    }

    /// Update cue markers (when cue points are edited)
    pub fn update_cue_markers(&mut self, cue_points: &[CuePoint]) {
        if self.duration_samples == 0 {
            return;
        }
        self.cue_markers = Self::cue_points_to_markers(cue_points, self.duration_samples);
    }

    /// Convert cue points to display markers
    fn cue_points_to_markers(cue_points: &[CuePoint], duration_samples: u64) -> Vec<CueMarker> {
        cue_points
            .iter()
            .map(|cue| {
                let position = if duration_samples > 0 {
                    cue.sample_position as f64 / duration_samples as f64
                } else {
                    0.0
                };
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position,
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect()
    }
}

impl Default for OverviewState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Zoomed Waveform State
// =============================================================================

/// Zoomed waveform state (detail view centered on playhead)
///
/// Contains data for rendering a zoomed-in view of the waveform:
/// - Cached peak data for the visible window
/// - Zoom level and visible range calculation
/// - Beat grid and cue markers (in sample positions)
///
/// Supports zoom levels from 1 to 64 bars via click+drag gesture.
#[derive(Debug, Clone)]
pub struct ZoomedState {
    /// Cached peak data for visible window [stem_idx] = Vec<(min, max)>
    pub cached_peaks: [Vec<(f32, f32)>; 4],
    /// Start sample of cached window
    pub cache_start: u64,
    /// End sample of cached window
    pub cache_end: u64,
    /// Current zoom level in bars (1-64)
    pub zoom_bars: u32,
    /// Beat grid positions in samples
    pub beat_grid: Vec<u64>,
    /// Cue markers with sample positions
    pub cue_markers: Vec<CueMarker>,
    /// Track duration in samples
    pub duration_samples: u64,
    /// Detected BPM (for bar calculation)
    pub bpm: f64,
    /// Whether the view has valid data
    pub has_track: bool,
}

impl ZoomedState {
    /// Create a new empty zoomed waveform state
    pub fn new() -> Self {
        Self {
            cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cache_start: 0,
            cache_end: 0,
            zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            bpm: 120.0,
            has_track: false,
        }
    }

    /// Create from track metadata
    pub fn from_metadata(bpm: f64, beat_grid: Vec<u64>, cue_markers: Vec<CueMarker>) -> Self {
        Self {
            cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cache_start: 0,
            cache_end: 0,
            zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid,
            cue_markers,
            duration_samples: 0,
            bpm: if bpm > 0.0 { bpm } else { 120.0 },
            has_track: true,
        }
    }

    /// Set track duration (called when stems are loaded)
    pub fn set_duration(&mut self, duration_samples: u64) {
        self.duration_samples = duration_samples;
    }

    /// Update BPM (called when track is loaded or BPM is changed)
    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = if bpm > 0.0 { bpm } else { 120.0 };
    }

    /// Update beat grid
    pub fn set_beat_grid(&mut self, beat_grid: Vec<u64>) {
        self.beat_grid = beat_grid;
    }

    /// Update cue markers
    pub fn update_cue_markers(&mut self, cue_points: &[CuePoint]) {
        if self.duration_samples == 0 {
            return;
        }

        self.cue_markers = cue_points
            .iter()
            .map(|cue| {
                let position = cue.sample_position as f64 / self.duration_samples as f64;
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position,
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();
    }

    /// Samples per bar at current BPM
    pub fn samples_per_bar(&self) -> u64 {
        let beats_per_bar = 4;
        let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / self.bpm) as u64;
        samples_per_beat * beats_per_bar
    }

    /// Calculate visible window centered on playhead
    pub fn visible_range(&self, playhead: u64) -> (u64, u64) {
        let window_samples = self.samples_per_bar() * self.zoom_bars as u64;
        let half_window = window_samples / 2;

        let start = playhead.saturating_sub(half_window);
        let end = (playhead + half_window).min(self.duration_samples);

        (start, end)
    }

    /// Set zoom level (clamped to valid range)
    pub fn set_zoom(&mut self, bars: u32) {
        self.zoom_bars = bars.clamp(MIN_ZOOM_BARS, MAX_ZOOM_BARS);
        // Invalidate cache when zoom changes
        self.cache_start = 0;
        self.cache_end = 0;
    }

    /// Get current zoom level
    pub fn zoom_bars(&self) -> u32 {
        self.zoom_bars
    }

    /// Check if cache is valid for current playhead position
    /// Returns true if cache needs recomputation
    pub fn needs_recompute(&self, playhead: u64) -> bool {
        if self.cached_peaks[0].is_empty() {
            return true;
        }

        let (start, end) = self.visible_range(playhead);

        // Recompute if visible range is outside cached range
        // Add some margin to reduce frequent recomputation
        let margin = (end - start) / 4;
        start < self.cache_start.saturating_add(margin)
            || end > self.cache_end.saturating_sub(margin)
    }

    /// Compute peaks for the visible window from stem data
    pub fn compute_peaks(&mut self, stems: &StemBuffers, playhead: u64, width: usize) {
        log::debug!(
            "[ZOOM-DBG] compute_peaks: playhead={}, width={}, duration_samples={}, stems.len()={}, has_track={}",
            playhead, width, self.duration_samples, stems.len(), self.has_track
        );

        let (start, end) = self.visible_range(playhead);
        log::debug!("[ZOOM-DBG] visible_range: start={}, end={}", start, end);

        // Cache a larger window to reduce recomputation frequency
        let window_size = end - start;
        let cache_start = start.saturating_sub(window_size / 2);
        let cache_end = (end + window_size / 2).min(self.duration_samples);

        self.cache_start = cache_start;
        self.cache_end = cache_end;

        let cache_len = (cache_end - cache_start) as usize;
        if cache_len == 0 || width == 0 {
            log::warn!("[ZOOM-DBG] compute_peaks: EARLY RETURN - cache_len={}, width={}", cache_len, width);
            self.cached_peaks = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            return;
        }

        // Compute peaks for the cached window
        self.cached_peaks = generate_peaks_for_range(stems, cache_start, cache_end, width);
        log::debug!(
            "[ZOOM-DBG] compute_peaks: generated {} peaks per stem (cache {}..{})",
            self.cached_peaks[0].len(), cache_start, cache_end
        );

        // Apply smoothing
        for stem_idx in 0..4 {
            if self.cached_peaks[stem_idx].len() >= PEAK_SMOOTHING_WINDOW {
                self.cached_peaks[stem_idx] = smooth_peaks(&self.cached_peaks[stem_idx]);
            }
        }
    }
}

impl Default for ZoomedState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Combined Waveform State
// =============================================================================

/// Combined waveform state containing both zoomed and overview
///
/// This is a convenience wrapper that holds both state types together.
/// Used with the combined canvas view to work around iced bug #3040
/// (multiple Canvas widgets don't render properly).
#[derive(Debug, Clone)]
pub struct CombinedState {
    /// Zoomed waveform state (detail view)
    pub zoomed: ZoomedState,
    /// Overview waveform state (full track)
    pub overview: OverviewState,
}

impl CombinedState {
    /// Create a new combined waveform state
    pub fn new() -> Self {
        Self {
            zoomed: ZoomedState::new(),
            overview: OverviewState::new(),
        }
    }
}

impl Default for CombinedState {
    fn default() -> Self {
        Self::new()
    }
}
