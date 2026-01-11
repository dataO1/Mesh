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

use rayon;

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
    /// Host track's total duration in samples (for pre-aligned buffer)
    pub host_duration: u64,
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
    /// Duration of the linked stem buffer in samples (after stretching)
    /// None if load failed
    pub linked_duration: Option<u64>,
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
    /// and pre-aligned so drops line up. The resulting buffer has length = host_duration
    /// and can be read directly at the host position (no runtime offset calculation).
    pub fn load_linked_stem(
        &self,
        host_deck_idx: usize,
        stem_idx: usize,
        source_path: PathBuf,
        host_bpm: f64,
        host_drop_marker: u64,
        host_duration: u64,
    ) -> Result<(), String> {
        self.tx
            .send(LoaderRequest::LinkedStem(LinkedStemLoadRequest {
                host_deck_idx,
                stem_idx,
                source_path,
                host_bpm,
                host_drop_marker,
                host_duration,
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
                // Track loads stay on the loader thread (need to complete before playback)
                handle_track_load(req, &tx, &target_sample_rate);
            }
            LoaderRequest::LinkedStem(req) => {
                // Spawn linked stem loads to rayon thread pool for parallel processing
                let tx_clone = tx.clone();
                let sample_rate = target_sample_rate.load(Ordering::SeqCst);

                rayon::spawn(move || {
                    handle_linked_stem_load_parallel(req, tx_clone, sample_rate);
                });
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
            log::info!(
                "[LOADER] Host track drop_marker={:?}, duration={}",
                track.metadata.drop_marker,
                duration
            );

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

/// Align overview peaks to the host timeline using drop marker alignment
///
/// Takes the 800-peak waveform from the stretched stem and maps it to the host timeline.
/// The output peaks show where the linked stem's waveform appears relative to the host track.
fn align_peaks_to_host(
    peaks: &[(f32, f32)],
    stretched_duration: usize,
    host_duration: usize,
    host_drop_marker: u64,
    stretched_drop_marker: u64,
) -> Vec<(f32, f32)> {
    const PEAK_WIDTH: usize = 800;

    let offset = host_drop_marker as i64 - stretched_drop_marker as i64;

    let mut aligned_peaks = vec![(0.0f32, 0.0f32); PEAK_WIDTH];

    for i in 0..PEAK_WIDTH {
        // Host position in samples for this peak index
        let host_sample = (i as f64 / PEAK_WIDTH as f64) * host_duration as f64;

        // Map to linked buffer position (inverse of audio alignment)
        let linked_sample = host_sample - offset as f64;

        if linked_sample >= 0.0 && (linked_sample as usize) < stretched_duration {
            // Find corresponding peak index in source peaks
            let linked_peak_idx =
                ((linked_sample / stretched_duration as f64) * peaks.len() as f64) as usize;
            if linked_peak_idx < peaks.len() {
                aligned_peaks[i] = peaks[linked_peak_idx];
            }
        }
        // else: stays at (0.0, 0.0) - silence region
    }

    aligned_peaks
}

/// Align a stretched buffer to the host timeline using drop marker alignment
///
/// Creates a new buffer with length = host_duration where the linked stem is
/// positioned so its drop marker aligns with the host's drop marker.
/// - Pads with silence at start if linked starts after host drop
/// - Trims start if linked starts before host drop
/// - Pads with silence at end if linked ends before host
///
/// After alignment, the buffer can be read directly at host position (no offset calculation).
fn align_buffer_to_host(
    stretched: &StereoBuffer,
    host_duration: usize,
    host_drop_marker: u64,
    stretched_drop_marker: u64,
) -> StereoBuffer {
    let mut aligned = StereoBuffer::silence(host_duration);

    // Offset = how much to shift linked stem in the aligned buffer
    // Positive: linked stem starts later (pad start with silence)
    // Negative: linked stem starts earlier (trim start of linked)
    let offset = host_drop_marker as i64 - stretched_drop_marker as i64;

    let aligned_slice = aligned.as_mut_slice();
    let stretched_slice = stretched.as_slice();
    let stretched_len = stretched.len();

    for i in 0..host_duration {
        // Source position in stretched buffer
        let src_pos = i as i64 - offset;

        if src_pos >= 0 && (src_pos as usize) < stretched_len {
            aligned_slice[i] = stretched_slice[src_pos as usize];
        }
        // else: already silence (from StereoBuffer::silence)
    }

    aligned
}

/// Handle a linked stem load request
///
/// This function:
/// 1. Loads the source track to get its metadata (BPM, drop marker)
/// 2. Extracts the requested stem from the 8-channel file
/// 3. Pre-stretches the stem to match the host deck's BPM
/// 4. Pre-aligns the buffer to host timeline (drops aligned, same length as host)
/// 5. Sends the result back to the UI thread
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

            // Scale drop marker position by stretch ratio and compensate for stretcher latency
            let stretch_ratio = request.host_bpm / source_bpm;
            let stretcher_latency = stem_link.stretcher_latency();

            let stretched_drop_marker = if stretch_ratio > 0.0 && (stretch_ratio - 1.0).abs() >= 0.001 {
                // Stretching occurred - compensate for stretcher latency
                // The stretcher shifts audio content FORWARD by latency samples,
                // so we ADD latency to the drop marker to match where the audio actually is
                let scaled_drop = ((source_drop_marker as f64) / stretch_ratio).round() as u64;
                scaled_drop + stretcher_latency as u64
            } else {
                // No stretching (same BPM) - no latency to compensate
                source_drop_marker
            };

            log::info!(
                "[STRETCH] ratio={:.4}, source_drop={}, stretched_drop={}, latency_comp={}",
                stretch_ratio, source_drop_marker, stretched_drop_marker, stretcher_latency
            );

            log::info!(
                "[PERF] Loader: Pre-stretched stem {} from {:.1} BPM to {:.1} BPM ({} -> {} samples) in {:?}",
                request.stem_idx,
                source_bpm,
                request.host_bpm,
                source_buffer.len(),
                stretched_buffer.len(),
                stretch_start.elapsed()
            );

            // Capture stretched length before alignment (needed for peak alignment)
            let stretched_len = stretched_buffer.len();

            // Pre-align the buffer to host timeline
            // After alignment: buffer length = host_duration, drops are aligned,
            // and the buffer can be read directly at host position (no offset calc)
            let align_start = std::time::Instant::now();
            let aligned_buffer = align_buffer_to_host(
                &stretched_buffer,
                request.host_duration as usize,
                request.host_drop_marker,
                stretched_drop_marker,
            );

            log::info!(
                "[ALIGN] host_drop={}, stretched_drop={}, offset={}, stretched_len={} -> aligned_len={}",
                request.host_drop_marker,
                stretched_drop_marker,
                request.host_drop_marker as i64 - stretched_drop_marker as i64,
                stretched_len,
                aligned_buffer.len()
            );

            log::info!(
                "[PERF] Loader: Pre-aligned buffer in {:?}",
                align_start.elapsed()
            );

            log::info!(
                "[PERF] Loader: Total linked stem load time: {:?}",
                total_start.elapsed()
            );

            // After alignment, duration = host_duration
            let linked_duration = aligned_buffer.len() as u64;

            // Extract overview peaks from source track's wvfm chunk (if available)
            // Then align them to host timeline (same alignment as audio buffer)
            let overview_peaks = source_track
                .metadata
                .waveform_preview
                .as_ref()
                .map(|preview| {
                    let raw_peaks = preview.extract_stem_peaks(request.stem_idx);
                    // Align peaks to host timeline so waveform matches audio alignment
                    align_peaks_to_host(
                        &raw_peaks,
                        stretched_len,
                        request.host_duration as usize,
                        request.host_drop_marker,
                        stretched_drop_marker,
                    )
                });

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

            // Send result with aligned buffer
            // drop_marker is now 0 (alignment is baked into buffer position)
            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Ok(LinkedStemData {
                    buffer: aligned_buffer,
                    original_bpm: source_bpm,
                    drop_marker: 0, // No longer needed - alignment is baked in
                    track_name,
                    track_path: Some(request.source_path),
                }),
                overview_peaks,
                linked_duration: Some(linked_duration),
            }));
        }
        Err(e) => {
            log::error!("Failed to load source track for linked stem: {}", e);

            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Err(e.to_string()),
                overview_peaks: None,
                linked_duration: None,
            }));
        }
    }
}

