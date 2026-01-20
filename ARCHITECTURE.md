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

---

## mesh-cue

The cue point editor application for preparing tracks with metadata, cue points, loops, and stem links.

### Module Structure

```
mesh-cue/
├── main.rs              # Application entry point
├── config.rs            # Configuration loading/saving
└── ui/
    ├── app.rs           # MeshCueApp - iced application (663 lines)
    ├── message.rs       # Message enum definitions
    ├── handlers/        # Message handlers (extracted)
    │   ├── browser.rs   # Dual playlist browser (parameterized)
    │   ├── track_loading.rs # Two-phase track loading
    │   ├── playback.rs  # Audio transport controls
    │   ├── waveform.rs  # Waveform interaction
    │   ├── stem_links.rs# Stem linking workflow
    │   └── tick.rs      # Periodic sync (60fps)
    ├── modals/          # Modal overlay utilities
    │   ├── overlay.rs   # with_modal_overlay() helper
    │   └── mod.rs
    ├── state/           # UI state types
    │   ├── collection.rs# Browser state + drag/drop
    │   └── loaded_track.rs # Loaded track metadata
    ├── collection_browser.rs # Dual browser view
    ├── cue_editor.rs    # Cue point editing UI
    ├── waveform.rs      # Combined waveform canvas
    ├── transport.rs     # Playback controls
    ├── settings.rs      # Settings modal
    ├── import_modal.rs  # Batch stem import
    ├── export_modal.rs  # USB export
    └── delete_modal.rs  # Delete confirmation
```

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                    MESH-CUE ARCHITECTURE                             │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      UI LAYER (iced)                         │    │
│  │  ┌─────────────────────────────────────────────────────────┐ │    │
│  │  │                     MeshCueApp                           │ │    │
│  │  │  ┌─────────────────────────────────────────────────────┐ │ │    │
│  │  │  │                    handlers/                         │ │ │    │
│  │  │  │  browser │ track_loading │ playback │ waveform │ ... │ │ │    │
│  │  │  └─────────────────────────────────────────────────────┘ │ │    │
│  │  │  ┌─────────────────────────────────────────────────────┐ │ │    │
│  │  │  │                    modals/                           │ │ │    │
│  │  │  │  overlay │ import │ export │ delete │ settings       │ │ │    │
│  │  │  └─────────────────────────────────────────────────────┘ │ │    │
│  │  └──────────────────────────┬──────────────────────────────┘ │    │
│  └─────────────────────────────┼────────────────────────────────┘    │
│                                │                                     │
│                                ▼                                     │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    SERVICE LAYER (mesh-core)                 │    │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌───────────┐ │    │
│  │  │AudioEngine │ │ Database   │ │BatchImport │ │UsbManager │ │    │
│  │  │(JACK)      │ │ (SQLite)   │ │ (Threaded) │ │(Hot-plug) │ │    │
│  │  └────────────┘ └────────────┘ └────────────┘ └───────────┘ │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Handler Module Design (Parameterized Browsers)

