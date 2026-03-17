//! Set recording — capture master output to WAV files
//!
//! # Architecture
//!
//! The recording system uses the same lock-free pattern as the cue output:
//!
//! ```text
//! Audio Thread
//!     │ rtrb::Producer<StereoSample> (never blocks)
//!     ▼
//! Recording Thread (normal priority)
//!     │ hound::WavWriter with periodic flush()
//!     ▼
//! USB Stick (mesh-recordings/YYYY-MM-DD_HH-MM.wav)
//! ```
//!
//! The ring buffer absorbs I/O stalls (2 seconds capacity).
//! If the buffer fills (writer fell behind), samples are dropped
//! rather than blocking the audio thread.

mod writer;

use crate::types::StereoSample;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Events sent from the recording thread back to the UI
#[derive(Debug, Clone)]
pub enum RecordingEvent {
    /// Recording started successfully
    Started {
        /// Path to the WAV file being written
        path: PathBuf,
    },
    /// Recording stopped normally
    Stopped {
        /// Path to the completed WAV file
        path: PathBuf,
        /// Duration in seconds
        duration_secs: f64,
        /// Path to the tracklist file (if generated)
        tracklist_path: Option<PathBuf>,
    },
    /// Recording failed (I/O error, disk full, etc.)
    Error {
        /// Path that was being written to
        path: PathBuf,
        /// Error description
        message: String,
    },
}

/// Receiver type for recording events (Arc<Mutex<>> for iced subscription)
pub type RecordingEventReceiver = Arc<std::sync::Mutex<mpsc::Receiver<RecordingEvent>>>;

/// Handle to an active recording on a single USB stick
///
/// Each `RecordingHandle` owns one recording thread writing to one WAV file.
/// Multiple handles can be active simultaneously (one per USB stick).
pub struct RecordingHandle {
    /// Path to the WAV file being written
    pub path: PathBuf,
    /// Stop flag — set to true to gracefully stop recording
    stop_flag: Arc<AtomicBool>,
    /// Thread handle (joined on drop)
    thread: Option<JoinHandle<()>>,
}

impl RecordingHandle {
    /// Signal the recording thread to stop and finalize the WAV file
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Release);
    }
}

impl Drop for RecordingHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Minimum free space required to start recording (2 GB).
/// A 2-hour WAV at 48kHz/16-bit/stereo is ~1.32 GB.
const MIN_FREE_SPACE_BYTES: u64 = 2_000_000_000;

/// Start recording master output to a WAV file on the given USB stick.
///
/// Creates `{usb_mount}/mesh-recordings/YYYY-MM-DD_HH-MM.wav` and spawns
/// a recording thread that reads from the returned ring buffer producer.
///
/// # Arguments
/// * `usb_mount` — Mount point of the USB stick (e.g., `/media/user/MESH_USB`)
/// * `sample_rate` — Audio sample rate (44100 or 48000)
/// * `available_bytes` — Free space on the USB stick in bytes
/// * `event_tx` — Channel to send recording events back to the UI
///
/// # Returns
/// * `(rtrb::Producer<StereoSample>, RecordingHandle)` — the producer goes to
///   the audio thread, the handle stays with the UI
pub fn start_recording(
    usb_mount: &Path,
    sample_rate: u32,
    available_bytes: u64,
    event_tx: mpsc::Sender<RecordingEvent>,
) -> Result<(rtrb::Producer<StereoSample>, RecordingHandle), String> {
    // Pre-flight disk space check
    if available_bytes < MIN_FREE_SPACE_BYTES {
        let avail_gb = available_bytes as f64 / 1_000_000_000.0;
        return Err(format!(
            "Not enough space for recording: {avail_gb:.1} GB free, need at least 2 GB"
        ));
    }

    // Create recordings directory
    let recordings_dir = usb_mount.join("mesh-recordings");
    std::fs::create_dir_all(&recordings_dir)
        .map_err(|e| format!("Failed to create recordings directory: {e}"))?;

    // Generate timestamped filename
    let now = chrono::Local::now();
    let filename = now.format("%Y-%m-%d_%H-%M").to_string();
    let wav_path = recordings_dir.join(format!("{filename}.wav"));

    // Avoid overwriting: append suffix if file exists
    let wav_path = if wav_path.exists() {
        let mut suffix = 1u32;
        loop {
            let candidate = recordings_dir.join(format!("{filename}_{suffix}.wav"));
            if !candidate.exists() {
                break candidate;
            }
            suffix += 1;
        }
    } else {
        wav_path
    };

    // Ring buffer: 2 seconds of stereo audio
    let capacity = sample_rate as usize * 2;
    let (producer, consumer) = rtrb::RingBuffer::<StereoSample>::new(capacity);

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);
    let path_clone = wav_path.clone();

    let thread = std::thread::Builder::new()
        .name("set-recorder".to_string())
        .spawn(move || {
            writer::recording_thread(
                consumer,
                &path_clone,
                sample_rate,
                stop_clone,
                event_tx,
            );
        })
        .map_err(|e| format!("Failed to spawn recording thread: {e}"))?;

    let handle = RecordingHandle {
        path: wav_path,
        stop_flag,
        thread: Some(thread),
    };

    Ok((producer, handle))
}

