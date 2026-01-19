# Mesh Architecture

This document describes the architecture, data flow, and threading model for the Mesh DJ system.

## Overview

Mesh is a 4-deck stem-based DJ application built with:
- **Rust** for performance and safety
- **JACK Audio** for professional low-latency audio
- **iced** for the reactive GUI framework
- **Lock-free atomics** for real-time audio/UI communication

## Crate Structure

```
mesh/
├── crates/
│   ├── mesh-core/       # Audio engine, database, track loading
│   ├── mesh-player/     # DJ player application (iced GUI)
│   ├── mesh-cue/        # Cue point editor application
│   ├── mesh-midi/       # MIDI controller support
│   └── mesh-widgets/    # Shared UI widgets (waveforms)
```

---

## mesh-core

The core audio engine and services layer.

### Module Structure

```
mesh-core/
├── engine/          # Real-time audio processing
│   ├── mod.rs       # AudioEngine, EngineCommand
│   ├── deck.rs      # Per-deck playback state
│   ├── slicer.rs    # Beat slicer processor
│   └── atomics.rs   # Lock-free state sharing
├── loader/          # Background track loading
│   ├── mod.rs       # TrackLoader service
│   └── linked.rs    # Linked stem loading
├── db/              # SQLite database access
│   ├── service.rs   # DatabaseService
│   └── queries.rs   # Track/metadata queries
├── usb/             # USB device management
│   ├── manager.rs   # Hot-plug detection
│   └── storage.rs   # USB collection access
└── types.rs         # Shared types (Stem, HotCue, etc.)
```

### Threading Model

```
┌─────────────────────────────────────────────────────────────────────┐
│                         THREAD ARCHITECTURE                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐     Commands      ┌──────────────────────────┐    │
│  │              │  ─────────────►   │                          │    │
│  │   UI Thread  │   (SPSC Ring)     │   JACK Audio Thread      │    │
│  │   (iced)     │                   │   (Real-time, 2.9ms)     │    │
│  │              │  ◄─────────────   │                          │    │
│  └──────────────┘     Atomics       └──────────────────────────┘    │
│         │              (Lock-free)            ▲                      │
│         │                                     │                      │
│         ▼                                     │                      │
│  ┌──────────────┐                    ┌────────┴─────────┐           │
│  │ Track Loader │                    │  Shared Buffers  │           │
│  │   Thread     │───────────────────►│  (basedrop Arc)  │           │
│  │ (Background) │  Stems + Metadata  └──────────────────┘           │
│  └──────────────┘                                                    │
│         │                                                            │
│         ▼                                                            │
│  ┌──────────────┐     ┌──────────────┐                              │
│  │   Database   │     │ Peaks Thread │                              │
│  │   (SQLite)   │     │ (Waveforms)  │                              │
│  └──────────────┘     └──────────────┘                              │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Data Flow: Track Loading

```
┌─────────────────────────────────────────────────────────────────────┐
│                      TRACK LOADING PIPELINE                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  User Action          Background Thread           Audio Thread       │
│  ───────────          ─────────────────           ────────────       │
│                                                                      │
│  LoadTrack(deck, path)                                               │
│       │                                                              │
│       ▼                                                              │
│  ┌─────────────┐                                                     │
│  │ TrackLoader │                                                     │
│  │  .request() │                                                     │
│  └──────┬──────┘                                                     │
│         │                                                            │
│         ▼                                                            │
│  ┌─────────────────────────────────────┐                            │
│  │     Loader Thread (Background)       │                            │
│  │  ┌─────────────────────────────────┐ │                            │
│  │  │ 1. Read .stems file (4 stems)   │ │                            │
│  │  │ 2. Decode FLAC (parallel)       │ │                            │
│  │  │ 3. Compute overview peaks       │ │                            │
│  │  │ 4. Fetch metadata from DB       │ │                            │
│  │  │ 5. Create Shared<StemBuffers>   │ │                            │
│  │  └─────────────────────────────────┘ │                            │
│  └──────────────┬──────────────────────┘                            │
│                 │                                                    │
│                 ▼                                                    │
│  ┌─────────────────────────────────────┐                            │
│  │   TrackLoadResult (via channel)      │                            │
│  │   - PreparedTrack (metadata, cues)   │                            │
│  │   - Shared<StemBuffers>              │                            │
│  │   - OverviewState (peaks)            │                            │
│  └──────────────┬──────────────────────┘                            │
│                 │                                                    │
│       ┌─────────┴─────────┐                                         │
│       ▼                   ▼                                         │
│  ┌─────────┐      ┌───────────────┐                                 │
│  │   UI    │      │ Audio Engine  │                                 │
│  │ Updates │      │ LoadTrack cmd │                                 │
│  │ waveform│      │ (SPSC ring)   │                                 │
│  └─────────┘      └───────────────┘                                 │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Audio Engine Command Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                    ENGINE COMMAND PROTOCOL                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  UI Thread                           Audio Thread                    │
│  ─────────                           ────────────                    │
│                                                                      │
│  domain.toggle_play(deck)                                            │
│       │                                                              │
│       ▼                                                              │
│  CommandSender::send()                                               │
│       │                                                              │
│       │  ┌────────────────────────────────────────┐                 │
│       └─►│     SPSC Ring Buffer (lock-free)       │                 │
│          │     EngineCommand::TogglePlay(deck)    │                 │
│          └────────────────────┬───────────────────┘                 │
│                               │                                      │
│                               ▼                                      │
│                    ┌──────────────────────┐                         │
│                    │  process() callback  │                         │
│                    │  (every 2.9ms)       │                         │
│                    │  ┌────────────────┐  │                         │
│                    │  │ drain_commands │  │                         │
│                    │  │ apply to deck  │  │                         │
│                    │  │ update atomics │  │                         │
│                    │  └────────────────┘  │                         │
│                    └──────────────────────┘                         │
│                               │                                      │
│                               ▼                                      │
│                    ┌──────────────────────┐                         │
│                    │   DeckAtomics        │                         │
│                    │   - position: u64    │◄──── UI reads           │
│                    │   - is_playing: bool │      (lock-free)        │
│                    │   - loop_active: bool│                         │
│                    └──────────────────────┘                         │
│                                                                      │
│  Key Commands:                                                       │
│  ├─ LoadTrack(deck, stems, metadata)                                │
│  ├─ TogglePlay(deck), CuePress/Release(deck)                        │
│  ├─ HotCuePress(deck, slot), SetCuePoint(deck)                      │
│  ├─ ToggleLoop(deck), AdjustLoopLength(deck, delta)                 │
│  ├─ SetVolume(deck, vol), SetEq*(deck, val)                         │
│  ├─ ToggleStemMute(deck, stem), ToggleStemSolo(deck, stem)          │
│  └─ LinkStem(deck, stem, data), ToggleLinkedStem(deck, stem)        │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Lock-Free State Sharing

