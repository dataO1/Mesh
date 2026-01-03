# Mesh

**A modern DJ software suite built in Rust with stem-based mixing and neural audio effects.**

Mesh is an open-source DJ application designed for live performance with a focus on stem separation, real-time audio processing, and creative sound manipulation through neural networks.

---

## Overview

### What is Mesh?

Mesh is a professional DJ software suite consisting of two applications:

- **mesh-player** — A 4-deck DJ player for live performance with stem-based mixing
- **mesh-cue** — A track preparation tool for analyzing, tagging, and organizing your music library

### What makes it different?

Unlike traditional DJ software that works with stereo audio files, Mesh is built around **stem-based mixing**. Each track is split into 4 stems (Vocals, Drums, Bass, Other), giving you independent control over each element:

- Mute the vocals for an instrumental mix
- Solo the drums for a breakdown
- Apply different effects to each stem
- Create mashups and remixes on the fly

Mesh also integrates **neural audio effects** powered by [RAVE](https://github.com/acids-ircam/RAVE) (Realtime Audio Variational autoEncoder), allowing you to transform sounds in ways that traditional effects cannot achieve.

### Goals

1. **Professional-grade audio quality** — Low-latency processing with proper gain staging and latency compensation
2. **Creative freedom** — Per-stem effects, neural processing, and flexible routing
3. **Open source** — No subscriptions, no cloud dependencies, runs entirely on your hardware
4. **Cross-platform** — Built with Rust for Linux (macOS and Windows support planned)

---

## Architecture

### Technology Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| **Audio Engine** | Rust | Real-time audio processing with zero-copy buffers |
| **Audio I/O** | JACK | Professional low-latency audio routing |
| **GUI** | iced | Native GPU-accelerated user interface |
| **Time Stretching** | signalsmith-stretch | High-quality tempo adjustment without pitch change |
| **Effects** | Pure Data (libpd) | Visual patching for custom effects |
| **Neural Audio** | RAVE + libtorch | Real-time neural audio transformation |
| **Audio Analysis** | Essentia | BPM detection, key detection, beat tracking |

### Project Structure

```
mesh/
├── crates/
│   ├── mesh-core/       # Core audio engine library
│   │   ├── audio_file/  # WAV/RF64 file loading with metadata
│   │   ├── effect/      # Effect system and native effects
│   │   ├── engine/      # Decks, mixer, latency compensation
│   │   └── timestretch/ # Tempo adjustment wrapper
│   ├── mesh-player/     # DJ player application
│   │   ├── audio.rs     # JACK client
│   │   └── ui/          # iced GUI components
│   └── mesh-cue/        # Track preparation app
└── flake.nix            # Nix development environment
```

### Audio Signal Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                         DECK (x4)                                │
│  ┌─────────┐   ┌─────────────────────────────────────────────┐  │
│  │  Track  │──▶│  Stems: Vocals │ Drums │ Bass │ Other       │  │
│  │  File   │   │         ↓         ↓       ↓       ↓         │  │
│  └─────────┘   │    Effect Chain (per stem)                  │  │
│                │         ↓         ↓       ↓       ↓         │  │
│                │    ─────────── Sum ──────────────           │  │
│                └─────────────────────────────────────────────┘  │
│                              ↓                                   │
│                    Latency Compensation                          │
│                              ↓                                   │
│                      Time Stretcher                              │
└──────────────────────────────┬──────────────────────────────────┘
                               ↓
┌─────────────────────────────────────────────────────────────────┐
│                          MIXER                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Channel Strip (x4): Trim → Filter → Volume → Cue/Master │   │
│  └──────────────────────────────────────────────────────────┘   │
│                    ↓                         ↓                   │
│              Master Bus                  Cue Bus                 │
│                    ↓                         ↓                   │
│              Master L/R                  Cue L/R                 │
└─────────────────────┬───────────────────────┬───────────────────┘
                      ↓                       ↓
                 ┌─────────────────────────────────┐
                 │         JACK Output             │
                 │   (4 channels to audio interface)│
                 └─────────────────────────────────┘
```

---

## Features

### Implemented ✅

**Core Engine**
- 4-deck architecture with independent playback
- Stem-based audio (4 stereo stems per track: Vocals, Drums, Bass, Other)
- Per-stem mute/solo and effect chains
- Global latency compensation across all stems and effects
- High-quality time stretching with signalsmith-stretch
- WAV/RF64 file support with embedded metadata

**Deck Controls**
- CDJ-style cue behavior (hold to preview, release to return)
- 8 hot cue points per deck
- Loop controls with adjustable length (1/4 to 16 beats)
- Beat jump forward/backward (uses loop length)
- Loop halve/double buttons with visual display
- Beat grid support from track metadata

**Mixer**
- 4-channel mixer with per-channel controls
- 3-band EQ per channel (low shelf, mid peak, high shelf with DJ-style kill)
- Trim, filter, and volume per channel
- Cue/headphone routing per channel
- Master and cue volume controls

**Effects**
- DJ Filter (combined high-pass/low-pass on single knob)
- Stereo Delay (tempo-syncable, with feedback and ping-pong mode)
- Reverb (Freeverb-style with room size, damping, and stereo width)
- Gain effect for volume adjustment
- Effect chain architecture with 8 mappable knobs per stem
- Bypass and parameter automation ready

**Audio Output**
- JACK audio client with 4 outputs (Master L/R, Cue L/R)
- Auto-connection to system playback
- Real-time priority processing

**User Interface**
- Dark theme optimized for live performance
- 4-deck grid layout with center file browser
- Transport controls (play, pause, cue, sync, loop)
- Hot cue buttons
- Stem tabs with per-stem mute/solo/volume controls
- Effect chain visualization with click-to-bypass toggles
- 8 mappable knobs per stem for real-time effect control
- Mixer section with EQ, filter, volume faders
- Global BPM control with slider

### In Progress 🚧

- Waveform display with beat markers and cue points
- Track loading via file browser UI
- Pitch/tempo fader connection
- Adding effects to stem chains via UI

### Planned 📋

**mesh-player**
- MIDI/HID controller mapping
- Keyboard shortcuts
- Quantized loops and hot cues
- Beat sync between decks
- Recording to file
- Pure Data effect patches
- RAVE neural effects integration

**mesh-cue** (In Development)
- Staging area for importing pre-separated stems (4 WAV files → 8-channel format)
- Collection browser for managing converted tracks
- Track editor with cue point management
- BPM and key detection using Essentia
- Beat grid generation and editing
- Export to mesh-player format with embedded metadata

*Planned:*
- Waveform display with beat markers
- JACK audio preview
- Playlist and crate management

---

## Getting Started

### Prerequisites

- Linux (tested on NixOS, should work on most distributions)
- JACK audio server
- Nix package manager (recommended) or Rust toolchain

### Building with Nix (Recommended)

```bash
# Clone the repository
git clone https://github.com/yourusername/mesh.git
cd mesh

# Enter the development shell
nix develop

# Build the project
cargo build --release

# Run mesh-player
cargo run -p mesh-player
```

### Building without Nix

You'll need to install the following dependencies:
- Rust 1.70+
- JACK development libraries
- Clang/LLVM (for bindgen)
- Wayland/X11 development libraries (for iced)

```bash
cargo build --release
```

### Running

1. Start JACK audio server:
   ```bash
   jackd -d alsa -r 44100
   ```
   Or use a JACK control application like QjackCtl or Cadence.

2. Run mesh-player (DJ application):
   ```bash
   cargo run -p mesh-player
   ```

3. Or run mesh-cue (track preparation):
   ```bash
   cargo run -p mesh-cue
   ```

---

## File Format

Mesh uses a custom stem file format based on WAV/RF64:

- **8 channels**: 4 stereo stems (L/R pairs for Vocals, Drums, Bass, Other)
- **Sample rate**: 44100 Hz
- **Bit depth**: 16-bit (24-bit and 32-bit float also supported)
- **Metadata**: Embedded in `bext` chunk with BPM, key, beat grid, and cue points

Example metadata format:
```
BPM:128.00|KEY:Am|GRID:0,22050,44100|ORIGINAL_BPM:125.00
```

The mesh-cue application converts pre-separated stems (from tools like Demucs or Ultimate Vocal Remover) into this format with automatic BPM/key analysis.

---

## Contributing

Contributions are welcome! Areas where help is especially appreciated:

- **Audio DSP** — More effects, better filters, EQ implementations
- **UI/UX** — Waveform rendering, better layouts, accessibility
- **Testing** — Integration tests, audio quality verification
- **Documentation** — Tutorials, API docs, video guides
- **Platform support** — macOS and Windows builds

Please open an issue to discuss major changes before submitting a PR.

---

## License

AGPL-3.0 — see [LICENSE](LICENSE) for details.

This project uses [Essentia](https://essentia.upf.edu/) which is licensed under AGPL-3.0, requiring this project to use the same license.

---

## Acknowledgments

- [signalsmith-stretch](https://signalsmith-audio.co.uk/code/stretch/) for high-quality time stretching
- [iced](https://iced.rs/) for the GUI framework
- [JACK](https://jackaudio.org/) for professional audio routing
- [RAVE](https://github.com/acids-ircam/RAVE) for neural audio synthesis
- [libpd](https://github.com/libpd/libpd) for Pure Data integration
- [Essentia](https://essentia.upf.edu/) for audio analysis (BPM, key, beat detection)

---

*Mesh is under active development. Star the repo to follow progress!*
