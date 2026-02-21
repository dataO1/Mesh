# 120Hz Rendering & Ultra-Wide Display Optimization

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Optimize mesh-player for 2880×864 @ 120Hz rendering on cage kiosk with Vulkan backend, canvas geometry caching, display-synced animation, and auto-resolution detection.

**Architecture:** Replace timer-based 60Hz tick with `window::frames()` for native refresh-rate animation. Add `canvas::Cache` to PlayerCanvas to eliminate per-frame geometry reconstruction. Switch embedded backend from GLES to Vulkan with Mailbox present mode. Auto-detect monitor size at startup.

**Tech Stack:** iced 0.14 (canvas::Cache, window::frames, window::monitor_size), wgpu 27 (PresentMode::Mailbox), cage/wlroots (wlr-randr), PanVK Vulkan 1.2

---

### Task 1: Replace timer-based tick with window::frames()

**Files:**
- Modify: `crates/mesh-player/src/ui/app.rs:1260-1262`

**Step 1: Replace time::every(16ms) with window::frames()**

In `crates/mesh-player/src/ui/app.rs`, change the subscription batch.

Replace:
```rust
// Update UI at ~60fps for smooth waveform animation
time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick),
```

With:
```rust
// Update UI synced to display refresh rate (60Hz, 120Hz, etc.)
iced::window::frames().map(|_| Message::Tick),
```

This uses the compositor's frame callback to drive animation at the native display refresh rate, rather than a hardcoded 16ms timer.

**Step 2: Verify the import exists**

Confirm that `iced::window::frames()` is available without additional imports — it's part of the `iced::window` module and returns `Subscription<Instant>`.

**Step 3: Build and verify**

Run: `cargo build -p mesh-player`
Expected: Compiles without errors.

**Step 4: Commit**

```bash
git add crates/mesh-player/src/ui/app.rs
git commit -m "perf(player): replace 60Hz timer with display-synced window::frames()"
```

---

### Task 2: Gate OTA journal polling behind settings modal

**Files:**
- Modify: `crates/mesh-player/src/ui/app.rs:1252-1258`

**Step 1: Add settings.is_open guard to journal polling**

The OTA journal poll subscription currently only checks `is_installing()`, but it should also require the settings modal to be open — the user only sees update progress in the settings panel.

Replace:
```rust
// Journal polling subscription for OTA update progress
let journal_poll_sub = if self.settings.update.as_ref().is_some_and(|u| u.is_installing()) {
    time::every(std::time::Duration::from_secs(2))
        .map(|_| Message::SystemUpdate(super::system_update::SystemUpdateMessage::PollJournal))
} else {
    Subscription::none()
};
```

With:
```rust
// Journal polling subscription for OTA update progress
// Only poll when settings modal is open AND an update is installing
let journal_poll_sub = if self.settings.is_open && self.settings.update.as_ref().is_some_and(|u| u.is_installing()) {
    time::every(std::time::Duration::from_secs(2))
        .map(|_| Message::SystemUpdate(super::system_update::SystemUpdateMessage::PollJournal))
} else {
    Subscription::none()
};
```

**Step 2: Build and verify**

Run: `cargo build -p mesh-player`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add crates/mesh-player/src/ui/app.rs
git commit -m "fix(player): only poll OTA journal when settings modal is open"
```

---

### Task 3: Add canvas::Cache to PlayerCanvas for geometry caching

This is the most impactful optimization. Currently, `draw()` rebuilds ALL geometry (paths, rectangles, text) for all 4 decks every single frame. With `canvas::Cache`, iced only reconstructs geometry when state changes.

**Files:**
- Modify: `crates/mesh-widgets/src/waveform/canvas/player.rs:1-406`
- Modify: `crates/mesh-widgets/src/waveform/state.rs` (add cache_key method)
- Modify: `crates/mesh-widgets/src/waveform/view.rs:189-205`

**Step 1: Add a cache key to PlayerCanvasState**

In `crates/mesh-widgets/src/waveform/state.rs`, add a generation counter that bumps on any state mutation:

```rust
// Add to PlayerCanvasState struct:
/// Cache invalidation counter — incremented when any visual state changes
cache_generation: u64,
```

Add a method to bump and read it:
```rust
/// Increment the cache generation (call after any state mutation)
pub fn invalidate_cache(&mut self) {
    self.cache_generation = self.cache_generation.wrapping_add(1);
}

