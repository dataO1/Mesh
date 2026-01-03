# Mesh DJ Player - Design Document

**Date:** 2026-01-03
**Status:** Approved
**Author:** Collaborative design session

---

## Overview

Mesh is a Rust-based DJ software suite consisting of two main applications:
1. **DJ Player** (mesh-player) - Real-time 4-deck performance application with stem-based mixing and neural audio effects
2. **Cue Software** (mesh-cue) - Track preparation, analysis, and playlist management (Phase 2)

Both applications share a core library and effect system.

---

## Tech Stack

- **Language:** Rust
- **GUI:** iced 0.14.0
- **Audio:** JACK (Linux-only) via jack 0.13.4
- **Time-stretching:** signalsmith-stretch 0.1.3
- **PD Integration:** libpd-rs 0.2.0
- **File Format:** RF64/BWF via riff 2.0.0
- **Build System:** Nix flake

---

## Architecture Overview

### High-Level Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│                              ICED UI                                    │
│                                                                         │
│ ┌─────────────┐  ┌─────────────────────────────┐  ┌─────────────┐      │
│ │   DECK 1    │  │       GLOBAL SECTION        │  │   DECK 2    │      │
│ │             │  │   [BPM: 128] [Master Vol]   │  │             │      │
│ ├─────────────┤  ├─────────────────────────────┤  ├─────────────┤      │
│ │   DECK 3    │  │     PLAYLIST BROWSER        │  │   DECK 4    │      │
│ │             │  │                             │  │             │      │
│ └─────────────┘  ├─────────────────────────────┤  └─────────────┘      │
│                  │        MIXER SECTION        │                        │
│                  │  ┌─────┬─────┬─────┬─────┐  │                        │
│                  │  │ D1  │ D2  │ D3  │ D4  │  │                        │
│                  │  │Trim │Trim │Trim │Trim │  │                        │
│                  │  │Filt │Filt │Filt │Filt │  │                        │
│                  │  │Vol  │Vol  │Vol  │Vol  │  │                        │
│                  │  │[Cue]│[Cue]│[Cue]│[Cue]│  │                        │
│                  │  └─────┴─────┴─────┴─────┘  │                        │
│                  └─────────────────────────────┘                        │
└────────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼
                           AUDIO ENGINE
                                  │
                                  ▼
                         JACK (4 channels)
                      Master L/R + Cue L/R