```
┌─────────────────────────────────────────────────────────────────────┐
│                    BROWSER HANDLER PATTERN                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Problem: Dual browsers (left/right) had 300+ lines of duplication  │
│                                                                      │
│  Solution: Parameterized handlers with BrowserSide enum             │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  impl MeshCueApp {                                            │   │
│  │    pub fn handle_browser_left(&mut self, msg) -> Task {       │   │
│  │      self.handle_browser(BrowserSide::Left, msg)              │   │
│  │    }                                                          │   │
│  │                                                               │   │
│  │    pub fn handle_browser_right(&mut self, msg) -> Task {      │   │
│  │      self.handle_browser(BrowserSide::Right, msg)             │   │
│  │    }                                                          │   │
│  │                                                               │   │
│  │    fn handle_browser(&mut self, side, msg) -> Task {          │   │
│  │      // Single implementation handles both sides              │   │
│  │      let browser = self.collection.browser_mut(side);         │   │
│  │      let tracks = self.collection.tracks_mut(side);           │   │
│  │      // ... all logic uses side parameter                     │   │
│  │    }                                                          │   │
│  │  }                                                            │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  CollectionState accessors:                                         │
│  ├─ browser_mut(side) -> &mut PlaylistBrowserState                 │
│  ├─ browser(side) -> &PlaylistBrowserState                         │
│  ├─ tracks_mut(side) -> &mut Vec<TrackRow>                         │
│  ├─ tracks(side) -> &Vec<TrackRow>                                 │
│  └─ side_name(side) -> &str  ("Left" / "Right")                    │
│                                                                      │
│  Result: 532 → 347 lines (35% reduction)                            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Modal Overlay Pattern

```
┌─────────────────────────────────────────────────────────────────────┐
│                    MODAL OVERLAY HELPER                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Before: Each modal duplicated backdrop + centering code            │
│                                                                      │
│  After: Single reusable helper                                      │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  pub fn with_modal_overlay<'a>(                               │   │
│  │      base: Element<'a, Message>,                              │   │
│  │      modal_content: Element<'a, Message>,                     │   │
│  │      close_message: Message,                                  │   │
│  │  ) -> Element<'a, Message>                                    │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  Constructs:                                                         │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                      stack![]                                │    │
│  │  ┌─────────────────────────────────────────────────────────┐│    │
│  │  │ Layer 0: base (main app content)                        ││    │
│  │  └─────────────────────────────────────────────────────────┘│    │
│  │  ┌─────────────────────────────────────────────────────────┐│    │
│  │  │ Layer 1: backdrop (60% opacity, click-to-close)         ││    │
│  │  └─────────────────────────────────────────────────────────┘│    │
│  │  ┌─────────────────────────────────────────────────────────┐│    │
│  │  │ Layer 2: center(opaque(modal_content))                  ││    │
│  │  └─────────────────────────────────────────────────────────┘│    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
│  Usage in view():                                                    │
│  ├─ with_modal_overlay(base, import_modal, Message::CloseImport)   │
│  ├─ with_modal_overlay(base, export_modal, Message::CloseExport)   │
│  └─ with_modal_overlay(base, delete_modal, Message::CancelDelete)  │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Track Loading Pipeline (Two-Phase)

