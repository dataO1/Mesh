# Set Recording Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Record the master audio output to WAV files on connected USB sticks with a companion tracklist TXT file.

**Architecture:** A lock-free SPSC ring buffer (`rtrb`) taps the master output in the audio thread and feeds a dedicated recording thread that writes 16-bit WAV via `hound`. The recording thread runs at normal priority and never blocks the audio thread. On stop, the session history DB is queried to generate a timestamped tracklist next to the WAV file.

**Tech Stack:** `hound` (pure Rust WAV), `rtrb` (already in project). No `chrono` — timestamps use `std::time::SystemTime` with manual formatting (project convention).

---

## Task 1: Add `hound` dependency to mesh-core

**Files:**
- Modify: `crates/mesh-core/Cargo.toml`

**Step 1: Add hound dependency**

In `crates/mesh-core/Cargo.toml`, add after the `basedrop` line (line 30):

```toml
hound = "3.5"           # WAV file writing for set recording
chrono = { version = "0.4", default-features = false, features = ["clock"] }  # Local timestamps for recording filenames
```

**Step 2: Verify it compiles**

Run: `cargo check -p mesh-core`
Expected: Compiles with hound available

**Step 3: Commit**

```bash
git add crates/mesh-core/Cargo.toml
git commit -m "feat(recording): add hound dependency for WAV writing"
```

---

## Task 2: Create the recording module (`mesh-core/src/recording/`)

This is the core recording infrastructure: types, state management, and the recording thread.

**Files:**
- Create: `crates/mesh-core/src/recording/mod.rs`
- Create: `crates/mesh-core/src/recording/writer.rs`
- Modify: `crates/mesh-core/src/lib.rs` (line 90, add `pub mod recording;`)

### `mod.rs` — Public API and types

```rust
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
    /// Ring buffer producer — audio thread pushes master samples here
    pub producer: rtrb::Producer<StereoSample>,
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

/// Start recording master output to a WAV file on the given USB stick.
///
/// Creates `{usb_mount}/mesh-recordings/YYYY-MM-DD_HH-MM.wav` and spawns
/// a recording thread that reads from the returned ring buffer producer.
///
/// # Arguments
/// * `usb_mount` — Mount point of the USB stick (e.g., `/media/user/MESH_USB`)
/// * `sample_rate` — Audio sample rate (44100 or 48000)
/// * `event_tx` — Channel to send recording events back to the UI
///
/// # Returns
/// * `RecordingHandle` containing the `rtrb::Producer<StereoSample>` that the
///   audio thread should push master samples into
/// Minimum free space required to start recording (2 GB).
/// A 2-hour WAV at 48kHz/16-bit/stereo is ~1.32 GB.
const MIN_FREE_SPACE_BYTES: u64 = 2_000_000_000;

pub fn start_recording(
    usb_mount: &Path,
    sample_rate: u32,
    available_bytes: u64,
    event_tx: mpsc::Sender<RecordingEvent>,
) -> Result<RecordingHandle, String> {
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

    Ok(RecordingHandle {
        producer,
        path: wav_path,
        stop_flag,
        thread: Some(thread),
    })
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
```

### `writer.rs` — Recording thread implementation