/// Generate a tracklist TXT file from session history.
///
/// Queries the `track_plays` DB relation for all tracks played during the
/// recording window and writes a formatted tracklist next to the WAV file.
///
/// # Arguments
/// * `wav_path` — Path to the WAV file (tracklist goes next to it with .txt extension)
/// * `recording_start_ms` — Unix timestamp (ms) when recording started
/// * `recording_end_ms` — Unix timestamp (ms) when recording stopped
/// * `session_id` — Session ID to filter plays
/// * `db` — Database service to query play history from
pub fn generate_tracklist(
    wav_path: &Path,
    recording_start_ms: i64,
    recording_end_ms: i64,
    session_id: i64,
    db: &crate::db::DatabaseService,
) -> Option<PathBuf> {
    let txt_path = wav_path.with_extension("txt");

    // Query all track plays from this session that overlap with the recording window
    let plays = match query_plays_in_window(db, session_id, recording_start_ms, recording_end_ms) {
        Ok(plays) => plays,
        Err(e) => {
            log::warn!("[RECORDING] Failed to query play history for tracklist: {e}");
            return None;
        }
    };

    if plays.is_empty() {
        log::info!("[RECORDING] No tracks played during recording — skipping tracklist");
        return None;
    }

    // Format tracklist
    let duration_secs = (recording_end_ms - recording_start_ms) as f64 / 1000.0;
    let duration_str = format_duration(duration_secs);
    let start_time = chrono::DateTime::from_timestamp_millis(recording_start_ms)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let mut content = format!(
        "Mesh Set Recording — {start_time}\nDuration: {duration_str}\n\n"
    );

    for play in &plays {
        let offset_ms = play.loaded_at.saturating_sub(recording_start_ms).max(0);
        let offset_str = format_duration(offset_ms as f64 / 1000.0);
        content.push_str(&format!(
            "{}  {} [Deck {}]\n",
            offset_str,
            play.track_name,
            play.deck_index + 1,
        ));
    }

    match std::fs::write(&txt_path, &content) {
        Ok(()) => {
            log::info!("[RECORDING] Tracklist written: {}", txt_path.display());
            Some(txt_path)
        }
        Err(e) => {
            log::warn!("[RECORDING] Failed to write tracklist: {e}");
            None
        }
    }
}

/// A track play record for tracklist generation
#[derive(Debug)]
struct TrackPlayEntry {
    track_name: String,
    deck_index: u8,
    loaded_at: i64,
}

/// Query track plays that fall within the recording time window
fn query_plays_in_window(
    db: &crate::db::DatabaseService,
    session_id: i64,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<TrackPlayEntry>, String> {
    use std::collections::BTreeMap;
    use cozo::DataValue;

    let mut params = BTreeMap::new();
    params.insert("session_id".to_string(), DataValue::from(session_id));
    params.insert("start_ms".to_string(), DataValue::from(start_ms));
    params.insert("end_ms".to_string(), DataValue::from(end_ms));

    let result = db.run_query(r#"
        ?[track_name, deck_index, loaded_at] :=
            *track_plays{session_id: $session_id, loaded_at, track_name, deck_index},
            loaded_at >= $start_ms,
            loaded_at <= $end_ms
        :order loaded_at
    "#, params).map_err(|e| e.to_string())?;

    let plays = result.rows.iter().filter_map(|row| {
        let track_name = row.get(0)?.get_str()?.to_string();
        let deck_index = row.get(1)?.get_int()? as u8;
        let loaded_at = row.get(2)?.get_int()?;
        Some(TrackPlayEntry { track_name, deck_index, loaded_at })
    }).collect();

    Ok(plays)
}

/// Format seconds as HH:MM:SS
fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
