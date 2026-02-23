//! Waveform state structures for iced canvas-based waveform widgets
//!
//! These structures hold the data for waveform visualization, separate from
//! rendering logic. Following iced 0.14 patterns, state lives at the application
//! level while view functions consume references to generate UI elements.

use super::CueMarker;
use super::shader::PeakBuffer;
use crate::{CUE_COLORS, STEM_COLORS};
use iced::Color;
use mesh_core::audio_file::{CuePoint, LoadedTrack, StemBuffers};
use std::sync::Arc;

use super::{compute_highres_width, generate_peaks, smooth_peaks_gaussian, DEFAULT_WIDTH};

// =============================================================================
// Configuration Constants
// =============================================================================

/// Overview waveform height in pixels (compact)
/// 81px = 54 × 1.5, scales to 162px on UHD (2160p)
pub const WAVEFORM_HEIGHT: f32 = 81.0;

/// Zoomed waveform height in pixels (detailed, larger)
/// 180px = 1080/6, scales to 360px on UHD (2160p)
pub const ZOOMED_WAVEFORM_HEIGHT: f32 = 180.0;

/// Gap between zoomed and overview waveforms in combined view
pub const COMBINED_WAVEFORM_GAP: f32 = 6.0;

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
/// - Loop region (for DJ player loop display)
///
/// This is pure data with builder methods - rendering is handled by view functions.
#[derive(Debug, Clone)]
pub struct OverviewState {
    /// Cached waveform data per stem (min/max pairs per column) - for overview display
    pub stem_waveforms: [Vec<(f32, f32)>; 4],
    /// High-resolution peaks for zoomed view (computed once at track load)
    /// This eliminates the need for background peak recomputation
    pub highres_peaks: [Vec<(f32, f32)>; 4],
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
    /// Loop region (start, end) as normalized positions (0.0 to 1.0), None if no loop active
    pub loop_region: Option<(f64, f64)>,
    /// Slicer region (start, end) as normalized positions (0.0 to 1.0), None if slicer not active
    pub slicer_region: Option<(f64, f64)>,
    /// Current playing slice index (0-7), for highlighting in visualization
    pub slicer_current_slice: Option<u8>,
    /// Drop marker position in samples (for linked stem alignment visualization)
    pub drop_marker: Option<u64>,
    /// Linked stem waveform peaks [stem_idx] - None if no linked stem for that slot
    /// When a stem has a linked stem and is active, this provides the peaks to display
    pub linked_stem_waveforms: [Option<Vec<(f32, f32)>>; 4],
    /// Drop marker position of each linked stem (samples, for split-view alignment)
    pub linked_drop_markers: [Option<u64>; 4],
    /// Duration of each linked stem buffer (samples, for split-view alignment scaling)
    pub linked_durations: [Option<u64>; 4],
    /// High-resolution peaks for linked stems (for stable zoomed view rendering)
    /// Computed once when linked stem is loaded, avoids recomputation during playback
    pub linked_highres_peaks: [Option<Vec<(f32, f32)>>; 4],

    /// Pre-computed gain correction for each linked stem (linear multiplier)
    /// Calculated from: 10^((target_lufs - linked_lufs) / 20)
    /// Scales linked stem amplitude to target LUFS for consistent visual display
    pub linked_lufs_gains: [f32; 4],

    /// Flattened overview peaks for GPU shader (created once at track load)
    pub overview_peak_buffer: Option<PeakBuffer>,
    /// Flattened high-resolution peaks for GPU shader (created once at track load)
    pub highres_peak_buffer: Option<PeakBuffer>,

    /// Cached 8-stem overview buffer: stems 0-3 = original, stems 4-7 = linked
    /// Rebuilt when linked overview peaks change. Shader uses linked_active uniform
    /// to decide which stems are active vs inactive.
    pub linked_overview_buffer: Option<PeakBuffer>,
    /// Cached 8-stem highres buffer: stems 0-3 = original, stems 4-7 = linked
    /// Same layout as linked_overview_buffer but at HIGHRES_WIDTH resolution.
    pub linked_highres_buffer: Option<PeakBuffer>,
}