```
┌─────────────────────────────────────────────────────────────────────┐
│                    ATOMIC STATE SHARING                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Audio Thread (Writer)              UI Thread (Reader)               │
│  ─────────────────────              ───────────────────              │
│                                                                      │
│  DeckAtomics [4 decks]                                               │
│  ├─ position.store(pos, Relaxed)  ───►  position.load(Relaxed)      │
│  ├─ play_state.store(state)       ───►  play_state.load()           │
│  ├─ loop_active.store(bool)       ───►  loop_active.load()          │
│  ├─ loop_start/end.store(u64)     ───►  loop_start/end.load()       │
│  └─ lufs_gain.store(f32)          ───►  lufs_gain.load()            │
│                                                                      │
│  SlicerAtomics [4 decks]                                             │
│  ├─ active.store(bool)            ───►  active.load()               │
│  ├─ current_slice.store(u8)       ───►  current_slice.load()        │
│  └─ queue.store([u8; 16])         ───►  queue.load()                │
│                                                                      │
│  LinkedStemAtomics [4 decks][4 stems]                                │
│  ├─ has_linked[stem].store(bool)  ───►  has_linked[stem].load()     │
│  └─ use_linked[stem].store(bool)  ───►  use_linked[stem].load()     │
│                                                                      │
│  Benefits:                                                           │
│  ├─ Zero contention (no locks)                                      │
│  ├─ ~5ns read latency                                               │
│  ├─ Audio thread never blocks                                       │
│  └─ UI reads at 60fps without affecting audio                       │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## mesh-player

The DJ player application with iced GUI.

### Module Structure

```
mesh-player/
├── main.rs              # Application entry point
├── config.rs            # Configuration loading/saving
├── domain/
│   └── mod.rs           # MeshDomain - service orchestration
└── ui/
    ├── app.rs           # MeshApp - iced application
    ├── message.rs       # Message enum definitions
    ├── state.rs         # UI state types
    ├── handlers/        # Message handlers (extracted)
    │   ├── mixer.rs     # Volume, EQ, filter
    │   ├── settings.rs  # Settings modal
    │   ├── midi_learn.rs# MIDI learn workflow
    │   ├── browser.rs   # Collection browser + USB
    │   ├── track_loading.rs # Load results
    │   ├── deck_controls.rs # Deck playback/stems
    │   └── tick.rs      # Periodic sync (60fps)
    ├── deck_view.rs     # Per-deck control UI
    ├── mixer_view.rs    # Mixer channel UI
    ├── collection_browser.rs # Track browser
    ├── player_canvas.rs # Waveform rendering
    ├── midi_learn.rs    # MIDI learn UI
    ├── settings.rs      # Settings modal UI
    └── theme.rs         # Color palette
