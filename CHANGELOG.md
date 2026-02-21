# Changelog

All notable changes to Mesh are documented in this file.

---

## [0.9.4]

### Performance

- **Rendering: GPU shader waveforms** — Zoomed waveform rendering now uses a custom
  WGSL fragment shader (`waveform.wgsl`) instead of CPU-based lyon tessellation via
  iced's Canvas widget. Peak data is uploaded once at track load as a GPU storage
  buffer (~128KB per deck). Per-frame updates require only a 400-byte uniform buffer
  write containing playhead position, stem colors, loop region, BPM grid, cue markers,
  smoothing parameters, and volume. This eliminates ~16,000 `line_to()` calls, ~8,000
  `exp()` calls, ~100 Vec allocations (~1MB), and lyon tessellation of 32+ paths that
  previously ran every frame. At 120Hz with 4 decks, this reduces CPU rendering time
  from ~12-16ms to <1ms per frame.

- **Rendering: change-guarded cache invalidation** — All 19 `set_*` methods on
  `PlayerCanvasState` now check `!= old_value` before calling `invalidate_cache()`.
  Previously, every setter invalidated unconditionally — the tick handler alone
  triggered 84 invalidations per tick (21 setters × 4 decks), making `canvas::Cache`
  completely useless during playback. With guards, only actual value changes trigger
  redraws (~4/tick for playing decks' playheads). Float comparisons use epsilon
  threshold to avoid false invalidations from floating-point drift.

### Added

- **Waveform shader module** (`mesh-widgets/waveform/shader/`) — New GPU-accelerated
  waveform rendering pipeline built on iced's `shader::Program` trait. Includes:
  - `PeakBuffer`: Arc-wrapped flattened peak data for zero-copy GPU upload, with
    `Arc::as_ptr()` change detection (zero-cost per frame, no content hashing)
  - `WaveformPrimitive`: Per-frame primitive carrying uniforms + peak buffer reference
  - `WaveformPipeline`: wgpu render pipeline with per-view resource caching, dynamic
    storage buffer resizing, and two-binding layout (uniform + storage)
  - `WaveformProgram`: iced shader widget with click-to-seek (overview) and
    drag-to-zoom (zoomed) interaction handling
  - Fragment shader renders all elements in one pass: background → loop region →
    beat markers (procedural from BPM) → stem envelopes × 4 → cue markers →
    playhead → volume dimming, with `smoothstep()` anti-aliasing
  - `WaveformAction` enum for message-agnostic seek/zoom events
  - View helpers: `waveform_shader_zoomed()`, `waveform_shader_overview()`,
    `waveform_player_shader()`

- **Rendering: hybrid canvas/shader composition** — The 4-deck waveform display now
  uses a two-layer `Stack`: canvas on bottom (headers + overview waveforms) and shader
  widgets on top (zoomed waveforms). The canvas only redraws on structural changes
  (track load, stem mute, loop toggle), not on every playhead tick. The shader handles
  all per-frame animation. This eliminates all CPU lyon tessellation from the playback
  hot path while preserving the canvas text rendering for deck headers (badge, track
  name, BPM, key, loop indicator, LUFS gain).

- **Rendering: stable grid-aligned peak sampling** — The WGSL shader now reproduces the
  old canvas's "STABLE RENDERING" approach: peaks are sampled at step-aligned grid points
  anchored to the track (not the window), then linearly interpolated. This eliminates
  "dancing peaks" caused by intersample jitter when the playhead shifts peak indices by
  fractional amounts between frames. Per-stem subsampling via `HIGHRES_PIXELS_PER_POINT`
  (Vocals/Drums/Other=1.0, Bass=2.5) reduces rendered detail to match the old abstract
  look. Gaussian smoothing at each grid point uses per-stem radius multipliers
  (Drums=0.1, Vocals=0.25, Bass=0.4, Other=0.4) to keep drums sharp while smoothing
  bass and pad instruments.

### Fixed

- **Rendering: beat grid not visible in shader** — Beat marker threshold calculations
  in the WGSL shader used UV-space pixel widths (`1.0/width`) for source-space
  comparisons, causing thresholds to exceed 1.0 in zoomed views (every pixel matched
  as a beat line, making them invisible). Fixed by computing `px_in_source` based on
  the view mode: `1.0/width` for overview, `(win_end - win_start)/width` for zoomed.
  Also corrected cue marker and slicer line widths with the same fix.

- **Rendering: waveform-audio drift** — The shader hardcoded sample rate as 44100 Hz
  but the audio engine runs at 48000 Hz. The 8.8% error caused ~2-3 seconds of visual
  drift over a 4-minute track because `interpolated_playhead()` advanced too slowly
  and BPM-based window width calculations were too narrow. Fixed by using the correct
  engine sample rate constant.

- **Rendering: incremental loading broken** — The `RegionLoaded` handler updated raw
  peak arrays but did not rebuild the GPU `PeakBuffer`, so the shader showed stale data
  until the full `Complete` message arrived. Now rebuilds `overview_peak_buffer` and
  `highres_peak_buffer` after each region, restoring incremental waveform growth.

- **Rendering: track edge jump** — Zoomed waveform used `u64::saturating_sub()` for
  window positioning, clamping the window start to 0 at the track beginning. This
  caused the playhead to visually jump off-center. Fixed with signed `i64` arithmetic
  that allows the window to extend before the track start, with the shader rendering
  out-of-range regions as silence.

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
