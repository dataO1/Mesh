# Multiband Effect Container UI - Implementation Plan

## Overview

Replace the current per-stem effect chain with a **MultibandHost** (renamed from MultibandHost) as the universal container. Every stem gets a multiband container that:
- Defaults to 1 band (passthrough mode - behaves like current system)
- Allows users to add bands and define crossover frequencies
- Accepts **any effect implementing the `Effect` trait** (PD, CLAP, native Rust, future types)
- Shows all effects in a lane-based UI with per-band plugin chains

## Design Principles

1. **Effect-agnostic**: The multiband container works with `Box<dyn Effect>`, not specific effect types
2. **Unified interface**: PD effects, CLAP plugins, and native effects all appear in the same picker
3. **Extensible**: Future effect types (VST3, LV2, etc.) automatically work if they implement `Effect`

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          MULTIBAND EDITOR UI                             │
├─────────────────────────────────────────────────────────────────────────┤
│  [Load Preset ▾]  [Save Preset]           Deck 1 - Drums         [×]   │
├─────────────────────────────────────────────────────────────────────────┤
│  Crossover Visualization (20Hz ────────────────────────────── 20kHz)   │
│  ════════════════╪════════════════════╪══════════════════════════════  │
│                200Hz                 2kHz                               │
│                  ↕ drag                ↕ drag                           │
├─────────────────────────────────────────────────────────────────────────┤
│ Band 1: 20-200Hz (Sub)     [S] [M]  ────────────────────────────────── │
│  ┌──────────────┐ ┌──────────────┐                                     │
│  │ Compressor   │ │              │  [+]                                │
│  │ ○○○○○○○○    │ │              │                                     │
│  │ Thr Rat Atk │ │              │                                     │
│  └──────────────┘ └──────────────┘                                     │
├─────────────────────────────────────────────────────────────────────────┤
│ Band 2: 200Hz-2kHz (Mid)   [S] [M]  ────────────────────────────────── │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐                    │
│  │ Saturation   │ │ EQ           │ │              │  [+]               │
│  │ ○○○○○○○○    │ │ ○○○○○○○○    │ │              │                    │
│  │ Drv Mix Tone│ │ Lo Mid Hi   │ │              │                    │
│  └──────────────┘ └──────────────┘ └──────────────┘                    │
├─────────────────────────────────────────────────────────────────────────┤
│ Band 3: 2kHz-20kHz (High)  [S] [M]  ────────────────────────────────── │
│  ┌──────────────┐                                                      │
│  │ De-esser     │  [+]                                                 │
│  │ ○○○○○○○○    │                                                      │
│  │ Freq Thr Rat│                                                      │
│  └──────────────┘                                                      │
├─────────────────────────────────────────────────────────────────────────┤
│                              [+ Add Band]                               │
├─────────────────────────────────────────────────────────────────────────┤
│ Macros:  [1:Attack] [2:Drive] [3:______] [4:______]                    │
│          [5:______] [6:______] [7:______] [8:______]                   │
└─────────────────────────────────────────────────────────────────────────┘
```

## Component Hierarchy

```
mesh-widgets/                          # NEW CRATE (reusable for mesh-cue)
├── Cargo.toml
└── src/
    ├── lib.rs
    └── multiband/
        ├── mod.rs                     # MultibandEditor widget
        ├── state.rs                   # MultibandEditorState
        ├── message.rs                 # MultibandEditorMessage enum
        ├── crossover_bar.rs           # Draggable frequency dividers
        ├── band_lane.rs               # Single band row with effects
        ├── effect_card.rs             # Individual effect with 8 knobs (any Effect type)
        └── macro_bar.rs               # 8 macro knobs at bottom

mesh-core/src/effect/
├── multiband.rs                       # MOVE from clap/ - now effect-agnostic
└── preset.rs                          # NEW: MultibandPreset save/load