/// Get the current cache generation for dirty checking
pub fn cache_generation(&self) -> u64 {
    self.cache_generation
}
```

Then add `self.invalidate_cache()` calls at the end of every `set_*` method that changes visual state: `set_playhead`, `set_master`, `set_stem_active`, `set_volume`, `set_loop_active`, `set_loop_length_beats`, `set_key_match_enabled`, `set_transpose`, `set_lufs_gain_db`, and the methods on `CombinedState` sub-structs (`set_position`, `set_loop_region`, `set_lufs_gain`, `set_slicer_region`).

**Step 2: Add canvas::Cache to PlayerCanvas struct**

The `PlayerCanvas` struct in `player.rs` is currently a borrowing struct with lifetimes. iced's `canvas::Cache` needs to live outside the `Program` impl since `draw()` takes `&self`. The cache should be stored on `PlayerCanvasState` (which lives on MeshApp).

In `crates/mesh-widgets/src/waveform/state.rs`, add:
```rust
use iced::widget::canvas::Cache;
```

Add to `PlayerCanvasState`:
```rust
/// Canvas geometry cache — cleared when state changes
pub canvas_cache: Cache,
/// Last generation the cache was built for
canvas_cache_generation: u64,
```

Initialize in `Default` impl:
```rust
canvas_cache: Cache::new(),
canvas_cache_generation: 0,
```

Update `invalidate_cache()` to also clear the cache:
```rust
pub fn invalidate_cache(&mut self) {
    self.cache_generation = self.cache_generation.wrapping_add(1);
    self.canvas_cache.clear();
}
```

**Step 3: Use the cache in PlayerCanvas::draw()**

In `crates/mesh-widgets/src/waveform/canvas/player.rs`, change `draw()`:

Replace the current `draw` implementation:
```rust
fn draw(
    &self,
    _interaction: &Self::State,
    renderer: &iced::Renderer,
    _theme: &Theme,
    bounds: Rectangle,
    _cursor: mouse::Cursor,
) -> Vec<Geometry> {
    if self.state.is_vertical_layout() {
        return self.draw_vertical(renderer, bounds);
    }

    let mut frame = Frame::new(renderer, bounds.size());
    // ... all the drawing code ...
    vec![frame.into_geometry()]
}
```

With:
```rust
fn draw(
    &self,
    _interaction: &Self::State,
    renderer: &iced::Renderer,
    _theme: &Theme,
    bounds: Rectangle,
    _cursor: mouse::Cursor,
) -> Vec<Geometry> {
    let geometry = self.state.canvas_cache.draw(renderer, bounds.size(), |frame| {
        if self.state.is_vertical_layout() {
            self.draw_vertical_into(frame, bounds);
            return;
        }

        let width = bounds.width;
        let cell_width = (width - DECK_GRID_GAP) / 2.0;
        let (cell_height, zoomed_height) = cell_height_from_bounds(bounds.height);

        let grid_positions = [
            (0.0, 0.0),
            (cell_width + DECK_GRID_GAP, 0.0),
            (0.0, cell_height + DECK_GRID_GAP),
            (cell_width + DECK_GRID_GAP, cell_height + DECK_GRID_GAP),
        ];

        let overview_scales = compute_overview_scales(self.state);

        for (deck_idx, (x, y)) in grid_positions.iter().enumerate() {
            let playhead = self.state.interpolated_playhead(deck_idx, SAMPLE_RATE);
            let is_master = self.state.is_master(deck_idx);
            let track_name = self.state.track_name(deck_idx);
            let track_key = self.state.track_key(deck_idx);
            let stem_active = self.state.stem_active(deck_idx);
            let transpose = self.state.transpose(deck_idx);
            let key_match_enabled = self.state.key_match_enabled(deck_idx);
            let (linked_stems, linked_active) = self.state.linked_stems(deck_idx);
            let lufs_gain_db = self.state.lufs_gain_db(deck_idx);
            let track_bpm = self.state.track_bpm(deck_idx);
            let cue_enabled = self.state.cue_enabled(deck_idx);
            let loop_length_beats = self.state.loop_length_beats(deck_idx);
            let loop_active = self.state.loop_active(deck_idx);
            let volume = self.state.volume(deck_idx);
            let mirrored = deck_idx >= 2;

            draw_deck_quadrant(
                frame,
                &self.state.decks[deck_idx],
                playhead,
                *x, *y,
                cell_width, zoomed_height,
                deck_idx, track_name, track_key,
                is_master, cue_enabled, stem_active,
                transpose, key_match_enabled,
                self.state.stem_colors(),
                linked_stems, linked_active,
                lufs_gain_db, track_bpm,
                overview_scales[deck_idx],
                loop_length_beats, loop_active,
                volume, mirrored,
            );
        }
    });

    vec![geometry]
}
```

Note: The `draw_vertical` method also needs a similar refactor — extract drawing logic into a `draw_vertical_into(&self, frame: &mut Frame, bounds: Rectangle)` method that takes a frame reference rather than creating and returning one.

**Step 4: Refactor draw_vertical similarly**

The existing `draw_vertical` method creates its own `Frame` and returns `Vec<Geometry>`. Refactor it to a `draw_vertical_into` that takes `&mut Frame` — the cache closure provides the frame.

**Step 5: Build and verify**

Run: `cargo build -p mesh-widgets -p mesh-player`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add crates/mesh-widgets/src/waveform/canvas/player.rs crates/mesh-widgets/src/waveform/state.rs
git commit -m "perf(canvas): add geometry caching to PlayerCanvas via canvas::Cache"
```

