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
| **Memory Management** | basedrop | RT-safe deferred deallocation for audio buffers |
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

### Real-Time Safe Architecture

Professional audio requires **deterministic timing**. JACK gives us ~21ms at 1024 samples @ 48kHz to process each audio buffer. Any operation that takes longer causes an **xrun** (audio dropout).

Mesh implements a fully real-time safe architecture:

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Thread Architecture                             │
│                                                                     │
│   ┌─────────────┐     lock-free      ┌─────────────┐               │
│   │  UI Thread  │────────────────────│ JACK Thread │               │
│   │  (iced)     │    command queue   │  (RT audio) │               │
│   │             │                    │             │               │
│   │ • Load track│    LoadTrack ───►  │ • Process   │               │
│   │ • Play/Pause│    Play/Pause ───► │   audio     │               │
│   │ • Set BPM   │    SetPitch ────►  │ • No allocs │               │
│   │ • Effects   │                    │ • No locks  │               │
│   └─────────────┘                    └──────┬──────┘               │
│                                             │ drop old track       │
│                                             ▼                       │
│                                      ┌─────────────┐               │
│                                      │  GC Thread  │               │
│                                      │  (audio-gc) │               │
│                                      │             │               │
│                                      │ • Deferred  │               │
│                                      │   dealloc   │               │
│                                      │ • 100ms     │               │
│                                      │   cycle     │               │
│                                      └─────────────┘               │
└─────────────────────────────────────────────────────────────────────┘
```

**Key design decisions:**

| Problem | Solution | Implementation |
|---------|----------|----------------|
| UI-to-audio communication | Lock-free SPSC queue | `mesh-core/src/engine/command.rs` |
| Large buffer sharing | Zero-copy via `Shared<T>` | 452MB stem buffers shared, not cloned |
| Memory deallocation | Deferred to GC thread | `basedrop::Shared` + `mesh-core/src/engine/gc.rs` |
| Stem buffer allocation | Sequential with yields | Prevents page fault storms |

**Result:** Track loading while playing another track causes **zero audio dropouts**.

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
- **Real-time safe architecture** — Lock-free command queue, zero-copy buffer sharing, deferred deallocation via basedrop
- **Zero xruns during track loading** — Load new tracks while playing without audio dropouts

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

**mesh-cue** (Working MVP)
- Staging area for importing pre-separated stems (4 WAV files → 8-channel format)
- BPM detection using Essentia's RhythmExtractor2013 algorithm
- Key detection using Essentia's KeyExtractor with EDMA profile (optimized for EDM)
- Beat grid generation from detected beat positions
- Export to 8-channel WAV with embedded metadata (bext chunk)
- Add to collection with automatic metadata embedding
- **Global configuration service** with YAML persistence
- **Settings modal** (gear icon) for configuring analysis parameters
- **Configurable BPM range** for genre-specific detection (e.g., DnB: 160-190 BPM)
- **Interactive waveform display** with 4-stem color coding, beat grid overlay, and cue markers
- **Downbeat highlighting** — First beat of each bar displayed in red for visual bar counting
- **Click-to-seek** on waveform with drag scrubbing support
- **CDJ-style transport controls** — Play/pause toggle, cue button with beat grid snap
- **Beat jump navigation** — Skip forward/backward by configurable beat count (1, 4, 8, 16, 32)
- **8 hot cue action buttons** — Click to jump, click empty slot to set, colored by index
- Track editor with cue point management
- **Save edited track metadata** (BPM, key, cue points) back to file
- **JACK audio preview** with click-to-seek waveform synchronization
- **Async track loading** — Instant UI response with background audio loading
- **Track name auto-fill** — Parses artist/name from stem filenames (e.g., "Artist - Track (Vocals).wav")
- **Configurable track name format** — Template with {artist} and {name} placeholders

**Collection Browser** (New!)
- **Dual-panel browser** — Two side-by-side playlist browsers for efficient track organization
- **Hierarchical tree navigation** — Collapsible folder tree with General Collection and Playlists sections
- **Track table with metadata** — Displays Name, Artist, BPM, Key, and Duration columns
- **Search and sort** — Filter tracks by name, click column headers to sort
- **Inline metadata editing** — Double-click Artist, BPM, or Key cells to edit directly (changes saved to file)
- **Drag and drop** — Drag tracks from table onto playlist folders in tree
- **Double-click to load** — Load tracks into editor for detailed editing
- **Playlist management** — Create, rename, and delete playlists (symlink-based storage)

**Batch Import System** (New!)
- **Automated stem import** — Drop stems into import folder, batch process with one click
- **Parallel processing** — 4-worker thread pool for fast analysis (BPM, key, beat grid)
- **Progress tracking** — Real-time progress bar with ETA at bottom of collection view
- **Stem grouping** — Automatically groups stems by track name (e.g., `Artist - Track_(Vocals).wav`)
- **Source cleanup** — Optionally deletes source stems after successful import
- **Results summary** — Shows success/failure count with detailed error messages

*Planned:*
- Smart playlists with auto-filtering

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
- **Metadata**: Embedded in `bext` chunk with artist, BPM, key, beat grid, and cue points

Example metadata format:
```
ARTIST:Daft Punk|BPM:128.00|KEY:Am|FIRST_BEAT:14335|ORIGINAL_BPM:125.00
```

| Field | Description |
|-------|-------------|
| `ARTIST` | Artist name (optional) |
| `BPM` | Current tempo in beats per minute |
| `KEY` | Musical key (e.g., Am, C#m, Gb) |
| `FIRST_BEAT` | Sample position of first beat (beat grid regenerated from BPM) |
| `ORIGINAL_BPM` | Original detected tempo before any adjustments |

The mesh-cue application converts pre-separated stems (from tools like Demucs or Ultimate Vocal Remover) into this format with automatic BPM/key analysis.

---

## Configuration

mesh-cue stores its configuration in YAML format alongside your collection:

```
~/Music/mesh-collection/config.yaml
```

### Settings

Click the **⚙** gear icon in the header to open the settings modal.

**Analysis → BPM Detection Range**

Configure the expected tempo range for your music genre:

| Genre | Min Tempo | Max Tempo |
|-------|-----------|-----------|
| House/Techno | 120 | 135 |
| DnB/Jungle | 160 | 190 |
| Dubstep | 70 | 75 (or 140-150 for double-time) |
| Hip-Hop | 80 | 115 |
| Default | 40 | 208 |

Setting a narrower range prevents half-tempo or double-tempo detection errors (e.g., DnB at 172 BPM being detected as 86 BPM).

**Import → Track Name Format**

Configure the template for auto-filling track names when importing stems:

| Tag | Description |
|-----|-------------|
| `{artist}` | Artist name parsed from filename |
| `{name}` | Track name parsed from filename |

Example: `{artist} - {name}` → "Daft Punk - One More Time"

Example `config.yaml`:
```yaml
analysis:
  bpm:
    min_tempo: 160
    max_tempo: 190
