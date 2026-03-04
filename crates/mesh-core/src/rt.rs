//! Real-time thread utilities for embedded builds (OrangePi 5 / RK3588S).
//!
//! Provides CPU affinity helpers for the big.LITTLE architecture:
//! - A55 cores 0-3 (LITTLE): reserved for RT audio + rayon DSP
//! - A76 cores 4-7 (big): background work (loaders, DB, HID, GC)
//!
//! All functions are no-ops when the `embedded-rt` feature is disabled.

/// Pin the current thread to A76 big cores 4-7.
///
/// Used for background/IO threads (track loading, preset building, GC,
/// DB writes, HID I/O) to keep them off the latency-sensitive A55 cores
/// where JACK RT and rayon DSP workers run.
///
/// On non-embedded builds this is a no-op.
#[cfg(feature = "embedded-rt")]
pub fn pin_to_big_cores() {
    unsafe {
        let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut cpuset);
        libc::CPU_SET(4, &mut cpuset);
        libc::CPU_SET(5, &mut cpuset);
        libc::CPU_SET(6, &mut cpuset);
        libc::CPU_SET(7, &mut cpuset);
        let ret = libc::sched_setaffinity(
            0,
            std::mem::size_of::<libc::cpu_set_t>(),
            &cpuset,
        );
        if ret != 0 {
            log::warn!(
                "[RT] pin_to_big_cores failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// No-op on non-embedded builds.
#[cfg(not(feature = "embedded-rt"))]
#[inline]
pub fn pin_to_big_cores() {}