---

### Task 4: Enable antialiasing explicitly

**Files:**
- Modify: `crates/mesh-player/src/main.rs:117-164`

**Step 1: Add .antialiasing(true) to the application builder**

In `crates/mesh-player/src/main.rs`, add `.antialiasing(true)` before `.run()`:

Replace:
```rust
.title("Mesh DJ Player")
.window_size(Size::new(1200.0, 800.0))
.run();
```

With:
```rust
.title("Mesh DJ Player")
.antialiasing(true)
.window_size(Size::new(1200.0, 800.0))
.run();
```

**Step 2: Build and verify**

Run: `cargo build -p mesh-player`
Expected: Compiles without errors.

**Step 3: Commit**

```bash
git add crates/mesh-player/src/main.rs
git commit -m "feat(player): enable antialiasing explicitly for smoother rendering"
```

---

### Task 5: Auto-detect monitor size and set window dimensions

**Files:**
- Modify: `crates/mesh-player/src/main.rs:117-164`
- Modify: `crates/mesh-player/src/ui/app.rs` (add message handler)
- Modify: `crates/mesh-player/src/ui/message.rs` (add message variant)

**Step 1: Add GotMonitorSize message variant**

In `crates/mesh-player/src/ui/message.rs`, add:
```rust
/// Monitor size detected at startup (for auto-sizing)
GotMonitorSize(Option<iced::Size>),
```

**Step 2: Query monitor size in boot function**

In `crates/mesh-player/src/main.rs`, change the boot function to query monitor size as a startup task:

Replace:
```rust
// If --midi-learn flag was passed, start MIDI learn mode (opens the drawer)
let startup_task = if start_midi_learn {
    Task::done(Message::MidiLearn(MidiLearnMessage::Start))
} else {
    Task::none()
};

(app, startup_task)
```