```

### UI Layout

- **Left column:** Deck 1 (top), Deck 3 (bottom)
- **Center column:** Global section (top), Playlist browser (middle), Mixer (bottom)
- **Right column:** Deck 2 (top), Deck 4 (bottom)

---

## Deck Structure

### Deck UI Layout

```
┌─────────────────────────────────────────────────────────────────┐
│                           DECK N                                 │
├─────────────────────────────────────────────────────────────────┤
│  ┌───────────────────────────────────────────────────┐ ┌─────┐  │
│  │              WAVEFORM DISPLAY                      │ │LOOP │  │
│  │  ════════════════════╪════════════════════        │ │     │  │
│  │  Vocals/Drums/Bass/Other (color-coded, overlapped)│ │[ENC]│  │
│  │                      ▲ playhead + beat grid       │ │1 bar│  │
│  └───────────────────────────────────────────────────┘ └─────┘  │
│                                                                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  STEM EFFECT CHAIN                                         │  │
│  │  Tabs: [Vocals] [Drums] [Bass] [Other]                     │  │
│  │  ┌─────────────────────────────────────────────────────┐   │  │
│  │  │ Selected: DRUMS              [M] [S]                 │   │  │
│  │  │ Chain: [RAVE ●]──[Delay ◯]──[Reverb ●]──[+]         │   │  │
│  │  └─────────────────────────────────────────────────────┘   │  │
│  │  ┌─────────────────────────────────────────────────────┐   │  │
│  │  │ CHAIN CONTROLS (8 mappable knobs)                    │   │  │
│  │  │  ◯1    ◯2    ◯3    ◯4    ◯5    ◯6    ◯7    ◯8       │   │  │
│  │  └─────────────────────────────────────────────────────┘   │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │  MODE: [CUE][___][___]                                     │  │
│  │  ┌───┬───┬───┬───┬───┬───┬───┬───┐  ┌─────────┐           │  │
│  │  │ 1 │ 2 │ 3 │ 4 │ 5 │ 6 │ 7 │ 8 │  │ [◄ JMP] │           │  │
│  │  └───┴───┴───┴───┴───┴───┴───┴───┘  │ [► JMP] │           │  │
│  │           [SHIFT]                    │  [CUE]  │           │  │
│  │                                      │  [▶ ▮▮] │           │  │
│  └──────────────────────────────────────┴─────────┘           │  │
└─────────────────────────────────────────────────────────────────┘
```

### Deck Components

1. **Waveform Display**
   - 4 color-coded stereo waveforms overlapped (Vocals, Drums, Bass, Other)
   - Playhead in center
   - Beat grid markers visible

2. **Loop Encoder** (inside, next to waveform)
   - Press: Toggle loop on/off
   - Rotate: Change loop length (0.25, 0.5, 1, 2, 4, 8, 16 beats)
   - Beat-grid quantized

3. **Stem Effect Chain**
   - 4 tabs (Vocals/Drums/Bass/Other) - only selected stem visible
   - Per-stem Mute [M] and Solo [S] toggles
   - Effect chain with bypass toggles (●/◯)
   - 8 mappable chain controls (one-to-many parameter mapping)

4. **8 Action Buttons**
   - Modal system with mode selector (CUE mode for MVP)
   - Shift modifier for alternate functions
   - CUE mode: Press empty = set cue, press existing = jump, shift+press = delete

5. **Transport** (outside, next to action buttons)
   - Beat jump backward [◄ JMP] (uses loop size)
   - Beat jump forward [► JMP] (uses loop size)
   - Cue button (CDJ-style: hold to preview, release to return)
   - Play/Pause [▶ ▮▮]

### Mixer Section (Global)

Per-deck channel strip:
- Trim knob (gain)
- Filter knob (center=flat, left=LP, right=HP)
- Volume fader
- Cue toggle (routes to cue bus at full volume, bypassing fader)

Global controls:
- BPM encoder (30-200 BPM)
- Master volume

---

## Audio Engine

### Signal Flow

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              DECK N                                      │
│                                                                          │
│  ┌─────────┐    ┌──────────────────────────────────────────┐            │
│  │ Buffer  │    │           STEM PROCESSORS                 │            │
│  │ Reader  │───►│  Vocals ──► [Chain] ──► [M/S]            │            │
│  │ (8ch)   │    │  Drums  ──► [Chain] ──► [M/S]            │            │
│  │ @original    │  Bass   ──► [Chain] ──► [M/S]            │            │
│  │  tempo  │    │  Other  ──► [Chain] ──► [M/S]            │            │
│  └─────────┘    └──────────────────┬───────────────────────┘            │
│                                    │                                     │
│                      ┌─────────────▼─────────────┐                       │
│                      │  GLOBAL LATENCY COMP.     │                       │
│                      └─────────────┬─────────────┘                       │
│                                    │                                     │
│                      ┌─────────────▼─────────────┐                       │
│                      │      STEM SUMMER          │                       │
│                      │   (4 stereo → 1 stereo)   │                       │
│                      └─────────────┬─────────────┘                       │
│                                    │                                     │
│                      ┌─────────────▼─────────────┐                       │
│                      │      TIMESTRETCH          │                       │
│                      │   (2ch @ global BPM)      │                       │
│                      └─────────────┬─────────────┘                       │
│                                    ▼ to mixer                            │
└─────────────────────────────────────────────────────────────────────────┘
```

### Key Design Decisions

1. **In-memory track buffer**: Entire track loaded into RAM for instant beat jumping
   - ~212 MB per 5-minute track (8ch, 44.1kHz, 16-bit)
   - ~850 MB for 4 loaded tracks

2. **Timestretch after stem sum**: Only 2 channels timestretched instead of 8 (4× less CPU)

3. **Global latency compensation**: All 16 stems (4 decks × 4 stems) aligned to maximum latency across the entire system for perfect sync

4. **Auto-sync**: Single global BPM, no manual pitch faders. BPM and beat grid from file metadata.

---

## Global Latency Compensation

All stems across all decks must be sample-aligned for proper beat sync.

```
Global Max Latency = max(all 16 stem chain latencies)

For each stem:
  compensation_delay = Global Max - stem_chain_latency
```

### Implementation

- 16 ring buffers (4 decks × 4 stems)
- Recalculated when any effect chain changes (add/remove/bypass)
- Tradeoff: High-latency effects increase overall system latency

---

## Effect System

### Effect Trait

```rust
pub trait Effect: Send {
    fn process(&mut self, buffer: &mut [f32], sample_rate: u32);
    fn latency_samples(&self) -> u32;
    fn info(&self) -> EffectInfo;
    fn get_params(&self) -> &[ParamValue];
    fn set_param(&mut self, index: usize, value: f32);
    fn set_bypass(&mut self, bypass: bool);
    fn is_bypassed(&self) -> bool;
}

pub struct EffectInfo {
    pub name: String,
    pub params: [ParamInfo; 8],  // Fixed 8 params max
}
```

### Effect Types

1. **Native Rust effects**: Implement Effect trait, recompile to add new ones
   - GainEffect (0 latency)
   - DjFilterEffect - HP/LP in one knob (minimal latency)
   - DelayEffect - beat-synced (latency = delay time)
   - ReverbEffect

2. **PD effects**: Pure Data patches via libpd-rs (dynamic, no recompile)
   - Must follow template contract
   - Report latency via outlet