```

### Three-Layer Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    MESH-PLAYER ARCHITECTURE                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      UI LAYER (iced)                         │    │
│  │  ┌─────────────────────────────────────────────────────────┐ │    │
│  │  │                     MeshApp                              │ │    │
│  │  │  ┌─────────┐  ┌──────────┐  ┌────────────────────────┐  │ │    │
│  │  │  │ update()│  │  view()  │  │    subscription()      │  │ │    │
│  │  │  │         │  │          │  │  - Tick (60fps)        │  │ │    │
│  │  │  │ Message │  │ Element  │  │  - TrackLoaded         │  │ │    │
│  │  │  │ dispatch│  │ tree     │  │  - PeaksComputed       │  │ │    │
│  │  │  └────┬────┘  └──────────┘  │  - LinkedStemLoaded    │  │ │    │
│  │  │       │                     │  - UsbEvents           │  │ │    │
│  │  │       ▼                     └────────────────────────┘  │ │    │
│  │  │  ┌─────────────────────────────────────────────────────┐│ │    │
│  │  │  │                   handlers/                          ││ │    │
│  │  │  │  tick │ deck │ mixer │ browser │ settings │ ...     ││ │    │
│  │  │  └───────────────────────┬─────────────────────────────┘│ │    │
│  │  └──────────────────────────┼──────────────────────────────┘ │    │
│  └─────────────────────────────┼────────────────────────────────┘    │
│                                │                                     │
│                                ▼                                     │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    DOMAIN LAYER                              │    │
│  │  ┌─────────────────────────────────────────────────────────┐ │    │
│  │  │                    MeshDomain                            │ │    │
│  │  │                                                          │ │    │
│  │  │  Owns:                    Provides:                      │ │    │
│  │  │  ├─ DatabaseService       ├─ toggle_play(deck)           │ │    │
│  │  │  ├─ TrackLoader           ├─ set_volume(deck, vol)       │ │    │
│  │  │  ├─ PeaksComputer         ├─ load_linked_stem(...)       │ │    │
│  │  │  ├─ UsbManager            ├─ request_track_load(...)     │ │    │
│  │  │  ├─ CommandSender         └─ apply_loaded_track(...)     │ │    │
│  │  │  ├─ deck_stems[4]                                        │ │    │
│  │  │  ├─ track_lufs[4]                                        │ │    │
│  │  │  └─ global_bpm                                           │ │    │
│  │  └──────────────────────────┬──────────────────────────────┘ │    │
│  └─────────────────────────────┼────────────────────────────────┘    │
│                                │                                     │
│                                ▼                                     │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    SERVICE LAYER (mesh-core)                 │    │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌───────────┐ │    │
│  │  │AudioEngine │ │TrackLoader │ │ Database   │ │UsbManager │ │    │
│  │  │(JACK)      │ │(Background)│ │ (SQLite)   │ │(Hot-plug) │ │    │
│  │  └────────────┘ └────────────┘ └────────────┘ └───────────┘ │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Message Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                      MESSAGE FLOW (iced)                             │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  User Input / Subscription                                           │
│          │                                                           │
│          ▼                                                           │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                        Message                                │   │
│  │  ├─ Tick                    → handlers::tick::handle()       │   │
│  │  ├─ TrackLoaded(msg)        → handlers::track_loading::...   │   │
│  │  ├─ PeaksComputed(result)   → handlers::track_loading::...   │   │
│  │  ├─ LinkedStemLoaded(msg)   → handlers::track_loading::...   │   │
│  │  ├─ Deck(idx, DeckMessage)  → handlers::deck_controls::...   │   │
│  │  ├─ Mixer(MixerMessage)     → handlers::mixer::handle()      │   │
│  │  ├─ CollectionBrowser(msg)  → handlers::browser::...         │   │
│  │  ├─ Settings(SettingsMsg)   → handlers::settings::handle()   │   │
│  │  ├─ MidiLearn(LearnMsg)     → handlers::midi_learn::handle() │   │
│  │  ├─ Usb(UsbMessage)         → handlers::browser::handle_usb()│   │
│  │  ├─ SetGlobalBpm(f64)       → domain.set_global_bpm_with_... │   │
│  │  ├─ LoadTrack(deck, path)   → domain.request_track_load()    │   │
│  │  ├─ DeckSeek(deck, pos)     → domain.seek()                  │   │
│  │  └─ DeckSetZoom(deck, bars) → player_canvas_state.set_zoom() │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Tick Handler (60fps UI Sync)

```
┌─────────────────────────────────────────────────────────────────────┐
│                    TICK HANDLER (handlers/tick.rs)                   │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Every 16ms (60fps):                                                 │
│                                                                      │
│  1. MIDI Input Polling                                               │
│     ├─ Drain MIDI messages from controller                          │
│     ├─ Route to deck/mixer/browser handlers                         │
│     └─ MIDI Learn capture (if active)                               │
│                                                                      │
│  2. Atomic State Sync (lock-free reads)                             │
│     ├─ Read DeckAtomics[4]                                          │
│     │   ├─ position, is_playing, loop_active                        │
│     │   ├─ lufs_gain, key_match, transpose                          │
│     │   └─ Update player_canvas_state                               │
│     ├─ Read SlicerAtomics[4]                                        │
│     │   ├─ active, current_slice, queue                             │
│     │   └─ Update deck_views + canvas                               │
│     └─ Read LinkedStemAtomics[4][4]                                 │
│         ├─ has_linked, use_linked per stem                          │
│         └─ Update waveform split-view state                         │
│                                                                      │
│  3. Waveform Peak Requests                                           │
│     ├─ Check if zoomed view needs recompute                         │
│     ├─ Send PeaksComputeRequest to background thread                │
│     └─ Results arrive via PeaksComputed subscription                │
│                                                                      │
│  4. MIDI LED Feedback                                                │
│     ├─ Build FeedbackState from current UI state                    │
│     └─ Send to controller (play LEDs, hot cue LEDs, etc.)           │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Waveform Rendering Pipeline

