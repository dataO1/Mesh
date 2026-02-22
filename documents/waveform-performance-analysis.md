# Waveform Rendering Performance Analysis

**Date:** 2026-02-21
**Problem:** Janky waveform rendering, worse with 4 decks open
**Root Cause:** canvas::Cache invalidated ~84 times per tick, making it useless during playback

---

## 1. The Smoking Gun: Cache Invalidation on Every Tick

Every `set_*` method on `PlayerCanvasState` calls `invalidate_cache()` **unconditionally**, even when the value hasn't changed. The tick handler calls these setters every frame in a 4-deck loop:

### Per-Tick Cache Invalidation Count

| Operation | Calls | Invalidations |
|-----------|-------|---------------|
| `set_playhead()` | 4 (per deck) | 4 |
| `set_master()` | 4 | 4 |
| `set_key_match_enabled()` | 4 | 4 |
| `set_transpose()` | 4 | 4 |
| `set_lufs_gain_db()` | 4 | 4 |
| `set_loop_length_beats()` | 4 | 4 |
| `set_loop_active()` | 4 | 4 |
| `set_volume()` | 4 | 4 |
| `set_stem_active()` | 16 (4×4 stems) | 16 |
| `set_loop_region()` | 8 (4×2 views) | 8 |
| `set_display_bpm()` | 4 | 4 |
| `set_slicer_region()` | 8 (4×2 views) | 8 |
| `set_linked_stem()` | 16 (4×4 stems) | 16 |
| **Total** | **84** | **84** |

**Result:** The cache we just added is cleared 84 times per tick, then rebuilt once for the draw. It provides zero benefit during playback — iced re-tessellates ALL 4 decks' geometry from scratch every single frame.

## 2. What Happens on Each Cache Rebuild

iced's canvas uses **lyon** for CPU-side path tessellation. Every cache miss triggers:

### Per-Frame CPU Work (4 Decks)

| Operation | Count | Cost |
|-----------|-------|------|
| `Vec::with_capacity(512)` allocations | 64+ (4 decks × 4 stems × 2 vecs × 2 views) | ~780 KB heap alloc |
| `Path::new()` closures | 32+ | Lyon tessellation setup |
| `line_to()` calls | 16,384 (4 decks × 4 stems × 2 views × 512 points) | Lyon vertex generation |
| `exp()` for Gaussian smoothing | 8,192+ (4 decks × 4 stems × 512 peaks) | Heavy math |
| `format!()` text formatting | 20+ (BPM, key, loop, gain, track name per deck) | String alloc |
| `stretch_peaks()` overview vecs | 16 (if BPM scaling) | ~64 KB alloc |
| Lyon fill tessellation | 32+ paths | CPU-intensive |

### Total estimated per-frame: 100+ heap allocations (~1 MB), 8K+ exp() calls, 16K+ vertex ops

At 60Hz this is **6,000+ allocations/sec** and **480K+ exp() calls/sec**. At 120Hz it doubles.

## 3. Why 4 Decks is 4× Worse

Everything scales linearly with deck count. The single `PlayerCanvas` draws all 4 decks in one canvas (optimal for GPU — one render pass), but the CPU tessellation cost is:

| Decks | Estimated Frame Time | 16.67ms Budget | Margin |
|-------|---------------------|----------------|--------|
| 1 | ~3-4 ms | 60 Hz | ~13ms |
| 2 | ~6-8 ms | 60 Hz | ~9ms |
| 4 | ~12-16 ms | 60 Hz | ~1-5ms |
| 4 | ~12-16 ms | 120 Hz (8.33ms) | **OVER BUDGET** |

With 4 decks, a single GC pause or scheduler hiccup pushes past the frame budget → visible jank.

## 4. iced Rendering Pipeline (How It Actually Works)

```
Path::new(|builder| { builder.line_to()... })     ← Your code
                    ↓
Lyon fill/stroke tessellator (CPU)                 ← Converts paths → triangles
                    ↓
Vec<SolidVertex2D> + Vec<u32> index buffer         ← CPU memory
                    ↓
wgpu staging belt upload → GPU buffers             ← GPU upload
                    ↓
Triangle render pass (separate from main for MSAA) ← GPU renders
                    ↓
MSAA resolve blit → final framebuffer              ← GPU composites
```

**Key insight:** The expensive part is lyon tessellation (CPU), NOT the GPU rendering. The GPU handles 16K triangles trivially. The CPU is the bottleneck.

## 5. Fixes — Priority Order

### Fix 1: Change-Guarded Setters (CRITICAL — eliminates 80+ unnecessary invalidations)

**Impact: Massive. Most setters receive the same value every tick.**

During normal playback, only `set_playhead()` actually changes every tick. `set_volume()`, `set_master()`, `set_stem_active()`, `set_loop_active()`, `set_key_match_enabled()`, `set_transpose()`, `set_lufs_gain_db()`, `set_display_bpm()`, `set_linked_stem()` are all constant between user interactions.

```rust
// Before (current):
pub fn set_volume(&mut self, idx: usize, volume: f32) {
    if idx < 4 {
        self.volume[idx] = volume;
        self.invalidate_cache();  // ALWAYS invalidates
    }
}

// After (change-guarded):
pub fn set_volume(&mut self, idx: usize, volume: f32) {
    if idx < 4 && self.volume[idx] != volume {
        self.volume[idx] = volume;
        self.invalidate_cache();  // Only if actually changed
    }
}
```

