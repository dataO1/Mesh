//! Background track loader for mesh-cue
//!
//! Provides progressive track loading with region-based streaming, matching
//! mesh-player's TrackLoader behavior. Priority regions (hot cues, drop marker,
//! first beat) load first so the DJ can play from entry points immediately,
//! while the rest of the audio fills in progressively.
//!
//! ## Architecture
//!
//! Same mpsc/subscription pattern as PresetLoader and mesh-player's TrackLoader:
//! - Background `std::thread` (not tokio — avoids runtime-in-runtime)
//! - `mpsc::channel` for requests + results
//! - `Arc<Mutex<Receiver>>` for iced subscription integration
//! - Region-based decoding with incremental peak updates

pub mod regions;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{AudioFileReader, StemBuffers, TrackMetadata};
use mesh_core::types::SAMPLE_RATE;
use mesh_widgets::{
    allocate_empty_peaks, compute_highres_width, update_peaks_for_region, DEFAULT_WIDTH,
};

use self::regions::{compute_gaps, compute_priority_regions};

/// Request to load a track in the background
pub struct CueTrackLoadRequest {
    /// Path to the audio file
    pub path: PathBuf,
    /// Pre-loaded metadata from the database
    pub metadata: TrackMetadata,
}

/// Result of a background track load — sent as one or more messages per track.
///
/// The streaming loader sends multiple results per track:
/// 1. `RegionLoaded` × N — incremental peak updates as regions load
/// 2. `Complete` — all audio loaded with final peaks
pub enum CueTrackLoadResult {
    /// Incremental update — a region of audio has been loaded.
    ///
    /// **Visual-only** (stems = None): cheap peak update for smooth waveform growth.
    /// **Playable** (stems = Some): includes full stem buffer clone so the engine can play.
    RegionLoaded {
        /// Stem buffer snapshot — None for visual-only, Some for playable updates
        stems: Option<Shared<StemBuffers>>,
        /// Track duration in samples
        duration_samples: usize,
        /// Full overview peaks (DEFAULT_WIDTH entries per stem)
        overview_peaks: [Vec<(f32, f32)>; 4],
        /// Full highres peaks
        highres_peaks: [Vec<(f32, f32)>; 4],
        /// Path for stale detection
        path: PathBuf,
    },
    /// All audio loaded — stems fully filled, waveform computed.
    Complete {
        stems: Shared<StemBuffers>,
        duration_samples: usize,
        path: PathBuf,
    },
    /// Error during loading
    Error { error: String },
}

/// Type alias for the result receiver (used with subscriptions)
pub type CueTrackLoadResultReceiver = Arc<Mutex<Receiver<CueTrackLoadResult>>>;

/// Handle to the background loader thread
pub struct CueTrackLoader {
    /// Channel to send load requests
    tx: Sender<CueTrackLoadRequest>,
    /// Channel to receive load results (wrapped for subscription support)
    rx: CueTrackLoadResultReceiver,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl CueTrackLoader {
    /// Spawn the background loader thread
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<CueTrackLoadRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<CueTrackLoadResult>();

        let handle = thread::Builder::new()
            .name("cue-track-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx);
            })
            .expect("Failed to spawn cue track loader thread");

        log::info!("CueTrackLoader spawned");

        Self {
            tx: request_tx,
            rx: Arc::new(Mutex::new(result_rx)),
            _handle: handle,
        }
    }

    /// Request loading a track (non-blocking)
    pub fn load(&self, path: PathBuf, metadata: TrackMetadata) -> Result<(), String> {
        self.tx
            .send(CueTrackLoadRequest { path, metadata })
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Get the result receiver for subscription-based message handling
    pub fn result_receiver(&self) -> CueTrackLoadResultReceiver {
        self.rx.clone()
    }
}

/// The background loader thread.
///
/// Receives load requests and processes them with region-based streaming.
fn loader_thread(rx: Receiver<CueTrackLoadRequest>, tx: Sender<CueTrackLoadResult>) {
    log::info!("Cue track loader thread started");

    while let Ok(request) = rx.recv() {
        handle_track_load(request, &tx);
    }

    log::info!("Cue track loader thread shutting down");
}