```
┌─────────────────────────────────────────────────────────────────────┐
│                    WAVEFORM RENDERING                                │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────┐     ┌─────────────────┐     ┌───────────────┐  │
│  │   TrackLoader   │────►│  OverviewState  │────►│   Overview    │  │
│  │ (pre-computed)  │     │  (4096 peaks)   │     │   Waveform    │  │
│  └─────────────────┘     └─────────────────┘     │  (GPU canvas) │  │
│                                                   └───────────────┘  │
│                                                                      │
│  ┌─────────────────┐     ┌─────────────────┐     ┌───────────────┐  │
│  │  PeaksComputer  │────►│   ZoomedState   │────►│    Zoomed     │  │
│  │   (on-demand)   │     │ (cached peaks)  │     │   Waveform    │  │
│  │                 │     │                 │     │  (scrolling)  │  │
│  │ Request params: │     │ Cache key:      │     └───────────────┘  │
│  │ - playhead pos  │     │ - position      │                        │
│  │ - zoom bars     │     │ - zoom_bars     │                        │
│  │ - stem buffers  │     │ - linked_active │                        │
│  │ - linked stems  │     │                 │                        │
│  └─────────────────┘     └─────────────────┘                        │
│                                                                      │
│  Per-stem colors (configurable palette):                            │
│  ├─ Vocals: Cyan      (#00FFFF)                                     │
│  ├─ Drums:  Magenta   (#FF00FF)                                     │
│  ├─ Bass:   Yellow    (#FFFF00)                                     │
│  └─ Other:  Green     (#00FF00)                                     │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Performance Characteristics

| Operation | Latency | Thread |
|-----------|---------|--------|
| Audio process callback | 2.9ms (128 samples @ 44.1kHz) | JACK RT |
| Atomic state read | ~5ns | UI |
| Command send (SPSC) | ~50ns | UI → Audio |
| Track load (full) | 2-5s | Background |
| Peak computation | 10-50ms | Background |
| UI render (60fps) | 16ms budget | UI |

## Key Design Decisions

1. **Lock-free audio**: Audio thread never acquires locks or allocates memory
2. **SPSC command ring**: Bounded, pre-allocated buffer for UI→Audio commands
3. **basedrop::Shared**: Reference-counted buffers safe for real-time deallocation
4. **Domain layer**: Encapsulates all service coordination, hides EngineCommand details
5. **Handler extraction**: Message handlers in separate modules for maintainability
6. **Atomic state sharing**: UI reads engine state without synchronization overhead
