# Changelog

All notable changes to Mesh are documented in this file.

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
