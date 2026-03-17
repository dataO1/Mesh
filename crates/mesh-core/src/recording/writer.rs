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
