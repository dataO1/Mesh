//! Background peak computation for waveform displays
//!
//! This module provides a background thread for computing waveform peaks,
//! preventing UI thread blocking during expensive peak generation operations.
//! Reusable by both mesh-player and mesh-cue.
//!
//! ## Design
//!
//! Peak computation can take 10-50ms depending on zoom level and track length.
//! Running this on the UI thread causes visible stuttering. The PeaksComputer
//! offloads this work to a dedicated thread:
//!
//! 1. UI sends `PeaksComputeRequest` with stems, playhead, zoom level
//! 2. Background thread computes peaks using `generate_peaks_for_range()`
//! 3. Background thread applies Gaussian smoothing
//! 4. UI polls for `PeaksComputeResult` in tick handler
//!
//! ## Usage
//!
//! ```ignore
//! // Create at startup
//! let peaks_computer = PeaksComputer::spawn();
//!
//! // In update loop, when peaks need recomputation:
//! peaks_computer.compute(PeaksComputeRequest {
//!     id: deck_idx,
//!     playhead,
//!     stems: stems.clone(),
//!     width: 1600,
//!     zoom_bars: 8,
//!     duration_samples,
//!     bpm: 128.0,
//!     view_mode: ZoomedViewMode::Scrolling,
//!     fixed_buffer_bounds: None,
//! });
//!
//! // In tick handler, poll for results:
//! while let Some(result) = peaks_computer.try_recv() {
//!     zoomed_state.apply_computed_peaks(result);
//! }
//! ```

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::StemBuffers;
use mesh_core::types::StereoBuffer;

use super::peak_computation::{
    CacheInfo, WindowInfo, compute_effective_width,
    generate_peaks_with_padding, generate_peaks_with_padding_and_linked, smooth_peaks,
};
use super::state::ZoomedViewMode;

/// Request to compute peaks for a waveform view
pub struct PeaksComputeRequest {
    /// Identifier for the request (deck_idx for player, 0 for cue editor)
    pub id: usize,
    /// Current playhead position in samples
    pub playhead: u64,
    /// Stem audio data (Shared for RT-safe deallocation)
    pub stems: Shared<StemBuffers>,
    /// Width of the waveform display in pixels
    pub width: usize,
    /// Zoom level in bars (1-64)
    pub zoom_bars: u32,
    /// Track duration in samples
    pub duration_samples: u64,
    /// Track BPM (for calculating samples per bar)
    pub bpm: f64,
    /// View mode (affects resolution scaling)
    pub view_mode: ZoomedViewMode,
    /// Fixed buffer bounds for FixedBuffer mode (start, end in samples)
    pub fixed_buffer_bounds: Option<(u64, u64)>,
    /// Linked stem buffers [stem_idx] - None if no linked stem for that slot
    /// When provided along with linked_active=true, peaks are computed from these instead
    pub linked_stems: [Option<Shared<StereoBuffer>>; 4],
    /// Which stems are currently using their linked buffer [stem_idx]
    /// Only meaningful when linked_stems[idx] is Some
    pub linked_active: [bool; 4],
}

impl std::fmt::Debug for PeaksComputeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeaksComputeRequest")
            .field("id", &self.id)
            .field("playhead", &self.playhead)
            .field("stems", &format!("<Shared<StemBuffers> {} frames>", self.stems.len()))
            .field("width", &self.width)
            .field("zoom_bars", &self.zoom_bars)
            .field("duration_samples", &self.duration_samples)
            .field("bpm", &self.bpm)
            .field("view_mode", &self.view_mode)
            .field("fixed_buffer_bounds", &self.fixed_buffer_bounds)
            .finish()
    }
}

/// Result of peak computation
#[derive(Debug, Clone)]
pub struct PeaksComputeResult {
    /// Identifier matching the request
    pub id: usize,
    /// Computed peaks per stem [stem_idx] = Vec<(min, max)>
    pub cached_peaks: [Vec<(f32, f32)>; 4],
    /// Start sample of the cached window
    pub cache_start: u64,
    /// End sample of the cached window
    pub cache_end: u64,
    /// Left padding samples in cache (for boundary centering)
    pub cache_left_padding: u64,
    /// Zoom level used for computation
    pub zoom_bars: u32,
    /// Which stems were linked-active when these peaks were computed
    /// Used to detect when to invalidate cache on stem link toggle
    pub linked_active: [bool; 4],
}