/// Parallel version of handle_linked_stem_load for rayon::spawn
///
/// Takes owned values instead of references since rayon::spawn requires 'static.
fn handle_linked_stem_load_parallel(
    request: LinkedStemLoadRequest,
    tx: Sender<LoaderResult>,
    sample_rate: u32,
) {
    log::info!(
        "[PERF] Loader: Loading linked stem {} for deck {} from {:?} (parallel)",
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

            // Scale drop marker position by stretch ratio and compensate for stretcher latency
            let stretch_ratio = request.host_bpm / source_bpm;
            let stretcher_latency = stem_link.stretcher_latency();

            let stretched_drop_marker = if stretch_ratio > 0.0 && (stretch_ratio - 1.0).abs() >= 0.001 {
                // Stretching occurred - compensate for stretcher latency
                let scaled_drop = ((source_drop_marker as f64) / stretch_ratio).round() as u64;
                scaled_drop + stretcher_latency as u64
            } else {
                source_drop_marker
            };

            log::info!(
                "[STRETCH] ratio={:.4}, source_drop={}, stretched_drop={}, latency_comp={}",
                stretch_ratio, source_drop_marker, stretched_drop_marker, stretcher_latency
            );

            log::info!(
                "[PERF] Loader: Pre-stretched stem {} from {:.1} BPM to {:.1} BPM ({} -> {} samples) in {:?}",
                request.stem_idx,
                source_bpm,
                request.host_bpm,
                source_buffer.len(),
                stretched_buffer.len(),
                stretch_start.elapsed()
            );

            // Capture stretched length before alignment (needed for peak alignment)
            let stretched_len = stretched_buffer.len();

            // Pre-align the buffer to host timeline
            let align_start = std::time::Instant::now();
            let aligned_buffer = align_buffer_to_host(
                &stretched_buffer,
                request.host_duration as usize,
                request.host_drop_marker,
                stretched_drop_marker,
            );

            log::info!(
                "[ALIGN] host_drop={}, stretched_drop={}, offset={}, stretched_len={} -> aligned_len={}",
                request.host_drop_marker,
                stretched_drop_marker,
                request.host_drop_marker as i64 - stretched_drop_marker as i64,
                stretched_len,
                aligned_buffer.len()
            );

            log::info!(
                "[PERF] Loader: Pre-aligned buffer in {:?}",
                align_start.elapsed()
            );

            log::info!(
                "[PERF] Loader: Total linked stem load time: {:?}",
                total_start.elapsed()
            );

            let linked_duration = aligned_buffer.len() as u64;

            // Extract overview peaks from source track's wvfm chunk (if available)
            let overview_peaks = source_track
                .metadata
                .waveform_preview
                .as_ref()
                .map(|preview| {
                    let raw_peaks = preview.extract_stem_peaks(request.stem_idx);
                    align_peaks_to_host(
                        &raw_peaks,
                        stretched_len,
                        request.host_duration as usize,
                        request.host_drop_marker,
                        stretched_drop_marker,
                    )
                });

            if overview_peaks.is_some() {
                log::debug!(
                    "[LINKED] Extracted {} overview peaks for linked stem {} from source wvfm",
                    overview_peaks.as_ref().map(|p| p.len()).unwrap_or(0),
                    request.stem_idx
                );
            }

            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Ok(LinkedStemData {
                    buffer: aligned_buffer,
                    original_bpm: source_bpm,
                    drop_marker: 0,
                    track_name,
                    track_path: Some(request.source_path),
                }),
                overview_peaks,
                linked_duration: Some(linked_duration),
            }));
        }
        Err(e) => {
            log::error!("Failed to load source track for linked stem: {}", e);

            let _ = tx.send(LoaderResult::LinkedStem(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Err(e.to_string()),
                overview_peaks: None,
                linked_duration: None,
            }));
        }
    }
}
