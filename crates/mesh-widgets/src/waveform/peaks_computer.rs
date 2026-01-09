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
use mesh_core::types::SAMPLE_RATE;

use super::peaks::{generate_peaks_for_range, smooth_peaks_gaussian};
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
    /// Zoom level used for computation
    pub zoom_bars: u32,
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

/// Calculate samples per bar at the given BPM
fn samples_per_bar(bpm: f64) -> u64 {
    let beats_per_bar = 4;
    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
    samples_per_beat * beats_per_bar
}

/// Calculate the visible sample range centered on playhead
fn visible_range(playhead: u64, zoom_bars: u32, bpm: f64, duration_samples: u64) -> (u64, u64) {
    let window_samples = samples_per_bar(bpm) * zoom_bars as u64;
    let half_window = window_samples / 2;

    let start = playhead.saturating_sub(half_window);
    let end = (playhead + half_window).min(duration_samples);

    (start, end)
}

/// The background peak computation thread
///
/// Resolution scaling depends on view mode:
/// - **Scrolling mode**: Uses base width (same as before), no scaling
/// - **FixedBuffer mode**: Scales up resolution for small buffers (slicer)
fn peaks_thread(rx: Receiver<PeaksComputeRequest>, tx: Sender<PeaksComputeResult>) {
    log::debug!("Peaks computer thread starting");

    while let Ok(request) = rx.recv() {
        let start_time = std::time::Instant::now();

        // Calculate visible window based on view mode
        let (start, end) = match request.view_mode {
            ZoomedViewMode::FixedBuffer => {
                // Use fixed buffer bounds if provided
                if let Some((s, e)) = request.fixed_buffer_bounds {
                    (s, e)
                } else {
                    // Fall back to scrolling behavior
                    visible_range(
                        request.playhead,
                        request.zoom_bars,
                        request.bpm,
                        request.duration_samples,
                    )
                }
            }
            ZoomedViewMode::Scrolling => {
                visible_range(
                    request.playhead,
                    request.zoom_bars,
                    request.bpm,
                    request.duration_samples,
                )
            }
        };

        // Cache window sizing depends on view mode
        let (cache_start, cache_end) = match request.view_mode {
            ZoomedViewMode::Scrolling => {
                // In scrolling mode, cache a larger window to reduce recomputation
                let window_size = end - start;
                let cs = start.saturating_sub(window_size / 2);
                let ce = (end + window_size / 2).min(request.duration_samples);
                (cs, ce)
            }
            ZoomedViewMode::FixedBuffer => {
                // In fixed buffer mode, cache exactly the visible range
                (start, end)
            }
        };

        let cache_len = (cache_end - cache_start) as usize;
        if cache_len == 0 || request.width == 0 || request.duration_samples == 0 {
            // Send empty result
            let _ = tx.send(PeaksComputeResult {
                id: request.id,
                cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
                cache_start: 0,
                cache_end: 0,
                zoom_bars: request.zoom_bars,
            });
            continue;
        }

        // Resolution scaling depends on view mode
        // Import DEFAULT_ZOOM_BARS value (8)
        const DEFAULT_ZOOM_BARS: u32 = 8;

        let effective_width = match request.view_mode {
            ZoomedViewMode::Scrolling => {
                // Scrolling mode: reduce resolution when zoomed out, keep base when zoomed in
                let zoom_ratio = request.zoom_bars as f64 / DEFAULT_ZOOM_BARS as f64;
                if zoom_ratio > 1.0 {
                    // Zoomed out: reduce resolution
                    (request.width as f64 / zoom_ratio.sqrt()).max(request.width as f64 / 4.0) as usize
                } else {
                    // Zoomed in or at default: keep base width
                    request.width
                }
            }
            ZoomedViewMode::FixedBuffer => {
                // Fixed buffer mode (slicer): use base width, no scaling
                request.width
            }
        };

        log::debug!(
            "Peaks compute: view_mode={:?}, base_width={}, effective_width={}",
            request.view_mode, request.width, effective_width
        );

        // Compute peaks for the cached window
        let mut cached_peaks = generate_peaks_for_range(
            &request.stems,
            cache_start,
            cache_end,
            effective_width,
        );

        // Apply Gaussian smoothing for smoother waveform display
        for stem_idx in 0..4 {
            if cached_peaks[stem_idx].len() >= 5 {
                cached_peaks[stem_idx] = smooth_peaks_gaussian(&cached_peaks[stem_idx]);
            }
        }

        let elapsed = start_time.elapsed();
        log::debug!(
            "Peaks computed for id={} in {:?} ({}..{}, {} peaks/stem)",
            request.id,
            elapsed,
            cache_start,
            cache_end,
            cached_peaks[0].len()
        );

        // Send result back to UI thread
        let _ = tx.send(PeaksComputeResult {
            id: request.id,
            cached_peaks,
            cache_start,
            cache_end,
            zoom_bars: request.zoom_bars,
        });
    }

    log::debug!("Peaks computer thread shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_visible_range() {
        let duration = 1_000_000u64;
        let bpm = 120.0;

        // At playhead 500000, zoom 8 bars = 8 * 88200 = 705600 samples window
        // Half = 352800, so range should be (147200, 852800) but clamped to duration
        let (start, end) = visible_range(500000, 8, bpm, duration);
        assert!(start < 500000);
        assert!(end > 500000);
        assert!(end <= duration);
    }

    #[test]
    fn test_peaks_computer_spawn() {
        let computer = PeaksComputer::spawn();
        // Just verify it starts without panicking
        assert!(computer.try_recv().is_none());
    }
}