```
┌─────────────────────────────────────────────────────────────────────┐
│                    TWO-PHASE TRACK LOADING                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Phase 1: Metadata (Fast)                 Phase 2: Audio (Slow)     │
│  ──────────────────────                   ─────────────────────     │
│                                                                      │
│  User double-clicks track                                            │
│       │                                                              │
│       ▼                                                              │
│  ┌─────────────────────────────┐                                    │
│  │ DatabaseService.get_track() │  < 10ms                            │
│  │ - BPM, key, LUFS            │                                    │
│  │ - Cue points, loops         │                                    │
│  │ - Beat grid, stem links     │                                    │
│  │ - Drop marker, first beat   │                                    │
│  └──────────────┬──────────────┘                                    │
│                 │                                                    │
│                 ▼                                                    │
│  ┌─────────────────────────────┐     ┌─────────────────────────┐   │
│  │ UI updates immediately      │     │ TrackLoader.request()   │   │
│  │ - Show cue editor           │     │ (Background thread)     │   │
│  │ - Display metadata          │     │ - Decode stems (FLAC)   │   │
│  │ - Show "loading" spinner    │     │ - Compute peaks         │   │
│  └─────────────────────────────┘     │ - 2-5 seconds           │   │
│                                       └────────────┬────────────┘   │
│                                                    │                 │
│                                                    ▼                 │
│                                       ┌─────────────────────────┐   │
│                                       │ Message::TrackLoaded    │   │
│                                       │ - StemBuffers (Arc)     │   │
│                                       │ - Overview peaks        │   │
│                                       │ - Duration samples      │   │
│                                       └────────────┬────────────┘   │
│                                                    │                 │
│                                                    ▼                 │
│                                       ┌─────────────────────────┐   │
│                                       │ UI updates waveform     │   │
│                                       │ - Combined stem display │   │
│                                       │ - Remove spinner        │   │
│                                       │ - Enable transport      │   │
│                                       └─────────────────────────┘   │
│                                                                      │
│  Benefit: User sees metadata instantly, can review cue points       │
│           while audio loads in background                            │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Track Editing Workflow

```
┌─────────────────────────────────────────────────────────────────────┐
│                    CUE POINT EDITING WORKFLOW                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  LoadedTrackState (in-memory working copy)                          │
│  ├─ path: PathBuf                                                   │
│  ├─ bpm, key, lufs                                                  │
│  ├─ drop_marker, first_beat_sample                                  │
│  ├─ cue_points: Vec<CuePoint>      ◄── Edit operations              │
│  ├─ saved_loops: Vec<SavedLoop>    ◄── modify this state            │
│  ├─ stem_links: Vec<StemLink>                                       │
│  └─ beat_grid: Vec<u64>                                             │
│                                                                      │
│  Edit Operations:                                                    │
│  ├─ SetCuePoint(index, sample, label)                               │
│  ├─ DeleteCuePoint(index)                                           │
│  ├─ SetSavedLoop(index, start, end)                                 │
│  ├─ SetDropMarker(sample)                                           │
│  ├─ AdjustBpm(delta)                                                │
│  ├─ SetKey(key_string)                                              │
│  └─ SetStemLink(stem_idx, source_track, source_stem)                │
│                                                                      │
│  Save Flow:                                                          │
│  ┌──────────────┐    ┌────────────────┐    ┌──────────────────┐    │
│  │ User presses │───►│ LoadedTrack →  │───►│ DatabaseService  │    │
│  │ Ctrl+S       │    │ Track struct   │    │ .save_track()    │    │
│  └──────────────┘    └────────────────┘    └──────────────────┘    │
│                                                                      │
│  Keyboard Shortcuts:                                                 │
│  ├─ 1-8: Set cue point at playhead                                  │
│  ├─ Shift+1-8: Jump to cue point                                    │
│  ├─ D: Set drop marker                                              │
│  ├─ L: Set loop start/end                                           │
│  └─ Ctrl+S: Save all changes                                        │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Message Handler Organization