/// Handle a track load request with streaming support.
///
/// mesh-cue tracks are always 8-channel WAV at 48kHz (pre-separated),
/// so no resampling is needed — we always use the streaming path.
fn handle_track_load(request: CueTrackLoadRequest, tx: &Sender<CueTrackLoadResult>) {
    let path = request.path.clone();
    let metadata = request.metadata;

    log::info!(
        "[LOADER] Starting progressive load: {:?}",
        path.file_name().unwrap_or_default()
    );

    let total_start = std::time::Instant::now();

    // Open the file
    let reader = match AudioFileReader::open(&path) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Failed to open audio file: {}", e);
            let _ = tx.send(CueTrackLoadResult::Error {
                error: e.to_string(),
            });
            return;
        }
    };

    let file_frames = reader.frame_count() as usize;
    // DB-sourced duration is authoritative — FLAC header may include block-size padding
    let metadata_frames = metadata
        .duration_seconds
        .map(|d| (d * SAMPLE_RATE as f64).round() as usize)
        .unwrap_or(file_frames);
    let frame_count = file_frames.min(metadata_frames);

    if file_frames != frame_count {
        log::info!(
            "[LOADER] Capping frame_count: file={} → metadata={} (delta={})",
            file_frames,
            frame_count,
            file_frames - frame_count
        );
    }

    log::info!(
        "[LOADER] Streaming load: frames={}, bpm={:?}, cues={}, path={:?}",
        frame_count,
        metadata.bpm,
        metadata.cue_points.len(),
        path.file_name().unwrap_or_default()
    );

    // 1. Allocate full buffer with silence
    let alloc_start = std::time::Instant::now();
    let mut stems = StemBuffers::with_length(frame_count);
    log::info!(
        "[PERF] Loader: StemBuffers allocation took {:?} ({:.1} MB)",
        alloc_start.elapsed(),
        (frame_count * 32) as f64 / 1_000_000.0
    );

    // 2. Pre-allocate peak arrays for incremental updates
    let bpm = metadata.bpm.unwrap_or(120.0);
    let quality_level: u8 = 0; // mesh-cue reference quality
    let screen_width: u32 = 1920; // mesh-cue reference width
    let highres_width =
        compute_highres_width(frame_count, bpm, screen_width, quality_level);
    let mut overview_peaks = allocate_empty_peaks(DEFAULT_WIDTH);
    let mut highres_peaks = allocate_empty_peaks(highres_width);

    // 3. Compute priority regions around hot cues, drop marker, first beat
    let regions = compute_priority_regions(&metadata, frame_count, SAMPLE_RATE);
    let gaps = compute_gaps(&regions, frame_count);

    let priority_samples: usize = regions.iter().map(|r| r.len()).sum();
    log::info!(
        "[LOADER] Priority regions: {} regions, {} samples ({:.1}% of track), {} gap regions",
        regions.len(),
        priority_samples,
        (priority_samples as f64 / frame_count as f64) * 100.0,
        gaps.len()
    );

    // 4. Parallel priority region decode
    let priority_start = std::time::Instant::now();

    let region_results: Vec<Result<StemBuffers, String>> = if regions.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = regions
                .iter()
                .enumerate()
                .map(|(i, region)| {
                    let reader_ref = &reader;
                    s.spawn(move || {
                        let len = region.len();
                        let start = std::time::Instant::now();
                        let result = reader_ref
                            .decode_region(region.start, len)
                            .map_err(|e| format!("Priority region {} decode failed: {}", i, e));
                        log::info!(
                            "[PERF] Loader: Priority region {} ({} frames from {}) decoded in {:?}",
                            i,
                            len,
                            region.start,
                            start.elapsed()
                        );
                        result
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    // Check for errors before merging
    for result in &region_results {
        if let Err(e) = result {
            log::error!("Failed to decode priority region: {}", e);
            let _ = tx.send(CueTrackLoadResult::Error { error: e.clone() });
            return;
        }
    }

    // Phase B: Sequential merge + peak updates + messages
    for (i, (region, result)) in regions.iter().zip(region_results.into_iter()).enumerate() {
        let local = result.unwrap();
        stems.copy_region_from(&local, region.start, region.len());
        drop(local); // Free region buffer immediately

        update_peaks_for_region(
            &stems,
            &mut overview_peaks,
            region.start,
            region.end,
            frame_count,
            DEFAULT_WIDTH,
        );
        update_peaks_for_region(
            &stems,
            &mut highres_peaks,
            region.start,
            region.end,
            frame_count,
            highres_width,
        );

        let is_last = i + 1 == regions.len();
        let _ = tx.send(CueTrackLoadResult::RegionLoaded {
            stems: if is_last {
                Some(Shared::new(
                    &mesh_core::engine::gc::gc_handle(),
                    stems.clone(),
                ))
            } else {
                None
            },
            duration_samples: frame_count,
            overview_peaks: overview_peaks.clone(),
            highres_peaks: highres_peaks.clone(),
            path: path.clone(),
        });
    }
    log::info!(
        "[PERF] Loader: Parallel priority regions took {:?} ({} regions)",
        priority_start.elapsed(),
        regions.len()
    );

    // 5. Parallel gap decode
    let gap_start = std::time::Instant::now();

    let gap_results: Vec<Result<StemBuffers, String>> = if gaps.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = gaps
                .iter()
                .enumerate()
                .map(|(i, gap)| {
                    let reader_ref = &reader;
                    s.spawn(move || {
                        let len = gap.len();
                        let start = std::time::Instant::now();
                        let result = reader_ref
                            .decode_region(gap.start, len)
                            .map_err(|e| format!("Gap region {} decode failed: {}", i, e));
                        log::info!(
                            "[PERF] Loader: Gap region {} ({} frames from {}) decoded in {:?}",
                            i,
                            len,
                            gap.start,
                            start.elapsed()
                        );
                        result
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    // Check for errors
    for result in &gap_results {
        if let Err(e) = result {
            log::error!("Failed to decode gap region: {}", e);
            let _ = tx.send(CueTrackLoadResult::Error { error: e.clone() });
            return;
        }
    }

    // Sequential merge + peak updates — send visual update after each gap
    let gap_count = gaps.len();
    for (i, (gap, result)) in gaps.iter().zip(gap_results.into_iter()).enumerate() {
        let local = result.unwrap();
        stems.copy_region_from(&local, gap.start, gap.len());
        drop(local);

        update_peaks_for_region(
            &stems,
            &mut overview_peaks,
            gap.start,
            gap.end,
            frame_count,
            DEFAULT_WIDTH,
        );
        update_peaks_for_region(
            &stems,
            &mut highres_peaks,
            gap.start,
            gap.end,
            frame_count,
            highres_width,
        );

        // Send incremental peak update so waveform fills progressively.
        // Last gap includes stem clone for final audio upgrade.
        let is_last = i + 1 == gap_count;
        let _ = tx.send(CueTrackLoadResult::RegionLoaded {
            stems: if is_last {
                Some(Shared::new(
                    &mesh_core::engine::gc::gc_handle(),
                    stems.clone(),
                ))
            } else {
                None
            },
            duration_samples: frame_count,
            overview_peaks: overview_peaks.clone(),
            highres_peaks: highres_peaks.clone(),
            path: path.clone(),
        });
    }
    log::info!(
        "[PERF] Loader: Parallel gap regions took {:?} ({} regions)",
        gap_start.elapsed(),
        gaps.len()
    );

    // 6. Send Complete — peaks already computed incrementally
    let final_stems = Shared::new(&mesh_core::engine::gc::gc_handle(), stems);

    let _ = tx.send(CueTrackLoadResult::Complete {
        stems: final_stems,
        duration_samples: frame_count,
        path,
    });

    log::info!(
        "[PERF] Loader: Total progressive load time: {:?}",
        total_start.elapsed()
    );
}
