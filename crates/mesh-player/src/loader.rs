//! Background track loader for Mesh DJ Player
//!
//! Moves expensive track loading operations (file I/O, waveform computation)
//! off the UI thread to prevent audio stuttering during track loads.
//!
//! The loader thread automatically resamples tracks to match JACK's sample rate,
//! ensuring correct playback speed regardless of the JACK server configuration.
//!
//! # Linked Stem Loading
//!
//! The loader also supports loading linked stems from other tracks:
//! - Extracts a single stem from an 8-channel file
//! - Pre-stretches to match the host deck's BPM
//! - Uses drop markers for structural alignment

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};

use basedrop::Shared;
use mesh_core::audio_file::{LoadedTrack, StemBuffers};
use mesh_core::engine::{LinkedStemData, PreparedTrack, StemLink};
use mesh_core::types::{Stem, StereoBuffer, SAMPLE_RATE};
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

// ────────────────────────────────────────────────────────────────────────────────
// Linked Stem Loading
// ────────────────────────────────────────────────────────────────────────────────

/// Request to load a linked stem from another track
#[derive(Debug)]
pub struct LinkedStemLoadRequest {
    /// Host deck that will receive the linked stem
    pub host_deck_idx: usize,
    /// Which stem slot to link (0=Vocals, 1=Drums, 2=Bass, 3=Other)
    pub stem_idx: usize,
    /// Path to the track containing the stem to link
    pub source_path: PathBuf,
    /// Host deck's current BPM (for pre-stretching)
    pub host_bpm: f64,
    /// Host deck's drop marker position (for alignment)
    pub host_drop_marker: u64,
}

/// Result of a linked stem load
pub struct LinkedStemLoadResult {
    /// Host deck index
    pub host_deck_idx: usize,
    /// Stem index that was linked
    pub stem_idx: usize,
    /// Pre-stretched linked stem data (or error)
    pub result: Result<LinkedStemData, String>,
    /// Pre-computed overview peaks from source track's wvfm chunk
    /// None if source track has no waveform preview, or if load failed
    pub overview_peaks: Option<Vec<(f32, f32)>>,
}

/// Unified loader request (track or linked stem)
enum LoaderRequest {
    Track(TrackLoadRequest),
    LinkedStem(LinkedStemLoadRequest),
}

/// Unified loader result (track or linked stem)
pub enum LoaderResult {
    Track(TrackLoadResult),
    LinkedStem(LinkedStemLoadResult),
}

