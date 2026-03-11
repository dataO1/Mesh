//! Pre-allocated StemBuffer pool for zero-allocation track loading.
//!
//! On embedded systems with `mlockall(MCL_FUTURE)`, allocating 452MB of StemBuffers
//! triggers ~452K page faults that cause TLB shootdown IPIs across all cores, including
//! the RT audio core. This pool pre-allocates and pre-touches buffers at startup,
//! turning track loads from mmap+fault sequences into simple memcpy operations.
//!
//! # Design
//!
//! Cyclic checkout/checkin pool with automatic recycling. Buffers checked out for
//! loading are automatically returned when dropped (via `StemBuffers::drop()`).
//! The basedrop GC thread calls `Drop` on old `Shared<StemBuffers>` values, which
//! triggers the recycle path — no type changes needed throughout the engine.
//!
//! Only pool-originated buffers are recycled (identified by capacity == max_samples).
//! Fresh allocations, clones, and decode-region temporaries have smaller capacity
//! and are dropped normally.
//!
//! # Memory
//!
//! 4 buffers × 10 min @ 48kHz ≈ 3.5 GB. Feasible on 8GB+ devices.
//! With recycling, 4 buffers sustain unlimited track loads.

use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock};

use crate::audio_file::StemBuffers;

/// Global pool reference, set once at startup. Accessed by `StemBuffers::drop()`
/// for automatic recycling.
static GLOBAL_BUFFER_POOL: OnceLock<Arc<StemBufferPool>> = OnceLock::new();

thread_local! {
    /// Recursion guard: prevents re-entry when the pool drops an excess buffer
    /// during checkin (that buffer's Drop would try to recycle again).
    static IN_STEM_RECYCLE: Cell<bool> = const { Cell::new(false) };
}

/// Register the global buffer pool for automatic recycling via `StemBuffers::drop()`.
///
/// Must be called once at startup after creating the pool. Subsequent calls are ignored.
pub fn set_global_pool(pool: Arc<StemBufferPool>) {
    let _ = GLOBAL_BUFFER_POOL.set(pool);
}

/// Try to return a StemBuffers to the global pool (called from `StemBuffers::drop()`).
///
/// Returns `true` if the buffer was recycled, `false` if not (wrong capacity, no pool,
/// pool full, or re-entry guard active).
pub(crate) fn try_recycle_stems(buf: &mut StemBuffers) -> bool {
    // Prevent re-entry (when pool drops excess buffers during checkin)
    if IN_STEM_RECYCLE.with(|c| c.get()) {
        return false;
    }

    let pool = match GLOBAL_BUFFER_POOL.get() {
        Some(p) => p,
        None => return false,
    };

    // Only recycle pool-originated buffers (capacity matches pool allocation)
    if buf.vocals.capacity() < pool.max_samples {
        return false;
    }

    IN_STEM_RECYCLE.with(|c| c.set(true));

    // Extract the allocations before they'd be deallocated
    let vocals = std::mem::take(&mut buf.vocals);
    let drums = std::mem::take(&mut buf.drums);
    let bass = std::mem::take(&mut buf.bass);
    let other = std::mem::take(&mut buf.other);

    // Reconstruct and return to pool
    let mut recycled = StemBuffers::from_raw(vocals, drums, bass, other);
    recycled.restore_to_pool_size(pool.max_samples);

    if let Ok(mut available) = pool.available.lock() {
        log::info!("[POOL] Recycled buffer, {} available", available.len() + 1);
        available.push(recycled);
    }

    IN_STEM_RECYCLE.with(|c| c.set(false));
    true
}

/// Pool of pre-allocated StemBuffers for zero-allocation track loading.
///
/// Pre-allocates `count` buffers at construction time, each sized for `max_samples`
/// frames. Pages are force-touched with volatile writes so they're physically
/// resident in RAM (not just COW zero pages).
///
/// Buffers are automatically recycled when dropped via `StemBuffers::drop()`.
pub struct StemBufferPool {
    available: Mutex<Vec<StemBuffers>>,
    max_samples: usize,
}

impl StemBufferPool {
    /// Create a pool with `count` pre-allocated, pre-touched buffers.
    ///
    /// Each buffer can hold tracks up to `max_samples` in length.
    /// All pages are force-touched with volatile writes at construction time
    /// so they're physically resident in RAM (not COW zero pages).
    ///
    /// This is intentionally slow (~2-4 seconds per buffer) — call once at startup
    /// before any track loading begins.
    pub fn new(count: usize, max_samples: usize) -> Self {
        let mut available = Vec::with_capacity(count);
        for i in 0..count {
            let mb = (max_samples as f64 * 8.0 * 4.0) / 1_048_576.0;
            log::info!(
                "[POOL] Pre-allocating buffer {}/{} ({} samples, {:.0} MB)",
                i + 1, count, max_samples, mb,
            );
            let mut buf = StemBuffers::with_length(max_samples);

            // Force-touch every page so it's backed by a real physical page,
            // not a COW zero page. Without this, the first write to each page
            // during track loading triggers a page fault even though the buffer
            // was "pre-allocated".
            let touch_start = std::time::Instant::now();
            buf.pre_touch_pages();
            log::info!(
                "[POOL] Pre-touched buffer {}/{} in {:?}",
                i + 1, count, touch_start.elapsed(),
            );

            available.push(buf);
        }
        log::info!(
            "[POOL] Buffer pool ready: {} buffers, max {} samples ({:.1} min @ 48kHz, {:.1} GB total)",
            count, max_samples, max_samples as f64 / (48000.0 * 60.0),
            (count as f64 * max_samples as f64 * 32.0) / (1024.0 * 1024.0 * 1024.0),
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
    /// capacity is preserved so the buffer can be recycled later).
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

    /// Maximum samples per buffer in this pool.
    pub fn max_samples(&self) -> usize {
        self.max_samples
    }

    /// Number of buffers currently available.
    pub fn available_count(&self) -> usize {
        self.available.lock().map(|v| v.len()).unwrap_or(0)
    }
}
