//! Linked stem background loader
//!
//! Provides async loading of linked stems from other tracks with:
//! - Time-stretching to match host BPM
//! - Drop marker alignment
//! - Waveform peak generation for UI display
//!
//! # Message-Driven Architecture
//!
//! The loader is designed for message-driven UIs like iced:
//! - Call `load_from_metadata()` or `load_linked_stem()` to queue loads
//! - Use `result_receiver()` to get a clonable receiver for subscriptions
//! - Results arrive as messages, no polling needed

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use basedrop::Shared;

use crate::audio_file::{LoadedTrack, StemLinkReference};
use crate::db::DatabaseService;
use crate::engine::{gc::gc_handle, LinkedStemData, StemLink};
use crate::types::{Stem, StereoBuffer};
use crate::usb::cache as usb_cache;

/// Width for high-resolution peaks (must match mesh-widgets HIGHRES_WIDTH = 65536)
const HIGHRES_WIDTH: usize = 65536;

/// Width for overview peaks
const OVERVIEW_PEAK_WIDTH: usize = 800;

// ────────────────────────────────────────────────────────────────────────────────
// Public Types
// ────────────────────────────────────────────────────────────────────────────────

/// Parameters about the host track needed for linked stem alignment
#[derive(Debug, Clone)]
pub struct HostTrackParams {
    /// Target deck index (0-3)
    pub deck_idx: usize,
    /// Host track's BPM (for time-stretching)
    pub bpm: f64,
    /// Host track's drop marker position (for alignment)
    pub drop_marker: Option<u64>,
    /// Host track's total duration in samples
    pub duration_samples: u64,
    /// Host track's LUFS (for gain matching)
    pub lufs: Option<f32>,
}

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
    /// High-resolution peaks for stable zoomed view rendering
    /// Computed from the audio buffer at HIGHRES_WIDTH resolution
    pub highres_peaks: Option<Vec<(f32, f32)>>,
    /// Duration of the linked stem buffer in samples (after stretching)
    /// None if load failed
    pub linked_duration: Option<u64>,
    /// Shared buffer reference for UI waveform computation
    /// This is the same buffer as in LinkedStemData, wrapped in Shared for
    /// zero-copy access from both audio engine and UI peaks computation
    pub shared_buffer: Option<Shared<StereoBuffer>>,
}

// ────────────────────────────────────────────────────────────────────────────────
// LinkedStemLoader
// ────────────────────────────────────────────────────────────────────────────────

/// Clonable receiver wrapper for use in iced subscriptions
pub type LinkedStemResultReceiver = Arc<Mutex<Receiver<LinkedStemLoadResult>>>;

/// Shared linked stem loader - can be used by any UI (mesh-player, mesh-cue)
///
/// Handles background loading of linked stems with time-stretching and alignment.
/// Uses rayon thread pool for parallel processing.
///
/// # Message-Driven Usage
///
/// ```ignore
/// // In your iced app:
/// fn subscription(&self) -> Subscription<Message> {
///     linked_stem_subscription(self.linked_stem_loader.result_receiver())
/// }
/// ```
pub struct LinkedStemLoader {
    /// Channel to send load requests
    request_tx: Sender<LinkedStemLoadRequest>,
    /// Channel to receive load results (wrapped for subscription use)
    result_rx: LinkedStemResultReceiver,
    /// Target sample rate for loading
    sample_rate: Arc<AtomicU32>,
    /// Loader thread handle
    _handle: JoinHandle<()>,
}

impl LinkedStemLoader {
    /// Create a new linked stem loader
    ///
    /// Spawns a background thread that processes load requests using rayon.
    pub fn new(sample_rate: u32, db_service: Arc<DatabaseService>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<LinkedStemLoadRequest>();
        let (result_tx, result_rx) = mpsc::channel::<LinkedStemLoadResult>();

        let rate = Arc::new(AtomicU32::new(sample_rate));
        let rate_clone = rate.clone();

        let handle = thread::Builder::new()
            .name("linked-stem-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx, rate_clone, db_service);
            })
            .expect("Failed to spawn linked stem loader thread");

        log::info!(
            "LinkedStemLoader spawned with sample rate: {} Hz",
            sample_rate
        );

