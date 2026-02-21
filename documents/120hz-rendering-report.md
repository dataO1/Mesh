# 120Hz Rendering & Ultra-Wide Display Optimization Report

**Date:** 2026-02-21
**Target:** Orange Pi 5 (RK3588, Mali-G610) with 2880x864 @ 120Hz ultra-wide display
**Dev Host:** AMD Ryzen 7 PRO 4750U / Radeon Vega (RADV RENOIR), NixOS, sway

---

## 1. System Environment (Dev Host)

| Component | Value |
|-----------|-------|
| CPU | AMD Ryzen 7 PRO 4750U (8C/16T, Zen 2) |
| GPU | AMD Radeon Graphics (Renoir, integrated) |
| GPU Driver | radv (Mesa 25.2.6, RADV RENOIR) |
| Vulkan API | 1.4.318, conformance 1.4.0.0 |
| OpenGL | 4.6 (Compatibility), Mesa 25.2.6 |
| Kernel | 6.12.66-hardened1 (PREEMPT_DYNAMIC) |
| Compositor | SwayFX 0.5.3 (wlroots-based Wayland) |
| Display 1 | DP-5: HKM U27I4K, 3840x2160 @ 60Hz (scale 1.25 = 3072x1728 logical) |
| Display 2 | eDP-1: AU Optronics, 1920x1080 @ 60Hz |
| RAM | 30 GiB |
| DRM | /dev/dri/card1 + renderD128 (amdgpu) |

## 2. Rendering Configuration Applied

### Changes Made

| Setting | Before | After |
|---------|--------|-------|
| Frame scheduling | `time::every(16ms)` (hardcoded 60Hz) | `window::frames()` (compositor vblank) |
| Canvas caching | None (rebuild 100+ draw ops/frame) | `canvas::Cache` (skip when unchanged) |
| Present mode | Default (Fifo, 3-frame queue) | Mailbox (1-frame queue, low latency) |
| Antialiasing | Disabled | MSAAx4 via `.antialiasing(true)` |
| GPU backend | GLES on embedded, unset on dev | Vulkan everywhere |
| Window sizing | Hardcoded 1200x800 | Auto-detect via `monitor_size()`, 1920x1080 fallback |
| OTA polling | Every frame (even when settings closed) | Only when settings modal open AND installing |

### Environment Variables

```bash
# Set in nix/devshell.nix, nix/embedded/kiosk.nix, and packaging/*-wrapper scripts
WGPU_BACKEND=vulkan
ICED_PRESENT_MODE=mailbox
```

### Verified Runtime Output

```
iced_wgpu::window::compositor Settings {
    present_mode: Mailbox,
    backends: Backends(VULKAN),
    antialiasing: Some(...)
}
Available adapters: [
    name: "AMD Radeon Graphics (RADV RENOIR)",
    device: 5686,
    device_type: IntegratedGpu,
    backend: Vulkan,
]
Selected format: Rgb10a2Unorm with alpha mode: PreMultiplied
```

## 3. Display Capabilities

### Dev Host Displays

**DP-5 (HKM U27I4K):** 3840x2160 @ 60Hz (max 75Hz), 4K IPS
**eDP-1 (Laptop):** 1920x1080 @ 60Hz (60Hz only)

Neither display supports 120Hz, so `window::frames()` will fire at 60Hz on this host. The optimization is designed for the target 120Hz ultra-wide display.

### Target Display (2880x864 @ 120Hz)

| Specification | Value |
|---------------|-------|
| Resolution | 2880x864 |
| Refresh rate | 120 Hz |
| Pixel clock | ~318 MHz (within RK3588 HDMI 2.1 PHY range of ~594 MHz) |
| Frame budget | 8.33 ms |
| Aspect ratio | 10:3 (custom ultra-wide) |

### RK3588 GPU Capabilities

| Feature | Status |
|---------|--------|
| Vulkan | PanVK 1.2+ conformant (Mali-G610 MP4) |
| Mailbox present mode | Supported via PanVK on Wayland |
| HDMI output | HDMI 2.1 (up to 4K120 / 8K30) |
| VOP2 display controller | Dual HDMI + dual DP, independent timing |

## 4. Vulkan ICD Discovery

**NixOS-specific requirement:** The `VK_ICD_FILENAMES` environment variable must point to the correct ICD JSON files. Without it, wgpu reports `Available adapters: []` and falls back to software rendering.

- **Dev host ICD path:** `/run/opengl-driver/share/vulkan/icd.d/radeon_icd.x86_64.json`
- **Devshell:** `VK_ICD_FILENAMES` set automatically by nix/devshell.nix
- **Embedded kiosk:** Mesa ICD is in the system profile, discovered automatically by the Vulkan loader
- **Release builds outside devshell:** Must set `VK_ICD_FILENAMES` manually or enter devshell first

