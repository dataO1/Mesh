# Changelog

All notable changes to Mesh are documented in this file.

---

## [Unreleased]

### Fixed

- **Track name parsing: number prefix leaking into artist** — Filenames with UVR5
  playlist+track-number prefixes (e.g., `1_01 Black Sun Empire - Feed the Machine`)
  left the track number in the artist field. Added a compound strip that only removes
  bare space-separated track numbers when a UVR5 prefix was present, avoiding false
  positives on legitimate names like "808 State".

- **Browser jumps to top after deleting a track** — `handle_confirm_delete()` called
  `clear_selection()` after refreshing the track list, leaving no selection and resetting
  scroll to the top. Now captures the selected index before deletion and re-selects the
  neighbor at that position (clamped to list bounds) after refresh.

- **USB stick removal leaves stale browser state** — `remove_usb_device()` checked
  `active_usb_idx` after `retain()` had already removed the device, so the check always
  failed and the browser never cleared. Fixed ordering to check before removal. Also
  wires up the existing `clear_usb_database()` function which was implemented but never
  called on disconnect.

- **MIDI/HID devices not detected when connected after launch** — Device enumeration
  only ran at startup. Added `check_new_devices()` to the existing 2-second poll loop,
  scanning for expected-but-unconnected devices from `midi.yaml`. Reuses the existing
  `try_connect_all_midi`/`try_connect_all_hid` which skip already-connected devices.

- **Slicer preset trigger uses wrong preset index** — `SlicerTrigger` handler checked
  which stems had patterns using the globally-selected editor preset instead of the
  preset corresponding to the pressed pad. Now uses `button_idx` directly, matching
  the index sent to the audio engine.

- **Slicer waveform not fixed in shader** — `build_uniforms()` always centered the
  zoomed window on the playhead, ignoring `FixedBuffer` view mode. In slicer mode
  the window now locks to the slicer buffer bounds so the waveform stays fixed and
  the playhead moves left-to-right across it.

- **Slicer LED feedback param mismatch** — `deck.slicer_slice_active` feedback looked
  for a `"slice"` parameter but the MIDI learn system generates `"pad"`. Fixed to
  match.

- **USB export stuck on preset sync** — `copy_dir_all()` was missing `sync_all()`
  after each file copy, leaving preset YAML data in kernel page cache instead of
  flushing to USB flash media. Subsequent phases would block 2-3 minutes waiting
  for implicit writeback. Now explicitly fsyncs each file, matching the existing
  `copy_large_file()` pattern used for WAV track copies.

- **Duplicate track import** — Batch import now detects tracks that already exist
  in the collection (by checking the output FLAC path) and skips them, avoiding
  redundant stem loading, BPM/key analysis, ML inference, and FLAC re-export.
  Applies to both pre-separated stem import and mixed-audio separation paths.

### Changed

- **Deck header text sizing** — Increased all header text sizes to better fill
  the 48px header height: track name 20→24, BPM/loop/LUFS 16→20, key 18→22,
  badge number 22→26. Badge now fills full header height (was 38px with 10px
  gap). Added more horizontal spacing (12→18px) between right-side info items.

- **Track display name format** — Deck headers and waveform overlays now show
  `{Artist} - {Name}` from parsed metadata instead of raw filenames. Falls back
  to filename without extension when metadata is unavailable. Added `name` field
  to `TrackMetadata` and `display_name()` method to `LoadedTrack`.

- **Waveform zoom-out subsampling** — Changed resolution scaling curve from
  linear to quadratic and lowered minimum resolution from 256 to 128 pixels.
  Reduces visual jitter at maximum zoom-out (64 bars) while preserving detail
  at moderate zoom levels.

---

## [0.9.7]

### Added

- **Embedded RT audio optimizations (Phase 1+2)** — Comprehensive real-time
  audio tuning for the OrangePi 5 (RK3588S) embedded image. Phase 1: kernel
  boot params (`rcu_nocbs`, `nohz_full`, `irqaffinity`, `transparent_hugepage=never`,
  `nosoftlockup`, `nowatchdog`), locked PipeWire quantum to 256, PipeWire RT
  module (priority 88), IRQ affinity service (audio IRQs → A55 core 0, all
  others → A76), disabled irqbalance, deep idle state disable on A55 cores,
  system service CPU pinning (NetworkManager/journald → A76), BFQ I/O scheduler
  for USB storage. Phase 2: `embedded-rt` feature flag with `mlockall()` to
  prevent page faults, `/dev/cpu_dma_latency=0` to disable C-states, SCHED_FIFO
  priority 70 for rayon audio workers, CPU affinity pinning (rayon → A55 cores
  2-3), 512KB stack pre-faulting, RT capability verification at startup. All
  application code is feature-gated (`#[cfg(feature = "embedded-rt")]`), auto-
  enabled on aarch64 builds only.

- **Resource monitoring in header** — CPU%, GPU%, RAM usage, and FPS counter
  displayed in the player header bar. GPU utilization reads Mali devfreq
  (aarch64) or AMD DRM sysfs (x86). FPS counted from iced frame events.
  Polls at 500ms intervals via `ResourceMonitor` in mesh-core (reusable by mesh-cue).

- **Mali linked stem split view** — Overview waveforms on the Mali shader path
  now show linked stems as a split view (active stem top half, inactive bottom
  half). Precomputed on CPU into the existing 4-stem buffer with signed
  min/max encoding — no shader changes or GPU upload increase needed.

- **Canvas stem mute & link indicators** — Zero-GPU stem status indicators
  rendered as iced container widgets beside the zoomed waveform. Mute column
  (always visible) + link column (when any stem linked). Replaces removed
  Mali shader indicators with no GPU cost.

- **Waveform abstraction setting** — New "Waveform Abstraction" option (Low,
  Medium, High) controlling the grid-aligned subsampling strength per stem. Low
  gives near-raw peak rendering, Medium (default) provides tuned per-stem
  abstraction (vocals smooth, drums detailed), High pushes further toward a
  stylized look. Takes effect immediately.

- **Render debug logging** — Added `[RENDER]` debug log entries throughout the
  waveform pipeline: peak computation at load time (computed peaks-per-pixel at
  reference zoom), and per-frame shader uniforms (zoom level, peak density,
  abstraction, blur). Visible with `RUST_LOG=debug`.

### Changed

- **Cage CPU affinity widened** — `CPUAffinity` changed from `4-7` (A76 only)
  to `0-7` (all cores) so mesh-player can set per-thread affinity internally:
  audio RT + UI → A55 (deterministic in-order), background loading → A76 (high
  throughput).

