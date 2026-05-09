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
/// Mesh's converted library files are **8-channel multi-stem FLACs** (vocals,
/// drums, bass, other — each as a stereo pair) at native 48 kHz, not the
/// 2-channel stereo files an earlier estimate assumed. For a typical
/// ~5-minute multi-stem track each worker holds, concurrently:
///
///   * full-file decode buffer             ≈  300 MB transient
///   * 4× stereo f32 stem buffers @ 48 kHz ≈  460 MB
///     (5 min × 60 s × 48 000 × 4 B × 2 ch × 4 stems)
///   * mono mix for mel                    ≈   60 MB
///   * mel spectrogram + ML activations    ≈  100 MB
///   * per-thread MuQ-MuLan ONNX session   ≈  700 MB (weights + arena)
///
/// Naïve sum ≈ 1.6 GB, but on long tracks (8–10 min) the stem buffers
/// roughly double and we observed ~2.5–3 GB peaks before the eager-drop
/// fix in `reanalysis::reanalyze_metadata_track` (stems are now consumed
/// for energy ratios and dropped *before* the mel + ONNX peak so the audio
/// buffers and the ONNX session never coexist at full size).
///
/// With eager drop, **2 GiB** is a realistic upper bound that still lets a
/// 70 GB workstation run all 24 cores without paging while leaving plenty
/// of headroom for the OS, page cache, and the rest of the app.
const PER_WORKER_PEAK_BYTES: u64 = 2 * 1024 * 1024 * 1024;

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
