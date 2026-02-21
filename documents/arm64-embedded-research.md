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

### Recommendation: Orange Pi 5 Pro 8GB (Primary) — Updated Feb 2026

**Orange Pi 5 Pro 8GB** is the primary pick for embedded mesh:
- **$80** — cheapest viable RK3588S board with all required features
- **89×56mm credit-card form factor** — fits behind a 7" display in a sandwich mount
- **Built-in WiFi 5 + BT 5.0** — no separate module needed
- **LPDDR5** — lower power consumption (RK3588S is 15-20% more efficient than full RK3588)
- **ES8388 onboard codec + I2S3 on GPIO** — identical audio architecture to OPi 5 Plus/Max
- **8 GB RAM is sufficient** — worst case 3.7 GB used (4 decks × 4 stems × 7-min tracks + heavy FX), leaving 4.3 GB free
- **No NVMe required** — DJs play from USB 3.0 sticks (CDJ workflow). M.2 slot available as optional upgrade for built-in library.
- RK3588S vs RK3588: same CPU/GPU/NPU/I2S, fewer PCIe lanes (irrelevant without NVMe) and one fewer HDMI 2.1 (irrelevant for single 7" display)

**Orange Pi 5 Max 16GB** is the upgrade pick (~$145):
- WiFi 6E + BT 5.3, PCIe 3.0 x4 NVMe, same 89×57mm form factor
- Best choice if adding built-in NVMe library or needing faster WiFi for track transfer

**Orange Pi 5 Plus 16GB** remains a solid alternative (~$142+$15 WiFi):
- Larger 100×70mm form factor, dual 2.5GbE, 2x USB 3.0
- Largest RK3588 community (Armbian, ubuntu-rockchip)
- WiFi requires separate M.2 E-Key module

**Rock 5T** wins on robustness:
- **12V barrel jack** — no USB-C PD negotiation failures in dark DJ booths
- Dual 2.5GbE for venue network + NAS
- 2x M.2 slots for NVMe + spare
- Industrial RK3588J variant (-40 to +85°C) for hot festival booths
- GPIO I2S routing less documented than Orange Pi boards

**Rock 5B+ vs Rock 5T**: The 5B+ is $5 cheaper and slightly smaller but uses USB-C PD for power instead of the 5T's robust barrel jack. For a live performance box, the barrel jack is worth the $5 premium. Otherwise, specs are nearly identical.

**Orange Pi 6 Plus (CIX CD8180) — evaluated and rejected (Feb 2026):**
The OPi 6 Plus is a 12-core ARMv9.2 board that significantly outperforms RK3588 in raw CPU throughput, but Linux kernel support is immature — mainline is missing GPU, VPU, display, and ACPI as of Feb 2026. PREEMPT_RT patches are unvalidated on CIX SoCs. Audio works on the vendor 6.1 kernel but real-time audio latency is undocumented. Not recommended until kernel maturity improves (reassess late 2026).

### LPDDR5 vs LPDDR4x: No Real Difference for This Workload

| Metric | LPDDR4x (OPi 5 Plus) | LPDDR5 (Rock 5T/5B+) | Impact on Mesh |
|--------|----------------------|----------------------|----------------|
| Peak bandwidth (theory) | ~33 GB/s | ~43 GB/s | — |
| Real memcpy (measured) | ~10.5 GB/s | ~12.3 GB/s | +18% |
| Access latency | ~100-120 ns (lower) | ~130-150 ns (higher) | LPDDR4x wins |
| Audio processing | Fits in A76 L2 cache | Fits in A76 L2 cache | **Identical** |
| GPU rendering (2x 1080p) | Needs ~3-6 GB/s | Needs ~3-6 GB/s | **Both 2x oversupplied** |
| Track loading | NVMe-bound, not RAM-bound | Same | **Identical** |
| ML inference (17MB EffNet) | Compute-bound on A76 | Same | **<5% difference** |
| DMC throttle risk | More predictable | Can throttle under load | LPDDR4x edge |

Audio processing at 48kHz with 512-sample buffers has a working set of ~256 KB — fits entirely in the A76's 512 KB L2 cache. DRAM bandwidth is not touched during real-time audio. The 18% bandwidth advantage of LPDDR5 only matters for memory-saturating workloads (LLMs, video encoding), not audio or 2D UI rendering.

### RAM Requirements: 16GB Is Enough

Worst-case analysis: 4 decks, 4 stems each, 1-2 linked stems per deck, heavy effects.

| Component | Typical | Worst Case |
|-----------|---------|------------|
| Audio buffers (4 decks × 4 stems, 5min tracks) | 1,840 MB | 2,200 MB |
| Linked stems (1-2 extra per deck) | 920 MB | 1,840 MB |
| CLAP plugins (32 instances) | 160 MB | 640 MB |
| Multiband buffers + delay lines | 128 MB | 128 MB |
| Database (500-1000 tracks) | 20 MB | 35 MB |
| UI + GPU + wgpu framebuffers | 40 MB | 55 MB |
| Framework overhead (threads, fonts, caches) | 45 MB | 50 MB |
| OS + PipeWire + cage compositor | ~500 MB | ~700 MB |
| **Total** | **~3.7 GB** | **~5.6 GB** |

16GB leaves 10+ GB free for OS page cache (accelerates track loading from NVMe). 24/32GB provides zero practical benefit for mesh-player. Audio data dominates (~58% of total): each 5-minute track decoded to 4 stems = ~460 MB per deck.

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

> **See "Primary BOM: Orange Pi 5 Max + I2S DAC" in the I2S DAC section below for the current recommended build (~$277-347).** The options below are preserved for historical reference but are superseded by the I2S DAC approach.

### Legacy Option A: OPi 5 Plus + USB Audio (~$350-380) — SUPERSEDED

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Plus 16GB | ~$129 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Behringer UMC204HD (4ch, USB-B) | ~$65 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$350-380** |

### Legacy Option B: Rock 5T + USB Audio (~$390-430) — SUPERSEDED

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Radxa Rock 5T 16GB | ~$140 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Behringer UMC204HD (4ch, USB-B) | ~$65 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$390-430** |

### Legacy Option C: OPi 5 Max + USB Audio (~$440-470) — SUPERSEDED

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Max 16GB (89×57mm) | ~$125 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Zoom AMS-44 (4ch, USB-C, 129×74mm) | ~$160 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$440-470** |

## NixOS Wayland Kiosk Setup

### Architecture

```
NixOS boot (10-15s) → cage compositor (wlroots) → mesh-player (iced/wgpu)
                                                          ↓
                                              GLES 3.1 via Panfrost (recommended)
                                              Vulkan 1.2 via PanVK (risky on Wayland)
```

### Cage Kiosk Compositor

[Cage](https://github.com/cage-kiosk/cage) is a wlroots-based Wayland kiosk compositor. It displays a single maximized application, no task switcher, no desktop. NixOS has a built-in `services.cage` module with auto-login via PAM — no display manager needed.

- **`-m extend`**: Spans both HDMI displays as one surface (e.g., 3840×1080 for 2× 1920×1080)
- **`-d`**: Disables client-side decorations
- Supports `wlr-output-management` protocol for runtime display config via `wlr-randr`
- If cage's extend mode is too limited, **labwc** (wlroots stacking compositor) or **sway** (in kiosk config) are alternatives with full output layout control

### GPU Backend: Use GLES, Not Vulkan

| Backend | Driver | Status on ARM Wayland | Recommendation |
|---------|--------|----------------------|----------------|
| GLES 3.1 | Panfrost (Mesa) | **Conformant, battle-tested** | **Production use** |
| Vulkan 1.2 | PanVK (Mesa 25.1+) | Known wgpu+Wayland surface bug (#6320) | Testing only |
| tiny-skia | CPU | 100-300% slower on ARM | Emergency fallback |

The wgpu Vulkan+Wayland combo has a known `VkSurfaceKHR` duplicate creation bug. GLES via Panfrost uses EGL — the standard ARM Wayland rendering path, mature and reliable.

### NixOS Configuration

```nix
services.cage = {
  enable = true;
  user = "mesh";
  program = "${meshPlayer}/bin/mesh-player";
  extraArguments = [ "-m" "extend" "-d" ];
  environment.WGPU_BACKEND = "gl";  # GLES via Panfrost
};

# PipeWire for low-latency audio
security.rtkit.enable = true;
services.pipewire = {
  enable = true;
  alsa.enable = true;
  jack.enable = true;
};

# Boot speed optimizations (10-15s target)
boot.loader.timeout = 0;
boot.plymouth.enable = false;
boot.initrd.systemd.enable = true;
systemd.services.systemd-udev-settle.enable = false;
systemd.services.NetworkManager-wait-online.enable = false;

# Alternative: greetd for auto-restart on crash
# services.greetd.settings.initial_session = {
#   command = "${pkgs.cage}/bin/cage -m extend -d -- ${meshPlayer}/bin/mesh-player";
#   user = "mesh";
# };
```

### NixOS on RK3588: Current Status

- **Community flake**: [gnull/nixos-rk3588](https://github.com/gnull/nixos-rk3588) — supports OPi 5/5+, Rock 5A/5B/5 ITX
- **Kernel 6.13+** required for RK3588 HDMI output (basic support, Collabora)
- **Dual HDMI** may need Armbian-patched kernel (gnull flake can provide this)
- **Key kernel modules**: `rockchipdrm`, `dw_hdmi`, `panthor` (or `panfrost`)
- **Boot-to-app**: 10-15s optimized (NVMe), 20-30s default
- **Touchscreen**: libinput auto-detects USB HID touch devices, no config needed

### iced 0.14 on Wayland

iced 0.14 uses winit 0.30.x which auto-selects Wayland when `WAYLAND_DISPLAY` is set (cage sets this automatically). Known issues:
- Touch events may not fire correctly on Wayland (iced #1392)
- Widget interaction freeze reported in some configs (iced #2297)
- Both are tracked upstream and improving

## Recommended OS Stack

- **OS**: NixOS aarch64-linux (gnull/nixos-rk3588 flake)
- **Kernel**: 6.13+ (HDMI output) with Armbian patches for dual HDMI, PREEMPT_RT
- **Mesa**: 25.1+ (Panfrost GLES 3.1 + PanVK Vulkan 1.2)
- **Compositor**: cage (`-m extend`) or labwc for more layout control
- **Audio**: PipeWire in Pro-Audio mode with JACK compatibility
- **GPU backend**: `WGPU_BACKEND=gl` (GLES via Panfrost)
- **Governor**: `performance` (eliminates frequency scaling latency)

## Risk Matrix

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Dual HDMI on mainline kernel** | **High** | Use Armbian-patched kernel via gnull/nixos-rk3588 flake |
| wgpu Vulkan+Wayland surface bug | High | Use `WGPU_BACKEND=gl` (GLES via Panfrost) |
| Waveform rendering FPS too low | Medium | Profile early, GLES is adequate for 2D canvas |
| iced touch input quirks on Wayland | Medium | Track upstream fixes (iced #1392, #2297) |
| Stem separation too slow on CPU | Low | Not needed on performance box (done in mesh-cue on desktop) |
| Insufficient CLAP plugin selection | Low | LSP + Airwindows cover core DJ effects |
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

## Software Implementation Guide

This section documents everything needed to build, deploy, and debug the embedded NixOS mesh-player system on the Orange Pi 5 Pro (or any RK3588/RK3588S board with ES8388 + I2S3 GPIO).

### How the I2S DAC Works (Physical Layer)

The PCM5102A DAC is connected to the Orange Pi's 40-pin GPIO header via **6 ordinary female-to-female Dupont jumper wires**. No soldering required for prototyping — the GY-PCM5102 breakout board has pin headers that accept standard jumper wires.

```
Orange Pi 5 Pro                    GY-PCM5102 Breakout
40-pin GPIO Header                 (PCM5102A DAC)
┌──────────────┐                   ┌──────────────┐
│ Pin 1  (3.3V)│───── red ────────▶│ VIN          │
│ Pin 6  (GND) │───── black ──────▶│ GND          │
│              │                   │ SCK ◀── GND  │  (tie SCK to GND pad on board)
│ Pin 35 (SCLK)│───── yellow ─────▶│ BCK          │
│ Pin 38 (LRCK)│───── green ──────▶│ LRCK         │
│ Pin 40 (SDO) │───── blue ───────▶│ DIN          │
└──────────────┘                   └──────┬───────┘
                                          │ 3.5mm jack
                                          ▼
                                    PA System / Mixer
                                    (MASTER output)
```

**How it works electrically:**

1. The RK3588S's I2S3 controller generates three signals: a bit clock (BCK/SCLK), a word select clock (LRCK, alternates L/R channel), and serial data (SDO/DIN — the actual audio samples)
2. These are 3.3V CMOS logic signals, clocked at BCK = sample_rate × bits × 2 channels (e.g., 44.1kHz × 32 × 2 = 2.822 MHz)
3. The PCM5102A's internal PLL regenerates a master clock from BCK — that's why SCK is tied to GND (tells the chip to use internal PLL mode)
4. The DAC converts the digital I2S stream to analog audio on its 3.5mm output jack
5. No I2C control bus is needed — the PCM5102A is a "dumb" DAC that just converts whatever I2S data it receives. All configuration (sample rate, bit depth) is implicit in the I2S clock signals

**For production:** Replace jumper wires with soldered connections or a small adapter PCB. The GY-PCM5102 board (30×20mm) mounts inside the enclosure with double-sided tape or M2 standoffs.

### How the Kernel Sees the DAC (Device Tree Overlay)

Linux doesn't automatically know there's a DAC connected to the I2S3 pins. A **Device Tree Overlay (DTBO)** tells the kernel:

1. Enable the `i2s3_2ch` controller (disabled by default on most OPi images)
2. Configure the pinmux so GPIO3_C2/C0/B7 become I2S3 signals instead of general GPIO
3. Register a `simple-audio-card` that pairs the I2S3 controller with a `ti,pcm5102a` codec driver
4. The PCM5102A codec driver is built into mainline Linux — it's essentially a no-op driver that tells ALSA "this is a stereo DAC, no control registers"

The overlay source lives in the I2S DAC section below. Compilation:

```bash
# Compile the overlay
dtc -I dts -O dtb -o pcm5102a-i2s3.dtbo pcm5102a-i2s3.dts

# On Armbian/ubuntu-rockchip: copy and enable
sudo cp pcm5102a-i2s3.dtbo /boot/dtb/rockchip/overlay/
sudo orangepi-config  # → System → Hardware → enable overlay

# On NixOS: loaded via hardware.deviceTree.overlays (see NixOS section below)
```

After loading the overlay and rebooting, `aplay -l` shows two sound cards:

```
card 0: rockchipes8388 [rockchip-es8388]    ← onboard codec (3.5mm jack, CUE)
card 1: PCM5102A [PCM5102A]                 ← I2S DAC on GPIO (MASTER)
```

### Audio Routing: How mesh-player Talks to Two Sound Cards

mesh-player uses **cpal** (cross-platform audio library) for audio output, which on Linux talks to ALSA. With PipeWire running, there are two paths:

#### Option A: Direct ALSA (lowest latency, recommended for production)

mesh-player opens two separate ALSA devices directly:

```
Master output → ALSA device "hw:1,0" (PCM5102A)    → PA system
Cue output    → ALSA device "hw:0,0" (ES8388)       → Headphones
```

In mesh-player's audio configuration, this means selecting the output device by ALSA card name. cpal enumerates all available ALSA devices — mesh-player picks the right one by matching the card name substring (`"PCM5102A"` for master, `"es8388"` for cue).

**Implementation in mesh-player:**

```rust
// Pseudocode for device selection
let host = cpal::default_host();
let devices = host.output_devices()?;

let master_device = devices.clone()
    .find(|d| d.name().unwrap_or_default().contains("PCM5102A"))?;
let cue_device = devices
    .find(|d| d.name().unwrap_or_default().contains("es8388"))?;

// Open separate output streams on each device
let master_stream = master_device.build_output_stream(&config, master_callback, err_fn, None)?;
let cue_stream = cue_device.build_output_stream(&config, cue_callback, err_fn, None)?;
```

Card numbering (`hw:0` vs `hw:1`) can change between boots. **Always match by name, never by number.**

#### Option B: PipeWire routing (more flexible, slightly more latency)

PipeWire exposes both cards as sinks. mesh-player outputs to PipeWire's default sink, and WirePlumber rules route streams to the correct physical device based on stream properties.

```
mesh-player master stream → PipeWire → alsa_output.platform-pcm5102a-sound → PA
mesh-player cue stream    → PipeWire → alsa_output.platform-rockchip-es8388 → HP
```

This requires mesh-player to set `media.role` or `node.name` properties on each stream so WirePlumber can distinguish them. More complex to set up, but allows runtime re-routing via `pavucontrol` or `pw-link`.

**Recommendation:** Use Option A (direct ALSA) for production. PipeWire adds ~2-5ms latency and an extra failure point. For development/debugging, PipeWire is more convenient.

#### ALSA Configuration for Direct Access

When using direct ALSA (bypassing PipeWire), ensure PipeWire doesn't grab the devices exclusively. In NixOS:

```nix
# Allow direct ALSA access alongside PipeWire
environment.etc."alsa/conf.d/99-mesh.conf".text = ''
  # Named device aliases for mesh-player
  pcm.mesh_master {
    type hw
    card "PCM5102A"
    device 0
  }
  pcm.mesh_cue {
    type hw
    card "rockchipes8388"
    device 0
  }
'';
```

mesh-player can then open `"mesh_master"` and `"mesh_cue"` as stable ALSA device names regardless of card numbering.

### NixOS Deployment: Full Workflow

#### 1. Initial OS Installation (One-Time)

**Step 1: Flash U-Boot to SPI NOR flash**

The OPi 5 Pro has SPI NOR flash for the bootloader. Flash it once using an Armbian SD card:

```bash
# On your x86 workstation:
# Download Armbian for Orange Pi 5 (RK3588S images work for OPi 5 Pro)
wget https://dl.armbian.com/orangepi5/Bookworm_current

# Flash to microSD
dd if=Armbian_*.img of=/dev/sdX bs=1M status=progress

# Boot the OPi 5 Pro from the microSD, then:
sudo armbian-install
# Select: "Install/Update the bootloader on SPI Flash"
# This writes U-Boot to the SPI NOR — board now boots from any media

# Power off, remove the Armbian SD card
```

**Step 2: Build the NixOS SD image**

On your x86 NixOS workstation:

```bash
# Enable aarch64 emulation (add to your workstation's configuration.nix)
# boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
# then: sudo nixos-rebuild switch

# Clone the mesh embedded flake (see flake structure below)
cd ~/Projects/mesh
nix build .#nixosConfigurations.mesh-embedded.config.system.build.sdImage

# Flash to microSD
dd if=result/sd-image/*.img of=/dev/sdX bs=1M status=progress
```

**Step 3: First boot**

Insert the microSD into the OPi 5 Pro, connect HDMI + USB keyboard, power on. NixOS boots to a login prompt. SSH is enabled by default.

```bash
# From your workstation, verify SSH access
ssh mesh@<board-ip>

# Done — all future updates happen remotely via nixos-rebuild
```

#### 2. Flake Structure

Add an embedded target to the existing mesh project flake:

```
mesh/
├── flake.nix                    # Add mesh-embedded NixOS config
├── nix/
│   ├── common.nix               # Existing build deps
│   └── embedded/
│       ├── configuration.nix    # NixOS system config
│       ├── hardware.nix         # OPi 5 Pro hardware (kernel, DT, GPU)
│       ├── audio.nix            # PipeWire + I2S DAC overlay + ALSA config
│       ├── kiosk.nix            # cage compositor + auto-login
│       └── pcm5102a-i2s3.dts   # Device tree overlay source
```

**flake.nix additions:**

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nixos-rk3588.url = "github:gnull/nixos-rk3588";
    # OR for OPi 5 Pro specifically:
    # nixos-opi5pro.url = "github:tlan16/nixos-orange-5-pro";
  };

  outputs = { self, nixpkgs, nixos-rk3588, ... }: {
    nixosConfigurations.mesh-embedded = nixpkgs.lib.nixosSystem {
      system = "aarch64-linux";
      modules = [
        nixos-rk3588.nixosModules.orangepi5  # board support (RK3588S)
        ./nix/embedded/configuration.nix
        ./nix/embedded/hardware.nix
        ./nix/embedded/audio.nix
        ./nix/embedded/kiosk.nix
      ];
    };
  };
}
```

#### 3. NixOS Configuration Modules

**hardware.nix** — Board-specific kernel, GPU, device tree:

```nix
{ pkgs, ... }: {
  # Kernel: Armbian-patched 6.1 LTS with PREEMPT_RT
  # (provided by gnull/nixos-rk3588 board module)

  # GPU: Use GLES via Panfrost (not Vulkan — known Wayland bug)
  hardware.graphics.enable = true;

  # Device tree overlay for PCM5102A DAC
  hardware.deviceTree.overlays = [
    {
      name = "pcm5102a-i2s3";
      dtsFile = ./pcm5102a-i2s3.dts;
    }
  ];

  # CPU governor: lock to performance (no frequency scaling latency)
  powerManagement.cpuFreqGovernor = "performance";

  # Pin audio threads to A76 big cores (cores 4-7 on RK3588S)
  # Done via systemd CPUAffinity on the mesh-player service (see kiosk.nix)

  # Boot speed optimizations
  boot.loader.timeout = 0;
  boot.plymouth.enable = false;
  boot.initrd.systemd.enable = true;
  systemd.services.systemd-udev-settle.enable = false;
  systemd.services.NetworkManager-wait-online.enable = false;

  # USB stick automounting (DJ plugs in their USB stick)
  services.udisks2.enable = true;
}
```

**audio.nix** — PipeWire + ALSA + I2S DAC:

```nix
{ pkgs, ... }: {
  # PipeWire as audio server
  security.rtkit.enable = true;
  services.pipewire = {
    enable = true;
    alsa.enable = true;
    alsa.support32Bit = false;  # aarch64 only
    jack.enable = true;         # JACK compatibility for pro-audio tools
  };

  # Persist ALSA card state across reboots
  hardware.alsa.enablePersistence = true;

  # Named ALSA device aliases for mesh-player
  environment.etc."alsa/conf.d/99-mesh.conf".text = ''
    pcm.mesh_master {
      type hw
      card "PCM5102A"
      device 0
    }
    pcm.mesh_cue {
      type hw
      card "rockchipes8388"
      device 0
    }
  '';

  # WirePlumber: disable suspend on audio devices (prevents pops/clicks)
  services.pipewire.wireplumber.extraConfig."99-mesh-audio" = {
    "monitor.alsa.rules" = [
      {
        matches = [
          { "node.name" = "~alsa_output.*es8388*"; }
          { "node.name" = "~alsa_output.*PCM5102A*"; }
        ];
        actions = {
          update-props = {
            "session.suspend-timeout-seconds" = 0;   # never suspend
            "api.alsa.period-size" = 256;             # ~5.8ms at 44.1kHz
            "api.alsa.headroom" = 256;
          };
        };
      }
    ];
  };

  # ES8388 headphone volume: set a safe default at boot
  # (ES8388 has software-controllable gain via I2C, amixer sets it)
  systemd.services.mesh-audio-init = {
    description = "Initialize audio card volumes";
    after = [ "sound.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${pkgs.alsa-utils}/bin/amixer -c rockchipes8388 set Headphone 80%";
    };
  };
}
```

**kiosk.nix** — Cage compositor + auto-start mesh-player:

```nix
{ pkgs, ... }:
let
  meshPlayer = pkgs.callPackage ../../default.nix {};  # mesh-player package
in {
  # Auto-login user
  users.users.mesh = {
    isNormalUser = true;
    extraGroups = [ "audio" "video" "input" "plugdev" ];
    initialPassword = "mesh";  # Change on first login
  };

  # Cage kiosk compositor — single fullscreen app, no desktop
  services.cage = {
    enable = true;
    user = "mesh";
    program = "${meshPlayer}/bin/mesh-player";
    extraArguments = [ "-d" ];  # disable client-side decorations
    environment = {
      WGPU_BACKEND = "gl";              # GLES via Panfrost (stable)
      # WGPU_BACKEND = "vulkan";        # PanVK (experimental, known bugs)
      MESA_GL_VERSION_OVERRIDE = "3.1"; # ensure GLES 3.1 exposure
      WLR_NO_HARDWARE_CURSORS = "1";    # software cursor (avoids KMS cursor issues)
    };
  };

  # Restart mesh-player on crash (watchdog)
  systemd.services."cage-tty1" = {
    serviceConfig = {
      Restart = "always";
      RestartSec = 2;
      CPUAffinity = "4-7";  # pin to A76 big cores
    };
  };

  # SSH for remote management (always available even if UI crashes)
  services.openssh = {
    enable = true;
    settings.PasswordAuthentication = true;
  };

  # Firewall: allow SSH
  networking.firewall.allowedTCPPorts = [ 22 ];
}
```

**configuration.nix** — Top-level system config:

```nix
{ pkgs, ... }: {
  system.stateVersion = "24.11";
  networking.hostName = "mesh-embedded";

  # Timezone
  time.timeZone = "Europe/Berlin";  # adjust to your locale

  # Networking
  networking.networkmanager.enable = true;

  # Basic packages for debugging/maintenance
  environment.systemPackages = with pkgs; [
    vim
    htop
    alsa-utils      # aplay, arecord, amixer, aplay -l
    pipewire        # pw-cli, pw-link, pw-dump
    usbutils        # lsusb
    pciutils        # lspci
    dtc             # device tree compiler (for overlay debugging)
    wlr-randr       # display configuration under Wayland
    evtest          # input device testing (touch, MIDI)
  ];

  # Nix settings for remote builds
  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    trusted-users = [ "mesh" ];
  };
}
```

#### 4. Building and Deploying

**Build approach: QEMU emulation on x86 workstation (recommended)**

```bash
# One-time: enable aarch64 emulation on your NixOS workstation
# In your workstation's configuration.nix:
#   boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
# Then: sudo nixos-rebuild switch

# Build the full NixOS system closure for the embedded target
# Most packages come from cache.nixos.org (pre-built aarch64 binaries)
# Only mesh-player itself needs to be compiled (via QEMU, ~5-10 min)
nix build .#nixosConfigurations.mesh-embedded.config.system.build.toplevel
```

**Why QEMU emulation over cross-compilation:**
- Cross-compiled derivations have **different store paths** → can't use the official binary cache
- QEMU emulation uses **native aarch64 store paths** → 95% of packages come pre-built from `cache.nixos.org`
- Only mesh-player + its Rust deps + Essentia need actual compilation
- QEMU is slow for compilation (~3-5x slower) but you only compile what's not cached

**Deploy to the board over SSH:**

```bash
# From your workstation (x86), deploy to the running board:
nixos-rebuild switch \
  --flake .#mesh-embedded \
  --target-host mesh@192.168.1.100 \
  --use-remote-sudo

# This:
# 1. Builds the system closure (using QEMU emulation + binary cache)
# 2. Copies new/changed store paths to the board via SSH
# 3. Runs `switch-to-configuration switch` on the board
# 4. Board restarts affected services (including cage → mesh-player)
```

**Alternative: deploy-rs (for multi-board fleets)**

```bash
# If managing multiple mesh-embedded boards:
nix run github:serokell/deploy-rs -- .#mesh-embedded
```

#### 5. Development Workflow

**Daily development cycle:**

```
  x86 workstation                          OPi 5 Pro (192.168.1.100)
  ┌──────────────┐                         ┌──────────────────────┐
  │ Edit Rust code│                         │ NixOS running        │
  │ in mesh repo  │                         │ cage → mesh-player   │
  │               │                         │                      │
  │ nixos-rebuild │──── SSH ───────────────▶│ switch-to-config     │
  │ --target-host │     (copy store paths)  │ restart cage service │
  │               │                         │ mesh-player starts   │
  │               │◀─── SSH ────────────────│ journalctl -f output │
  │ See logs      │     (debug)             │                      │
  └──────────────┘                         └──────────────────────┘
```

1. Edit mesh-player Rust code on your x86 workstation
2. Run `nixos-rebuild switch --target-host mesh@<ip> --use-remote-sudo`
3. Build runs locally via QEMU (~2-5 min for incremental Rust builds)
4. New binary is copied to the board, cage restarts, mesh-player launches
5. Debug via SSH: `journalctl -u cage-tty1 -f` for live logs

**Faster iteration: native build on the board**

For quick compile-test cycles, you can also build directly on the board:

```bash
# SSH into the board
ssh mesh@192.168.1.100

# Clone mesh repo (or mount via NFS/sshfs)
git clone https://github.com/your/mesh.git
cd mesh

# Build mesh-player natively on the RK3588S (4x A76 cores)
cargo build --release -p mesh-player
# ~3-5 min for full build, ~30-60s for incremental

# Stop the kiosk, run manually for testing
sudo systemctl stop cage-tty1
WGPU_BACKEND=gl ./target/release/mesh-player
```

**Optional: Cachix for faster CI/CD**

```bash
# Push aarch64 builds to a private Cachix cache
# In CI or after a successful local build:
cachix push mesh-embedded $(nix build .#mesh-player-aarch64 --print-out-paths)

# Board pulls from cache instead of building:
# Add to configuration.nix:
#   nix.settings.substituters = [ "https://mesh-embedded.cachix.org" ];
#   nix.settings.trusted-public-keys = [ "mesh-embedded.cachix.org-1:XXXX" ];
```

#### 6. Debugging & Troubleshooting

**Audio debugging:**

```bash
# List all sound cards — verify both appear
aplay -l
# Expected:
# card 0: rockchipes8388 [rockchip-es8388], device 0: ...
# card 1: PCM5102A [PCM5102A], device 0: ...

# Test master output (PCM5102A DAC)
speaker-test -D hw:1,0 -c 2 -t wav
# You should hear "Front Left", "Front Right" from the PA output

# Test cue output (ES8388 headphone)
speaker-test -D hw:0,0 -c 2 -t wav
# You should hear audio in headphones

# Test using named ALSA aliases
speaker-test -D mesh_master -c 2 -t wav
speaker-test -D mesh_cue -c 2 -t wav

# Check PipeWire sees both sinks
pw-cli list-objects | grep -A2 "alsa_output"

# Check WirePlumber routing
wpctl status

# Monitor ALSA in real-time (shows xruns/underruns)
cat /proc/asound/card1/pcm0p/sub0/status

# Adjust ES8388 headphone volume
amixer -c rockchipes8388 set Headphone 90%
amixer -c rockchipes8388 contents  # show all controls
```

**Display debugging:**

```bash
# Check connected displays
wlr-randr
# Shows resolution, refresh rate, and position of each output

# Check GPU driver
cat /sys/kernel/debug/dri/0/name    # should show "panfrost" or "panthor"
glxinfo | grep "OpenGL renderer"     # under X11
# Under Wayland: run `eglinfo` or check weston-info

# If display is black: check cage service
systemctl status cage-tty1
journalctl -u cage-tty1 --no-pager -n 50
```

**Device tree debugging:**

```bash
# Verify overlay loaded
ls /proc/device-tree/ | grep pcm5102a
# Should show: pcm5102a, pcm5102a-sound

# Check I2S3 status
cat /proc/device-tree/i2s@fe4a0000/status    # should say "okay"

# Decompile the live device tree (useful for debugging)
dtc -I fs -O dts /proc/device-tree/ 2>/dev/null | grep -A10 "i2s3"

# If overlay didn't load, check kernel log
dmesg | grep -i "i2s\|pcm5102\|simple-audio"
```

**General system debugging:**

```bash
# System logs (all services)
journalctl -b --no-pager | tail -100

# mesh-player specific logs
journalctl -u cage-tty1 -f              # live follow
journalctl -u cage-tty1 --since "5 min ago"

# CPU/memory usage
htop

# USB devices (verify USB stick, MIDI controller)
lsusb

# GPIO state (verify I2S pins are in correct mode)
cat /sys/kernel/debug/pinctrl/pinctrl-rockchip-pinctrl/pinmux-pins | grep -i i2s3

# Temperature monitoring
cat /sys/class/thermal/thermal_zone0/temp  # divide by 1000 for °C

# Network
ip addr              # verify WiFi connected
nmcli device wifi    # scan for networks
```

**Emergency recovery:**

```bash
# If the board won't boot / cage is broken:
# 1. Connect a USB keyboard + HDMI monitor
# 2. At the boot menu, select a previous NixOS generation
#    (NixOS keeps old generations, you can roll back)

# Or: hold the board's MASKROM button during power-on to enter recovery mode
# Then re-flash the microSD with a known-good image

# SSH backdoor: even if cage crashes, SSH stays up
# (cage-tty1 is a separate service from sshd)
ssh mesh@<board-ip>
sudo systemctl restart cage-tty1

# Roll back to previous generation remotely
sudo nixos-rebuild switch --rollback
```

#### 7. Update Workflow (Ongoing)

```bash
# On your x86 workstation, after updating mesh-player code:

# 1. Commit changes to mesh repo
git add -A && git commit -m "fix: whatever"

# 2. Deploy to the board
nixos-rebuild switch \
  --flake .#mesh-embedded \
  --target-host mesh@192.168.1.100 \
  --use-remote-sudo

# 3. If something breaks, roll back instantly:
nixos-rebuild switch --rollback \
  --target-host mesh@192.168.1.100 \
  --use-remote-sudo

# NixOS atomic updates mean:
# - Old generation is preserved (instant rollback)
# - No partial updates (either the whole system switches or nothing does)
# - The board is never in a broken state during update
# - Even power loss during update is safe (old generation still works)
```

**OTA updates over WiFi:**

The board connects to WiFi automatically (configured in NixOS). From anywhere on the same network:

```bash
nixos-rebuild switch --flake .#mesh-embedded --target-host mesh@mesh-embedded.local --use-remote-sudo
```

For remote boards (different network), use a VPN (WireGuard or Tailscale — both trivial to add in NixOS):

```nix
# In configuration.nix:
services.tailscale.enable = true;
# Then: nixos-rebuild switch --target-host mesh@100.x.x.x --use-remote-sudo
```

## I2S DAC & Onboard Audio Research

### Goal

Eliminate the external USB audio interface (Behringer UMC204HD, ~65 EUR, 185mm wide) by using onboard I2S/codec resources for 4-channel output (2 master + 2 headphone cue). Target: under 60 EUR total audio cost, zero external boxes.

### RK3588 I2S Hardware Capabilities

The RK3588 SoC contains **4 I2S/PCM/TDM controllers**:

| Controller | Channels | Base Address | Notes |
|------------|----------|-------------|-------|
| **I2S0_8CH** | 8ch TX + 8ch RX | `0xfe470000` | Used by onboard codec on most boards |
| **I2S1_8CH** | 8ch TX + 8ch RX | `0xfe480000` | Available for external use |
| **I2S2_2CH** | 2ch TX + 2ch RX | `0xfe490000` | Routed to M.2 E-Key on some boards |
| **I2S3_2CH** | 2ch TX + 2ch RX | `0xfe4a0000` | Routed to 40-pin GPIO on OPi 5 Plus |

All controllers support: master/slave mode, 16-32 bit resolution, 8-192 kHz sample rate, I2S/PCM/TDM formats. Each has independent clock trees derived from `PLL_AUPLL`, so they can run simultaneously at different sample rates. All fall under the `RK3588_PD_AUDIO` power domain.

Additionally: 2x PDM controllers, 1x SPDIF TX/RX, 1x VAD (Voice Activity Detection), 1x Audio PWM.

Sources: [RK3588 TRM Part 1](https://github.com/FanX-Tek/rk3588-TRM-and-Datasheet), [I2S support kernel patches](https://patchew.org/linux/20230315114806.3819515-1-cristian.ciocaltea@collabora.com/)

### Board-Specific I2S & Audio Breakdown

#### Orange Pi 5 Plus

| Feature | Detail |
|---------|--------|
| **Onboard codec** | Everest ES8388 (24-bit, 96 kHz) |
| **Codec I2S bus** | **I2S0** (`i2s0_8ch`), controlled via I2C7 at address `0x11` |
| **MCLK** | `I2S0_8CH_MCLKOUT` @ 12.288 MHz |
| **3.5mm jack** | Yes — TRRS (stereo headphone + mono mic) |
| **Speaker header** | Mono, driven by AWINIC AW8733ATQR (2W Class K) |
| **HDMI audio** | HDMI 2.1 eARC on both outputs |
| **GPIO I2S** | **I2S3** (`i2s3_2c`) on pins 12, 31, 35, 38, 40 |
| **Other I2S on GPIO** | **No** — only I2S3 confirmed on 40-pin header |
| **ALSA devices** | `es8388-sound` (headphone) + `dp0-sound`/`dp1-sound` (HDMI) |

The I2S3 pins on the GPIO header have been community-verified working with a PCM5102A DAC via device tree overlay on ubuntu-rockchip builds. The overlay targets `i2s3_2c` and uses the `simple-audio-card` framework.

Sources: [GitHub ubuntu-rockchip Discussion #1116](https://github.com/Joshua-Riek/ubuntu-rockchip/discussions/1116), [Armbian Forum](https://forum.armbian.com/topic/32178-i2s-spi-and-i2c-on-orangepi-5-plus/), [OPi 5 Plus Wiki](http://www.orangepi.org/orangepiwiki/index.php/Orange_Pi_5_Plus)

#### Orange Pi 5 Max

| Feature | Detail |
|---------|--------|
| **Onboard codec** | Everest ES8388 (24-bit, 96 kHz) — same as OPi 5 Plus |
| **Codec I2S bus** | **I2S0** (`i2s0_8ch`), controlled via I2C |
| **3.5mm jack** | Yes — TRRS (stereo headphone + mono mic) |
| **Onboard mic** | Yes |
| **HDMI audio** | HDMI 2.1 eARC on both outputs |
| **GPIO I2S** | **I2S3** (`i2s3_2c`) on pins 12, 35, 38, 40 — same as OPi 5 Plus |
| **WiFi/BT** | **Built-in** WiFi 6E + BT 5.3 (no M.2 E-Key module needed) |
| **Board size** | **89×57mm** (credit-card, fits behind 7" display) |
| **NVMe** | M.2 PCIe 3.0 x4 |
| **ALSA devices** | `es8388-sound` (headphone) + `dp0-sound`/`dp1-sound` (HDMI) |

Confirmed: the OPi 5 Max uses the same RK3588 SoC and the same ES8388 codec as the OPi 5 Plus. The 40-pin GPIO header exposes I2S3 on the same pins. The PCM5102A device tree overlay from the OPi 5 Plus works without modification.

Sources: [Orange Pi 5 Max Product Page](http://www.orangepi.org/html/hardWare/computerAndMicrocontrollers/details/Orange-Pi-5-Max.html), [CNX Software Review](https://www.cnx-software.com/2024/08/01/rockchip-rk3588-powered-orange-pi-5-max-sbc-features-up-to-16gb-lpddr5-2-5gbe-onboard-wifi-6e-and-bluetooth-5-3/), [Armbian Forum — OPi 5 Max I2S](https://forum.armbian.com/topic/51422-orange-pi-5-max-enabling-i2s-for-pcm/)

#### Radxa Rock 5B / 5B+

| Feature | Detail |
|---------|--------|
| **Onboard codec** | Everest ES8316 (24-bit, 96 kHz) |
| **Codec I2S bus** | **I2S0** (`i2s0_8ch`, address `0xfe470000`) |
| **3.5mm jack** | Yes — 4-ring TRRS (stereo HP + mic), drives 32 ohm directly |
| **GPIO I2S** | Limited — Radxa "ran out of GPIO on 5B" |
| **I2S on M.2 E-Key** | **I2S2** accessible via M.2 E-Key connector |
| **Community request** | Dedicated I2S header requested but not implemented |

Sources: [Rock 5B GPIO Wiki](https://wiki.radxa.com/Rock5/hardware/5b/gpio), [Rock 5B I2S Header Discussion](https://forum.radxa.com/t/rock-5b-new-i2s-header/10646), [Rock 5B+ Schematic](https://dl.radxa.com/rock5/5b+/docs/hw/radxa_rock5bp_v1.2_schematic.pdf)

#### Radxa Rock 5T

| Feature | Detail |
|---------|--------|
| **Onboard codec** | ES8316 (likely, same as Rock 5B/5B+ — same RK3588 platform) |
| **3.5mm jack** | Yes — 4-ring TRRS (stereo HP + mic) |
| **GPIO I2S** | 40-pin header lists "1x PCM/I2S" |
| **Schematic** | [Rock 5T V1.2 Schematic PDF](https://dl.radxa.com/rock5/5t/docs/hw/radxa_rock5t_schematic_v1.2_20250109.pdf) |

The Rock 5T product brief lists the 40-pin header as supporting: 2x UART, 2x SPI, 2x I2C, **1x PCM/I2S**, 1x SPDIF, 1x PWM, 1x ADC, 6x GPIO. Only one I2S bus is exposed.

Sources: [Rock 5T Product Brief](https://dl.radxa.com/rock5/5t/docs/radxa_rock5t_product_brief.pdf), [Radxa Docs](https://docs.radxa.com/en/rock5/rock5t/getting-started/introduction)

### RPi GPIO Pinout Compatibility

The RK3588 boards use a 40-pin header that is **broadly compatible** with the Raspberry Pi layout for power (pins 1-2, 4, 6, 9, 14, 17, 20, 25, 30, 34, 39 = GND/3.3V/5V) but the alternate functions on signal pins are **different**. RPi I2S uses pins 18 (BCK), 19 (LRCK), 20 (GND), 21 (DOUT) — on RK3588 boards these may be assigned to entirely different peripherals. RPi I2S HATs/pHATs will **not** work without rewiring or a custom adapter board.

### Onboard Codec Audio Quality

| Spec | ES8388 (OPi 5 Plus) | ES8316 (Rock 5B/5T) | PCM5102A (reference) |
|------|---------------------|---------------------|---------------------|
| DAC SNR | 96 dB (typ) | 93 dB (typ) | **112 dB** |
| DAC THD+N | -83 dB (typ) to -100 dB | -85 dB (typ) | **-93 dB** |
| DAC Dynamic Range | 96 dB | 93 dB | **112 dB** |
| ADC SNR | 95 dB | 92 dB | N/A |
| Max Sample Rate | 96 kHz | 96 kHz | **384 kHz** |
| Bit Depth | 24-bit | 24-bit | 32-bit |
| HP Amp | Integrated (40 mW) | Integrated (ground-centered) | External needed |
| Package | QFN-28 | QFN-28 | TSSOP-20 |

**Assessment**: The onboard codecs (ES8388/ES8316) are adequate for headphone cueing — 93-96 dB SNR is comparable to mid-range consumer headphone outputs and well above the noise floor of a DJ booth. They are NOT hi-fi quality but perfectly serviceable for monitoring/preview purposes. The PCM5102A at 112 dB SNR is genuinely good for a master output feeding a PA system.

**Known ES8388 headphone output limitations on Orange Pi boards (Feb 2026 testing):**

- **Bass roll-off**: The headphone output is AC-coupled through small electrolytic capacitors on the PCB (typically 22µF/6.3V in 0603 packages, based on OPi 4 LTS schematic — similar circuit). These form a high-pass filter with headphone impedance: `f_c = 1 / (2π × C × Z)`. With 32Ω DJ headphones, -3dB @ ~220 Hz — significant bass loss. With 250Ω studio headphones, -3dB @ ~29 Hz — inaudible. This is a board-level hardware limitation, not a codec limitation.
- **Noise**: The ES8388's analog section shares the power domain with the RK3588S SoC. Digital switching noise from the 8-core processor couples into the analog output, raising the effective noise floor above the chip's 96 dB SNR datasheet spec. Professional audio interfaces use isolated power domains and careful analog PCB layout to avoid this.
- **No software EQ compensation recommended**: Boosting bass in software to compensate the analog roll-off drives the codec harder into distortion/noise — the signal is already attenuated in the analog domain before reaching the jack.
- **Upgrade path**: A USB DAC dongle (e.g., Apple USB-C adapter with Cirrus Logic CS43131, ~$15, 112 dB SNR) eliminates both issues. Higher impedance headphones (150-250Ω) mitigate the bass roll-off at zero cost.

Sources: [ES8388 Datasheet](https://datasheet.lcsc.com/lcsc/1912111437_Everest-semi-Everest-Semiconductor-ES8388_C365736.pdf), [ES8316 Datasheet](http://everest-semi.com/pdf/ES8316%20PB.pdf), [PCM5102A Datasheet](https://www.ti.com/product/PCM5102A)

### I2S DAC Options

#### PCM5102A Breakout Boards (~3-8 EUR)

The Texas Instruments PCM5102A is the go-to cheap I2S DAC:

- **112 dB SNR**, -93 dB THD+N, 2.1 Vrms output
- **No control interface** — no I2C/SPI needed, just 3 I2S wires (BCK, LRCK, DIN) + power
- Internal PLL generates MCLK from BCK — tie SCK pin to GND
- 16/24/32-bit, up to 384 kHz
- DirectPath charge-pump: ground-centered output, no DC blocking caps needed
- Available as GY-PCM5102 pHAT-format boards (3-6 EUR on AliExpress) or Adafruit breakout (~$6 USD)
- Line-level 3.5mm or pin header output depending on board variant

Sources: [TI PCM5102A Product Page](https://www.ti.com/product/PCM5102A), [Adafruit PCM5102 Breakout](https://www.adafruit.com/product/6250)

#### ES9023 Breakout Boards (~8-15 EUR)

The ESS Sabre ES9023 is a step up in quality:

- **112 dB SNR** (matches PCM5102A), advanced Sabre DAC technology
- 24-bit, 192 kHz
- No control interface (like PCM5102A — I2S only)
- RCA output on most boards
- Higher price (~8-15 EUR for basic boards, ~70 EUR for premium Hifimediy version)
- Less commonly available than PCM5102A

Sources: [Audiophonics ES9023 V2.1](https://www.audiophonics.fr/en/dac-and-interfaces-for-raspberry-pi/audiophonics-dac-sabre-es9023-v21-i2s-to-analogue-24bit-192khz-p-8396.html), [Hifimediy ES9023](https://hifimediy.com/product/hifimediy-es9023-i2s-dac-for-raspberry-pi-mod-b-192khz24bit/)

### Dual-DAC Approach Analysis (2x I2S DACs on separate buses)

**The problem**: On all researched boards, only **one I2S bus** is exposed on the 40-pin GPIO header. The onboard codec consumes I2S0, and only I2S3 (OPi 5 Plus) or limited I2S signals (Rock 5B/5T) are available on GPIO.

| Board | I2S0 | I2S1 | I2S2 | I2S3 | Dual I2S on GPIO? |
|-------|------|------|------|------|-------------------|
| OPi 5 Plus | Onboard ES8388 | Not on header | Not on header | **On GPIO (pins 12,31,35,38,40)** | **No** |
| Rock 5B/5B+ | Onboard ES8316 | Not on header | On M.2 E-Key | Limited | **No** |
| Rock 5T | Onboard ES8316 | Not on header | Unknown | On GPIO (1x PCM/I2S) | **No** |

**Verdict: True dual-I2S via GPIO is not feasible** on any of these boards. The SoC has 4 I2S buses, but board designers only route one to the 40-pin header due to pin constraints.

**Theoretical workarounds** (all impractical):
1. Rock 5B M.2 E-Key I2S2 + GPIO I2S: Requires custom M.2 breakout board — fragile, defeats simplicity goal
2. 8-channel TDM on I2S1: If I2S1 pins were accessible, could drive 2x stereo DACs from one bus in TDM mode — but I2S1 is not on the header
3. Custom carrier board: Design a board that breaks out I2S0 + I2S3 — overkill for this project

### Combined Approach: Onboard Codec + I2S DAC (RECOMMENDED)

**This is the winning strategy.** Use the onboard 3.5mm jack (ES8388/ES8316 via I2S0) for headphone cueing + one I2S DAC (PCM5102A via I2S3) for master output. This gives 4 independent channels from 2 separate ALSA devices.

#### Architecture

```
RK3588 SoC
├── I2S0 → ES8388/ES8316 → 3.5mm TRRS jack → Headphones (CUE output, ch 3-4)
│                          ALSA: card "es8388-sound" or "rk3588-es8316"
│
├── I2S3 → PCM5102A DAC → 3.5mm/RCA line out → PA System (MASTER output, ch 1-2)
│          (via GPIO header)  ALSA: card "pcm5102a-sound"
│
├── I2S1 → (not routed to GPIO)
└── I2S2 → (M.2 E-Key on some boards, unused)
```

#### Bill of Materials

| Component | Cost (EUR) | Notes |
|-----------|-----------|-------|
| PCM5102A GY-PCM5102 board | 3-6 | AliExpress/eBay, pHAT format |
| Dupont jumper wires (5x) | 1-2 | BCK, LRCK, DIN, VCC, GND |
| **Total** | **4-8** | vs. 65 EUR for Behringer UMC204HD |

**Savings: ~57-61 EUR** compared to the USB audio interface approach.

#### Wiring (Orange Pi 5 Plus)

| PCM5102A Pin | OPi 5 Plus 40-Pin | GPIO | I2S3 Signal |
|-------------|-------------------|------|-------------|
| BCK | Pin 35 | GPIO3_C2 | I2S3_SCLK |
| LRCK | Pin 38 | GPIO3_C0 | I2S3_LRCK_TX |
| DIN | Pin 40 | GPIO3_B7 | I2S3_SDO |
| SCK | Tie to GND | — | Internal PLL |
| VIN | Pin 1 (3.3V) | — | Power |
| GND | Pin 6 (GND) | — | Ground |

Pin 12 (GPIO4_A6) = I2S3_MCLK — not needed by PCM5102A (has internal PLL), but available if using a DAC that requires external MCLK.

#### Device Tree Overlay (Orange Pi 5 Plus)

```dts
/dts-v1/;
/plugin/;

/ {
    compatible = "xunlong,orangepi-5-plus", "rockchip,rk3588";

    fragment@0 {
        target = <&i2s3_2ch>;
        __overlay__ {
            status = "okay";
            #sound-dai-cells = <0>;
            pinctrl-names = "default";
            pinctrl-0 = <&i2s3m0_sclk &i2s3m0_lrck &i2s3m0_sdo>;
            rockchip,playback-channels = <2>;
        };
    };

    fragment@1 {
        target-path = "/";
        __overlay__ {
            pcm5102a_codec: pcm5102a {
                compatible = "ti,pcm5102a";
                #sound-dai-cells = <0>;
            };

            pcm5102a_sound: pcm5102a-sound {
                compatible = "simple-audio-card";
                simple-audio-card,name = "PCM5102A";
                simple-audio-card,format = "i2s";

                simple-audio-card,cpu {
                    sound-dai = <&i2s3_2ch>;
                };
                simple-audio-card,codec {
                    sound-dai = <&pcm5102a_codec>;
                };
            };
        };
    };
};
```

This overlay has been community-tested on Orange Pi 5 Plus with ubuntu-rockchip. The PCM5102A appears in ALSA as `alsa_output.platform-pcm5102a-sound.stereo-fallback`.

#### ALSA/PipeWire Configuration

After both cards are active, `aplay -l` shows:

```
card 0: rockchipes8388 [rockchip-es8388], device 0: ...  (onboard headphone jack)
card 1: PCM5102A [PCM5102A], device 0: ...               (GPIO I2S DAC)
```

In mesh-player, configure ALSA device routing:
- Master output (ch 1-2) → `hw:1,0` (PCM5102A → PA system)
- Cue output (ch 3-4) → `hw:0,0` (ES8388 → headphones)

With PipeWire, both sinks appear independently and can be assigned to different application output ports.

#### Advantages Over USB Audio

| Aspect | USB Interface (UMC204HD) | Onboard + I2S DAC |
|--------|-------------------------|-------------------|
| Cost | ~65 EUR | ~5 EUR |
| External hardware | Yes (185×130mm box + USB cable) | No (DAC board inside enclosure) |
| Latency | USB + ALSA buffering | **Direct I2S — zero USB overhead** |
| Reliability | USB enumeration can fail | **Always present at boot** |
| Power | Bus-powered via USB | Board 3.3V rail |
| Master quality (SNR) | ~100 dB (UMC204HD) | **112 dB (PCM5102A)** |
| Cue quality (SNR) | ~100 dB (same device) | ~93-96 dB (onboard codec) |
| Form factor | External box | PCB inside enclosure |

#### Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| DTS overlay not working on NixOS | Medium | Test on ubuntu-rockchip first; NixOS can load arbitrary DTBO files via `hardware.deviceTree.overlays` |
| ES8388 headphone output too noisy | Low | 96 dB SNR is fine for DJ cueing; upgrade to USB DAC later if needed |
| PCM5102A board quality variance | Low | Test with oscilloscope; buy from Adafruit if cheap boards are noisy |
| Pin conflict with other GPIO usage | Low | I2S3 pins (31,35,38,40) don't overlap with common I2C/SPI/UART pins |
| No onboard headphone amp gain control | Low | ES8388 has software-controllable gain via I2C; driver handles this |

### Summary & Recommendation

**Use the combined approach** (onboard codec for cue + PCM5102A on I2S3 for master):

- **Orange Pi 5 Pro 8GB** is the primary pick (updated Feb 2026) — cheapest board with ES8388 + I2S3 GPIO, built-in WiFi 5 + BT 5.0, 89×56mm credit-card size, LPDDR5, $80
- **Orange Pi 5 Max 16GB** is the upgrade pick — WiFi 6E, PCIe 3.0 x4 NVMe, same form factor, $145
- **Orange Pi 5 Plus 16GB** remains a solid alternative — confirmed I2S3 on GPIO, confirmed ES8388, community-verified PCM5102A overlay, largest community, $142+$15 WiFi
- **Rock 5T/5B+** can work but GPIO I2S routing is less documented; the ES8316 on these boards is slightly lower quality than the OPi's ES8388
- Total audio cost: **~5 EUR** (one PCM5102A board + wires) vs. 65 EUR for the cheapest viable USB interface
- Master output quality: **better** than the USB approach (112 dB PCM5102A vs. ~100 dB UMC204HD)
- Zero external audio hardware — the entire audio path fits inside the SBC enclosure
- I2S is fundamentally lower-latency than USB audio — no 1ms USB frame interval, direct synchronous serial bus

#### OPi 5 Pro vs OPi 5 Plus vs OPi 5 Max for Mesh

| Spec | OPi 5 Pro (primary) | OPi 5 Plus | OPi 5 Max |
|---|---|---|---|
| **SoC** | RK3588S | RK3588 (full) | RK3588 (full) |
| **RAM** | Up to 16GB LPDDR5 | Up to 32GB LPDDR4x | Up to 16GB LPDDR5 |
| **NVMe** | PCIe 2.1 (~800 MB/s) | PCIe 3.0 x4 (~3500 MB/s) | PCIe 3.0 x4 (~3500 MB/s) |
| **HDMI out** | 1x 2.1 + 1x 2.0 | 2x 2.1 | 2x 2.1 |
| **Ethernet** | 1x GbE | 2x 2.5GbE | 1x 2.5GbE |
| **WiFi/BT** | Built-in WiFi 5 + BT 5.0 | M.2 E-Key (add ~$15) | Built-in WiFi 6E + BT 5.3 |
| **USB** | 1x USB 3.1 + 3x USB 2.0 | 2x USB 3.0 + 2x USB 2.0 | 2x USB 3.0 + 2x USB 2.0 |
| **Audio codec** | ES8388, 3.5mm TRRS | ES8388, 3.5mm TRRS | ES8388, 3.5mm TRRS |
| **I2S3 on GPIO** | Same pins (12/35/38/40) | Same pins | Same pins |
| **Board size** | 89×56mm | 100×70mm | 89×57mm |
| **Power** | ~6-12W | ~8-15W | ~8-15W |
| **Price (8GB)** | **~$80** | ~$100 | ~$105 |
| **Price (16GB)** | ~$109 | ~$142 | ~$145 |

8 GB RAM is sufficient for mesh-player. Worst-case analysis (4 decks × 4 stems, 7-min tracks, heavy FX): ~3.7 GB used, leaving 4.3 GB free. NVMe is optional — DJs play from USB 3.0 sticks (100-150 MB/s, ~4% utilization at 5.5 MB/s sustained read for 4 decks × 4 stems). PCIe 2.1 vs 3.0 is irrelevant when NVMe is not the primary storage path.

### Primary BOM: Orange Pi 5 Pro 8GB + I2S DAC (Updated Feb 2026)

#### Audio Architecture

```
RK3588S SoC (Orange Pi 5 Pro)
│
├── I2S0 → ES8388 codec → 3.5mm TRRS jack → HEADPHONES (CUE)
│          (onboard, free)    96 dB SNR         ALSA: "es8388-sound"
│
└── I2S3 → PCM5102A DAC  → 3.5mm/RCA out  → PA SYSTEM (MASTER)
           (GPIO header)     112 dB SNR         ALSA: "pcm5102a-sound"
           ~$5               better than any USB interface under $200
```

DJ workflow: tracks loaded from USB 3.0 stick (brought by DJ), same as CDJ workflow.

#### GPIO Wiring (6 wires, identical across all OPi 5 boards)

| PCM5102A Pin | 40-Pin Header | GPIO | Signal |
|---|---|---|---|
| BCK | Pin 35 | GPIO3_C2 | I2S3_SCLK |
| LRCK | Pin 38 | GPIO3_C0 | I2S3_LRCK_TX |
| DIN | Pin 40 | GPIO3_B7 | I2S3_SDO |
| SCK | Tie to GND | — | Internal PLL |
| VIN | Pin 1 | — | 3.3V power |
| GND | Pin 6 | — | Ground |

#### Core Bill of Materials (required)

| # | Component | Spec | Price |
|---|---|---|---|
| 1 | **Orange Pi 5 Pro 8GB** | RK3588S, LPDDR5, WiFi 5, BT 5.0, 89×56mm | **$80** |
| 2 | **microSD card** | 32GB A2 U3 (OS boot only) | **$8** |
| 3 | **GY-PCM5102 I2S DAC** | PCM5102A, 112 dB SNR, 32-bit/384kHz | **$5** |
| 4 | **Dupont jumper wires** | Female-to-female, 6 pcs, 10-15cm | **$1** |
| 5 | **USB-C PSU** | 5V/5A Type-C | **$12** |
| 6 | **Micro-HDMI cable** | 15-30cm, board → display | **$6** |
| | | **Core total** | **$112** |

Onboard ES8388 cue headphone output via 3.5mm TRRS = $0 (included on board).

#### Enclosure & Thermal

| # | Component | Spec | Price |
|---|---|---|---|
| 7 | **Aluminum project box** | ~150×120×50mm | **$25** |
| 8 | **M2.5 standoff kit** | Brass, board mount | **$5** |
| 9 | **40mm Noctua fan** | 5V PWM | **$12** |
| 10 | **Thermal pad** | SoC heatsink contact | **$3** |
| 11 | **Panel-mount 3.5mm** (x2) | Master out + cue out | **$4** |
| 12 | **Panel-mount USB-A** | Pass-through for MIDI controller / USB stick | **$4** |
| | | **Enclosure total** | **$53** |

#### Optional Upgrades

| # | Component | Spec | Price |
|---|---|---|---|
| 13 | NVMe SSD | M.2 2280, 500GB–1TB (built-in library) | ~$40-55 |
| 14 | 7" IPS touchscreen | 1024×600, HDMI + USB capacitive touch | ~$40 |
| 15 | Powered USB 3.0 hub | 4-port (multiple USB devices) | ~$15 |
| 16 | RTC battery | CR2032 + holder | ~$2 |
| 17 | OPi 5 Max 16GB (upgrade) | WiFi 6E, PCIe 3.0 x4, +$65 over Pro 8GB | ~$145 |

#### Cost Summary

| Build Tier | Includes | Total |
|---|---|---|
| **Board + audio only** | OPi 5 Pro 8GB + SD + DAC + wires + PSU + HDMI cable | **~$112** |
| **Full enclosed unit** | + case, fan, thermal, panel mounts | **~$165** |
| **With NVMe** | + 1TB SSD (optional built-in library) | **~$220** |
| **With display** | + 7" touchscreen (if needed) | **~$205 / $260** |

#### Legacy Options (preserved for reference)

##### Option A (original): OPi 5 Plus + USB Audio (~$350-380)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Plus 16GB | ~$129 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB (track library) | ~$40 |
| Display | 2x 7" HDMI IPS touchscreen (1024×600) | ~$80 |
| Audio | Behringer UMC204HD (4ch, USB-B) | ~$65 |
| Enclosure | Custom 3D printed or aluminum case | ~$20-50 |
| **Total** | | **~$350-380** |

## Small HDMI Display Research

### Goal

Find compact HDMI screens to mount on top of a DJ controller enclosure. The SBC (Orange Pi 5 Plus, 100x70mm) and a Traktor F1 controller (120x294mm) define the available mounting surface. Screens must work on Linux via standard HDMI (no proprietary drivers), be IPS for good downward viewing angles, and cost under 40 EUR each.

### Size Context

| Reference Device | Width (mm) | Depth (mm) |
|-----------------|------------|------------|
| Orange Pi 5 Plus | 100 | 70 |
| Traktor F1 | 120 | 294 |

### Category 1: 3.5" HDMI Screens (~80x55mm)

These are the smallest practical HDMI screens. Good for a single-deck status display but limited for waveforms.

#### Waveshare 3.5" HDMI LCD (Standard)

| Spec | Value |
|------|-------|
| Resolution | 480x320 |
| Panel | IPS |
| Viewing Angle | 160 degrees |
| Touch | Resistive (SPI) |
| Display Interface | HDMI (mini connector on board) |
| Audio | 3.5mm jack (HDMI audio output) |
| Weight | 142g |
| Overall Dimensions | ~85x56mm (PCB, from dimension drawing) |
| Brightness | Not specified (OSD adjustable) |
| Contrast | Not specified |
| Refresh Rate | 60Hz |
| Price | ~36 USD (~33 EUR) / ~50 EUR on Amazon.de |
| Linux | Driver-free HDMI, touch needs SPI GPIO |

**Assessment**: 480x320 is too low for useful waveform display. Resistive touch is poor. The IPS panel and compact size are good, but resolution is the dealbreaker.

Sources: [Waveshare Product Page](https://www.waveshare.com/3.5inch-hdmi-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/3.5inch_HDMI_LCD)

#### Waveshare 3.5" HDMI LCD (E) - Capacitive

| Spec | Value |
|------|-------|
| Resolution | 640x480 |
| Panel | IPS |
| Viewing Angle | 170 degrees |
| Touch | 5-point capacitive (USB-C or I2C) |
| Display Interface | HDMI |
| Audio | 3.5mm jack + 4-pin header |
| Weight | 195g |
| Brightness | Not specified |
| Contrast | Not specified |
| Refresh Rate | 60Hz |
| Toughened Glass | 6H hardness |
| Price | ~44 USD (~40 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: Much better than the standard model. 640x480 is adequate for a compact single-deck display. Capacitive touch via USB-C is driver-free on Linux. At ~40 EUR it sits right at the budget limit. Good candidate for a compact two-screen setup (one per deck).

Sources: [Waveshare Product Page](https://www.waveshare.com/3.5inch-hdmi-lcd-e.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/3.5inch_HDMI_LCD_(E))

#### Waveshare 3.5" 480x800 LCD

| Spec | Value |
|------|-------|
| Resolution | 480x800 (portrait native) |
| Panel | IPS |
| Touch | 5-point capacitive |
| Display Interface | HDMI |
| Refresh Rate | 60Hz |
| Price | ~30 USD (~27 EUR) |

**Assessment**: Portrait-native 480x800 is interesting -- rotated to landscape it becomes 800x480, which is usable for waveforms. However, software rotation may add latency. Worth considering if the compact form factor is needed.

Sources: [Waveshare Product Page](https://www.waveshare.com/3.5inch-480x800-lcd.htm)

#### Newhaven Display NHD-3.5-HDMI-HR-RSXP

| Spec | Value |
|------|-------|
| Resolution | 640x480 |
| Panel | IPS |
| Viewing Angle | Full (IPS) |
| Touch | Non-touch / Capacitive variant (CTU) |
| Brightness | 810-950 cd/m2 (sunlight readable) |
| Display Interface | HDMI + micro-USB power |
| Physical Dimensions | ~77x64x3.2mm (panel only, PCB larger) |
| EMI Shielding | Yes |
| PWM Backlight | Yes |
| Price | ~57 USD (~52 EUR) |

**Assessment**: Industrial-grade, excellent brightness and build quality. The sunlight-readable 950 nit backlight is overkill for DJ use but nice to have. Over budget at ~52 EUR. Best-in-class 3.5" panel but priced for embedded/industrial markets.

Sources: [Newhaven Display Product Page](https://newhavendisplay.com/3-5-inch-ips-high-resolution-hdmi-tft-module/), [DigiKey Listing](https://www.digikey.com/en/products/detail/newhaven-display-intl/NHD-3-5-HDMI-HR-RSXP/26694315)

#### 3.5" Category Summary

The Waveshare 3.5" (E) at 640x480 with capacitive touch is the best option in this size class. At ~40 EUR it is tight on budget. The 480x320 standard model should be avoided due to resolution. The 3.5" size fits well on the OPi 5 Plus (85x56mm PCB on a 100x70mm board).

### Category 2: 4" to 4.3" HDMI Screens (~95x65mm)

This is the sweet spot for fitting on top of the Orange Pi 5 Plus (100x70mm).

#### Waveshare 4" HDMI LCD (480x800)

| Spec | Value |
|------|-------|
| Resolution | 480x800 (portrait native, landscape = 800x480) |
| Panel | IPS |
| Viewing Angle | 170 degrees |
| Touch | Resistive (SPI) |
| Display Interface | HDMI |
| Audio | 3.5mm jack (HDMI audio) |
| Weight | 128g |
| Price | ~38 USD (~35 EUR) |
| Linux | Driver-free HDMI, touch needs GPIO/SPI |

**Assessment**: Portrait-native 480x800 means 800x480 in landscape. Resistive touch is a negative but the display itself is decent. At ~35 EUR it is good value. The 4" form factor should fit within the OPi5+ footprint. Panel dimension drawing available as [PDF from Waveshare](https://www.waveshare.com/wiki/File:4inch_HDMI_LCD_panel_dimension.pdf).

Sources: [Waveshare Product Page](https://www.waveshare.com/4inch-hdmi-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/4inch_HDMI_LCD)

#### Waveshare 4" HDMI LCD (C) - 720x720 Square

| Spec | Value |
|------|-------|
| Resolution | 720x720 (square) |
| Panel | IPS |
| Touch | 5-point capacitive |
| Optical Bonding | Yes |
| Toughened Glass | 6H hardness |
| Audio | 3.5mm jack + speaker header |
| Price | ~66-76 USD (~60-70 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: The square 720x720 format is unique but not ideal for waveform display (waveforms are inherently wide). Resolution is good. Way over budget at ~60-70 EUR. Would be interesting for a status/mixer display but not recommended for this project.

Sources: [Waveshare Product Page](https://www.waveshare.com/4inch-hdmi-lcd-c.htm)

#### Waveshare 4.3" HDMI LCD (B) - Capacitive

| Spec | Value |
|------|-------|
| Resolution | 800x480 |
| Panel | IPS |
| Viewing Angle | 160 degrees |
| Touch | 5-point capacitive (USB micro-B) |
| Display Interface | HDMI |
| Audio | 3.5mm jack + speaker header |
| Toughened Glass | 6H hardness |
| Weight | 259g |
| Overall Dimensions | ~121x76mm (estimated from typical measurements) |
| Active Area | ~96x57mm (estimated) |
| Brightness | OSD adjustable (value not specified) |
| Contrast | Not specified |
| Price | ~50 USD (~46 EUR) / ~55 EUR on Amazon.de |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: This is the main contender in the 4" class. 800x480 IPS with capacitive touch via USB (driver-free on Linux) is solid. However, at ~121x76mm it slightly overhangs the OPi 5 Plus (100x70mm) on both axes. The price (~46 EUR) is slightly over the 40 EUR target. Very popular for Raspberry Pi projects -- huge community support.

Sources: [Waveshare Product Page](https://www.waveshare.com/4.3inch-hdmi-lcd-b.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/4.3inch_HDMI_LCD_(B))

#### Pimoroni HyperPixel 4.0 (NOT HDMI)

| Spec | Value |
|------|-------|
| Resolution | 800x480 |
| Panel | IPS |
| Viewing Angle | 160 degrees |
| Touch | Capacitive (optional) |
| Interface | **DPI (GPIO) -- NOT HDMI** |
| Dimensions | 58.5x97x12mm |
| Active Area | 86.4x51.8mm |
| Color Depth | 18-bit |
| Refresh Rate | 60 FPS |
| Price | ~35-45 GBP (~40-52 EUR) |

**Assessment**: DISQUALIFIED. Despite being listed as "HDMI-like speed", the HyperPixel 4.0 uses the DPI interface via GPIO pins, not HDMI. It is Raspberry Pi-specific and will NOT work with the Orange Pi 5 Plus. Included here for reference since it often appears in display searches.

Sources: [Pimoroni Product Page](https://shop.pimoroni.com/en-us/products/hyperpixel-4), [Pimoroni GitHub](https://github.com/pimoroni/hyperpixel4)

#### 4-4.3" Category Summary

The Waveshare 4.3" HDMI LCD (B) is the best all-rounder but slightly exceeds both the size constraint (121x76mm vs. 100x70mm target) and budget (46 EUR vs. 40 EUR). The 4" 480x800 model is cheaper but has resistive touch. For the OPi5+ mounting scenario, the 4" models fit better physically but the 4.3" wins on specs. Consider the 4.3" (B) if the enclosure can accommodate the slight overhang.

### Category 3: 5" HDMI Screens (~120x80mm)

These are slightly too large for the OPi 5 Plus (100x70mm) but match the F1 controller width (120mm). Good compromise size.

#### Waveshare 5" HDMI LCD (H) V4 - Capacitive (RECOMMENDED)

| Spec | Value |
|------|-------|
| Resolution | 800x480 |
| Panel | IPS (assumed, consistent with H series) |
| Overall Dimensions | 121.00(H) x 89.48(V) mm |
| Active Area | 108.00(H) x 64.80(V) mm |
| Color Gamut | 50% NTSC |
| Brightness | 380 cd/m2 |
| Contrast | 350:1 |
| Refresh Rate | 60Hz |
| Touch | 5-point capacitive (USB), 10-point on Windows |
| Toughened Glass | 6H hardness |
| Display Interface | Standard HDMI port |
| Power | 5V micro-USB, 2W consumption |
| Weight | ~310g (H model), V4 likely lighter |
| Price | ~30-35 USD (~27-32 EUR) / ~58 EUR on welectron.com |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: The V4 "slimmed-down" version is the best-value 5" option. At 121x89mm it overhangs the OPi 5 Plus by ~21mm on one axis, but matches the F1 controller width perfectly (120mm). The 380 cd/m2 brightness is good for indoor DJ use. 800x480 is the minimum acceptable resolution for waveforms. Capacitive touch via USB is driver-free on Linux. The V4 has lower power consumption (2W) than older versions. At ~30 EUR from Chinese vendors this is well within budget.

Sources: [Waveshare Product Page](https://www.waveshare.com/5inch-hdmi-lcd-h-v4.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/5inch_HDMI_LCD_(H)_V4), [TME Specs](https://www.tme.com/us/en-us/details/wsh-14300/tft-displays/waveshare/14300/)

#### Waveshare 5" 720x1280 LCD - High Resolution

| Spec | Value |
|------|-------|
| Resolution | 720x1280 (portrait native, landscape = 1280x720) |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Touch | 5-point capacitive (USB-C) |
| Touch Area | 68.70(H) x 128.00(V) mm |
| Active Area | 62.10(H) x 110.40(V) mm |
| Brightness | 350 cd/m2 |
| Contrast | 800:1 |
| Optical Bonding | Yes |
| Toughened Glass | 6H hardness |
| Onboard Gyroscope | QMI8658C |
| Weight | 183g |
| Price | ~60 USD (~55 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: Excellent resolution (1280x720 in landscape) with optical bonding and wide viewing angles. The 178-degree IPS is ideal for looking down at a DJ controller. Portrait-native requires software rotation. Over budget at ~55 EUR but the resolution jump from 800x480 to 1280x720 is significant for waveform detail. The active area is actually small at 62x110mm due to the portrait orientation and thick bezels. Worth the premium if the enclosure design allows portrait mounting with rotation.

Sources: [Waveshare Product Page](https://www.waveshare.com/5inch-720x1280-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/5inch_720x1280_LCD)

#### Waveshare 5" HDMI AMOLED

| Spec | Value |
|------|-------|
| Resolution | 960x544 |
| Panel | AMOLED |
| Touch | 5-point capacitive |
| Optical Bonding | Yes |
| Toughened Glass | 6H hardness |
| Audio | 3.5mm jack |
| Price | ~80 USD (~73 EUR) |
| Burn-in Warning | Static content max 1 hour |

**Assessment**: AMOLED provides infinite contrast and vibrant colors, which would make waveforms look stunning. However: 960x544 is an odd resolution, the price is nearly double budget, and AMOLED burn-in is a real risk for a DJ application with semi-static UI elements (knobs, labels). NOT recommended for this use case.

Sources: [Waveshare Product Page](https://www.waveshare.com/5inch-hdmi-amoled.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/5inch_HDMI_AMOLED)

#### LESOWN P50C 5" Monitor

| Spec | Value |
|------|-------|
| Resolution | 800x480 |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Overall Dimensions | 123x79x12.1mm (non-touch) / 123x79x16.3mm (touch) |
| Active Area | 108.0(W) x 64.8(H) mm |
| Color Gamut | 50% NTSC |
| Brightness | 300 cd/m2 |
| Contrast | 800:1 |
| Response Time | 25ms |
| Refresh Rate | 60Hz |
| Interface | 1x mini-HDMI + 1x micro-USB (power) |
| Weight | 117g (non-touch) |
| Case | Aluminum VESA mount |
| Touch (P50C-T variant) | 5-point capacitive + dual speakers |
| Price | ~35-50 USD (~32-46 EUR) |
| Linux | Driver-free, plug and play |

**Assessment**: The aluminum VESA case is more durable than the Waveshare plastic frame. Very lightweight at 117g. The 800:1 contrast is better than the Waveshare H V4 (350:1) but brightness is lower (300 vs 380 cd/m2). The 58x49mm RPi mounting holes on the back could potentially align with the OPi5+ standoffs. At 123x79mm the footprint is very similar to the Waveshare 5" (121x89mm) but shorter in the vertical axis.

Sources: [LESOWN Product Page](https://www.lesown.com/products/manufacturer-mini-monitor-5-inch-monitor-metal-case-800x480-ips-5-lcd-display-vesa-mount-computer-monitor), [Amazon](https://www.amazon.com/Brightness-Monitor-800x480-Capacitive-Display/dp/B0B1LPN3BT)

#### VIEWMEI 5" IPS Monitor (1280x720)

| Spec | Value |
|------|-------|
| Resolution | 1280x720 |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Contrast | 1000:1 |
| Pixel Pitch | 0.254mm |
| Interface | Mini-HDMI + USB-C (power) |
| Built-in Speakers | Yes |
| Package Dimensions | 224x139x67mm |
| Weight | 472g (with packaging) |
| Price | ~25-35 USD (~23-32 EUR) |
| Linux | Driver-free |

**Assessment**: Excellent value -- 1280x720 IPS at ~25-35 USD is remarkably cheap. The 1000:1 contrast is the best in this size class. Built-in speakers add versatility. The package dimensions suggest the actual monitor is significantly smaller. No touch option. At under 30 EUR for 720p IPS, this is potentially the best bang-for-buck waveform display. Touch is missing but may not be needed if the F1 controller handles all input.

Sources: [Amazon](https://www.amazon.com/VIEWMEI-Monitor-Screen-Portable-Display/dp/B0D6NP484X), [Newegg](https://www.newegg.com/p/3C6-05M1-000H0)

#### Elecrow RC050S 5" HDMI

| Spec | Value |
|------|-------|
| Resolution | 800x480 |
| Panel | LCD (TFT, likely not IPS) |
| Overall Dimensions | 121.11x77.93x14.1mm |
| Active Area | 109x66mm |
| Refresh Rate | 60Hz |
| Touch | 5-point capacitive (USB) |
| Built-in Speaker | Yes |
| Cooling Fan | Yes (included) |
| Backlight Control | Yes (4-level switch) |
| Weight | 126g |
| Price | ~43 USD (~39 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: Compact and lightweight with useful extras (speaker, fan, backlight control). The 121x78mm footprint is slightly smaller than the Waveshare 5". At ~39 EUR it is right at the budget limit. The panel is likely TN rather than IPS, which is a concern for viewing angles in a top-mounted DJ display scenario. Verify panel type before purchasing.

Sources: [Elecrow Product Page](https://www.elecrow.com/rc050s-hdmi-5-inch-800x480-capacitive-touch-monitor-built-in-speaker-with-backlight-control.html), [Electronics Lab Review](https://www.electronics-lab.com/elecrow-rc050s-hd-review-a-5-800x480-hdmi-display-with-capacitive-touchscreen-and-speaker-for-diy-projects/)

#### OSOYOO 5" HDMI Capacitive Touch

| Spec | Value |
|------|-------|
| Resolution | 800x480 (adjustable up to 1920x1080) |
| Touch | 5-point capacitive (USB) |
| Overall Dimensions | 121x93x15mm |
| Weight | 127g |
| Audio | 3.5mm output |
| Backlight | 4-level adjustable |
| Working Temp | -5 to 38 degrees C |
| Price | ~25-35 USD (~23-32 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: Budget-friendly with capacitive touch and audio output. At 121x93mm it is taller (more vertical space) than the Waveshare and LESOWN options. The narrow operating temperature range (-5 to 38C) is a concern for outdoor festival use but fine for indoor venues. Good value at ~25 EUR.

Sources: [OSOYOO Product Page](https://osoyoo.com/2021/09/23/osoyoo-5-inch-hdmi-800-x-480-capacitive-touch-lcd-display/), [Amazon](https://www.amazon.com/OSOYOO-Capacitive-Raspberry-Compatible-Raspbian/dp/B09HZ7Q8DV)

#### 5" Category Summary

**Best value (no touch)**: VIEWMEI 5" 1280x720 at ~25-30 EUR. Highest resolution in this size class, excellent contrast (1000:1), but no touch input. Perfect if the DJ controller handles all interaction.

**Best value (with touch)**: Waveshare 5" HDMI LCD (H) V4 at ~30 EUR. Proven capacitive touch, huge community, 800x480 is adequate for waveforms.

**Best resolution (with touch)**: Waveshare 5" 720x1280 at ~55 EUR. 1280x720 in landscape mode with optical bonding. Over budget but the resolution difference is noticeable.

**Best build quality**: LESOWN P50C at ~35 EUR. Aluminum case, VESA mount, lightest weight (117g).

### Category 4: 7" HDMI Screens (~165x100mm) - Reference

Already selected in the current Option A/B/C hardware setups. Included for comparison.

#### Waveshare 7" HDMI LCD (H) - Capacitive

| Spec | Value |
|------|-------|
| Resolution | 1024x600 |
| Panel | IPS |
| Touch | 5-point capacitive (USB), 10-point on Windows |
| Toughened Glass | 6H hardness |
| Audio | 3.5mm jack + speaker header |
| Overall Dimensions | ~170x107mm (estimated) |
| Price | ~40-45 USD (~37-41 EUR) / ~50 EUR on Amazon.de |
| Linux | Driver-free HDMI + USB touch |

#### LESOWN R7-S 7" Monitor

| Spec | Value |
|------|-------|
| Resolution | 1024x600 |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Active Area | 154.21(W) x 85.92(H) mm |
| Color Gamut | 50% NTSC |
| Brightness | 400 cd/m2 |
| Contrast | 800:1 |
| Touch | 5-point capacitive |
| Interface | 1x HDMI + 2x USB |
| Weight | 161g |
| Price | ~35-50 USD (~32-46 EUR) |

**Assessment**: 7" screens at 1024x600 are the current baseline in the hardware options. At ~165x100mm they are significantly larger than the OPi5+ (100x70mm) and would overhang by 65mm in width. They make sense if the enclosure is designed around the display rather than the SBC. For a compact controller-mounted design, they are too large. For a standalone box design (SBC + displays in one unit), they remain the best option at ~40 EUR.

Sources: [Waveshare 7" (H)](https://www.waveshare.com/7inch-hdmi-lcd-h.htm), [LESOWN R7-S](https://www.lesown.com/products/7inch-1024x600-ips-touchscreen-capacitive-portable-ultra-hd-display-supports-raspberry-pi-banana-pi-windows)

### Category 5: Ultrawide/Bar-Type HDMI Displays

These are the most interesting option for a DJ waveform display. A long narrow screen naturally matches the horizontal waveform layout.

#### LESOWN P88 8.8" Stretched Bar - 1920x480

| Spec | Value |
|------|-------|
| Resolution | 1920x480 (or 480x1920 portrait native) |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Overall Dimensions | 65x232x12.5mm |
| Active Area | 54.72(W) x 218.88(H) mm (portrait) / 218.88(W) x 54.72(H) mm (landscape) |
| Color Gamut | 50% NTSC |
| Brightness | 600 cd/m2 |
| Contrast | 800:1 |
| Response Time | 30ms |
| Refresh Rate | 60Hz |
| Display Colors | 16.7M (8-bit) |
| Interface | 1x mini-HDMI + 1x micro-USB (power) |
| RPi Mounting Holes | 58x49mm on back |
| Weight | 172g |
| Touch (P88-T variant) | Capacitive touch |
| Operating Temp | -10 to 60 degrees C |
| Price | ~40-60 USD (~37-55 EUR) non-touch / ~55-75 USD (~50-69 EUR) touch |
| Price (AliExpress) | ~25-45 EUR non-touch |
| Linux | Driver-free HDMI, plug and play |

**Assessment**: THIS IS THE MOST INTERESTING OPTION FOR DJ USE. At 219mm (landscape width) x 55mm (height), this bar display could span across the top of an F1 controller (294mm) or be mounted above the SBC. The 1920x480 resolution gives excellent horizontal detail for waveform display. The 600 cd/m2 brightness is the highest of any display researched. The 4:1 aspect ratio naturally suits waveform visualization.

**Key concern**: The 480x1920 native portrait orientation requires the GPU to output this non-standard resolution or use software rotation. Most docking stations and HDMI adapters do NOT support 480x1920. The display must be connected directly to the GPU's HDMI port. On the RK3588, the `rockchipdrm` driver should handle arbitrary resolution modes via EDID, but this needs testing.

**For the DJ enclosure**: At 232mm long and 65mm wide, the bar fits perfectly along the length of the F1 controller (294mm) with space to spare. Two bars stacked vertically (2x 65mm = 130mm) would give dual-deck waveform display at 1920x480 each in only 130mm of vertical space.

Sources: [LESOWN Product Page](https://www.lesown.com/products/8-8-inch-ips-usb-widescreen-tft-lcd-display-480x1920-bar-pc-case-monitor-portatil-screen-monitor-strip-small-computer-monitors), [Amazon](https://www.amazon.com/Monitor-Stretched-1920x480-Widescreen-Monitoring/dp/B0B1HPGMSD), [AliExpress DE](https://de.aliexpress.com/item/1005004829276116.html)

#### Waveshare 8.8" IPS Side Monitor - 480x1920

| Spec | Value |
|------|-------|
| Resolution | 480x1920 (portrait native) |
| Panel | IPS |
| Viewing Angle | 170 degrees |
| Touch | None |
| Audio | Dual HiFi speakers (built-in) |
| Case | CNC alloy enclosure |
| Interface | Full-size HDMI + USB-C (power) |
| Backlight Brightness | Adjustable (260-500mA at dimmest/brightest) |
| Collapsible Stand | Built-in rear stand |
| Weight | 415g |
| Price | ~97 USD (~89 EUR) / 109 EUR on welectron.com |
| Linux | Driver-free HDMI |

**Assessment**: Premium build quality with metal CNC enclosure and HiFi speakers. However, at ~89-109 EUR it is way over budget (more than 2x the 40 EUR target). The LESOWN P88 at ~35-45 EUR offers nearly identical display specs at less than half the price. The Waveshare only wins on build quality and speakers (which are unnecessary for a DJ display). NOT recommended unless the metal enclosure is a strong requirement.

Sources: [Waveshare Product Page](https://www.waveshare.com/8.8inch-side-monitor.htm), [Amazon](https://www.amazon.com/Waveshare-8-8inch-Side-Monitor-Resolution/dp/B09JP2565M), [welectron.com](https://www.welectron.com/Waveshare-20818-88inch-Side-Monitor_1)

#### GeeekPi 11.26" 1920x440 HDMI Bar Display

| Spec | Value |
|------|-------|
| Resolution | 1920x440 (440x1920 portrait native) |
| Panel | IPS |
| Viewing Angle | 178 degrees |
| Viewing Area | 252.69(H) x 57.90(V) mm |
| Body Size | 276.29(W) x 76.5(H) mm |
| Brightness | 350 cd/m2 |
| Contrast | 600:1 |
| Response Time | 30ms |
| Color Gamut | 72% NTSC |
| Display Colors | 16.7M (8-bit) |
| Touch | 10-point capacitive (In-Cell) |
| Audio | Built-in speakers + microphone |
| Interface | HDMI |
| Mounting | Holes for Pi board mount |
| Price | ~50-70 USD (~46-64 EUR) estimated |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: At 276mm wide, this display almost perfectly spans the F1 controller (294mm). The 1920x440 resolution is essentially the same as the 8.8" bar displays but in a longer, thinner form factor. The 72% NTSC color gamut is better than the LESOWN's 50%. The 10-point capacitive In-Cell touch is premium. At ~50-65 EUR it is over the 40 EUR target but could serve as a single widescreen display replacing two smaller screens.

**For the DJ enclosure**: A single 11.26" bar (276x77mm) across the top of an F1 controller (294mm wide) would provide a stunning widescreen waveform display. At 77mm tall, it is compact enough to sit above the controller without excessive bulk. This replaces the need for two separate screens and simplifies cabling (1 HDMI instead of 2).

Sources: [Amazon](https://www.amazon.com/GeeekPi-1920x440-Capacitive-Screen-Raspberry/dp/B0F6NPCX1V), [52Pi Store](https://52pi.com/products/52pi-11-26-inch-capacitive-touch-screen-1920x440-hdmi-display-screen-with-speakers-for-raspberry-pi-5-4b-3b-3b)

#### Waveshare 11.9" HDMI LCD - 320x1480

| Spec | Value |
|------|-------|
| Resolution | 320x1480 (portrait native) |
| Panel | IPS |
| Viewing Angle | 170 degrees |
| Touch | 5-point capacitive |
| Toughened Glass | 6H hardness |
| Case | Zinc alloy |
| Audio | 3.5mm jack + dual stereo speakers |
| Collapsible Stand | Yes |
| Price | ~134 USD (~123 EUR) |
| Linux | Driver-free HDMI + USB touch |

**Assessment**: DISQUALIFIED for this use case. At 320x1480 (landscape = 1480x320), the vertical resolution of 320 pixels is far too low for any useful display. This is designed as a vertical sidebar monitor, not a horizontal bar. The price (~123 EUR) is also far over budget. Included for completeness.

Sources: [Waveshare Product Page](https://www.waveshare.com/11.9inch-hdmi-lcd.htm), [Amazon](https://www.amazon.com/11-9inch-Capacitive-LCD-Resolution-HDMI/dp/B092LSDMP8)

#### LESOWN 14.1" Stretched Bar - 1920x550

| Spec | Value |
|------|-------|
| Resolution | 1920x550 |
| Panel | IPS |
| Brightness | 400 cd/m2 |
| Color Gamut | 100% sRGB |
| Interface | USB-C + mini-HDMI |
| Dimensions | ~354x120x13mm (estimated) |
| Weight | 340g |
| Speakers | Built-in stereo |
| VESA | 75x75mm |
| Price | ~80-100 USD (~73-92 EUR) |
| Linux | Driver-free |

**Assessment**: The 100% sRGB gamut is excellent for color accuracy. At ~354mm wide it exceeds the F1 controller length (294mm) -- too long for this application. The 1920x550 resolution provides a bit more vertical space than 1920x480. Over budget and oversized for this project but interesting as a reference.

Sources: [Amazon](https://www.amazon.com/LESOWN-Ultrawide-Portable-1920x550-Secondary/dp/B0CQSXV8PP)

#### Bar Display Category Summary

**Best value**: LESOWN P88 8.8" (1920x480) at ~35-45 EUR from AliExpress. Highest brightness (600 cd/m2), compact enough to mount on the controller, standard HDMI + USB power.

**Best single-screen solution**: GeeekPi 11.26" (1920x440) at ~50-65 EUR. Spans the full controller width, 10-point touch, built-in speakers. Slightly over budget but eliminates the need for two screens.

**Avoid**: Waveshare 8.8" Side Monitor (too expensive), Waveshare 11.9" (too low vertical resolution), LESOWN 14.1" (too long).

### Recommended Display Configurations

Based on all research, here are the recommended configurations ranked by fitness for a compact DJ controller:

#### Config 1: Dual 8.8" Bar Displays (BEST FOR WAVEFORMS)

| Component | Details | Price (EUR) |
|-----------|---------|-------------|
| 2x LESOWN P88 8.8" | 1920x480, IPS, 600cd/m2 | ~70-90 (2x 35-45) |
| Layout | Stacked vertically, one per deck | |
| Total dimensions | 232x130mm (W x H for 2 stacked) | |
| HDMI connections | 2x mini-HDMI (uses both OPi5+ HDMI outputs) | |

**Pros**: Highest horizontal resolution (1920px each), best brightness, natural waveform shape, compact height.
**Cons**: Non-standard 480x1920 resolution needs GPU/compositor testing, slightly over 40 EUR per screen from EU sellers.

#### Config 2: Single 11.26" Bar Display (SIMPLEST)

| Component | Details | Price (EUR) |
|-----------|---------|-------------|
| 1x GeeekPi 11.26" | 1920x440, IPS, 10-pt touch, speakers | ~50-65 |
| Layout | Single screen spanning controller width | |
| Total dimensions | 276x77mm (W x H) | |
| HDMI connections | 1x HDMI (leaves second OPi5+ HDMI free) | |

**Pros**: Single cable, touch input, built-in speakers, fits F1 width, simplest software config.
**Cons**: Single screen for all decks (split view needed), slightly over budget, 440px vertical vs 480px.

#### Config 3: Dual 5" Displays (SAFE CHOICE)

| Component | Details | Price (EUR) |
|-----------|---------|-------------|
| 2x Waveshare 5" (H) V4 | 800x480, IPS, capacitive touch | ~54-64 (2x 27-32) |
| Layout | Side by side, one per deck | |
| Total dimensions | 242x89mm (W x H for 2 side-by-side) | |
| HDMI connections | 2x HDMI (standard HDMI, guaranteed to work) | |

**Pros**: Standard resolution (guaranteed GPU support), cheapest option, proven Linux compatibility, capacitive touch.
**Cons**: Lower resolution (800x480 vs 1920x480), more wasted bezel space side-by-side.

#### Config 4: Dual 5" High-Res (PREMIUM)

| Component | Details | Price (EUR) |
|-----------|---------|-------------|
| 2x VIEWMEI 5" 1280x720 | 1280x720, IPS, 1000:1 contrast | ~46-64 (2x 23-32) |
| Layout | Side by side, one per deck | |
| Total dimensions | ~250x80mm (estimated) | |
| HDMI connections | 2x mini-HDMI | |

**Pros**: Highest per-screen resolution in budget, best contrast ratio (1000:1), very affordable.
**Cons**: No touch, exact physical dimensions need verification, less community documentation.

### Updated Hardware Options with Display Variants

#### Option A-1: Compact Waveform Display (~$290-320)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Plus 16GB | ~$129 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB | ~$40 |
| Display | 2x LESOWN P88 8.8" bar (1920x480) | ~$70-90 |
| Audio (master) | PCM5102A I2S DAC on GPIO I2S3 | ~$5 |
| Audio (cue) | Onboard ES8388 3.5mm jack | $0 |
| Enclosure | Custom enclosure | ~$20-50 |
| **Total** | | **~$280-330** |

#### Option A-2: Safe Standard Display (~$270-310)

| Component | Recommendation | Est. Cost |
|-----------|---------------|-----------|
| SBC | Orange Pi 5 Plus 16GB | ~$129 |
| Cooling | Active heatsink + fan | ~$15 |
| Storage | M.2 NVMe SSD 500GB | ~$40 |
| Display | 2x Waveshare 5" (H) V4 (800x480) | ~$55-65 |
| Audio (master) | PCM5102A I2S DAC on GPIO I2S3 | ~$5 |
| Audio (cue) | Onboard ES8388 3.5mm jack | $0 |
| Enclosure | Custom enclosure | ~$20-50 |
| **Total** | | **~$265-305** |

## MIPI DSI Display Research

### Goal

Evaluate MIPI DSI as an alternative to HDMI for the embedded displays. MIPI DSI connects directly to the SoC via an FPC ribbon cable -- no HDMI encoder/decoder chain. Potential benefits: lower latency, thinner profile, lower power, more integrated (no external HDMI connector/cable). Target: 3.5" to 5" diagonal, IPS, under 40 EUR per screen.

### Orange Pi 5 Plus MIPI DSI Hardware

#### DSI Connector

The OPi 5 Plus has a **single MIPI DSI connector** on the board edge, positioned between the touchscreen I2C connector and the CSI camera connector. Based on RK3588 reference designs and the official user manual, it is a **30-pin 0.5mm pitch FPC (ZIF)** connector carrying one 4-lane MIPI DSI interface.

The pinout is **NOT compatible with Raspberry Pi DSI**. The RPi uses a 15-pin 1.0mm pitch FPC connector with a completely different signal layout. Raspberry Pi DSI displays will NOT work directly -- an adapter board or different cable is required.

The OPi 5 Plus also has a separate **6-pin FPC socket** for touchscreen I2C (INT, RST, SDA, SCL, VCC, GND), positioned adjacent to the DSI connector. This means DSI panels with integrated touch controllers need two cables: one for DSI video, one for I2C touch.

Sources: [Orange Pi 5 Plus Product Page](http://www.orangepi.org/html/hardWare/computerAndMicrocontrollers/details/Orange-Pi-5-plus.html), [Orange Pi 5 Plus Wiki](http://www.orangepi.org/orangepiwiki/index.php/Orange_Pi_5_Plus)

#### RK3588 DSI Capabilities

| Feature | Specification |
|---------|---------------|
| DSI interfaces on SoC | 2x MIPI DSI (DSI0 + DSI1) |
| PHY type | DPHY v2.0 / CPHY v1.1 (combo PHY) |
| DPHY lanes | 4 lanes per interface |
| DPHY max data rate | 4.5 Gbps per lane |
| Max resolution per DSI | 4096x2304 @ 60Hz |
| DSC support | DSC 1.1 / 1.2a |
| DSI version | v2.0 |

The SoC has TWO DSI interfaces, but the Orange Pi 5 Plus board only exposes ONE via FPC connector. The second DSI is either not routed or used internally. Some other RK3588 boards (Firefly ROC-RK3588S-PC, LubanCat-4, ArmSoM Sige7) expose both DSI0 and DSI1.

Sources: [Rockchip RK3588 Datasheet](https://wiki.friendlyelec.com/wiki/images/e/ee/Rockchip_RK3588_Datasheet_V1.6-20231016.pdf), [Firefly Display Wiki](https://wiki.t-firefly.com/en/ROC-RK3588S-PC/usage_display.html)

#### Simultaneous DSI + HDMI

Yes -- the RK3588 VOP (Video Output Processor) supports up to **4 simultaneous displays**. Each display interface binds to a separate VOP port:

- VOP0: 7680x4320 @ 60Hz (typically HDMI0)
- VOP1: 4096x2304 @ 60Hz (typically HDMI1)
- VOP2: 4096x2304 @ 60Hz (DSI, eDP, or DP)
- VOP3: 1920x1080 @ 60Hz (DSI, eDP, or DP)

The OPi 5 Plus can drive **2x HDMI + 1x DSI simultaneously** (or 1x HDMI + 1x DSI + 1x USB-C DP, etc.). Device tree configuration assigns each interface to a VOP port via `status = "okay"` on the corresponding `*_in_vpN` nodes.

Sources: [Firefly RK3588 Display Wiki](https://wiki.t-firefly.com/en/ROC-RK3588-PC/usage_display.html), [CNX Software OPi 5 Plus](https://www.cnx-software.com/2023/05/10/orange-pi-5-plus-sbc-switches-to-rockchip-rk3588-soc-brings-dual-hdmi-2-1-dual-2-5gbe-m-2-pcie-sockets/)

### Candidate DSI Panels

#### Orange Pi Official 10.1" DSI LCD

| Spec | Value |
|------|-------|
| Size | 10.1" |
| Resolution | 800x1280 |
| Panel | TFT LCD IPS |
| Touch | Capacitive (needs adapter board) |
| Panel IC | ILI9881C |
| Interface | MIPI DSI via FPC |
| Adapter | "Orange Pi RK LCD v1.1" adapter board required |
| Compatibility | OPi 5, 5B, 5 Plus |
| Price | ~$50-70 USD |

**Assessment**: The only officially supported DSI display for the OPi 5 Plus. However, at 10.1" it is far too large for this application (target: 3.5-5"). The adapter board adds bulk. Useful as a reference for working device tree configurations. The ILI9881C driver is in mainline Linux.

Sources: [Orange Pi Touch Screen Product Page](http://www.orangepi.org/html/hardWare/otherAccessories/details/Orange-Pi-Touch-Screen-Pi5.html), [Amazon](https://www.amazon.com/Orange-Pi-Portable-Compatible-Computer/dp/B0BS18CVGZ)

#### Radxa Display 8 HD

| Spec | Value |
|------|-------|
| Size | 8.0" |
| Resolution | 800x1280 |
| Panel | TFT LCD IPS |
| Brightness | 300 cd/m2 |
| Touch | 5-point capacitive (GT911 controller) |
| Interface | Single FPC cable (power + display + touch) |
| FPC | 39-pin 0.3mm pitch (SBC side) to 40-pin 0.5mm pitch (panel side) |
| Gravity sensor | Built-in (Android auto-rotation) |
| Compatible boards | Rock 5A, 5B, 5C, 4C+, 3B |
| Availability | Guaranteed until September 2029 |
| Price | ~$30-45 USD |

**Assessment**: Elegant single-cable design (display + touch + power over one FPC). However, at 8" it is too large. The 39-pin FPC connector does NOT match the OPi 5 Plus 30-pin connector -- an adapter cable would be needed. Radxa provides overlays for their own Rock boards but not for Orange Pi boards. Cross-board compatibility would require writing a custom device tree overlay.

Sources: [Radxa Display 8 HD Docs](https://docs.radxa.com/en/accessories/lcd-8-hd/lcd-8-hd-product), [Radxa Product Page](https://radxa.com/products/accessories/display-8hd/), [ThinkRobotics](https://thinkrobotics.com/products/radxa-display-8-hd)

#### Waveshare 4" DSI LCD (480x800)

| Spec | Value |
|------|-------|
| Size | 4.0" |
| Resolution | 480x800 (portrait native) |
| Panel | IPS |
| Viewing angle | 170 degrees |
| Touch | 5-point capacitive (I2C) |
| Glass | Toughened, 6H hardness, optical bonding |
| Interface | 15-pin 1.0mm pitch FPC (Raspberry Pi DSI) |
| Power | ~1W via DSI |
| Weight | 97g |
| Supported platforms | RPi 5/4B/3B+/3A+, CM3/3+/4 |
| Price | ~$50 USD / ~40 EUR |

**Assessment**: Right size for this project. However, it uses the **Raspberry Pi 15-pin DSI connector** which is incompatible with the OPi 5 Plus 30-pin connector. The internal panel controller IC is not publicly documented by Waveshare, making it harder to write a custom driver. The Waveshare DSI driver has been open-sourced and merged into the Raspberry Pi kernel (`vc4-kms-dsi-waveshare-panel` overlay), but this driver is specific to the Broadcom VC4/V3D GPU pipeline and will NOT work on Rockchip's `rockchipdrm`.

To use this panel on RK3588, you would need to:
1. Fabricate or source a 15-pin RPi DSI to 30-pin OPi5+ DSI adapter cable (signal remapping required)
2. Reverse-engineer the panel IC and initialization sequence from the RPi kernel driver source
3. Write a custom Rockchip device tree overlay with the panel init sequence
4. Build a custom kernel module or modify `panel-simple.c`

Sources: [Waveshare Product Page](https://www.waveshare.com/4inch-dsi-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/4inch_DSI_LCD), [Waveshare DSI LCD GitHub](https://github.com/waveshareteam/Waveshare-DSI-LCD)

#### Waveshare 4.3" DSI LCD (800x480)

| Spec | Value |
|------|-------|
| Size | 4.3" |
| Resolution | 800x480 |
| Panel | IPS |
| Viewing angle | 160 degrees |
| Touch | 5-point capacitive (I2C, Goodix controller) |
| Interface | 15-pin 1.0mm pitch FPC (Raspberry Pi DSI) |
| Thickness | 5mm |
| Power | ~1.2W |
| Supported platforms | RPi 5/4B/3B+/3A+, CM3/3+/4 |
| Price | ~$36 USD / ~40 EUR |
| European price | 39.90 EUR at welectron.com |

**Assessment**: Same connector incompatibility as the 4" model. 800x480 landscape is better suited for waveforms than 480x800 portrait. At 40 EUR it hits the exact budget limit. Same adaptation challenges apply.

Sources: [Waveshare Product Page](https://www.waveshare.com/4.3inch-dsi-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/4.3inch_DSI_LCD), [welectron.com](https://www.welectron.com/Waveshare-16239-43inch-DSI-LCD_1)

#### Waveshare 5" DSI LCD (800x480)

| Spec | Value |
|------|-------|
| Size | 5.0" |
| Resolution | 800x480 |
| Panel | IPS (or TFT depending on variant) |
| Viewing angle | 160 degrees |
| Touch | 5-point capacitive (I2C) |
| Interface | 15-pin 1.0mm pitch FPC (Raspberry Pi DSI) |
| Thickness | 5mm |
| Power | ~1.2W |
| Weight | 208g |
| Supported platforms | RPi 5/4B/3B+/3A+, CM3/3+/4 |
| Price | ~$40 USD / ~37 EUR |

**Assessment**: Largest of the Waveshare small DSI panels. Same RPi connector incompatibility. The ultra-thin 5mm profile is very attractive for an integrated enclosure. Same adaptation challenges as other Waveshare DSI panels.

Sources: [Waveshare Product Page](https://www.waveshare.com/5inch-dsi-lcd.htm), [Waveshare Wiki](https://www.waveshare.com/wiki/5inch_DSI_LCD)

#### Generic Bare MIPI DSI Panels (AliExpress / Industrial Suppliers)

Bare MIPI DSI panels (phone/tablet replacement LCDs) are available for 8-20 EUR on AliExpress and from industrial display suppliers. Common specifications:

| Size | Resolution | Controller IC | Lanes | Price (AliExpress) |
|------|-----------|---------------|-------|--------------------|
| 3.5" | 480x800 | ST7701S | 2 | ~8-15 EUR |
| 4.0" | 480x480 | ST7701S | 2 | ~10-18 EUR |
| 4.3" | 480x800 | ST7701S | 2 | ~10-18 EUR |
| 4.5" | 480x854 | ST7701S | 2 | ~10-20 EUR |
| 5.0" | 720x1280 | ILI9881C | 4 | ~15-25 EUR |
| 5.0" | 800x480 | ST7701S | 2 | ~10-20 EUR |

**Common Panel ICs and Linux Support**:

| IC | Resolution | Lanes | Linux Driver | Mainline? |
|----|-----------|-------|-------------|-----------|
| **ILI9881C** | Up to 800x1280 | 4 | `panel-ilitek-ili9881c.c` | YES |
| **HX8394** | Up to 800x1280 | 4 | `display_hx8394.c` | Partial |
| **ST7701S** | Up to 480x854 | 2 | `panel-sitronix-st7701s.c` | Community |
| **JD9365DA** | Up to 800x1280 | 4 | Community patches | No |

The **ILI9881C** is the best-supported IC for RK3588 -- it has a mainline Linux driver and has been confirmed working on the Orange Pi 5 by the Armbian community (with kernel recompilation to add panel-specific init sequences and timings).

**What you need for a bare panel**:
1. Panel datasheet with initialization sequence and timing parameters
2. Device tree overlay targeting `dsi0` or `dsi1` with correct VOP port binding
3. Correct panel init sequence in `panel-init-sequence` DTS property
4. FPC cable or adapter matching the panel's FPC connector to the OPi5+ 30-pin DSI connector
5. Backlight driver (usually PWM-controlled via a GPIO, configured in DTS)
6. Touch controller wiring to the OPi5+ touchscreen I2C header (if touch is desired)

Sources: [Mainline ILI9881C driver](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/panel/panel-ilitek-ili9881c.c), [Armbian ILI9881C on OPi5 thread](https://forum.armbian.com/topic/29825-ili9881c-panel-bringup-errors/), [ArmSoM RK3588 Panel Configuration Guide](https://forum.armsom.org/t/rk3588-mipi-panel-debugging-rk3588-mipi-dsi-panel-configuration/146)

### DSI vs HDMI Tradeoffs

| Aspect | MIPI DSI | HDMI |
|--------|----------|------|
| **Display latency** | <10ms (direct GPU path) | 20-50ms (encoder/decoder chain) |
| **Touch latency** | 5-15ms (I2C on same board) | 10-30ms (separate USB) |
| **Power consumption** | 1-2W per panel | 3-5W per panel |
| **Cable/connector** | FPC ribbon (fragile, <30cm) | HDMI cable (robust, up to meters) |
| **Profile thickness** | **5mm** (panel only, no connector box) | 12-20mm (PCB + HDMI connector) |
| **Hot-swap** | **NO** (can damage SoC) | Yes |
| **Driver complexity** | Custom DTS overlay + possible kernel module | **Plug-and-play** (EDID auto-config) |
| **Resolution auto-detect** | No (hardcoded in DTS) | **Yes** (EDID) |
| **EMI sensitivity** | High (unshielded FPC) | Low (shielded cable) |
| **NixOS setup** | Hard (DTS overlays, kernel config) | **Easy** (standard DRM/KMS) |
| **Community support** | Sparse (RK3588 DSI is niche) | **Extensive** (universal) |
| **Dual display** | Only 1 DSI on OPi5+ FPC | **2x HDMI on OPi5+** |
| **Panel cost** | Similar or cheaper bare panels | Similar for modules |
| **Reliability** | Board-level (no connectors to wiggle) | **Connector can loosen** |

#### Latency Analysis

The <10ms DSI latency advantage is real but context-dependent. For the mesh-player DJ use case:

- **Audio latency budget**: 2.67ms (128 samples @ 48kHz). Display latency is already 10-50x longer than audio latency regardless of interface.
- **Waveform rendering**: GPU renders at 60fps (16.7ms per frame). The display interface latency sits on top of the frame rendering time.
- **DSI advantage**: ~10-40ms less display lag. At 60fps this is 0.5-2.5 frames faster. For a waveform display this is noticeable but not critical.
- **Touch advantage**: 5-15ms faster touch response. More noticeable for interactive controls, but the Traktor F1 handles most input via USB HID, not touchscreen.

The latency advantage of DSI matters most for **touch-interactive applications** (medical, automotive HMI). For a **waveform display with hardware controller input**, the latency difference is marginal.

### Dual DSI on RK3588

The RK3588 SoC has TWO independent MIPI DSI interfaces (DSI0 and DSI1), each with 4 lanes. In theory, you can drive two separate DSI panels simultaneously by assigning them to different VOP ports:

```
DSI0 → VOP2 (4096x2304 capable)
DSI1 → VOP3 (1920x1080 capable)
```

Device tree configuration:
```dts
&dsi0_in_vp2 { status = "okay"; };
&dsi1_in_vp3 { status = "okay"; };
```

**However, the Orange Pi 5 Plus only exposes one DSI connector.** To use dual DSI, you would need a board that routes both to FPC connectors (e.g., Firefly ROC-RK3588S-PC, LubanCat-4). On the OPi 5 Plus, the maximum DSI display count is one.

For dual displays on the OPi 5 Plus, the configuration would be:
- **1x HDMI + 1x DSI** (hybrid approach)
- **2x HDMI** (current recommendation, proven working)

Sources: [LubanCat RK3588 MIPI Screen Guide](https://doc.embedfire.com/linux/rk3588/quick_start/en/latest/quick_start/screen/mipi_dsi.html), [Firefly RK3588S Display Wiki](https://wiki.t-firefly.com/en/ROC-RK3588S-PC/usage_display.html)

### Touch Integration with DSI Panels

Touch on DSI panels is always a **separate connection** from the video signal. MIPI DSI carries only display data -- touch input travels over a dedicated I2C (or sometimes SPI/USB) bus.

#### Typical Wiring

```
DSI panel FPC → OPi5+ DSI 30-pin connector (video only)
Touch IC FPC  → OPi5+ 6-pin touchscreen I2C header (touch data)
                 or → GPIO header I2C bus (pins 3/5 for I2C3, or other)
```

#### Common Touch Controller ICs

| IC | Touch Points | Interface | I2C Address | Linux Driver | Mainline? |
|----|-------------|-----------|-------------|-------------|-----------|
| **GT911** (Goodix) | 5-10 | I2C (400kHz max) | 0x14 or 0x5D | `goodix.c` | YES |
| **GT9271** (Goodix) | 10 | I2C | 0x14 or 0x5D | `goodix.c` | YES |
| **FT5406** (FocalTech) | 5 | I2C | 0x38 | `edt-ft5x06.c` | YES |
| **FT6206** (FocalTech) | 2 | I2C | 0x38 | `edt-ft5x06.c` | YES |
| **CST340** (Hynitron) | 5 | I2C | 0x1A | Community | No |

The **GT911** (Goodix) is the most common touch IC paired with DSI panels in the 3.5-10" range. It is well-supported in mainline Linux via `drivers/input/touchscreen/goodix.c`. The driver supports auto-detection of the GT911 variant via I2C probing.

#### Device Tree Configuration for Touch

```dts
&i2c3 {
    status = "okay";

    gt911: touchscreen@14 {
        compatible = "goodix,gt911";
        reg = <0x14>;
        interrupt-parent = <&gpio0>;
        interrupts = <RK_PA0 IRQ_TYPE_EDGE_FALLING>;
        irq-gpios = <&gpio0 RK_PA0 GPIO_ACTIVE_LOW>;
        reset-gpios = <&gpio0 RK_PA1 GPIO_ACTIVE_LOW>;
        touchscreen-size-x = <800>;
        touchscreen-size-y = <1280>;
    };
};
```

Key requirements:
- **INT pin**: Must be connected to a GPIO with interrupt capability. This is the most common cause of touch failures.
- **RST pin**: Optional but recommended for reliable initialization.
- **I2C address selection**: GT911 address is determined by the state of INT pin during reset. Address 0x14 when INT is low during reset, 0x5D when high.
- **Firmware config**: Optional `goodix_911_cfg.bin` file can configure resolution, sensitivity, and gesture parameters.

Sources: [Mainline Goodix driver](https://github.com/torvalds/linux/blob/master/drivers/input/touchscreen/goodix.c), [GT911 Datasheet](https://www.fortec-integrated.de/fileadmin/pdf/produkte/Touchcontroller/DDGroup/GT911_Datasheet.pdf), [Focus LCDs GT911 Programming Guide](https://focuslcds.com/application-notes/programming-a-capacitive-touch-panel-utilizing-the-gt911-touch-controller/)

### NixOS-Specific DSI Challenges

Setting up MIPI DSI on NixOS adds significant complexity beyond what Armbian or ubuntu-rockchip require:

1. **Device tree overlays**: NixOS uses `hardware.deviceTree.overlays` in the system configuration. There have been reported issues with overlays failing silently or not being applied correctly on NixOS (see [nixpkgs #125354](https://github.com/NixOS/nixpkgs/issues/125354)). The `hardware.deviceTree.base` option was deprecated, and the replacement syntax has caused boot failures for some users.

2. **Kernel module building**: If the DSI panel requires a custom kernel module (not in mainline), NixOS's reproducible build system makes it harder to patch kernels compared to `make menuconfig` on Debian-based systems.

3. **Debugging**: DSI init failures produce no visible output (the display stays black). On Armbian you can check `dmesg | grep dsi` after SSH-ing in. On a NixOS kiosk (cage compositor, no SSH by default), debugging requires serial console access or switching to a known-working HDMI display.

4. **Overlay testing workflow**: Each DTS change requires a `nixos-rebuild switch` cycle (30-60 seconds) vs. simply replacing a `.dtbo` file and rebooting on Armbian.

Sources: [NixOS DT Overlay Issues](https://github.com/NixOS/nixpkgs/issues/125354), [Armbian DT Overlay Docs](https://docs.armbian.com/User-Guide_Armbian_overlays/)

### DSI Community Status on Orange Pi 5

- **Armbian community**: One developer confirmed ILI9881C-based DSI panel working on the Orange Pi 5 with kernel recompilation (modifying init sequence and timings in the driver). However, the Armbian forum thread received zero replies and the effort appears to have stalled.

- **BitBuilt retro handheld community**: A group building a portable handheld with the OPi 5 confirmed DSI working with an ILI9881C panel after modifying the stock kernel driver. They also got backlight and touch working.

- **Official Orange Pi support**: Only the official 10.1" LCD is supported. No smaller DSI panels have official device tree overlays.

- **Mainline kernel**: MIPI DSI PHY support for RK3588 was submitted for Linux 6.14+ (Heiko Stubner patches). The DSI2 controller driver based on the Synopsys IP block has been submitted. As of early 2026, mainline DSI on RK3588 should be functional but bleeding-edge.

Sources: [Armbian OPi5 ILI9881C Thread](https://forum.armbian.com/topic/29825-ili9881c-panel-bringup-errors/), [BitBuilt Retro Lite CM5](https://bitbuilt.net/forums/threads/retro-lite-cm5.5815/), [RK3588 DSI PHY Patches](https://patchew.org/linux/20241113221018.62150-1-heiko@sntech.de/)

### DSI Recommendation

**Do NOT use MIPI DSI for the initial embedded build. Use HDMI.**

Reasons:

1. **Single DSI port on OPi5+**: Only one DSI connector is exposed. For dual-deck display, you need two screens. With DSI you would still need one HDMI display plus one DSI display -- a hybrid setup that doubles the complexity.

2. **Connector incompatibility**: All readily available small DSI panels (Waveshare, Radxa) use Raspberry Pi 15-pin connectors. The OPi5+ uses a 30-pin connector. No off-the-shelf adapter exists.

3. **Driver complexity**: Every DSI panel requires a custom device tree overlay with exact init sequences and timings. This is kernel-level work with poor debugging tools (black screen = no feedback). HDMI is plug-and-play.

4. **NixOS friction**: Device tree overlay handling in NixOS has known issues. Adding DSI panel bring-up to the NixOS kiosk setup multiplies risk.

5. **Marginal latency benefit**: The ~10-40ms latency reduction from DSI is not meaningful for a waveform display application. The real-time constraint is in the audio path (2.67ms), not the display path.

6. **Community support**: Zero confirmed small (3.5-5") DSI panels working on the OPi 5 Plus. The few success stories are on the regular OPi 5 with custom kernel builds.

**When DSI makes sense (future phase)**:

- If building a custom enclosure where the 5mm DSI panel profile is critical
- If switching to a board with dual DSI connectors (Firefly ROC-RK3588S-PC, LubanCat-4)
- If a bare ILI9881C 5" panel (720x1280, ~15-20 EUR) is paired with a custom adapter board
- If the NixOS kernel build includes the correct DTS overlay from the start
- After HDMI displays are proven working and the system is stable

**The HDMI bar displays (LESOWN P88 8.8" at 1920x480) remain the superior choice**: higher resolution, plug-and-play, proven on RK3588, no driver work, dual displays via dual HDMI.

## Verdict: GO

The mesh-player codebase is remarkably portable to ARM64. Zero architectural changes needed -- only build infrastructure fixes. The RK3588/RK3588S provides sufficient CPU, GPU, and I/O for real-time DJ performance with displays and low-latency audio.

**Primary target board (Feb 2026):** Orange Pi 5 Pro 8GB + PCM5102A I2S DAC. Core BOM: **~$112**. Full enclosed unit: **~$165**. No external audio interface needed — master out via I2S GPIO, cue/headphones via onboard ES8388 codec. No NVMe required — DJ plays from USB 3.0 stick. Upgrade path: OPi 5 Max 16GB ($145) for WiFi 6E + built-in NVMe library.
