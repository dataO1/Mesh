//! Background track loader for Mesh DJ Player
//!
//! Moves expensive track loading operations (file I/O, waveform computation)
//! off the UI thread to prevent audio stuttering during track loads.
//!
//! The loader thread automatically resamples tracks to match the audio system's
//! sample rate, ensuring correct playback speed regardless of the system configuration.
//!
//! Note: Linked stem loading is handled separately by `mesh_core::loader::LinkedStemLoader`.
//!
//! ## Database Independence
//!
//! The loader is deliberately database-agnostic. It receives pre-loaded metadata
//! with each request, allowing the domain layer to choose which database to query
//! (local or USB). This enables proper metadata loading when playing from USB drives.

pub mod regions;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{AudioFileReader, LoadedTrack, StemBuffers, TrackMetadata};
use mesh_core::engine::PreparedTrack;
use mesh_widgets::{
    CueMarker, OverviewState, ZoomedState, CUE_COLORS,
    allocate_empty_peaks, compute_highres_width, generate_peaks, update_peaks_for_region,
    DEFAULT_WIDTH,
};

use self::regions::{compute_gaps, compute_priority_regions};


/// Request to load a track in the background
///
/// Contains pre-loaded metadata from the domain layer. This allows the loader
/// to be database-agnostic - it doesn't need to know whether the track is from
/// local storage or USB.
pub struct TrackLoadRequest {
    /// Deck index (0-3)
    pub deck_idx: usize,
    /// Path to the audio file
    pub path: PathBuf,
    /// Pre-loaded metadata from the appropriate database (local or USB)
    pub metadata: TrackMetadata,
    /// Waveform quality level: 0=Low, 1=Medium, 2=High, 3=Ultra
    pub quality_level: u8,
    /// Screen width in pixels (for BPM-aware peak resolution)
    pub screen_width: u32,
}

/// Result of a background track load — sent as one or more messages per track.
///
/// The streaming loader sends multiple results per track:
/// 1. `RegionLoaded` × N — incremental peak updates as regions load
/// 2. `Complete` — all audio loaded with final waveform peaks
///
/// Files that need resampling skip streaming and send a single `Complete`.
pub enum TrackLoadResult {
    /// Incremental update — a region of audio has been loaded.
    ///
    /// **Visual-only** (stems = None): cheap peak update (~2 MB) for smooth
    /// waveform growth. Sent after each small read batch.
    ///
    /// **Playable** (stems = Some): includes a full stem buffer clone (~460 MB)
    /// so the engine can play from loaded regions. Sent at clone intervals.
    RegionLoaded {
        deck_idx: usize,
        /// Stem buffer snapshot — None for visual-only updates, Some for playable updates
        stems: Option<Shared<StemBuffers>>,
        /// Track duration in samples
        duration_samples: usize,
        /// Full overview peaks (800 entries per stem, ~25 KB clone)
        overview_peaks: [Vec<(f32, f32)>; 4],
        /// Full highres peaks (65536 entries per stem, ~2 MB clone)
        highres_peaks: [Vec<(f32, f32)>; 4],
        path: PathBuf,
    },
    /// All audio loaded — stems fully filled, waveform computed.
    /// Upgrade engine stems + update waveform display.
    Complete {
        deck_idx: usize,
        result: Result<PreparedTrack, String>,
        overview_state: OverviewState,
        zoomed_state: ZoomedState,
        stems: Shared<StemBuffers>,
        duration_samples: usize,
        path: PathBuf,
        /// When true, peaks were computed incrementally via RegionLoaded messages —
        /// skip overview/zoomed state replacement and redundant UpgradeStems.
        incremental: bool,
    },
    /// Error during loading
    Error {
        deck_idx: usize,
        error: String,
    },
}

/// Type alias for the result receiver (used with subscriptions)
pub type TrackLoadResultReceiver = Arc<Mutex<Receiver<TrackLoadResult>>>;