```rust
//! Recording thread — reads from ring buffer, writes WAV via hound
//!
//! The thread runs at normal priority and uses buffered I/O.
//! It periodically calls `hound::WavWriter::flush()` to update the WAV
//! header, so partial recordings are recoverable after crashes.

use crate::types::StereoSample;
use super::RecordingEvent;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::io::BufWriter;

/// Flush interval: update WAV header every ~10 seconds for crash safety
const FLUSH_INTERVAL_SAMPLES: u64 = 48000 * 10; // ~10s at 48kHz

/// Recording thread main function
///
/// Reads `StereoSample` from the ring buffer consumer, converts to 16-bit PCM,
/// and writes via hound. Runs until `stop_flag` is set or the producer is dropped.
pub fn recording_thread(
    mut consumer: rtrb::Consumer<StereoSample>,
    path: &Path,
    sample_rate: u32,
    stop_flag: Arc<AtomicBool>,
    event_tx: mpsc::Sender<RecordingEvent>,
) {
    // Pin to big cores on embedded (A76 cores 4-7) so we never compete
    // with the RT JACK audio thread on the LITTLE A55 cores 0-3.
    crate::rt::pin_to_big_cores();

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    // Open WAV writer with buffered I/O (128 KB buffer)
    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            let _ = event_tx.send(RecordingEvent::Error {
                path: path.to_path_buf(),
                message: format!("Failed to create WAV file: {e}"),
            });
            return;
        }
    };
    let buf_writer = BufWriter::with_capacity(128 * 1024, file);
    let mut writer = match hound::WavWriter::new(buf_writer, spec) {
        Ok(w) => w,
        Err(e) => {
            let _ = event_tx.send(RecordingEvent::Error {
                path: path.to_path_buf(),
                message: format!("Failed to initialize WAV writer: {e}"),
            });
            return;
        }
    };

    // Notify UI that recording has started
    let _ = event_tx.send(RecordingEvent::Started {
        path: path.to_path_buf(),
    });

    let mut total_samples: u64 = 0;
    let mut samples_since_flush: u64 = 0;
    let start_time = std::time::Instant::now();

    loop {
        // Check stop flag
        if stop_flag.load(Ordering::Acquire) {
            break;
        }

        // Read available samples from ring buffer
        let available = consumer.slots();
        if available == 0 {
            // No data available — sleep briefly to avoid busy-spinning
            // 1ms sleep is fine: at 48kHz with 256-sample buffers, new data
            // arrives every ~5.3ms. We have 2 seconds of buffer headroom.
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }

        // Read in chunks to reduce per-sample overhead
        let chunk = consumer.read_chunk(available).unwrap();
        let (first, second) = chunk.as_slices();

        for slice in [first, second] {
            for sample in slice {
                // Convert f32 [-1.0, 1.0] to i16 with clipping
                let left = f32_to_i16(sample.left);
                let right = f32_to_i16(sample.right);

                if let Err(e) = writer.write_sample(left) {
                    let _ = event_tx.send(RecordingEvent::Error {
                        path: path.to_path_buf(),
                        message: format!("WAV write error: {e}"),
                    });
                    return;
                }
                if let Err(e) = writer.write_sample(right) {
                    let _ = event_tx.send(RecordingEvent::Error {
                        path: path.to_path_buf(),
                        message: format!("WAV write error: {e}"),
                    });
                    return;
                }

                total_samples += 1;
                samples_since_flush += 1;
            }
        }
        chunk.commit_all();

        // Periodic flush for crash safety
        if samples_since_flush >= FLUSH_INTERVAL_SAMPLES {
            if let Err(e) = writer.flush() {
                // Covers both USB removal (ENODEV/EIO) and disk full (ENOSPC)
                let _ = event_tx.send(RecordingEvent::Error {
                    path: path.to_path_buf(),
                    message: format!("WAV flush error (USB removed or disk full?): {e}"),
                });
                return;
            }
            samples_since_flush = 0;
        }
    }

    // Finalize WAV file (writes correct header sizes)
    let duration = start_time.elapsed().as_secs_f64();
    match writer.finalize() {
        Ok(()) => {
            log::info!(
                "[RECORDING] Finalized: {} ({:.1}s, {} samples)",
                path.display(), duration, total_samples
            );
            // Note: tracklist generation happens on the UI side after this event
            let _ = event_tx.send(RecordingEvent::Stopped {
                path: path.to_path_buf(),
                duration_secs: duration,
                tracklist_path: None,
            });
        }
        Err(e) => {
            let _ = event_tx.send(RecordingEvent::Error {
                path: path.to_path_buf(),
                message: format!("WAV finalize error: {e}"),
            });
        }
    }
}

/// Convert f32 sample [-1.0, 1.0] to i16 with hard clipping
#[inline]
fn f32_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32) as i16
}
```

