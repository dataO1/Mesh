# Mesh

**A modern DJ software suite built in Rust with stem-based mixing and neural audio effects.**

Mesh is an open-source DJ application designed for live performance with a focus on stem separation, real-time audio processing, and creative sound manipulation through neural networks.

---

## Quick Start for DJs

**New to Mesh?** Here's what you need to know:

### 1. Prepare Your Tracks (mesh-cue)

Mesh supports **two ways** to prepare your tracks:

#### Option A: Import Mixed Audio (Automatic Separation)

Drop any audio file into the import folder and mesh-cue will **automatically separate it into stems**:

```
~/Music/mesh-collection/import/
├── Daft Punk - One More Time.mp3
├── Justice - Genesis.flac
└── Deadmau5 - Strobe.wav
```

Click **Import** → Select **"Mixed Audio"** mode → Tracks are separated (Vocals, Drums, Bass, Other), analyzed (BPM, key, beats), and combined into the stem format.

> **Note:** Stem separation requires ~4GB RAM per track and may take several minutes. Uses the Demucs neural network model.

#### Option B: Import Pre-Separated Stems

If you've already separated your tracks using [Demucs](https://github.com/facebookresearch/demucs), [Ultimate Vocal Remover](https://ultimatevocalremover.com/), or similar tools:

```
~/Music/mesh-collection/import/
├── Artist - Track_(Vocals).wav
├── Artist - Track_(Drums).wav
├── Artist - Track_(Bass).wav
└── Artist - Track_(Other).wav
```

Click **Import** → Select **"Pre-separated Stems"** mode → Tracks are analyzed and combined.

### 2. Play Your Set (mesh-player)

Launch mesh-player, load tracks onto any of the 4 decks, and start mixing:

| Feature | What It Does |
|---------|--------------|
| **Stem Mute/Solo** | Mute the vocals, solo the drums — full control over each element |
| **Auto Beat Sync** | Tracks automatically phase-lock when you press play |
| **Auto Key Match** | Enable KEY button to harmonically match tracks |
| **Stem Slicer** | Remix on the fly by rearranging beats and phrases |
| **Find Similar** | Discover tracks with similar energy and vibe (NEW) |

### 3. Export to USB

Going to a gig without your laptop? Export playlists to a USB drive:
- Incremental sync (only copies changed files)
- Works with ext4, exFAT, and FAT32
- Full playlist structure preserved

### What You'll Need

- **Linux** (NixOS recommended, other distros work too)
- **JACK audio server** (for low-latency audio)
- **Stem-separated tracks** (mesh-cue converts them to the 8-channel format)

---

## Installation

### AppImage (Recommended for most Linux users)