/// Background thread for computing waveform peaks
///
/// Prevents UI thread blocking during expensive peak generation.
/// Supports multiple concurrent requests but only keeps the latest result per ID.
pub struct PeaksComputer {
    /// Channel to send compute requests
    tx: Sender<PeaksComputeRequest>,
    /// Channel to receive compute results
    rx: Receiver<PeaksComputeResult>,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl PeaksComputer {
    /// Spawn the background peak computation thread
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<PeaksComputeRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<PeaksComputeResult>();

        let handle = thread::Builder::new()
            .name("peaks-computer".to_string())
            .spawn(move || {
                peaks_thread(request_rx, result_tx);
            })
            .expect("Failed to spawn peaks computer thread");

        log::info!("PeaksComputer background thread started");

        Self {
            tx: request_tx,
            rx: result_rx,
            _handle: handle,
        }
    }

    /// Submit a peak computation request (non-blocking)
    ///
    /// The result will be available via `try_recv()` once computation completes.
    /// If the thread is busy with a previous request, this request will be queued.
    pub fn compute(&self, request: PeaksComputeRequest) -> Result<(), String> {
        self.tx
            .send(request)
            .map_err(|e| format!("Peaks computer thread disconnected: {}", e))
    }

    /// Try to receive a completed computation result (non-blocking)
    ///
    /// Returns `Some(result)` if a computation has completed, `None` otherwise.
    /// Call this in your tick handler to poll for completed work.
    pub fn try_recv(&self) -> Option<PeaksComputeResult> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                log::error!("Peaks computer thread disconnected unexpectedly");
                None
            }
        }
    }
}