/// Handle to the background loader thread
pub struct TrackLoader {
    /// Channel to send load requests
    tx: Sender<TrackLoadRequest>,
    /// Channel to receive load results (wrapped for subscription support)
    rx: TrackLoadResultReceiver,
    /// Target sample rate for loading (audio system's sample rate)
    target_sample_rate: Arc<AtomicU32>,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl TrackLoader {
    /// Spawn the background loader thread
    ///
    /// # Arguments
    /// * `target_sample_rate` - Audio system's sample rate for resampling tracks on load
    ///
    /// Note: The loader is database-agnostic. Metadata is provided with each
    /// load request by the domain layer, which knows which database to query.
    pub fn spawn(target_sample_rate: u32) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<TrackLoadRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<TrackLoadResult>();

        // Store sample rate in Arc<AtomicU32> so loader thread can access it
        let rate = Arc::new(AtomicU32::new(target_sample_rate));
        let rate_for_thread = rate.clone();

        let handle = thread::Builder::new()
            .name("track-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx, rate_for_thread);
            })
            .expect("Failed to spawn track loader thread");

        log::info!("TrackLoader spawned with target sample rate: {} Hz", target_sample_rate);

        Self {
            tx: request_tx,
            rx: Arc::new(Mutex::new(result_rx)),
            target_sample_rate: rate,
            _handle: handle,
        }
    }

    /// Update the target sample rate (if audio system rate changes)
    pub fn set_sample_rate(&self, sample_rate: u32) {
        self.target_sample_rate.store(sample_rate, Ordering::SeqCst);
        log::info!("TrackLoader target sample rate updated to: {} Hz", sample_rate);
    }

    /// Get the current target sample rate
    pub fn sample_rate(&self) -> u32 {
        self.target_sample_rate.load(Ordering::SeqCst)
    }

    /// Request loading a track (non-blocking)
    ///
    /// # Arguments
    /// * `deck_idx` - Deck to load the track into (0-3)
    /// * `path` - Path to the audio file
    /// * `metadata` - Pre-loaded metadata from the appropriate database
    ///
    /// The metadata should be loaded by the domain layer from whichever database
    /// is currently active (local or USB).
    pub fn load(&self, deck_idx: usize, path: PathBuf, metadata: TrackMetadata, quality_level: u8, screen_width: u32) -> Result<(), String> {
        self.tx
            .send(TrackLoadRequest { deck_idx, path, metadata, quality_level, screen_width })
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Get the result receiver for subscription-based message handling
    ///
    /// Use with `mesh_widgets::mpsc_subscription` to receive track load results
    /// via iced subscription instead of polling in Tick handler.
    ///
    /// # Example
    /// ```ignore
    /// fn subscription(&self) -> Subscription<Message> {
    ///     mpsc_subscription(self.track_loader.result_receiver())
    ///         .map(Message::TrackLoaded)
    /// }
    /// ```
    pub fn result_receiver(&self) -> TrackLoadResultReceiver {
        self.rx.clone()
    }

    /// Try to receive a completed load result (non-blocking)
    ///
    /// Note: Consider using `result_receiver()` with subscriptions instead
    /// for cleaner message-driven architecture.
    pub fn try_recv(&self) -> Option<TrackLoadResult> {
        match self.rx.lock().ok().and_then(|rx| rx.try_recv().ok()) {
            Some(result) => Some(result),
            None => None,
        }
    }
}

/// The background loader dispatch thread.
///
/// Receives load requests and spawns a dedicated thread per load for parallel
/// execution. Uses std::thread (not rayon) because track loads are long-running
/// and I/O-bound — rayon workers would be pinned for minutes, starving the
/// audio engine's par_iter and other short rayon tasks.
fn loader_thread(
    rx: Receiver<TrackLoadRequest>,
    tx: Sender<TrackLoadResult>,
    target_sample_rate: Arc<AtomicU32>,
) {
    log::info!("Track loader dispatch thread started");

    while let Ok(request) = rx.recv() {
        let tx = tx.clone();
        let rate = target_sample_rate.clone();
        let deck_idx = request.deck_idx;
        if let Err(e) = thread::Builder::new()
            .name(format!("track-load-{}", deck_idx))
            .spawn(move || {
                handle_track_load(request, tx, rate);
            })
        {
            log::error!("Failed to spawn load thread: {}", e);
        }
    }

    log::info!("Track loader dispatch thread shutting down");
}