impl OverviewState {
    /// Create a new empty waveform state
    pub fn new() -> Self {
        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            highres_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            has_track: false,
            loading: false,
            missing_preview_message: None,
            grid_bars: 32, // Default: red grid line every 32 beats (8 bars)
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            drop_marker: None,
            linked_stem_waveforms: [None, None, None, None],
            linked_drop_markers: [None, None, None, None],
            linked_durations: [None, None, None, None],
            linked_highres_peaks: [None, None, None, None],
            linked_lufs_gains: [1.0, 1.0, 1.0, 1.0], // Unity gain (no correction)
            overview_peak_buffer: None,
            highres_peak_buffer: None,
            linked_overview_buffer: None,
            linked_highres_buffer: None,
        }
    }

    /// Set the main cue point position (normalized 0.0 to 1.0)
    pub fn set_cue_position(&mut self, position: Option<f64>) {
        self.cue_position = position;
    }

    /// Set high-resolution peaks for zoomed view
    ///
    /// Called after overview is created from preview, when stems become available.
    /// This enables stable zoomed waveform rendering without recomputation.
    pub fn set_highres_peaks(&mut self, peaks: [Vec<(f32, f32)>; 4]) {
        self.highres_peak_buffer = PeakBuffer::from_stem_peaks(&peaks);
        self.highres_peaks = peaks;
        log::debug!(
            "Set highres_peaks: {} peaks per stem",
            self.highres_peaks[0].len()
        );
    }

    /// Set the grid density (beats between major grid lines)
    pub fn set_grid_bars(&mut self, beats: u32) {
        self.grid_bars = beats.clamp(8, 64);
    }

    /// Set the loop region (normalized positions 0.0 to 1.0)
    ///
    /// Pass `None` to clear the loop region.
    pub fn set_loop_region(&mut self, region: Option<(f64, f64)>) {
        self.loop_region = region;
    }

    /// Set the slicer region (normalized positions 0.0 to 1.0)
    ///
    /// Pass `None` to clear the slicer region.
    pub fn set_slicer_region(&mut self, region: Option<(f64, f64)>, current_slice: Option<u8>) {
        self.slicer_region = region;
        self.slicer_current_slice = current_slice;
    }

    /// Set the drop marker position (in samples)
    ///
    /// The drop marker is used for linked stem alignment visualization.
    /// Pass `None` to clear the drop marker.
    pub fn set_drop_marker(&mut self, position: Option<u64>) {
        self.drop_marker = position;
    }

    /// Create an empty state with a message explaining why data is missing
    ///
    /// Used when a track doesn't have a cached waveform preview (needs re-analysis).
    pub fn empty_with_message(message: &str, cue_points: &[CuePoint], duration_samples: u64) -> Self {
        let cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);

        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            highres_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers: Vec::new(),
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: Some(message.to_string()),
            grid_bars: 32,
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            drop_marker: None,
            linked_stem_waveforms: [None, None, None, None],
            linked_drop_markers: [None, None, None, None],
            linked_durations: [None, None, None, None],
            linked_highres_peaks: [None, None, None, None],
            linked_lufs_gains: [1.0, 1.0, 1.0, 1.0],
            overview_peak_buffer: None,
            highres_peak_buffer: None,
            linked_overview_buffer: None,
            linked_highres_buffer: None,
        }
    }

    /// Create a placeholder from metadata only (no audio data yet)
    ///
    /// Shows cue markers while audio loads in background.
    pub fn from_metadata(metadata: &mesh_core::audio_file::TrackMetadata, duration_samples: u64) -> Self {
        let cue_markers = Self::cue_points_to_markers(&metadata.cue_points, duration_samples);

        let beat_markers: Vec<f64> = if duration_samples > 0 {
            metadata.beat_grid.beats.iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect()
        } else {
            Vec::new()
        };

        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            highres_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            cue_position: None,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: true,
            missing_preview_message: None,
            grid_bars: 32,
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            drop_marker: metadata.drop_marker,
            linked_stem_waveforms: [None, None, None, None],
            linked_drop_markers: [None, None, None, None],
            linked_durations: [None, None, None, None],
            linked_highres_peaks: [None, None, None, None],
            linked_lufs_gains: [1.0, 1.0, 1.0, 1.0],
            overview_peak_buffer: None,
            highres_peak_buffer: None,
            linked_overview_buffer: None,
            linked_highres_buffer: None,
        }
    }

    /// Set stems and generate waveform data (called when audio finishes loading)
    ///
    /// `quality_level`: 0=Low, 1=Medium, 2=High, 3=Ultra — controls highres peak count
    pub fn set_stems(
        &mut self,
        stems: &StemBuffers,
        cue_points: &[CuePoint],
        beat_grid: &[u64],
        bpm: f64,
        screen_width: u32,
        quality_level: u8,
    ) {
        let duration_samples = stems.len() as u64;
        self.duration_samples = duration_samples;
        self.loading = false;

        // Generate peak data for each stem (overview resolution)
        self.stem_waveforms = generate_peaks(stems, DEFAULT_WIDTH);

        // Generate high-resolution peaks for zoomed view (computed once, reused)
        let highres_width = compute_highres_width(stems.len(), bpm, screen_width, quality_level);
        self.highres_peaks = generate_peaks(stems, highres_width);

        // Apply Gaussian smoothing for smoother overview waveform
        for stem_idx in 0..4 {
            if self.stem_waveforms[stem_idx].len() >= 5 {
                self.stem_waveforms[stem_idx] = smooth_peaks_gaussian(&self.stem_waveforms[stem_idx]);
            }
        }


        // Convert beat grid to normalized positions
        if duration_samples > 0 {
            self.beat_markers = beat_grid
                .iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect();

            // Update cue markers with correct normalized positions
            self.cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);
        }

        // Rebuild GPU peak buffers
        self.overview_peak_buffer = PeakBuffer::from_stem_peaks(&self.stem_waveforms);
        self.highres_peak_buffer = PeakBuffer::from_stem_peaks(&self.highres_peaks);

        // Rebuild linked buffers if any linked stems exist (original peaks changed)
        self.rebuild_linked_buffers();

        log::debug!(
            "Generated waveform peaks: overview={}px, highres={}px",
            self.stem_waveforms[0].len(),
            self.highres_peaks[0].len()
        );
    }

    /// Create from a loaded track
    ///
    /// `quality_level`: 0=Low, 1=Medium, 2=High, 3=Ultra — controls highres peak count
    pub fn from_track(track: &Arc<LoadedTrack>, cue_points: &[CuePoint], bpm: f64, screen_width: u32, quality_level: u8) -> Self {
        let duration_samples = track.duration_samples as u64;

        // Generate peak data for each stem (overview resolution)
        let mut stem_waveforms = generate_peaks(&track.stems, DEFAULT_WIDTH);

        // Generate high-resolution peaks for zoomed view (computed once, reused)
        let highres_width = compute_highres_width(track.stems.len(), bpm, screen_width, quality_level);
        let highres_peaks = generate_peaks(&track.stems, highres_width);

        // Apply Gaussian smoothing for smoother overview waveform
        for stem_idx in 0..4 {
            if stem_waveforms[stem_idx].len() >= 5 {
                stem_waveforms[stem_idx] = smooth_peaks_gaussian(&stem_waveforms[stem_idx]);
            }
        }



        // Convert beat grid to normalized positions
        let beat_markers: Vec<f64> = track
            .metadata
            .beat_grid
            .beats
            .iter()
            .map(|&pos| pos as f64 / duration_samples as f64)
            .collect();

        let cue_markers = Self::cue_points_to_markers(cue_points, duration_samples);

        log::debug!(
            "Created OverviewState from track: overview={}px, highres={}px",
            stem_waveforms[0].len(),
            highres_peaks[0].len()
        );

        // Build GPU peak buffers before moving arrays into struct
        let overview_peak_buffer = PeakBuffer::from_stem_peaks(&stem_waveforms);
        let highres_peak_buffer = PeakBuffer::from_stem_peaks(&highres_peaks);

        Self {
            stem_waveforms,
            highres_peaks,
            position: 0.0,
            cue_position: None,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: None,
            grid_bars: 32,
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            drop_marker: track.metadata.drop_marker,
            linked_stem_waveforms: [None, None, None, None],
            linked_drop_markers: [None, None, None, None],
            linked_durations: [None, None, None, None],
            linked_highres_peaks: [None, None, None, None],
            linked_lufs_gains: [1.0, 1.0, 1.0, 1.0],
            overview_peak_buffer,
            highres_peak_buffer,
            linked_overview_buffer: None,
            linked_highres_buffer: None,
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

    /// Rebuild cached 8-stem GPU buffers for linked stem visualization.
    ///
    /// Called whenever linked or original peak data changes. Produces buffers with
    /// stems 0-3 = original, stems 4-7 = linked (LUFS-corrected). The shader uses
    /// `linked_active` uniforms to decide which set is active vs inactive.
    pub fn rebuild_linked_buffers(&mut self) {
        let has_any_linked = self.linked_stem_waveforms.iter().any(|o| o.is_some());

        if has_any_linked {
            self.linked_overview_buffer = PeakBuffer::from_linked(
                &self.stem_waveforms,
                &self.linked_stem_waveforms,
                &self.linked_lufs_gains,
            );
            self.linked_highres_buffer = PeakBuffer::from_linked(
                &self.highres_peaks,
                &self.linked_highres_peaks,
                &self.linked_lufs_gains,
            );
            log::debug!(
                "Rebuilt linked buffers: overview={}, highres={}",
                self.linked_overview_buffer.as_ref().map_or(0, |b| b.data.len()),
                self.linked_highres_buffer.as_ref().map_or(0, |b| b.data.len()),
            );
        } else {
            self.linked_overview_buffer = None;
            self.linked_highres_buffer = None;
        }
    }

    /// Set linked stem waveform peaks for a specific stem slot
    ///
    /// Called when a linked stem is loaded and its peaks are extracted from the source file.
    /// The peaks should already be smoothed (Gaussian smoothing applied in extraction).
    pub fn set_linked_stem_peaks(&mut self, stem_idx: usize, peaks: Vec<(f32, f32)>) {
        if stem_idx < 4 {
            self.linked_stem_waveforms[stem_idx] = Some(peaks);
            self.rebuild_linked_buffers();
        }
    }

    /// Clear linked stem peaks for a specific stem slot
    ///
    /// Called when a linked stem is unloaded or the track is unloaded.
    pub fn clear_linked_stem_peaks(&mut self, stem_idx: usize) {
        if stem_idx < 4 {
            self.linked_stem_waveforms[stem_idx] = None;
            self.rebuild_linked_buffers();
        }
    }

    /// Clear all linked stem peaks (when track is unloaded)
    pub fn clear_all_linked_stem_peaks(&mut self) {
        self.linked_stem_waveforms = [None, None, None, None];
        self.linked_overview_buffer = None;
        self.linked_highres_buffer = None;
    }

    /// Set linked stem high-resolution peaks for stable zoomed view rendering
    ///
    /// Called when a linked stem is loaded. These peaks are computed once and reused
    /// to avoid recomputation during playback.
    pub fn set_linked_highres_peaks(&mut self, stem_idx: usize, peaks: Vec<(f32, f32)>) {
        if stem_idx < 4 {
            self.linked_highres_peaks[stem_idx] = Some(peaks);
            self.rebuild_linked_buffers();
        }
    }

    /// Set the LUFS-based gain correction for a linked stem
    ///
    /// The gain should be calculated as: 10^((target_lufs - linked_lufs) / 20)
    /// This scales the linked stem's visual amplitude to match the target LUFS.
    pub fn set_linked_lufs_gain(&mut self, stem_idx: usize, gain: f32) {
        if stem_idx < 4 {
            self.linked_lufs_gains[stem_idx] = gain;
            log::debug!(
                "Linked stem {} LUFS gain set to {:.3} ({:+.1} dB)",
                stem_idx, gain, 20.0 * gain.log10()
            );
        }
    }

    /// Clear linked stem LUFS gain (reset to unity)
    pub fn clear_linked_lufs_gain(&mut self, stem_idx: usize) {
        if stem_idx < 4 {
            self.linked_lufs_gains[stem_idx] = 1.0;
        }
    }

    /// Clear linked stem high-resolution peaks for a specific stem slot
    pub fn clear_linked_highres_peaks(&mut self, stem_idx: usize) {
        if stem_idx < 4 {
            self.linked_highres_peaks[stem_idx] = None;
        }
    }

    /// Clear all linked stem high-resolution peaks (when track is unloaded)
    pub fn clear_all_linked_highres_peaks(&mut self) {
        self.linked_highres_peaks = [None, None, None, None];
    }

    /// Set linked stem metadata for alignment in split-view rendering
    ///
    /// Called when a linked stem is loaded. Drop marker and duration are used
    /// to calculate the x-offset for aligning the linked waveform with the host.
    pub fn set_linked_stem_metadata(&mut self, stem_idx: usize, drop_marker: u64, duration: u64) {
        if stem_idx < 4 {
            self.linked_drop_markers[stem_idx] = Some(drop_marker);
            self.linked_durations[stem_idx] = Some(duration);
        }
    }

    /// Clear linked stem metadata for a specific stem slot
    pub fn clear_linked_stem_metadata(&mut self, stem_idx: usize) {
        if stem_idx < 4 {
            self.linked_drop_markers[stem_idx] = None;
            self.linked_durations[stem_idx] = None;
        }
    }

    /// Clear all linked stem metadata (when track is unloaded)
    pub fn clear_all_linked_stem_metadata(&mut self) {
        self.linked_drop_markers = [None, None, None, None];
        self.linked_durations = [None, None, None, None];
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
/// View mode for zoomed waveform
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ZoomedViewMode {
    /// Playhead at center, waveform scrolls (default)
    #[default]
    Scrolling,
    /// Slicer buffer fixed to view, playhead moves left-to-right
    FixedBuffer,
}

/// State for zoomed waveform rendering
///
/// - Zoom level and visible range calculation
/// - Beat grid and cue markers (in sample positions)
///
/// Peak data is stored on OverviewState as PeakBuffers (uploaded to GPU once at track load).
/// Supports zoom levels from 1 to 64 bars via click+drag gesture.
#[derive(Debug, Clone)]
pub struct ZoomedState {
    /// Current zoom level in bars (1-64)
    pub zoom_bars: u32,
    /// Zoom level saved from scrolling mode (restored when exiting FixedBuffer)
    scrolling_zoom_bars: u32,
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
    /// Loop region (start, end) as normalized positions (0.0 to 1.0)
    pub loop_region: Option<(f64, f64)>,
    /// Slicer region (start, end) as normalized positions (0.0 to 1.0), None if slicer not active
    pub slicer_region: Option<(f64, f64)>,
    /// Current playing slice index (0-15), for highlighting in visualization
    pub slicer_current_slice: Option<u8>,
    /// View mode (scrolling vs fixed buffer)
    pub view_mode: ZoomedViewMode,
    /// Fixed buffer bounds in samples (for FixedBuffer mode)
    pub fixed_buffer_bounds: Option<(u64, u64)>,
    /// Drop marker position in samples (for linked stem alignment visualization)
    pub drop_marker: Option<u64>,
    /// LUFS-based gain for waveform amplitude scaling (linear multiplier)
    /// Quieter tracks are boosted to match the visual amplitude of louder tracks.
    /// 1.0 = unity (no scaling), >1.0 = boost for quiet tracks
    pub lufs_gain: f32,
}

impl ZoomedState {
    /// Create a new empty zoomed waveform state
    pub fn new() -> Self {
        Self {
            zoom_bars: DEFAULT_ZOOM_BARS,
            scrolling_zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            bpm: 120.0,
            has_track: false,
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            view_mode: ZoomedViewMode::Scrolling,
            fixed_buffer_bounds: None,
            drop_marker: None,
            lufs_gain: 1.0,
        }
    }

    /// Create from track metadata
    pub fn from_metadata(bpm: f64, beat_grid: Vec<u64>, cue_markers: Vec<CueMarker>) -> Self {
        Self {
            zoom_bars: DEFAULT_ZOOM_BARS,
            scrolling_zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid,
            cue_markers,
            duration_samples: 0,
            bpm: if bpm > 0.0 { bpm } else { 120.0 },
            has_track: true,
            loop_region: None,
            slicer_region: None,
            slicer_current_slice: None,
            view_mode: ZoomedViewMode::Scrolling,
            fixed_buffer_bounds: None,
            drop_marker: None,
            lufs_gain: 1.0,
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

    /// Set the loop region (normalized positions 0.0 to 1.0)
    ///
    /// Pass `None` to clear the loop region.
    pub fn set_loop_region(&mut self, region: Option<(f64, f64)>) {
        self.loop_region = region;
    }

    /// Set the slicer region (normalized positions 0.0 to 1.0)
    ///
    /// Pass `None` to clear the slicer region.
    pub fn set_slicer_region(&mut self, region: Option<(f64, f64)>, current_slice: Option<u8>) {
        self.slicer_region = region;
        self.slicer_current_slice = current_slice;
    }

    /// Set the drop marker position (in samples)
    ///
    /// The drop marker is used for linked stem alignment visualization.
    /// Pass `None` to clear the drop marker.
    pub fn set_drop_marker(&mut self, position: Option<u64>) {
        self.drop_marker = position;
    }

    /// Set LUFS-based gain for waveform amplitude scaling
    ///
    /// Quieter tracks are visually boosted to match the amplitude of louder tracks.
    /// The gain is a linear multiplier applied to all peak values before smoothing.
    ///
    /// # Arguments
    /// * `gain` - Linear gain multiplier (1.0 = unity, >1.0 = boost for quiet tracks)
    pub fn set_lufs_gain(&mut self, gain: f32) {
        self.lufs_gain = gain;
    }

    /// Set the view mode (scrolling or fixed buffer)
    ///
    /// When entering FixedBuffer mode:
    /// - Saves current zoom_bars to scrolling_zoom_bars
    /// - Set zoom_bars appropriately via `set_fixed_buffer_zoom()`
    ///
    /// When exiting to Scrolling mode:
    /// - Restores zoom_bars from scrolling_zoom_bars
    pub fn set_view_mode(&mut self, mode: ZoomedViewMode) {
        if self.view_mode == mode {
            return; // No change
        }

        match mode {
            ZoomedViewMode::FixedBuffer => {
                // Save current zoom level before entering fixed buffer mode
                self.scrolling_zoom_bars = self.zoom_bars;
            }
            ZoomedViewMode::Scrolling => {
                // Restore saved zoom level when exiting fixed buffer mode
                self.zoom_bars = self.scrolling_zoom_bars;
            }
        }

        self.view_mode = mode;
    }

    /// Set the zoom level for fixed buffer mode based on buffer size in bars
    ///
    /// Should be called after `set_view_mode(FixedBuffer)` and `set_fixed_buffer_bounds()`.
    /// Sets zoom_bars to match the buffer size for appropriate resolution.
    pub fn set_fixed_buffer_zoom(&mut self, buffer_bars: u32) {
        if self.view_mode == ZoomedViewMode::FixedBuffer {
            self.zoom_bars = buffer_bars.clamp(MIN_ZOOM_BARS, MAX_ZOOM_BARS);
        }
    }

    /// Get the current view mode
    pub fn view_mode(&self) -> ZoomedViewMode {
        self.view_mode
    }

    /// Set fixed buffer bounds in samples (for FixedBuffer mode)
    pub fn set_fixed_buffer_bounds(&mut self, bounds: Option<(u64, u64)>) {
        self.fixed_buffer_bounds = bounds;
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


    /// Set zoom level (clamped to valid range)
    pub fn set_zoom(&mut self, bars: u32) {
        self.zoom_bars = bars.clamp(MIN_ZOOM_BARS, MAX_ZOOM_BARS);
    }

    /// Get current zoom level
    pub fn zoom_bars(&self) -> u32 {
        self.zoom_bars
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
    /// Whether each stem has a linked stem from another track [4 stems]
    pub linked_stems: [bool; 4],
    /// Whether the linked stem is currently active (vs playing original) [4 stems]
    pub linked_active: [bool; 4],
    /// Which stems are muted [4 stems]
    pub stem_active: [bool; 4],
}

impl CombinedState {
    /// Create a new combined waveform state
    pub fn new() -> Self {
        Self {
            zoomed: ZoomedState::new(),
            overview: OverviewState::new(),
            linked_stems: [false; 4],
            linked_active: [false; 4],
            stem_active: [true; 4], // All stems active by default
        }
    }

    /// Set linked stem status for a specific stem
    ///
    /// `has_linked` indicates whether a linked stem exists
    /// `is_active` indicates whether the linked stem is currently playing
    pub fn set_linked_stem(&mut self, stem_idx: usize, has_linked: bool, is_active: bool) {
        if stem_idx < 4 {
            self.linked_stems[stem_idx] = has_linked;
            self.linked_active[stem_idx] = is_active;
        }
    }

    /// Clear all linked stem status (when track is unloaded)
    pub fn clear_linked_stems(&mut self) {
        self.linked_stems = [false; 4];
        self.linked_active = [false; 4];
    }

    /// Set stem active state (mute/unmute)
    pub fn set_stem_active(&mut self, stem_idx: usize, active: bool) {
        if stem_idx < 4 {
            self.stem_active[stem_idx] = active;
        }
    }
}

impl Default for CombinedState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Player Canvas State (4-Deck Unified View)
// =============================================================================

/// Height of deck header row showing deck number and track name
pub const DECK_HEADER_HEIGHT: f32 = 48.0;

/// State for 4-deck player canvas (all waveforms in one)
///
/// This holds waveform data for all 4 decks in a DJ player, allowing
/// them to be rendered in a single Canvas widget (working around iced bug #3040).
///
/// ## Layout (per deck quadrant)
/// - **Header row**: Deck number + track name (22px)
/// - **Zoomed waveform**: Detail view centered on playhead (120px)
/// - **Overview waveform**: Full track view (35px)
///
/// Grid: Deck 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
#[derive(Debug)]
pub struct PlayerCanvasState {
    /// Per-deck combined state (zoomed + overview)
    pub decks: [CombinedState; 4],
    /// Per-deck playhead positions in samples
    pub playheads: [u64; 4],
    /// Track names for each deck (displayed in header)
    track_names: [String; 4],
    /// Track keys for each deck (displayed in header, e.g. "Am", "C#m")
    track_keys: [String; 4],
    /// Track BPM for each deck (original analyzed BPM, displayed in header)
    track_bpm: [Option<f64>; 4],
    /// Stem active status per deck [deck][stem] (4 decks × 4 stems)
    /// true = stem is playing, false = stem is bypassed/muted
    stem_active: [[bool; 4]; 4],
    /// Audio-thread timestamp of last position update (nanos since PROCESS_EPOCH)
    /// Used for accurate cross-thread interpolation between audio callbacks.
    position_timestamps_ns: [u64; 4],
    /// Playback rate per deck (1.0 = normal, from stretch_ratio)
    playback_rates: [f64; 4],
    /// Whether each deck is currently playing (for interpolation)
    is_playing: [bool; 4],
    /// Whether each deck is the master (longest playing, others sync to it)
    is_master: [bool; 4],
    /// Current transpose in semitones per deck (-12 to +12, 0 if disabled or compatible)
    current_transpose: [i8; 4],
    /// Whether key matching is enabled per deck
    key_match_enabled: [bool; 4],
    /// Stem colors for waveform rendering [Vocals, Drums, Bass, Other]
    stem_colors: [Color; 4],
    /// Linked stem status per deck [deck][stem] (4 decks × 4 stems)
    /// true = stem has a linked stem from another track
    linked_stems: [[bool; 4]; 4],
    /// Whether linked stem is active per deck [deck][stem]
    /// true = currently playing linked stem, false = playing original
    linked_stems_active: [[bool; 4]; 4],
    /// LUFS gain compensation in dB per deck (None if no track or no LUFS data)
    /// Positive = boost (quiet track), Negative = cut (loud track)
    lufs_gain_db: [Option<f32>; 4],
    /// Whether cue (headphone monitoring) is enabled per deck
    cue_enabled: [bool; 4],
    /// Current loop length in beats per deck (None if no track loaded)
    loop_length_beats: [Option<f32>; 4],
    /// Whether loop is currently active per deck
    loop_active: [bool; 4],
    /// Channel volume per deck (0.0-1.0, for waveform dimming)
    volume: [f32; 4],
    /// Display BPM per deck (global BPM for BPM-aligned overview rendering)
    /// When set, overview waveforms are stretched so beat grids align across decks.
    /// None = no stretching (track has no BPM data or no global sync active)
    display_bpm: [Option<f64>; 4],
    /// Whether vertical waveform layout is active (time flows top-to-bottom)
    vertical_layout: bool,
    /// Whether the vertical Y axis is inverted (time flows bottom-to-top)
    vertical_inverted: bool,
    /// Waveform abstraction level (0=low, 1=medium, 2=high; controls subsampling grid size)
    pub abstraction_level: u8,
    /// Waveform motion blur level (0=low/crisp, 1=medium, 2=high; controls smoothstep edge width)
    pub motion_blur_level: u8,
    /// Waveform depth fade level (0=low, 1=medium, 2=high; controls baseline-to-edge alpha gradient)
    pub depth_fade_level: u8,
    /// Whether depth fade is inverted (opaque at center, transparent at edges)
    pub depth_fade_inverted: bool,
    /// Minimum pixel width multiplier for thin transient peaks (0.0 = disabled, 1.5 = default)
    pub peak_width_mult: f32,
    /// Edge AA algorithm (0=Standard, 1=SlopeL1, 2=SlopeL2, 3=SlopeL2Clamped)
    pub edge_aa_level: u8,
    /// Monotonic frame counter incremented every tick (vsync), used for loading pulse animation
    pub frame_count: u32,
}

impl PlayerCanvasState {
    /// Create a new player canvas state with 4 empty decks
    pub fn new() -> Self {
        Self {
            decks: [
                CombinedState::new(),
                CombinedState::new(),
                CombinedState::new(),
                CombinedState::new(),
            ],
            playheads: [0; 4],
            track_names: [
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
            track_keys: [
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
            track_bpm: [None; 4],        // No BPM data initially
            stem_active: [[true; 4]; 4], // All stems active by default
            position_timestamps_ns: [0; 4],
            playback_rates: [1.0; 4],
            is_playing: [false, false, false, false],
            is_master: [false, false, false, false],
            current_transpose: [0; 4],
            key_match_enabled: [false; 4],
            stem_colors: STEM_COLORS,
            linked_stems: [[false; 4]; 4],       // No linked stems by default
            linked_stems_active: [[false; 4]; 4], // All using original stems
            lufs_gain_db: [None; 4],             // No LUFS data initially
            cue_enabled: [false; 4],             // No cue enabled by default
            loop_length_beats: [None; 4],        // No loop length initially
            loop_active: [false; 4],             // No loop active initially
            volume: [1.0; 4],                    // Full volume by default
            display_bpm: [None; 4],              // No BPM alignment initially
            vertical_layout: false,              // Horizontal layout by default
            vertical_inverted: false,
            abstraction_level: 1,                // Medium abstraction by default
            motion_blur_level: 0,                // Low (crisp) by default
            depth_fade_level: 2,                 // Medium depth fade by default (0=off, 1=low, 2=med, 3=high)
            depth_fade_inverted: false,          // Normal: transparent center, opaque edges
            peak_width_mult: 1.5,                // Medium transient width (fw * 1.5)
            edge_aa_level: 3,                    // Slope L2 Clamped by default
            frame_count: 0,
        }
    }

    /// Advance the frame counter (call once per tick/vsync)
    pub fn tick(&mut self) {
        self.frame_count = self.frame_count.wrapping_add(1);
    }

    /// Set the track name for a deck (displayed in header)
    pub fn set_track_name(&mut self, idx: usize, name: String) {
        if idx < 4 && self.track_names[idx] != name {
            self.track_names[idx] = name;
        }
    }

    /// Get the track name for a deck
    pub fn track_name(&self, idx: usize) -> &str {
        if idx < 4 {
            &self.track_names[idx]
        } else {
            ""
        }
    }

    /// Clear track name when deck is unloaded
    pub fn clear_track_name(&mut self, idx: usize) {
        if idx < 4 && !self.track_names[idx].is_empty() {
            self.track_names[idx].clear();

        }
    }

    /// Set the track key for a deck (displayed in header)
    pub fn set_track_key(&mut self, idx: usize, key: String) {
        if idx < 4 && self.track_keys[idx] != key {
            self.track_keys[idx] = key;

        }
    }

    /// Get the track key for a deck
    pub fn track_key(&self, idx: usize) -> &str {
        if idx < 4 {
            &self.track_keys[idx]
        } else {
            ""
        }
    }

    /// Set the track BPM for a deck (displayed in header)
    pub fn set_track_bpm(&mut self, idx: usize, bpm: Option<f64>) {
        if idx < 4 && self.track_bpm[idx] != bpm {
            self.track_bpm[idx] = bpm;

        }
    }

    /// Get the track BPM for a deck
    pub fn track_bpm(&self, idx: usize) -> Option<f64> {
        if idx < 4 {
            self.track_bpm[idx]
        } else {
            None
        }
    }

    /// Set stem active status for a deck (true = playing, false = bypassed)
    pub fn set_stem_active(&mut self, deck_idx: usize, stem_idx: usize, active: bool) {
        if deck_idx < 4 && stem_idx < 4 && self.stem_active[deck_idx][stem_idx] != active {
            self.stem_active[deck_idx][stem_idx] = active;

        }
    }

    /// Get stem active status for a deck (true = playing, false = bypassed)
    pub fn stem_active(&self, deck_idx: usize) -> &[bool; 4] {
        if deck_idx < 4 {
            &self.stem_active[deck_idx]
        } else {
            &[true; 4]  // Default: all stems active
        }
    }

    /// Set linked stem status for a deck (true = has linked stem, false = no link)
    pub fn set_linked_stem(&mut self, deck_idx: usize, stem_idx: usize, has_linked: bool, is_active: bool) {
        if deck_idx < 4 && stem_idx < 4
            && (self.linked_stems[deck_idx][stem_idx] != has_linked
                || self.linked_stems_active[deck_idx][stem_idx] != is_active)
        {
            self.linked_stems[deck_idx][stem_idx] = has_linked;
            self.linked_stems_active[deck_idx][stem_idx] = is_active;

        }
    }

    /// Get linked stem status for a deck [stem_idx] -> (has_linked, is_active)
    pub fn linked_stems(&self, deck_idx: usize) -> (&[bool; 4], &[bool; 4]) {
        if deck_idx < 4 {
            (&self.linked_stems[deck_idx], &self.linked_stems_active[deck_idx])
        } else {
            (&[false; 4], &[false; 4])
        }
    }

    /// Set whether a deck is the master (longest playing)
    pub fn set_master(&mut self, idx: usize, is_master: bool) {
        if idx < 4 && self.is_master[idx] != is_master {
            self.is_master[idx] = is_master;

        }
    }

    /// Check if a deck is the master
    pub fn is_master(&self, idx: usize) -> bool {
        if idx < 4 {
            self.is_master[idx]
        } else {
            false
        }
    }

    /// Set the current transpose for a deck
    pub fn set_transpose(&mut self, idx: usize, semitones: i8) {
        if idx < 4 && self.current_transpose[idx] != semitones {
            self.current_transpose[idx] = semitones;

        }
    }

    /// Get the current transpose for a deck
    pub fn transpose(&self, idx: usize) -> i8 {
        if idx < 4 {
            self.current_transpose[idx]
        } else {
            0
        }
    }

    /// Set whether key matching is enabled for a deck
    pub fn set_key_match_enabled(&mut self, idx: usize, enabled: bool) {
        if idx < 4 && self.key_match_enabled[idx] != enabled {
            self.key_match_enabled[idx] = enabled;

        }
    }

    /// Check if key matching is enabled for a deck
    pub fn key_match_enabled(&self, idx: usize) -> bool {
        if idx < 4 {
            self.key_match_enabled[idx]
        } else {
            false
        }
    }

    /// Set LUFS gain compensation in dB for a deck
    ///
    /// Display in header: "+2.1dB" (boost) or "-3.5dB" (cut)
    pub fn set_lufs_gain_db(&mut self, idx: usize, gain_db: Option<f32>) {
        if idx < 4 && self.lufs_gain_db[idx] != gain_db {
            self.lufs_gain_db[idx] = gain_db;

        }
    }

    /// Get LUFS gain compensation in dB for a deck
    pub fn lufs_gain_db(&self, idx: usize) -> Option<f32> {
        if idx < 4 {
            self.lufs_gain_db[idx]
        } else {
            None
        }
    }

    /// Set cue (headphone monitoring) enabled state for a deck
    pub fn set_cue_enabled(&mut self, idx: usize, enabled: bool) {
        if idx < 4 && self.cue_enabled[idx] != enabled {
            self.cue_enabled[idx] = enabled;

        }
    }

    /// Get cue (headphone monitoring) enabled state for a deck
    pub fn cue_enabled(&self, idx: usize) -> bool {
        if idx < 4 {
            self.cue_enabled[idx]
        } else {
            false
        }
    }

    /// Set loop length in beats for a deck
    pub fn set_loop_length_beats(&mut self, idx: usize, beats: Option<f32>) {
        if idx < 4 && self.loop_length_beats[idx] != beats {
            self.loop_length_beats[idx] = beats;

        }
    }

    /// Get loop length in beats for a deck
    pub fn loop_length_beats(&self, idx: usize) -> Option<f32> {
        if idx < 4 {
            self.loop_length_beats[idx]
        } else {
            None
        }
    }

    /// Set loop active state for a deck
    pub fn set_loop_active(&mut self, idx: usize, active: bool) {
        if idx < 4 && self.loop_active[idx] != active {
            self.loop_active[idx] = active;

        }
    }

    /// Get loop active state for a deck
    pub fn loop_active(&self, idx: usize) -> bool {
        if idx < 4 {
            self.loop_active[idx]
        } else {
            false
        }
    }

    /// Set channel volume for a deck (0.0-1.0)
    pub fn set_volume(&mut self, idx: usize, volume: f32) {
        if idx < 4 && (self.volume[idx] - volume).abs() > f32::EPSILON {
            self.volume[idx] = volume;

        }
    }

    /// Get channel volume for a deck (0.0-1.0)
    pub fn volume(&self, idx: usize) -> f32 {
        if idx < 4 {
            self.volume[idx]
        } else {
            1.0
        }
    }

    /// Set display BPM for a deck (global BPM used for overview alignment)
    pub fn set_display_bpm(&mut self, idx: usize, bpm: Option<f64>) {
        if idx < 4 && self.display_bpm[idx] != bpm {
            self.display_bpm[idx] = bpm;

        }
    }

    /// Get display BPM for a deck
    pub fn display_bpm(&self, idx: usize) -> Option<f64> {
        if idx < 4 {
            self.display_bpm[idx]
        } else {
            None
        }
    }

    /// Compute BPM-aligned display fraction for overview rendering.
    ///
    /// Returns the fraction of display width this deck occupies when all loaded
    /// decks share a common time axis scaled by BPM. The deck with the most
    /// "beat content" (duration × BPM) gets D=1.0 and fills the full width;
    /// shorter/slower tracks get D<1.0 with silence padding at the end.
    ///
    /// Formula: `D = (dur × track_bpm) / max(dur_j × track_bpm_j)`
    ///
    /// Returns `None` when no scaling is needed (single track, no BPM data,
    /// or all tracks have equal beat content).
    pub fn overview_display_fraction(&self, deck_idx: usize) -> Option<f64> {
        if deck_idx >= 4 { return None; }

        let display_bpm = self.display_bpm(deck_idx)?;
        if display_bpm <= 0.0 { return None; }

        let track_bpm = self.track_bpm(deck_idx)?;
        if track_bpm <= 0.0 { return None; }

        let dur = self.decks[deck_idx].overview.duration_samples;
        if dur == 0 { return None; }

        // Beat content for this deck
        let my_beat_content = dur as f64 * track_bpm;

        // Find max beat content across all loaded decks
        let mut max_beat_content = my_beat_content;
        let mut loaded_count = 0usize;

        for i in 0..4 {
            let overview = &self.decks[i].overview;
            if overview.has_track && overview.duration_samples > 0 {
                if let Some(tbpm) = self.track_bpm(i) {
                    if tbpm > 0.0 {
                        let bc = overview.duration_samples as f64 * tbpm;
                        max_beat_content = max_beat_content.max(bc);
                        loaded_count += 1;
                    }
                }
            }
        }

        if max_beat_content <= 0.0 || loaded_count < 2 {
            return None;
        }

        let d = my_beat_content / max_beat_content;
        // Skip when D ≈ 1.0 (this deck is the longest, no padding needed)
        if (d - 1.0).abs() < 0.001 { None } else { Some(d) }
    }

    /// Set stem colors for waveform rendering [Vocals, Drums, Bass, Other]
    pub fn set_stem_colors(&mut self, colors: [Color; 4]) {
        if self.stem_colors != colors {
            self.stem_colors = colors;

        }
    }

    /// Get stem colors for waveform rendering [Vocals, Drums, Bass, Other]
    pub fn stem_colors(&self) -> &[Color; 4] {
        &self.stem_colors
    }

    /// Set vertical waveform layout mode
    pub fn set_vertical_layout(&mut self, vertical: bool) {
        if self.vertical_layout != vertical {
            self.vertical_layout = vertical;

        }
    }

    /// Set vertical Y axis inversion
    pub fn set_vertical_inverted(&mut self, inverted: bool) {
        if self.vertical_inverted != inverted {
            self.vertical_inverted = inverted;

        }
    }

    /// Check if vertical waveform layout is active
    pub fn is_vertical_layout(&self) -> bool {
        self.vertical_layout
    }

    /// Check if the vertical Y axis is inverted (time flows bottom-to-top)
    pub fn is_vertical_inverted(&self) -> bool {
        self.vertical_inverted
    }

    /// Get a reference to a deck's state
    pub fn deck(&self, idx: usize) -> &CombinedState {
        &self.decks[idx]
    }

    /// Get a mutable reference to a deck's state
    pub fn deck_mut(&mut self, idx: usize) -> &mut CombinedState {
        &mut self.decks[idx]
    }

    /// Set the playhead position for a deck (in samples)
    ///
    /// Takes the audio-thread timestamp and playback rate for accurate
    /// cross-thread interpolation between audio callbacks.
    pub fn set_playhead(
        &mut self,
        idx: usize,
        position: u64,
        is_playing: bool,
        timestamp_ns: u64,
        playback_rate: f32,
    ) {
        if idx < 4
            && (self.playheads[idx] != position || self.is_playing[idx] != is_playing)
        {
            self.playheads[idx] = position;
            self.position_timestamps_ns[idx] = timestamp_ns;
            self.playback_rates[idx] = playback_rate as f64;
            self.is_playing[idx] = is_playing;

        }
    }

    /// Get the playhead position for a deck (in samples)
    pub fn playhead(&self, idx: usize) -> u64 {
        self.playheads[idx]
    }

    /// Get interpolated playhead position for smooth rendering
    ///
    /// Uses the audio-thread timestamp and playback rate for accurate
    /// interpolation between audio callbacks. The timestamp comes from the
    /// same monotonic clock (PROCESS_EPOCH) on both threads, so the elapsed
    /// time calculation is accurate regardless of audio buffer size.
    ///
    /// Formula: `position + elapsed_since_audio_callback * sample_rate * playback_rate`
    pub fn interpolated_playhead(&self, idx: usize, sample_rate: u32) -> u64 {
        if idx >= 4 {
            return 0;
        }

        // If not playing, return the exact position (no interpolation needed)
        if !self.is_playing[idx] {
            return self.playheads[idx];
        }

        let ts_ns = self.position_timestamps_ns[idx];
        if ts_ns == 0 {
            // No timestamp yet (first frame) — return raw position
            return self.playheads[idx];
        }

        // Elapsed time since the audio thread last updated the position
        let now_ns = Self::process_epoch_nanos();
        let elapsed_ns = now_ns.saturating_sub(ts_ns);
        let elapsed_secs = elapsed_ns as f64 / 1_000_000_000.0;

        // Advance by elapsed * sample_rate * playback_rate
        let rate = self.playback_rates[idx];
        let samples_ahead = (elapsed_secs * sample_rate as f64 * rate) as u64;

        // Safety clamp: don't extrapolate more than 100ms ahead
        let max_ahead = (sample_rate as u64) / 10;
        self.playheads[idx].saturating_add(samples_ahead.min(max_ahead))
    }

    /// Get nanoseconds since PROCESS_EPOCH (same clock as audio thread)
    fn process_epoch_nanos() -> u64 {
        // LazyLock ensures the epoch is initialized on first use.
        // Both audio thread and UI thread use the same LazyLock, so
        // timestamps are directly comparable.
        mesh_core::engine::PROCESS_EPOCH.elapsed().as_nanos() as u64
    }
}

impl Default for PlayerCanvasState {
    fn default() -> Self {
        Self::new()
    }
}
