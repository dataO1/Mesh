# Mesh

**Open-source DJ software with stem-based mixing and neural audio separation.**

Mesh lets you mix music by controlling individual elements (vocals, drums, bass, instruments) independently. Import any audio file and Mesh automatically separates it into stems using AI — no external tools required.

---

## Overview

Mesh is a DJ software suite with two applications:

| Application | Purpose |
|-------------|---------|
| **mesh-cue** | Prepare your music library — import tracks, separate stems, analyze BPM/key, edit beat grids, organize playlists |
| **mesh-player** | Perform live — 4-deck mixing with stem control, beat sync, effects, and MIDI controller support |

### Why Mesh?

Traditional DJ software mixes complete stereo tracks. Mesh gives you control over **each element**:

- Mute the vocals for an instrumental breakdown
- Solo the drums during a transition
- Swap the bassline from one track with another
- Apply effects to individual stems

Mesh also includes **built-in stem separation** — drop any MP3, FLAC, or WAV file and it's automatically split into 4 stems using the Demucs neural network.

---

## Features

### Track Preparation (mesh-cue)

- **Automatic stem separation** — Import regular audio files; AI separates into Vocals, Drums, Bass, and Other
- **BPM and key detection** — Automatic analysis with configurable tempo ranges for genre-specific accuracy
- **Beat grid editing** — Fine-tune beat markers, set downbeats, adjust BPM with visual feedback
- **8 hot cues per track** — Set, edit, and color-code cue points
- **Stem linking** — Prepare mashups by linking stems from different tracks (e.g., vocals from track A over instrumentals from track B)
- **ML-enhanced audio analysis** — Automatic genre classification and mood tagging using neural network models (EffNet + Discogs400). Vocal/instrumental detection via stem energy analysis. Optional arousal/valence estimation for smarter energy-aware suggestions
- **Playlist management** — Organize tracks into playlists with drag-and-drop
- **USB export** — Sync playlists to USB drives for portable performance

### Live Performance (mesh-player)

- **4-deck architecture** — Load and mix up to 4 tracks simultaneously
- **Per-stem control** — Mute, solo, and adjust volume for each stem independently
- **Automatic beat sync** — Tracks phase-lock automatically when you press play
- **Automatic key matching** — Pitch-shift tracks to match harmonically (Camelot wheel)
- **Stem slicer** — Real-time remixing by rearranging slice playback order
- **Effects** — DJ filter, delay, reverb, plus CLAP plugins and Pure Data patches with per-stem routing
- **MIDI controller support** — Configure any controller with MIDI Learn wizard
- **Smart suggestions** — AI-powered track recommendations based on what's currently loaded. Finds harmonically compatible tracks using audio fingerprint similarity, key matching, BPM proximity, and perceptual arousal. An energy direction fader lets you steer suggestions toward higher-energy or cooler tracks. Choose between two key matching algorithms in Settings:
  - **Camelot** — Classic DJ wheel with hand-tuned transition scores
  - **Krumhansl** — Perceptual key distance based on music psychology research, better at rating cross-mode transitions (e.g., C major to C minor)
- **Auto-gain** — Tracks are loudness-normalized so volumes are consistent
- **Track tags** — Color-coded tag pills in the browser for genres, moods, or custom labels. Suggestion results include auto-generated reason tags (see below)

### Audio Quality

- **Zero-dropout loading** — Load new tracks while playing without audio glitches
- **High-quality time stretching** — Tempo changes without pitch artifacts
- **Master bus protection** — Built-in limiter and clipper prevent distortion and protect your speakers, even when mixing hot
- **Low-latency audio** — JACK on Linux, WASAPI on Windows
- **Professional routing** — Separate master and cue outputs for headphone monitoring

---

## Installation

### Release Packages

| Package | Platform | Description |
|---------|----------|-------------|
| `mesh-cue_amd64.deb` | Linux (Debian/Ubuntu) | Full DJ application with stem separation (CPU) |
| `mesh-cue-cuda_amd64.deb` | Linux (Debian/Ubuntu) | Full DJ application with NVIDIA CUDA acceleration |
| `mesh-cue_win.zip` | Windows 10/11 | Full DJ application with DirectML GPU acceleration |
| `mesh-player_amd64.deb` | Linux (Debian/Ubuntu) | Lightweight stem player |
| `mesh-player_win.zip` | Windows 10/11 | Lightweight stem player |