Note: MultibandHost moves from clap/ to effect/ since it's no longer CLAP-specific.
It holds Vec<Box<dyn Effect>> per band, accepting PD, CLAP, or any Effect impl.
```

## Implementation Phases

### Phase 1: Backend Enhancements (mesh-core)

**1.1 Enhance MultibandHost API**
- Add `get_band_count()`, `get_crossover_freqs()` accessor methods
- Add `get_band_effects(band_idx)` to enumerate effects per band
- Add `get_effect_info(band_idx, effect_idx)` for UI display
- Add `get_band_state(idx) -> BandState { muted, soloed, gain }`

**1.2 Create Preset System**
```rust
// crates/mesh-core/src/effect/preset.rs
#[derive(Serialize, Deserialize)]
pub struct MultibandPreset {
    pub name: String,
    pub crossover_freqs: Vec<f32>,
    pub bands: Vec<BandPreset>,
    pub macros: Vec<MacroPreset>,
}

#[derive(Serialize, Deserialize)]
pub struct BandPreset {
    pub gain: f32,
    pub muted: bool,
    pub effects: Vec<EffectPreset>,
}

/// Effect preset - works for ANY effect type implementing Effect trait
#[derive(Serialize, Deserialize)]
pub struct EffectPreset {
    pub effect_id: String,          // CLAP ID, PD folder name, or native effect ID
    pub source: EffectSource,       // Pd, Clap, or Native (extensible enum)
    pub params: [f32; 8],           // Normalized parameter values
    pub bypassed: bool,
}

#[derive(Serialize, Deserialize)]
pub enum EffectSource {
    Pd,                             // Pure Data patch
    Clap,                           // CLAP plugin
    Native,                         // Built-in Rust effect
    // Future: Vst3, Lv2, etc.
}
```

The preset system stores `EffectSource` to know which manager to use when recreating:
- `Pd` → `pd_manager.create_effect(id)`
- `Clap` → `clap_manager.create_effect(id)`
- `Native` → `native_manager.create_effect(id)`

**1.3 Add Preset Directory**
- Location: `~/Music/mesh-collection/presets/multiband/`
- Format: JSON files with `.mbpreset` extension

### Phase 2: Widget Crate (mesh-widgets)

**2.1 Create mesh-widgets crate**
```toml
# crates/mesh-widgets/Cargo.toml
[package]
name = "mesh-widgets"
version = "0.1.0"

[dependencies]
iced = { workspace = true }
mesh-core = { path = "../mesh-core" }
```

**2.2 MultibandEditorState**
```rust
pub struct MultibandEditorState {
    /// Which deck/stem this editor is for
    pub deck: usize,
    pub stem: usize,

    /// Crossover frequencies (N-1 for N bands)
    pub crossover_freqs: Vec<f32>,

    /// Dragging state for crossover dividers
    pub dragging_crossover: Option<usize>,

    /// Per-band state
    pub bands: Vec<BandUiState>,

    /// Currently selected effect (for parameter focus)
    pub selected_effect: Option<(usize, usize)>, // (band_idx, effect_idx)

    /// Macro names and values
    pub macros: [(String, f32); 8],

    /// Preset browser state
    pub preset_browser_open: bool,
    pub available_presets: Vec<String>,
}

pub struct BandUiState {
    pub name: String,           // "Sub", "Low", "Mid", "High", etc.
    pub freq_low: f32,          // Hz
    pub freq_high: f32,         // Hz
    pub muted: bool,
    pub soloed: bool,
    pub effects: Vec<EffectUiState>,
}

/// UI state for any effect (PD, CLAP, native, etc.)
pub struct EffectUiState {
    pub id: String,                 // Effect identifier for recreation
    pub name: String,               // Display name from effect.info().name
    pub source: EffectSource,       // Pd, Clap, or Native
    pub bypassed: bool,
    pub param_names: [String; 8],   // From effect.info().params
    pub param_values: [f32; 8],     // Current normalized values
}
```

**2.3 MultibandEditorMessage**
```rust
pub enum MultibandEditorMessage {
    // Crossover
    StartDragCrossover(usize),
    DragCrossover(f32),          // New frequency in Hz
    EndDragCrossover,

    // Bands
    AddBand,
    RemoveBand(usize),
    SetBandMute(usize, bool),
    SetBandSolo(usize, bool),

