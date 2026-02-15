# ARM64 Embedded Hardware Research

## Goal

Eliminate the laptop requirement for live DJ performance by running mesh-player on a small embedded ARM64 box with 1-2 small displays (one per deck).

## SBC Comparison

All viable candidates use the **RK3588** SoC — no other ARM chip has comparable open-source Vulkan driver maturity (PanVK/Panthor on Mali-G610). Non-RK3588 boards (MediaTek Genio 1200, Amlogic A311D2, Qualcomm) all fall short on RAM, NVMe, GPU drivers, or price.

### Top Candidates

| Spec | Orange Pi 5 Plus | Radxa Rock 5T | Orange Pi 5 Max | Radxa Rock 5B+ |
|------|-----------------|---------------|-----------------|----------------|
| SoC | RK3588 | RK3588 | RK3588 | RK3588 |
| CPU | 4x A76 @2.4 + 4x A55 | Same | Same | 4x A76 @2.2-2.4 + 4x A55 |
| GPU | Mali-G610 MP4 | Same | Same | Same |
| RAM | Up to 32GB LPDDR4x | Up to 32GB LPDDR5 | Up to 16GB LPDDR5 | Up to 32GB LPDDR5 |
| Display | **2x HDMI 2.1** + MIPI | **2x HDMI** + USB-C DP + MIPI + eDP | **2x HDMI 2.1** + MIPI | **2x HDMI 2.1** + USB-C DP + MIPI |
| NVMe | M.2 **PCIe 3.0 x4** | **2x** M.2 PCIe 3.0 x2 | M.2 **PCIe 3.0 x4** | 2x M.2 PCIe 3.0 x2 |
| USB-A | 2x 3.0 + 2x 2.0 | 2x 3.2 + 2x 2.0 + **2x header** | 2x 3.0 + 2x 2.0 | 2x 3.0 + 2x 2.0 |
| Ethernet | 2x 2.5GbE | **2x 2.5GbE** | 1x 2.5GbE | 1x 2.5GbE |
| WiFi | Optional module | Onboard WiFi 6/6E | **Onboard WiFi 6E** | Onboard WiFi 6 |
| Power | USB-C | **12V DC barrel** | USB-C | USB-C PD |
| Industrial | No | **Yes (RK3588J)** | No | No |
| Size | 100×70mm | 110×82mm | **89×57mm** | 100×75mm |
| Price (16GB) | **~$129** | ~$140 | **~$125** | ~$135 |
| Community | **Largest** (Armbian) | Strong (Radxa) | Growing | Strong (Radxa) |

### Disqualified Boards

| Board | Price | Why Not |
|-------|-------|---------|
| Khadas Edge2 Pro | $300 | No NVMe, 1x HDMI, 2x overpriced |
| Mixtile Blade 3 | $259 | 0 USB-A ports, server-oriented |
| Firefly ROC-RK3588S-PC | $299+ | Overpriced, RK3588S, small community |
| Cool Pi 4B | ~$142 | RK3588S, hard to source, micro-HDMI |
| Banana Pi BPI-W3 | $162 (8GB) | Too large (148×101mm), only 8GB available |
| Pine64 QuartzPro64 | $150 | Developer-only, not retail, 180×180mm |
| Radxa NIO 12L (MT Genio) | ~$150 | No NVMe, proprietary GPU blob |
| Khadas VIM4 (A311D2) | $220 | Max 8GB, old A73 cores, blob GPU |
| ASUS Tinker Board 3N | varies | RK3568 — only A55 cores, max 8GB |

### Recommendation: Orange Pi 5 Plus or Rock 5T

