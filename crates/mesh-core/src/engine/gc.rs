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
//! audio underruns if done on the RT audio thread.
//!
//! With `basedrop::Shared<T>`:
//! - Drop on RT thread: ~50ns (just enqueues a pointer)
//! - Actual deallocation: happens on GC thread where latency doesn't matter
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        Application Startup                          │
//! │                               │                                     │
//! │                    ┌──────────▼──────────┐                          │
//! │                    │   gc_handle()       │  First call initializes  │
//! │                    │   OnceLock<Handle>  │  the GC system           │
//! │                    └──────────┬──────────┘                          │
//! │                               │                                     │
//! │              ┌────────────────┴────────────────┐                    │
//! │              │                                 │                    │
//! │   ┌──────────▼──────────┐          ┌──────────▼──────────┐         │
//! │   │   audio-gc thread   │          │    Handle (Clone)   │         │
//! │   │   owns Collector    │◄─────────│  distributed to     │         │
//! │   │   runs collect()    │  mpsc    │  all threads        │         │
//! │   │   every 100ms       │          └─────────────────────┘         │
//! │   └─────────────────────┘                                          │
//! └─────────────────────────────────────────────────────────────────────┘
//!
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        Runtime Operation                            │
//! │                                                                     │
//! │   UI Thread                         Audio RT Thread                 │
//! │   ┌────────────────┐                ┌────────────────┐              │
//! │   │ Load new track │                │ Replace deck   │              │
//! │   │                │                │ track with new │              │
//! │   │ Shared::new()  │────────────────│                │              │
//! │   │ ~1µs           │   command      │ Old Shared     │              │
//! │   └────────────────┘   queue        │ dropped ~50ns  │              │
//! │                                     └───────┬────────┘              │
//! │                                             │ enqueue               │
//! │                                             ▼                       │
//! │                              ┌──────────────────────────┐           │
//! │                              │    audio-gc thread       │           │
//! │                              │    collector.collect()   │           │
//! │                              │    actual dealloc ~100ms │           │
//! │                              │    (doesn't block RT)    │           │
//! │                              └──────────────────────────┘           │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## When to Use `Shared<T>` vs `Arc<T>`
//!
//! | Type | Use Case | Drop Behavior |
//! |------|----------|---------------|
//! | `Shared<T>` | Large buffers (audio, images) that may be dropped on RT threads | Deferred to GC thread |
//! | `Arc<T>` | Small data, config, metadata not dropped on RT threads | Immediate deallocation |
//!
//! **Rule of thumb**: Use `Shared<T>` for any data that:
//! 1. Is large (>1MB)
//! 2. Might be dropped on the audio callback thread
//! 3. Would cause latency spikes if deallocated synchronously
//!
//! ## Performance Characteristics
//!
//! | Operation | Time | Notes |
//! |-----------|------|-------|
//! | `Shared::new()` | ~1µs | Same as `Arc::new()` + handle lookup |
//! | `Shared::clone()` | ~10ns | Atomic increment, same as `Arc` |
//! | `Shared::drop()` (not last ref) | ~10ns | Atomic decrement, same as `Arc` |
//! | `Shared::drop()` (last ref) | ~50ns | Enqueues pointer, no syscalls |
//! | `collector.collect()` | varies | Actual deallocation, 100ms+ for large buffers |
//!
//! ## Thread Safety
//!
//! - `Handle`: `Clone + Send + Sync` - can be shared across threads
//! - `Shared<T>`: `Clone + Send + Sync` (if T is) - same semantics as `Arc<T>`
//! - `Collector`: `!Sync` - must stay on one thread (the audio-gc thread)
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
//!
//! ## Caveats
//!
//! 1. **Memory is not freed immediately**: After dropping the last `Shared<T>`,
//!    memory stays allocated until the next GC cycle (up to 100ms).
//!
//! 2. **Debug trait**: `Shared<T>` does not implement `Debug`. Use wrapper types
//!    with manual `Debug` impls if needed (see `StemsLoadResult` in mesh-cue).
//!
//! 3. **GC thread runs forever**: The audio-gc thread is spawned once and never
//!    terminates. This is intentional for audio applications that run until exit.

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