- **Collection track format: WAV to FLAC** — Stem files now use 8-channel FLAC
  lossless compression instead of raw WAV. ~58% file size reduction (e.g., 240 MB
  WAV → 104 MB FLAC) with zero audio quality loss. Encoding via `flacenc` crate,
  decoding via symphonia. Existing collections must reimport tracks (delete
  `tracks/` folder and reimport).

- **In-memory audio file reader** — `AudioFileReader` now reads the entire file
  into memory (`Arc<[u8]>`) on open, then creates independent symphonia decoders
  per region from the shared buffer. All I/O happens once at open; subsequent
  region reads are pure CPU decode with no file system access.

- **Simplified peak interpolation** — Removed the hybrid max-hold/bilinear
  interpolation in `sample_peak()`. The old approach blended between interpolated
  and preserved (max-hold) values based on peak magnitude, adding complexity
  without clear visual benefit now that peak resolution is much higher. The new
  code uses straightforward bilinear interpolation between grid points.

### Performance

- **Instant grid render on track load** — Beat grid, cue markers, loop regions,
  playhead, and stem indicators now render immediately when a track is loaded,
  instead of waiting for peak data to arrive from the background loader. The
  shader's early-exit guard was split so only stem envelope rendering (which
  genuinely needs peak data) is gated behind peak availability. A pulsing
  brightness overlay signals that audio is still loading and interaction is ready.

- **Parallel priority region decode** — Priority regions (around cue points) are
  now decoded in parallel via `std::thread::scope`. Each thread creates its own
  decoder from the shared `Arc<[u8]>` buffer. Results are merged sequentially
  after all threads complete. First playable audio arrives ~3x faster.

- **Parallel gap decode** — Non-priority gap regions are also decoded in parallel,
  removing the old sequential sub-batching loop. Combined with priority parallelism,
  total decode time drops from ~4s to ~1.5s on 4+ cores.

- **Full LTO for release builds** — Changed from thin LTO to full LTO
  (`lto = true`) for maximum cross-crate optimization in release binaries.

- **Native CPU targeting** — Added `target-cpu=native` to RUSTFLAGS for all build
  targets (NixOS, deb container, Windows cross-compile). Enables host-specific
  SIMD extensions (AVX2, SSE4.2, NEON) for decode and analysis hot paths. Aarch64
  cross-compilation uses `cortex-a76` (RK3588) instead of native.

- **Parallel track loading** — Track loader now dispatches each load request to
  rayon's thread pool instead of processing sequentially on a single thread.
  All 4 decks load simultaneously when loading tracks in parallel.

- **Linked stem stretch threads** — `MAX_STRETCH_THREADS` increased from 2 to 8.
  Pre-stretching runs at nice(10) priority so JACK audio thread preempts safely.

- **Dynamic waveform peak resolution** — Highres peak count is now proportional to
  actual audio length and BPM instead of a fixed 65K constant. A BPM-aware formula
  targets 1 peak per pixel at 4-bar zoom (the closest practical zoom level).
  Short tracks allocate proportionally less memory.

- **GPU buffer vec2 packing** — Waveform peak storage changed from `array<f32>` to
  `array<vec2<f32>>` in the WGSL shader, halving the number of buffer reads per
  peak lookup. The CPU-side interleaved `[min, max, ...]` layout is bit-identical
  to `vec2<f32>`, so no data conversion is needed.

### Fixed

- **FLAC block-size padding** — Work around flacenc-rs#242 where
  `encode_with_fixed_block_size()` produces malformed final frames when sample
  count is not a multiple of the block size (default 4096). Samples are now padded
  to the next block-size boundary with silence before encoding.

- **Nix build missing .cargo/config.toml** — The Nix source filter excluded
  `.cargo/config.toml`, meaning NixOS builds never received `--export-dynamic`
  (needed for PD externals) or `target-cpu=native`. Added `config.toml` to the
  filter for both `mesh-build.nix` and `mesh-player.nix`.

- **Prelinked stems missing from waveform** — Linked stems loaded asynchronously
  (prelinked in track metadata) were not shown in overview or zoomed waveforms.
  `TrackLoadResult::Complete` was replacing the entire `OverviewState`, discarding
  linked stem peaks that arrived earlier from the async loader. Fix preserves
  linked stem data across the state replacement and rebuilds GPU buffers.

- **Settings MIDI navigation indices** — Fixed `next_idx` for dynamic settings
  sections (Network, System Update, MIDI Learn) to match the actual entry count,
  preventing index collisions during MIDI encoder navigation.

- **Track drift from FLAC seek overshoot** — Symphonia's FLAC decoder seeks to
  the nearest block boundary (every 4096 samples), not the exact requested frame.
  Parallel region decoding did not account for this, leaving up to 4095 extra
  leading samples per region. Over multiple seeks this caused audible sync drift.
  The decoder loop now skips leading frames based on the `SeekedTo` return value.

- **Beat grid integer truncation** — `regenerate_with_rate()` cast
  `samples_per_beat` to `u64`, truncating the fractional part. At 174 BPM this
  accumulated ~7.5 ms drift over 500 beats. Replaced with f64 accumulation and
  per-beat rounding (max error ±0.5 samples, never accumulates).

- **FLAC padding inflating duration** — The FLAC encoder pads to block-size
  boundaries, inflating `total_samples` in the stream header by up to 4095
  samples. `frame_count` and `duration_samples` are now capped at the
  metadata-derived duration from the database.

- **USB linked stem metadata lookup** — Linking a stem from a USB track (e.g.
  via smart suggestions from another USB stick) silently fell back to 120 BPM
  defaults because `LoadedTrack::load_to()` passed absolute paths to USB
  databases that store relative paths. Introduced `resolve_track_metadata()` as
  the single source of truth for path-aware DB resolution: tries local DB first,
  then detects USB collection roots, strips the prefix, and queries the correct
  USB database. The linked stem loader and domain layer now delegate to this
  function instead of duplicating path resolution logic.

- **Linked stem BPM source** — `confirm_stem_link_selection()` used the
  global master BPM instead of the host deck's native track BPM for
  time-stretching linked stems. This caused incorrect stretch ratios when the
  master tempo differed from the host track's original BPM.

- **Redundant Complete re-decode** — The streaming loader's `Complete` path
  redundantly re-computed all waveform peaks (~200 ms) and replaced the
  incrementally-built overview state, requiring a fragile linked-stem
  preservation hack. `Complete` now carries an `incremental` flag; when true
  (streaming path), the handler skips state replacement and redundant stem
  upgrades.