### Linux

```bash
# Standard build (CPU stem separation)
sudo dpkg -i mesh-cue_amd64.deb

# OR with NVIDIA GPU acceleration
sudo dpkg -i mesh-cue-cuda_amd64.deb

# Optional: lightweight player only
sudo dpkg -i mesh-player_amd64.deb
```

**Requirements:**
- Ubuntu 22.04+, Debian 12+, Pop!_OS 22.04+, or similar
- PipeWire or JACK audio server
- For CUDA build: NVIDIA driver 525+ and CUDA 12

### Windows

1. Download and extract `mesh-cue_win.zip` or `mesh-player_win.zip`
2. Run `mesh-cue.exe` or `mesh-player.exe`

**Requirements:**
- Windows 10 or 11
- DirectX 12 capable GPU for accelerated stem separation (optional — falls back to CPU)

---

## Quick Start

### 1. Import Your Music

Launch **mesh-cue** and import your tracks:

**Option A: Automatic Stem Separation**
1. Copy audio files (MP3, FLAC, WAV) to `~/Music/mesh-collection/import/`
2. Click **Import** → Select **"Mixed Audio"** mode
3. Tracks are automatically separated into stems, analyzed, and added to your collection

> **Note:** Stem separation requires ~4GB RAM per track and takes 2-5 minutes on CPU, or 15-30 seconds with GPU acceleration.

**Option B: Pre-Separated Stems**
If you've already separated stems using Demucs, UVR, or similar tools:
1. Name files as `Artist - Track_(Vocals).wav`, `Artist - Track_(Drums).wav`, etc.
2. Click **Import** → Select **"Pre-separated Stems"** mode

### 2. Prepare Your Tracks

In mesh-cue, load a track to:
- Verify BPM detection and adjust if needed
- Set hot cues at key moments (drops, breakdowns, vocals)
- Fine-tune the beat grid alignment
- Optionally link stems from other tracks for mashups

### 3. Perform

Launch **mesh-player**, load tracks onto the 4 decks, and start mixing:

| Control | Function |
|---------|----------|
| **Play/Cue** | CDJ-style transport (hold cue to preview) |
| **Stem buttons** | Mute individual stems |
| **Sync** | Automatic beat alignment |
| **Key** | Automatic harmonic matching |
| **Slicer** | Enter slice mode for real-time remixing |

---

## MIDI Controllers

Mesh works with any MIDI controller. Use the **MIDI Learn** wizard to map your hardware:

```bash
mesh-player --midi-learn
```

The wizard guides you through mapping transport controls, performance pads, mixer faders, and browser navigation. Mappings are saved to `~/.config/mesh-player/midi.yaml`.

**Tested controllers:**
- Pioneer DDJ-SB2 (profile included)

---

## Stem Separation

> **Experimental:** Stem separation quality may vary. GPU acceleration is untested on some hardware.

