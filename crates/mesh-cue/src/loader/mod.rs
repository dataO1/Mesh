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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{AudioFileReader, StemBuffers, TrackMetadata};
use mesh_core::types::SAMPLE_RATE;
use mesh_widgets::{
    SharedPeakBuffer, compute_highres_width, update_peaks_for_region_flat, DEFAULT_WIDTH,
};

use self::regions::{compute_gaps, compute_priority_regions};

/// Number of parallel decode workers. Capped to avoid overloading the CPU
/// while still maximising I/O throughput for in-memory FLAC/WAV decoding.
const DECODE_WORKERS: usize = 4;

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
        /// Shared overview peaks (Arc clone = refcount bump, zero data copy)
        shared_overview: Arc<SharedPeakBuffer>,
        /// Shared highres peaks (Arc clone = refcount bump, zero data copy)
        shared_highres: Arc<SharedPeakBuffer>,
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
    // Pre-allocate shared peak buffers — written to by merge thread, read by UI
    let shared_overview = Arc::new(SharedPeakBuffer::new_empty(DEFAULT_WIDTH as u32, 4));
    let shared_highres = Arc::new(SharedPeakBuffer::new_empty(highres_width as u32, 4));

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

    // 4. Build unified work queue: priority regions first, then gap regions.
    //    Workers pull from this queue using an atomic index, so priority
    //    regions are always decoded first. As each decode finishes, the
    //    result is sent to the merge channel for immediate UI update.
    let num_priority = regions.len();
    let all_regions: Vec<&regions::LoadRegion> =
        regions.iter().chain(gaps.iter()).collect();
    let total_regions = all_regions.len();

    if total_regions == 0 {
        log::info!("[LOADER] No regions to decode (empty track?)");
    } else {
        let decode_start = std::time::Instant::now();
        let work_index = Arc::new(AtomicUsize::new(0));
        let num_workers = DECODE_WORKERS.min(total_regions);

        // Channel for decoded results: (region_index, decoded_buffer)
        let (decode_tx, decode_rx) =
            std::sync::mpsc::channel::<(usize, Result<StemBuffers, String>)>();

        // Spawn parallel decode workers inside a thread scope.
        // Workers claim tasks atomically; priority regions are at indices 0..num_priority.
        std::thread::scope(|s| {
            for _worker in 0..num_workers {
                let idx = work_index.clone();
                let dtx = decode_tx.clone();
                let reader_ref = &reader;
                let regions_ref = &all_regions;

                s.spawn(move || {
                    loop {
                        let i = idx.fetch_add(1, Ordering::SeqCst);
                        if i >= total_regions {
                            break;
                        }
                        let region = regions_ref[i];
                        let len = region.len();
                        let t = std::time::Instant::now();
                        let result = reader_ref
                            .decode_region(region.start, len)
                            .map_err(|e| format!("Region {} decode failed: {}", i, e));
                        let kind = if i < num_priority { "Priority" } else { "Gap" };
                        log::info!(
                            "[PERF] Loader: {} region {} ({} frames from {}) decoded in {:?}",
                            kind, i, len, region.start, t.elapsed()
                        );
                        // If send fails, merge thread is gone (error already reported)
                        let _ = dtx.send((i, result));
                    }
                });
            }
            // Drop our copy so decode_rx closes when all workers finish
            drop(decode_tx);

            // Merge thread: receive decoded regions as they complete,
            // merge into stems buffer, update peaks, send UI messages.
            let mut priority_merged = 0usize;
            let mut total_merged = 0usize;

            for (i, result) in decode_rx.iter() {
                let local = match result {
                    Ok(buf) => buf,
                    Err(e) => {
                        log::error!("Decode failed: {}", e);
                        let _ = tx.send(CueTrackLoadResult::Error { error: e });
                        return;
                    }
                };

                let region = all_regions[i];
                stems.copy_region_from(&local, region.start, region.len());
                drop(local);

                // Write peaks directly into shared flat buffers (zero-copy path)
                {
                    let mut data = shared_overview.write_data();
                    update_peaks_for_region_flat(&stems, &mut data, DEFAULT_WIDTH as u32, region.start, region.end, frame_count);
                }
                shared_overview.increment_generation();
                {
                    let mut data = shared_highres.write_data();
                    update_peaks_for_region_flat(&stems, &mut data, highres_width as u32, region.start, region.end, frame_count);
                }
                shared_highres.increment_generation();

                let is_priority = i < num_priority;
                if is_priority {
                    priority_merged += 1;
                }
                total_merged += 1;

                // Include a playable stems snapshot when:
                // - All priority regions have been merged (first interaction point)
                // - All regions are done (final audio upgrade)
                let all_priority_done = priority_merged == num_priority;
                let all_done = total_merged == total_regions;
                let send_stems = (is_priority && all_priority_done) || all_done;

                // Send lightweight message — Arc clone is just a refcount bump
                let _ = tx.send(CueTrackLoadResult::RegionLoaded {
                    stems: if send_stems {
                        Some(Shared::new(
                            &mesh_core::engine::gc::gc_handle(),
                            stems.clone(),
                        ))
                    } else {
                        None
                    },
                    duration_samples: frame_count,
                    shared_overview: shared_overview.clone(),
                    shared_highres: shared_highres.clone(),
                    path: path.clone(),
                });

                if all_priority_done && is_priority {
                    log::info!(
                        "[PERF] Loader: All {} priority regions merged at {:?}",
                        num_priority, decode_start.elapsed()
                    );
                }
            }

            log::info!(
                "[PERF] Loader: All {} regions decoded+merged in {:?} ({} workers)",
                total_regions, decode_start.elapsed(), num_workers
            );
        });
    }

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