track_name_format: "{artist} - {name}"
```

---

## Using the Collection Browser

The Collection Browser provides a dual-panel interface for organizing and editing your track library.

### Layout

```
┌─────────────────────────────────────────────────────────────────┐
│                      Track Editor (top)                          │
├─────────────────────────────────┬───────────────────────────────┤
│     Left Browser                │       Right Browser            │
│  ┌──────────┬────────────────┐  │  ┌──────────┬────────────────┐ │
│  │  Tree    │  Track Table   │  │  │  Tree    │  Track Table   │ │
│  │ ▼ General│ Name  BPM Key  │  │  │ ▼ General│ Name  BPM Key  │ │
│  │   tracks │ Song1 128 Am   │  │  │   tracks │ Song5 140 Cm   │ │
│  │ ▼ Playlis│ Song2 140 Dm   │  │  │ ▼ Playlis│ Song6 128 Fm   │ │
│  │   Set 1  │ Song3 174 Em   │  │  │   Set 2  │                │ │
│  └──────────┴────────────────┘  │  └──────────┴────────────────┘ │
└─────────────────────────────────┴───────────────────────────────┘
```

### Quick Actions

| Action | How To |
|--------|--------|
| **Load track** | Double-click a track in the table |
| **Navigate folders** | Click folder in tree to show contents |
| **Expand/collapse** | Click ▶/▼ arrow next to folder |
| **Edit metadata** | Double-click Artist, BPM, or Key cell |
| **Save edit** | Press Enter |
| **Cancel edit** | Press Escape or click away |
| **Search tracks** | Type in search box above table |
| **Sort by column** | Click column header (▲/▼ indicates direction) |
| **Create playlist** | Right-click on Playlists folder |
| **Add to playlist** | Drag track from table onto playlist in tree |

### Inline Metadata Editing

You can edit track metadata directly in the browser without loading the track:

1. **Double-click** on an editable cell (Artist, BPM, or Key)
2. The cell transforms into a text input
3. **Type** the new value
4. Press **Enter** to save (writes directly to the WAV file's bext chunk)
5. Press **Escape** to cancel

**Note:** Name and Duration columns are read-only. Duration is calculated from the audio file, and Name is derived from the filename.

---

## Batch Import

The Batch Import system allows you to quickly import multiple tracks at once. Instead of manually loading stems one by one, you can drop all your pre-separated stems into a folder and import them in batch.

### Import Folder Location

```
~/Music/mesh-collection/import/
```

Place your stem files here before importing. The folder is automatically created when you first run mesh-cue.

### Stem File Naming

Stems must follow this naming pattern:

```
BaseName_(StemType).wav
```

| Stem Type | Example Filename |
|-----------|------------------|
| Vocals | `Daft Punk - One More Time_(Vocals).wav` |
| Drums | `Daft Punk - One More Time_(Drums).wav` |
| Bass | `Daft Punk - One More Time_(Bass).wav` |
| Other | `Daft Punk - One More Time_(Other).wav` |

**Note:** `_(Instrumental).wav` is also accepted as an alias for `_(Other).wav`.

The `BaseName` can be anything — typically `Artist - Track` format. Stems with the same base name are automatically grouped together.

### Import Workflow

1. **Prepare stems** — Use a stem separation tool (Demucs, Ultimate Vocal Remover, etc.) to split your tracks into 4 stems

2. **Copy to import folder**:
   ```bash
   cp *_(Vocals).wav *_(Drums).wav *_(Bass).wav *_(Other).wav ~/Music/mesh-collection/import/
   ```

3. **Open mesh-cue** and click the **Import** button (above the playlist browsers)

4. **Review detected tracks** — The modal shows all detected track groups with completion status:
   - ✓ = All 4 stems present (ready to import)
   - 2/4 = Missing stems (will be skipped)

5. **Click "Start Import"** — Tracks are processed in parallel:
   - Stems are loaded and combined
   - BPM, key, and beat grid are analyzed
   - 8-channel WAV is exported with embedded metadata
   - Original stems are deleted on success

6. **View results** — A summary popup shows successful and failed imports

### Progress Bar

During import, a progress bar appears at the bottom of the collection view showing:
- Current track being processed
- Progress (X/Y completed)
- Estimated time remaining

You can continue browsing your collection while the import runs in the background.

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
- [basedrop](https://github.com/glowcoil/basedrop) for RT-safe memory management
- [RAVE](https://github.com/acids-ircam/RAVE) for neural audio synthesis
- [libpd](https://github.com/libpd/libpd) for Pure Data integration
- [Essentia](https://essentia.upf.edu/) for audio analysis (BPM, key, beat detection)

---

*Mesh is under active development. Star the repo to follow progress!*