/// Handle to the background loader thread
pub struct TrackLoader {
    /// Channel to send load requests (unified for tracks and linked stems)
    tx: Sender<LoaderRequest>,
    /// Channel to receive load results (unified for tracks and linked stems)
    rx: Receiver<LoaderResult>,
    /// Target sample rate for loading (JACK's sample rate)
    target_sample_rate: Arc<AtomicU32>,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl TrackLoader {
    /// Spawn the background loader thread
    ///
    /// # Arguments
    /// * `target_sample_rate` - JACK's sample rate for resampling tracks on load
    pub fn spawn(target_sample_rate: u32) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<LoaderRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<LoaderResult>();

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
            rx: result_rx,
            target_sample_rate: rate,
            _handle: handle,
        }
    }

    /// Update the target sample rate (if JACK rate changes)
    pub fn set_sample_rate(&self, sample_rate: u32) {
        self.target_sample_rate.store(sample_rate, Ordering::SeqCst);
        log::info!("TrackLoader target sample rate updated to: {} Hz", sample_rate);
    }

    /// Request loading a track (non-blocking)
    pub fn load(&self, deck_idx: usize, path: PathBuf) -> Result<(), String> {
        self.tx
            .send(LoaderRequest::Track(TrackLoadRequest { deck_idx, path }))
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Request loading a linked stem from another track (non-blocking)
    ///
    /// The linked stem will be pre-stretched to match the host deck's BPM
    /// and use drop markers for structural alignment.
    pub fn load_linked_stem(
        &self,
        host_deck_idx: usize,
        stem_idx: usize,
        source_path: PathBuf,
        host_bpm: f64,
        host_drop_marker: u64,
    ) -> Result<(), String> {
        self.tx
            .send(LoaderRequest::LinkedStem(LinkedStemLoadRequest {
                host_deck_idx,
                stem_idx,
                source_path,
                host_bpm,
                host_drop_marker,
            }))
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Try to receive a completed load result (non-blocking)
    ///
    /// Returns either a track load result or a linked stem load result.
    pub fn try_recv(&self) -> Option<LoaderResult> {
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
fn loader_thread(
    rx: Receiver<LoaderRequest>,
    tx: Sender<LoaderResult>,
    target_sample_rate: Arc<AtomicU32>,
) {
    log::info!("Track loader thread started");

    while let Ok(request) = rx.recv() {
        match request {
            LoaderRequest::Track(req) => {
                handle_track_load(req, &tx, &target_sample_rate);
            }
            LoaderRequest::LinkedStem(req) => {
                handle_linked_stem_load(req, &tx, &target_sample_rate);
            }
        }
    }

    log::info!("Track loader thread shutting down");
}

/// Handle a track load request
fn handle_track_load(
    request: TrackLoadRequest,
    tx: &Sender<LoaderResult>,
    target_sample_rate: &Arc<AtomicU32>,
) {
    // Read the current target sample rate (may have been updated)
    let sample_rate = target_sample_rate.load(Ordering::SeqCst);

    log::info!(
        "[PERF] Loader: Starting track load for deck {}: {:?} (target: {} Hz)",
        request.deck_idx,
        request.path,
        sample_rate
    );

    let total_start = std::time::Instant::now();

    // Load the track with resampling to JACK's sample rate
    let load_start = std::time::Instant::now();
    let result = LoadedTrack::load_to(&request.path, sample_rate);
    log::info!("[PERF] Loader: LoadedTrack::load_to({} Hz) took {:?}", sample_rate, load_start.elapsed());

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
            let mut overview_state = if let Some(ref preview) = track.metadata.waveform_preview {
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

            // Set drop marker if present in track metadata
            overview_state.set_drop_marker(track.metadata.drop_marker);

            // Pre-compute zoomed waveform state
            let mut zoomed_state = ZoomedState::from_metadata(
                bpm,
                track.metadata.beat_grid.beats.clone(),
                cue_markers,
            );
            zoomed_state.set_duration(duration);
            zoomed_state.set_drop_marker(track.metadata.drop_marker);

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
            let _ = tx.send(LoaderResult::Track(TrackLoadResult {
                deck_idx: request.deck_idx,
                result: Ok(prepared),
                overview_state,
                zoomed_state,
                stems,
            }));
        }
        Err(e) => {
            log::error!("Failed to load track: {}", e);

            // Send error result
            let _ = tx.send(LoaderResult::Track(TrackLoadResult {
                deck_idx: request.deck_idx,
                result: Err(e.to_string()),
                overview_state: OverviewState::new(),
                zoomed_state: ZoomedState::new(),
                stems: Shared::new(&mesh_core::engine::gc::gc_handle(), StemBuffers::with_length(0)),
            }));
        }
    }
}

/// Handle a linked stem load request
///
/// This function:
/// 1. Loads the source track to get its metadata (BPM, drop marker)
/// 2. Extracts the requested stem from the 8-channel file
/// 3. Pre-stretches the stem to match the host deck's BPM
/// 4. Sends the result back to the UI thread
fn handle_linked_stem_load(
    request: LinkedStemLoadRequest,
    tx: &Sender<LoaderResult>,
    target_sample_rate: &Arc<AtomicU32>,
) {
    let sample_rate = target_sample_rate.load(Ordering::SeqCst);

    log::info!(
        "[PERF] Loader: Loading linked stem {} for deck {} from {:?}",
        request.stem_idx,
        request.host_deck_idx,
        request.source_path
    );

    let total_start = std::time::Instant::now();

    // Load the source track (we need full track to get metadata and stems)
    let load_result = LoadedTrack::load_to(&request.source_path, sample_rate);

    match load_result {
        Ok(source_track) => {
            // Get source track metadata
            let source_bpm = source_track.metadata.bpm.unwrap_or(120.0);
            let source_drop_marker = source_track.metadata.drop_marker.unwrap_or(0);
            let track_name = request
                .source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string();

            // Extract the requested stem
            let stem = Stem::ALL.get(request.stem_idx).copied().unwrap_or(Stem::Vocals);
            let stem_buffer = source_track.stems.get(stem);

            // Convert stem slice to StereoBuffer for pre-stretching
            let mut source_buffer = StereoBuffer::silence(stem_buffer.len());
            source_buffer.as_mut_slice().copy_from_slice(stem_buffer.as_slice());

            // Pre-stretch to host BPM
            let stretch_start = std::time::Instant::now();
            let mut stem_link = StemLink::new_with_sample_rate(sample_rate);
            let stretched_buffer = stem_link.pre_stretch(&source_buffer, source_bpm, request.host_bpm);

            // Scale drop marker position by stretch ratio
            let stretch_ratio = request.host_bpm / source_bpm;
            let stretched_drop_marker = if stretch_ratio > 0.0 {
                ((source_drop_marker as f64) / stretch_ratio) as u64
            } else {
                source_drop_marker
            };

            log::info!(
                "[PERF] Loader: Pre-stretched stem {} from {:.1} BPM to {:.1} BPM ({} -> {} samples) in {:?}",
                request.stem_idx,
                source_bpm,
                request.host_bpm,
                source_buffer.len(),
                stretched_buffer.len(),
                stretch_start.elapsed()
            );

            log::info!(
                "[PERF] Loader: Total linked stem load time: {:?}",
                total_start.elapsed()
            );

            // Extract overview peaks from source track's wvfm chunk (if available)
            // These are pre-computed peaks, no generation needed!
            let overview_peaks = source_track
                .metadata
                .waveform_preview
                .as_ref()
                .map(|preview| preview.extract_stem_peaks(request.stem_idx));

            if overview_peaks.is_some() {
                log::debug!(
                    "[LINKED] Extracted {} overview peaks for linked stem {} from source wvfm",
                    overview_peaks.as_ref().map(|p| p.len()).unwrap_or(0),
                    request.stem_idx
                );
            } else {
                log::debug!(
                    "[LINKED] No waveform preview in source track for stem {}",
                    request.stem_idx
                );
            }

            // Send result
            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Ok(LinkedStemData {
                    buffer: stretched_buffer,
                    original_bpm: source_bpm,
                    drop_marker: stretched_drop_marker,
                    track_name,
                    track_path: Some(request.source_path),
                }),
                overview_peaks,
            }));
        }
        Err(e) => {
            log::error!("Failed to load source track for linked stem: {}", e);

            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Err(e.to_string()),
                overview_peaks: None,
            }));
        }
    }
}
