# Changelog

All notable changes to Mesh are documented in this file.

## [0.5.0] - 2026-01-28

> **⚠️ Note:** Stem separation features in this release are experimental. GPU acceleration (CUDA on Linux, DirectML on Windows) is untested and may not work on all systems.

### Added

- **HTDemucs hybrid model support** — Full implementation of the Demucs v4 hybrid transformer architecture with both time-domain and frequency-domain branches. The frequency branch uses STFT/ISTFT processing to capture spectral details that the time branch might miss.

- **Shift augmentation** — Configurable random time-shifting (1-5 shifts) that averages multiple inference passes for improved separation quality. Each additional shift adds ~0.2 SDR improvement at the cost of proportionally longer processing time. Configurable in Settings under "Separation Shifts".

- **Residual "other" stem computation** — Instead of using the model's direct "other" prediction (which often contains vocal/hihat artifacts), the "other" stem is now computed as `mix - drums - bass - vocals`. This ensures perfect reconstruction and eliminates bleed from other stems.

- **50% segment overlap** — Increased from 25% to 50% overlap between processing segments. This provides better handling of transients (especially hihats) at segment boundaries, at the cost of ~2x processing time.

- **High-frequency preservation for drums** — Neural networks often attenuate frequencies above 14kHz. For the drum stem, frequencies above 14kHz are now blended from the original mix using a smooth spectral crossfade, restoring hihat and cymbal crispness without introducing bleed.

- **External data file support** — Model downloads now correctly handle ONNX external data files (`.onnx.data`). Large models store weights separately from the graph structure, and both files are now downloaded and managed together.

- **Fine-tuned model option** — Added support for `htdemucs_ft` (fine-tuned) model variant, which provides ~1-3% better SDR than the standard model with the same architecture.

- **Model selection in settings** — Choose between "Demucs 4-stem" (fast) and "Demucs 4-stem Fine-tuned" (better quality) in Settings > Separation Model.

### Changed

- **Model download URLs** — Models are now downloaded from GitHub releases (`releases/download/models/`) for faster and more reliable downloads.

- **Simplified model options** — Removed 6-stem model support. Only 4-stem models (vocals, drums, bass, other) are now available, which is the standard configuration for DJ workflows.

- **Progress reporting for downloads** — Model download progress now correctly accounts for both `.onnx` (~2MB) and `.onnx.data` (~160MB) files, with the large data file representing 98% of progress.

### Fixed

- **WAVE_FORMAT_EXTENSIBLE support** — Fixed WAV file import for files using the extensible format header (common in professional audio software).

- **STFT preprocessing** — Fixed spectrogram computation to exactly match Demucs' `standalone_spec` function, including proper reflection padding, frame cropping, and normalization.

### Build System

- **GPU-accelerated builds** — Added compile-time GPU acceleration support for stem separation:
  - `nix run .#build-deb` — CPU-only Linux build (works everywhere)
  - `nix run .#build-deb-cuda` — Linux build with NVIDIA CUDA 12 support
  - `nix run .#build-windows` — Windows build with DirectML (AMD/NVIDIA/Intel via DirectX 12)

- **DirectML for Windows** — Windows builds now include DirectML support by default. DirectML is built into Windows 10+ and provides GPU acceleration for any DirectX 12 capable GPU without additional driver installation.

- **CUDA for Linux** — Optional CUDA 12 builds available for NVIDIA GPU users. Requires NVIDIA driver 525+ and CUDA toolkit on target system.

- **Runtime DLL loading for Windows** — Windows builds now use the `load-dynamic` ort feature, which loads `onnxruntime.dll` at runtime via `LoadLibrary()` instead of linking at compile time. This bypasses MinGW/MSVC ABI incompatibility and enables MinGW cross-compiled builds to use Microsoft's pre-built DirectML binaries.

- **Fixed Windows packaging** — Zip package creation now removes old zip files before creating new ones, preventing "missing end signature" errors from corrupted partial builds.

---

## [0.4.4] - 2026-01-26

### Added

- **Automatic stem separation** — Import regular audio files (MP3, FLAC, WAV, OGG, M4A) and mesh-cue will automatically separate them into stems using the Demucs neural network. No external tools required!

- **Import mode toggle** — Switch between "Pre-separated Stems" and "Mixed Audio" modes in the import modal. Mixed audio mode handles stem separation automatically.

- **Separation progress display** — During mixed audio import, the progress bar shows real-time separation progress (e.g., "Track Name (separating 45%)").

- **Modular separation backend** — Architecture supports swappable backends (ONNX Runtime, future Charon). Models are downloaded automatically on first use (~171MB, cached in `~/.cache/mesh-cue/models/`).

### Changed

- **Import modal redesigned** — New dual-mode UI with toggle buttons for selecting import type. Mixed audio mode shows detected audio files with format indicators (MP3, FLAC, etc.).

- **README Quick Start updated** — Now explains both import options (automatic separation vs. pre-separated stems) for new users.

---

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

- **Zoomed waveform scrubbing** — Click and drag horizontally on the zoomed waveform to scrub through the track. Drag direction is auto-detected: horizontal = scrub, vertical = zoom. Waveform moves 1:1 with mouse like grabbing vinyl.

- **BPM adjustment buttons** — Plus and minus buttons next to the BPM field for quick ±1 BPM adjustments. Beat grid is automatically recalculated.

- **Beat grid align button** — New "│" button next to grid nudge controls sets the current playhead position as a downbeat. Scrub to where you hear the "1" of the bar, click the button (or press "m"), and the grid aligns to that position.

- **Vinyl-style scratch audio** — During zoomed waveform scrubbing, audio plays based on mouse velocity like real vinyl. Moving the mouse plays audio at proportional speed; stopping outputs silence (not a looped buffer). Backward movement plays audio in reverse. Previous play state is restored when scrubbing ends.

- **Scratch interpolation setting** — Choose between Linear (fast, acceptable quality) and Cubic (Catmull-Rom, smoother audio) interpolation for scratch audio in Settings > Audio Output. Cubic interpolation provides higher quality variable-speed playback at the cost of slightly more CPU usage.

### Changed

- **MidiController architecture** — Refactored from single-device (`Option<MidiInputHandler>`) to multi-device (`HashMap<String, ConnectedDevice>`) support.

- **Global shift state** — Shift state is now shared across all connected MIDI devices for consistent behavior.

- **LED feedback** — Feedback is now sent to all connected devices, not just the first one.

### Fixed

- **Cross-system MIDI compatibility** — Devices now connect correctly when hardware enumeration differs between systems (e.g., `hw:1,0,0` on Pop!_OS vs `hw:3,0,0` on NixOS).

- **Collection auto-refresh after import** — Newly imported tracks now appear in the file browser immediately after analysis completes.

- **Text overlap in file browser** — Long track names and labels are now clipped instead of overlapping adjacent columns.

- **File browser layout** — Editor and browser panels now use proportional sizing (3:1 ratio) to prevent the browser from squeezing hot cue buttons.

- **Beat grid preserved on BPM change** — Changing BPM now anchors the grid on the beat nearest to the playhead instead of the first beat. This preserves nudge adjustments — the beat you're listening to stays in place while others recalculate.

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