### Register the module

In `crates/mesh-core/src/lib.rs`, add at line 90 (before `pub use types::*`):

```rust
pub mod recording;
```

**Step: Verify compilation**

Run: `cargo check -p mesh-core`
Expected: PASS — new module compiles, no consumers yet

**Step: Commit**

```bash
git add crates/mesh-core/src/recording/ crates/mesh-core/src/lib.rs
git commit -m "feat(recording): add recording module with WAV writer thread"
```

---

## Task 3: Add recording state to the audio backends

The audio thread needs an `Option<rtrb::Producer<StereoSample>>` to push master samples into when recording is active. This is controlled by new `EngineCommand` variants.

**Files:**
- Modify: `crates/mesh-core/src/engine/command.rs` — add `StartRecording` / `StopRecording` variants
- Modify: `crates/mesh-core/src/audio/jack_backend.rs` — add producer field + push in process loop
- Modify: `crates/mesh-core/src/audio/cpal_backend.rs` — add producer field + push in both stream builders

### Step 1: Add engine commands

In `crates/mesh-core/src/engine/command.rs`, add after `SetPhaseSync(bool)` (line 570), before the closing `}`:

```rust

    // ─────────────────────────────────────────────────────────────
    // Set Recording
    // ─────────────────────────────────────────────────────────────
    /// Start recording master output to a ring buffer
    ///
    /// The producer writes to a recording thread that saves WAV to disk.
    /// Multiple producers can be active (one per USB stick).
    StartRecording {
        /// Ring buffer producer — audio thread pushes master samples here
        producer: rtrb::Producer<crate::types::StereoSample>,
    },
    /// Stop all active recordings
    ///
    /// Drops all recording producers, which signals the recording threads
    /// to finalize their WAV files and exit.
    StopRecording,
```

**Important:** This adds `rtrb::Producer<StereoSample>` (24 bytes) to the enum. The command size test at line 621 asserts `size <= 40`. `rtrb::Producer` is a pointer + two atomics = ~24 bytes, plus the discriminant. Check with `cargo test -p mesh-core` — if the test fails, raise the limit to 48.

### Step 2: Add recording producers to JACK backend

In `crates/mesh-core/src/audio/jack_backend.rs`:

Add field to `JackProcessor` struct (after `latency_measured`, around line 107):

```rust
    /// Active recording producers (one per USB stick)
    recording_producers: Vec<rtrb::Producer<crate::types::StereoSample>>,
```

Initialize in the constructor (find where `JackProcessor` is created, add):

```rust
recording_producers: Vec::new(),
```

In `process()` method (after the for loop at line 142-150, before `Control::Continue`), add:

```rust
        // Push master samples to recording producers (if recording)
        if !self.recording_producers.is_empty() {
            let master_slice = self.master_buffer.as_slice();
            self.recording_producers.retain_mut(|producer| {
                for &sample in &master_slice[..n_frames] {
                    if producer.push(sample).is_err() {
                        // Buffer full — drop samples rather than block audio
                        break;
                    }
                }
                // Keep producer if it's still connected
                !producer.is_abandoned()
            });
        }
```

Add command processing (in `process_commands` or wherever `EngineCommand` is matched in the engine — the audio thread processes commands via `self.engine.process_commands()` which is on the engine, not the processor):

**Note:** The engine's `process_commands` match needs updating, but recording producers live on the *backend processor*, not the engine. We need to handle `StartRecording`/`StopRecording` in the processor's command loop, BEFORE passing to the engine.

Actually, examining the architecture more carefully: commands go through `rtrb::Consumer<EngineCommand>` which is consumed by `engine.process_commands()`. The engine matches all variants. So we need to add the match arms in the engine's command processing.

