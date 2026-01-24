# Changelog

All notable changes to Mesh are documented in this file.

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