/// The background peak computation thread
///
/// Uses shared peak_computation module for consistent calculations:
/// - `WindowInfo::compute()` for window/boundary handling
/// - `CacheInfo::from_window()` for cache sizing
/// - `compute_effective_width()` for resolution scaling
/// - `generate_peaks_with_padding()` for peak generation with boundary padding
fn peaks_thread(rx: Receiver<PeaksComputeRequest>, tx: Sender<PeaksComputeResult>) {
    log::debug!("Peaks computer thread starting");

    while let Ok(request) = rx.recv() {
        let start_time = std::time::Instant::now();

        // Use shared WindowInfo for consistent window calculation with boundary padding
        let window = WindowInfo::compute(
            request.playhead,
            request.zoom_bars,
            request.bpm,
            request.view_mode,
            request.fixed_buffer_bounds,
        );

        // Window-based cache: cache visible window + small margin
        // Bresenham integer math in canvas.rs prevents jiggling
        let cache = CacheInfo::from_window(&window, request.view_mode);

        let cache_len = (cache.end - cache.start) as usize;
        if cache_len == 0 || request.width == 0 || request.duration_samples == 0 {
            // Send empty result
            let _ = tx.send(PeaksComputeResult {
                id: request.id,
                cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
                cache_start: 0,
                cache_end: 0,
                cache_left_padding: 0,
                zoom_bars: request.zoom_bars,
                linked_active: request.linked_active,
            });
            continue;
        }

        // Use shared resolution calculation
        let effective_width = compute_effective_width(
            request.width,
            request.zoom_bars,
            request.view_mode,
        );

        log::debug!(
            "Peaks compute: view_mode={:?}, base_width={}, effective_width={}, cache={}..{}",
            request.view_mode, request.width, effective_width, cache.start, cache.end
        );

        // Create cache window with padding info for peak generation
        let cache_window = WindowInfo {
            start: cache.start,
            end: cache.end,
            left_padding: cache.left_padding,
            total_samples: cache.end - cache.start + cache.left_padding,
        };

        // Extract linked stem references for peak generation
        // Convert Shared<StereoBuffer> to &StereoBuffer references
        let linked_refs: [Option<&StereoBuffer>; 4] = [
            request.linked_stems[0].as_ref().map(|s| &**s),
            request.linked_stems[1].as_ref().map(|s| &**s),
            request.linked_stems[2].as_ref().map(|s| &**s),
            request.linked_stems[3].as_ref().map(|s| &**s),
        ];

        // Generate peaks for cache window at effective resolution
        // Uses linked stems when linked_active[i] is true and a buffer exists
        let mut cached_peaks = generate_peaks_with_padding_and_linked(
            &request.stems,
            &cache_window,
            effective_width,
            &linked_refs,
            &request.linked_active,
        );

        // Adaptive smoothing based on zoom level:
        // - Zoomed in (1-2 bars): more detail visible, needs more smoothing
        // - Zoomed out (16+ bars): natural averaging, less/no smoothing needed
        let smoothing_passes = match request.zoom_bars {
            1..=2 => 3,   // Very zoomed in: heavy smoothing
            3..=4 => 2,   // Zoomed in: medium smoothing
            5..=12 => 1,  // Default range: light smoothing
            _ => 0,       // Zoomed out (16+): no smoothing
        };

        for _ in 0..smoothing_passes {
            smooth_peaks(&mut cached_peaks);
        }

        // Wide smoothing for bass stem (index 2):
        // Low frequencies (~50Hz) create slow oscillations spanning ~16 pixels.
        // At very zoomed in levels, apply multiple passes to cover longer cycles.
        const BASS_STEM_IDX: usize = 2;
        let bass_wide_passes = match request.zoom_bars {
            1 => 3,       // Very zoomed in: 3 passes (~48 pixel coverage)
            2 => 2,       // Zoomed in: 2 passes (~32 pixel coverage)
            3..=8 => 1,   // Default: 1 pass (~16 pixel coverage)
            _ => 0,       // Zoomed out: skip (natural averaging sufficient)
        };

        for _ in 0..bass_wide_passes {
            if cached_peaks[BASS_STEM_IDX].len() >= 17 {
                cached_peaks[BASS_STEM_IDX] =
                    super::peaks::smooth_peaks_gaussian_wide(&cached_peaks[BASS_STEM_IDX]);
            }
        }

        let elapsed = start_time.elapsed();
        log::debug!(
            "Peaks computed for id={} in {:?} ({}..{}, {} peaks/stem, padding={})",
            request.id,
            elapsed,
            cache.start,
            cache.end,
            cached_peaks[0].len(),
            cache.left_padding
        );

        // Send result back to UI thread
        let _ = tx.send(PeaksComputeResult {
            id: request.id,
            cached_peaks,
            cache_start: cache.start,
            cache_end: cache.end,
            cache_left_padding: cache.left_padding,
            zoom_bars: request.zoom_bars,
            linked_active: request.linked_active,
        });
    }

    log::debug!("Peaks computer thread shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::peak_computation::samples_per_bar;

    #[test]
    fn test_samples_per_bar() {
        // At 120 BPM, one beat = 0.5 seconds = 24000 samples (at 48kHz)
        // One bar = 4 beats = 96000 samples
        assert_eq!(samples_per_bar(120.0), 96000);

        // At 128 BPM, one beat = 60/128 = 0.46875 seconds
        // At 48000Hz: 0.46875 * 48000 = 22500 samples per beat
        // One bar = 4 beats = 90000 samples
        let samples = samples_per_bar(128.0);
        assert_eq!(samples, 90000);
    }

    #[test]
    fn test_window_info() {
        let bpm = 120.0;

        // At playhead 500000, zoom 8 bars: window should be centered
        let window = WindowInfo::scrolling(500000, 8, bpm);
        assert!(window.start < 500000, "Window start should be before playhead");
        assert!(window.end > 500000, "Window end should be after playhead");
        assert_eq!(window.left_padding, 0, "No padding needed in middle of track");

        // At playhead 0, should have left padding
        let window_at_start = WindowInfo::scrolling(0, 8, bpm);
        assert!(window_at_start.left_padding > 0, "Should have left padding at track start");
        assert_eq!(window_at_start.start, 0, "Start should be clamped to 0");
    }

    #[test]
    fn test_peaks_computer_spawn() {
        let computer = PeaksComputer::spawn();
        // Just verify it starts without panicking
        assert!(computer.try_recv().is_none());
    }
}
