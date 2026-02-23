//! System resource monitoring — CPU%, GPU%, RAM usage.
//!
//! Provides a [`ResourceMonitor`] struct that polls system metrics at a
//! configurable interval. GPU monitoring is Linux-only, probing sysfs
//! paths for Mali (devfreq) and AMD (drm) at startup.
//!
//! Usage:
//! ```ignore
//! let mut monitor = ResourceMonitor::new();
//! // Call every ~500ms (sysinfo needs >=200ms between CPU refreshes)
//! monitor.refresh();
//! println!("CPU: {:.0}%  GPU: {:?}%  RAM: {:.1}/{:.1}G",
//!     monitor.cpu_percent, monitor.gpu_percent,
//!     monitor.ram_used_gb, monitor.ram_total_gb);
//! ```

use std::path::PathBuf;

/// System resource readings, updated by [`ResourceMonitor::refresh`].
pub struct ResourceMonitor {
    sys: sysinfo::System,
    gpu_path: Option<GpuSource>,
    /// Overall CPU usage percentage (0.0–100.0). Needs two refresh cycles to stabilize.
    pub cpu_percent: f32,
    /// RAM in use (GiB).
    pub ram_used_gb: f32,
    /// Total system RAM (GiB).
    pub ram_total_gb: f32,
    /// GPU utilization percentage (0–100). `None` when no sensor is detected.
    pub gpu_percent: Option<u32>,
}

/// Detected GPU sensor backend.
enum GpuSource {
    /// Mali via devfreq: `/sys/class/devfreq/<device>/load`
    /// Format: `<load>@<freq>Hz` — parse integer before `@`.
    Devfreq(PathBuf),
    /// AMD via DRM sysfs: `/sys/class/drm/card<N>/device/gpu_busy_percent`
    /// Format: plain integer 0–100.
    AmdDrm(PathBuf),
}

const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;

impl ResourceMonitor {
    /// Create a new monitor, probing for GPU sensors.
    ///
    /// The first `cpu_percent` reading will be 0 — call [`refresh`] after
    /// >=200ms to get an accurate delta.
    pub fn new() -> Self {
        let mut sys = sysinfo::System::new();
        // Prime the CPU delta (first call records baseline, second gives real %)
        sys.refresh_cpu_usage();

        let gpu_path = probe_gpu();

        // Read initial RAM (available immediately, no delta needed)
        sys.refresh_memory();
        let ram_total_gb = (sys.total_memory() as f64 / BYTES_PER_GIB) as f32;
        let ram_used_gb = (sys.used_memory() as f64 / BYTES_PER_GIB) as f32;

        Self {
            sys,
            gpu_path,
            cpu_percent: 0.0,
            ram_used_gb,
            ram_total_gb,
            gpu_percent: None,
        }
    }

    /// Poll all sensors. Call every ~500ms for smooth updates.
    pub fn refresh(&mut self) {
        // CPU (delta-based — needs >=200ms since last refresh)
        self.sys.refresh_cpu_usage();
        self.cpu_percent = self.sys.global_cpu_usage();

        // RAM
        self.sys.refresh_memory();
        self.ram_used_gb = (self.sys.used_memory() as f64 / BYTES_PER_GIB) as f32;
        self.ram_total_gb = (self.sys.total_memory() as f64 / BYTES_PER_GIB) as f32;

        // GPU
        self.gpu_percent = self.gpu_path.as_ref().and_then(read_gpu);
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// GPU sensor probing (Linux sysfs)
// =============================================================================

#[cfg(target_os = "linux")]
fn probe_gpu() -> Option<GpuSource> {
    // 1. Mali via devfreq (RK3588 / Orange Pi 5)
    for path in &[
        "/sys/class/devfreq/fb000000.gpu/load",
        "/sys/class/devfreq/fb000000.gpu-panthor/load",
    ] {
        let p = PathBuf::from(path);
        if p.exists() {
            log::info!("GPU monitor: Mali devfreq at {}", path);
            return Some(GpuSource::Devfreq(p));
        }
    }

    // 2. AMD via DRM sysfs
    if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let gpu_busy = entry.path().join("device/gpu_busy_percent");
            if gpu_busy.exists() {
                log::info!("GPU monitor: AMD DRM at {}", gpu_busy.display());
                return Some(GpuSource::AmdDrm(gpu_busy));
            }
        }
    }

    log::info!("GPU monitor: no sensor found");
    None
}

#[cfg(not(target_os = "linux"))]
fn probe_gpu() -> Option<GpuSource> {
    log::info!("GPU monitor: not available on this platform");
    None
}

/// Read current GPU utilization from a probed sensor.
fn read_gpu(source: &GpuSource) -> Option<u32> {
    match source {
        GpuSource::Devfreq(path) => {
            // Format: "28@300000000Hz"
            let content = std::fs::read_to_string(path).ok()?;
            content.split('@').next()?.trim().parse().ok()
        }
        GpuSource::AmdDrm(path) => {
            // Format: plain "45\n"
            let content = std::fs::read_to_string(path).ok()?;
            content.trim().parse().ok()
        }
    }
}
