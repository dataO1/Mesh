//! Pre-allocated StemBuffer pool for zero-allocation track loading.
//!
//! On embedded systems with `mlockall(MCL_FUTURE)`, allocating 452MB of StemBuffers
//! triggers ~452K page faults that cause TLB shootdown IPIs across all cores, including
//! the RT audio core. This pool pre-allocates and pre-touches buffers at startup,
//! turning track loads from mmap+fault sequences into simple memcpy operations.
//!
//! # Design
//!
//! One-shot pool: buffers are checked out but not returned. This handles the critical
//! startup case (4 simultaneous deck loads) without requiring type changes throughout
//! the engine. After pool exhaustion, loading falls back to normal allocation.
//!
//! # Memory
//!
//! 4 buffers × 10 min @ 48kHz ≈ 3.5 GB. Feasible on 8GB+ devices.

use std::sync::Mutex;

use crate::audio_file::StemBuffers;

/// Pool of pre-allocated StemBuffers for zero-allocation track loading.
///
/// Pre-allocates `count` buffers at construction time, each sized for `max_samples`
/// frames. When a loader thread needs a buffer, it calls `checkout()` which pops
/// a pre-touched buffer from the pool (no page faults). If the pool is empty or the
/// track exceeds `max_samples`, the caller falls back to normal allocation.
pub struct StemBufferPool {
    available: Mutex<Vec<StemBuffers>>,
    max_samples: usize,
}

impl StemBufferPool {
    /// Create a pool with `count` pre-allocated, pre-touched buffers.
    ///
    /// Each buffer can hold tracks up to `max_samples` in length.
    /// All pages are touched at construction time so they're resident in RAM.
    ///
    /// This is intentionally slow (~1-2 seconds per buffer) — call once at startup
    /// before any track loading begins.
    pub fn new(count: usize, max_samples: usize) -> Self {
        let mut available = Vec::with_capacity(count);
        for i in 0..count {
            let mb = (max_samples as f64 * 8.0 * 4.0) / 1_048_576.0;
            log::info!(
                "[POOL] Pre-allocating buffer {}/{} ({} samples, {:.0} MB)",
                i + 1, count, max_samples, mb,
            );
            let buf = StemBuffers::with_length(max_samples);
            available.push(buf);
        }
        log::info!(
            "[POOL] Buffer pool ready: {} buffers, max {} samples ({:.1} min @ 48kHz)",
            count, max_samples, max_samples as f64 / (48000.0 * 60.0),
        );
        Self {
            available: Mutex::new(available),
            max_samples,
        }
    }

    /// Take a pre-allocated buffer from the pool.
    ///
    /// Returns `Some(buffer)` if the pool has an available buffer large enough.
    /// Returns `None` if the pool is empty or the track exceeds `max_samples`
    /// (caller should fall back to `StemBuffers::with_length()`).
    ///
    /// The returned buffer is truncated to `needed_samples` (no deallocation,
    /// capacity is preserved).
    pub fn checkout(&self, needed_samples: usize) -> Option<StemBuffers> {
        if needed_samples > self.max_samples {
            log::info!(
                "[POOL] Track needs {} samples (max {}), falling back to allocation",
                needed_samples, self.max_samples,
            );
            return None;
        }
        let mut available = self.available.lock().ok()?;
        if let Some(mut buf) = available.pop() {
            let remaining = available.len();
            drop(available); // Release lock before truncation
            buf.truncate(needed_samples);
            log::info!(
                "[POOL] Checked out buffer ({} samples), {} remaining",
                needed_samples, remaining,
            );
            Some(buf)
        } else {
            log::info!("[POOL] Pool exhausted, falling back to allocation");
            None
        }
    }

    /// Number of buffers currently available.
    pub fn available_count(&self) -> usize {
        self.available.lock().map(|v| v.len()).unwrap_or(0)
    }
}
