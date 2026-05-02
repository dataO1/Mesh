//! Mesh Cue Software - Track preparation for mesh DJ software
//!
//! This application provides tools for preparing tracks:
//!
//! 1. **Batch Import**: Import pre-separated stems from the import folder,
//!    run audio analysis (BPM, key, beat grid), and export to 8-channel WAV format.
//!
//! 2. **Collection Editor**: Browse converted tracks, edit cue points, adjust beat grid,
//!    and save changes back to the files.
//!
//! ## Architecture
//!
//! - **Domain Layer** (`domain/`): Business logic, services, and state management
//! - **UI Layer** (`ui/`): Display and user input handling only
//!
//! The UI layer delegates all business logic to the domain layer.

pub mod analysis;
pub mod audio;
pub mod batch_import;

/// Estimated peak resident-memory cost of one analysis worker, in bytes.
///
/// Each worker holds, concurrently, for a typical ~5-minute stereo FLAC at
/// 48 kHz:
///   * full-file decode buffer            ≈ 150 MB transient
///   * 4× stereo f32 stem buffers @ 48 k  ≈ 460 MB (5 min × 60 × 48 000 × 8 B × 4)
///   * mono mix for mel                   ≈  60 MB
///   * mel spectrogram + MAEST activations ≈ 100 MB (per-thread ORT session)
///
/// Sum ≈ 770 MB peak. Round to **1 GiB** for headroom against longer tracks
/// and allocator overhead. Used by `analysis_workers()` to cap concurrency
/// so we don't OOM on smaller boxes.
const PER_WORKER_PEAK_BYTES: u64 = 1024 * 1024 * 1024;

/// Fraction of currently-available RAM the analysis pool is allowed to plan
/// against. The rest is left for the OS, the rest of the app's heap, page
/// cache for FLAC reads, and slack against under-estimating per-worker peak.
const ANALYSIS_MEMORY_BUDGET_FRACTION: f64 = 0.70;

/// Number of parallel workers to use for batch analysis / import.
///
/// Combines two limits and takes the smaller:
///
/// 1. **CPU**: `std::thread::available_parallelism()` — logical CPUs honoring
///    cgroups, CPU affinity, and container quotas. Cross-platform std API.
/// 2. **RAM**: `sysinfo::System::available_memory()` × budget fraction
///    divided by `PER_WORKER_PEAK_BYTES`. `sysinfo` works on Linux, macOS,
///    Windows, and BSD via the same `available_memory()` accessor — verified
///    cross-platform.
///
/// Floors at 1 worker. The intent is "use every core if RAM allows, else
/// dial back so a 24-core / 8-GB box doesn't OOM analysing the same library
/// a 24-core / 64-GB workstation breezes through."
///
/// Called once per batch (cheap — one `System::new()` + `refresh_memory()`
/// is a single OS call on every supported platform).
pub fn analysis_workers() -> usize {
    use sysinfo::System;

    let cpu_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Probe RAM. If the platform doesn't expose available memory (returns 0),
    // fall back to the CPU count rather than capping to 0.
    let mut sys = System::new();
    sys.refresh_memory();
    let available = sys.available_memory(); // bytes; 0 if unsupported
    let memory_workers = if available == 0 {
        cpu_workers
    } else {
        let budget = (available as f64 * ANALYSIS_MEMORY_BUDGET_FRACTION) as u64;
        ((budget / PER_WORKER_PEAK_BYTES) as usize).max(1)
    };

    let workers = cpu_workers.min(memory_workers);
    log::info!(
        "analysis_workers: cpu={}, ram_avail={:.1} GiB → ram_cap={}, using {}",
        cpu_workers,
        available as f64 / (1024.0 * 1024.0 * 1024.0),
        memory_workers,
        workers,
    );
    workers
}
pub mod metadata;
pub mod config;
pub mod domain;
pub mod export;
pub mod features;
pub mod import;
pub mod keybindings;
pub mod loader;
pub mod ml_analysis;
pub mod pca;
pub mod reanalysis;
pub mod separation;
pub mod ui;
