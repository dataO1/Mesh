# Changelog

All notable changes to Mesh are documented in this file.

## [0.4.3] - 2026-01-25

### Added

- **Multi-device MIDI support** — Connect multiple MIDI controllers simultaneously. All configured devices are connected at startup, not just the first match.

- **Drag-and-drop on file list** — Tracks can now be dropped onto the file list panel (right column) in mesh-cue, not just playlist labels.

- **Shift+click to delete stem links** — Hold Shift and click a linked stem button to remove the link.

- **Drag indicator** — When dragging tracks, a semi-transparent label appears near the cursor showing the track name (or "name..." for multiple tracks).

- **Drop zone highlight** — The file browser shows a teal border outline when hovering over it while dragging tracks, indicating where tracks will be dropped.

- **Drag threshold** — Dragging only starts after moving 8 pixels from the initial click, preventing accidental drags and preserving double-click to load tracks.

- **Automatic port name capture** — During MIDI learn, the actual system port name is captured and stored as `learned_port_name` for precise device matching on reconnection.

- **Port name normalization** — Hardware IDs like `[hw:3,0,0]` are stripped from port names, so devices match regardless of which USB port they're connected to.

- **Device matching with fallback** — Exact match against `learned_port_name` is tried first, falling back to substring match against `port_match` for backwards compatibility.

- **New MidiController methods** — `connected_count()`, `connected_device_names()`, `first_connected_port()`, and `drain_raw_events_with_source()` for multi-device management.

- **Dynamic audio output switching (JACK)** — On Linux with JACK, audio outputs can now be changed in settings without restarting the app. Works in both mesh-player (master/cue outputs) and mesh-cue (single output). Device selection is saved to config.

### Changed

- **MidiController architecture** — Refactored from single-device (`Option<MidiInputHandler>`) to multi-device (`HashMap<String, ConnectedDevice>`) support.

- **Global shift state** — Shift state is now shared across all connected MIDI devices for consistent behavior.

- **LED feedback** — Feedback is now sent to all connected devices, not just the first one.

### Fixed

- **Cross-system MIDI compatibility** — Devices now connect correctly when hardware enumeration differs between systems (e.g., `hw:1,0,0` on Pop!_OS vs `hw:3,0,0` on NixOS).

- **Collection auto-refresh after import** — Newly imported tracks now appear in the file browser immediately after analysis completes.

- **Text overlap in file browser** — Long track names and labels are now clipped instead of overlapping adjacent columns.

- **File browser layout** — Editor and browser panels now use proportional sizing (3:1 ratio) to prevent the browser from squeezing hot cue buttons.

---

## [0.4.2] - 2026-01-24

### Changed

- **Container-based .deb packaging** — Replaced Nix-based .deb derivation with Ubuntu 22.04 container build. Ensures compatibility with Pop!_OS 22.04+, Ubuntu 22.04+, Debian 12+, and Linux Mint 21+ by targeting glibc 2.35.

- **Bundled TagLib 2.x** — Added `libtag.so.2` to bundled libraries for mesh-cue. Older distros only ship TagLib 1.x, which is ABI-incompatible.

### Added

- **New build command** — `nix run .#build-deb` builds portable .deb packages in `dist/deb/`. First build takes ~15 minutes (caches Rust toolchain and dependencies), subsequent builds ~1-2 minutes.

- **Build caching** — Container builds cache Rust toolchain, cargo registry, compiled dependencies, FFmpeg 4.x, TagLib 2.x, and Essentia in `target/deb-build/` for fast incremental builds.

- **Verbose build output** — Build progress shows numbered phases [1/8] through [8/8] with detailed status messages.

### Removed

- **Nix .deb derivation** — Removed `nix/packages/mesh-deb.nix` and `nix build .#mesh-deb`. The Nix-based build used the host's glibc (2.39+), causing "GLIBC_2.39 not found" errors on older distros.

### Fixed

- **glibc compatibility** — .deb packages now work on systems with glibc 2.35+ (previously required 2.39+).

- **TagLib dependency** — Fixed "libtag.so.2 not found" error by bundling TagLib 2.x instead of depending on system package.

---

## [0.4.1] - 2026-01-24

### Added

- **Native JACK backend for Linux** — Full port-level routing control for pro-audio interfaces (e.g., route master to outputs 1-2 and cue to outputs 3-4 on a Scarlett 18i20). Enabled by default on Linux.
- **Cross-platform audio via CPAL** — Windows and macOS support using the system's native audio API (WASAPI/CoreAudio).
- **Windows builds** — Cross-compiled `.exe` distributions with bundled DLLs.
- **Debian/Ubuntu packages** — Native `.deb` packages with bundled Essentia and FFmpeg 4.x libraries.

### Fixed

- Console window no longer appears on Windows GUI applications.
- Sample positions in track metadata now correctly scale after resampling.
- Cross-platform home directory detection using `dirs::home_dir()`.

### Changed

- JACK backend is now the default on Linux (use `--no-default-features` for CPAL).
- Improved PipeWire compatibility with JACK port naming (FL/FR/RL/RR).

---

## [0.3.2] - Previous Release

Initial stem-based DJ mixing with 4-deck architecture, beat sync, key matching, and stem slicer.
