//! RT-safe garbage collection for audio buffers
//!
//! This module provides a global `basedrop` collector that enables deferred
//! deallocation of large audio buffers. When a `Shared<T>` pointer is dropped
//! on the audio thread, it doesn't immediately free memory - instead it enqueues
//! the pointer for collection by a background GC thread.
//!
//! ## Why This Matters
//!
//! Memory deallocation involves system calls (munmap, madvise) that can take
//! 100ms+ for large buffers like `StemBuffers` (~450MB). This would cause
//! JACK xruns if done on the RT audio thread.
//!
//! With `basedrop::Shared<T>`:
//! - Drop on RT thread: ~50ns (just enqueues a pointer)
//! - Actual deallocation: happens on GC thread where latency doesn't matter
//!
//! ## Usage
//!
//! ```ignore
//! use basedrop::Shared;
//! use crate::engine::gc::gc_handle;
//!
//! // Create a Shared pointer (instead of Arc)
//! let data = Shared::new(&gc_handle(), StemBuffers::with_length(1000));
//!
//! // Clone works the same as Arc
//! let data2 = data.clone();
//!
//! // When dropped on any thread, deallocation is deferred to GC thread
//! drop(data);
//! drop(data2);  // Last reference - queued for GC, not freed immediately
//! ```

use basedrop::{Collector, Handle};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

/// Global handle for creating Shared<T> allocations
///
/// This is initialized once and can be cloned cheaply.
/// The actual Collector lives on a dedicated GC thread.
static GC_HANDLE: OnceLock<Handle> = OnceLock::new();

/// Initialize the global collector and return a handle
fn init_gc() -> Handle {
    // Channel to send the handle from GC thread to main thread
    let (tx, rx) = mpsc::channel();

    // Spawn GC thread that owns the Collector
    thread::Builder::new()
        .name("audio-gc".to_string())
        .spawn(move || {
            // Create collector on this thread (Collector is !Sync)
            let mut collector = Collector::new();

            // Send a handle to the main thread
            let handle = collector.handle();
            tx.send(handle).expect("Failed to send GC handle");

            log::info!("Audio GC thread started");

            // Run collection loop forever
            loop {
                // Collect all deferred drops
                collector.collect();

                // Sleep to avoid busy-waiting
                // 100ms is fast enough for memory reclamation
                thread::sleep(Duration::from_millis(100));
            }
        })
        .expect("Failed to spawn audio GC thread");

    // Wait for handle from GC thread
    rx.recv().expect("Failed to receive GC handle")
}

/// Get a handle for creating Shared<T> allocations
///
/// Call this when you need to wrap a value in `Shared<T>`.
/// The handle is lightweight and can be cloned.
///
/// ## Example
///
/// ```ignore
/// use basedrop::Shared;
/// use crate::engine::gc::gc_handle;
///
/// let stems = Shared::new(&gc_handle(), StemBuffers::with_length(1000));
/// ```
pub fn gc_handle() -> Handle {
    GC_HANDLE.get_or_init(init_gc).clone()
}