With:
```rust
// Query monitor size for auto-resolution
let monitor_task = iced::window::monitor_size(iced::window::Id::MAIN)
    .map(Message::GotMonitorSize);

// If --midi-learn flag was passed, start MIDI learn mode (opens the drawer)
let startup_task = if start_midi_learn {
    Task::batch([
        monitor_task,
        Task::done(Message::MidiLearn(MidiLearnMessage::Start)),
    ])
} else {
    monitor_task
};

(app, startup_task)
```

**Step 3: Handle GotMonitorSize in update()**

In `crates/mesh-player/src/ui/app.rs`, add a handler in the main `update` match:

```rust
Message::GotMonitorSize(Some(size)) => {
    log::info!("Monitor size detected: {}x{}", size.width, size.height);
    // Resize window to fill the monitor
    return iced::window::resize(iced::window::Id::MAIN, size);
}
Message::GotMonitorSize(None) => {
    log::warn!("Could not detect monitor size, using default window size");
    Task::none()
}
```

**Step 4: Remove hardcoded window_size from main.rs**

The initial `window_size` serves as fallback before monitor detection kicks in. Keep it as a reasonable default but the auto-detection will resize to fill the display.

**Step 5: Build and verify**

Run: `cargo build -p mesh-player`
Expected: Compiles without errors.

**Step 6: Commit**

```bash
git add crates/mesh-player/src/main.rs crates/mesh-player/src/ui/app.rs crates/mesh-player/src/ui/message.rs
git commit -m "feat(player): auto-detect monitor size and resize window at startup"
```

---

### Task 6: Switch embedded backend to Vulkan with Mailbox present mode

**Files:**
- Modify: `nix/embedded/kiosk.nix:92-97`

**Step 1: Update cage environment variables**

Replace:
```nix
environment = {
    # Use GLES via Panthor (Mali-G610)
    WGPU_BACKEND = "gl";
    MESA_GL_VERSION_OVERRIDE = "3.1";
    WLR_NO_HARDWARE_CURSORS = "1";
};
```

With:
```nix
environment = {
    # Use Vulkan via PanVK (Mali-G610, conformant Vulkan 1.2+)
    # Enables Mailbox present mode for low-latency tearless rendering
    WGPU_BACKEND = "vulkan";
    ICED_PRESENT_MODE = "mailbox";
    WLR_NO_HARDWARE_CURSORS = "1";
};
```

**Step 2: Commit**

```bash
git add nix/embedded/kiosk.nix
git commit -m "perf(embedded): switch to Vulkan + Mailbox for low-latency 120Hz rendering"
```

---

### Task 7: Run diagnostics and document findings

**Files:**
- Create: `documents/120hz-rendering-report.md`

**Step 1: Run mesh-player and collect diagnostics**

Run these commands while mesh-player is running to verify optimal settings:

```bash
# 1. Check wgpu backend in use
RUST_LOG=iced_wgpu=debug cargo run -p mesh-player 2>&1 | head -50

# 2. Check display info (if running on Wayland)
wlr-randr 2>/dev/null || echo "Not running on Wayland or wlr-randr not available"

# 3. Check GPU info
cat /proc/driver/gpu/* 2>/dev/null || echo "No GPU driver info"
lspci 2>/dev/null | grep -i vga || echo "No PCI GPU"
glxinfo 2>/dev/null | grep -E "renderer|version" | head -5 || echo "glxinfo not available"

# 4. Check Vulkan support
vulkaninfo --summary 2>/dev/null || echo "vulkaninfo not available"

# 5. Check display server
echo "XDG_SESSION_TYPE: $XDG_SESSION_TYPE"
echo "WAYLAND_DISPLAY: $WAYLAND_DISPLAY"
echo "WGPU_BACKEND: $WGPU_BACKEND"
echo "ICED_PRESENT_MODE: $ICED_PRESENT_MODE"

# 6. Monitor resolution and refresh
xrandr 2>/dev/null || echo "xrandr not available (Wayland-only)"
xdpyinfo 2>/dev/null | grep dimensions || echo "xdpyinfo not available"

# 7. CPU info for frame budget estimation
nproc
cat /proc/cpuinfo | grep "model name" | head -1

# 8. Memory available
free -h

# 9. Check vsync and compositor
cat /sys/class/drm/card*/status 2>/dev/null
cat /sys/class/drm/card*/modes 2>/dev/null | head -10
```