/// Handle a track load request with streaming support.
///
/// For native-rate files: reads priority regions first (hot cues, drop marker),
/// sends a PriorityReady result, then reads the remaining gaps and sends Complete.
///
/// For files needing resampling: falls back to full load (rubato needs contiguous
/// blocks), sends a single Complete result.
fn handle_track_load(
    request: TrackLoadRequest,
    tx: Sender<TrackLoadResult>,
    target_sample_rate: Arc<AtomicU32>,
) {
    let sample_rate = target_sample_rate.load(Ordering::SeqCst);
    let deck_idx = request.deck_idx;
    let path = request.path.clone();

    log::info!(
        "[PERF] Loader: Starting track load for deck {}: {:?} (target: {} Hz)",
        deck_idx, path, sample_rate
    );

    let total_start = std::time::Instant::now();

    // Try to open the file and check if we can use the streaming path
    let reader_result = AudioFileReader::open(&path);
    let reader = match reader_result {
        Ok(r) => r,
        Err(e) => {
            log::error!("Failed to open audio file: {}", e);
            let _ = tx.send(TrackLoadResult::Error {
                deck_idx,
                error: e.to_string(),
            });
            return;
        }
    };

    if reader.needs_resampling(sample_rate) {
        // RESAMPLING FALLBACK: rubato needs contiguous blocks, can't do region-based loading.
        // Use the existing full-load path via LoadedTrack::load_with_metadata().
        log::info!("[LOADER] File needs resampling ({} Hz → {} Hz), using full-load path",
            reader.format().sample_rate, sample_rate);
        drop(reader); // Close the reader, load_with_metadata will open its own
        handle_full_load(request, &tx, sample_rate);
    } else {
        // STREAMING PATH: native rate, can read regions directly
        handle_streaming_load(request, &tx, reader, sample_rate);
    }

    log::info!(
        "[PERF] Loader: Total load time: {:?} for deck {}",
        total_start.elapsed(), deck_idx
    );
}

/// Full-load fallback for files that need resampling.
/// Same as the original loader: read everything, resample, send one Complete result.
fn handle_full_load(
    request: TrackLoadRequest,
    tx: &Sender<TrackLoadResult>,
    sample_rate: u32,
) {
    let deck_idx = request.deck_idx;
    let path = request.path.clone();

    let load_start = std::time::Instant::now();
    let result = LoadedTrack::load_with_metadata(&request.path, request.metadata, sample_rate);
    log::info!("[PERF] Loader: LoadedTrack::load_with_metadata({} Hz) took {:?}",
        sample_rate, load_start.elapsed());

    match result {
        Ok(track) => {
            let duration_samples = track.duration_samples;
            let (overview_state, zoomed_state) = build_waveform_states(&track, request.quality_level, request.screen_width);
            let stems = track.stems.clone();
            let prepared = PreparedTrack::prepare(track);

            let _ = tx.send(TrackLoadResult::Complete {
                deck_idx,
                result: Ok(prepared),
                overview_state,
                zoomed_state,
                stems,
                duration_samples,
                path,
                incremental: false,
            });
        }
        Err(e) => {
            log::error!("Failed to load track: {}", e);
            let _ = tx.send(TrackLoadResult::Error {
                deck_idx,
                error: e.to_string(),
            });
        }
    }
}