### PD Effect Contract

```
REQUIRED INLETS:
  [inlet~ L]  [inlet~ R]  - Audio input (stereo)

REQUIRED OUTLETS:
  [outlet~ L] [outlet~ R] - Audio output (stereo)
  [outlet latency]        - Latency in samples (bang to query)

PARAMETER RECEIVES (optional, up to 8):
  [r param0] ... [r param7]  - Float 0.0-1.0

BYPASS RECEIVE:
  [r bypass]  - 0 = process, 1 = bypass
```

### Effect Chain Controls

- 8 mappable knobs per stem chain
- One-to-many mapping: single knob can control multiple effect parameters
- Used for live performance control

---

## File Format

### Multi-track WAV (RF64/BWF)

- **Channels**: 8 (4 stereo stems: Vocals L/R, Drums L/R, Bass L/R, Other L/R)
- **Sample rate**: 44.1 kHz (fixed, Cue Software handles conversion)
- **Bit depth**: 16-bit

### Metadata Storage

**bext chunk (Broadcast Extension):**
```
BPM:128.00|KEY:Am|GRID:0,22050,44100,...|ORIGINAL_BPM:125.00
```

**cue chunk**: Hot cue sample positions (up to 8)

**adtl LIST chunk**: Cue labels and colors
```
<cue_number>:<label>|color:<hex>
Example: 1:Drop|color:#FF5500
```

---

## Track Loading & Playlist

### Workflow

1. **Cue Software** manages:
   - Collection (folders with multi-track WAV files)
   - Playlists (symlinked references, nested folders allowed)

2. **DJ Player** track selector (center of UI):
   - Encoder scroll through items
   - Encoder press → enter playlist/folder
   - Back button → navigate up
   - 4 deck load buttons → load selected track to deck

### On Track Load

1. Read entire WAV into memory (8 channels)
2. Parse metadata (BPM, key, beat grid)
3. Load hot cues into 8 action button slots
4. Compute waveform display data
5. File handle closed (all data in RAM)

---

## Monitoring & Outputs

### JACK Outputs (4 channels)

- **Master L/R**: Summed deck outputs respecting volume faders
- **Cue L/R**: Decks with cue enabled at full volume (bypasses fader)

### Headphone Monitoring

- Per-deck cue toggle
- Cue/Master blend knob for headphones
- Multiple decks can be cued simultaneously

---

## Crate Structure

```
mesh/
├── Cargo.toml                    # Workspace
├── flake.nix                     # Nix flake (build + devshell)
│
├── crates/
│   ├── mesh-core/                # Single shared library
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── effect/           # Effect system
│   │       ├── audio_file/       # RF64/BWF handling
│   │       ├── timestretch/      # signalsmith-stretch wrapper
│   │       ├── pd/               # libpd-rs integration
│   │       ├── engine/           # Audio engine, deck, mixer
│   │       └── types.rs
│   │
│   ├── mesh-player/              # DJ Player binary
│   │   └── src/
│   │       ├── main.rs
│   │       ├── app.rs
│   │       ├── audio.rs
│   │       ├── state.rs
│   │       ├── messages.rs
│   │       └── ui/
│   │
│   └── mesh-cue/                 # Cue Software binary (Phase 2)
│
├── effects/
│   ├── native/                   # Rust native effects
│   │   ├── gain/
│   │   ├── filter/
│   │   ├── delay/
│   │   └── reverb/
│   │
│   └── pd/                       # Pure Data effects
│       ├── externals/            # Shared PD externals (nn~)
│       ├── rave/
│       ├── granular/
│       └── _template/
│
├── rave/                         # Experimentation folder
│
└── docs/
    └── plans/
```

---

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| jack | 0.13.4 | JACK audio client |
| iced | 0.14.0 | GUI framework |
| libpd-rs | 0.2.0 | Pure Data embedding |
| signalsmith-stretch | 0.1.3 | Time-stretching |
| riff | 2.0.0 | WAV/RF64 chunk parsing |

---

## Phase 2: Cue Software

Deferred features for Cue Software:
- Track analysis (BPM detection, beat grid, key detection)
- Cue point editing with color assignment
- Playlist management with symlinks
- Sample rate conversion during import
- Waveform preview

---

## Phase 2: Additional Features

Deferred features for DJ Player:
- MIDI control with MIDI learn/mapping (YAML config)
- VampNet integration (look-ahead buffer replacement)
- Additional action button modes (loops, samples, effects)
- CLAP plugin support

---

## VampNet Integration (Deferred)

Concept for future implementation:
- User selects beats/bars to "regenerate" (can span 2 decks for transitions)
- Preview on cue output
- On commit, replaces buffer at that location
- Not part of real-time effect chain

---

## Notes

- Effects should be optimized for low latency (global latency affects all audio)
- Consider displaying current global latency in UI
- Native Rust effects preferred for minimal latency
- PD effects allow experimentation without recompilation