**Step 2: Document all findings**

Create `documents/120hz-rendering-report.md` with all diagnostic output, analysis, and configuration recommendations. Structure:

1. **System Environment** — display server, GPU, driver versions
2. **Current Rendering Configuration** — present mode, backend, antialiasing
3. **Display Capabilities** — resolution, refresh rate, pixel clock
4. **Performance Baseline** — frame times, CPU/GPU utilization
5. **Configuration Applied** — what was changed and why
6. **Remaining Work** — items deferred until hardware arrives

**Step 3: Commit**

```bash
git add documents/120hz-rendering-report.md
git commit -m "docs: add 120Hz rendering optimization report"
```

---

## Task Dependency Graph

```
Task 1 (window::frames)     ──┐
Task 2 (OTA polling fix)    ──┤
Task 4 (antialiasing)       ──┼── Task 7 (diagnostics + docs)
Task 5 (auto-resolution)    ──┤
Task 6 (Vulkan + Mailbox)   ──┤
Task 3 (canvas::Cache)      ──┘
```

Tasks 1-6 are independent and can be done in any order. Task 7 (diagnostics) should run last to capture the final state.

## Key Technical Notes for Implementer

### canvas::Cache invalidation
The critical thing is that `cache.clear()` must be called whenever ANY visual state changes. Missing an invalidation means stale frames. The generation counter pattern catches this — every `set_*` method bumps the counter and clears the cache.

The cache is extremely effective here because at 120Hz, most frames have ONLY the playhead position changed. With the cache, iced skips geometry reconstruction entirely for identical frames and only rebuilds when `invalidate_cache()` was called since the last draw.

**Caveat**: `interpolated_playhead()` is called inside the cache closure, which means the cache gets hit and rebuilt every frame since the playhead moves continuously. This is intentional — the waveform scrolls with the playhead, so geometry changes every frame during playback. The cache saves time during PAUSE (no rebuilds at all) and reduces allocation overhead even during playback (no Vec allocations for point buffers).

For a future optimization beyond this plan: split into two cache layers (static elements: headers, cue markers, overview | dynamic elements: zoomed waveform, playhead). The static layer would only rebuild on track load/config change.

### ICED_PRESENT_MODE env var
iced reads this via `iced_wgpu::settings::present_mode_from_env()`. Accepted values: `vsync`, `no_vsync`, `immediate`, `fifo`, `fifo_relaxed`, `mailbox`. Mailbox gives low-latency tearless presentation on Wayland/Vulkan — single-frame queue vs Fifo's ~3-frame queue.

### window::frames() vs time::every()
`window::frames()` is driven by the compositor's frame callback, not a timer. On 120Hz it fires every ~8.33ms, on 60Hz every ~16.67ms. It automatically adapts to the display's actual refresh rate. It also stops firing when the window is occluded (not visible), saving CPU — but in a kiosk this doesn't apply.

### Vulkan backend on Orange Pi 5
PanVK achieved Vulkan 1.2 conformance on Mali-G610 (Collabora, 2025). If Vulkan causes issues with wgpu 27, fall back to `WGPU_BACKEND=gl` with `MESA_GL_VERSION_OVERRIDE=3.1`. Mailbox present mode is only available with Vulkan on Wayland — GLES gets Fifo only.
