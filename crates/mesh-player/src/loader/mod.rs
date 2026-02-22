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
use mesh_core::audio_file::{AudioFileReader, LoadedTrack, StemBuffers, TrackMetadata, read_waveform_preview_from_file};
use mesh_core::engine::PreparedTrack;
use mesh_widgets::{
    CueMarker, OverviewState, ZoomedState, CUE_COLORS,
    allocate_empty_peaks, generate_peaks, update_peaks_for_region,
    DEFAULT_WIDTH, HIGHRES_WIDTH,
};

use self::regions::{compute_gaps, compute_priority_regions};

/// Number of frames per visual batch (peak-only updates).
/// ~15 seconds of audio at 48 kHz → smooth waveform growth.
/// Peak snapshots are ~2 MB each — negligible cost.
const VISUAL_BATCH_FRAMES: usize = 750_000;

/// Number of frames between stem buffer clones (playback updates).
/// ~100 seconds of audio at 48 kHz → ~3 clones for a 5-minute track.
/// Each clone copies the full ~460 MB buffer (~100 ms).
const CLONE_INTERVAL_FRAMES: usize = 4_800_000;

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
    pub fn load(&self, deck_idx: usize, path: PathBuf, metadata: TrackMetadata) -> Result<(), String> {
        self.tx
            .send(TrackLoadRequest { deck_idx, path, metadata })
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

/// The background loader thread function
fn loader_thread(
    rx: Receiver<TrackLoadRequest>,
    tx: Sender<TrackLoadResult>,
    target_sample_rate: Arc<AtomicU32>,
) {
    log::info!("Track loader thread started");

    while let Ok(request) = rx.recv() {
        handle_track_load(request, &tx, &target_sample_rate);
    }

    log::info!("Track loader thread shutting down");
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
    tx: &Sender<TrackLoadResult>,
    target_sample_rate: &Arc<AtomicU32>,
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
        handle_full_load(request, tx, sample_rate);
    } else {
        // STREAMING PATH: native rate, can read regions directly
        handle_streaming_load(request, tx, reader, sample_rate);
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
            let (overview_state, zoomed_state) = build_waveform_states(&track);
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
    mut reader: AudioFileReader,
    sample_rate: u32,
) {
    let deck_idx = request.deck_idx;
    let path = request.path.clone();
    let mut metadata = request.metadata;

    // Read waveform preview from file (separate chunk, not in data)
    match read_waveform_preview_from_file(&path) {
        Ok(Some(waveform)) => metadata.waveform_preview = Some(waveform),
        Ok(None) => {}
        Err(e) => log::warn!("Failed to read waveform preview: {}", e),
    }

    let frame_count = reader.frame_count() as usize;

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
    let mut overview_peaks = allocate_empty_peaks(DEFAULT_WIDTH);
    let mut highres_peaks = allocate_empty_peaks(HIGHRES_WIDTH);

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

    // 4. Read ALL priority regions: peak-only visual updates per region,
    //    single expensive stem clone after all regions are read.
    let priority_start = std::time::Instant::now();
    for (i, region) in regions.iter().enumerate() {
        if let Err(e) = reader.read_region_into(&mut stems, region.start, region.start, region.len()) {
            log::error!("Failed to read priority region: {}", e);
            let _ = tx.send(TrackLoadResult::Error {
                deck_idx,
                error: format!("Failed to read priority region: {}", e),
            });
            return;
        }

        // Update peaks (cheap: ~5 ms per region)
        update_peaks_for_region(&stems, &mut overview_peaks, region.start, region.end, frame_count, DEFAULT_WIDTH);
        update_peaks_for_region(&stems, &mut highres_peaks, region.start, region.end, frame_count, HIGHRES_WIDTH);

        let is_last = i + 1 == regions.len();
        let _ = tx.send(TrackLoadResult::RegionLoaded {
            deck_idx,
            // Clone stems only on the last priority region (all regions playable)
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
    log::info!("[PERF] Loader: Priority regions read took {:?}", priority_start.elapsed());

    // 5. Read remaining gaps: visual-only updates every VISUAL_BATCH_FRAMES,
    //    stem clones every CLONE_INTERVAL_FRAMES.
    let gap_start = std::time::Instant::now();
    let mut frames_since_clone: usize = 0;
    for gap in &gaps {
        let mut pos = gap.start;
        while pos < gap.end {
            let batch_end = (pos + VISUAL_BATCH_FRAMES).min(gap.end);
            let batch_len = batch_end - pos;

            if let Err(e) = reader.read_region_into(&mut stems, pos, pos, batch_len) {
                log::error!("Failed to read gap batch: {}", e);
                let _ = tx.send(TrackLoadResult::Error {
                    deck_idx,
                    error: format!("Failed to read gap batch: {}", e),
                });
                return;
            }

            // Update peaks for this batch (cheap: ~5 ms)
            update_peaks_for_region(&stems, &mut overview_peaks, pos, batch_end, frame_count, DEFAULT_WIDTH);
            update_peaks_for_region(&stems, &mut highres_peaks, pos, batch_end, frame_count, HIGHRES_WIDTH);

            frames_since_clone += batch_len;
            let clone_now = frames_since_clone >= CLONE_INTERVAL_FRAMES;

            let _ = tx.send(TrackLoadResult::RegionLoaded {
                deck_idx,
                stems: if clone_now {
                    frames_since_clone = 0;
                    Some(Shared::new(&mesh_core::engine::gc::gc_handle(), stems.clone()))
                } else {
                    None
                },
                duration_samples: frame_count,
                overview_peaks: overview_peaks.clone(),
                highres_peaks: highres_peaks.clone(),
                path: path.clone(),
            });

            pos = batch_end;
        }
    }
    // Clone any remaining uncloned frames so all gap audio is playable before Complete
    if frames_since_clone > 0 {
        let _ = tx.send(TrackLoadResult::RegionLoaded {
            deck_idx,
            stems: Some(Shared::new(&mesh_core::engine::gc::gc_handle(), stems.clone())),
            duration_samples: frame_count,
            overview_peaks: overview_peaks.clone(),
            highres_peaks: highres_peaks.clone(),
            path: path.clone(),
        });
    }
    log::info!("[PERF] Loader: Gap regions read took {:?}", gap_start.elapsed());

    // 6. Build final waveform states from complete stems
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

    let (overview_state, zoomed_state) = build_waveform_states(&track);
    let prepared = PreparedTrack::prepare(track);

    // 7. Send complete result (single UpgradeStems to engine)
    let _ = tx.send(TrackLoadResult::Complete {
        deck_idx,
        result: Ok(prepared),
        overview_state,
        zoomed_state,
        stems: final_stems,
        duration_samples,
        path,
    });
}

/// Build overview and zoomed waveform states from a loaded track.
/// Shared between full-load and streaming paths.
fn build_waveform_states(track: &LoadedTrack) -> (OverviewState, ZoomedState) {
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
    // then compute peaks from raw stems. We avoid from_preview() here because
    // stored previews may have LUFS gain baked in, causing a visual "pop" when
    // replacing the incrementally-built raw peaks.
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
    let highres_peaks = generate_peaks(&track.stems, HIGHRES_WIDTH);
    overview_state.set_highres_peaks(highres_peaks);
    log::info!("[PERF] Loader: Highres peaks took {:?}", highres_start.elapsed());

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