**Orange Pi 5 Plus** wins on value:
- $10-15 cheaper at 16GB, 32GB option available
- Full PCIe 3.0 x4 NVMe (double per-slot bandwidth vs Rock 5T's x2)
- Largest RK3588 community (Armbian, ubuntu-rockchip)
- 4 USB-A without pin headers
- Compact 100×70mm

**Rock 5T** wins on robustness:
- **12V barrel jack** — no USB-C PD negotiation failures in dark DJ booths
- Dual 2.5GbE for venue network + NAS
- 2x M.2 slots for NVMe + spare
- Industrial RK3588J variant (-40 to +85°C) for hot festival booths
- LPDDR5 (higher bandwidth than OPi5+'s LPDDR4x)

**Orange Pi 5 Max** is the tiny option (89×57mm, credit card sized) if enclosure size is critical, but capped at 16GB.

**Rock 5B+ vs Rock 5T**: The 5B+ is $5 cheaper and slightly smaller but uses USB-C PD for power instead of the 5T's robust barrel jack. For a live performance box, the barrel jack is worth the $5 premium. Otherwise, specs are nearly identical.

## Codebase Compatibility: Excellent

### Zero x86-Specific Code

Comprehensive audit found:
- No `cfg(target_arch)` conditionals anywhere
- No SIMD intrinsics (SSE/AVX)
- No inline assembly
- No architecture-specific build logic in any crate

### Already ARM-Ready

- `.cargo/config.toml` has `[target.aarch64-unknown-linux-gnu]` with correct rustflags
- `flake.nix` uses `flake-utils.lib.eachDefaultSystem` (includes `aarch64-linux`)
- `patches/libpd-sys/build.rs` already handles aarch64 explicitly

### Required Build Fixes

Only ~10 lines of hardcoded x86 paths need updating:

| File | Line(s) | Issue | Fix |
|------|---------|-------|-----|
| `nix/apps/build-deb.nix` | 271 | `x86_64-linux-gnu` pkg-config path | Use `$(dpkg-architecture -qDEB_HOST_MULTIARCH)` |
| `nix/apps/build-deb.nix` | 389-394 | `x86_64-linux-gnu` rpath in patchelf | Same dynamic arch detection |
| `nix/apps/build-deb.nix` | 356-363 | `onnxruntime-linux-x64-*` download URL | Branch on arch for `aarch64` variant |
| `nix/devshell.nix` | 85 | `x86_64-unknown-linux-gnu` in CXXFLAGS | Use Nix `system` variable |
| `nix/devshell.nix` | 127 | `intel_icd.x86_64.json` Vulkan ICD paths | ARM Mali ICD path |
| `nix/common.nix` | Essentia build | No `--no-msse` flag | Add `--no-msse` when `system == "aarch64-linux"` |

## Dependency Compatibility

### All Dependencies ARM64 Compatible

| Dependency | Version | ARM64 Status | Notes |
|------------|---------|-------------|-------|
| **iced** | 0.14 | Supported | Vulkan via PanVK on Mali-G610 |
| **wgpu** | 27 | Supported | Vulkan 1.2 confirmed working (Zed editor on RK3588) |
| **cpal** | 0.15 | Supported | ALSA backend, architecture-independent |
| **ort** | 2.0.0-rc.11 | Supported | aarch64-linux prebuilt binaries from pyke CDN |
| **cozo** | 0.7.6 | Supported | Official ARM64 support (SQLite backend) |
| **clack-host** | latest | Supported | Pure Rust FFI, architecture-agnostic |
| **rubato** | 0.16 | Supported | Optional NEON feature (nightly only), scalar fallback works |
| **signalsmith-stretch** | 0.1.3 | Supported | Header-only C++, no SIMD requirements |
| **hidapi** | 2.6 | Supported | hidraw interface, architecture-independent |
| **midir** | 0.10 | Supported | ALSA sequencer, architecture-independent |
| **libpd-sys** | patched | Supported | CMake auto-detection, Pure Data is portable C |
| **essentia** | 0.1.5 | Buildable | Needs `--no-msse` flag on ARM, builds from source |
| **libopenmpt** | system | Supported | In Arch Linux ARM repos |
| **mpg123** | system | Supported | Optional ARM NEON optimizations |
| **procspawn** | 1.0 | Supported | fork/exec, ELF linker flags identical on aarch64 |
| **symphonia** | 0.5 | Supported | Pure Rust audio decoding |
| **rayon** | 1.10 | Supported | Pure Rust parallelism |
| **ndarray** | 0.17 | Supported | Pure Rust array operations |
| **baseview** | latest | Supported | X11/xcb, architecture-independent |

## GPU Rendering Stack

```
iced 0.14 --> wgpu 27 --> Vulkan 1.2 --> PanVK (Mesa 25.1+) --> Mali-G610 MP4
                    \---> OpenGL ES 3.1 --> Panfrost --> Mali-G610 (fallback)
                     \--> tiny-skia (CPU fallback, 100-300% slower on ARM)
```

### PanVK (Open-Source Vulkan Driver)

- **Vulkan 1.2 conformant** on Mali-G610 as of May 2025 (Collabora + ARM)
- Kernel driver: **Panthor** (mainline since Linux 6.10)
- Mesa version: 25.1+ required for Vulkan 1.2
- **Real-world validation**: Zed editor (wgpu-based Rust app) confirmed working on RK3588 with Mali-G610

### Mali-G610 TBDR Architecture Benefits

The tile-based deferred rendering architecture is well-suited to DJ UIs:

- **Transaction elimination**: Skips writing unchanged tiles — up to 99% bandwidth savings for mostly-static UI
- **MSAA in tile memory**: 4x anti-aliasing in on-chip SRAM with minimal bandwidth cost
- **Bandwidth math**: 2x 1280x800 @ 60fps = ~491 MB/s — well within LPDDR5's ~34 GB/s

### Display Options

| Combo | Minimum Kernel | Notes |
|-------|---------------|-------|
| Dual HDMI | 6.13+ | Best for off-the-shelf HDMI touchscreens |
| HDMI + MIPI DSI | 6.14+ | Good for custom embedded panels |
| Dual MIPI DSI | 6.14+ | Best for compact enclosures |

RK3588 has 4 VOPs supporting up to 4 simultaneous displays. Target kernel 6.14+ for full display support.

## Audio Stack

### Real-Time Audio Performance

- **cpal 0.15**: ALSA backend works identically on ARM64
- **JACK/PipeWire**: Full ARM64 support, PipeWire in Pro-Audio mode recommended
- **Achievable latency**: 128 samples @ 48kHz = **2.67ms** buffer latency
- **Proven**: Sub-2ms latency demonstrated on Raspberry Pi (less capable than RK3588 A76 cores)

### Performance Tuning for big.LITTLE

```bash
# Pin audio threads to A76 big cores (typically CPU 4-7)
taskset -c 4-7 ./mesh-player

# Lock cores at max frequency (eliminates 50-200us scaling spikes)
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor

# Optional: isolate cores exclusively for audio
# Add to kernel cmdline: isolcpus=6,7
```

- **PREEMPT_RT** mainline since Linux 6.12 for ARM64 — no patches needed
- Use `SCHED_FIFO` or `SCHED_DEADLINE` for audio threads
- Route USB audio IRQs to same core as audio thread via `/proc/irq/<N>/smp_affinity`

### USB Audio Interface

**Critical requirement**: DJ use needs **4 independent output channels** — 2 for master (speakers) + 2 for cue (headphones). Most "2-in/2-out" interfaces mirror the headphone to main output in hardware — these will NOT work for DJ cueing.

All USB Audio Class 2 devices work via the architecture-independent `snd-usb-audio` kernel module.

#### Viable 4-Channel Interfaces (budget-friendly, compact)

| Interface | Price (EUR) | Size (mm) | Weight | Outputs | USB | Linux Status | Notes |
|-----------|-------------|-----------|--------|---------|-----|-------------|-------|
| **Behringer UMC204HD** | ~65 | 185×130×46 | 600g | 2 TRS + 2 RCA + HP (A/B switch) | USB-B | YES (kernel 5.15+, ALSA UCM config) | **Best value** — true 2×4, cheapest 4ch option |
| **Zoom U-44** | ~100-140 | 192×92×43 | 310g | 2 TRS + 2 RCA + HP (balance knob) | Mini-B | YES (class compliant) | **Best for DJ** — HP blends output pairs, battery option |
| **Zoom AMS-44** | ~160 | 129×74×46 | 223g | 2 TRS + 2 HP (3.5mm) | USB-C | YES (UAC 2.0) | **Most compact** 4ch, bus-powered + battery |
| Focusrite Scarlett 4i4 | ~240 | 180×130×59 | 808g | 4 TRS + HP (independent mix) | USB-C | YES (kernel 6.8+, alsa-scarlett-gui) | Best audio quality but over budget |

#### NOT Suitable for DJ Cueing (headphone mirrors main)

MOTU M2/M4, Audient EVO 4, Steinberg UR22C, NI Komplete Audio 2, Arturia MiniFuse 2, SSL 2, PreSonus AudioBox GO — all mirror headphone to main output.

#### Recommendation

**Behringer UMC204HD** (~65 EUR) for budget. ALSA sees 4 playback channels: outputs 1-2 → TRS (master to speakers), outputs 3-4 → RCA (cue to headphone amp or direct). The HP jack has A/B switch between pairs. Downside: USB-B connector, 185mm wide.

**Zoom U-44** (~100-140 EUR) if budget allows. Designed for DJ use — hardware headphone balance knob between output pairs 1-2 and 3-4. Can run on 2x AA batteries for portable gigs.

## CLAP Plugin Ecosystem

| Plugin | ARM64 Builds | Notes |
|--------|-------------|-------|
| **LSP Plugins** | Official aarch64 (v1.2.26) | Fixed ARM-specific freeze bug in latest release |
| **Airwindows** | Native RPi/ARM64 | CLAP format via community builds |
| **Surge XT** | Arch Linux ARM packages | Full synth + effects |
| **Dexed** | Arch Linux ARM packages | FM synth |
| **Vital** | Not available | Requires SSE intrinsic porting |
| **Commercial plugins** | Generally unavailable | Proprietary = x86 only |

The `clack-host` Rust FFI layer compiles on any architecture. Plugin `.so` files must be native aarch64 binaries.

## ML Inference

### CPU Inference (Recommended — Zero Code Changes)

| Model | Size | Per-Patch (A76 CPU) | Per-Track (4min) |
|-------|------|---------------------|------------------|
| EffNet (genre/embeddings) | 17MB | ~10-20ms | ~0.9-1.2s |
| Jamendo mood head | 2.7MB | <1ms | <1ms |

**Under 1.5 seconds per track** for offline batch analysis — perfectly adequate. ONNX Runtime uses ARM NEON optimizations + KleidiAI (28-51% uplift on ARM Cortex cores).

### RK3588 NPU (Optional Future Phase)

The 6 TOPS NPU could provide 5-12x speedup but requires:

1. Model conversion: ONNX → RKNN format via `rknn-toolkit2` (x86 host only)
2. INT8 quantization with calibration dataset (risk: genre threshold accuracy)
3. New runtime dependency: `rknpu2-rs` Rust bindings or raw FFI to `librknnrt.so`
4. Separate `.rknn` model files to ship

**Not recommended initially** — CPU is fast enough for all current models.

## Recommended Hardware Setup

### Option A: Best Value (~$290-340)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Plus 16GB | ~$129 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Behringer UMC204HD (4ch, USB-B) | ~$65 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$350-380** |

### Option B: Most Robust (~$330-380)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Radxa Rock 5T 16GB | ~$140 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Behringer UMC204HD (4ch, USB-B) | ~$65 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$390-430** |

### Option C: Most Compact (~$350-400)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Max 16GB (89×57mm) | ~$125 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Zoom AMS-44 (4ch, USB-C, 129×74mm) | ~$160 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$440-470** |

## Recommended OS Stack

- **OS**: Ubuntu 24.04 ARM64 or NixOS aarch64-linux
- **Kernel**: 6.14+ (Panthor + dual display) with PREEMPT_RT
- **Mesa**: 25.1+ (PanVK Vulkan 1.2)
- **Audio**: PipeWire in Pro-Audio mode (or JACK2)
- **Governor**: `performance` (eliminates frequency scaling latency)

## Risk Matrix

| Risk | Severity | Mitigation |
|------|----------|------------|
| PanVK driver bugs with complex wgpu shaders | Medium | GLES fallback (`WGPU_BACKEND=gl`), tiny-skia CPU fallback |
| Waveform rendering FPS too low | Medium | Profile early, optimize canvas redraw strategy |
| Stem separation too slow on CPU | Low | Not needed on performance box (done in mesh-cue on desktop) |
| Insufficient CLAP plugin selection | Low | LSP + Airwindows cover core DJ effects (EQ, compressor, reverb, delay) |
| INT8 NPU quantization accuracy | Low | Don't use NPU initially; CPU inference is fast enough |
| Thermal throttling under sustained load | Low | Active cooling + `performance` governor |

## Implementation Phases

### Phase 1: Build Infrastructure (no hardware needed)
- Fix hardcoded x86 paths in Nix files (~10 lines)
- Add `--no-msse` to Essentia build for aarch64
- Test cross-compilation or QEMU-based Nix build

### Phase 2: Hardware Validation (requires RK3588 board)
- Boot Ubuntu 24.04 ARM64 or NixOS
- Build mesh-player natively on device
- Test iced wgpu rendering through PanVK on Mali-G610
- Validate dual HDMI display setup
- Benchmark audio latency with USB audio interface (4ch routing)

### Phase 3: Performance Optimization
- Profile waveform canvas rendering, optimize if needed
- Configure big.LITTLE core pinning for audio threads
- Test CLAP plugin loading (LSP Plugins aarch64)
- Benchmark ML inference (genre/mood analysis)

### Phase 4: Production Packaging
- Design custom enclosure for SBC + displays + audio interface
- Create ARM64 .deb or NixOS package
- Auto-start mesh-player on boot (kiosk mode)
- Optional: NPU integration for faster analysis

## Verdict: GO

The mesh-player codebase is remarkably portable to ARM64. Zero architectural changes needed — only build infrastructure fixes. The RK3588 provides sufficient CPU, GPU, and I/O for real-time DJ performance with dual displays and low-latency audio.