        Self {
            request_tx,
            result_rx: Arc::new(Mutex::new(result_rx)),
            sample_rate: rate,
            _handle: handle,
        }
    }

    /// Get a clonable reference to the result receiver for use in subscriptions
    ///
    /// This allows iced apps to create a subscription that receives load results
    /// as messages rather than polling.
    pub fn result_receiver(&self) -> LinkedStemResultReceiver {
        self.result_rx.clone()
    }

    /// Update the target sample rate
    pub fn set_sample_rate(&self, sample_rate: u32) {
        self.sample_rate.store(sample_rate, Ordering::SeqCst);
    }

    /// Queue linked stems for loading from track metadata
    ///
    /// Loads all stem links referenced in the track's metadata.
    pub fn load_from_metadata(&self, stem_links: &[StemLinkReference], host: HostTrackParams) {
        let host_drop = host.drop_marker.unwrap_or(0);

        for link in stem_links {
            let request = LinkedStemLoadRequest {
                host_deck_idx: host.deck_idx,
                stem_idx: link.stem_index as usize,
                source_path: link.source_path.clone(),
                host_bpm: host.bpm,
                host_drop_marker: host_drop,
                host_duration: host.duration_samples,
            };

            if let Err(e) = self.request_tx.send(request) {
                log::error!("Failed to queue linked stem load: {}", e);
            }
        }
    }

    /// Load a single linked stem (non-blocking)
    pub fn load_linked_stem(
        &self,
        host_deck_idx: usize,
        stem_idx: usize,
        source_path: PathBuf,
        host_bpm: f64,
        host_drop_marker: u64,
        host_duration: u64,
    ) -> Result<(), String> {
        self.request_tx
            .send(LinkedStemLoadRequest {
                host_deck_idx,
                stem_idx,
                source_path,
                host_bpm,
                host_drop_marker,
                host_duration,
            })
            .map_err(|e| format!("Loader thread disconnected: {}", e))
    }

    /// Try to receive a single result (non-blocking)
    ///
    /// Prefer using `result_receiver()` with a subscription for message-driven UIs.
    /// This method is provided for simpler use cases or testing.
    pub fn try_recv(&self) -> Option<LinkedStemLoadResult> {
        self.result_rx
            .lock()
            .ok()
            .and_then(|rx| rx.try_recv().ok())
    }
}

// ────────────────────────────────────────────────────────────────────────────────
// Background Thread
// ────────────────────────────────────────────────────────────────────────────────

fn loader_thread(
    rx: Receiver<LinkedStemLoadRequest>,
    tx: Sender<LinkedStemLoadResult>,
    sample_rate: Arc<AtomicU32>,
    db_service: Arc<DatabaseService>,
) {
    log::info!("Linked stem loader thread started");

    while let Ok(request) = rx.recv() {
        // Spawn to rayon thread pool for parallel processing
        let tx_clone = tx.clone();
        let rate = sample_rate.load(Ordering::SeqCst);
        let db = db_service.clone();

        rayon::spawn(move || {
            handle_linked_stem_load(request, tx_clone, rate, &db);
        });
    }

    log::info!("Linked stem loader thread exiting");
}