/// Streaming load for native-rate files with incremental visual feedback.
///
/// Reads priority regions (hot cues, drop marker) first, then gap regions in
/// batches. After each read, computes affected peak columns and sends a
/// `RegionLoaded` message so the waveform grows visually. At the end, sends
/// a single `Complete` with the full stems and final waveform states.
///
/// Each `RegionLoaded` message carries a cloned `Shared<StemBuffers>` snapshot
/// (~460 MB per clone, ~100 ms) so the engine can play from any loaded region
/// immediately. The visible waveform peaks always match the playable audio.
fn handle_streaming_load(
    request: TrackLoadRequest,
    tx: &Sender<TrackLoadResult>,
    reader: AudioFileReader,
    sample_rate: u32,
) {
    let deck_idx = request.deck_idx;
    let path = request.path.clone();
    let metadata = request.metadata;

    let file_frames = reader.frame_count() as usize;
    // DB-sourced duration is authoritative — FLAC header may include block-size padding
    let metadata_frames = metadata.duration_seconds
        .map(|d| (d * sample_rate as f64).round() as usize)
        .unwrap_or(file_frames);
    let frame_count = file_frames.min(metadata_frames);
    if file_frames != frame_count {
        log::info!(
            "[LOADER] Capping frame_count: file={} → metadata={} (delta={})",
            file_frames, frame_count, file_frames - frame_count
        );
    }

    log::info!(
        "[LOADER] Streaming load: deck={}, frames={}, bpm={:?}, cues={}, path={:?}",
        deck_idx, frame_count, metadata.bpm, metadata.cue_points.len(),
        path.file_name().unwrap_or_default()
    );

    // 1. Allocate full buffer with silence (single allocation, no clone)
    let alloc_start = std::time::Instant::now();
    let mut stems = StemBuffers::with_length(frame_count);
    log::info!("[PERF] Loader: StemBuffers allocation took {:?} ({:.1} MB)",
        alloc_start.elapsed(), (frame_count * 32) as f64 / 1_000_000.0);

    // 2. Pre-allocate peak arrays for incremental updates
    let bpm = metadata.bpm.unwrap_or(120.0);
    let highres_width = compute_highres_width(frame_count, bpm, request.screen_width, request.quality_level);
    {
        let samples_per_beat = (48000.0_f64 * 60.0 / bpm) as usize;
        let samples_per_bar = samples_per_beat * 4;
        let ref_zoom = 4u32; // PEAK_REFERENCE_ZOOM_BARS
        let window_at_ref = samples_per_bar * ref_zoom as usize;
        let ppp_at_ref = if window_at_ref > 0 {
            highres_width as f64 * window_at_ref as f64 / (frame_count as f64 * request.screen_width as f64)
        } else { 0.0 };
        log::info!(
            "[RENDER] Highres peaks: {} peaks | quality={} bpm={:.1} screen={}px | \
             ref_zoom={}bars → {:.2} pp/px | samples_per_bar={} | track={}samples ({:.1}s)",
            highres_width, request.quality_level, bpm, request.screen_width,
            ref_zoom, ppp_at_ref, samples_per_bar,
            frame_count, frame_count as f64 / 48000.0,
        );
    }
    let mut overview_peaks = allocate_empty_peaks(DEFAULT_WIDTH);
    let mut highres_peaks = allocate_empty_peaks(highres_width);

    // 3. Compute priority regions around hot cues, drop marker, first beat
    let regions = compute_priority_regions(&metadata, frame_count, sample_rate);
    let gaps = compute_gaps(&regions, frame_count);

    let priority_samples: usize = regions.iter().map(|r| r.len()).sum();
    log::info!(
        "[LOADER] Priority regions: {} regions, {} samples ({:.1}% of track), {} gap regions",
        regions.len(), priority_samples,
        (priority_samples as f64 / frame_count as f64) * 100.0,
        gaps.len()
    );

    // 4. PARALLEL priority region decode using std::thread::scope.
    //    Each thread creates its own decoder from Arc<[u8]> and decodes into
    //    a local StemBuffers — no shared mutable state. Results are merged
    //    sequentially after all threads complete.
    //    Uses OS threads (not rayon) to avoid starving the audio engine's rayon pool.
    let priority_start = std::time::Instant::now();

    // Phase A: Parallel decode — each region decoded independently
    let region_results: Vec<Result<StemBuffers, String>> = if regions.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = regions.iter().enumerate().map(|(i, region)| {
                let reader_ref = &reader;
                s.spawn(move || {
                    let len = region.len();
                    let start = std::time::Instant::now();
                    let result = reader_ref.decode_region(region.start, len)
                        .map_err(|e| format!("Priority region {} decode failed: {}", i, e));
                    log::info!("[PERF] Loader: Priority region {} ({} frames from {}) decoded in {:?}",
                        i, len, region.start, start.elapsed());
                    result
                })
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    // Check for errors before merging
    for result in &region_results {
        if let Err(e) = result {
            log::error!("Failed to decode priority region: {}", e);
            let _ = tx.send(TrackLoadResult::Error {
                deck_idx,
                error: e.clone(),
            });
            return;
        }
    }

    // Phase B: Sequential merge + peak updates + messages
    for (i, (region, result)) in regions.iter().zip(region_results.into_iter()).enumerate() {
        let local = result.unwrap();
        stems.copy_region_from(&local, region.start, region.len());
        drop(local); // Free region buffer immediately

        update_peaks_for_region(&stems, &mut overview_peaks, region.start, region.end, frame_count, DEFAULT_WIDTH);
        update_peaks_for_region(&stems, &mut highres_peaks, region.start, region.end, frame_count, highres_width);

        let is_last = i + 1 == regions.len();
        let _ = tx.send(TrackLoadResult::RegionLoaded {
            deck_idx,
            stems: if is_last {
                Some(Shared::new(&mesh_core::engine::gc::gc_handle(), stems.clone()))
            } else {
                None
            },
            duration_samples: frame_count,
            overview_peaks: overview_peaks.clone(),
            highres_peaks: highres_peaks.clone(),
            path: path.clone(),
        });
    }
    log::info!("[PERF] Loader: Parallel priority regions took {:?} ({} regions, {} threads)",
        priority_start.elapsed(), regions.len(), regions.len());

    // 5. PARALLEL gap decode — each gap region decoded independently.
    //    Same pattern as priority regions: parallel decode, sequential merge.
    let gap_start = std::time::Instant::now();

    let gap_results: Vec<Result<StemBuffers, String>> = if gaps.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = gaps.iter().enumerate().map(|(i, gap)| {
                let reader_ref = &reader;
                s.spawn(move || {
                    let len = gap.len();
                    let start = std::time::Instant::now();
                    let result = reader_ref.decode_region(gap.start, len)
                        .map_err(|e| format!("Gap region {} decode failed: {}", i, e));
                    log::info!("[PERF] Loader: Gap region {} ({} frames from {}) decoded in {:?}",
                        i, len, gap.start, start.elapsed());
                    result
                })
            }).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    // Check for errors
    for result in &gap_results {
        if let Err(e) = result {
            log::error!("Failed to decode gap region: {}", e);
            let _ = tx.send(TrackLoadResult::Error {
                deck_idx,
                error: e.clone(),
            });
            return;
        }
    }

    // Sequential merge + peak updates — send visual update after each gap
    let gap_count = gaps.len();
    for (i, (gap, result)) in gaps.iter().zip(gap_results.into_iter()).enumerate() {
        let local = result.unwrap();
        stems.copy_region_from(&local, gap.start, gap.len());
        drop(local);

        update_peaks_for_region(&stems, &mut overview_peaks, gap.start, gap.end, frame_count, DEFAULT_WIDTH);
        update_peaks_for_region(&stems, &mut highres_peaks, gap.start, gap.end, frame_count, highres_width);

        // Send incremental peak update so waveform fills progressively.
        // Last gap includes stem clone for final audio upgrade.
        let is_last = i + 1 == gap_count;
        let _ = tx.send(TrackLoadResult::RegionLoaded {
            deck_idx,
            stems: if is_last {
                Some(Shared::new(&mesh_core::engine::gc::gc_handle(), stems.clone()))
            } else {
                None
            },
            duration_samples: frame_count,
            overview_peaks: overview_peaks.clone(),
            highres_peaks: highres_peaks.clone(),
            path: path.clone(),
        });
    }
    log::info!("[PERF] Loader: Parallel gap regions took {:?} ({} regions, {} threads)",
        gap_start.elapsed(), gaps.len(), gaps.len());

    // 6. Send finalization — peaks already computed incrementally, skip build_waveform_states()
    let final_stems = Shared::new(&mesh_core::engine::gc::gc_handle(), stems);

    let duration_samples = frame_count;
    let duration_seconds = frame_count as f64 / sample_rate as f64;
    let track = LoadedTrack {
        path: path.clone(),
        stems: final_stems.clone(),
        metadata,
        duration_samples,
        duration_seconds,
    };
    let prepared = PreparedTrack::prepare(track);

    let _ = tx.send(TrackLoadResult::Complete {
        deck_idx,
        result: Ok(prepared),
        overview_state: OverviewState::default(),
        zoomed_state: ZoomedState::default(),
        stems: final_stems,
        duration_samples,
        path,
        incremental: true,
    });
}

/// Build overview and zoomed waveform states from a loaded track.
/// Shared between full-load and streaming paths.
fn build_waveform_states(track: &LoadedTrack, quality_level: u8, screen_width: u32) -> (OverviewState, ZoomedState) {
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

    // Build overview from metadata (beat markers, cue markers, drop marker)
    // then compute peaks from raw stems.
    let mut overview_state = OverviewState::from_metadata(&track.metadata, duration);
    overview_state.set_drop_marker(track.metadata.drop_marker);
    overview_state.loading = false;

    // Compute overview peaks from raw stems (consistent with incremental path)
    let overview_start = std::time::Instant::now();
    overview_state.stem_waveforms = generate_peaks(&track.stems, DEFAULT_WIDTH);
    overview_state.overview_peak_buffer =
        mesh_widgets::PeakBuffer::from_stem_peaks(&overview_state.stem_waveforms);
    log::info!("[PERF] Loader: Overview peaks took {:?}", overview_start.elapsed());

    // Compute high-resolution peaks
    let highres_start = std::time::Instant::now();
    let highres_width = compute_highres_width(track.stems.len(), bpm, screen_width, quality_level);
    let highres_peaks = generate_peaks(&track.stems, highres_width);
    overview_state.set_highres_peaks(highres_peaks);
    {
        let samples_per_beat = (48000.0_f64 * 60.0 / bpm) as usize;
        let samples_per_bar = samples_per_beat * 4;
        let ref_zoom = 4u32;
        let window_at_ref = samples_per_bar * ref_zoom as usize;
        let total = track.stems.len();
        let ppp_at_ref = if window_at_ref > 0 && total > 0 {
            highres_width as f64 * window_at_ref as f64 / (total as f64 * screen_width as f64)
        } else { 0.0 };
        log::info!(
            "[RENDER] Final highres: {} peaks in {:?} | quality={} bpm={:.1} screen={}px | \
             ref_zoom={}bars → {:.2} pp/px",
            highres_width, highres_start.elapsed(), quality_level, bpm, screen_width,
            ref_zoom, ppp_at_ref,
        );
    }

    // Pre-compute zoomed waveform state
    let mut zoomed_state = ZoomedState::from_metadata(
        bpm,
        track.metadata.beat_grid.beats.clone(),
        cue_markers,
    );
    zoomed_state.set_duration(duration);
    zoomed_state.set_drop_marker(track.metadata.drop_marker);

    (overview_state, zoomed_state)
}
