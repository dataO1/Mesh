# Waveform Supersampling Research Report

*Date: 2026-02-22*

## 1. Current Architecture Summary

The waveform renderer uses the **industry-recommended fullscreen triangle + fragment shader** approach:

| Aspect | Implementation | Status |
|--------|---------------|--------|
| Rendering | Single fullscreen triangle, all waveform computed in fragment shader | Optimal |
| Data storage | `var<storage, read> peaks: array<f32>` (wgpu SSBO) | Optimal |
| Anti-aliasing | Analytical `smoothstep()` + `fwidth()` per-fragment | Optimal |
| Draw calls | 1 per view, 3 vertices, no vertex buffer | Optimal |
| Peak upload | Once at track load, `Arc::as_ptr()` change detection | Efficient |
| Uniforms | 384 bytes/frame per view | Negligible |

The single oversized triangle in `vs_main` (vertices at `(-1,-1)`, `(3,-1)`, `(-1,3)`) is clipped by the GPU to fill the viewport. The fragment shader computes for each pixel: "is this pixel inside the waveform envelope?" — an SDF-like approach. This is the same technique recommended by the Blender VSE team and validated by Michal Drobot's GCN cache analysis (~10% better L1/L2 hit ratio than a two-triangle quad).

## 2. The Fixed 65536 Peak Problem

### Current behavior

`peaks.rs:18` — `HIGHRES_WIDTH = 65536` peaks per stem, regardless of track length.

| Track Length | Samples (48kHz) | Samples/Peak | Peaks visible at 8-bar zoom (1920px) |
|-------------|-----------------|-------------|--------------------------------------|
| 2 min | 5.76M | ~88 | ~5.4 peaks/pixel |
| 5 min | 14.4M | ~220 | ~2.2 peaks/pixel |
| 8 min | 23.0M | ~352 | ~1.4 peaks/pixel |
| 12 min | 34.6M | ~528 | ~0.9 peaks/pixel |

For long tracks at close zoom (1-4 bars), fewer than 1 peak per pixel means the shader interpolates between sparse data points, producing a smoothed-out approximation rather than true audio detail. For short tracks, memory is wasted on finer peaks than needed.

### Pixel-count rendering analysis

The maximum peaks-per-pixel ratio that produces visible quality improvement:

| Peaks/Pixel | Effect |
|-------------|--------|
| < 1.0 | Interpolation artifacts — visible smoothing, loss of transients |
| 1.0 | Pixel-perfect — each pixel shows exactly one peak's min/max |
| 2.0-3.0 | Mild supersampling — the shader's `minmax_reduce` picks true envelope |
| > 4.0 | Diminishing returns — extra peaks don't change the visible envelope |

The shader's `get_subsample_target()` already uses 2.0-3.0 as the grid step.

## 3. Rendering Approach Comparison

### Research verdict: Fragment shader (current approach) is best

| Approach | Vertices/frame | CPU work/frame | GPU efficiency | AA quality |
|----------|---------------|----------------|----------------|------------|
| **Triangle strips** (old canvas) | ~16K+ | ~1MB tessellation | Poor (geometry overhead) | Needs MSAA |
| **Instanced rectangles** (1 per column) | 6 × columns | Instance buffer upload | Medium (overdraw) | Needs MSAA |
| **Fullscreen triangle + fragment shader** (current) | 3 | 384 bytes uniform | Excellent (cache-coherent) | Free analytical AA |
| **Compute shader → texture** | 0 (dispatch) | Dispatch command | Excellent | Manual |

The Blender VSE team specifically moved from per-column quads to an SSBO + fragment shader approach. The fragment shader approach wins because:

1. **Zero geometry generation** — no CPU tessellation, no vertex buffers
2. **Cache-coherent access** — adjacent pixels read adjacent peak indices
3. **Free analytical AA** — `smoothstep(fwidth(...))` gives resolution-independent anti-aliasing
4. **Single draw call** — 1 draw per view vs hundreds/thousands for geometry-based

## 4. Anti-Aliasing Assessment

### MSAA: Not applicable

MSAA anti-aliases **triangle edges**, not fragment-shader-computed shapes. Since the waveform is computed entirely in `fs_main`, MSAA would only smooth the viewport boundary of the fullscreen triangle. The pipeline correctly uses `MultisampleState::default()` (count=1).

### FXAA: Inferior

FXAA is a post-process that detects edges and blurs them. For waveforms:
- Blurs beat lines, playhead, and cue markers along with waveform edges
- Misses sub-pixel features (thin waveform envelopes)
- Adds a full-screen post-process pass
- Produces "soft/muddy" appearance

### Analytical AA (current): Optimal

The shader implements the gold standard technique:

```wgsl
let fw = fwidth(uv.y);           // pixel size in UV space — resolution-independent
let aa_top = smoothstep(-outside_ext, fw, d_top);
let aa_bot = smoothstep(-outside_ext, fw, d_bot);
```

Plus the thin-envelope handler for sub-pixel coverage. No changes needed.

## 5. Recommended Quality Tiers