**Alternative approach:** Instead of routing through EngineCommand (which the engine matches), store the recording producers directly on the processor/callback state and handle the commands there. But this requires a separate command channel just for recording, which is over-engineered.

**Simplest approach:** Add `recording_producers: Vec<rtrb::Producer<StereoSample>>` to `AudioEngine` itself and handle the commands there. The engine already has access to the master buffer after `process()`. But the engine processes audio, it doesn't own the output buffers.

**Best approach for this codebase:** Since the backend processors (JackProcessor, AudioCallbackState) own the output buffers and call `engine.process()`, and they also have their own command consumers, the cleanest path is:

1. Add `recording_producers` to `AudioEngine` (it already processes commands)
2. After `engine.process()`, the backend reads `engine.recording_producers` and pushes samples
3. The engine handles `StartRecording`/`StopRecording` by modifying `recording_producers`

This requires making `recording_producers` `pub(crate)` on `AudioEngine`.

In `crates/mesh-core/src/engine/engine.rs`, add a field to `AudioEngine`:

```rust
    /// Active recording producers (one per USB stick being recorded to)
    pub(crate) recording_producers: Vec<rtrb::Producer<StereoSample>>,
```

Initialize in `AudioEngine::new()`:

```rust
    recording_producers: Vec::new(),
```

In the engine's command processing (find where `EngineCommand` variants are matched), add:

```rust
EngineCommand::StartRecording { producer } => {
    self.recording_producers.push(producer);
    log::info!("[ENGINE] Recording started ({} active)", self.recording_producers.len());
}
EngineCommand::StopRecording => {
    let count = self.recording_producers.len();
    self.recording_producers.clear();
    log::info!("[ENGINE] Recording stopped ({count} producers dropped)");
}
```

### Step 3: Push samples in JACK backend

In `crates/mesh-core/src/audio/jack_backend.rs`, in the `process()` method, after the output copy loop and before `Control::Continue`:

```rust
        // Push master samples to active recording producers
        if !self.engine.recording_producers.is_empty() {
            let master_slice = self.master_buffer.as_slice();
            self.engine.recording_producers.retain_mut(|producer| {
                for &sample in &master_slice[..n_frames] {
                    if producer.push(sample).is_err() {
                        break; // Buffer full — drop rather than block
                    }
                }
                !producer.is_abandoned()
            });
        }
```

### Step 4: Push samples in CPAL backend

In `crates/mesh-core/src/audio/cpal_backend.rs`, add a helper method to `AudioCallbackState`:

```rust
    /// Push master samples to active recording producers
    fn push_to_recording(&mut self, n_frames: usize) {
        if !self.engine.recording_producers.is_empty() {
            let master_slice = self.master_buffer.as_slice();
            self.engine.recording_producers.retain_mut(|producer| {
                for &sample in &master_slice[..n_frames] {
                    if producer.push(sample).is_err() {
                        break;
                    }
                }
                !producer.is_abandoned()
            });
        }
    }
```

Call `state.push_to_recording(n_frames)` in:
- `build_output_stream()` — after the master output copy loop (around line 585)
- `build_master_stream_dual()` — after the cue push loop (around line 706)

**Important:** `push_to_recording` is called while `state` is locked via `state.lock().unwrap()`. The push to `rtrb::Producer` is wait-free (~50ns), so this does not meaningfully extend the lock duration.

### Step 5: Verify compilation and run tests

Run: `cargo check -p mesh-core && cargo test -p mesh-core`
Expected: PASS. If the `test_command_size` test fails, update the assertion from `<= 40` to `<= 48`.

### Step 6: Commit

```bash
git add crates/mesh-core/src/engine/command.rs crates/mesh-core/src/engine/engine.rs \
        crates/mesh-core/src/audio/jack_backend.rs crates/mesh-core/src/audio/cpal_backend.rs
git commit -m "feat(recording): tap master output to recording ring buffer in audio backends"
```

---