    // Effects within bands
    OpenEffectPicker(usize),     // band_idx - opens picker to add effect
    RemoveEffect(usize, usize),  // band_idx, effect_idx
    BypassEffect(usize, usize, bool),
    SelectEffect(usize, usize),  // Focus for parameter editing
    SetEffectParam(usize, usize, usize, f32), // band, effect, param, value

    // Macros
    SetMacro(usize, f32),
    RenameMacro(usize, String),
    OpenMacroMapper(usize),      // Opens dialog to map macro to params

    // Presets
    OpenPresetBrowser,
    LoadPreset(String),
    SavePreset(String),
    ClosePresetBrowser,

    // Modal
    Close,
}
```

**2.4 CrossoverBar Widget**
- Horizontal bar showing 20Hz to 20kHz (log scale)
- Draggable divider lines at crossover frequencies
- Band labels between dividers
- Mouse interaction: drag to adjust crossover

**2.5 BandLane Widget**
- Horizontal row for one frequency band
- Header: band name, freq range, [S]olo [M]ute buttons
- Content: horizontal list of PluginCard widgets
- Footer: [+] button to add effect

**2.6 PluginCard Widget**
- Compact card showing one effect
- Header: effect name, bypass toggle, remove [×]
- Content: 8 small rotary knobs in 2 rows of 4
- Knob labels underneath

**2.7 MacroBar Widget**
- 8 macro knobs in a row
- Editable name labels
- Click knob to open macro mapping dialog

### Phase 3: Integration (mesh-player)

**3.1 Replace Effect Chain Display**
- Current: `[Effect1●]──[Effect2◯]──[+]`
- New: `[Multiband ▾]` button that opens MultibandEditor

**3.2 Modify DeckView**
```rust
// In deck_view.rs
pub struct DeckView {
    // Replace:
    // stem_effect_names: [Vec<String>; 4],
    // stem_effect_bypassed: [Vec<bool>; 4],

    // With:
    multiband_editors: [MultibandEditorState; 4], // One per stem
}
```

**3.3 Update App Message Routing**
- Add `MultibandEditorMessage` to main Message enum
- Route to appropriate deck/stem's MultibandHost
- Sync UI state after backend changes

**3.4 Engine Command Additions**
```rust
// In command.rs
pub enum EngineCommand {
    // Existing...