Download the latest release from [GitHub Releases](https://github.com/yourusername/mesh/releases):

```bash
# Download (replace with actual URL from releases page)
wget https://github.com/yourusername/mesh/releases/latest/download/mesh-player-x86_64.AppImage
wget https://github.com/yourusername/mesh/releases/latest/download/mesh-cue-x86_64.AppImage

# Make executable
chmod +x mesh-player-x86_64.AppImage mesh-cue-x86_64.AppImage

# Run
./mesh-player-x86_64.AppImage
./mesh-cue-x86_64.AppImage
```

The AppImage bundles all dependencies and automatically detects whether you're using **PipeWire** (Ubuntu 22.04+, Fedora 34+) or **JACK** and configures audio accordingly.

**Requirements:**
- PipeWire (modern distros) or JACK2 audio server
- GPU drivers for Vulkan (optional — falls back to software rendering)

### NixOS / Nix

```bash
# Run directly without installing
nix run github:yourusername/mesh#mesh-player
nix run github:yourusername/mesh#mesh-cue

# Or install to your profile
nix profile install github:yourusername/mesh#mesh-player
nix profile install github:yourusername/mesh#mesh-cue
```

### Building from Source

See [Getting Started](#getting-started) for build instructions.

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

### Key Highlights

| Category | Features |
|----------|----------|
| **Performance** | Instant track loading at any library size, zero audio dropouts during loading |
| **Mixing** | 4 decks, auto beat sync, auto key matching, 3-band EQ with kill |
| **Creative** | Per-stem effects, stem slicer, stem linking for mashups |
| **Discovery** | Find similar tracks by audio fingerprint, harmonic mixing suggestions |
| **Preparation** | Batch import, BPM/key detection, beat grid editing, USB export |

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
| **MIDI I/O** | midir | Cross-platform MIDI input/output |

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
│   ├── mesh-midi/       # MIDI controller support
│   │   ├── config.rs    # YAML profile schema
│   │   ├── input.rs     # MIDI input handling
│   │   └── mapping.rs   # Control-to-action mapping
│   └── mesh-cue/        # Track preparation app
├── midi/                # MIDI controller profiles
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
- **Automatic beat sync** — Tracks automatically phase-align when playing (see [Auto Beat Sync](#automatic-beat-sync))
- **Automatic key matching** — Per-deck pitch transposition to match the master deck's key (see [Key Matching](#automatic-key-matching))
- **Stem Slicer** — Real-time audio remixing by rearranging slice playback order (see [Stem Slicer](#stem-slicer))

**Mixer**
- 4-channel mixer with per-channel controls
- 3-band EQ per channel (low shelf, mid peak, high shelf with DJ-style kill)
- Trim, filter, and volume per channel
- Cue/headphone routing per channel
- Master and cue volume controls
- **Auto-gain based on LUFS** — Tracks are loudness-normalized during import to -14 LUFS (configurable), so all tracks play at consistent volume without manual trim adjustment

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

**MIDI Controller Support**
- Configurable MIDI mapping via YAML profiles
- Support for Note On/Off and Control Change messages
- Layer toggle mode for 2-deck controllers accessing 4 virtual decks
- Auto value normalization (MIDI 0-127 → control ranges)
- MIDI shift button for secondary functions
- LED feedback output with change detection
- Included profile: Pioneer DDJ-SB2

### In Progress 🚧

- Waveform display with beat markers and cue points
- Track loading via file browser UI
- Pitch/tempo fader connection
- Adding effects to stem chains via UI

### Planned 📋

**mesh-player**
- Keyboard shortcuts
- Recording to file
- Pure Data effect patches
- RAVE neural effects integration

**mesh-cue** (Working MVP)
- Batch import system for pre-separated stems (4 WAV files → 8-channel format)
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

### Audio Backend Feature Flags

Mesh supports multiple audio backends depending on your platform:

| Feature | Platform | Description |
|---------|----------|-------------|
| `jack-backend` | Linux | Native JACK with full port-level routing **(default)** |
| (none) | Windows/macOS | CPAL backend (WASAPI on Windows, CoreAudio on macOS) |

**Linux with JACK (default):**

The `jack-backend` feature is enabled by default and provides direct JACK integration with port-level control. This is essential for pro-audio setups where you need to route master and cue to different physical outputs (e.g., Scarlett 18i20 outputs 1-2 vs 3-4).

```bash
# JACK backend is used by default on Linux
cargo run -p mesh-player
```

**Cross-platform (CPAL backend):**

For CPAL-only builds (used for Windows and macOS), disable the default features:

```bash
# Build without JACK (CPAL backend)
cargo run -p mesh-player --no-default-features
```

> **Note:** When using PipeWire with JACK compatibility, run mesh-player under `pw-jack`:
> ```bash
> pw-jack cargo run -p mesh-player
> ```

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

1. Start JACK audio server (48kHz recommended):
   ```bash
   jackd -d alsa -r 48000
   ```
   Or use a JACK control application like QjackCtl or Cadence.

   > **Note:** Mesh stores tracks at 48kHz internally. JACK can run at any sample rate (44.1kHz, 48kHz, 96kHz, etc.) — tracks are automatically resampled during loading to match JACK's rate.

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
- **Sample rate**: 48000 Hz (stems are resampled during import if needed)
- **Bit depth**: 16-bit (24-bit and 32-bit float also supported for input)
- **Metadata**: Embedded in `bext` chunk with artist, BPM, key, beat grid, and cue points

> **Sample Rate Handling:** When importing stems (e.g., from Demucs at 44.1kHz), mesh-cue automatically resamples them to 48kHz using a high-quality FFT-based resampler. This ensures consistent playback speed regardless of the source material's sample rate.

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

### mesh-player Theme

mesh-player supports customizable stem colors via a theme configuration file:

```
~/.config/mesh-player/theme.yaml
```

This allows you to personalize the waveform display colors for each stem type.

Example `theme.yaml`:
```yaml
stems:
  vocals: "#33CC66"   # Green
  drums: "#CC3333"    # Dark Red
  bass: "#E6604D"     # Orange-Red
  other: "#00CCCC"    # Cyan
```

| Stem | Default Color | Hex Code |
|------|---------------|----------|
| Vocals | Green | `#33CC66` |
| Drums | Dark Red | `#CC3333` |
| Bass | Orange-Red | `#E6604D` |
| Other | Cyan | `#00CCCC` |

Colors use standard hex format (`#RRGGBB`). Changes take effect on next launch.

---

## Automatic Beat Sync

Mesh includes **automatic inter-deck phase synchronization** — when you start playing a second track while another is already playing, the beats automatically align. No manual nudging or sync buttons required.

### How It Works

1. **Master Deck**: The deck that has been playing the longest is automatically the "master" (shown with a green dot on its waveform)
2. **Phase Lock on Play**: When you press play on another deck, it snaps to match the master's beat phase
3. **Phase Lock on Hot Cues**: Jumping to a hot cue while playing re-syncs to the master's current phase
4. **Automatic Handoff**: If the master deck stops, the next longest-playing deck becomes the new master
5. **Drift-Free Playback**: Fractional sample accumulation ensures tracks stay perfectly aligned indefinitely

### Example

```
Deck A: Playing for 2 minutes (MASTER - green dot)
        Currently 200 samples past beat 47

Deck B: You press PLAY
        Cued at beat 12

        → Automatically jumps to 200 samples past beat 12
        → Both decks' beats now land at exactly the same time
```

### What This Means for DJing

- **No beatmatching required** — Just press play and the tracks are in sync
- **Hot cues stay in phase** — Jump around the track without losing sync
- **Seamless transitions** — Focus on the creative mix, not the technical alignment
- **Works with any tempo** — All tracks are time-stretched to the global BPM
- **Zero drift** — Tracks stay phase-locked for hours without any cumulative timing errors

### Technical Note: Drift-Free Time Stretching

When time-stretching audio to match a global BPM, the ideal number of samples to read each frame is often fractional (e.g., 254.54 samples). Simply rounding this value each frame would cause ~1 second of drift per 10 minutes of playback.

Mesh solves this using **fractional sample accumulation**: the remainder from each frame is carried forward and eventually "catches up," ensuring mathematically perfect sync over any duration.

### Configuration

Beat sync can be toggled on/off in **Settings → Playback → Automatic Beat Sync**. When disabled, tracks play from their exact cued position without phase adjustment.

> **Note:** This feature requires tracks to have beat grids. mesh-cue automatically generates beat grids during import using Essentia's beat detection.

---

## Automatic Key Matching

Mesh includes **automatic key matching** — when enabled per-deck, tracks are automatically pitch-shifted to harmonically match the master deck's musical key.

### How It Works

1. **Per-Deck Toggle**: Each deck has a KEY button next to SLIP to enable/disable key matching
2. **Master Deck Reference**: The longest-playing deck is the master (no transpose applied to it)
3. **Automatic Transposition**: Slave decks with key matching enabled are pitch-shifted to match the master's key
4. **Relative Key Detection**: Compatible keys (Am ↔ C major) are detected — no transpose needed
5. **Real-Time Updates**: When the master deck changes, all slave decks automatically re-transpose

### Waveform Header Display

The waveform header shows the current key matching status:

| Display | Meaning |
|---------|---------|
| `Am` | Key matching disabled, or this is the master deck |
| `Am ✓` | Key match enabled, keys are compatible (no transpose needed) |
| `Am → +2` | Key match enabled, transposing +2 semitones |
| `Am → -5` | Key match enabled, transposing -5 semitones |

### Music Theory

Mesh uses the **Camelot Wheel** system for key compatibility:

- **Same key**: Am → Am = no transpose needed
- **Relative keys**: Am → C (relative major) = no transpose needed
- **Different keys**: Am → Em = transpose by -5 semitones (or +7)

Transposition always uses the **smallest interval** (±6 semitones max) to minimize pitch artifacts.

### What This Means for DJing

- **Harmonic mixing made easy** — Enable KEY and tracks blend harmonically
- **No manual pitch shifting** — The system calculates optimal transposition
- **Relative keys respected** — Am and C major are treated as compatible
- **Per-deck control** — Enable on slave decks, disable on master
- **Visual feedback** — See transpose amount in the waveform header

### Technical Details

Key matching uses [signalsmith-stretch](https://signalsmith-audio.co.uk/code/stretch/)'s `set_transpose_factor_semitones()` for high-quality pitch shifting without tempo change. This is applied in the audio engine's real-time processing loop alongside time stretching.

> **Note:** This feature requires tracks to have key metadata. mesh-cue automatically detects keys during import using Essentia's KeyExtractor.

---

## Track Discovery (Find Similar)

Mesh analyzes your tracks to create **audio fingerprints** — 16-dimensional vectors that capture rhythm, harmony, energy, and timbre. This enables intelligent track discovery:

### Audio Fingerprint

Each track is analyzed to extract these characteristics:

| Dimension | What It Captures | Why It Matters |
|-----------|------------------|----------------|
| **Rhythm** (4) | BPM, beat strength, regularity | Matches groove and danceability |
| **Harmony** (4) | Key, mode (major/minor), complexity | Finds harmonically compatible tracks |
| **Energy** (4) | Loudness (LUFS), dynamic range | Matches intensity and energy flow |
| **Timbre** (4) | Spectral character, brightness | Matches sonic texture and "feel" |

### Finding Similar Tracks

From the track context menu or sidebar, click **Find Similar** to discover:

- Tracks with similar energy levels and rhythmic feel
- Harmonically compatible options for smooth transitions
- Sonically cohesive sets that flow naturally

Results are ranked by **cosine similarity** — tracks that share similar fingerprints appear first.

### Mix Suggestions

Mesh can suggest what to play next based on:

| Suggestion Type | Description |
|-----------------|-------------|
| **Similar Energy** | Maintain the current vibe |
| **Build Up** | Tracks with higher energy for peak moments |
| **Cool Down** | Lower energy tracks for winding down |
| **Harmonic Match** | Tracks in compatible keys (Camelot wheel) |

### Technical Details

- Fingerprints are computed during import using [Essentia](https://essentia.upf.edu/) audio analysis algorithms
- Stored in a **HNSW vector index** (CozoDB) for instant similarity search (<5ms for 10K tracks)
- All analysis runs in isolated subprocesses (Essentia's C++ library isn't thread-safe)

> **Note:** This feature requires tracks to be imported through mesh-cue. Legacy tracks can be re-analyzed via the context menu.

---

## Stem Slicer

Mesh includes a **Stem Slicer** — a real-time remixing tool that divides stems into slices and lets you rearrange their playback order on the fly. Features include velocity control for ghost notes, layered slices, per-stem presets, and smooth muted transitions.

### How It Works

1. **Buffer Window**: A configurable window (1/4/8/16 bars) is divided into **16 equal slices**
2. **Step Sequence**: Each step can play up to 2 slices simultaneously with independent velocities
3. **Beat-Aligned**: Slices snap to the track's beat grid for musical timing
4. **Per-Stem Presets**: Each preset defines different patterns per stem — one button, coordinated remix
5. **Muted Steps**: Steps can be silent with automatic release fade to avoid clicks

### Using the Slicer

1. Click **SLICER** button on a deck to enter slicer mode (activates on next beat)
2. **Button 1-8**: Load preset pattern for all affected stems simultaneously
3. **Shift + Button**: Assign slice to current timing slot + preview it immediately
4. **Shift + Slicer**: Reset queue to default [0,1,2...15]
5. Click **HOTCUE** to exit slicer mode

### New Features

| Feature | Description |
|---------|-------------|
| **Velocity per step** | Control dynamics — full hits at 100%, ghost notes at 30% |
| **Layered slices** | Play 2 slices simultaneously (e.g., kick + hi-hat on same beat) |
| **Per-stem presets** | One button loads different patterns per stem (drums get half-time, bass gets stutter) |
| **Muted steps** | Create rhythmic gaps with smooth 1/4-slice release fade |

### Presets

Default presets apply patterns to drums only. Custom presets can define different patterns per stem:

| Button | Name | Effect |
|--------|------|--------|
| 1 | Sequential | Normal playback (reset) |
| 2 | Half-time | Double each slice |
| 3 | Kick Emphasis | Repeat kick hits |
| 4 | Snare Roll | Snare stutters |
| 5 | Shuffle | Syncopated pattern |
| 6 | Full Reverse | Backwards playback |
| 7 | Stutter | Rapid repeats |
| 8 | Rapid Fire | Quarter-note jumps |

Presets are configurable in `~/.config/mesh-player/config.yaml`.

### Configuration

In **Settings → Slicer**:

| Setting | Options | Description |
|---------|---------|-------------|
| Buffer Size | 1, 4, 8, 16 bars | Size of the sliced window (always 16 slices) |
| Affected Stems | Vocals, Drums, Bass, Other | Which stems receive preset patterns |

### Example

With a 4-bar buffer at 174 BPM:
- Each slice = 1 beat (4 bars ÷ 16 slices)
- **Button 2**: Load "Half-time" — drums play half-speed pattern, other stems bypass
- **Shift + Button 5**: Assign slice 5 to current timing position + hear it immediately
- **Shift + Slicer**: Reset everything back to normal sequential playback

### What This Means for DJing

- **Ghost notes** — Add subtle snare hits at low velocity for groove variation
- **Layered hits** — Stack kick + percussion on the same step for impact
- **Coordinated presets** — One button activates drums half-time + bass stutter together
- **Clean gaps** — Muted steps fade out smoothly instead of clicking
- **Create live remixes** — Rearrange drum patterns, vocal phrases, or bass lines
- **Build tension** — Use reverse or stutter presets for buildups

---

## Stem Linking

Mesh includes **Stem Linking** — a mashup feature that lets you swap individual stems between tracks. Replace the vocals from one track with vocals from another, keep the drums from your current track but bring in the bass from a different song, or create entirely new combinations on the fly.

### What is Stem Linking?

Traditional DJing mixes two tracks together. Stem linking goes further by letting you **mix individual stems** from different tracks:

```
Track A (Playing):        Track B (Linked):
├── Vocals ◄──────────────── Vocals (swapped!)
├── Drums  (original)
├── Bass   (original)
└── Other  (original)
```

When you toggle a linked stem active, the audio and waveform instantly swap to show the linked track's stem.

### How It Works

1. **Preparation in mesh-cue**: Open your host track and use the stem link buttons to assign stems from other tracks
2. **Automatic loading**: When you load the track in mesh-player, linked stems are pre-loaded in the background
3. **Toggle to swap**: Press **Shift + Stem Mute** button to swap between original and linked audio
4. **Visual feedback**: The waveform updates to show the linked stem's peaks when active

### Drop Marker Alignment

Stem links use **drop markers** to align tracks structurally, not just by tempo:

```
Host Track:     [Intro]──────[Build]──────[DROP]──────[Break]
                                             │
Linked Track:   [Intro]──[Build]─────────[DROP]──────[Breakdown]
                                             │
                                    ← Aligned here
```

When you're at the drop in your host track, the linked stem plays from its drop — even if the tracks have different arrangements. This keeps the energy aligned.

### Preparing Stem Links (mesh-cue)

1. **Open your host track** in mesh-cue
2. **Set a drop marker** (if not already set) — this is your alignment reference point
3. **Click a stem link button** (below the hot cues) to enter selection mode
4. **Browse and select** the source track for that stem
5. **Save the track** — stem links are stored in the WAV file's `mslk` chunk

### Using Stem Links (mesh-player)

| Action | How To |
|--------|--------|
| **Toggle linked stem** | Shift + Stem Mute button |
| **Visual indicator** | Waveform shows linked stem's peaks when active |
| **Load prepared links** | Automatic — links load when the track loads |

### Creative Possibilities

- **Vocal swaps** — Put acapella vocals over any instrumental
- **Drum replacements** — Swap in punchier drums from a different track
- **Mashup construction** — Build unique stem combinations in mesh-cue, perform in mesh-player
- **A/B comparison** — Toggle between original and linked to compare during prep

### Technical Notes

- **Pre-stretched**: Linked stems are time-stretched to match the host track's BPM when loaded
- **Pre-computed waveforms**: Linked stem waveform peaks are extracted from the source track's metadata (no runtime computation)
- **Memory efficient**: Only the stems you link are loaded, not the entire source track

---

## MIDI Controller Support

Mesh supports MIDI controllers for hands-on DJ performance. Controllers are configured via YAML profiles stored in `~/.config/mesh-player/midi.yaml`.

### Supported Features

| Feature | Description |
|---------|-------------|
| **Note On/Off** | Buttons, pads, transport controls |
| **Control Change** | Faders, knobs, encoders |
| **Layer Toggle** | 2-deck controllers can access 4 virtual decks |
| **Shift Button** | Secondary functions via MIDI shift |
| **LED Feedback** | Controller LEDs reflect deck state |
| **Value Normalization** | MIDI 0-127 auto-maps to control ranges |

### Layer Toggle Mode

For 2-deck controllers like the Pioneer DDJ-SB2, layer toggle lets each physical deck control two virtual decks:

```
Physical Deck 1 ──► Layer A: Deck 1  │  Layer B: Deck 3
Physical Deck 2 ──► Layer A: Deck 2  │  Layer B: Deck 4

[DECK 1/3 toggle] switches left side between Deck 1 and Deck 3
[DECK 2/4 toggle] switches right side between Deck 2 and Deck 4
```

Mixer controls (volume, EQ, filter) always map directly to channels 1-2, while transport and performance pads follow the layer selection.

### Configuration

Create or edit `~/.config/mesh-player/midi.yaml`:

```yaml
devices:
  - name: "My Controller"
    port_match: "Controller"  # Case-insensitive substring match

    shift:
      type: "Note"
      channel: 0
      note: 0x63

    mappings:
      - control: { type: "Note", channel: 0, note: 0x0B }
        action: "deck.play"
        physical_deck: 0

      - control: { type: "ControlChange", channel: 0, cc: 0x13 }
        action: "mixer.volume"
        deck_index: 0
        behavior: continuous
```

### MIDI Learn

The easiest way to configure a new MIDI controller is using **MIDI Learn mode** — a guided wizard that walks you through mapping every control on your device.

#### Starting MIDI Learn

1. Connect your MIDI controller
2. Launch mesh-player with the `--midi-learn` flag:
   ```bash
   cargo run -p mesh-player -- --midi-learn
   ```
3. Or click **MIDI Learn** in the Settings tab

#### The Learning Process

MIDI Learn guides you through mapping controls in a logical order:

| Phase | Controls Mapped |
|-------|-----------------|
| **Setup** | Controller name, deck count, shift button |
| **Transport** | Play, cue, loop, beat jump, mode buttons (per deck) |
| **Pads** | 8 hot cue pads (per deck) |
| **Stems** | 4 stem mute buttons (per deck) |
| **Mixer** | Volume, filter, EQ hi/mid/lo, cue button (per channel) |
| **Browser** | Scroll encoder, select button, master/cue volumes, load buttons |

For each control:
1. The UI highlights the target control with a **red border**
2. A prompt tells you what to press/move (e.g., "Press PLAY button on deck 1")
3. Press/move the control on your hardware
4. The mapping is captured automatically and you advance to the next control

#### Hardware Type Auto-Detection

MIDI Learn automatically detects what type of physical control you're using by analyzing the MIDI messages:

| Hardware Type | Detection Method | Auto-Configuration |
|---------------|------------------|-------------------|
| **Button** | Note On/Off messages | Momentary behavior |
| **Knob** | CC values with wide range, variable direction | Absolute mode |
| **Fader** | CC values with wide range, monotonic movement | Absolute mode |
| **Encoder** | CC values centered around 64, small range | Relative mode |
| **Jog Wheel** | Like encoder but high message rate (>15/sec) | Relative mode |
| **14-bit Fader** | CC pair (N and N+32) arriving within 5ms | High-resolution mode |

When you move a control, MIDI Learn samples the messages for about 1 second to determine the hardware type. The UI shows "Sampling... X samples (Y%)" during detection and "Detected: Knob" (or similar) when complete.

**Why this matters:**
- **Knob mapped to button action**: Automatically adds threshold conversion (value > 63 = pressed)
- **Encoder mapped to scroll**: Automatically uses relative mode instead of absolute
- **14-bit fader detected**: Automatically combines MSB + LSB for high resolution

#### Encoder Press Capture

When MIDI Learn detects an **encoder** (like a browse encoder), it automatically prompts you to capture the encoder's push/click function as a separate mapping:

1. Turn the encoder → Hardware detected as "Encoder"
2. Prompt appears: "Now PRESS the encoder (or skip if it doesn't click)"
3. Press the encoder → Captured as a button (e.g., `browser.select`)
4. Or click **Skip** if your encoder doesn't have a push function

This captures both the rotation (CC) and press (Note) on the same physical control as two separate mappings.

#### Tips for Best Results

- **Wait for the prompt** — There's a 1-second debounce between captures to prevent accidental double-mappings
- **Use Skip (→)** for controls your hardware doesn't have
- **Use Back (←)** to re-map a control if you pressed the wrong button
- **Mappings work live** — You can test your mappings while still in learn mode
- **Move the full range** — For knobs/faders, move from min to max during sampling for best detection

#### Saving Your Profile

When you complete all phases, click **Save** to write your mappings to:
```
~/.config/mesh-player/midi.yaml
```

Your controller is now ready to use! The profile includes LED feedback mappings that mirror button presses back to your controller's LEDs.

### Included Profiles

Device profiles are included in the `midi/` folder:

| Controller | File | Features |
|------------|------|----------|
| Pioneer DDJ-SB2 | `ddj-sb2.yaml` | Layer toggle, 8 hot cues, transport, mixer, LED feedback |

Copy a profile to `~/.config/mesh-player/midi.yaml` and adjust `port_match` to match your device.

### Available Actions

**Deck Actions** (use `physical_deck` for layer-resolved mapping):
- `deck.play` — Toggle play/pause
- `deck.cue_press` / `deck.cue_release` — CDJ-style cue
- `deck.hot_cue_press` / `deck.hot_cue_clear` — Hot cue with params: `{ slot: 0-7 }`
- `deck.loop_toggle` / `deck.loop_halve` / `deck.loop_double`
- `deck.load_selected` — Load selected browser track

**Mixer Actions** (use `deck_index` for direct channel mapping):
- `mixer.volume` / `mixer.filter` — Continuous 0-1
- `mixer.eq_hi` / `mixer.eq_mid` / `mixer.eq_lo` — EQ bands
- `mixer.cue_toggle` — Headphone cue

**Browser Actions**:
- `browser.scroll` — Encoder with `encoder_mode: relative`

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

The Batch Import system allows you to quickly import multiple tracks at once. Mesh supports two import modes:

| Mode | Input | Use When |
|------|-------|----------|
| **Mixed Audio** | Regular audio files (MP3, FLAC, WAV) | You have normal music files and want automatic stem separation |
| **Pre-separated Stems** | 4 WAV files per track | You've already separated stems using Demucs, UVR, etc. |

### Import Folder Location

```
~/Music/mesh-collection/import/
```

Place your files here before importing. The folder is automatically created when you first run mesh-cue.

### Mixed Audio Mode (Automatic Separation)

For regular audio files that need to be separated into stems:

1. **Copy audio files** to the import folder:
   ```bash
   cp "My Track.mp3" "Another Song.flac" ~/Music/mesh-collection/import/
   ```

2. **Open mesh-cue** and click **Import**

3. **Select "Mixed Audio"** mode using the toggle buttons

4. **Review detected files** — The modal shows all detected audio files (MP3, FLAC, WAV, OGG, M4A)

5. **Click "Start Import"** — Each track is processed sequentially:
   - Audio is loaded and decoded
   - **Neural stem separation** extracts Vocals, Drums, Bass, and Other
   - BPM, key, and beat grid are analyzed
   - 8-channel WAV is exported with embedded metadata
   - Original file is deleted on success

> **⚠ Resource Requirements:** Stem separation uses the Demucs neural network and requires approximately **4GB RAM per track**. Processing is sequential (one track at a time) to manage memory usage. Expect 2-5 minutes per track depending on length and CPU.

### Pre-separated Stems Mode

For stems you've already separated using external tools:

#### Stem File Naming

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

#### Import Workflow

1. **Prepare stems** — Use a stem separation tool (Demucs, Ultimate Vocal Remover, etc.) to split your tracks into 4 stems

2. **Copy to import folder**:
   ```bash
   cp *_(Vocals).wav *_(Drums).wav *_(Bass).wav *_(Other).wav ~/Music/mesh-collection/import/
   ```

3. **Open mesh-cue** and click the **Import** button

4. **Select "Pre-separated Stems"** mode (default)

5. **Review detected tracks** — The modal shows all detected track groups with completion status:
   - ✓ = All 4 stems present (ready to import)
   - 2/4 = Missing stems (will be skipped)

6. **Click "Start Import"** — Tracks are processed in parallel:
   - Stems are loaded and combined
   - BPM, key, and beat grid are analyzed
   - 8-channel WAV is exported with embedded metadata
   - Original stems are deleted on success

7. **View results** — A summary popup shows successful and failed imports

### Progress Bar

During import, a progress bar appears at the bottom of the collection view showing:
- Current track being processed (with separation % for mixed audio mode)
- Progress (X/Y completed)
- Estimated time remaining

You can continue browsing your collection while the import runs in the background.

---

## Stem Separation

> **⚠️ Experimental Feature:** Stem separation is experimental. Quality may vary depending on the source material. GPU acceleration (CUDA/DirectML) is untested and may not work on all systems.

Mesh includes built-in **neural stem separation** powered by [Demucs](https://github.com/facebookresearch/demucs) (Meta AI). When you import regular audio files, Mesh automatically separates them into 4 stems: **Vocals**, **Drums**, **Bass**, and **Other**.

### How It Works

The separation uses the **HTDemucs** (Hybrid Transformer Demucs) model — a state-of-the-art neural network that processes audio in both the time and frequency domains:

```
Input Audio ──► Time Branch ──────────┐
                                      ├──► Merged ──► 4 Stems
            ──► Frequency Branch ─────┘
                (STFT → Transformer → ISTFT)
```

This hybrid architecture captures both transient details (drums, percussion) and harmonic content (vocals, bass) with high accuracy.

### Quality Options

Configure separation quality in **Settings → Separation**:

| Setting | Options | Effect |
|---------|---------|--------|
| **Model** | Standard / Fine-tuned | Fine-tuned has ~1-3% better SDR (signal-to-distortion ratio) |
| **Shifts** | 1-5 | More shifts = better quality but slower. Each shift adds ~0.2 SDR improvement |

**Recommended settings:**
- **Quick preview**: 1 shift (fastest, good enough for auditioning)
- **Final library**: 3-5 shifts (best quality for your main collection)

### GPU Acceleration

Stem separation is computationally intensive. GPU acceleration dramatically reduces processing time:

| Hardware | Approximate Time (4-min track) |
|----------|-------------------------------|
| CPU (8-core) | 3-5 minutes |
| NVIDIA RTX 3070 | 20-30 seconds |
| NVIDIA RTX 4090 | 10-15 seconds |

#### Linux with NVIDIA GPU (CUDA)

For NVIDIA GPU acceleration on Linux, use the CUDA-enabled build:

```bash
# Build from source with CUDA support
nix run .#build-deb-cuda

# Or install the CUDA .deb package
sudo dpkg -i mesh-cue-cuda_*.deb
```

**Requirements:**
- NVIDIA GPU with CUDA Compute Capability 6.0+ (GTX 1000 series or newer)
- NVIDIA driver 525 or newer
- CUDA 12 toolkit installed on your system

**Verify CUDA is working:**
```bash
# Check NVIDIA driver
nvidia-smi

# Check CUDA toolkit
nvcc --version
```

If CUDA is not available at runtime, Mesh automatically falls back to CPU processing.

#### Windows with DirectX 12 GPU

Windows builds include **DirectML** support, which provides GPU acceleration for any DirectX 12 capable GPU (NVIDIA, AMD, or Intel):

- No additional drivers needed (DirectML is built into Windows 10+)
- Works automatically if a compatible GPU is detected
- Falls back to CPU if no GPU is available

### Resource Requirements

| Resource | Minimum | Recommended |
|----------|---------|-------------|
| **RAM** | 4 GB per track | 8 GB+ for comfortable headroom |
| **Disk** | ~50 MB per track (output) | SSD recommended for faster I/O |
| **Model Download** | ~170 MB (one-time) | Cached in `~/.cache/mesh-cue/models/` |

**Memory management:** Tracks are processed sequentially (one at a time) to manage memory usage. The model is loaded once and reused for all tracks in a batch.

### Separation Quality Tips

1. **Higher-quality source files** produce better separations. Prefer FLAC/WAV over low-bitrate MP3.

2. **Shifts setting** is the biggest quality lever. Use 3+ shifts for tracks you'll play frequently.

3. **Check the "other" stem** — it contains everything that isn't vocals, drums, or bass (guitars, synths, FX). If you hear unwanted bleed, the source audio may have challenging content.

4. **Residual computation** ensures stems sum back to the original mix perfectly. There's no energy loss or added artifacts.

---

## USB Export

Mesh supports exporting your playlists to USB drives for portable DJ setups. Export from mesh-cue, then browse and play from mesh-player — no laptop needed at the venue.

### Supported Filesystems

| Filesystem | Symlinks | Notes |
|------------|----------|-------|
| **ext4** | ✓ | Recommended for Linux-only setups. Space-efficient symlink playlists |
| **exFAT** | ✗ | Cross-platform. Tracks are copied to playlists |
| **FAT32** | ✗ | Maximum compatibility. 4GB file size limit |

### USB Directory Structure

When you export to a USB drive, Mesh creates this structure:

```
<usb_mount>/mesh-collection/
├── tracks/                    # Audio files
│   └── Artist - Track.wav
├── playlists/                 # Playlist folders
│   └── Live Set/
│       └── Artist - Track.wav   # Symlink (ext4) or copy (FAT/exFAT)
├── mesh-manifest.yaml         # SHA256 hashes for incremental sync
└── player-config.yaml         # Exported settings (optional)
```

### Exporting Playlists (mesh-cue)

1. **Connect your USB drive** — It will be detected automatically
2. **Click Export** (next to Import button in the collection browser header)
3. **Select your USB device** from the dropdown
4. **Check the playlists** you want to export
5. **Optionally enable "Include settings"** to export your audio/display configuration
6. **Click "Calculate Changes"** — Mesh computes which files need copying using SHA256 hashes
7. **Review the sync plan** — Shows files to copy, delete, and skip
8. **Click "Start Export"** — Files are copied with verification

### Incremental Sync

Mesh uses **SHA256 content hashing** for efficient sync:

- **First export**: All tracks are copied
- **Subsequent exports**: Only new/changed files are copied
- **Removed tracks**: Optionally deleted from USB
- **Unchanged files**: Skipped (instant)

The hash manifest (`mesh-manifest.yaml`) is stored on the USB drive.

### ext4 Permission Fix

If you're using an **ext4-formatted USB drive** and see "Permission denied", the drive's root directory is owned by root. Fix with:

```bash
sudo chown -R $USER /run/media/$USER/<your-usb-label>
```

This only needs to be done once per drive — the ownership is stored on the USB filesystem.

**Why does this happen?** ext4 stores Unix permissions. When you format a drive as ext4, the root directory is created as root. FAT32 and exFAT don't have this issue because they use mount options for access control.

### Browsing USB in mesh-player

mesh-player automatically detects connected USB drives with a `mesh-collection` folder:

1. **Connect your USB** — It appears in the browser tree under "USB Devices"
2. **Browse playlists** — Navigate just like local playlists
3. **Load tracks** — Double-click to load onto a deck
4. **Hot-plug support** — USB devices can be connected/disconnected while running

### Exporting Settings

When "Include settings" is enabled, these settings are exported:

| Setting | Included |
|---------|----------|
| Global BPM | ✓ |
| Phase sync enabled | ✓ |
| Loudness normalization | ✓ |
| Default loop length | ✓ |
| Zoom/grid bars | ✓ |
| Slicer buffer size | ✓ |
| MIDI mappings | ✗ (hardware-specific) |

Settings are stored in `player-config.yaml` on the USB and can be loaded by mesh-player.

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
- [midir](https://github.com/Boddlnagg/midir) for cross-platform MIDI I/O

---

*Mesh is under active development. Star the repo to follow progress!*