## Task 4: Add recording messages and state to mesh-player

**Files:**
- Modify: `crates/mesh-player/src/ui/message.rs` — add `RecordingMessage` enum, `ToggleRecording` variant to `SettingsMessage`
- Modify: `crates/mesh-player/src/ui/app.rs` — add recording state fields to `MeshApp`

### Step 1: Add message types

In `crates/mesh-player/src/ui/message.rs`, add a new message enum:

```rust
/// Messages for set recording
#[derive(Debug, Clone)]
pub enum RecordingMessage {
    /// Recording event received from recording thread
    Event(mesh_core::recording::RecordingEvent),
}
```

Add variant to the main `Message` enum:

```rust
    Recording(RecordingMessage),
```

Add to `SettingsMessage`:

```rust
    /// Toggle set recording on/off
    ToggleRecording(bool),
```

### Step 2: Add recording state to MeshApp

In `crates/mesh-player/src/ui/app.rs`, add fields to `MeshApp`:

```rust
    /// Active recording state (None = not recording)
    pub recording_state: Option<RecordingState>,
```

Define `RecordingState` (in app.rs or a new `recording.rs` handler file):

```rust
/// UI-side state for an active set recording
pub struct RecordingState {
    /// When recording started (for elapsed time display)
    pub started_at: std::time::Instant,
    /// When recording started (Unix ms, for tracklist query window)
    pub started_at_ms: i64,
    /// Active recording handles (one per USB stick)
    pub handles: Vec<mesh_core::recording::RecordingHandle>,
    /// Event receiver for recording thread messages
    pub event_rx: mesh_core::recording::RecordingEventReceiver,
    /// Event sender (kept alive so recording threads can send)
    pub event_tx: std::sync::mpsc::Sender<mesh_core::recording::RecordingEvent>,
    /// Number of recordings that have errored
    pub error_count: usize,
    /// WAV paths that completed successfully (for tracklist generation)
    pub completed_paths: Vec<std::path::PathBuf>,
}
```

Initialize `recording_state: None` in `MeshApp::new()`.

### Step 3: Commit

```bash
git add crates/mesh-player/src/ui/message.rs crates/mesh-player/src/ui/app.rs
git commit -m "feat(recording): add recording messages and state to mesh-player"
```

---

## Task 5: Implement recording start/stop logic in domain

**Files:**
- Modify: `crates/mesh-player/src/domain/mod.rs` or create `crates/mesh-player/src/ui/handlers/recording.rs`

### Recording toggle handler

When the settings toggle is switched ON:

1. Get list of connected USB sticks from `self.collection_browser.usb_devices`
2. For each USB stick, call `mesh_core::recording::start_recording(mount_path, sample_rate, event_tx)`
3. For each returned `RecordingHandle`, send `EngineCommand::StartRecording { producer }` where producer is taken from the handle
4. Store all handles in `RecordingState`

**Important subtlety:** `RecordingHandle` owns the `rtrb::Producer`, but we need to move the producer into the `EngineCommand`. So `start_recording()` should return the producer and handle separately, OR `RecordingHandle` should not own the producer (the audio thread owns it).

**Revised approach:** `start_recording()` returns `(rtrb::Producer<StereoSample>, RecordingHandle)` where `RecordingHandle` just has the stop flag and thread handle. The caller sends the producer via EngineCommand and stores the handle for lifecycle management.

Update `start_recording()` signature accordingly:

```rust
pub fn start_recording(
    usb_mount: &Path,
    sample_rate: u32,
    available_bytes: u64,
    event_tx: mpsc::Sender<RecordingEvent>,
) -> Result<(rtrb::Producer<StereoSample>, RecordingHandle), String>
```

Where `RecordingHandle` no longer contains the producer:

```rust
pub struct RecordingHandle {
    pub path: PathBuf,
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
```

When the settings toggle is switched OFF:

1. Send `EngineCommand::StopRecording` — audio thread drops all producers
2. Each recording thread detects the abandoned consumer and finalizes its WAV
3. For each completed recording, call `generate_tracklist()` with the session history

### Subscription for recording events

Set up an iced subscription (same pattern as PresetLoader):

```rust
fn recording_subscription(&self) -> iced::Subscription<Message> {
    if let Some(ref state) = self.recording_state {
        let rx = state.event_rx.clone();
        iced::Subscription::run_with_id(
            "recording-events",
            async_stream::stream! {
                loop {
                    let event = {
                        let rx = rx.lock().unwrap();
                        rx.try_recv().ok()
                    };
                    if let Some(event) = event {
                        yield Message::Recording(RecordingMessage::Event(event));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
        )
    } else {
        iced::Subscription::none()
    }
}
```

Add to the app's `subscription()` method.

### Step: Commit

```bash
git add crates/mesh-player/src/
git commit -m "feat(recording): implement start/stop logic and event subscription"
```

---

## Task 6: Add recording toggle to settings UI

**Files:**
- Modify: `crates/mesh-player/src/ui/settings.rs`

### Step 1: Add recording toggle as first settings item

In `build_settings_items()`, add as the very first item (before the Power Off entry), so it's at the top of settings:

```rust
    // ── Set Recording (always first for quick access) ──
    items.push(
        SettingsItem::new("Record Set", SettingsBehavior::Toggle {
            value: state.recording_active,
            on_toggle: |v| SettingsMessage::ToggleRecording(v),
        })
            .section("Recording")
            .hint("Record master output to WAV on all connected USB sticks")
    );
```

### Step 2: Add `recording_active` to `SettingsState`

```rust
    /// Whether set recording is active
    pub recording_active: bool,
```

Initialize as `false` in `from_config()`.

### Step 3: Commit

```bash
git add crates/mesh-player/src/ui/settings.rs
git commit -m "feat(recording): add recording toggle to settings UI"
```

---

## Task 7: Add pulsing recording indicator to header

**Files:**
- Modify: `crates/mesh-player/src/ui/app.rs` — in `view_header()`

### Step 1: Add recording indicator to the right group

In `view_header()`, before the `right_group` construction (around line 1822), add:

```rust
        // Recording indicator (pulsing red dot + elapsed time)
        let recording_indicator: Element<'_, Message> = if let Some(ref rec) = self.recording_state {
            let elapsed = rec.started_at.elapsed().as_secs();
            let h = elapsed / 3600;
            let m = (elapsed % 3600) / 60;
            let s = elapsed % 60;
            let elapsed_str = format!("{h:02}:{m:02}:{s:02}");

            // Pulsing: toggle visibility based on elapsed seconds (blink every second)
            let dot_visible = elapsed % 2 == 0;
            let dot_color = if dot_visible {
                Color::from_rgb(1.0, 0.0, 0.0)
            } else {
                Color::from_rgb(0.5, 0.0, 0.0)
            };

            row![
                text("●").size(sz(14.0)).color(dot_color),
                text(format!(" REC {elapsed_str}")).size(sz(12.0)).color(Color::from_rgb(1.0, 0.3, 0.3)),
            ]
            .align_y(CenterAlign)
            .into()
        } else {
            Space::new().width(0).into()
        };
```

Then add `recording_indicator` to the `right_group` row, before `stats_label`:

```rust
        let right_group: Element<'_, Message> = row![
            recording_indicator,
            stats_label,
            connection_status,
            latency_label,
            settings_btn,
        ]
        .spacing(12)
        .align_y(CenterAlign)
        .into();
```

**Note:** The pulsing effect relies on the existing `tick` subscription that already refreshes the UI periodically (for waveform animation). The dot toggles between bright red and dark red each second, creating a pulse effect without any additional state.

### Step 2: Commit

```bash
git add crates/mesh-player/src/ui/app.rs
git commit -m "feat(recording): add pulsing REC indicator to header"
```

---

