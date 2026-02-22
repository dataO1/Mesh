# Orange Pi 5 Performance Analysis & Optimization Guide

**Target Hardware**: Orange Pi 5 (RK3588S, Mali G610 MP4, 8 GB LPDDR4X, Panfrost driver)
**Dev Hardware**: AMD Ryzen 7 PRO 4750U, Radeon Vega 7 (512 MB VRAM), 32 GB DDR4
**Application**: Mesh DJ Player — iced 0.14 + wgpu + CLAP plugin hosting
**Symptom**: Smooth on dev laptop (Vega 7 iGPU), jagged/stuttering after loading a second track on OPi5

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Hardware Comparison: Vega 7 vs Mali G610](#2-hardware-comparison)
3. [Real-World Peak Data Analysis](#3-real-world-peak-data-analysis)
4. [Bottleneck Analysis](#4-bottleneck-analysis)
   - 4.1 [GPU ALU Throughput (PRIMARY)](#41-gpu-alu-throughput)
   - 4.2 [SSBO Cache Behavior — Corrected](#42-ssbo-cache-behavior--corrected)
   - 4.3 [Panfrost Driver Efficiency](#43-panfrost-driver-efficiency)
   - 4.4 [Memory Pressure](#44-memory-pressure)
   - 4.5 [Per-Frame Debug Logging](#45-per-frame-debug-logging)
   - 4.6 [Rayon on Heterogeneous Cores](#46-rayon-on-heterogeneous-cores)
   - 4.7 [Audio Thread Budget](#47-audio-thread-budget)
5. [Proposed Solutions](#5-proposed-solutions)
   - 5.1 [Quick Wins (hours)](#51-quick-wins)
   - 5.2 [Medium-Term (days)](#52-medium-term)
   - 5.3 [Long-Term (weeks)](#53-long-term)
6. [Profiling & Measurement](#6-profiling--measurement)
   - 6.1 [Runtime Resource Meters](#61-runtime-resource-meters)
   - 6.2 [Frame Timing Instrumentation](#62-frame-timing-instrumentation)
   - 6.3 [iced Comet Debugger](#63-iced-comet-debugger)
   - 6.4 [Tracy / Perfetto GPU Profiling](#64-tracy--perfetto-gpu-profiling)
7. [Reference Data](#7-reference-data)

---

## 1. Executive Summary

The primary cause of jank on the Orange Pi 5 is **GPU ALU throughput saturation**. The waveform fragment shader runs ~415 floating-point operations per pixel across ~1.85 million fragments per frame (8 draw calls). On the dev laptop's Vega 7 iGPU this consumes ~20% of available throughput. On the Mali G610 with Panfrost, the same workload consumes ~110% — hence jank appears exactly when loading a second track doubles the active views from 4 to 8.

**Corrected understanding**: The earlier hypothesis of "cache thrashing from 5 MB peak buffers" was overstated. At the actual quality=0 setting with 1.0 peaks-per-pixel, the per-view working set is only ~30 KB — well within Mali's ~128 KB L2 cache. The bottleneck is raw shader ALU complexity, not memory access patterns.

**Priority ranking of bottlenecks:**

| # | Bottleneck | Severity | Effort to Fix |
|---|-----------|----------|---------------|
| 1 | GPU ALU throughput (1.85M fragments × complex shader) | **Critical** | Medium |
| 2 | Panfrost driver efficiency (~25-35% of peak GFLOPS) | **High** | None (upstream) |
| 3 | 8 draw calls with TBDR tile binning overhead | **Medium** | Medium |
| 4 | SSBO cache pressure at high zoom-out (pp/px ≥ 4) | **Medium** | Low-Medium |
| 5 | Memory pressure (8 GB shared) | **Low** | N/A |
| 6 | `log::debug!` in render hot path | **Low** | Trivial |
| 7 | Rayon on big.LITTLE cores | **Low** | Low |

---

## 2. Hardware Comparison

### Apples-to-Apples: Both Are Integrated GPUs

The dev laptop is **not** a discrete GPU system. Both GPUs are integrated, sharing system RAM:

| Spec | AMD Vega 7 (dev laptop) | Mali G610 MP4 (OPi5) | Gap |
|------|-------------------------|----------------------|-----|
| Architecture | GCN 5.0 (Vega) — **IMR** | Valhall — **TBDR** | Architectural |
| Compute Units / Cores | 7 CUs × 64 shaders = **448** | 4 cores × 64 FMA = **256 equiv** | **~1.75x** |
| Clock Speed | **1600 MHz** | **850-1000 MHz** | **~1.7x** |
| Peak FP32 GFLOPS | **~1,434** | **~512** | **~2.8x** |
| L2 Cache | **1 MB** | **~128-256 KB** | **~4-8x** |
| VRAM | **512 MB** dedicated carve-out | **None** (shared only) | Qualitative |
| System RAM | 32 GB DDR4 | 8 GB LPDDR4X | 4x |
| Memory Bandwidth | ~50 GB/s (dual-channel DDR4) | ~25 GB/s (shared) | **~2x** |
| Subgroup/Wave Size | 64 (wavefront64) | 16 (typical Mali) | Different |
| Driver | **RADV** (Mesa 25.2, very mature) | **Panfrost** (Mesa, reverse-engineered) | Qualitative |
| GPU busy (idle) | 6% | — | — |

### The Real Gap: Not 500x, But ~5-8x Effective

The raw GFLOPS gap is only ~2.8x. But the **effective gap** for this workload is larger due to:

1. **Driver efficiency**: RADV (Vega 7) achieves ~50-70% of peak GFLOPS in sustained shader workloads. Panfrost achieves ~25-35%. This alone doubles the raw gap to ~4-6x.
2. **SSBO path**: Vega is an Immediate Mode Renderer — SSBO and texture reads share the same unified 1 MB L2 cache. On Mali's TBDR, SSBOs go through the separate load/store unit, adding ~20-50% overhead per read.
3. **Tile binning overhead**: Each of the 8 fullscreen-triangle draw calls must be binned to all tiles on Mali (no equivalent overhead on Vega's IMR).

**Estimated effective throughput gap: ~5-8x** for this specific shader workload.

### Why 1 Track Works, 2 Tracks Breaks

| Scenario | Active Views | Total Fragments | Est. GPU Time (Vega 7) | Est. GPU Time (Mali G610) |
|----------|-------------|-----------------|------------------------|--------------------------|
| 1 track loaded | 4 (2 zoom + 2 overview) | ~924K | ~1.5-2 ms | ~8-10 ms |
| 2 tracks loaded | 8 (4 zoom + 4 overview) | ~1.85M | ~3-4 ms | ~16-20 ms |
| 4 tracks loaded | 8 (same, inactive decks early-return) | ~1.85M | ~3-4 ms | ~16-20 ms |

Frame budget at 60 Hz = 16.67 ms. With iced UI overhead (~3-5 ms), the Mali has:
- 1 track: 8-10 ms GPU + 3-5 ms iced = **11-15 ms → OK**
- 2 tracks: 16-20 ms GPU + 3-5 ms iced = **19-25 ms → OVER BUDGET → jank**

This matches the observed symptom exactly.

---

## 3. Real-World Peak Data Analysis

### Measured Example

From the dev laptop at quality=0:

```
[RENDER] Highres peaks: 110189 peaks | quality=0 bpm=172.0 screen=1920px |
         ref_zoom=4bars → 1.00 pp/px | samples_per_bar=66976 |
         track=15375098samples (320.3s)
```

### Peak Count Breakdown

| Quality | Target pp/px at 4-bar | Peaks/Stem | 4 Stems | 4 Decks | GPU Buffer Total |
|---------|----------------------|-----------|---------|---------|-----------------|
| **0 (Low)** | **1.0** | **110,189** | **3.4 MB** | **13.8 MB** | **Used on dev laptop** |
| 1 (Medium) | 2.0 | ~220,378 | 6.9 MB | 27.5 MB | |
| 2 (High) | 4.0 | ~440,756 | 13.7 MB | 55 MB | |
| 3 (Ultra) | 8.0 | ~881,512 | 27.5 MB | 110 MB | |

### Peaks-Per-Pixel at Various Zoom Levels (Quality=0)

| Zoom Level | pp/px | minmax_reduce Iters/Stem | SSBO Reads/Pixel (4 stems) | Working Set/View |
|-----------|-------|--------------------------|---------------------------|-----------------|
| 1 bar | 0.25 | 1 | 4 | 7.5 KB |
| 2 bars | 0.5 | 1 | 4 | 15 KB |
| **4 bars (ref)** | **1.0** | **1** | **4** | **30 KB** |
| 8 bars | 2.0 | 2 | 8-16* | 61 KB |
| 16 bars | 4.0 | 4 | 16-32* | 122 KB |
| 32 bars | 8.0 | 8 | 32-64* | 245 KB |
| Overview (800 peaks) | 0.83 | 1 | 4 | 25 KB |

*With abstraction enabled, `sample_peak()` calls `minmax_reduce` **twice** (for interpolation between grid points), doubling the read count.

### Vega 7 Equivalent

The dev laptop log shows **quality=0, 1.0 pp/px** runs smoothly with 4 decks. At this setting:
- `minmax_reduce` does exactly **1 iteration** per stem per pixel (range=1, step=1)
- Each pixel performs **4 SSBO reads** (one `raw_peak()` per stem)
- Total SSBO reads per frame: 1.85M pixels × 4 = **7.4M reads**

On the Vega 7's 1 MB L2, the 30 KB per-view working set is trivially cached. On Mali's 128 KB L2, it **also fits** — cache thrashing is NOT the issue at this setting.

### When Cache DOES Matter

Cache becomes a factor at **pp/px ≥ 4** (16-bar zoom-out or higher quality levels):

| pp/px | Working Set (4 stems × 8B) | Fits Mali L2? | Fits Vega 7 L2? |
|-------|---------------------------|---------------|-----------------|
| 1.0 | 30 KB | Yes | Yes |
| 2.0 | 61 KB | Yes | Yes |
| 4.0 | 122 KB | **Borderline** | Yes |
| 8.0 | 245 KB | **No** | Yes |
| 16.0 | 491 KB | No | **Borderline** |
| 32.0 | 983 KB | No | **Borderline** |

**Optimal peak count for Mali at quality=0**: The current 110K is fine. The working set only exceeds L2 at extreme zoom-out (≥32 bars), which is uncommon in performance mode. **Reducing peaks would hurt zoomed-in detail without fixing the primary bottleneck.**

### Peak Buffer Stride Analysis (Corrected)

The buffer layout is `[stem0_peaks..., stem1_peaks..., stem2_peaks..., stem3_peaks...]`. With 110K peaks/stem at 8 bytes each:
- Stem 0 base: offset 0
- Stem 1 base: offset 881 KB
- Stem 2 base: offset 1,762 KB
- Stem 3 base: offset 2,644 KB

On a 128 KB L2 with 4-way set associativity:
- Each stem's working window (30 KB at 1.0 pp/px) maps to ~470 cache lines
- The 4 stems map to **different cache sets** (stride is not a power-of-2 of the cache size)
- Total: 4 × 470 = 1,880 lines needed, out of ~2,048 available → **barely fits but works**

**Conclusion**: At quality=0/1.0 pp/px, the stride pattern does NOT cause systematic set conflicts. The earlier claim of "every stem switch is a cache miss" was wrong for this peak count.

---

## 4. Bottleneck Analysis

### 4.1 GPU ALU Throughput

**Severity**: Critical (primary cause of jank)

#### Per-Pixel ALU Cost Breakdown

The fragment shader (`fs_main`, waveform.wgsl:281-819) executes this work **per pixel**:

| Section | Lines | Est. Float Ops | Notes |
|---------|-------|---------------|-------|
| Coordinate mapping | 298-335 | ~15 | UV → source_x, px_in_source |
| Loop region tint | 338-369 | ~10 | if-branch, smoothstep |
| Beat grid | 388-417 | ~25 | 2× fract, threshold, mix |
| **Stem loop (×4 stems)**: | | | |
| → sample_peak + minmax_reduce | 602, 101-177 | ~60 | SSBO read + reduction + grid interp |
| → Envelope + peak width | 604-623 | ~40 | multiply, clamp, expand |
| → Edge AA (algo 3 / L2 clamped) | 632-650 | ~100 | dpdx×2, dpdy×2, length×2, clamp×2, smoothstep×2 |
| → Depth fade | 662-663 | ~30 | smoothstep, clamp, multiply |
| → Color blend | 664-672 | ~25 | blend_over (4 multiplies + adds) |
| → **Subtotal per stem** | | **~255** | |
| **4 stems total** | | **~1020** | |
| Cue markers (×8 max) | 680-711 | ~60 | loop, abs, smoothstep per cue |
| Playhead | 714-733 | ~15 | abs, smoothstep |
| Volume dimming | 737-742 | ~10 | multiply, blend |
| Stem indicators (zoomed) | 747-816 | ~80 | 4-iteration loop, conditions, blends |
| **Total per pixel** | | **~1,235** | At quality=0, 1.0 pp/px |

Note: "float ops" here counts individual FP32 operations. Each `smoothstep` ≈ 5 ops, each `length(vec2)` ≈ 4 ops, each `blend_over` ≈ 8 ops.

#### Total Workload Per Frame

```
1.85M fragments × ~1,235 float ops = ~2.28 billion float ops per frame
At 60 Hz: ~137 GFLOPS sustained throughput required
```

#### GPU Capacity

| GPU | Peak GFLOPS | Realistic Sustained* | Utilization at 137 GFLOPS |
|-----|-------------|---------------------|--------------------------|
| Vega 7 (RADV) | 1,434 | ~700-1,000 | **14-20%** → comfortable |
| Mali G610 (Panfrost) | 512 | **~130-180** | **76-105%** → at the edge |

*Sustained = peak × driver efficiency × occupancy. RADV achieves ~50-70%; Panfrost ~25-35%.

**With 1 track (4 views ≈ 924K fragments)**: ~68 GFLOPS → Mali at ~38-52% → OK
**With 2 tracks (8 views ≈ 1.85M fragments)**: ~137 GFLOPS → Mali at ~76-105% → **JANK**

#### The Edge AA Is the Biggest Per-Pixel Cost

The edge AA section (lines 632-650) accounts for ~100 float ops per stem — nearly **40% of the per-stem cost** and **33% of the total per-pixel cost**. The L2 Clamped algorithm (algo=3) computes:

```wgsl
fw_top = clamp(length(vec2<f32>(dpdx(d_top), dpdy(d_top))), fw, fw * 3.0);
fw_bot = clamp(length(vec2<f32>(dpdx(d_bot), dpdy(d_bot))), fw, fw * 3.0);
let aa_top = smoothstep(-fw_top * blur_outside_mult(), fw_top * blur_inner_mult(), d_top);
let aa_bot = smoothstep(-fw_bot * blur_outside_mult(), fw_bot * blur_inner_mult(), d_bot);
```

That's 4 derivative operations, 2 vector lengths, 2 clamps, 2 smoothsteps, and 4 multiplies — **per stem, per pixel**. On Mali, derivatives (`dpdx`/`dpdy`) are computed via helper lane differences in a quad, which is additional cross-lane communication overhead.

---

### 4.2 SSBO Cache Behavior — Corrected

**Severity**: Medium (not the primary issue at quality=0)

#### At Quality=0, 1.0 pp/px: Cache Is NOT the Problem

The `minmax_reduce` function (lines 101-121) loops based on the range of peaks to reduce:

```wgsl
let range = e - s + 1u;
let step = max(1u, range / 64u);
```

At 1.0 pp/px, `range ≈ 1`, so `step = 1` and the loop executes **exactly once**. Each pixel performs only **4 SSBO reads** (one per stem). The per-view working set is:

```
960 visible pixels × 1.0 pp/px × 4 stems × 8 bytes = 30,720 bytes = 30 KB
```

This fits comfortably in Mali's ~128 KB L2 cache. **There is no cache thrashing at this setting.**

#### Where Cache Matters: High pp/px

At wider zoom levels or higher quality settings, `minmax_reduce` iterates more:

| Setting | pp/px | Iters/Stem (no abstraction) | Iters/Stem (with abstraction, 2×calls) | SSBO Reads/Pixel |
|---------|-------|----------------------------|-----------------------------------------|-----------------|
| Q0, 4 bars | 1.0 | 1 | 2×1 = 2 | 4 (no abstr) / 8 |
| Q0, 8 bars | 2.0 | 2 | 2×2 = 4 | 8 / 16 |
| Q0, 16 bars | 4.0 | 4 | 2×5 = 10 | 16 / 40 |
| Q0, 32 bars | 8.0 | 8 | 2×10 = 20 | 32 / 80 |
| Q1, 4 bars | 2.0 | 2 | 2×3 = 6 | 8 / 24 |
| Q2, 4 bars | 4.0 | 4 | 2×5 = 10 | 16 / 40 |
| Q3, 4 bars | 8.0 | 8 | 2×10 = 20 | 32 / 80 |

The "up to 256 reads per pixel" from the original analysis **only occurs at quality=3 with 32-bar zoom and abstraction enabled** — an extreme scenario.

#### Texture Conversion: Still Beneficial But Not Urgent

Converting peaks from SSBO to a 2D texture (`texture_2d<f32>`, width=peaks, height=stems) would:
- Move reads from the load/store unit to the texture unit (~20-50% faster per read on Mali)
- Enable hardware-assisted linear interpolation (could eliminate the double-call interpolation in `sample_peak`)
- Benefit from Mali's texture prefetcher

But since the primary bottleneck is ALU (not memory), this optimization is **medium priority**, not critical.

---

### 4.3 Panfrost Driver Efficiency

**Severity**: High (but not fixable by us)

The Panfrost driver is reverse-engineered and has known shader compilation inefficiencies:

- **Register pressure**: The waveform fragment shader has 15+ branches, 4 nested loops, and uses derivative ops. Panfrost's register allocator may spill to stack memory, causing severe performance degradation.
- **Instruction scheduling**: Complex control flow (`loop` with dynamic break, multiple `switch` statements, nested `if/else`) may produce suboptimal Mali ISA instruction scheduling.
- **Sustained throughput**: Real-world Mali G610 + Panfrost achieves roughly 25-35% of the theoretical 512 GFLOPS peak, compared to RADV achieving 50-70% of Vega 7's peak.

This means the **same shader source** runs ~2x worse on Mali/Panfrost than it would on Mali/proprietary-driver, compounding the hardware gap.

---

### 4.4 Memory Pressure

**Severity**: Low (not the bottleneck with 2 tracks)

#### Audio Data

| Tracks | Audio Buffers | Peak Buffers | Total GPU+Audio |
|--------|--------------|--------------|-----------------|
| 1 track (320s, 172 BPM) | 461 MB | 3.4 MB | ~465 MB |
| 2 tracks | 922 MB | 6.9 MB | ~929 MB |
| 4 tracks | 1,843 MB | 13.8 MB | ~1,857 MB |

On 8 GB: 4 tracks = ~1.9 GB + ~500 MB OS + ~200 MB app = **~2.6 GB used, ~5.4 GB free**. Memory is not the constraint.

The GPU peak buffers (13.8 MB for 4 decks) are negligible. **No peak count reduction needed for memory reasons.**

---

### 4.5 Per-Frame Debug Logging

**File**: `crates/mesh-widgets/src/waveform/shader/mod.rs`, lines 396-406
**Severity**: Low (but trivially fixable)

The `log::debug!` in `build_uniforms()` evaluates 14 format arguments (including floats) 480 times/second. Without `max_level_info`, this is not compiled away in release builds.

---

### 4.6 Rayon on Heterogeneous Cores

**File**: `crates/mesh-core/src/effect/multiband.rs`, line 1547
**Severity**: Low (unless multi-band effects are active)

Rayon doesn't distinguish A76 (fast) from A55 (slow) cores. In single-band mode (default), `par_iter_mut()` on 1 element adds overhead with zero benefit.

---

### 4.7 Audio Thread Budget

**Severity**: Low (well-designed, not bottlenecked)

Lock-free architecture with atomic state. 256-sample buffer at 48 kHz = 5.3 ms callback interval. Passthrough mode uses ~1.6 ms for 16 stems. Not a problem.

---

## 5. Proposed Solutions

### 5.1 Quick Wins (hours of work)

#### 5.1a. Skip Rendering Unloaded Decks

**Impact**: Up to 50% GPU reduction (with 2 of 4 decks empty)
**Effort**: ~30 minutes

The shader already has an early return at line 294-296:

```wgsl
if (has_track < 0.5 || pps == 0u) {
    return color;  // Dark background, no shader work
}
```

But the **draw call itself still happens** — the GPU must still bin the fullscreen triangle to all tiles and dispatch fragments. Skip the `draw()` call entirely on the CPU side when a deck has no track loaded:

```rust
// In pipeline.rs render():
if !resources.has_track {
    return;  // Skip draw call entirely, don't even submit to GPU
}
```

This avoids TBDR tile binning overhead and 300K+ fragment shader invocations per empty deck.

#### 5.1b. Use Simple AA on ARM Targets

**Impact**: ~33% reduction in per-pixel ALU cost
**Effort**: ~15 minutes (uniform change)

Switch from L2 Clamped AA (algo=3) to Standard AA (algo=0) on the OPi5. This eliminates the 4 derivative ops, 2 vector lengths, 2 clamps per stem — saving ~100 ops/stem × 4 stems = **400 ops per pixel** (32% of total).

```rust
// In build_uniforms(), detect ARM target:
let edge_aa_algo = if cfg!(target_arch = "aarch64") { 0.0 } else { 3.0 };
```

Visual impact: slightly wobblier diagonal edges on zoomed waveforms. Flat/horizontal edges look identical.

#### 5.1c. Disable Depth Fade and Stem Indicators on ARM

**Impact**: ~15% reduction in per-pixel ALU
**Effort**: ~15 minutes

Depth fade (lines 662-663) adds ~30 ops/stem × 4 = 120 ops. Stem indicators (lines 747-816) add ~80 ops. Total saving: ~200 ops per pixel.

```rust
// Depth fade level 0 = disabled:
let depth_fade_level = if cfg!(target_arch = "aarch64") { 0 } else { user_setting };
```

#### 5.1d. Compile Away Debug Logs

**Impact**: Small but free
**Effort**: 1 line change

```toml
log = { version = "0.4", features = ["max_level_info"] }
```

#### 5.1e. Add Frame Timing Instrumentation

**Impact**: Diagnostic — confirms CPU vs GPU bottleneck
**Effort**: ~30 minutes

```rust
fn update(app: &mut MeshApp, message: Message) -> Task<Message> {
    let start = std::time::Instant::now();
    let result = app.update(message);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    if ms > 5.0 { log::warn!("update: {:.1}ms", ms); }
    result
}
```

#### 5.1f. Cap Rayon Thread Pool on ARM

**Impact**: Prevents A55 bottleneck in multiband processing
**Effort**: ~15 minutes

```rust
#[cfg(target_arch = "aarch64")]
rayon::ThreadPoolBuilder::new().num_threads(4).build_global().ok();
```

### Combined Quick-Win Estimate

| Fix | Ops Saved/Pixel | Fragment Reduction | Est. GPU Time Saved |
|-----|----------------|-------------------|-------------------|
| Skip empty decks | 0 per pixel | -50% fragments (2 decks empty) | ~8-10 ms |
| Simple AA | ~400 | 0 | ~4-5 ms |
| No depth fade + indicators | ~200 | 0 | ~2-3 ms |
| **Total (2 tracks, 2 empty decks)** | | | **~10-14 ms saved** |

With all quick wins applied (2 tracks loaded):
- 4 active views (skip 4 empty): ~924K fragments
- ~635 ops/pixel (down from ~1,235): ~587M float ops/frame
- ~35 GFLOPS sustained needed → Mali at ~20-27% → **smooth at 60 Hz**

**This should fix the jank without any shader/peak architecture changes.**

---

### 5.2 Medium-Term (days of work)

#### 5.2a. "Mali Quality" Shader Profile

Create a uniform-driven quality preset that reduces per-pixel work:

| Feature | Desktop | Mali Profile |
|---------|---------|-------------|
| Edge AA | L2 Clamped (algo=3) | Standard (algo=0) |
| Depth fade | Level 1-3 | Disabled (level=0) |
| Stem indicators | Enabled | Disabled |
| Motion blur | User setting | Disabled |
| Abstraction | User setting | Off (saves double minmax_reduce call) |
| Cue markers | smoothstep AA | step() AA |
| Peak width expansion | Enabled | Simplified |

Detect via `wgpu::Adapter::get_info()`:

```rust
let info = adapter.get_info();
let is_mali = info.name.contains("Mali") || info.driver.contains("panfrost");
```

#### 5.2b. Convert Peak Data to 2D Texture

**Impact**: ~20-50% faster SSBO reads on Mali (moves to texture unit)
**Effort**: ~2 days

```wgsl
// Before:
@group(0) @binding(1)
var<storage, read> peaks: array<vec2<f32>>;

// After:
@group(0) @binding(1)
var peak_texture: texture_2d<f32>;  // RG16Float, width=peaks, height=stems
@group(0) @binding(2)
var peak_sampler: sampler;          // nearest or linear
```

Benefits on Mali:
- Texture unit has dedicated L1 cache separate from load/store
- Hardware prefetcher optimized for spatial locality
- With linear filtering: could eliminate `minmax_reduce` entirely at low pp/px

**Caveat**: `RG32Float` textures may not be filterable on all hardware. Use `RG16Float` (half-precision — sufficient for normalized [-1, 1] audio peaks, ~0.001 precision).

#### 5.2c. Reduce Draw Calls via Instancing

**Impact**: Moderate (reduces TBDR tile binning overhead)
**Effort**: ~1 day

Batch 8 draw calls into 2 (one for zoomed, one for overview) using instanced rendering. The vertex shader uses `@builtin(instance_index)` to select deck-specific uniforms and viewport offsets.

#### 5.2d. Reduce `minmax_reduce` Iteration Cap on ARM

**Impact**: Only matters at high pp/px (zoom-out, higher quality)
**Effort**: ~1 hour

```wgsl
let max_iters = u32(u.platform_params.x);  // 64 on desktop, 16 on ARM
let step = max(1u, range / max_iters);
```

---

### 5.3 Long-Term (weeks of work)

#### 5.3a. Pre-Computed LOD Mipmap Chain for Peaks

Pre-compute a hierarchy of reduced peaks at track load time:

```
Level 0: 110K peaks (original, 1.0 pp/px at 4-bar)
Level 1: 55K peaks (0.5 pp/px at 4-bar, 1.0 at 8-bar)
Level 2: 27.5K peaks (1.0 at 16-bar)
Level 3: 13.75K peaks (1.0 at 32-bar)
...
```

The shader selects the appropriate level based on zoom, always getting ~1.0 pp/px, eliminating `minmax_reduce` entirely. Storage cost: ~2x original (geometric series) = ~7 MB per deck.

#### 5.3b. Vulkan Compute Pre-Pass for Peak Reduction

Run a compute shader that reduces peaks to exactly the viewport resolution, writing to a texture. The fragment shader then does 1 texture lookup per stem per pixel. This cleanly separates data reduction from rendering.

---

## 6. Profiling & Measurement

### 6.1 Runtime Resource Meters

#### CPU & RAM: `sysinfo` crate

Already in workspace (`mesh-core/Cargo.toml`, version 0.33):

```rust
use sysinfo::System;
let mut sys = System::new();
sys.refresh_cpu_usage();
sys.refresh_memory();

let cpu_pct: f32 = sys.global_cpu_usage();
let used_ram: u64 = sys.used_memory();
let total_ram: u64 = sys.total_memory();
```

#### GPU: Tiered Fallback

1. **Rockchip vendor kernel**: `/sys/class/devfreq/fb000000.gpu/load` — `<load>@<freq>Hz`
2. **Mainline devfreq**: `/sys/class/devfreq/fb000000.gpu/cur_freq` — frequency only
3. **DRM fdinfo** (needs udev rule): `/proc/self/fdinfo/<fd>` — engine time deltas
4. **Desktop (AMD)**: `/sys/class/drm/card1/device/gpu_busy_percent` (currently reads `6` at idle)
5. **Unavailable**: Display "N/A"

#### Widget: `mesh-widgets/src/resource_monitor/`

Shared widget for both mesh-player and mesh-cue. Update via 1-second `iced::time::every()`.
Display: `CPU 45% | RAM 2.1/7.6 GB | GPU 78%`

---

### 6.2 Frame Timing Instrumentation

```
Total frame time = tick_to_tick_interval
CPU time = update_duration + view_duration
GPU time (estimate) = total - CPU time - vsync_wait
```

**Diagnosis**:
- `update + view > 10ms` → CPU bottleneck
- `update + view < 3ms` but frames drop → GPU bottleneck
- Neither slow → presentation/driver issue

---

### 6.3 iced Comet Debugger

Add `"debug"` to iced features, press F12 at runtime.

---

### 6.4 Tracy / Perfetto GPU Profiling

- **Tracy**: `profiling` crate with Tracy backend, connect over TCP
- **Perfetto + gfx-pps**: Mali hardware counters (core utilization, cache hit rates)
- **perf + flamegraph**: CPU hotspots on ARM64 with `--call-graph dwarf`

---

## 7. Reference Data

### Peak Data from Dev Laptop (quality=0)

```
Track: 320.3s, 172 BPM, 48 kHz
Peaks per stem: 110,189
Peaks per pixel at 4-bar zoom: 1.00
Samples per bar: 66,976
Screen width: 1920px

Per-stem buffer:  110,189 × 8 bytes = 861 KB
Per-deck (4 stems): 3.44 MB
4 decks: 13.8 MB
```

### Per-Pixel Shader Cost Comparison

| Pixel Cost Component | Full Quality | Mali Optimized (proposed) |
|---------------------|-------------|--------------------------|
| Coordinate mapping | 15 ops | 15 ops |
| Beat grid | 25 ops | 25 ops |
| Stem loop (×4 stems): | | |
| → Peak sampling | 60 ops | 60 ops |
| → Envelope | 40 ops | 40 ops |
| → Edge AA | **100 ops** | **15 ops** (algo=0) |
| → Depth fade | **30 ops** | **0 ops** (disabled) |
| → Color blend | 25 ops | 25 ops |
| Cue markers | 60 ops | 60 ops |
| Playhead | 15 ops | 15 ops |
| Volume dim | 10 ops | 10 ops |
| Stem indicators | **80 ops** | **0 ops** (disabled) |
| **Total** | **~1,235 ops** | **~705 ops** |
| **Reduction** | — | **~43%** |

### Frame Budget at 60 Hz

| Component | Budget |
|-----------|--------|
| Total frame | 16.67 ms |
| iced UI overhead | ~3-5 ms |
| **Available for waveform GPU** | **~11-14 ms** |

### GPU Throughput Summary

| GPU | Peak GFLOPS | Sustained GFLOPS | Budget (4 views) | Budget (8 views) |
|-----|-------------|-------------------|-------------------|-------------------|
| Vega 7 + RADV | 1,434 | ~800 | ~8.5% | ~17% |
| Mali G610 + Panfrost | 512 | ~150 | ~45% | ~91% |
| Mali G610 + Panfrost (Mali profile) | 512 | ~150 | ~26% | ~52% |

### Key Source Files

| File | What | Hot Path? |
|------|------|-----------|
| `mesh-widgets/src/waveform/shader/waveform.wgsl:281-819` | Fragment shader | Yes (per-pixel) |
| `mesh-widgets/src/waveform/shader/waveform.wgsl:101-121` | `minmax_reduce` loop | Yes (per-stem-per-pixel) |
| `mesh-widgets/src/waveform/shader/waveform.wgsl:632-650` | Edge AA (biggest cost) | Yes |
| `mesh-widgets/src/waveform/shader/mod.rs:396` | `build_uniforms()` debug log | Yes (8×/frame) |
| `mesh-widgets/src/waveform/shader/pipeline.rs:175` | Alpha blending config | Config |
| `mesh-widgets/src/waveform/peaks.rs:40` | `compute_highres_width()` | No (load-time) |
| `mesh-core/src/effect/multiband.rs:1547` | `bands.par_iter_mut()` | Yes (audio thread) |
| `mesh-player/src/ui/handlers/tick.rs` | Per-frame sync | Yes (60×/sec) |

### GPU Monitoring Paths (Linux)

| Path | Platform | Root? | Gives |
|------|----------|-------|-------|
| `/sys/class/drm/card*/device/gpu_busy_percent` | AMD (amdgpu) | No | GPU busy % |
| `/sys/class/devfreq/fb000000.gpu/load` | Rockchip vendor | No | `<load>@<freq>Hz` |
| `/sys/class/devfreq/fb000000.gpu/cur_freq` | Any devfreq | No | Frequency Hz |
| `/proc/self/fdinfo/<fd>` | Mainline 6.4+ | No (own proc) | Engine time ns |
| `/sys/bus/platform/drivers/panfrost/*/profiling` | Mainline | Yes (write) | Enable fdinfo |
