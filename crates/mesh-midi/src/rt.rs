//! Real-time thread utilities for embedded builds (OrangePi 5 / RK3588S).
//!
//! Pins background threads to A76 big cores 4-7, keeping A55 LITTLE cores
//! free for RT audio and DSP. Duplicate of mesh_core::rt (avoids cross-crate dep).

/// Pin the current thread to A76 big cores 4-7.
///
/// Used for HID I/O and feedback evaluation threads to keep them off
/// the latency-sensitive A55 cores where JACK RT and rayon DSP run.
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