### Dynamic peak resolution

Replace fixed `HIGHRES_WIDTH = 65536` with track-relative calculation:

```rust
fn compute_highres_width(total_samples: usize, quality: WaveformQuality) -> usize {
    let divisor = match quality {
        WaveformQuality::Low    => 128,  // ~65K for 5-min track
        WaveformQuality::Medium => 32,   // ~450K for 5-min track
        WaveformQuality::High   => 8,    // ~1.8M for 5-min track
        WaveformQuality::Ultra  => 2,    // ~7.2M for 5-min track
    };
    let raw = total_samples / divisor;
    raw.next_power_of_two().clamp(65536, 8_388_608)
}
```

| Quality | 5-min peaks | Memory (4 stems) | Samples/Peak | Peaks/pixel at 4-bar zoom |
|---------|------------|-------------------|-------------|--------------------------|
| Low | 65,536 | 2 MB | ~220 | ~2.2 |
| Medium | 524,288 | 16.8 MB | ~27 | ~18 |
| High | 2,097,152 | 67 MB | ~7 | ~72 |
| Ultra | 8,388,608 | 268 MB | ~1.7 | ~290 |

GPU buffer limits: wgpu default `max_storage_buffer_binding_size` = 128 MiB. Native desktop (Vulkan/Metal/D3D12) can request up to 2 GiB. Even Ultra's 268 MB (8-stem linked) needs elevated limits but is within hardware capability.

### vec2 peak packing

Change storage buffer from `array<f32>` to `array<vec2<f32>>`. Halves buffer read instructions and improves 8-byte alignment:

```wgsl
// Before:
@group(0) @binding(1) var<storage, read> peaks: array<f32>;
fn raw_peak(...) { return vec2(peaks[base], peaks[base + 1]); }

// After:
@group(0) @binding(1) var<storage, read> peaks: array<vec2<f32>>;
fn raw_peak(...) { return peaks[base]; }
```

## 6. What NOT to change

| Component | Why it's already right |
|-----------|----------------------|
| Fullscreen triangle rendering | Optimal cache coherency, zero geometry overhead |
| Fragment shader waveform | SDF approach gives free AA, single draw call |
| `smoothstep` + `fwidth` AA | Resolution-independent, zero memory cost |
| Storage buffer for peaks | Sequential access = optimal for SSBO (96 GB/s read-only) |
| Two-tier peaks (overview + highres) | Overview at 800 peaks is fine for full-track view |
| Grid-aligned sampling in shader | Prevents temporal jitter during playback scroll |
| `Arc::as_ptr()` change detection | Avoids redundant GPU uploads |

## 7. Memory Budget Analysis

### Per-deck worst case (8-stem linked, Ultra quality, 12-min track)

```
12 min × 48000 Hz = 34,560,000 samples
34,560,000 / 2 = 17,280,000 peaks → next_power_of_two → 33,554,432
Clamped to 8,388,608
8,388,608 peaks × 8 stems × 2 values × 4 bytes = 536 MB
```

This exceeds the wgpu 128 MiB default. Options:
- Request elevated `max_storage_buffer_binding_size` (hardware supports it)
- Clamp Ultra to a lower ceiling for linked stems
- Use a mipmap hierarchy to keep the GPU buffer smaller while maintaining quality

### Practical recommendation

Clamp peak count so the buffer stays under 128 MiB (safe default):
- 4 stems: max ~4M peaks/stem (128 MB)
- 8 stems (linked): max ~2M peaks/stem (128 MB)

Ultra quality would auto-downgrade to High for linked stems on hardware with default limits.

## Sources

- [GCN Execution Patterns in Full Screen Passes — Michal Drobot](https://michaldrobot.com/2014/04/01/gcn-execution-patterns-in-full-screen-passes/)
- [Optimizing Triangles for a Full-screen Pass — Chris Wallis](https://wallisc.github.io/rendering/2021/04/18/Fullscreen-Pass.html)
- [Analytical Anti-Aliasing — frost.kiwi](https://blog.frost.kiwi/analytical-anti-aliasing/)
- [SDF Anti-aliasing — Red Blob Games](https://www.redblobgames.com/blog/2024-09-22-sdf-antialiasing/)
- [Texture and Buffer Access Performance — RasterGrid](https://www.rastergrid.com/blog/2010/11/texture-and-buffer-access-performance/)
- [wgpu Limits documentation](https://docs.rs/wgpu/latest/wgpu/struct.Limits.html)
- [Blender VSE waveform rendering PR #115311](https://projects.blender.org/blender/blender/pulls/115311)
- [BBC audiowaveform Data Format](https://github.com/bbc/audiowaveform/blob/master/doc/DataFormat.md)
- [MeadowlarkDAW audio-waveform-mipmap](https://github.com/MeadowlarkDAW/audio-waveform-mipmap)
- [gl-waveform — WebGL waveform renderer](https://github.com/dy/gl-waveform)
- [FXAA — Coding Horror](https://blog.codinghorror.com/fast-approximate-anti-aliasing-fxaa/)
