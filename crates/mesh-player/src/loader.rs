//! Background track loader for Mesh DJ Player
//!
//! Moves expensive track loading operations (file I/O, waveform computation)
//! off the UI thread to prevent audio stuttering during track loads.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{LoadedTrack, StemBuffers};
use mesh_core::engine::PreparedTrack;
use mesh_widgets::{CueMarker, OverviewState, ZoomedState, CUE_COLORS};

/// Request to load a track in the background
#[derive(Debug)]
pub struct TrackLoadRequest {
    /// Deck index (0-3)
    pub deck_idx: usize,
    /// Path to the audio file
    pub path: PathBuf,
}

/// Result of a background track load
///
/// Contains a `PreparedTrack` instead of raw `LoadedTrack` so the UI thread
/// can apply the track with minimal mutex hold time (~1ms vs 10-50ms).
pub struct TrackLoadResult {
    /// Deck index (0-3)
    pub deck_idx: usize,
    /// Pre-prepared track for fast application (or error message)
    ///
    /// The expensive work (cue label cloning, metadata parsing) is already done.
    /// Use `engine.load_track_fast()` to apply with minimal lock time.
    pub result: Result<PreparedTrack, String>,
    /// Pre-computed overview waveform state
    pub overview_state: OverviewState,
    /// Pre-computed zoomed waveform state
    pub zoomed_state: ZoomedState,
    /// Stem buffers for waveform recomputation (Shared for RT-safe deallocation)
    pub stems: Shared<StemBuffers>,
}

/// Handle to the background loader thread
pub struct TrackLoader {
    /// Channel to send load requests
    tx: Sender<TrackLoadRequest>,
    /// Channel to receive load results
    rx: Receiver<TrackLoadResult>,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl TrackLoader {
    /// Spawn the background loader thread
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<TrackLoadRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<TrackLoadResult>();

        let handle = thread::Builder::new()
            .name("track-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx);
            })
            .expect("Failed to spawn track loader thread");

        Self {
            tx: request_tx,
            rx: result_rx,
            _handle: handle,
        }
    }

    /// Request loading a track (non-blocking)
    pub fn load(&self, deck_idx: usize, path: PathBuf) -> Result<(), String> {
        self.tx
            .send(TrackLoadRequest { deck_idx, path })
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Try to receive a completed load result (non-blocking)
    pub fn try_recv(&self) -> Option<TrackLoadResult> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                log::error!("Loader thread disconnected unexpectedly");
                None
            }
        }
    }
}

/// The background loader thread function
fn loader_thread(rx: Receiver<TrackLoadRequest>, tx: Sender<TrackLoadResult>) {
    log::info!("Track loader thread started");

    while let Ok(request) = rx.recv() {
        log::info!(
            "[PERF] Loader: Starting track load for deck {}: {:?}",
            request.deck_idx,
            request.path
        );

        let total_start = std::time::Instant::now();

        // Load the track (expensive file I/O)
        let load_start = std::time::Instant::now();
        let result = LoadedTrack::load(&request.path);
        log::info!("[PERF] Loader: LoadedTrack::load() took {:?}", load_start.elapsed());

        match result {
            Ok(track) => {
                let duration = track.duration_samples as u64;
                let bpm = track.metadata.bpm.unwrap_or(120.0);

                // Create cue markers for display
                let cue_markers: Vec<CueMarker> = track
                    .metadata
                    .cue_points
                    .iter()
                    .map(|cue| {
                        let position = if duration > 0 {
                            cue.sample_position as f64 / duration as f64
                        } else {
                            0.0
                        };
                        CueMarker {
                            position,
                            label: cue.label.clone(),
                            color: CUE_COLORS[(cue.index as usize) % 8],
                            index: cue.index,
                        }
                    })
                    .collect();

                // Pre-compute overview waveform (from preview if available)
                let overview_state = if let Some(ref preview) = track.metadata.waveform_preview {
                    OverviewState::from_preview(
                        preview,
                        &track.metadata.beat_grid.beats,
                        &track.metadata.cue_points,
                        duration,
                    )
                } else {
                    OverviewState::empty_with_message(
                        "No waveform preview",
                        &track.metadata.cue_points,
                        duration,
                    )
                };

                // Pre-compute zoomed waveform state
                let mut zoomed_state = ZoomedState::from_metadata(
                    bpm,
                    track.metadata.beat_grid.beats.clone(),
                    cue_markers,
                );
                zoomed_state.set_duration(duration);

                // Share stems Shared between UI and engine (zero-copy!)
                // LoadedTrack uses basedrop::Shared<StemBuffers> for RT-safe deallocation
                // Clone is ~50ns, and when dropped on RT thread, deallocation is deferred to GC
                let stems = track.stems.clone();
                log::info!(
                    "[PERF] Loader: stems.clone() (zero-copy, {} frames, {:.1} MB shared)",
                    stems.len(),
                    (stems.len() * 32) as f64 / 1_000_000.0
                );

                // Compute initial zoomed peaks (expensive but done in background)
                let peaks_start = std::time::Instant::now();
                zoomed_state.compute_peaks(&stems, 0, 1600);
                log::info!("[PERF] Loader: compute_peaks() took {:?}", peaks_start.elapsed());

                // Prepare track for fast application (string cloning happens here)
                // This is the key optimization: all expensive work is done in this
                // background thread, not while holding the engine mutex.
                let prepare_start = std::time::Instant::now();
                let prepared = PreparedTrack::prepare(track);
                log::info!("[PERF] Loader: PreparedTrack::prepare() took {:?}", prepare_start.elapsed());

                log::info!(
                    "[PERF] Loader: Total load time: {:?} for deck {}",
                    total_start.elapsed(),
                    request.deck_idx
                );

                // Send result back to UI thread
                let _ = tx.send(TrackLoadResult {
                    deck_idx: request.deck_idx,
                    result: Ok(prepared),
                    overview_state,
                    zoomed_state,
                    stems,
                });
            }
            Err(e) => {
                log::error!("Failed to load track: {}", e);

                // Send error result
                let _ = tx.send(TrackLoadResult {
                    deck_idx: request.deck_idx,
                    result: Err(e.to_string()),
                    overview_state: OverviewState::new(),
                    zoomed_state: ZoomedState::new(),
                    stems: Shared::new(&mesh_core::engine::gc::gc_handle(), StemBuffers::with_length(0)),
                });
            }
        }
    }

    log::info!("Track loader thread shutting down");
}