**Expected result:** During steady-state playback, invalidation drops from 84/tick to ~4/tick (just `set_playhead()` for playing decks). Non-playing decks cause zero invalidations.

### Fix 2: Two-Layer Cache Split (HIGH — keeps overview cached during playback)

Split `canvas_cache` into `static_cache` (headers, overview waveforms, beat grid, stem indicators) and `dynamic_cache` (zoomed waveforms, playhead lines, volume overlays). During playback, only `dynamic_cache` rebuilds.

**Impact:** The overview waveforms (~50% of total tessellation cost) stay cached throughout playback. Only the zoomed viewport around the playhead is re-tessellated per frame.

```rust
pub struct PlayerCanvasState {
    pub static_cache: Cache,   // overview + headers + grid
    pub dynamic_cache: Cache,  // zoomed waveforms + playheads
}

// In draw():
fn draw(&self, ...) -> Vec<Geometry> {
    let static_geo = self.state.static_cache.draw(renderer, size, |frame| {
        // Headers, overview waveforms, beat grid, cue markers
        // Only rebuilds on: track load, stem toggle, zoom change
    });
    let dynamic_geo = self.state.dynamic_cache.draw(renderer, size, |frame| {
        // Zoomed waveforms, playhead lines, loop highlights, volume overlays
        // Rebuilds every tick during playback
    });
    vec![static_geo, dynamic_geo]
}
```

Note: Two `Geometry` items in one `draw()` share the same render pass — no extra GPU cost.

### Fix 3: Reuse Vec Allocations (MEDIUM — eliminates ~100 allocs/frame)

Pre-allocate the waveform point vectors in the state struct instead of creating new ones each frame.

```rust
pub struct PlayerCanvasState {
    // Reusable buffers for waveform drawing (avoid per-frame allocation)
    upper_points: Vec<(f32, f32)>,
    lower_points: Vec<(f32, f32)>,
}

// In draw code:
self.upper_points.clear();  // Reuse capacity, no alloc
self.upper_points.extend(peaks.iter().map(|...| ...));
```

### Fix 4: Cache Stretched Peaks (MEDIUM — eliminates redundant overview computation)

`stretch_peaks()` creates 4 new Vecs per deck per frame for BPM-scaled overview waveforms. Cache the result and only recompute when BPM scaling factor `D` changes.

### Fix 5: Skip Gaussian Smoothing in Hot Path (LOW-MEDIUM)

`sample_peak_smoothed()` calls `exp()` for every visible peak. Options:
- Pre-smooth peaks at track load time (store smoothed variant)
- Use simpler box-filter approximation (sum/count instead of Gaussian weights)
- Skip smoothing for overview (already low resolution)

### Fix 6: Cache Formatted Text (LOW)

Store formatted strings (`format!("{:.1}", bpm)` etc.) in state and only reformat when values change. Minor win (~20 format calls/frame eliminated).

## 6. Future: Shader Widget (Nuclear Option)

If the above fixes aren't enough for 120Hz on Mali-G610, the iced `Shader` widget provides direct wgpu access. Waveform rendering moves entirely to a WGSL fragment shader:

- Peak data uploaded to GPU storage buffer **once** at track load
- Per-frame cost: update one 64-byte uniform (playhead position)
- Fragment shader samples peak buffer procedurally — zero CPU tessellation
- Renders inline in existing render pass (no MSAA pass break)
- Anti-aliasing via `smoothstep()` at envelope edges

This eliminates lyon, Path objects, Vec allocations, and all CPU waveform work. The GPU renders 4 waveforms in ~1 draw call. But it requires reimplementing the waveform renderer in WGSL, so it's a significant effort.

## 7. Single Canvas is Already Optimal

The current single `PlayerCanvas` (all 4 decks in one widget) is correct. iced's triangle rendering breaks the render pass for MSAA — multiple canvas widgets would cause multiple pass breaks. One canvas = one pass break = optimal GPU usage.

## 8. MSAA Impact

MSAAx4 adds one render pass break + one resolve blit. The cost is modest (~10-15% of GPU frame time for waveform edge pixels). Worth keeping for visual quality. The Shader widget approach would need manual `smoothstep()` AA since MSAA doesn't apply to shader primitives.

## Recommended Implementation Order

1. **Fix 1 (change-guarded setters)** — Immediate, high confidence. Reduces invalidations from 84/tick to ~4/tick. All setters in state.rs need `!= old_value` guard. Estimated time: 1 hour.

2. **Fix 2 (two-layer cache)** — Next priority. Splits overview (expensive) from zoomed (changes every tick). Requires refactoring draw() into two closures. Estimated time: 2-3 hours.

3. **Fix 3 (reuse Vecs)** — Can be done alongside Fix 2. Move point buffers into state struct. Estimated time: 30 minutes.

4. **Fixes 4-6** — Incremental wins, can be done later.

5. **Fix 6 (Shader widget)** — Only if 120Hz on Mali-G610 still drops frames after fixes 1-3.