    // New multiband commands
    SetCrossoverFreq { deck: usize, stem: Stem, crossover_idx: usize, freq: f32 },
    AddBandEffect { deck: usize, stem: Stem, band_idx: usize, effect: Box<dyn Effect> },
    RemoveBandEffect { deck: usize, stem: Stem, band_idx: usize, effect_idx: usize },
    SetBandMute { deck: usize, stem: Stem, band_idx: usize, muted: bool },
    SetBandSolo { deck: usize, stem: Stem, band_idx: usize, soloed: bool },
    SetBandEffectParam { deck: usize, stem: Stem, band_idx: usize, effect_idx: usize, param_idx: usize, value: f32 },
    SetMacro { deck: usize, stem: Stem, macro_idx: usize, value: f32 },
    AddBand { deck: usize, stem: Stem },
    RemoveBand { deck: usize, stem: Stem, band_idx: usize },
}
```

**3.5 Initialize Stems with Multiband Wrapper**
- When deck loads track, create MultibandHost per stem
- Default: 1 band, no crossover, empty effect chain
- Store reference for UI sync

### Phase 4: Preset System

**4.1 Preset Save**
- Serialize MultibandHost state to MultibandPreset
- Write to `~/Music/mesh-collection/presets/multiband/{name}.mbpreset`

**4.2 Preset Load**
- Parse JSON file
- Create MultibandHost from preset
- Load each effect (PD or CLAP) with saved parameters

**4.3 Preset Browser**
- Simple modal with list of available presets
- Preview info (band count, effects)
- Load/Delete buttons

## File Changes Summary

### New Files
```
crates/mesh-widgets/Cargo.toml
crates/mesh-widgets/src/lib.rs
crates/mesh-widgets/src/multiband/mod.rs
crates/mesh-widgets/src/multiband/state.rs
crates/mesh-widgets/src/multiband/message.rs
crates/mesh-widgets/src/multiband/crossover_bar.rs
crates/mesh-widgets/src/multiband/band_lane.rs
crates/mesh-widgets/src/multiband/effect_card.rs
crates/mesh-widgets/src/multiband/macro_bar.rs
crates/mesh-core/src/effect/multiband.rs      # MOVED from clap/multiband.rs
crates/mesh-core/src/effect/preset.rs         # Effect-agnostic preset system
```

### Modified Files
```
Cargo.toml                                    # Add mesh-widgets workspace member
crates/mesh-core/src/effect/mod.rs            # Export multiband, preset modules
crates/mesh-core/src/clap/mod.rs              # Remove multiband (moved to effect/)
crates/mesh-core/src/engine/command.rs        # Add multiband commands
crates/mesh-core/src/engine/engine.rs         # Handle multiband commands
crates/mesh-player/Cargo.toml                 # Depend on mesh-widgets
crates/mesh-player/src/ui/deck_view.rs        # Replace effect chain with multiband
crates/mesh-player/src/ui/app.rs              # Route multiband messages
crates/mesh-player/src/ui/message.rs          # Add multiband message variant
crates/mesh-player/src/domain/mod.rs          # Add multiband domain methods
```

## User Interaction Flow

### Adding an Effect
1. User clicks `[Multiband ▾]` on stem
2. MultibandEditor modal opens (default: 1 band)
3. User clicks `[+]` in band lane
4. Effect picker opens with unified list:
   - PD effects (from `~/Music/mesh-collection/effects/`)
   - CLAP plugins (from `~/.clap/`, `/usr/lib/clap/`)
   - Native effects (built-in filters, etc.)
   - Filter buttons: [All] [PD] [CLAP] [Native]
5. User selects any effect type
6. Effect card appears in band lane with 8 knobs (from `effect.info().params`)

### Adding a Band
1. User clicks `[+ Add Band]` at bottom
2. New band lane appears
3. Crossover divider appears at default frequency (e.g., midpoint)
4. User drags divider to set crossover frequency

### Saving a Preset
1. User clicks `[Save Preset]`
2. Name input dialog appears
3. User enters name, clicks Save
4. Preset saved to `~/Music/mesh-collection/presets/multiband/`

### Loading a Preset
1. User clicks `[Load Preset ▾]`
2. Dropdown/modal shows available presets
3. User clicks preset name
4. All bands, effects, and mappings restored

## Open Questions

1. **Crossover Implementation**: Should we bundle a built-in crossover or use external plugin?
   - Recommendation: Built-in Linkwitz-Riley crossover for reliability (no external dependency)
   - Falls back gracefully if user wants to use LSP Crossover instead

2. **Max Bands**: Currently 8, is this sufficient?
   - Recommendation: Keep at 8, matches pro tools like Multipass

3. **Macro Mapping UI**: How to map macros to parameters?
   - Recommendation: Click macro → Click parameter on effect card → Mapped

4. **Effect Reordering**: Drag-and-drop within band?
   - Recommendation: Phase 2 feature, not MVP

5. **Mixed Effect Types**: Can a band have both PD and CLAP effects?
   - Recommendation: Yes! Each band is `Vec<Box<dyn Effect>>`, effect type doesn't matter

## Estimated Effort

| Phase | Description | Effort |
|-------|-------------|--------|
| 1 | Backend Enhancements | 2-3 hours |
| 2 | Widget Crate | 6-8 hours |
| 3 | Integration | 4-5 hours |
| 4 | Preset System | 2-3 hours |
| **Total** | | **14-19 hours** |

## Success Criteria

- [ ] User can open multiband editor for any stem
- [ ] User can add/remove frequency bands with draggable crossovers
- [ ] User can add PD or CLAP effects to any band
- [ ] User can adjust effect parameters via rotary knobs
- [ ] User can save/load multiband presets
- [ ] Single-band mode (default) behaves like current effect chain
- [ ] Widget is reusable for mesh-cue integration
