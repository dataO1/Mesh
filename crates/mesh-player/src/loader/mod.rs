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
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{AudioFileReader, LoadedTrack, StemBuffers, TrackMetadata};
use mesh_core::buffer_pool::StemBufferPool;
use mesh_core::engine::PreparedTrack;
use mesh_widgets::{
    CueMarker, OverviewState, SharedPeakBuffer, ZoomedState, CUE_COLORS,
    compute_highres_width, generate_peaks, update_peaks_for_region_flat, DEFAULT_WIDTH,
};

use self::regions::{compute_gaps, compute_priority_regions};

/// Number of parallel decode workers. On ARM (RK3588S etc.) we use fewer workers
/// to avoid saturating the shared LPDDR4X memory bus, which starves the GPU and
/// causes waveform stutter across all decks during loading.
const DECODE_WORKERS: usize = if cfg!(target_arch = "aarch64") { 2 } else { 4 };

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
        /// Shared overview peaks (Arc clone = refcount bump, zero data copy)
        shared_overview: Arc<SharedPeakBuffer>,
        /// Shared highres peaks (Arc clone = refcount bump, zero data copy)
        shared_highres: Arc<SharedPeakBuffer>,
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
    /// Pre-allocated buffer pool (eliminates page fault storms on embedded).
    /// Kept alive here so the pool outlives the loader thread.
    #[allow(dead_code)]
    buffer_pool: Option<Arc<StemBufferPool>>,
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
    pub fn spawn(target_sample_rate: u32, buffer_pool: Option<Arc<StemBufferPool>>) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<TrackLoadRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<TrackLoadResult>();

        // Store sample rate in Arc<AtomicU32> so loader thread can access it
        let rate = Arc::new(AtomicU32::new(target_sample_rate));
        let rate_for_thread = rate.clone();
        let pool_for_thread = buffer_pool.clone();

        let handle = thread::Builder::new()
            .name("track-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx, rate_for_thread, pool_for_thread);
            })
            .expect("Failed to spawn track loader thread");

        log::info!("TrackLoader spawned with target sample rate: {} Hz", target_sample_rate);

        Self {
            tx: request_tx,
            rx: Arc::new(Mutex::new(result_rx)),
            target_sample_rate: rate,
            buffer_pool,
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
    buffer_pool: Option<Arc<StemBufferPool>>,
) {
    // Pin dispatch thread to big cores — track loading is heavy background work
    mesh_core::rt::pin_to_big_cores();

    log::info!("Track loader dispatch thread started");

    while let Ok(request) = rx.recv() {
        let tx = tx.clone();
        let rate = target_sample_rate.clone();
        let pool = buffer_pool.clone();
        let deck_idx = request.deck_idx;
        if let Err(e) = thread::Builder::new()
            .name(format!("track-load-{}", deck_idx))
            .spawn(move || {
                mesh_core::rt::pin_to_big_cores();
                handle_track_load(request, tx, rate, pool);
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
    buffer_pool: Option<Arc<StemBufferPool>>,
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
        handle_streaming_load(request, &tx, reader, sample_rate, buffer_pool);
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
    buffer_pool: Option<Arc<StemBufferPool>>,
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

    // 1. Allocate full buffer — try pool first, fall back to fresh allocation
    let alloc_start = std::time::Instant::now();
    let mut stems = buffer_pool
        .as_ref()
        .and_then(|pool| pool.checkout(frame_count))
        .unwrap_or_else(|| StemBuffers::with_length(frame_count));
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
    // Pre-allocate shared peak buffers — written to by merge thread, read by UI
    let shared_overview = Arc::new(SharedPeakBuffer::new_empty(DEFAULT_WIDTH as u32, 4));
    let shared_highres = Arc::new(SharedPeakBuffer::new_empty(highres_width as u32, 4));

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
        let decode_start_time = std::time::Instant::now();
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
                    mesh_core::rt::pin_to_big_cores();
                    loop {
                        let i = idx.fetch_add(1, Ordering::SeqCst);
                        if i >= total_regions {
                            break;
                        }
                        let region = regions_ref[i];
                        let len = region.len();
                        let t = std::time::Instant::now();
                        let result = reader_ref.decode_region(region.start, len)
                            .map_err(|e| format!("Region {} decode failed: {}", i, e));
                        let kind = if i < num_priority { "Priority" } else { "Gap" };
                        log::info!(
                            "[PERF] Loader: {} region {} ({} frames from {}) decoded in {:?}",
                            kind, i, len, region.start, t.elapsed()
                        );
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
                        let _ = tx.send(TrackLoadResult::Error { deck_idx, error: e });
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

                // Include a playable stems snapshot when all priority regions
                // are merged (first interaction point — user can play immediately).
                // The final full-quality stems are delivered via Complete (move, no clone).
                let all_priority_done = priority_merged == num_priority;
                let _all_done = total_merged == total_regions;
                let send_stems = is_priority && all_priority_done;

                // Send lightweight message — Arc clone is just a refcount bump (~16 bytes),
                // NOT a 5.3 MB peak data clone like before
                let _ = tx.send(TrackLoadResult::RegionLoaded {
                    deck_idx,
                    stems: if send_stems {
                        // Create a snapshot for the engine. Try pool first (pre-touched
                        // pages = no page faults), fall back to clone (fresh allocation).
                        let snap_start = std::time::Instant::now();
                        let snapshot = if let Some(mut pool_buf) = buffer_pool
                            .as_ref()
                            .and_then(|pool| pool.checkout(frame_count))
                        {
                            stems.snapshot_into(&mut pool_buf);
                            log::info!("[PERF] Loader: Priority snapshot via pool memcpy in {:?} ({:.1} MB)",
                                snap_start.elapsed(), (frame_count as f64 * 32.0) / 1_000_000.0);
                            pool_buf
                        } else {
                            let cloned = stems.clone();
                            log::info!("[PERF] Loader: Priority snapshot via clone in {:?} ({:.1} MB, page faults likely)",
                                snap_start.elapsed(), (frame_count as f64 * 32.0) / 1_000_000.0);
                            cloned
                        };
                        Some(Shared::new(&mesh_core::engine::gc::gc_handle(), snapshot))
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
                        num_priority, decode_start_time.elapsed()
                    );
                }
            }

            log::info!(
                "[PERF] Loader: All {} regions decoded+merged in {:?} ({} workers)",
                total_regions, decode_start_time.elapsed(), num_workers
            );
        });
    }

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

    // Compute overview peaks from raw stems
    let overview_start = std::time::Instant::now();
    let overview_tuples = generate_peaks(&track.stems, DEFAULT_WIDTH);
    overview_state.shared_overview = Some(Arc::new(SharedPeakBuffer::from_stem_peaks(&overview_tuples)));
    log::info!("[PERF] Loader: Overview peaks took {:?}", overview_start.elapsed());

    // Compute high-resolution peaks
    let highres_start = std::time::Instant::now();
    let highres_width = compute_highres_width(track.stems.len(), bpm, screen_width, quality_level);
    let highres_tuples = generate_peaks(&track.stems, highres_width);
    overview_state.shared_highres = Some(Arc::new(SharedPeakBuffer::from_stem_peaks(&highres_tuples)));
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