### Removed

- **Unified waveform rendering pipeline** — Removed the dual-path GPU shader
  architecture. The CPU-precomputed "Mali" path (1:1 peak per pixel, zero GPU
  reduction loops) is now the only rendering pipeline for all platforms. This
  eliminates:
  - **Desktop shader** (`waveform.wgsl`, 834 lines) with its GPU-side
    `minmax_reduce` loops (up to 64 iterations per pixel per stem)
  - **`mali-shader` feature flag** from all three `Cargo.toml` files and the
    Nix build (`mesh-player.nix`)
  - **~18 `#[cfg]` gates** in the shader Rust code (`mod.rs`, `pipeline.rs`)
  - **6 settings** that only affected the desktop shader: Waveform Quality,
    Motion Blur, Depth Fade, Depth Fade Inverted, Peak Width, Edge AA — along
    with their config enums, draft state fields, handler arms, and UI sections
  - **5 `PlayerCanvasState` fields**: `motion_blur_level`, `depth_fade_level`,
    `depth_fade_inverted`, `peak_width_mult`, `edge_aa_level`
  - **Engine command** `SetWaveformQuality` and domain method
    `set_waveform_quality()`
  - **Unused peak functions**: `smooth_peaks()`, `smooth_peaks_gaussian_wide()`,
    `GAUSSIAN_WEIGHTS_17`
  - Quality level hardcoded to 0 (Low) throughout the loader pipeline
  - Settings entry count reduced from 20 to 14, MIDI nav indices renumbered
  - Total: **~1,550 lines deleted** across 23 files, zero new code

- **WAV chunk parsers** — Removed `parse_mlop_chunk()`, `parse_mslk_chunk()`,
  `serialize_mslk_chunk()`, and `align_peaks_to_host()` — legacy WAV custom chunk
  handling no longer needed with FLAC format.

- **Waveform preview from file** — Removed `WaveformPreview`, `from_preview()`,
  and `read_waveform_preview_from_file()`. Waveform peaks are now computed from
  decoded audio during the streaming load, not read from embedded file chunks.

---

## [0.9.6]

### Performance

- **Mali GPU hyper-optimized waveform shader** — New `waveform_mali.wgsl` shader
  variant for Mali Valhall GPUs (Orange Pi 5 / RK3588). Reduces per-pixel ALU cost
  from ~1,320 to ~200 ops by removing depth fade, peak width expansion, stem
  indicators, playhead glow, motion blur branching, and `fwidth()` derivative calls.
  Replaces `smoothstep` with linear clamp AA and `dpdx`/`dpdy` derivatives with
  analytical slope estimation from adjacent peaks.

- **CPU-precomputed waveform peaks** — On Mali builds, peak subsampling (grid-aligned
  min/max reduction) is computed on the CPU instead of the GPU. Each pixel column gets
  exactly one precomputed (min, max) pair per stem, guaranteeing the 1:1 peak-per-pixel
  invariant at ALL zoom levels (not just 4-bar). This eliminates the `minmax_reduce`
  loop from the shader entirely — the GPU does a single buffer read per stem per pixel.
  CPU cost is ~0.6ms/frame on an A76 core; upload cost is ~40KB per view.

- **Draw call skip for empty decks** — Unloaded decks now skip the GPU draw call
  entirely (checked via `has_track` uniform), avoiding TBDR tile binning overhead on
  Mali's tiled renderer.

### Added

- **`mali-shader` Cargo feature flag** — Enables the Mali-optimized shader and CPU
  peak precomputation. Automatically activated on aarch64 nix builds; can be enabled
  on x86 for testing with `--features mali-shader`. Propagated through mesh-player
  and mesh-cue Cargo.toml. *(Removed in 0.9.7 — Mali path became the universal
  default.)*

---

## [0.9.5]

### Improved

- **Stem link LED feedback** — Stem mute LEDs now toggle between two color shades
  when a linked stem is present: primary shade for the original, alternate shade for the
  linked version. Stems with a linked counterpart pulse subtly to signal interactivity.

- **Darker stem LED colors** — Redesigned stem LED colors for better contrast and F1
  HID compatibility (7-bit, 0-125 range): dark green (vocals), deep navy (drums),
  rusty orange (bass), violet (other).

- **Auto-open browser on stem link** — Pressing shift+stem on an unlinked stem now
  automatically opens the browser overlay and activates browse mode so the encoder
  navigates the track list. The selected track is highlighted in the stem's color.

### Fixed

- **Linked stem visual LUFS scaling** — Linked stem waveforms were double-corrected
  for LUFS (once baked into peak buffer, once in shader). Now normalizes linked→host
  level only; the shader handles host→-9 LUFS uniformly for both original and linked.

- **JACK xruns during linked stem loading** — Time stretching for linked stems used
  up to 4 threads, saturating all CPU cores and starving the JACK audio callback.
  Reduced to 2 threads with lowered scheduling priority (`nice 10`) to leave headroom
  for real-time audio processing.

---

## [0.9.4]

### Performance

- **GPU-accelerated waveforms** — Zoomed waveform rendering moved from CPU to GPU.
  Waveform data is uploaded once when a track loads; only the playhead position and
  display state are sent each frame. Dramatically reduces CPU usage during playback,
  especially at high refresh rates with multiple decks.

- **Smarter redraw scheduling** — Waveform display only redraws when something
  actually changes, instead of rebuilding every frame unconditionally.

- **Removed background peaks thread** — Eliminated a legacy background thread that
  recomputed zoomed waveform peaks every tick. The GPU shader reads peak data uploaded
  once at track load, making this thread pure overhead.

### Improved

- **Smoother waveform appearance** — Waveforms now have a cleaner, more abstract look
  with per-stem detail tuning. Bass is the smoothest, drums retain more detail, and
  vocals/other sit in between. Thin peaks render with proper anti-aliasing instead of
  flickering between pixel rows.

- **Playhead brightness gradient** — Waveform peaks near the playhead are subtly
  brighter, with an inverse-exponential falloff so the effect is concentrated around
  the current position. Peak edges glow more than centers for a natural depth effect.

- **Overview window indicator** — The overview waveform now highlights the region
  currently visible in the zoomed view with a subtle overlay.

- **Red downbeat markers** — Bar lines in the beat grid are now red to distinguish
  them from regular beat lines, matching the overview waveform style.