```
┌─────────────────────────────────────────────────────────────────────┐
│                    MESSAGE ROUTING (app.rs)                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │  fn update(&mut self, message: Message) -> Task<Message> {    │   │
│  │    match message {                                            │   │
│  │                                                               │   │
│  │      // Browser handlers (parameterized)                      │   │
│  │      Message::BrowserLeft(msg) =>                             │   │
│  │          self.handle_browser_left(msg),                       │   │
│  │      Message::BrowserRight(msg) =>                            │   │
│  │          self.handle_browser_right(msg),                      │   │
│  │                                                               │   │
│  │      // Drag and drop                                         │   │
│  │      Message::DragTrackStart { .. } =>                        │   │
│  │          self.handle_drag_track_start(..),                    │   │
│  │      Message::DropTracksOnPlaylist { .. } =>                  │   │
│  │          self.handle_drop_tracks_on_playlist(..),             │   │
│  │                                                               │   │
│  │      // Track operations                                      │   │
│  │      Message::TrackLoaded(result) =>                          │   │
│  │          self.handle_track_loaded(result),                    │   │
│  │      Message::SaveTrack =>                                    │   │
│  │          self.handle_save_track(),                            │   │
│  │                                                               │   │
│  │      // Audio transport                                       │   │
│  │      Message::TogglePlay =>                                   │   │
│  │          self.handle_toggle_play(),                           │   │
│  │      Message::Seek(pos) =>                                    │   │
│  │          self.handle_seek(pos),                               │   │
│  │                                                               │   │
│  │      // Simple operations inline                              │   │
│  │      Message::Tick => self.handle_tick(),                     │   │
│  │      Message::CloseExport => { .. },                          │   │
│  │    }                                                          │   │
│  │  }                                                            │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  Handler file mapping:                                               │
│  ├─ handlers/browser.rs    → BrowserLeft, BrowserRight, Drag/Drop  │
│  ├─ handlers/track_loading.rs → TrackLoaded, LoadTrack             │
│  ├─ handlers/playback.rs   → TogglePlay, Seek, SetLoop             │
│  ├─ handlers/waveform.rs   → Click, Drag on waveform               │
│  ├─ handlers/stem_links.rs → Stem linking operations               │
│  └─ handlers/tick.rs       → 60fps atomic state sync               │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### USB Export Architecture

The USB export system provides atomic per-track exports with efficient database operations.

```
┌─────────────────────────────────────────────────────────────────────┐
│                    USB EXPORT ARCHITECTURE                           │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  UI Layer (mesh-cue)                                                 │
│  ────────────────────                                                │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ ExportState                                                   │   │
│  │ ├─ phase: ExportPhase (SelectDevice → Exporting → Complete)  │   │
│  │ ├─ tracks_complete, total_tracks                             │   │
│  │ └─ sync_plan: SyncPlan                                       │   │
│  └────────────────────────────┬─────────────────────────────────┘   │
│                               │ UsbMessage (via subscription)        │
│                               ▼                                      │
│  Domain Layer (UsbManager)                                           │
│  ─────────────────────────                                           │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ UsbManager (background thread)                                │   │
│  │ ├─ Receives UsbCommand::StartExport                          │   │
│  │ ├─ Creates ExportService (thread pool)                       │   │
│  │ └─ Forwards ExportProgress → UsbMessage                      │   │
│  └────────────────────────────┬─────────────────────────────────┘   │
│                               │                                      │
│                               ▼                                      │
│  Export Service Layer (mesh-core/export/)                            │
│  ────────────────────────────────────────                            │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ ExportService                                                 │   │
│  │ ├─ thread_pool: rayon::ThreadPool (4 threads)                │   │
│  │ ├─ cancel_flag: Arc<AtomicBool>                              │   │
│  │ └─ start_export() → Receiver<ExportProgress>                 │   │
│  │                                                               │   │
│  │ Per-Thread Worker (atomic export per track):                  │   │
│  │ ┌─────────────────────────────────────────────────────────┐  │   │
│  │ │ 1. Copy WAV with verification (3 retries)               │  │   │
│  │ │ 2. Sync track to USB database (batch inserts)           │  │   │
│  │ │ 3. Send ExportProgress::TrackComplete                   │  │   │
│  │ └─────────────────────────────────────────────────────────┘  │   │
│  └────────────────────────────┬─────────────────────────────────┘   │
│                               │                                      │
│                               ▼                                      │
│  Database Layer (mesh-core/db/)                                      │
│  ──────────────────────────────                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ DatabaseService::sync_track_atomic()                          │   │
│  │                                                               │   │
│  │ Uses BatchQuery for efficient bulk inserts:                   │   │
│  │ ├─ batch_insert_cue_points()    (1 query for N cues)         │   │
│  │ ├─ batch_insert_saved_loops()   (1 query for N loops)        │   │
│  │ ├─ batch_insert_stem_links()    (1 query for N links)        │   │
│  │ └─ batch_delete_track_metadata() (1 query)                   │   │
│  │                                                               │   │
│  │ Total: ~5 queries per track vs 18+ with individual inserts   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Export Message Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│                    EXPORT MESSAGE FLOW                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ExportProgress (mesh-core/export/)     UsbMessage (mesh-core/usb/) │
│  ─────────────────────────────────────  ─────────────────────────── │
│                                                                      │
│  ExportProgress::Started          ───►  UsbMessage::ExportStarted   │
│    { total_tracks, total_bytes }          { total_tracks, ... }     │
│                                                                      │
│  ExportProgress::TrackStarted     ───►  UsbMessage::ExportTrackStarted│
│    { filename, track_index }              { filename, track_index } │
│                                                                      │
│  ExportProgress::TrackComplete    ───►  UsbMessage::ExportTrackComplete│
│    { filename, track_index,               { filename, track_index,  │
│      total_tracks, bytes_complete,          total_tracks, ... }     │
│      total_bytes }                                                   │
│                                                                      │
│  ExportProgress::TrackFailed      ───►  UsbMessage::ExportTrackFailed│
│    { filename, track_index, error }       { filename, ... error }   │
│                                                                      │
│  ExportProgress::Complete         ───►  UsbMessage::ExportComplete  │
│    { duration, tracks_exported,           { duration, ... }         │
│      failed_files }                                                  │
│                                                                      │
│  ExportProgress::Cancelled        ───►  UsbMessage::ExportCancelled │
│                                                                      │
│  Key Insight: Progress is only sent AFTER both WAV copy AND         │
│  database sync complete. This ensures the UI progress bar           │
│  accurately reflects tracks that are fully exported.                │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Batch Insert Pattern (CozoDB)