## Task 8: Handle recording events and tracklist generation

**Files:**
- Modify: `crates/mesh-player/src/ui/app.rs` (or handler file)

### Step 1: Handle RecordingMessage::Event

In the app's `update()` method, add a match arm for `Message::Recording`:

```rust
Message::Recording(RecordingMessage::Event(event)) => {
    match event {
        RecordingEvent::Started { path } => {
            log::info!("[UI] Recording started: {}", path.display());
        }
        RecordingEvent::Stopped { path, duration_secs, .. } => {
            log::info!("[UI] Recording stopped: {} ({:.1}s)", path.display(), duration_secs);
            // Generate tracklist for this recording
            if let Some(ref rec_state) = self.recording_state {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                // Spawn tracklist generation in background
                let db = self.domain.local_db().clone();
                let session_id = self.history.session_id();
                let start_ms = rec_state.started_at_ms;
                let wav_path = path.clone();
                std::thread::spawn(move || {
                    mesh_core::recording::generate_tracklist(
                        &wav_path, start_ms, now_ms, session_id, &db,
                    );
                });
            }
        }
        RecordingEvent::Error { path, message } => {
            log::error!("[UI] Recording error on {}: {}", path.display(), message);
            self.status = format!("Recording error: {message}");
            if let Some(ref mut rec_state) = self.recording_state {
                rec_state.error_count += 1;
            }
        }
    }
    Task::none()
}
```

### Step 2: Expose session_id on HistoryManager

Add a public getter to `HistoryManager`:

```rust
    pub fn session_id(&self) -> i64 {
        self.session_id
    }
```

### Step 3: Expose local_db on MeshDomain

Add a public getter if not already present:

```rust
    pub fn local_db(&self) -> &Arc<DatabaseService> {
        &self.local_db
    }
```

### Step 4: Commit

```bash
git add crates/mesh-player/src/ crates/mesh-player/src/history/
git commit -m "feat(recording): handle recording events and generate tracklist on stop"
```

---

## Task 9: Wire SettingsMessage::ToggleRecording handler

**Files:**
- Modify: wherever `SettingsMessage` is matched (likely `crates/mesh-player/src/ui/handlers/settings.rs` or in `app.rs`)

### Step 1: Implement toggle handler

```rust
SettingsMessage::ToggleRecording(enabled) => {
    self.settings.recording_active = enabled;

    if enabled {
        // Start recording on all connected USB sticks
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let event_rx = Arc::new(std::sync::Mutex::new(event_rx));
        let sample_rate = self.audio_sample_rate;
        let mut handles = Vec::new();

        // Get USB devices with mount points and available space
        let usb_devices: Vec<(PathBuf, u64)> = self.collection_browser.usb_devices
            .iter()
            .filter_map(|d| d.mount_point.clone().map(|mp| (mp, d.available_bytes)))
            .collect();

        if usb_devices.is_empty() {
            self.status = "No USB sticks connected for recording".to_string();
            self.settings.recording_active = false;
            return Task::none();
        }

        for (mount, available_bytes) in &usb_devices {
            match mesh_core::recording::start_recording(mount, sample_rate, *available_bytes, event_tx.clone()) {
                Ok((producer, handle)) => {
                    // Send producer to audio thread
                    self.domain.send_command(EngineCommand::StartRecording { producer });
                    handles.push(handle);
                }
                Err(e) => {
                    log::error!("[RECORDING] Failed to start on {}: {e}", mount.display());
                }
            }
        }

        if handles.is_empty() {
            self.status = "Failed to start recording on any USB stick".to_string();
            self.settings.recording_active = false;
            return Task::none();
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        self.recording_state = Some(RecordingState {
            started_at: std::time::Instant::now(),
            started_at_ms: now_ms,
            handles,
            event_rx,
            event_tx,
            error_count: 0,
            completed_paths: Vec::new(),
        });

        self.status = format!("Recording to {} USB stick(s)", handles.len());
    } else {
        // Stop recording
        self.domain.send_command(EngineCommand::StopRecording);
        // Handles are dropped, which sets stop flags and joins threads
        self.recording_state = None;
        self.settings.recording_active = false;
        self.status = "Recording stopped".to_string();
    }

    Task::none()
}
```