- **LUFS-normalized waveform amplitude** — All tracks are visually scaled to match
  -9 LUFS, so quiet and loud tracks appear at the same visual amplitude in the
  waveform display.

- **Slicer shows 16 divisions** — Slicer overlay now correctly displays 16 slice
  divisions instead of 8. The currently playing slice is highlighted with an orange
  tint, and the next slice boundary has a yellow accent.

- **Beat grid respects density setting** — The overview waveform beat grid now follows
  the grid density setting (8, 16, 32, or 64 beats between red markers). Each period is
  subdivided into 4 equal parts (1 red + 3 gray) for consistent visual rhythm. Overview
  grid lines are subtler to avoid clutter. Zoomed view shows individual beat lines.

- **BPM-aligned overview waveforms** — Overview waveforms are now scaled so that beat
  markers align across all loaded decks. The longest track (in beats) fills the full
  width, and shorter tracks are padded proportionally.

- **GPU waveforms in mesh-cue** — The track editor now uses the same GPU shader
  waveform renderer as the player, replacing the old CPU canvas rendering.

- **Stem mute indicators restored** — Zoomed waveform shows colored rectangles on the
  outer edge indicating each stem's mute state (bright = active, dark = muted). Indicators
  appear on the left edge for decks 1 and 3, right edge for decks 2 and 4.

- **Linked stem indicators in waveform** — When a stem has a linked stem loaded, a second
  indicator column appears next to the mute indicators. Shows full color when the linked
  stem is active, dimmed when inactive. Replaces the diamond symbols in the header.

- **Linked stem waveform toggling** — Zoomed waveform now visually switches to the linked
  stem's peaks when a linked stem is activated, matching the audio output. Overview waveform
  shows a mirrored split: active stem peaks go upward from the center line, inactive
  alternative peaks go downward (dimmed), so you can see both versions at a glance. Peak
  buffers are cached and rebuilt only when linked stem data arrives, with toggle display
  handled entirely by GPU uniforms for instant visual response.

- **Overview split rendering** — Non-linked stems in split mode now render only on the
  top half of the overview waveform. The bottom half is reserved exclusively for linked
  stem alternatives, giving a cleaner visual separation.

- **MIDI shift+stem mute toggles linked stems** — Pressing shift + a stem mute button on
  a MIDI controller now toggles the linked stem, matching the UI behavior. Uses a dedicated
  `deck.stem_link` action resolved by the mapping engine, eliminating reliance on UI-side
  shift state synchronization.

### Fixed

- **Buttery-smooth playhead scrolling** — Playhead interpolation now uses timestamps
  from the audio thread with playback rate compensation, eliminating the rhythmic
  micro-stuttering caused by audio buffer quantization.

- **Correct stem overlap rendering** — Fixed alpha blending from premultiplied to
  straight alpha, eliminating the white/washed-out outlines where stems overlap.

- **Waveform stays in sync with audio** — Fixed two sources of visual drift that caused
  the zoomed waveform to gradually fall out of sync with the audio over longer tracks.

- **Beat grid always visible** — Beat grid lines now appear for all tracks, including
  those without detailed beat analysis (falls back to BPM-based grid).

- **Stable waveform at all zoom levels** — Waveform no longer jumps or wobbles when
  changing zoom level or at deep zoom.

- **Waveform loads progressively** — Overview waveform fills in as the track loads
  instead of appearing all at once.

- **Playhead stays centered at track edges** — Zoomed waveform no longer snaps the
  playhead off-center when near the beginning or end of a track.

- **Overview waveform stays visible after loading** — Fixed a bug where the overview
  waveform would appear during progressive loading but disappear once loading completed.

- **Beat markers no longer too thick** — Reduced beat grid line thickness and opacity
  for a cleaner look that doesn't obscure the waveform.

- **Smooth waveform scrolling in mesh-cue** — Replaced fixed 16ms timer with
  display-synced frame scheduling, and fixed playhead interpolation to only reset when
  the audio position actually changes. Eliminates bursty waveform movement caused by
  audio buffer quantization.

- **Beat grid visible on all track lengths** — Fixed beat grid disappearing on longer
  tracks due to an overly aggressive rendering threshold in the GPU shader.

- **Smooth cue preview waveform** — Zoomed waveform now scrolls smoothly during cue and
  hot cue preview, matching the smoothness of normal playback.

- **Settings auto-save on close** — Closing the settings panel (via UI or MIDI controller)
  now automatically saves any changed settings to disk. Previously, changes made via MIDI
  encoder were applied in-memory but lost on restart because the async save task was
  discarded.

---

## [0.9.3]

### Performance

- **Rendering: display-synced frame scheduling** — Replaced hardcoded 60Hz timer
  (`time::every(16ms)`) with `window::frames()`, which fires at the compositor's
  native vblank rate. Automatically adapts to 60Hz, 120Hz, or any display refresh
  rate without code changes. Previously, 120Hz displays were capped at 60fps.

- **Rendering: canvas geometry caching** — Added `canvas::Cache` to
  `PlayerCanvasState`, eliminating per-frame reconstruction of all waveform
  geometry (~100+ draw ops, 32 Vec allocations, 16 Path closures per frame across
  4 decks). Cache invalidates on visual state changes (playhead, volume, stem
  mute, loop, etc.) and skips reconstruction entirely when paused. At 120Hz this
  prevents ~12,000 unnecessary draw operations per second during idle.

- **Rendering: Mailbox present mode** — Set `ICED_PRESENT_MODE=mailbox` as
  default across all environments (devshell, embedded kiosk, Debian/RPM packages).
  Mailbox uses a single-frame queue (~8ms latency at 120Hz) vs Fifo's 3-frame
  queue (~25ms). Wayland compositors guarantee tearless presentation regardless.