fn handle_linked_stem_load(request: LinkedStemLoadRequest, tx: Sender<LinkedStemLoadResult>, sample_rate: u32, db_service: &DatabaseService) {
    log::info!(
        "[PERF] LinkedStemLoader: Loading stem {} for deck {} from {:?}",
        request.stem_idx,
        request.host_deck_idx,
        request.source_path
    );

    let total_start = std::time::Instant::now();

    // Determine which database to use based on path
    // USB paths are typically under /run/media/, /media/, or /mnt/
    let load_result = if is_usb_path(&request.source_path) {
        // Try to load metadata from USB's database
        match get_usb_database_for_path(&request.source_path) {
            Some(usb_db) => {
                log::info!(
                    "[LINKED] Using USB database for linked stem from {:?}",
                    request.source_path
                );
                LoadedTrack::load_to(&request.source_path, &usb_db, sample_rate)
            }
            None => {
                log::warn!(
                    "[LINKED] Could not find USB database for {:?}, using local",
                    request.source_path
                );
                LoadedTrack::load_to(&request.source_path, db_service, sample_rate)
            }
        }
    } else {
        // Local path - use local database
        LoadedTrack::load_to(&request.source_path, db_service, sample_rate)
    };

    match load_result {
        Ok(source_track) => {
            // Get source track metadata
            let source_bpm = source_track.metadata.bpm.unwrap_or(120.0);
            let source_drop_marker = source_track.metadata.drop_marker.unwrap_or(0);
            let source_lufs = source_track.metadata.lufs;
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
            source_buffer
                .as_mut_slice()
                .copy_from_slice(stem_buffer.as_slice());

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
                stretch_ratio,
                source_drop_marker,
                stretched_drop_marker,
                stretcher_latency
            );

            log::info!(
                "[PERF] LinkedStemLoader: Pre-stretched stem {} from {:.1} BPM to {:.1} BPM ({} -> {} samples) in {:?}",
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
                "[PERF] LinkedStemLoader: Pre-aligned buffer in {:?}",
                align_start.elapsed()
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

            // Compute high-resolution peaks for stable zoomed view rendering
            let highres_start = std::time::Instant::now();
            let highres_peaks = generate_single_stem_peaks(&aligned_buffer, HIGHRES_WIDTH);
            let highres_elapsed = highres_start.elapsed();
            let total_samples = aligned_buffer.len();
            let samples_per_ms = if highres_elapsed.as_micros() > 0 {
                (total_samples as f64 * 1000.0) / highres_elapsed.as_micros() as f64
            } else {
                0.0
            };
            log::info!(
                "[PERF] Linked stem {} highres peaks: {} samples -> {} peaks in {:?} ({:.1}M samples/sec)",
                request.stem_idx,
                total_samples,
                highres_peaks.len(),
                highres_elapsed,
                samples_per_ms / 1000.0
            );

            log::info!(
                "[PERF] LinkedStemLoader: Total load time: {:?}",
                total_start.elapsed()
            );

            // Wrap aligned buffer in Shared for zero-copy access
            let shared_buffer = Shared::new(&gc_handle(), aligned_buffer);

            let _ = tx.send(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Ok(LinkedStemData {
                    buffer: shared_buffer.clone(),
                    original_bpm: source_bpm,
                    drop_marker: 0, // Not used after alignment
                    track_name,
                    track_path: Some(request.source_path),
                    lufs: source_lufs,
                }),
                overview_peaks,
                highres_peaks: Some(highres_peaks),
                linked_duration: Some(linked_duration),
                shared_buffer: Some(shared_buffer),
            });
        }
        Err(e) => {
            log::error!("Failed to load source track for linked stem: {}", e);

            let _ = tx.send(LinkedStemLoadResult {
                host_deck_idx: request.host_deck_idx,
                stem_idx: request.stem_idx,
                result: Err(e.to_string()),
                overview_peaks: None,
                highres_peaks: None,
                linked_duration: None,
                shared_buffer: None,
            });
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────────
// Helper Functions
// ────────────────────────────────────────────────────────────────────────────────

/// Generate peaks from a single StereoBuffer at the specified resolution
fn generate_single_stem_peaks(buffer: &StereoBuffer, width: usize) -> Vec<(f32, f32)> {
    let len = buffer.len();
    if len == 0 || width == 0 {
        return Vec::new();
    }

    let samples_per_column = len / width;
    if samples_per_column == 0 {
        return Vec::new();
    }

    (0..width)
        .map(|col| {
            let start = col * samples_per_column;
            let end = ((col + 1) * samples_per_column).min(len);

            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;

            for i in start..end {
                let sample = (buffer[i].left + buffer[i].right) / 2.0;
                min = min.min(sample);
                max = max.max(sample);
            }

            if min.is_infinite() {
                min = 0.0;
            }
            if max.is_infinite() {
                max = 0.0;
            }

            (min, max)
        })
        .collect()
}

/// Align overview peaks to the host timeline using drop marker alignment
fn align_peaks_to_host(
    peaks: &[(f32, f32)],
    stretched_duration: usize,
    host_duration: usize,
    host_drop_marker: u64,
    stretched_drop_marker: u64,
) -> Vec<(f32, f32)> {
    let offset = host_drop_marker as i64 - stretched_drop_marker as i64;

    let mut aligned_peaks = vec![(0.0f32, 0.0f32); OVERVIEW_PEAK_WIDTH];

    for i in 0..OVERVIEW_PEAK_WIDTH {
        // Host position in samples for this peak index
        let host_sample = (i as f64 / OVERVIEW_PEAK_WIDTH as f64) * host_duration as f64;

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

/// Check if a path appears to be on a USB device
///
/// Uses common Linux mount point patterns for removable media.
fn is_usb_path(path: &std::path::Path) -> bool {
    if let Some(path_str) = path.to_str() {
        path_str.starts_with("/run/media/")
            || path_str.starts_with("/media/")
            || path_str.starts_with("/mnt/")
    } else {
        false
    }
}

/// Get the DatabaseService for a USB path
///
/// Delegates to the centralized USB cache in the usb module.
fn get_usb_database_for_path(path: &std::path::Path) -> Option<Arc<DatabaseService>> {
    usb_cache::get_usb_database_for_path(path)
}