Mesh uses [Demucs](https://github.com/facebookresearch/demucs) (Meta AI) for neural stem separation:

| Setting | Options | Effect |
|---------|---------|--------|
| **Model** | Standard / Fine-tuned | Fine-tuned has ~1-3% better quality |
| **Shifts** | 1-5 | More shifts = better quality, slower processing |

**Performance:**

| Hardware | Time (4-min track) |
|----------|-------------------|
| CPU (8-core) | 3-5 minutes |
| NVIDIA RTX 3070 | 20-30 seconds |
| NVIDIA RTX 4090 | 10-15 seconds |

Configure in **Settings → Separation**.

---

## Pure Data Effects

Mesh supports custom audio effects written in [Pure Data](https://puredata.info/), a visual programming language for audio. Create your own filters, delays, distortions, or even neural audio effects like RAVE.

Place effects in `~/Music/mesh-collection/effects/` and they'll appear in the effect picker.

See [examples/pd-effects/](examples/pd-effects/) for templates, documentation, and working examples including a RAVE neural percussion processor.

---

## CLAP Plugins

Mesh supports [CLAP](https://cleveraudio.org/) (CLever Audio Plugin) — the modern open-source plugin standard. Load any Linux CLAP plugin as a stem effect:

- **LSP Plugins** — Professional compressors, EQs, reverbs, gates
- **Dragonfly Reverb** — Algorithmic room and plate reverbs
- **Airwindows** — Hundreds of boutique effects
- **BYOD, ChowTapeModel** — Guitar amp sims and tape saturation

**Plugin locations:**
```
~/.clap/              # User plugins
/usr/lib/clap/        # System plugins
```

Install CLAP plugins from your distro's package manager or download from plugin developers. Plugins appear automatically in the effect picker under their categories.

---

## Smart Suggestions

When you toggle suggestions on in the collection browser, Mesh analyzes the tracks loaded on your decks and recommends what to play next. Each suggestion gets colored **reason tags** that explain *why* it was recommended.

### Reason Tags

Each suggestion row shows a colored pill describing the key relationship to your currently playing tracks. The arrow indicates the direction of movement on the Camelot wheel:

| Tag | Meaning |
|-----|---------|
| **━ Same Key** | Same key — maximum harmonic safety |
| **▲ Adjacent** | One step clockwise on the Camelot wheel — lifts energy while staying harmonically safe |
| **▼ Adjacent** | One step counter-clockwise — cools energy, still safe |
| **▲ Diagonal** | Cross-mode step up (e.g., 8B→9A) — shifts mood while raising energy |
| **▼ Diagonal** | Cross-mode step down (e.g., 8A→7B) — shifts mood while cooling |
| **▲ Boost** | Two steps clockwise (+2) — noticeable energy increase, fewer shared notes |
| **▼ Cool** | Two steps counter-clockwise (-2) — noticeable energy decrease |
| **▲ Mood Lift** | Minor→major at the same Camelot position (e.g., 8A→8B = Am→C) — brightens mood |
| **▼ Darken** | Major→minor at the same position (e.g., 8B→8A = C→Am) — darkens mood |
| **▲ Semitone** | Classic pop key change (+7 on Camelot = one semitone up in pitch) — dramatic lift |
| **▼ Semitone** | One semitone down (-7 on Camelot) — dramatic drop |
| **▲/▼ Far** | 3-5 steps on the Camelot wheel — risky but available at extreme energy settings |
| **▼ Tritone** | 6 steps (maximum dissonance) — only appears at extreme energy drop settings |

### Symbols

- **▲** — Transition moves clockwise on the Camelot wheel (raises musical tension)
- **▼** — Transition moves counter-clockwise (releases musical tension)
- **━** — Same key (no movement)

### Color Coding

Tag pill colors use a traffic-light system based on harmonic compatibility:

| Color | Meaning |
|-------|---------|
| **Green** | Excellent match (key score ≥ 0.7) — harmonically safe transition |
| **Amber** | Acceptable match (key score ≥ 0.4) — use with care |
| **Red** | Risky match (key score < 0.4) — clashing keys, dramatic effect only |

### Energy Direction Fader

The energy direction fader (center of the suggestions panel) steers what kinds of tracks are recommended:

- **Center** — Strict harmonic matching only (same key, adjacent, relative)
- **Right** — Progressively unlocks energy-raising transitions (boost, mood lift, semitone up)
- **Left** — Progressively unlocks energy-cooling transitions (cool, darken, tritone)

At extreme fader positions, the harmonic filter relaxes to allow dramatic key changes that would normally be filtered out.

### Key Scoring Models

Two algorithms are available in **Settings → Display → Key Matching**:

| Model | Description |
|-------|-------------|
| **Camelot** (default) | Classic DJ wheel — hand-tuned scores for each transition category. Well-understood, predictable. |
| **Krumhansl** | Based on the Krumhansl-Kessler (1982) music psychology research. Uses a 24×24 perceptual key distance matrix computed from listener probe-tone ratings. Better at rating cross-mode transitions (e.g., C major to A minor) where the Camelot model uses coarse categories. |

---

## Roadmap

### Working Now

- [x] 4-deck stem mixing with mute/solo
- [x] Automatic BPM and key detection
- [x] Beat grid editing and alignment
- [x] Auto beat sync between decks
- [x] Auto key matching (harmonic mixing)
- [x] Stem slicer with customizable presets
- [x] Stem linking for mashups
- [x] Built-in Demucs stem separation
- [x] GPU acceleration (CUDA/DirectML)
- [x] MIDI controller support with learn wizard
- [x] USB export for portable performance
- [x] Track similarity search (audio fingerprinting)
- [x] Smart suggestions with energy direction control
- [x] ML genre classification and mood tagging (EffNet/Discogs400)
- [x] Auto-gain loudness normalization
- [x] Master bus limiter and clipper (PA protection)
- [x] Effects: filter, delay, reverb
- [x] Pure Data effect patches (custom DSP via PD)
- [x] CLAP plugin hosting (LSP, Dragonfly, Airwindows, etc.)
- [x] RAVE neural audio effects (via nn~ external)
- [x] Multiband effect container with macro knob routing

### Coming Soon

- [ ] Session history and set reconstruction
- [ ] Beat grid analysis improvements (EDM-specific detection)
- [ ] On-the-fly stem linking during performance
- [ ] Slicer morph knob for preset banks
- [ ] Real-time LUFS normalization per stem

### Planned

- [ ] macOS support
- [ ] Recording to file

---

## Configuration

### Collection Location

```
~/Music/mesh-collection/
├── import/          # Drop files here for import
├── tracks/          # Your stem library
├── playlists/       # Playlist folders (symlinks)
└── config.yaml      # Settings
```

### Settings

Click the **gear icon** in mesh-cue to configure:

| Setting | Description |
|---------|-------------|
| **BPM Range** | Set min/max tempo for genre-specific detection (e.g., DnB: 160-190) |
| **Separation Model** | Standard or Fine-tuned Demucs |
| **Separation Shifts** | Quality vs. speed tradeoff (1-5) |
| **Target Loudness** | LUFS target for auto-gain (-14 LUFS default) |

### Theme

Customize stem colors in `~/.config/mesh-player/theme.yaml`:

```yaml
stems:
  vocals: "#33CC66"   # Green
  drums: "#CC3333"    # Red
  bass: "#E6604D"     # Orange
  other: "#00CCCC"    # Cyan
```

---

## Troubleshooting

### Audio Issues

**No audio output:**
- Linux: Ensure JACK or PipeWire is running (`pw-jack mesh-player` for PipeWire)
- Windows: Check audio device in system settings

**Audio dropouts:**
- Increase buffer size in your audio server settings
- Close other audio applications

### Stem Separation

**"CUDA not available" on Linux:**
- Install NVIDIA driver 525+ and CUDA 12 toolkit
- Use the `mesh-cue-cuda_amd64.deb` package

**Separation quality issues:**
- Use higher quality source files (FLAC/WAV over low-bitrate MP3)
- Increase "Shifts" setting in Settings → Separation

### MIDI

**Controller not detected:**
- Check connection and permissions
- On Linux: ensure user is in `audio` group

---

## Building from Source

For developers:

```bash
# Clone and enter dev environment
git clone https://github.com/yourusername/mesh.git
cd mesh
nix develop

# Build
cargo build --release

# Run
cargo run -p mesh-player
cargo run -p mesh-cue
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for technical details on the audio engine, real-time architecture, and signal flow.

---

## Contributing

Contributions welcome! Areas where help is appreciated:

- **Audio DSP** — Effects, EQ implementations
- **UI/UX** — Waveform rendering, accessibility
- **Testing** — Integration tests, audio quality verification
- **Platform support** — macOS builds

Please open an issue to discuss major changes before submitting a PR.

---

## License

AGPL-3.0 — see [LICENSE](LICENSE) for details.

Uses [Essentia](https://essentia.upf.edu/) (AGPL-3.0) and [Demucs](https://github.com/facebookresearch/demucs) for audio analysis and stem separation. Genre and mood classification models from the [Essentia model hub](https://essentia.upf.edu/models.html) (CC BY-NC-SA 4.0).

---

## Acknowledgments

- [Demucs](https://github.com/facebookresearch/demucs) — Neural stem separation
- [signalsmith-stretch](https://signalsmith-audio.co.uk/code/stretch/) — Time stretching
- [Essentia](https://essentia.upf.edu/) — Audio analysis (BPM, key, beats)
- [iced](https://iced.rs/) — GUI framework
- [JACK](https://jackaudio.org/) — Professional audio routing

---

*Mesh is under active development. Star the repo to follow progress!*