- **Rendering: Vulkan backend** — Set `WGPU_BACKEND=vulkan` as default everywhere,
  replacing GLES on embedded (which couldn't use Mailbox). Vulkan is required for
  Mailbox present mode and enables `PowerPreference::HighPerformance` GPU selection.
  On embedded, uses PanVK (Mali-G610, Vulkan 1.2+ conformant).

- **Rendering: MSAAx4 antialiasing** — Enabled `.antialiasing(true)` for smooth
  waveform line rendering. Also ensures `PowerPreference::HighPerformance` for GPU
  adapter selection via wgpu.

- **Rendering: OTA journal polling gated** — Journal polling for OTA updates now
  only runs when the settings modal is open AND an update is installing. Previously
  polled every frame unconditionally, adding unnecessary work to the render loop.

### Changed

- **Window: default size 1920x1080** — Default window size increased from 1200x800
  to 1920x1080 (Full HD). Auto-detection via `monitor_size()` is attempted at
  startup but returns `None` on Wayland tiling WMs (known winit limitation). On the
  target cage kiosk, the window auto-fills the display regardless.

- **Packaging: Vulkan wrapper scripts** — Debian and RPM packages now install
  binaries to `/usr/lib/mesh/` with a thin wrapper at `/usr/bin/` that sets
  `WGPU_BACKEND=vulkan` and `ICED_PRESENT_MODE=mailbox` before exec. Env vars
  use `${VAR:-default}` so users can override. Previously, binaries launched with
  no GPU backend preference, falling back to wgpu auto-detection.

- **Nix: fixed Vulkan ICD discovery** — Removed broken `VK_ICD_FILENAMES` from
  devshell that pointed to `pkgs.vulkan-loader` (which has no ICD files). The
  Vulkan loader automatically discovers ICDs from `/run/opengl-driver/` on NixOS
  via `hardware.graphics.enable`. The old path silently disabled ICD discovery.

### Fixed

- **USB: multi-stick metadata resolution** — When multiple USB sticks were
  connected, switching between playlists from different sticks could load tracks
  with wrong metadata (missing beatgrid, default 120 BPM, no key). Root cause:
  `load_track_metadata()` only checked the "active" USB database, ignoring other
  mounted sticks. Now resolves the correct database from the track's path itself
  via `find_collection_root()` + the centralized USB database cache, making
  metadata lookup independent of which stick is currently browsed. Also fixed the
  browser storage sync guard that prevented USB→USB switches between sticks.

- **USB: export progress and performance** — Pressing "Export" showed no UI feedback
  for metadata-only changes (no progress bar, export button stayed clickable). The UI
  now transitions immediately when export starts, and the progress bar correctly counts
  metadata-only updates. Also eliminated an expensive database re-open on USB flash
  after export completes (lazy cache invalidation instead).

- **USB: sync plan performance** — "Calculating changes" in the export modal took
  60+ seconds for a 200-track collection because supplementary metadata (cue points,
  saved loops, stem links, ML analysis, tags, audio features) was fetched with 6
  individual database queries per track — over 2,400 sequential round trips on USB
  flash storage. Replaced with 6 bulk parameterless queries that fetch all rows in
  a single pass and group by track ID in Rust, reducing scan time to ~1-2 seconds.

- **USB: device label resolution** — USB sticks were showing kernel device names
  (e.g. "/dev/sda") instead of human-readable names. On Linux, now resolves the
  filesystem label from `/dev/disk/by-label/`, falling back to the hardware model
  name from sysfs (e.g. "STORE N GO"), then `/dev/sdX` as last resort. macOS and
  Windows are unaffected (sysinfo already returns proper volume labels there).

- **Embedded: ES8388 audio init** — `mesh-audio-init` service was failing on every
  boot because the `Headphone` mixer control is a switch, not a volume. Replaced
  the single broken `amixer` command with a proper init script that enables the
  headphone amplifier path (`hp switch` on), sets PCM and output volumes, disables
  3D spatial processing, and ensures left/right mixer paths are enabled.
- **Embedded: ALSA device aliases** — `mesh_cue` and `mesh_master` PCM aliases
  used `type hw` (raw hardware access), which rejected mono audio and any format
  the ES8388 doesn't natively accept. Changed to `type plug` with nested
  `slave.pcm` for automatic format, channel, and sample rate conversion.
- **Embedded: PipeWire low-latency config** — Added PipeWire clock configuration
  with 256-sample quantum at 48kHz (5.33ms per period), min 64, max 1024. Without
  this, PipeWire defaulted to 1024 samples (21.3ms).
- **Embedded: WirePlumber device rules** — Split the combined ES8388/PCM5102A
  match into separate rules with per-device priorities. Reduced `api.alsa.headroom`
  from 256 to 0 (I2S codecs use DMA, not USB batch transfer, so headroom adds
  unnecessary latency). PCM5102A gets higher `priority.driver` so it becomes the
  graph clock source when connected.
- **Embedded: JACK audio routing via pw-link** — PipeWire JACK clients with
  `node.always-process=true` (set by the JACK layer) remain on Dummy-Driver
  unless explicit port links exist to a real ALSA sink. `target.object`,
  `PIPEWIRE_NODE`, and `priority.driver` all proved insufficient — they are
  routing hints that don't force driver assignment. The kiosk wrapper now starts
  mesh-player via `pw-jack` in the background, waits for its JACK ports to
  register, then creates `pw-link` connections from `master_left`/`master_right`
  to the ES8388's `playback_FL`/`playback_FR`. This reliably moves mesh-player
  off Dummy-Driver onto the ES8388 graph driver.
- **Embedded: WirePlumber config via environment.etc** — The NixOS
  `services.pipewire.wireplumber.extraConfig` option silently fails to create
  config files on NixOS 24.11 (`/etc/wireplumber/` was empty). Switched to
  `environment.etc` for direct file creation with WirePlumber 0.5 SPA-JSON
  format, ensuring ALSA tuning rules (`session.suspend-timeout-seconds`,
  `api.alsa.period-size`, `priority.driver`) are actually deployed.
- **CI: Windows cross-compilation bindgen** — `signalsmith-stretch` bindgen
  failed with `stdbool.h` not found after Phase 4's `unset BINDGEN_EXTRA_CLANG_ARGS`.
  bindgen auto-injects `--target=x86_64-pc-windows-gnu` which makes clang lose
  its resource directory. Now re-exports `BINDGEN_EXTRA_CLANG_ARGS` with just
  the clang include path (no MinGW sysroot) immediately after the unset.
- **Embedded: mesh-player logging** — Process substitution (`> >(systemd-cat ...)`)
  doesn't survive through `pw-jack`'s exec chain, so all mesh-player log output
  was silently lost. Replaced with a named FIFO pipe to `systemd-cat`, making
  logs available via `journalctl -t mesh-player`. Also sets `RUST_LOG=info` by
  default (overridable via `systemctl set-environment`).

### Added

- **MIDI: Master BPM slider control** — The master BPM slider is now controllable
  via MIDI. `GlobalAction::SetBpm` was stubbed out; it now routes through the
  full pipeline: `range_for_action("global.bpm")` maps CC 0-127 to 60-200 BPM,
  the mapping engine converts to `SetBpm`, and the app handler calls
  `set_global_bpm_with_engine()`. The MIDI learn wizard includes a "Move the
  BPM slider" step at the end of the Browser phase across all layout variants.
- **USB: Set filesystem label during export** — When exporting to a USB device,
  a new "Label" text input lets you set a custom filesystem label (e.g. "Mesh DJ").
  Tries `FS_IOC_SETFSLABEL` ioctl first (works on mounted ext4/btrfs/xfs, and FAT on
  kernel 7.0+). Falls back to udisks2 D-Bus `SetLabel` (works for regular users on
  removable devices via polkit, no root needed). Pre-fills with the device's current
  label; shows filesystem-specific max length hints. Label setting is non-fatal —
  failure is logged but doesn't abort the export.
- **Embedded: Default config files** — Ship `midi.yaml`, `slicer-presets.yaml`,
  and `theme.yaml` to `/home/mesh/Music/mesh-collection/` via systemd tmpfiles
  `C` (copy-if-not-exists) rules, so the Orange Pi boots with working defaults
  while preserving any user modifications on subsequent updates.
- **Embedded: PAM audio limits** — `@audio` group gets unlimited memlock,
  rtprio 99, and nice -19 for real-time audio scheduling.
- **Embedded: RT kernel tuning** — Added `threadirqs` kernel parameter (threads
  all IRQ handlers for priority control) and `vm.swappiness=10` (keeps audio
  buffers in RAM).

### Changed

- **CI: Split native deps cache from Rust build cache** — Essentia, FFmpeg, and
  TagLib (pinned C/C++ libraries that never change) now have a separate GitHub
  Actions cache keyed on the build script hash instead of `Cargo.lock`. Previously,
  any Rust dependency update invalidated the entire cache, forcing 10-60 minute
  rebuilds of unchanged native libraries. The stable deps cache persists across
  Cargo.lock changes, saving significant CI time on every release.
- **CI: Regenerated binary cache signing key** — Replaced the cache signing key
  pair and fixed the narinfo signing pipeline. `nix copy` and `nix store sign`
  are now separate steps with key format validation and post-sign verification
  to prevent silent signing failures.

---

## [0.9.2]

### Added

- **In-app WiFi management** — Settings now include a Network section with WiFi
  scanning, connection, and disconnection. Uses `nmrs` (Rust D-Bus bindings for
  NetworkManager) instead of shell-based `nmcli` for type-safe, reliable network
  operations. Each D-Bus call runs on a dedicated thread with its own
  single-threaded tokio runtime to work around nmrs's `!Send` futures and iced's
  nested-runtime constraint. Secured networks open an on-screen keyboard for
  password entry. The Cancel button is part of the key grid (after Done) so it's
  reachable via MIDI encoder navigation. Keys with distinct shifted symbols
  (numbers, punctuation) show a small dark-gray hint in the bottom-right corner
  so users know which symbols are available via Shift without guessing.
  Platform-gated: Linux-only via `#[cfg(target_os = "linux")]` with no-op stubs
  on other platforms, so Windows builds are unaffected. The on-screen keyboard
  widget lives in mesh-widgets for reuse across crates.
- **OTA system updates** — New System Update section in settings checks GitHub
  releases for newer versions, installs via the `mesh-update` systemd service,
  shows live journal output during installation, and restarts the cage compositor
  to run the new binary. Only active on NixOS embedded (detected by `/etc/NIXOS`).
- **MIDI settings navigation** — New `global.settings_toggle` action
  opens/closes the settings modal via MIDI. When open, the browser encoder
  scrolls through settings, encoder press enters editing mode for the focused
  setting, and scroll cycles through options with live draft preview. Closing
  auto-saves if changes were made. Opening settings automatically forces browse
  mode on the mapping engine so encoders that share loop-size and browser-scroll
  mappings (mode-switched) produce browser events for navigation. Previous
  browse mode state is saved and restored on close. The settings scrollable
  auto-scrolls to keep the focused setting visible as the encoder moves through
  the list. Audio device dropdowns expand into inline button groups during
  editing mode so all options are visible while cycling with the encoder.
- **MIDI sub-panel navigation** — When MIDI-navigating to the Network or System
  Update entries in settings, pressing the encoder enters a domain-specific
  sub-panel directly (no editing-mode step). WiFi sub-panel: encoder cycles
  through scanned networks, press connects (or opens keyboard for secured
  networks). Update sub-panel: encoder cycles between Check and Install/Restart
  actions with visual highlighting on the focused action. Shift+encoder press
  steps out of the current mode (sub-panel → scroll). The MIDI Learn section
  is now a navigable entry — encoder press triggers Start MIDI Learn directly.
  Priority chain: keyboard > sub-panel > settings edit > settings scroll >
  normal MIDI.
- **Embedded: silent boot** — Comprehensive kernel param and systemd
  configuration for minimal boot output: `loglevel=0`, `quiet`,
  `rd.systemd.show_status=false`, `systemd.show_status=false`,
  `rd.udev.log_level=3`, `kernel.printk=0 0 0 0`, `vt.global_cursor_default=0`,
  `logo.nologo`. Replaces the previous Plymouth-based splash which failed to
  render the custom script theme on ARM/RK3588S (fell back to NixOS default).
- **Embedded: NetworkManager permissions** — mesh user added to
  `networkmanager` group, polkit rules expanded to allow managing both
  `mesh-update.service` and `cage-tty1.service`.

---

## [0.9.1]

### Fixed

- **Embedded: mesh-player crash on boot (`NoWaylandLib`)** — PipeWire's PAM
  session overrides the systemd `Environment=` directive, clobbering
  `LD_LIBRARY_PATH` with only `pipewire-jack/lib`. winit/wgpu `dlopen()` calls
  for `libwayland-client.so`, `libxkbcommon.so`, `libEGL.so`, and
  `libvulkan.so` then fail. Fixed with a wrapper script that sets
  `LD_LIBRARY_PATH` before exec'ing mesh-player, immune to PAM overrides.

### Added

- **Embedded: USB automounting** — udev rules auto-mount USB sticks to
  `/media/<label>` via `systemd-mount` when plugged in, and clean up on removal.
  No daemon, no D-Bus session, no polkit required — runs directly from udev
  context. Mounted with `noatime` to reduce background writes and make
  hot-unplug safer. mesh-player detects new mounts via its existing 2-second
  `sysinfo` polling loop.
- **Embedded: debugging infrastructure** — cage `-s` flag enables VT switching
  (Ctrl+Alt+F2), TTY2 getty provides a login shell for local debugging,
  persistent journal (`Storage=persistent`, 50MB cap) preserves logs across
  reboots, and `boot.initrd.systemd.emergencyAccess` enables emergency shell
  access during boot failures.
- **Windows cross-compilation failing on `stdbool.h`** — The container-based
  Windows build (`build-windows.nix`) set `BINDGEN_EXTRA_CLANG_ARGS` with
  `--sysroot=/usr/x86_64-w64-mingw32` for Essentia's cross-compilation, but
  forgot to unset it before building mesh-player. This caused clang to search
  the MinGW sysroot for compiler built-in headers like `stdbool.h`, which
  aren't there — they live in clang's resource directory. Fixed by unsetting
  `BINDGEN_EXTRA_CLANG_ARGS` before Phase 4 (mesh-player) and re-exporting it
  with the clang resource directory explicitly included before Phase 5
  (mesh-cue).

---

## [0.9.0]

### Added

- **Metadata parsing for import pipeline** — Artist and title extraction now
  reads embedded audio tags (ID3v2, Vorbis, MP4, FLAC) via lofty before falling
  back to filename parsing. The filename parser handles UVR5 numeric prefixes
  (`56_Artist - Title`), track number prefixes (`01 - `), en/em dashes,
  underscore separators, and multi-dash filenames with DB-assisted known-artist
  disambiguation (`Black Sun Empire - Arrakis - Remix` correctly splits on the
  artist). Artist connectors (`&`, `feat.`, `ft.`, `x`, `vs.`) are normalized
  to comma-separated lists, square brackets are converted to parentheses, and
  `(Original Mix)` is stripped. The known-artist set is loaded once per batch
  from the database for case-insensitive matching.
- **Suggestion energy slider in MIDI learn** — The Browser phase now includes
  two Suggestion Energy steps (left and right side), placed after the browse
  encoder. This allows mapping physical knobs/faders to the smart suggestion
  energy direction slider during MIDI learn. The `deck.suggestion_energy` action
  controls the global energy bias (DROP ↔ PEAK) used by the suggestion engine.
- **Streaming track loading with priority regions** — Track loading is now a
  three-phase progressive pipeline: (1) skeleton with metadata loads instantly
  (<10 ms), giving immediate access to beat markers, cue markers, and navigation;
  (2) priority regions around hot cues and the drop marker load next (~200 ms);
  (3) remaining audio fills in incrementally. The DJ can beat-jump, seek, and
  navigate cue points while audio loads in the background.
- **Incremental waveform visualization** — The overview waveform now grows
  visually as audio loads. Priority regions (hot cue areas) appear first, then
  gap regions fill in progressively in ~15-second visual batches. Unloaded areas
  render as flat/silent, giving clear visual feedback of which parts of the track
  are ready for playback. High-resolution zoomed peaks also update incrementally.
- **Instant partial playback during loading** — Stem buffer snapshots are
  delivered to the audio engine at ~100-second intervals via `UpgradeStems`, so
  the DJ can press play or cue and hear audio from any loaded region. Visual
  peak updates (cheap, ~2 MB) are decoupled from stem clones (expensive,
  ~460 MB) — the waveform grows smoothly while playback catches up at clone
  boundaries. Unloaded areas produce silence on playback.
- **Region-based audio file reading** — New `read_region_into()` method on
  `AudioFileReader` enables seeking to arbitrary sample positions and reading
  directly into pre-allocated stem buffers. Supports 16-bit, 24-bit, and
  32-bit (float and integer) formats. Existing full-read methods now delegate
  to the region reader internally, eliminating code duplication.
- **Engine `UpgradeStems` command** — New real-time-safe command that upgrades
  a deck's stem buffers without resetting playback position. Uses `basedrop::Shared`
  for lock-free deallocation on the audio thread.
- **Skeleton track loading** — `create_skeleton_and_load()` on the domain layer
  creates an instant-load track with zero-length stems but correct duration,
  beat grid, cue points, and metadata. The engine uses `duration_samples` for
  navigation and `stem_data.len()` for audio reads, so navigation works
  immediately while stems are still empty.
- **Original filename preservation** — The raw filename (`base_name`) is now
  saved as `original_name` in the tracks database before metadata parsing
  normalizes it into artist/title. This enables re-running metadata analysis
  later (e.g., after parser improvements) without reimporting. Existing
  databases are migrated automatically on startup.
- **Reanalysis overhaul** — The context menu now offers two reanalysis actions
  instead of five: "Re-analyse Beats" fires immediately (unchanged BPM/beat
  grid pipeline), while "Re-analyse Metadata..." opens a modal with four
  checkboxes (all enabled by default): Name/Artist (re-parse `original_name`),
  Loudness (LUFS via Essentia subprocess), Key (key detection via Essentia),
  and Tags (genre, mood, vocal detection via EffNet ML pipeline). Only the
  ticked analyses run, and Essentia subprocess calls are batched when both
  loudness and key are selected. Beat analysis is kept separate since beat
  grids are frequently edited manually.
- **DB schema migration for tracks** — Automatic migration detects old track
  schemas missing the `original_name` column. Data is backed up, the relation
  is recreated with the new schema, and all rows are restored with
  `original_name` defaulted to empty string. Runs transparently on startup.

### Improved

- **Drop-aware LUFS measurement** — Loudness analysis now targets the loudest
  sections of a track (drops, refrains) instead of the whole-track average, so
  auto-gain matches tracks where it matters. Requires LUFS reanalysis.
- **Suggestion energy MIDI debounce** — The energy direction fader now uses
  trailing-edge debounce (300ms) so the suggestion query only fires once the
  fader stops moving, instead of on every value change. Moving the fader also
  auto-enables suggestion mode if not already active.
- **Track load memory usage** — Stem clones (~460 MB each) are sent only at
  ~100-second intervals (~5 clones per 5-minute track, ~500 ms total overhead).
  Visual peak updates are sent every ~15 seconds at negligible cost (~2 MB).
  Peak memory during loading is ~920 MB; the `basedrop` GC thread collects
  stale clones within 100 ms, preventing unbounded growth.
- **Priority region planning** — New `regions` module computes optimal load
  regions around hot cues, drop markers, and the first beat. Regions within
  64 beats of each other are merged to minimize seek operations. Gap regions
  (everything not covered by priority areas) are computed for sequential
  background filling.

### Fixed

- **Embedded SD image not booting on Orange Pi 5** — The SD card image built by
  CI was missing the U-Boot bootloader. The upstream `gnull/nixos-rk3588` Orange
  Pi 5 module does not embed U-Boot (it expects SPI NOR flash to be
  pre-programmed). Added prebuilt U-Boot binaries (idbloader.img + u-boot.itb,
  extracted from official Orange Pi Debian v1.1.8) and `postBuildCommands` that
  `dd` them into the image gap at the Rockchip-mandated sector offsets (64 and
  16384). The image now boots on a factory-fresh board with no prior setup.
- **Board name mismatch** — All references to "Orange Pi 5 Pro" corrected to
  "Orange Pi 5" across flake.nix, CI workflows, devshell, and flash script.
  The target board is the base Orange Pi 5 (RK3588S).

---

## [0.8.10]

### Improved

- **USB export throughput** — Rewrote the export pipeline to separate file I/O
  from database I/O. Track files are now copied sequentially with 1 MB buffered
  writes and `fsync` per file (replacing parallel random writes via `par_iter`).
  The USB database is staged locally: copied to a temp directory, updated there
  with all metadata/playlist/deletion operations, then written back as a single
  sequential copy. This eliminates random I/O on flash storage and should reduce
  export times by 50–70%.
- **Batched tag inserts** — `sync_track_atomic` now uses a single CozoDB batch
  query for tag insertion instead of N individual `:put` operations per track.
- **Buffered file copy with fsync** — New `copy_large_file()` utility uses
  `BufReader`/`BufWriter` with 1 MB buffers, `posix_fadvise(SEQUENTIAL)` on
  Linux, and `sync_all()` for data safety on removable media.
- **Simplified export progress** — Merged five separate metadata/playlist
  progress phases into a single unified "Updating database" phase, reducing UI
  complexity and message overhead.

---

## [0.8.9]

### Fixed

- **Browser not updating during import/reanalysis** — Track metadata (BPM, key,
  tags, new tracks) now refreshes in real-time as each track completes instead
  of requiring manual navigation. The tick handler was silently discarding all
  `Task` returns from progress handlers; these are now collected and returned
  via `Task::batch()`. Reanalysis also fires per-track `RefreshCollection` on
  success, matching the pattern import already used.
- **Audio muted during USB export** — Removed unnecessary audio stream
  pause/resume around USB export. Only import and reanalysis (which are
  CPU-intensive) pause the stream; export is I/O-bound and doesn't need it.
- **Tags column too wide in mesh-cue** — Reduced track table Tags column from
  300px to 150px so the Name column has more room.
- **mesh-cue Windows build failing** — mesh-cue hardcoded `pd-effects` as a
  direct dependency, pulling in `libffi-sys` which fails to cross-compile for
  MinGW. Now feature-gated like mesh-player: `pd-effects` is a default feature
  (enabled on Linux) but disabled by `--no-default-features` on Windows. The
  PD stub module was also updated with missing methods/fields. Windows build
  script now fails on either crate instead of silently skipping mesh-cue.
- **mesh-cue Windows linker error** — `build.rs` emitted ELF-specific linker
  flags (`--disable-new-dtags`, `--no-as-needed`, `-rpath`) unconditionally.
  MinGW's `ld` doesn't recognize these. Now gated behind a `TARGET` check so
  they only apply on Linux.

---

## [0.8.8]

### Fixed

- **Audio crackling during batch operations** — CPAL audio stream is now paused
  during import, export, and reanalysis to eliminate buffer underruns caused by
  CPU contention between the real-time audio callback and heavy processing
  threads (ML inference, stem separation, file I/O). The stream starts paused
  at launch (no track loaded = no audio needed) and resumes only when a track
  is loaded for preview. Cancel and error paths also correctly resume audio.

---

## [0.8.7]

### Added

- **Cross-source suggestions** — Suggestions now query all connected databases
  (local + USB). HNSW vector search runs across both sources, combining results
  into a unified ranked list with source tags ("Local" / "USB") on each
  suggestion.
- **Cross-source deduplication** — When the same track exists in both local and
  USB databases, only the entry with the best HNSW distance is kept, preventing
  duplicate suggestions.
- **USB export: tags, ML analysis, audio features & presets** — USB export now
  syncs ML analysis data, track tags, and audio feature vectors alongside track
  files. Effect presets (stems, decks, slicer) are also copied to USB.
- **Metadata sync progress** — USB export reports per-track progress during the
  metadata-only sync phase, keeping the overlay progress bar responsive.
- **ML audio analysis** — 6 new EffNet classification heads: timbre
  (bright/dark), tonal/atonal, acoustic, electronic, danceability, and
  approachability. ML-based vocal detection replaces RMS-based approach.
- **Energy-direction suggestion scoring** — Suggestions incorporate ML arousal
  scores, genre-normalized aggression, and production match scoring. Key scoring
  blends toward energy direction at fader extremes.
- **Event-driven seed refresh** — Suggestion seeds now auto-refresh on deck
  load, play/pause, and volume changes with debounced timer.
- **Multi-factor reason tags** — Suggestion entries show sorted reason tags
  (key compatibility, energy direction) with color-coded confidence.
- **Hierarchical USB playlists** — USB export supports nested playlist folders
  with portable relative paths.

### Fixed

- **Audio features not exported to USB** — `get_audio_features()` failed
  silently on CozoDB's `DataValue::Vec(Vector::F32(...))` type, only matching
  `DataValue::List`. Audio feature vectors were never synced to USB.
- **Cross-DB HNSW search** — `find_similar_by_vector()` passed the query vector
  as `DataValue::List`, but HNSW requires a proper Vector type. Fixed with
  CozoScript's `vec()` function.
- **USB track metadata lookup** — `load_track_metadata()` now converts absolute
  paths to relative paths for USB storage.
- **USB playlist browsing** — Fixed playlist browsing broken after relative-path
  migration.
- **Track deletion cleanup** — `delete_track` now cleans all child relations
  (cue points, saved loops, stem links, tags, ML analysis, audio features).
- **Export phase message ordering** — Corrected progress message ordering to
  prevent UI stall during export.
- **DnB sub-genre consolidation** — Consolidated DnB sub-genre tags, suppressed
  redundant Instrumental genre tag.

### Improved

- **Tick handler performance** — Optimized hot-path tick handler with
  documentation for lock-free architecture.
- **Export metadata sync performance** — Replaced O(n^2) per-track
  `get_all_tracks()` scan with pre-built `HashMap` lookup.