### Step 2: Commit

```bash
git add crates/mesh-player/src/
git commit -m "feat(recording): wire settings toggle to start/stop recording"
```

---

## Task 10: Integration test and final verification

**Step 1: Build and run**

```bash
cargo build -p mesh-player 2>&1 | head -50
```

Fix any compilation errors.

**Step 2: Manual test plan**

1. Launch mesh-player with a USB stick connected
2. Open Settings → Recording section should be at the top
3. Toggle "Record Set" ON → status should say "Recording to 1 USB stick(s)"
4. Verify pulsing red REC indicator in header with elapsed time
5. Load and play tracks on decks
6. Toggle "Record Set" OFF → recording stops
7. Check USB stick for `mesh-recordings/YYYY-MM-DD_HH-MM.wav` and `.txt` tracklist
8. Verify WAV plays correctly in any audio player
9. Verify tracklist contains track names with timestamps

**Step 3: Commit final state**

```bash
git add -A
git commit -m "feat(recording): complete set recording integration"
```

---

## Task 11: Update documentation (TODO.md, README.md, CHANGELOG.md)

**Files:**
- Modify: `TODO.md` — mark set recording as done
- Modify: `README.md` — add recording to features and roadmap
- Modify: `CHANGELOG.md` — add to 0.9.11

### TODO.md

Change:
```markdown
- [ ] Set recording master output.
```
To:
```markdown
- [x] Set recording master output.
```

### CHANGELOG.md

Add under `## [0.9.11]`:

```markdown
## [0.9.11]

### Added

- **Set recording** — Record the master output to WAV files directly on connected
  USB sticks. Toggle recording in Settings → Recording. A pulsing red indicator
  in the header shows elapsed time while recording. When recording stops, a
  companion tracklist TXT file is automatically generated from the session history,
  listing each track played with timestamps relative to the recording start.
  Recordings are saved to `mesh-recordings/` on each connected USB stick.
  The recording thread uses a lock-free ring buffer and never blocks the audio
  thread — zero impact on playback performance.
```

### README.md

In the "Live Performance" features section, add:

```markdown
- **Set recording** — Record the master output to WAV on connected USB sticks with automatic tracklist generation
```

In the roadmap "Coming Soon", move recording to "Working Now":

```markdown
- [x] Set recording with tracklist export
```

### Step: Commit

```bash
git add TODO.md README.md CHANGELOG.md
git commit -m "docs: document set recording feature in TODO, README, and CHANGELOG"
```

---

## Task 12: Remove per-deck Sync stubs

While implementing recording, also clean up the dead Sync code per user request.

**Files:**
- Modify: `crates/mesh-midi/src/messages.rs` — remove `Sync` from `DeckAction`
- Modify: `crates/mesh-midi/src/mapping.rs` — remove `"deck.sync"` action
- Modify: `crates/mesh-player/src/ui/deck_view.rs` — remove `Sync` from `DeckMessage`
- Modify: `crates/mesh-player/src/ui/app.rs` — remove `MidiDeckAction::Sync` match
- Modify: `crates/mesh-player/src/ui/handlers/deck_controls.rs` — remove `Sync` match arm
- Modify: `crates/mesh-midi/src/hid/devices/kontrol_f1.rs` — remove BTN_SYNC, sync button descriptor, LED mapping, test

Remove each `Sync` variant/reference, then:

```bash
cargo check -p mesh-player
cargo test -p mesh-midi
```

Fix any remaining references, then:

```bash
git add crates/mesh-midi/ crates/mesh-player/
git commit -m "refactor: remove per-deck sync stubs (sync is global-only)"
```