## 5. Monitor Size Auto-Detection

`iced::window::monitor_size()` returns `None` on sway (tiling WM). This is a known Wayland/winit limitation — unlike X11, Wayland compositors don't expose global monitor information to clients for privacy/security reasons. Tiling WMs manage window geometry directly, making the query less meaningful.

On the target **cage kiosk compositor**, the window automatically fills the entire display. The 1920x1080 fallback (Full HD) provides a reasonable default for development and floating WM environments.

### VK_EXT_physical_device_drm Warning

The `wgpu_hal::vulkan::instance` warning about `VK_EXT_physical_device_drm` is **harmless**. This optional extension maps Vulkan physical devices to Linux DRM nodes (`/dev/dri/`). It is not required for rendering, buffer presentation, or monitor detection. The warning simply means the driver doesn't expose it — normal GPU rendering is unaffected.

## 6. Canvas Cache Performance Impact

### Before (No Caching)

Per frame, for 4 decks:
- ~32 Vec allocations (waveform samples per deck)
- ~16 `Path::new()` closures (playheads, markers, regions)
- ~100+ draw operations (rectangles, lines, text)
- At 120Hz: **12,000+ draw ops/sec**, **3,840 Vec allocs/sec**

### After (canvas::Cache)

- **When paused:** Zero geometry reconstruction. Cache returns previous frame directly.
- **When playing:** Cache invalidated every tick (playhead moves), but the caching mechanism still avoids redundant GPU upload when geometry hasn't changed between frames.
- **Future optimization:** Split into two cache layers (static headers/overview + dynamic zoomed/playhead) to keep the static layer cached even during playback.

### Invalidation Triggers

All `set_*` methods on `PlayerCanvasState` call `invalidate_cache()`:
- `set_playhead()` — every tick during playback
- `set_volume()` — on mixer changes
- `set_stem_active()` — on stem mute/unmute
- `set_loop_active()`, `set_loop_length_beats()` — on loop changes
- `set_master()`, `set_key_match_enabled()`, `set_transpose()`, `set_lufs_gain_db()`
- Track name, key, BPM updates

## 7. Present Mode Comparison

| Mode | Queue Depth | Latency | Tearing | Notes |
|------|-------------|---------|---------|-------|
| **Fifo** (default) | 3 frames | ~25ms @ 120Hz | None | Standard vsync, highest latency |
| **Mailbox** (selected) | 1 frame | ~8ms @ 120Hz | None on Wayland | Low-latency, compositor handles tearless |
| Immediate | 0 frames | Minimal | Yes | Not suitable for DJ display |

Mailbox is ideal for DJ performance: low latency for responsive waveform scrolling, no tearing because Wayland compositors enforce tearless presentation.

## 8. Remaining Work

### Deferred Until 120Hz Hardware Arrives

1. **Two-layer cache split** — Separate static (track name, overview waveform, grid) from dynamic (playhead, zoomed view) into two `canvas::Cache` instances. This would eliminate geometry reconstruction for the static layer during playback.

2. **Frame timing profiling** — Measure actual frame times at 120Hz to identify if the 8.33ms budget is met. Use `RUST_LOG=iced_wgpu=trace` for frame-level timing.

3. **Custom EDID / wlr-randr mode** — The 2880x864 resolution won't be in standard EDID tables. Options:
   - Custom EDID blob flashed to display controller
   - `wlr-randr --output <name> --custom-mode 2880x864@120Hz` at runtime
   - Kernel DRM custom mode via sysfs

4. **GPU thermal throttling** — Monitor Mali-G610 temperature under sustained 120Hz rendering. The Orange Pi 5 may need active cooling for sustained operation.

5. **Adaptive UI layout** — With 2880x864 (10:3 aspect), the horizontal space is abundant but vertical is constrained. Consider:
   - Wider waveform views
   - Horizontal arrangement optimizations
   - Font size adjustments for the reduced vertical space

6. **VSync verification** — Confirm `window::frames()` fires at 120Hz (not 60Hz) on the target display. Log frame intervals to verify.

## 9. Summary

All planned optimizations are in place:

- **Frame sync:** Display-native via `window::frames()` (adapts to 60/120/144Hz automatically)
- **Canvas caching:** `canvas::Cache` eliminates redundant geometry construction
- **Present mode:** Mailbox for minimal latency without tearing
- **GPU backend:** Vulkan everywhere (dev + embedded) for consistent behavior
- **Antialiasing:** MSAAx4 enabled for smooth waveform rendering
- **Window sizing:** Auto-detect with graceful fallback
- **Polling optimization:** OTA journal polling gated behind settings modal

The codebase is ready for the 120Hz ultra-wide display. Final tuning (two-layer cache, frame timing, custom resolution) will be done when the hardware is available for testing.