```
┌─────────────────────────────────────────────────────────────────────┐
│                    BATCH INSERT OPTIMIZATION                         │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Before (18+ queries per track):                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ for cue in cue_points:           # 8 queries                  │   │
│  │     INSERT INTO cue_points ...                                │   │
│  │ for loop in saved_loops:         # 8 queries                  │   │
│  │     INSERT INTO saved_loops ...                               │   │
│  │ for link in stem_links:          # 4 queries                  │   │
│  │     INSERT INTO stem_links ...                                │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  After (5 queries per track):                                        │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ 1. Upsert track row                                           │   │
│  │ 2. DELETE FROM cue_points WHERE track_id = ?                  │   │
│  │    DELETE FROM saved_loops WHERE track_id = ?                 │   │
│  │    DELETE FROM stem_links WHERE track_id = ?                  │   │
│  │ 3. ?[...] <- $cues :put cue_points {...}      # 1 batch query │   │
│  │ 4. ?[...] <- $loops :put saved_loops {...}    # 1 batch query │   │
│  │ 5. ?[...] <- $links :put stem_links {...}     # 1 batch query │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  CozoDB Batch Syntax:                                                │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ ?[track_id, index, sample_position, label, color] <- $rows    │   │
│  │ :put cue_points {track_id, index => sample_position, ...}     │   │
│  │                                                               │   │
│  │ Where $rows is a Vec<Vec<DataValue>> passed as parameter      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  Performance: ~70% reduction in DB operations during export         │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Stem Link ID Remapping

```
┌─────────────────────────────────────────────────────────────────────┐
│                    STEM LINK REMAPPING                               │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  Problem: Stem links reference tracks by database ID, but IDs       │
│           differ between local and USB databases.                    │
│                                                                      │
│  Local DB:                      USB DB:                              │
│  ├─ track_id: 42               ├─ track_id: 7                       │
│  │  path: ".../song.stems"     │  path: ".../tracks/song.wav"       │
│  └─ stem_link → source: 42     └─ stem_link → source: ???           │
│                                                                      │
│  Solution (in sync_track_atomic):                                    │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ fn remap_stem_links_for_export(                               │   │
│  │     links: &[StemLink],                                       │   │
│  │     source_db: &DatabaseService,  // local DB                 │   │
│  │ ) -> Vec<StemLink> {                                          │   │
│  │                                                               │   │
│  │     for link in links:                                        │   │
│  │         // 1. Get source track path from local DB             │   │
│  │         local_track = source_db.get_track(link.source_id)     │   │
│  │         filename = local_track.path.file_name()               │   │
│  │                                                               │   │
│  │         // 2. Find matching track in USB DB by filename       │   │
│  │         usb_track = self.get_track_by_path(                   │   │
│  │             "{usb_root}/tracks/{filename}"                    │   │
│  │         )                                                     │   │
│  │                                                               │   │
│  │         // 3. Use USB track ID for the remapped link          │   │
│  │         remapped_links.push(StemLink {                        │   │
│  │             source_track_id: usb_track.id,                    │   │
│  │             ...link                                           │   │
│  │         })                                                    │   │
│  │ }                                                             │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```
