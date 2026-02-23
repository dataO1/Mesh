# Real-Time Audio Optimizations for Mesh on OrangePi 5 (RK3588)

Comprehensive optimization guide for interruption-free real-time audio processing on the
OrangePi 5 embedded NixOS image. All optimizations are build-target dependent -- applied only
in the CI-built embedded image, not development builds.

**Target hardware**: OrangePi 5, RK3588S SoC (4x Cortex-A76 @ 2.4GHz + 4x Cortex-A55 @ 1.8GHz)
**Audio pipeline**: PipeWire (JACK compat) -> mesh-player JACK client -> 4 decks x 4 stems -> multiband effects -> mixer -> I2S DACs
**Current buffer**: 256 samples @ 48kHz = 5.33ms per period

---

## Table of Contents

1. [CPU Core Pinning / Affinity](#1-cpu-core-pinning--affinity)
2. [Real-Time Scheduling](#2-real-time-scheduling)
3. [IRQ Affinity](#3-irq-affinity)
4. [Memory Management](#4-memory-management)
5. [Kernel Tuning](#5-kernel-tuning)
6. [Audio Subsystem](#6-audio-subsystem)
7. [Power Management](#7-power-management)
8. [Process Isolation](#8-process-isolation)
9. [Filesystem / I/O](#9-filesystem--io)
10. [RK3588-Specific Considerations](#10-rk3588-specific-considerations)
11. [Implementation Priority Matrix](#11-implementation-priority-matrix)

---

## 1. CPU Core Pinning / Affinity

### 1.1 Which Cores for Audio?

**Recommendation: Pin audio RT threads to A55 LITTLE cores (CPUs 0-3).
Pin background heavy-lifting to A76 big cores (CPUs 4-7).**

This is the *opposite* of the naive assumption that "audio needs the fastest cores." The
reasoning is based on measured workload analysis of the actual audio callback:

#### Audio Callback Is Lightweight

The JACK RT callback processes pre-decoded PCM buffers through effects and mixing. The actual
computational cost per 256-sample buffer (5.33ms budget):

| Workload | Desktop (x86) | A55 (2 cores, estimated) | Budget used (A55) |
|----------|:-:|:-:|:-:|
| Minimal effects (passthrough + time stretch) | ~200-250 us | ~600-750 us | 11-14% |
| Moderate effects (1-2 plugins per stem) | ~400-500 us | ~1,200-1,500 us | 22-28% |
| Heavy effects (3+ plugins per stem) | ~800-1200 us | ~2,400-3,600 us | 45-68% |

Desktop measurements were taken on x86 with 4 rayon cores. A55 scaling factor is ~3x (2.5-3x
per-core throughput ratio + 2-batch overhead from 2 cores instead of 4). The time stretcher
(signalsmith FFT) is the dominant cost: ~300-600µs per deck on A55, accounting for 40-50% of
total callback time with 4 decks playing.

The working set is tiny: 256 samples x 4 stems x 8 bytes = 8KB per deck, 32KB total --
**fits entirely in the A55's 32KB L1 D-cache** (2-cycle access latency).

#### A55 Has More Deterministic Timing

The A55's in-order pipeline provides tighter worst-case execution time (WCET) bounds:
- **A55 (in-order)**: WCET is 1.2-1.5x average case. No reorder buffer, no speculative
  execution, no branch misprediction flush beyond 7 cycles. Stall duration is bounded.
- **A76 (out-of-order)**: WCET can be 2-5x average case due to speculative execution
  resource conflicts, branch misprediction storms (18+ cycles), and cache pressure from
  other cores sharing the L2.

For RT audio, **predictably fast beats unpredictably faster**. A consistent 400us callback
is better than one that averages 200us but occasionally spikes to 2ms.

#### Background Work Is the Real Heavy Lifting

These CPU-intensive tasks benefit directly from the A76's 2.7x FLOP advantage:

| Background Task | Typical Duration | A76 Benefit |
|----------------|:-:|:-:|
| FLAC 8-channel decode | 2-5 seconds | 2.7x faster I/O + compute |
| Linked stem pre-stretch | 500ms-2s | FFT-heavy, benefits from 2x NEON pipes |
| Peak generation (highres) | 100-500ms | Throughput-bound |
| ML inference (beat/genre) | 1-10 seconds | Matrix ops scale with FLOPS |

#### A55 vs A76 Specifications

| Metric | A55 @ 1.8GHz | A76 @ 2.4GHz | Ratio |
|--------|:-:|:-:|:-:|
| Peak SP GFLOPS | 14.4 | 38.4 | 2.7x |
| NEON pipes | 1x 128-bit | 2x 128-bit | 2x |
| FMLA throughput | 1/cycle | 2/cycle | 2x |
| L1 D-cache | 32KB / 2 cycles | 64KB / 4 cycles | Smaller but faster |
| L2 cache | 64-256KB / 8 cycles | 256-512KB / 9 cycles | Larger |
| Issue width | 2-wide (in-order) | 4-wide (OoO) | 2x |
| WCET predictability | High | Low | A55 wins |

On the RK3588S, the CPU topology is:
```
CPU 0-3: Cortex-A55 (LITTLE cluster) -- audio RT
CPU 4-7: Cortex-A76 (big cluster)    -- background (loading, stretching, peaks)
```

### 1.2 Current State

The cage systemd service currently pins to big cores:
```nix
# nix/embedded/kiosk.nix line 102-108
systemd.services."cage-tty1" = {
  serviceConfig = {
    CPUAffinity = "4-7";  # A76 big cores
  };
};
```

This puts everything (audio, UI, background) on the same 4 A76 cores. The audio callback
competes with track loading and UI rendering for the same cores.

### 1.3 Fine-Grained Pinning Strategy

The ideal layout for the embedded image (mesh-player only, no mesh-cue/ML):

| Thread(s) | CPU Cores | Rationale |
|-----------|-----------|-----------|
| PipeWire/JACK RT callback | CPU 0 (A55) | Dedicated core; single thread drives all 4 decks sequentially, dispatches stems to rayon |
| Rayon audio workers (4 threads) | CPU 2-3 (A55) | Parallel stem processing; 2 cores handle 4 stems in 2 batches (~50µs overhead, negligible) |
| iced UI / Wayland / cage | CPU 1 (A55) | GPU-accelerated rendering at 120Hz; CPU work is ~0.3-1.5ms/frame (see analysis below) |
| Track loading / linked stem pre-stretch / peaks | CPU 4-7 (A76) | CPU-heavy FLAC decode, FFT stretching, peak generation — benefits from 2.7x FLOPS |
| PresetLoader / USB sync | CPU 4-7 (A76) | Background I/O + plugin construction |
| System services (NetworkManager, journald) | CPU 4-7 (A76) | Keep off A55 audio cluster entirely |

**Why 1 core for JACK RT is sufficient**: The JACK process callback is a single thread that
calls `AudioEngine::process()`. This loops over 4 decks *sequentially*, calling `deck.process()`
(which fans out to rayon for parallel stem processing) then `stretcher.process()` (sequential,
~100-200µs per deck). The 2 JACK output ports (master + cue) are just `memcpy` at the end.
The JACK thread never needs more than 1 core — the parallel work happens on the rayon pool.

**Why 2 rayon cores is enough**: Each stem's workload is ~20-50µs (buffer read + effect chain).
With 2 cores, rayon processes 4 stems in 2 batches instead of 4 parallel — adding ~50µs per
deck. Total overhead across 4 decks: ~200µs. The 5,330µs budget absorbs this easily.

Adding a 3rd core (e.g., moving UI to A76) provides **only 5-15% improvement** because 4 stems
still require 2 batches regardless of whether there are 2 or 3 cores:

```
2 cores: [stem 0+1] → [stem 2+3]    = 2 batches
3 cores: [stem 0+1+2] → [stem 3]    = still 2 batches (4th task doesn't fit in batch 1)
4 cores: [stem 0+1+2+3]             = 1 batch (only possible with 4+ cores)
```

Rayon scheduling overhead on A55 is negligible (~30µs total, 0.6% of budget). The in-order
pipeline makes work-stealing more predictable: no speculative buffer flushes on atomics,
7-cycle branch misprediction (vs A76's 15+), and same-cluster L2 sharing means cache
coherency between rayon threads is fast (~8 cycles).

**Headroom estimates (2 A55 cores, 4 rayon threads)**:

| Scenario | Estimated Time (A55) | Budget (5,330µs) | Headroom |
|----------|:--------------------:|:-----------------:|:--------:|
| 2 decks, light FX | ~1,500 us | 5,330 us | 72% |
| 4 decks, moderate FX (1-2 plugins/stem) | ~3,200 us | 5,330 us | 40% |
| 4 decks, heavy FX (3+ plugins/stem) | ~4,600 us | 5,330 us | 14% |

The time stretcher is the dominant cost (signalsmith FFT, ~300-600µs/deck on A55). With 4
decks, time stretching alone consumes 1,200-2,400µs (23-45% of budget). This runs sequentially
per deck and is not affected by the number of rayon cores.

**Why A55 is sufficient for 120Hz UI at 2880x864**: The iced UI is GPU-accelerated via wgpu
(Vulkan on Mali-G610 MP4). The CPU's per-frame work is strictly bounded:

| Phase | A55 Estimated | Notes |
|-------|:-------------:|-------|
| Tick handler (atomic reads, field writes) | 0.1-0.5ms | Zero allocations, lock-free |
| iced layout/diff (reactive rendering) | 0.05-0.2ms | Performance mode: ~20 widgets |
| Uniform buffer preparation | 0.1-0.5ms | ~2KB total (8 waveforms × 416B + knobs) |
| Command recording + Vulkan submit | 0.1-0.35ms | ~10 draw calls, batched by primitive type |
| Cage compositor | 0-0.1ms | Direct scanout likely (single fullscreen client) |
| **Total CPU per frame** | **0.3-1.5ms** | **4-18% of 8.33ms budget at 120Hz** |

Key factors that make this cheap:
- **Waveforms are 100% GPU-rendered** via WGSL fragment shaders. The old Canvas/lyon CPU
  tessellation path is fully deprecated. Peak data is uploaded once at track load (via
  `Arc<PeakBuffer>`), not per frame — pointer comparison (`Arc::as_ptr()`) detects changes.
- **GPU fill rate is massively overprovisioned**: 2880×864 @ 120Hz = ~300 Mpixels/s. The
  Mali-G610 MP4 pushes ~8 Gpixels/s theoretical = 3.7% utilization. Even with 3x overdraw,
  only ~11%.
- **Cage direct scanout**: With a single maximized client at native resolution, wlroots can
  bypass compositing entirely and DMA-flip the client buffer directly to the display controller.
- **No VU meters or continuous CPU animation**: The only per-frame animated element is the
  playhead position (4 atomic reads + uniform update = 8 bytes per deck).

Moving UI to an A76 core would waste 2.7x the compute power for a workload that uses <18% of
an A55. That A76 core is far better utilized for track loading (FLAC decode benefits directly
from out-of-order execution and 2x NEON pipes).

### 1.4 Application-Level Implementation (Rust Code)

#### a) Rayon Thread Pool Affinity

The rayon global thread pool is initialized in `crates/mesh-player/src/main.rs`:

```rust
// Current code (line 57-61)
rayon::ThreadPoolBuilder::new()
    .num_threads(4)
    .thread_name(|i| format!("rayon-audio-{}", i))
    .build_global()
    .expect("Failed to initialize Rayon thread pool");
```

Add CPU affinity to rayon worker threads using the `start_handler` callback:

```rust
// Proposed change for embedded builds
rayon::ThreadPoolBuilder::new()
    .num_threads(4)
    .thread_name(|i| format!("rayon-audio-{}", i))
    .start_handler(|_thread_idx| {
        // Pin rayon workers to A55 cores 2-3 (core 0 = JACK RT, core 1 = UI)
        // 2 cores for 4 threads: rayon work-steals, processing stems in 2 batches.
        // Only on embedded builds -- dev builds use all cores.
        #[cfg(feature = "embedded-rt")]
        {
            let mut cpuset: libc::cpu_set_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::CPU_ZERO(&mut cpuset);
                libc::CPU_SET(2, &mut cpuset);
                libc::CPU_SET(3, &mut cpuset);
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
            }
        }
    })
    .build_global()
    .expect("Failed to initialize Rayon thread pool");
```

**Dependencies**: The `libc` crate is already in mesh-core's dependencies. Add `libc` to
mesh-player's Cargo.toml, or expose a helper function from mesh-core.

#### b) JACK/PipeWire RT Thread Affinity

The JACK RT callback thread is created by PipeWire's JACK compatibility layer, not by
mesh-player. PipeWire itself manages RT thread creation. To pin it:

**Option 1 (recommended)**: Use PipeWire's built-in affinity support:
```
# pipewire.conf context.properties
context.properties = {
    cpu.affinity = [ 0 ]  # Pin data thread to CPU 0 (A55, dedicated for RT)
}
```

**Option 2**: Pin from within the JACK process callback on first invocation:
```rust
// In JackProcessor::process() -- only runs once
if !self.affinity_set {
    #[cfg(feature = "embedded-rt")]
    {
        let mut cpuset: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::CPU_ZERO(&mut cpuset);
            libc::CPU_SET(0, &mut cpuset);  // Dedicated A55 core (deterministic in-order)
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
        }
    }
    self.affinity_set = true;
}
```

### 1.5 NixOS Configuration

```nix
# nix/embedded/hardware.nix -- refine cage-tty1 affinity
# Allow all cores so mesh-player can set per-thread affinity internally:
# Audio RT + UI → A55 (0-3), background → A76 (4-7)
systemd.services."cage-tty1" = {
  serviceConfig = {
    CPUAffinity = "0-7";  # Allow all cores, let app manage internally
  };
};

# Pin system services to A76 cores (keep them off the A55 audio cluster)
systemd.services.NetworkManager.serviceConfig.CPUAffinity = "4-7";
systemd.services.systemd-journald.serviceConfig.CPUAffinity = "4-7";
systemd.services.systemd-udevd.serviceConfig.CPUAffinity = "4-7";
```

### 1.6 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low |
| **Complexity** | Medium (application + NixOS changes) |
| **Expected Impact** | High -- eliminates core contention between audio RT and UI/IO |
| **Layer** | Application code + OS configuration |

---

## 2. Real-Time Scheduling

### 2.1 SCHED_FIFO vs SCHED_RR

**Recommendation: SCHED_FIFO for the JACK RT callback thread.**

- `SCHED_FIFO`: Thread runs until it blocks or a higher-priority FIFO thread becomes runnable. Perfect for the audio callback which runs for a short burst each period then blocks until the next JACK cycle.
- `SCHED_RR`: Round-robin among same-priority threads. Unnecessary overhead for the audio path where there is one primary RT thread.

**Priority levels**:

| Thread | Policy | Priority | Rationale |
|--------|--------|----------|-----------|
| PipeWire data thread | SCHED_FIFO | 88 | Drives the audio graph |
| JACK RT callback (mesh-player) | SCHED_FIFO | 80 | Inherits from PipeWire JACK |
| Rayon audio workers | SCHED_FIFO | 70 | Stem processing (called from RT context) |
| iced UI thread | SCHED_OTHER | nice -5 | Responsive but not RT |
| Track loader / I/O | SCHED_OTHER | nice 0 | Background, best-effort |

### 2.2 Current State

PipeWire already handles RT scheduling for the JACK callback thread via rtkit (enabled in
`nix/embedded/audio.nix` line 12). PAM limits allow rtprio 99 for the audio group.

The rayon worker threads do NOT currently have RT priority -- they inherit the default
SCHED_OTHER policy. This is a significant gap: when the JACK RT thread calls into
`deck.process()` which dispatches to rayon's `par_iter_mut()`, the parallel workers may be
preempted by non-RT tasks, causing xruns.

### 2.3 Application-Level Implementation

#### a) Rayon Workers with RT Priority

```rust
// In the rayon start_handler (combined with affinity from section 1)
.start_handler(|_thread_idx| {
    #[cfg(feature = "embedded-rt")]
    {
        // Set CPU affinity (as above)
        // ...

        // Set SCHED_FIFO priority 70 for rayon audio workers
        unsafe {
            let param = libc::sched_param {
                sched_priority: 70,
            };
            let ret = libc::sched_setscheduler(0, libc::SCHED_FIFO, &param);
            if ret != 0 {
                eprintln!("Warning: could not set RT priority for rayon worker: {}",
                    std::io::Error::last_os_error());
            }
        }
    }
})
```

**Critical note**: `sched_setscheduler` requires `CAP_SYS_NICE` or rtprio PAM limits. The
mesh user is in the audio group with rtprio=99, so this will work if the process has the
correct rlimits.

#### b) Verify RT Limits at Startup

Add a startup check to mesh-player:

```rust
#[cfg(feature = "embedded-rt")]
fn verify_rt_capabilities() {
    unsafe {
        let mut rlim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_RTPRIO, &mut rlim) == 0 {
            log::info!("RLIMIT_RTPRIO: soft={}, hard={}", rlim.rlim_cur, rlim.rlim_max);
            if rlim.rlim_cur < 80 {
                log::warn!("RT priority limit too low for audio threads!");
            }
        }
    }
}
```

### 2.4 PREEMPT_RT Kernel

The nixos-rk3588 module provides a vendor kernel (5.10.x or 6.1.x). A full PREEMPT_RT
patched kernel has been successfully tested on RK3588 (5.10.209-rt89) with sub-2us
cyclictest latency.

**For the embedded image**: The nixos-rk3588 vendor kernel likely uses `CONFIG_PREEMPT`
(voluntary preemption) by default. Upgrading to `CONFIG_PREEMPT_RT` would give the best
worst-case latency but requires:

1. Matching RT patch version to the vendor kernel
2. Verifying all vendor drivers (I2S, DMA, GPU) are compatible with PREEMPT_RT
3. Custom kernel build in the Nix flake

**Recommendation**: Start without PREEMPT_RT. The `threadirqs` boot parameter (already set
in `hardware.nix` line 44) forces threaded IRQ handlers, which gives most of the benefit on
a non-RT kernel. Evaluate PREEMPT_RT only if xruns persist after all other optimizations.

### 2.5 NixOS Configuration

```nix
# nix/embedded/audio.nix -- already correct, verify these remain
security.rtkit.enable = true;
security.pam.loginLimits = [
  { domain = "@audio"; type = "-"; item = "memlock"; value = "unlimited"; }
  { domain = "@audio"; type = "-"; item = "rtprio";  value = "99"; }
  { domain = "@audio"; type = "-"; item = "nice";    value = "-19"; }
];

# nix/embedded/hardware.nix -- add kernel.sched_rt_runtime_us
boot.kernel.sysctl = {
  "kernel.sched_rt_runtime_us" = -1;  # Allow RT threads 100% CPU (no 95% limit)
  # Default is 950000 (95%), which can cause RT threads to be throttled
};
```

### 2.6 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Medium (RT priority bugs can hang the system) |
| **Complexity** | Medium |
| **Expected Impact** | High -- prevents rayon workers from being preempted during audio callback |
| **Layer** | Application code + OS configuration |

---

## 3. IRQ Affinity

### 3.1 What and Why

Hardware interrupts (IRQs) cause immediate context switches. If an unrelated IRQ (USB,
network, GPU) fires on a core running the audio callback, it adds latency jitter. By
steering audio IRQs to the audio cores and everything else away, we reduce worst-case
latency.

### 3.2 RK3588 Audio IRQs

The audio path uses:
- **I2S3 IRQ** (PCM5102A DAC on GPIO) -- master output
- **ES8388 I2S IRQ** -- headphone/cue output
- **DMA controller IRQs** for both I2S channels

These IRQs should be pinned to CPU 0 (the dedicated A55 audio core).

### 3.3 Implementation

#### a) Disable irqbalance

irqbalance dynamically reassigns IRQs across cores, which conflicts with manual pinning.

```nix
# nix/embedded/hardware.nix
services.irqbalance.enable = false;
```

#### b) Pin IRQs via udev/systemd

Create a systemd service that runs after audio initialization:

```nix
# nix/embedded/audio.nix
systemd.services.mesh-irq-affinity = {
  description = "Pin audio IRQs to A55 audio core, move others to A76";
  after = [ "sound.target" "mesh-audio-init.service" ];
  wantedBy = [ "multi-user.target" ];
  serviceConfig = {
    Type = "oneshot";
    ExecStart = pkgs.writeShellScript "mesh-irq-affinity" ''
      # Pin audio-related IRQs to CPU 0 (A55, dedicated audio core)
      # IRQ numbers vary by DT -- find them dynamically
      for irqdir in /proc/irq/*/; do
        irq=$(basename "$irqdir")
        [ "$irq" = "default_smp_affinity" ] && continue

        # Read the IRQ action name
        actions=$(cat "$irqdir/actions" 2>/dev/null || echo "")

        case "$actions" in
          *i2s*|*es8388*|*rockchip-i2s*|*dma*audio*)
            # Audio IRQs -> CPU 0 (A55, dedicated audio core; bitmask 0x01)
            echo 01 > "$irqdir/smp_affinity" 2>/dev/null
            echo "Pinned IRQ $irq ($actions) to CPU 0 (A55)"
            ;;
          *)
            # Everything else -> A76 cores 4-7 (bitmask 0xf0)
            echo f0 > "$irqdir/smp_affinity" 2>/dev/null
            ;;
        esac
      done

      # Default affinity for new IRQs -> A76 cores (keep off audio cluster)
      echo f0 > /proc/irq/default_smp_affinity
    '';
  };
};
```

#### c) Kernel Parameter

```nix
# nix/embedded/hardware.nix -- add to boot.kernelParams
"irqaffinity=4-7"  # Default IRQ affinity to A76 cores (keep off A55 audio cluster)
```

### 3.4 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low |
| **Complexity** | Low |
| **Expected Impact** | Medium -- reduces jitter from non-audio IRQ contention |
| **Layer** | OS configuration |

---

## 4. Memory Management

### 4.1 mlockall -- Prevent Page Faults

Page faults in the audio callback are catastrophic: a single major fault (loading from disk)
takes 1-10ms, far exceeding the 5.33ms buffer period. `mlockall(MCL_CURRENT | MCL_FUTURE)`
locks all current and future memory mappings into physical RAM.

#### Application-Level Implementation

```rust
// In mesh-player main.rs, early in main() before any threads
#[cfg(feature = "embedded-rt")]
{
    unsafe {
        let ret = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
        if ret == 0 {
            log::info!("Memory locked: mlockall(MCL_CURRENT | MCL_FUTURE) succeeded");
        } else {
            log::warn!("mlockall failed: {} -- page faults may cause xruns",
                std::io::Error::last_os_error());
        }
    }
}
```

**Memory impact**: mesh-player's RSS is approximately 200-400MB (track audio data, waveform
peaks, effect buffers). The OrangePi 5 has 4-16GB RAM depending on variant, so this is
well within budget.

**PAM prerequisite**: `memlock=unlimited` is already set for the audio group in
`nix/embedded/audio.nix`.

### 4.2 Stack Pre-Faulting

After `mlockall(MCL_FUTURE)`, new stack pages are locked on first touch. However, the first
touch still causes a minor fault (kernel allocates the page). Pre-fault rayon worker stacks:

```rust
// In rayon start_handler, after setting affinity and RT priority
{
    // Pre-fault 2MB of stack to avoid minor faults during audio processing
    let stack_size = 2 * 1024 * 1024; // 2MB
    let buf = vec![0u8; stack_size];
    std::hint::black_box(&buf); // Prevent optimization
}
```

### 4.3 Disable Transparent Huge Pages (THP)

THP can cause latency spikes when the kernel compacts memory to form huge pages. The
compaction runs in the background but can steal CPU time and cause page table lock contention.

```nix
# nix/embedded/hardware.nix
boot.kernelParams = [
  # ... existing params ...
  "transparent_hugepage=never"
];
```

### 4.4 Huge Pages for Audio Buffers (Optional)

Static huge pages (2MB) can reduce TLB misses for large audio buffers. However, the
per-stem buffers are only 8192 samples x 8 bytes = 64KB each, which fits in regular pages.
The main benefit would be for track audio data (~50MB per track x 4 decks = ~200MB).

**Not recommended initially** -- the complexity of managing a hugetlbfs pool outweighs the
marginal benefit. TLB misses are unlikely to be the bottleneck given the audio buffer sizes.

### 4.5 NUMA Considerations

The RK3588 has a single memory controller with unified address space. There are no NUMA
nodes to worry about.

### 4.6 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low (mlockall), Medium (THP disable may affect other workloads) |
| **Complexity** | Low |
| **Expected Impact** | High -- eliminates the single most common cause of audio glitches |
| **Layer** | Application code (mlockall) + OS configuration (THP) |

---

## 5. Kernel Tuning

### 5.1 Scheduler Parameters

```nix
# nix/embedded/hardware.nix
boot.kernel.sysctl = {
  # Allow RT threads to use 100% of CPU time (default 95% with 5% safety margin)
  # Without this, RT threads are throttled after 950ms out of every 1000ms
  "kernel.sched_rt_runtime_us" = -1;

  # Reduce CFS scheduler latency for better responsiveness of non-RT threads
  # Default: 6ms for latency, 0.75ms for granularity
  "kernel.sched_latency_ns" = 4000000;         # 4ms (from 6ms)
  "kernel.sched_min_granularity_ns" = 500000;   # 0.5ms (from 0.75ms)

  # Existing: reduce swappiness further (currently 10, go to 1)
  "vm.swappiness" = 1;  # Only swap when absolutely necessary

  # Disable kernel printk to avoid console I/O in audio path
  "kernel.printk" = "0 0 0 0";  # Already set
};
```

### 5.2 Kernel Boot Parameters

```nix
# nix/embedded/hardware.nix -- enhanced boot.kernelParams
boot.kernelParams = [
  # Existing
  "quiet" "loglevel=0"
  "systemd.show_status=false" "rd.systemd.show_status=false"
  "udev.log_level=3" "rd.udev.log_level=3"
  "vt.global_cursor_default=0" "logo.nologo"
  "threadirqs"                        # Already present: force threaded IRQ handlers

  # New: real-time audio optimizations
  "transparent_hugepage=never"        # Disable THP compaction
  "irqaffinity=4-7"                   # Default IRQs to A76 cores (keep off A55 audio cluster)
  "rcu_nocbs=0-3"                     # Offload RCU callbacks from A55 audio cores
  "rcu_nocb_poll"                     # Kthread polls for RCU instead of IPI wakeup
  "nohz_full=0-3"                     # Tickless operation on A55 audio cores (when single task)
  "skew_tick=1"                       # Offset timer ticks across cores (reduce lock contention)
  "tsc=reliable"                      # (x86 only, harmless on ARM -- kernel ignores)
  "nosoftlockup"                      # Disable soft lockup detector (avoids NMI-like jitter)
  "nowatchdog"                        # Disable watchdog timer on audio cores
];
```

**Key additions explained**:

- **`rcu_nocbs=0-3`**: RCU (Read-Copy-Update) callbacks are deferred kernel work that runs
  on the current CPU. By marking cores 0-3 (A55 audio cluster) as "no-callback", RCU work
  is offloaded to dedicated kthreads on the A76 cores, eliminating surprise latency on the
  audio cluster.

- **`nohz_full=0-3`**: When a CPU has only one runnable task (the audio thread), the kernel
  stops the scheduling timer tick entirely. This eliminates the ~1ms periodic interrupt that
  otherwise fires even when there is nothing to schedule. Applied to the A55 audio cluster.

- **`skew_tick=1`**: On a multi-core system, all cores' timer ticks fire simultaneously by
  default, causing lock contention on shared kernel data structures. Skewing distributes
  ticks across cores.

### 5.3 CPU Frequency Governor

Already set to `performance` in `hardware.nix` line 22:
```nix
powerManagement.cpuFreqGovernor = "performance";
```

This locks all cores at maximum frequency, eliminating frequency transition latency (which
can be 100-200us on RK3588). For the A55 cores running audio, this is essential.

### 5.4 Kernel Config (if Building Custom Kernel)

If a custom kernel is built for the embedded image, these config options are important:

```
CONFIG_PREEMPT=y              # Voluntary preemption (at minimum)
# CONFIG_PREEMPT_RT=y         # Full RT (ideal but requires matching RT patch)
CONFIG_NO_HZ_FULL=y           # Support tickless operation (needed for nohz_full=)
CONFIG_RCU_NOCB_CPU=y         # Support RCU callback offloading (needed for rcu_nocbs=)
CONFIG_HZ=1000                # 1000Hz timer (1ms resolution, best for audio)
CONFIG_HIGH_RES_TIMERS=y      # High-resolution timers (sub-ms precision)
CONFIG_CPU_FREQ_GOV_PERFORMANCE=y  # Performance governor built-in
CONFIG_SCHED_DEBUG=n          # Disable scheduler debug (reduces overhead)
CONFIG_FTRACE=n               # Disable function tracer (reduces overhead)
CONFIG_KPROBES=n              # Disable kprobes (reduces overhead)
```

### 5.5 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Medium (sched_rt_runtime_us=-1 removes safety limit; a RT bug can hang system) |
| **Complexity** | Low (sysctl/boot params) to High (custom kernel) |
| **Expected Impact** | Medium-High -- eliminates OS-level jitter sources |
| **Layer** | OS configuration / Kernel configuration |

---

## 6. Audio Subsystem

### 6.1 PipeWire Configuration

Current configuration (`nix/embedded/audio.nix` lines 29-37):

```nix
services.pipewire.extraConfig.pipewire."92-low-latency" = {
  "context.properties" = {
    "default.clock.rate" = 48000;
    "default.clock.quantum" = 256;
    "default.clock.min-quantum" = 64;
    "default.clock.max-quantum" = 1024;
  };
};
```

**Recommended enhancements**:

```nix
services.pipewire.extraConfig.pipewire."92-low-latency" = {
  "context.properties" = {
    "default.clock.rate" = 48000;
    "default.clock.quantum" = 256;
    "default.clock.min-quantum" = 256;   # Changed: prevent dynamic quantum reduction
    "default.clock.max-quantum" = 256;   # Changed: lock quantum to 256 (no adaptive sizing)
    "default.clock.force-quantum" = 256; # Force quantum (overrides client requests)
  };
  "context.modules" = [
    {
      name = "libpipewire-module-rt";
      args = {
        "nice.level" = -15;
        "rt.prio" = 88;                  # High RT priority for data thread
        "rt.time.soft" = -1;             # No soft time limit
        "rt.time.hard" = -1;             # No hard time limit
      };
    }
  ];
};
```

### 6.2 Buffer Size Trade-offs

| Buffer (samples) | Latency (ms) | Safety Margin | Use Case |
|:-:|:-:|:-:|:-:|
| 64 | 1.33 | Very tight | Not viable with 16 stems + effects |
| 128 | 2.67 | Tight | Possible if effects are lightweight |
| **256** | **5.33** | **Good** | **Current setting -- recommended** |
| 512 | 10.67 | Very safe | Fallback if xruns persist |

**256 samples is the right balance** for 4 decks x 4 stems with CLAP effects. The A76 cores
at 2.4GHz provide ~5760 cycles per sample (2.4GHz / 48kHz * 256 / 256), which is sufficient
for multiband processing with delay lines.

### 6.3 WirePlumber ALSA Tuning

The current WirePlumber config (`audio.nix` lines 65-102) already sets:
- `api.alsa.period-size = 256` (matches PipeWire quantum)
- `session.suspend-timeout-seconds = 0` (prevents device suspension)
- `api.alsa.headroom = 0` (no extra buffering)

**Additional optimizations**:

```nix
# Add to WirePlumber ALSA rules (both devices)
"api.alsa.disable-batch" = true      # Disable ALSA batch mode (reduces latency)
"node.always-process" = true          # Never suspend the node
"resample.quality" = 0               # Disable resampling (both are 48kHz native)
```

### 6.4 Direct ALSA Access (Alternative)

PipeWire adds one extra buffer period of latency compared to direct ALSA access. If the
PipeWire overhead proves problematic, mesh-player could use ALSA directly via the existing
CPAL backend:

```rust
// cpal_backend.rs already supports direct ALSA
// Configuration: set ALSA device to "hw:PCM5102A,0" (raw hardware, no plug conversion)
```

**Trade-off**: Losing PipeWire means losing automatic format conversion (the ES8388 only
accepts stereo, which the `plug` wrapper handles) and hot-plug support. Only consider this
as a last resort.

### 6.5 JACK node.always-process

The kiosk wrapper (`kiosk.nix`) currently uses `pw-link` to manually connect ports. An
alternative is to configure PipeWire to always process the mesh-player JACK client, even
before links are established:

```
# In PipeWire JACK client properties
PIPEWIRE_NODE = "{ node.always-process = true, node.want-driver = true }"
```

This is set via the `PIPEWIRE_PROPS` environment variable before launching mesh-player.

### 6.6 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low (PipeWire config), High (direct ALSA) |
| **Complexity** | Low |
| **Expected Impact** | Medium -- stabilizes audio pipeline timing |
| **Layer** | OS configuration |

---

## 7. Power Management

### 7.1 Disable CPU Idle States (C-States)

When a CPU enters a C-state (idle state), waking it takes time:
- **C1** (WFI on ARM): ~1-5us wakeup
- **C2** (deeper idle): ~50-200us wakeup
- **C3+ (cluster powerdown)**: ~500us-2ms wakeup

A 200us wakeup during the 5.33ms audio period steals ~4% of the budget and adds jitter.

#### a) Runtime C-State Disable via /dev/cpu_dma_latency

The most portable approach: open `/dev/cpu_dma_latency` and write 0, which tells the kernel
to never enter any C-state deeper than C0.

```rust
// In mesh-player main.rs
#[cfg(feature = "embedded-rt")]
fn disable_cpu_idle() -> Option<std::fs::File> {
    use std::io::Write;
    let mut f = std::fs::File::create("/dev/cpu_dma_latency").ok()?;
    // Write 0 (little-endian i32) = maximum 0us latency tolerance
    f.write_all(&0i32.to_le_bytes()).ok()?;
    log::info!("CPU idle states disabled via /dev/cpu_dma_latency");
    // MUST keep file handle open for the entire process lifetime
    Some(f)
}

fn main() -> iced::Result {
    // ...
    #[cfg(feature = "embedded-rt")]
    let _cpu_dma_latency_fd = disable_cpu_idle();
    // ...
}
```

**Critical**: The file descriptor must remain open for the entire process lifetime. Dropping
it re-enables C-states. Store it in a variable that lives as long as `main()`.

#### b) Per-Core Idle State Disable

Disable deep idle only on the A55 audio cores. The A76 cores can use idle states freely
since they run latency-tolerant background work:

```nix
# nix/embedded/hardware.nix
systemd.services.mesh-cpu-idle = {
  description = "Disable deep CPU idle states on A55 audio cores";
  wantedBy = [ "multi-user.target" ];
  serviceConfig = {
    Type = "oneshot";
    RemainAfterExit = true;
    ExecStart = pkgs.writeShellScript "disable-cpu-idle" ''
      # Disable all idle states except C0 on A55 audio cores (0-3)
      for cpu in 0 1 2 3; do
        for state in /sys/devices/system/cpu/cpu$cpu/cpuidle/state*/disable; do
          echo 1 > "$state" 2>/dev/null
        done
      done
      echo "Disabled idle states on CPUs 0-3 (A55 audio cluster)"
    '';
  };
};
```

### 7.2 CPU Frequency Pinning

The `performance` governor is already set globally. Verify it is active on audio cores:

```nix
# Verification command (for debugging, not config):
# cat /sys/devices/system/cpu/cpu4/cpufreq/scaling_governor
# Should output: performance
```

No additional configuration needed -- `powerManagement.cpuFreqGovernor = "performance"` is
already in `hardware.nix`.

### 7.3 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low (slightly higher idle power consumption) |
| **Complexity** | Low |
| **Expected Impact** | High -- eliminates C-state wakeup jitter (up to 2ms) |
| **Layer** | Application code (/dev/cpu_dma_latency) + OS configuration (per-core) |

---

## 8. Process Isolation

### 8.1 isolcpus (Kernel-Level Isolation)

`isolcpus` removes CPUs from the kernel scheduler entirely. No tasks will be scheduled on
isolated CPUs unless explicitly pinned via `sched_setaffinity`. This is the strongest
form of isolation.

```nix
# nix/embedded/hardware.nix
boot.kernelParams = [
  # ... existing params ...
  "isolcpus=managed_irq,domain,0-3"  # Isolate A55 audio cores from scheduler
];
```

**Flags explained**:
- `domain`: Removes CPUs from scheduler load-balancing domains
- `managed_irq`: Also prevents managed IRQs from being assigned to isolated CPUs

**Implication**: After booting with `isolcpus=0-3`, only threads that explicitly call
`sched_setaffinity` to cores 0-3 will run there. The cage systemd service's CPUAffinity
directive will still work because it calls `sched_setaffinity`.

**Combined with Section 1**: The cage service sets `CPUAffinity=0-7` (allow all), but
due to `isolcpus`, only the threads that explicitly pin to 0-3 (JACK RT, iced UI, rayon audio)
will run on the A55 cluster. Track loaders, preset builders, and other non-pinned threads will
only run on the A76 cores 4-7.

### 8.2 cgroups v2 CPU Isolation

A more flexible alternative to `isolcpus` that does not require a reboot to change:

```nix
# Create a cgroup for audio threads
systemd.services."cage-tty1" = {
  serviceConfig = {
    Slice = "audio-rt.slice";
  };
};

# Define the audio-rt slice
systemd.slices.audio-rt = {
  description = "Real-time audio processing";
  sliceConfig = {
    AllowedCPUs = "4-7";
    CPUWeight = 10000;  # Maximum weight
  };
};
```

**Trade-off**: cgroups v2 provides the `cpuset` controller for CPU pinning, but it does not
prevent the scheduler from running kernel threads on those CPUs. `isolcpus` is more
aggressive and deterministic.

### 8.3 nohz_full for Tickless Audio Cores

Already covered in Section 5.2 as a boot parameter. When combined with `isolcpus=0-3`, the
effect is maximized: the isolated A55 audio cores with nohz_full have virtually zero kernel
interference when running a single audio thread each.

### 8.4 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Medium (isolcpus limits system flexibility; misconfiguration leaves cores idle) |
| **Complexity** | Medium |
| **Expected Impact** | High -- strongest isolation, eliminates scheduler jitter on audio cores |
| **Layer** | OS configuration (kernel params) |

---

## 9. Filesystem / I/O

### 9.1 I/O Priority for Background Tasks

Track loading, USB sync, and database operations should not compete with audio for I/O
bandwidth. Use `ionice` (via `libc::ioprio_set`) for background threads.

```rust
// In the track loader thread (crates/mesh-player/src/loader/mod.rs)
#[cfg(feature = "embedded-rt")]
fn set_background_io_priority() {
    // IOPRIO_CLASS_IDLE (3) = only gets I/O time when no other task needs it
    // IOPRIO_PRIO_VALUE(class, data) = (class << 13) | data
    let ioprio = (3 << 13) | 0; // IDLE class, priority 0
    unsafe {
        // ioprio_set(IOPRIO_WHO_PROCESS=1, 0=self, ioprio)
        libc::syscall(libc::SYS_ioprio_set, 1i32, 0i32, ioprio);
    }
}
```

### 9.2 I/O Scheduler for USB Devices

Audio tracks are typically loaded from USB sticks. The default I/O scheduler (mq-deadline
or bfq) should be configured for throughput:

```nix
# nix/embedded/hardware.nix
services.udev.extraRules = ''
  # ... existing USB automount rules ...

  # Set BFQ I/O scheduler for USB storage (better for rotational + mixed workloads)
  ACTION=="add|change", KERNEL=="sd[a-z]", SUBSYSTEM=="block", ATTR{queue/scheduler}="bfq"
'';
```

### 9.3 Readahead for Audio Streaming

The track loader pre-reads entire tracks into memory (all stems are decoded to PCM arrays).
Once loaded, audio playback is purely from RAM -- no filesystem I/O in the audio path.

However, during track loading, use `posix_fadvise(FADV_SEQUENTIAL)` to optimize readahead:

```rust
// Already in mesh-core (libc dependency exists for posix_fadvise)
// Verify this is used in the track loading path
```

The `libc` dependency already exists in mesh-core for this purpose (`Cargo.toml` line 58:
`libc = "0.2" # posix_fadvise for sequential read hints on USB export`).

### 9.4 tmpfs for Temporary Files

Ensure temporary files (PipeWire sockets, log FIFOs) are on tmpfs to avoid disk I/O:

```nix
# This is default on NixOS -- /tmp is tmpfs
# Verify: boot.tmp.useTmpfs = true (default)
```

### 9.5 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low |
| **Complexity** | Low |
| **Expected Impact** | Low-Medium -- audio path is already in-RAM; helps during track loading |
| **Layer** | Application code (ionice) + OS configuration (scheduler) |

---

## 10. RK3588-Specific Considerations

### 10.1 big.LITTLE Scheduling: EAS vs Manual Pinning

The kernel's Energy Aware Scheduler (EAS) dynamically migrates tasks between big and LITTLE
cores based on energy/performance heuristics. For audio workloads, this is harmful:

- **Task migration latency**: Moving a task between clusters flushes L1/L2 caches (~50-100us)
- **Non-deterministic placement**: EAS may decide the audio thread is "light" and move it to A55
- **Thermal-driven migration**: Under sustained load, EAS may throttle A76 and migrate to A55

**Solution**: The combination of `isolcpus=0-3` + explicit `sched_setaffinity` completely
bypasses EAS for audio and UI threads. EAS only affects non-isolated cores (4-7), which is fine for
background tasks (loading, stretching, peaks).

If `isolcpus` is not used, disable EAS migration for specific tasks:

```rust
// Write 0 to /proc/self/task/<tid>/migrate_disable -- Linux 5.14+
// This prevents the scheduler from migrating the thread between clusters
```

### 10.2 Mali GPU Considerations

The iced UI renders via wgpu (Vulkan on PanVK) on the Mali-G610 MP4 GPU. The GPU handles all
pixel-level rendering (waveforms, text rasterization, compositing), while the CPU only prepares
uniform buffers and submits draw commands.

**GPU workload analysis**:

| Metric | Value | Notes |
|--------|:-----:|-------|
| Resolution | 2880 × 864 | Custom ultrawide |
| Refresh rate | 120Hz | 8.33ms frame budget |
| Pixels per frame | 2,488,320 | |
| Required fill rate | ~300 Mpixels/s | With 3x overdraw: ~900 Mpixels/s |
| Mali-G610 MP4 theoretical | ~8 Gpixels/s | 4 shader cores @ ~1GHz, 2 pixels/clock/core |
| **GPU utilization** | **~4-11%** | Massively overprovisioned for 2D UI |

**Waveform shaders** (`mesh-widgets/src/waveform/shader/`): Each deck uploads 416 bytes of
uniforms and issues a single fullscreen-triangle draw call. The fragment shader performs
per-pixel SDF computation for peaks, beat grid, cue markers, and playhead. This is pure GPU
work — no CPU tessellation.

**Potential conflicts**:

- **GPU IRQs**: Mali GPU interrupts should route to A76 cores (not A55 audio cores).
  Handled by the IRQ affinity setup (Section 3).
- **Memory bandwidth**: GPU rendering and audio both access DDR. However, audio's 32KB per
  buffer period is negligible. GPU is the primary DDR consumer but well within the 34GB/s
  LPDDR4X bandwidth.
- **Thermal**: GPU load generates heat that may throttle adjacent A76 cores.
- **PanVK driver maturity**: The open-source Vulkan driver for Mali-G610 reached Vulkan 1.2
  conformance (Collabora, 2025). For fallback, the Panthor OpenGL ES 3.1+ driver is more
  battle-tested on RK3588.

**Mitigations**:
1. GPU IRQs are handled by the IRQ affinity setup (Section 3)
2. iced uses Vulkan Mailbox present mode (already set in `kiosk.nix`), which minimizes
   GPU memory bandwidth usage vs Fifo mode
3. The mali-shader feature (enabled for aarch64 in `mesh-player.nix` line 113) uses
   GPU-native WGSL shaders for waveform rendering, eliminating CPU tessellation overhead

### 10.3 Thermal Throttling Prevention

The A76 cores at 2.4GHz under sustained load generate significant heat. The RK3588's
thermal management may throttle clock speed:

```nix
# Monitor thermal (for debugging):
# cat /sys/class/thermal/thermal_zone*/temp
# cat /sys/class/thermal/thermal_zone*/type

# If throttling is observed, options include:
# 1. Passive cooling: heatsink + fan on the OrangePi 5
# 2. Reduce max frequency slightly for thermal headroom:
#    (NOT recommended unless actively throttling)
```

**Recommendation**: Ensure the OrangePi 5 has a heatsink installed. The stock board often
ships without one. A passive heatsink with fan is strongly recommended for sustained
real-time audio workloads.

### 10.4 DDR Bandwidth

RK3588S has LPDDR4X at up to 2133MHz (34GB/s theoretical bandwidth). Audio processing
at 4 decks x 4 stems x 256 samples x 8 bytes = ~32KB per buffer period -- negligible
compared to DDR bandwidth. GPU rendering is the primary DDR consumer.

**No optimization needed** -- DDR bandwidth is not a bottleneck for audio.

### 10.5 I2S/DMA Audio Hardware

The PCM5102A DAC connects via I2S3 on the GPIO header. The ES8388 is the onboard codec.
Both use the Rockchip I2S-TDM controller with DMA transfers.

**DMA buffer configuration**: PipeWire/ALSA configures the DMA ring buffer size. With
`api.alsa.period-size = 256` and 2 periods, the DMA buffer is 512 samples (21.3ms total,
5.33ms per period). This is handled by the WirePlumber ALSA rules already in place.

### 10.6 Assessment

| Aspect | Value |
|--------|-------|
| **Risk** | Low (thermal), Medium (EAS bypass) |
| **Complexity** | Low-Medium |
| **Expected Impact** | Medium -- prevents platform-specific gotchas |
| **Layer** | Hardware (heatsink) + OS configuration |

---

## 11. Implementation Priority Matrix

Ordered by impact-to-effort ratio:

| Priority | Optimization | Impact | Effort | Risk |
|:--------:|-------------|:------:|:------:|:----:|
| **1** | `mlockall(MCL_CURRENT \| MCL_FUTURE)` | High | Low | Low |
| **2** | `/dev/cpu_dma_latency` = 0 (disable C-states) | High | Low | Low |
| **3** | `kernel.sched_rt_runtime_us = -1` | High | Low | Medium |
| **4** | `rcu_nocbs=0-3 nohz_full=0-3` boot params | High | Low | Low |
| **5** | RT priority for rayon workers (SCHED_FIFO 70) | High | Medium | Medium |
| **6** | Fine-grained CPU affinity (per-thread) | High | Medium | Low |
| **7** | `isolcpus=managed_irq,domain,0-3` | High | Low | Medium |
| **8** | IRQ affinity pinning | Medium | Low | Low |
| **9** | `transparent_hugepage=never` | Medium | Low | Low |
| **10** | PipeWire fixed quantum + RT module config | Medium | Low | Low |
| **11** | Disable deep idle on audio cores | Medium | Low | Low |
| **12** | Background I/O priority (ionice) | Low | Low | Low |
| **13** | PREEMPT_RT kernel | High | High | High |

### Recommended Implementation Phases

**Phase 1 -- Quick Wins (NixOS config only, no code changes)**:
- Items 3, 4, 8, 9, 10, 11
- Modify `nix/embedded/hardware.nix` and `nix/embedded/audio.nix`
- Zero application code changes required

**Phase 2 -- Application Code (feature-gated behind `embedded-rt`)**:
- Items 1, 2, 5, 6
- Add `embedded-rt` feature to mesh-player's Cargo.toml
- Enable in `nix/packages/mesh-player.nix` for aarch64 builds

**Phase 3 -- Strong Isolation (after Phase 1+2 validated)**:
- Item 7 (`isolcpus`)
- Requires careful testing -- if any thread fails to set affinity, it will only run on A55
- Only proceed after Phase 2's per-thread affinity code is stable

**Phase 4 -- Nuclear Option (only if xruns persist)**:
- Item 13 (PREEMPT_RT kernel)
- Requires custom kernel build in the Nix flake
- Needs vendor kernel source + matching RT patch

---

## Appendix A: Complete NixOS Configuration Diff

### hardware.nix changes

```nix
# nix/embedded/hardware.nix
{ pkgs, lib, ... }:

{
  hardware.deviceTree.filter = "rk3588s-orangepi-5*.dtb";
  hardware.deviceTree.overlays = [
    { name = "pcm5102a-i2s3"; dtsFile = ./pcm5102a-i2s3.dts; }
  ];

  hardware.graphics.enable = true;

  powerManagement.cpuFreqGovernor = "performance";

  boot.loader.timeout = 0;
  boot.initrd.systemd.enable = true;
  boot.initrd.systemd.emergencyAccess = true;

  boot.consoleLogLevel = 0;
  boot.initrd.verbose = false;
  boot.kernel.sysctl = {
    "kernel.printk" = "0 0 0 0";
    "vm.swappiness" = 1;                         # Changed: 10 -> 1
    "kernel.sched_rt_runtime_us" = -1;            # NEW: allow 100% RT CPU
    "kernel.sched_latency_ns" = 4000000;          # NEW: 4ms CFS latency
    "kernel.sched_min_granularity_ns" = 500000;   # NEW: 0.5ms CFS granularity
  };

  boot.kernelParams = [
    "quiet" "loglevel=0"
    "systemd.show_status=false" "rd.systemd.show_status=false"
    "udev.log_level=3" "rd.udev.log_level=3"
    "vt.global_cursor_default=0" "logo.nologo"
    "threadirqs"
    # NEW: real-time audio optimizations
    "transparent_hugepage=never"
    "irqaffinity=4-7"
    "rcu_nocbs=0-3"
    "rcu_nocb_poll"
    "nohz_full=0-3"
    "skew_tick=1"
    "nosoftlockup"
    "nowatchdog"
    # Phase 3 (after app-level affinity is validated):
    # "isolcpus=managed_irq,domain,0-3"
  ];

  # Disable irqbalance (conflicts with manual IRQ pinning)
  services.irqbalance.enable = false;

  # Disable deep CPU idle states on A55 audio cores
  systemd.services.mesh-cpu-idle = {
    description = "Disable deep CPU idle states on A55 audio cores";
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = pkgs.writeShellScript "disable-cpu-idle" ''
        for cpu in 0 1 2 3; do
          for state in /sys/devices/system/cpu/cpu$cpu/cpuidle/state*/disable; do
            echo 1 > "$state" 2>/dev/null
          done
        done
        echo "Disabled deep idle states on CPUs 0-3 (A55 audio cluster)"
      '';
    };
  };

  # Existing unchanged...
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;
  services.udisks2.enable = true;

  # Pin system services to A76 cores (keep off A55 audio cluster)
  systemd.services.NetworkManager.serviceConfig.CPUAffinity = "4-7";
  systemd.services.systemd-journald.serviceConfig.CPUAffinity = "4-7";

  services.udev.extraRules = ''
    # Existing USB automount rules ...

    # BFQ I/O scheduler for USB storage
    ACTION=="add|change", KERNEL=="sd[a-z]", SUBSYSTEM=="block", ATTR{queue/scheduler}="bfq"
  '';
}
```

### audio.nix changes

```nix
# nix/embedded/audio.nix -- additions to PipeWire and IRQ config
{
  # ... existing content ...

  # Enhanced PipeWire RT configuration
  services.pipewire.extraConfig.pipewire."92-low-latency" = {
    "context.properties" = {
      "default.clock.rate" = 48000;
      "default.clock.quantum" = 256;
      "default.clock.min-quantum" = 256;     # Lock quantum
      "default.clock.max-quantum" = 256;     # Lock quantum
      "default.clock.force-quantum" = 256;   # Force quantum
    };
    "context.modules" = [
      {
        name = "libpipewire-module-rt";
        args = {
          "nice.level" = -15;
          "rt.prio" = 88;
          "rt.time.soft" = -1;
          "rt.time.hard" = -1;
        };
      }
    ];
  };

  # IRQ affinity for audio
  systemd.services.mesh-irq-affinity = {
    description = "Pin audio IRQs to A55 audio core, move others to A76";
    after = [ "sound.target" "mesh-audio-init.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "mesh-irq-affinity" ''
        for irqdir in /proc/irq/*/; do
          irq=$(basename "$irqdir")
          [ "$irq" = "default_smp_affinity" ] && continue
          actions=$(cat "$irqdir/actions" 2>/dev/null || echo "")
          case "$actions" in
            *i2s*|*es8388*|*rockchip-i2s*|*dma*)
              # Audio IRQs -> CPU 0 (A55, dedicated audio core; bitmask 0x01)
              echo 01 > "$irqdir/smp_affinity" 2>/dev/null ;;
            *)
              # Everything else -> A76 cores 4-7 (bitmask 0xf0)
              echo f0 > "$irqdir/smp_affinity" 2>/dev/null ;;
          esac
        done
        echo f0 > /proc/irq/default_smp_affinity
      '';
    };
  };
}
```

---

## Appendix B: Application Code Changes

### Feature Flag

```toml
# crates/mesh-player/Cargo.toml
[features]
embedded-rt = []
```

```nix
# nix/packages/mesh-player.nix -- enable for aarch64 builds
cargoBuildFlags = [ "-p" "mesh-player" ]
  ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isAarch64
    [ "--features" "mali-shader,embedded-rt" ];
```

### main.rs Initialization

```rust
// crates/mesh-player/src/main.rs -- early in main()

#[cfg(feature = "embedded-rt")]
mod rt_init {
    /// Lock all memory to prevent page faults in audio callback
    pub fn mlockall() {
        unsafe {
            let ret = libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE);
            if ret == 0 {
                log::info!("[RT] Memory locked (mlockall MCL_CURRENT|MCL_FUTURE)");
            } else {
                log::warn!("[RT] mlockall failed: {}", std::io::Error::last_os_error());
            }
        }
    }

    /// Disable CPU idle states via /dev/cpu_dma_latency
    /// Returns the file handle (MUST be kept alive for the process lifetime)
    pub fn disable_cpu_idle() -> Option<std::fs::File> {
        use std::io::Write;
        match std::fs::File::create("/dev/cpu_dma_latency") {
            Ok(mut f) => {
                if f.write_all(&0i32.to_le_bytes()).is_ok() {
                    log::info!("[RT] CPU idle states disabled via /dev/cpu_dma_latency");
                    Some(f)
                } else {
                    log::warn!("[RT] Failed to write to /dev/cpu_dma_latency");
                    None
                }
            }
            Err(e) => {
                log::warn!("[RT] Could not open /dev/cpu_dma_latency: {}", e);
                None
            }
        }
    }

    /// Verify the process has sufficient RT capabilities
    pub fn verify_rt_caps() {
        unsafe {
            let mut rlim: libc::rlimit = std::mem::zeroed();
            if libc::getrlimit(libc::RLIMIT_RTPRIO, &mut rlim) == 0 {
                log::info!("[RT] RLIMIT_RTPRIO: soft={}, hard={}", rlim.rlim_cur, rlim.rlim_max);
            }
            if libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut rlim) == 0 {
                log::info!("[RT] RLIMIT_MEMLOCK: soft={}, hard={}",
                    rlim.rlim_cur, rlim.rlim_max);
            }
        }
    }
}

fn main() -> iced::Result {
    // ... logger init ...

    #[cfg(feature = "embedded-rt")]
    {
        rt_init::verify_rt_caps();
        rt_init::mlockall();
    }

    #[cfg(feature = "embedded-rt")]
    let _cpu_dma_latency_guard = rt_init::disable_cpu_idle();

    // Rayon thread pool with RT affinity + priority
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|i| format!("rayon-audio-{}", i))
        .start_handler(|_thread_idx| {
            #[cfg(feature = "embedded-rt")]
            {
                // Pin to A55 cores 2-3 (core 0 = JACK RT, core 1 = UI)
                // A55 in-order pipeline provides deterministic timing for RT audio
                unsafe {
                    let mut cpuset: libc::cpu_set_t = std::mem::zeroed();
                    libc::CPU_ZERO(&mut cpuset);
                    libc::CPU_SET(2, &mut cpuset);
                    libc::CPU_SET(3, &mut cpuset);
                    libc::sched_setaffinity(0,
                        std::mem::size_of::<libc::cpu_set_t>(), &cpuset);
                }
                // Set SCHED_FIFO priority 70
                unsafe {
                    let param = libc::sched_param { sched_priority: 70 };
                    let ret = libc::sched_setscheduler(0, libc::SCHED_FIFO, &param);
                    if ret != 0 {
                        eprintln!("[RT] Warning: sched_setscheduler failed for rayon worker: {}",
                            std::io::Error::last_os_error());
                    }
                }
                // Pre-fault stack pages
                let stack = vec![0u8; 512 * 1024]; // 512KB
                std::hint::black_box(&stack);
            }
        })
        .build_global()
        .expect("Failed to initialize Rayon thread pool");

    // ... rest of main() ...
}
```

---

## Appendix C: Monitoring and Debugging

### Verify Optimizations Are Active

```bash
# Check CPU affinity of all mesh-player threads
for tid in $(ls /proc/$(pgrep mesh-player)/task/); do
  name=$(cat /proc/$(pgrep mesh-player)/task/$tid/comm 2>/dev/null)
  affinity=$(taskset -p $tid 2>/dev/null | awk '{print $NF}')
  policy=$(chrt -p $tid 2>/dev/null | head -1)
  echo "$tid ($name): affinity=$affinity $policy"
done

# Check memory locking
grep -i locked /proc/$(pgrep mesh-player)/status

# Check C-state usage (should show 0 on A55 audio cores 0-3)
cat /sys/devices/system/cpu/cpu0/cpuidle/state*/usage

# Check IRQ affinity
for irq in /proc/irq/*/smp_affinity; do
  echo "$(dirname $irq | xargs basename): $(cat $irq) -- $(cat $(dirname $irq)/actions 2>/dev/null)"
done

# Check RT scheduling
chrt -p $(pgrep -f "rayon-audio")

# Check nohz_full is active
dmesg | grep -i nohz

# Monitor xruns
journalctl -u cage-tty1 -f | grep -i xrun

# cyclictest for worst-case latency on A55 audio cores (install via rt-tests)
cyclictest -p 80 -t 4 -a 0-3 -m -D 60s
```

---

## Appendix D: References

- [Optimizing RK3588 Performance (SBCwiki)](https://sbcwiki.com/news/articles/tune-your-rk3588/)
- [musnix: Real-time audio in NixOS](https://github.com/musnix/musnix)
- [Reducing jitter on Linux with task isolation](https://www.codeblueprint.co.uk/2020/05/03/reducing-jitter-on-linux-with-task-isolation.html)
- [CPU Isolation -- nohz_full (SUSE Labs)](https://www.suse.com/c/cpu-isolation-nohz_full-part-3/)
- [Ubuntu Real-Time Kernel Tuning](https://ubuntu.com/blog/real-time-kernel-tuning)
- [PipeWire RT Module Documentation](https://docs.pipewire.org/page_module_rt.html)
- [JACK RT Configuration](https://jackaudio.org/faq/linux_rt_config.html)
- [NixOS Audio Production Wiki](https://wiki.nixos.org/wiki/Audio_production)
- [Armbian RK3588 RT Kernel Forum Thread](https://forum.armbian.com/topic/28559-realtime-kernel-for-orange-pi-5/)
- [PREEMPT_RT on RK3588 (Radxa Forum)](https://forum.radxa.com/t/preemt-rt-linux-on-rk3588/20292)
- [Linux Foundation: CPU idle for RT workloads](https://wiki.linuxfoundation.org/realtime/documentation/howto/applications/cpuidle)
- [CPU C-States Explanation (Red Hat)](https://access.redhat.com/solutions/202743)
- [core_affinity Rust Crate](https://docs.rs/core_affinity/latest/core_affinity/)
- [PREEMPT_RT Kernel Versions](https://wiki.linuxfoundation.org/realtime/preempt_rt_versions)
- [ARM Cortex-A76 vs A55 Comparison](https://versus.com/en/arm-cortex-a55-vs-arm-cortex-a76)
