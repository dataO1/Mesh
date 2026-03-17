# Mesh Effects System

Mesh provides a per-stem multiband effect system. Each of the 4 stems on each
deck has its own independent effect chain. Effects can be split across frequency
bands for precise, surgical control over individual parts of a mix.

This document covers the effect types available, how multiband processing works,
how to install and create effects, and how to manage effect presets.

---

## Table of Contents

- [Effect Types](#effect-types)
  - [Built-in Effects](#built-in-effects)
  - [CLAP Plugins](#clap-plugins)
  - [Pure Data Patches](#pure-data-patches)
- [Multiband Processing](#multiband-processing)
  - [How Bands Work](#how-bands-work)
  - [Per-Band Controls](#per-band-controls)
- [Macro Knobs](#macro-knobs)
- [Latency Compensation](#latency-compensation)
- [Installing CLAP Plugins](#installing-clap-plugins)
  - [Plugin Locations](#plugin-locations)
  - [Tested Plugins](#tested-plugins)
  - [Bundled Dependencies (Portable Setup)](#bundled-dependencies-portable-setup)
  - [CLAP Limitations](#clap-limitations)
- [Creating Pure Data Effects](#creating-pure-data-effects)
  - [Directory Layout](#directory-layout)
  - [metadata.json Reference](#metadatajson-reference)
  - [Writing the PD Patch](#writing-the-pd-patch)
  - [PD Externals](#pd-externals)
  - [RAVE Neural Effects](#rave-neural-effects)
  - [PD Limitations](#pd-limitations)
- [Effect Presets](#effect-presets)
  - [Stem Presets](#stem-presets)
  - [Deck Presets](#deck-presets)
  - [Global FX Presets](#global-fx-presets)
- [Using Effects in Performance (mesh-player)](#using-effects-in-performance-mesh-player)
- [Using Effects in Preparation (mesh-cue)](#using-effects-in-preparation-mesh-cue)

---

## Effect Types

### Built-in Effects

These effects are implemented natively in Rust. They have zero plugin overhead
and minimal latency.

| Effect | Description | Parameters |
|--------|-------------|------------|
| DJ Filter | Sweepable highpass/lowpass combo (60 Hz -- 20 kHz). In the mixer, the filter knob sweeps from lowpass (left) through flat (center) to highpass (right). Also available as a per-stem effect in the multiband editor. | Cutoff, Resonance |
| Stereo Delay | Tempo-synced delay with optional ping-pong mode. Up to 2 seconds of delay time. | Time, Feedback, Mix, Ping-Pong |
| Reverb | Algorithmic reverb using 8 comb filters and 4 allpass filters for dense, diffuse reflections. | Room Size, Damping, Width, Mix |
| Gain | Simple volume scaling. Useful for level matching between bands or boosting/cutting specific frequency ranges. | Gain |

### CLAP Plugins

CLAP (CLever Audio Plugin) is a modern, open-source audio plugin standard. It is
the successor to VST and LV2 for new plugin development, and many professional
audio plugins now ship in CLAP format.

Mesh scans standard directories for CLAP plugins at startup and makes them
available in the effect picker, organized by category.

See [Installing CLAP Plugins](#installing-clap-plugins) for setup instructions.

### Pure Data Patches

Pure Data (PD) is a visual programming language for audio. You create effects by
connecting objects with virtual cables in a graphical editor. Mesh embeds the PD
runtime and can run your patches as live stem effects.

PD effects are especially useful for experimental processing, custom filter
designs, or neural audio synthesis via the nn~ external.

See [Creating Pure Data Effects](#creating-pure-data-effects) for a full
walkthrough.

---

## Multiband Processing

### How Bands Work

Each stem's effect chain is wrapped in a multiband container. By default, there
is a single band that processes the full frequency range. You can split the
signal into up to 8 frequency bands using Linkwitz-Riley 24 dB/octave
crossover filters, which sum back to unity gain with no phase issues.

- **Single-band** (default): No frequency splitting. Effects process the entire
  stem signal.
- **Multi-band** (2--8 bands): The signal is split by frequency before entering
  the effect chains, then recombined after processing.

When adding bands, the crossover frequency is automatically placed at the
geometric mean between existing bands. You can drag the crossover dividers in the
multiband editor to set exact split points anywhere from 20 Hz to 20 kHz.

Bands are named by frequency range: Sub, Bass, Low-Mid, Mid, High-Mid, Presence,
Air.

### Per-Band Controls

Each band provides:

- **Effect chain**: Up to 8 effects per band, processed in series. Drag effects
  to reorder them.
- **Chain dry/wet knob**: Blends between the unprocessed band signal and the
  output of the effect chain.
- **Solo (S)**: Audition only this band.
- **Mute (M)**: Silence this band.
- **Add effect (+)**: Open the effect picker to insert a new effect into the
  band's chain.

In addition to per-band chains, the multiband container has pre-FX and post-FX
slots that process the full signal before splitting and after recombination.

---

## Macro Knobs

Each deck has 4 macro knobs that provide one-knob control over multiple effect
parameters at once. This is where multiband effects become truly powerful for
live performance.

Each macro knob can map up to 8 parameters across any stem on that deck. The
target types are:

| Target Type | What It Controls |
|-------------|------------------|
| Effect Parameter | A specific knob on an effect (cutoff, feedback, etc.) |
| Effect Dry/Wet | The dry/wet blend of a single effect |
| Chain Dry/Wet | The dry/wet blend of an entire band's effect chain |
| Global Dry/Wet | The overall dry/wet of the stem's multiband processor |

Macros are configured in the multiband editor and saved as part of deck presets.
They can be controlled via the UI sliders or via MIDI (FX macro knobs on your
controller).

**Example use case:** Map a single macro to simultaneously control reverb wet on
the vocals stem, filter cutoff on the bass stem, and delay feedback on the
melody stem. Moving one knob creates a complex transition effect.

---

## Latency Compensation

Mesh automatically measures and compensates for effect latency at multiple
levels:

- **Per-effect**: Latency is measured when each plugin is loaded.
- **Per-chain**: If effects within a band's chain have different latencies, the
  dry signal path is delayed to match.
- **Cross-band**: If different bands within a stem have different total
  latencies, the faster bands are delayed so all bands stay aligned.
- **Cross-stem**: If one stem's effect chain has more total latency than another,
  the other stems are delayed to keep everything in sync.
- **Dynamic updates**: When a CLAP plugin changes its latency at runtime (for
  example, LSP plugins adjusting a lookahead parameter), mesh detects the change
  and re-compensates automatically.

The maximum compensation is 8000 samples (~165 ms at 48 kHz).

**In practice:** This means you can add a high-latency mastering compressor to
the vocals without the vocals drifting out of sync with the drums, bass, or
other stems.

---

## Installing CLAP Plugins

### Plugin Locations

Mesh scans the following directories for `.clap` plugin bundles at startup:

```
~/.clap/                      # User plugins (recommended)
/usr/lib/clap/                # System-wide plugins
/usr/local/lib/clap/          # Locally installed plugins
```

To install a CLAP plugin:

1. Download the `.clap` file from the plugin developer's site.
2. Place it in `~/.clap/` (create the directory if it does not exist).
3. Restart mesh. The plugin will appear in the effect picker under its category.

### Tested Plugins

The following CLAP plugins have been tested and work well with mesh:

- **LSP Plugins** -- Compressors, EQs, gates, reverbs, delays. Professional
  quality, open-source. Available from your distribution's package manager or
  [lsp-plug.in](https://lsp-plug.in/).
- **Dragonfly Reverb** -- Algorithmic room, hall, and plate reverbs.
- **Airwindows** -- Hundreds of boutique effects including saturation, tape
  emulation, and vintage EQ.
- **BYOD** -- Guitar amp simulation and distortion.
- **ChowTapeModel** -- Analog tape saturation.

### Bundled Dependencies (Portable Setup)

On some systems (especially NixOS or other non-FHS distributions), CLAP plugins
may have missing shared library dependencies. You can bundle these dependencies
alongside the plugin for portability.

Mesh automatically adds a `lib/` subdirectory next to your `.clap` files to
`LD_LIBRARY_PATH` when loading plugins. To set this up:

1. Check for missing dependencies:
   ```bash
   ldd ~/.clap/my-plugin.clap | grep "not found"
   ```

2. Copy the missing libraries into a `lib/` folder alongside your plugins:
   ```
   ~/.clap/
   ├── my-plugin.clap
   └── lib/
       ├── libsndfile.so.1
       └── libcairo.so.2
   ```

3. Patch the RPATH so libraries can find each other:
   ```bash
   patchelf --set-rpath '$ORIGIN' ~/.clap/lib/*.so*
   patchelf --set-rpath '$ORIGIN/lib' ~/.clap/my-plugin.clap
   ```

### CLAP Limitations

- **GUI windows**: CLAP plugin GUIs are supported in mesh-cue (the preparation
  app) but not in mesh-player (the performance app). In mesh-player, effect
  parameters are controlled via the knob interface.
- **Parameter visibility**: Up to 8 parameters per effect are exposed in the
  mesh UI. The plugin's internal parameters still function, but only the first 8
  are visible as knobs.
- **Latency reporting**: Some plugins do not report correct latency at
  activation time. Mesh handles this via restart detection -- when a plugin
  requests a restart to update its latency, mesh performs the
  deactivate/reactivate cycle and re-queries the latency automatically.

---

## Creating Pure Data Effects

### Directory Layout

PD effects live inside your mesh collection directory:

```
~/Music/mesh-collection/effects/pd/
├── my-effect/
│   ├── metadata.json        # Effect configuration (required)
│   └── my-effect.pd         # Pure Data patch (required, filename must match folder)
├── another-effect/
│   ├── metadata.json
│   └── another-effect.pd
├── externals/               # Shared PD external objects
│   ├── nn~.pd_linux
│   └── lib/
│       └── libtorch.so
└── models/                  # ML models for nn~ and similar externals
    └── rave-model.ts
```

Each effect gets its own folder. The `.pd` filename must match the folder name.

### metadata.json Reference

Every PD effect requires a `metadata.json` file that tells mesh about the
effect's name, category, parameters, and requirements.

**Minimal example:**

```json
{
  "name": "My Filter",
  "category": "Filter",
  "latency_samples": 0
}
```

**Full example with all fields:**

```json
{
  "name": "Custom Resonant Filter",
  "category": "Filter",
  "author": "Your Name",
  "version": "1.0.0",
  "description": "A resonant lowpass filter with drive",
  "latency_samples": 0,
  "sample_rate": 48000,
  "requires_externals": [],
  "params": [
    {
      "name": "Cutoff",
      "default": 0.7,
      "min": 20.0,
      "max": 20000.0,
      "unit": "Hz"
    },
    {
      "name": "Resonance",
      "default": 0.3,
      "min": 0.0,
      "max": 1.0,
      "unit": ""
    },
    {
      "name": "Drive",
      "default": 0.0,
      "min": 0.0,
      "max": 10.0,
      "unit": "dB"
    }
  ]
}
```

**Field reference:**

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Display name in the effect picker |
| `category` | Yes | Category for grouping (e.g., "Filter", "Delay", "Neural", "Utility") |
| `latency_samples` | Yes | Fixed latency in samples. Set to 0 if your effect has no lookahead. |
| `author` | No | Effect author |
| `version` | No | Version string |
| `description` | No | Short description shown in the effect picker |
| `sample_rate` | No | Sample rate the `latency_samples` value refers to. Defaults to 48000. Mesh scales the latency automatically if running at a different rate. |
| `requires_externals` | No | List of PD external objects the patch needs (e.g., `["nn~"]`). These must be present in `effects/pd/externals/`. |
| `params` | No | Up to 8 parameter definitions (see below) |

**Parameter fields:**

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | -- | Parameter display name |
| `default` | No | 0.5 | Default value, normalized 0.0 to 1.0 |
| `min` | No | -- | Minimum display value (for labeling only; PD receives the normalized 0--1 value) |
| `max` | No | -- | Maximum display value (for labeling only) |
| `unit` | No | -- | Unit label (e.g., "Hz", "ms", "%", "dB") |

**Important:** The `default` value must be between 0.0 and 1.0. The `min` and
`max` fields are for display purposes only -- PD always receives the normalized
value. Your patch is responsible for scaling the 0--1 range to the actual
parameter range.

### Writing the PD Patch

Your PD patch receives stereo audio and parameter values through standard PD
mechanisms:

- **Audio input**: `[adc~ 1]` (left) and `[adc~ 2]` (right)
- **Audio output**: `[dac~ 1]` (left) and `[dac~ 2]` (right)
- **Parameters**: Received via `[r $0-param0]` through `[r $0-param7]`
  (normalized 0.0--1.0 values)
- **Bypass**: `[r $0-bypass]` sends 0 when the effect is active, allowing you to
  implement bypass gating

The `$0-` prefix ensures each effect instance is isolated (PD assigns a unique
number per patch instance).

**Getting started:** Copy the template from `examples/pd-effects/_template/` in
the mesh repository. It provides the complete I/O structure with audio routing,
parameter receivers, and bypass gating already wired up. You only need to fill
in the `[pd procesing]` subpatch with your DSP logic.

A working RAVE neural percussion example is also included at
`examples/pd-effects/rave-percussion/`.

### PD Externals

If your patch uses external objects (anything not built into vanilla PD), place
them in the shared externals directory:

```
~/Music/mesh-collection/effects/pd/externals/
├── nn~.pd_linux         # The external object
└── lib/
    └── libtorch.so      # Runtime dependencies the external needs
```

List any required externals in your `metadata.json` under `requires_externals`.
Mesh checks for their presence before loading the effect and will report a clear
error if an external is missing.

### RAVE Neural Effects

RAVE (Real-time Audio Variational autoEncoder) enables neural network audio
models to run as live effects. This allows for effects like timbre transfer,
neural percussion transformation, and generative audio processing -- all running
in real time on individual stems.

To use RAVE effects:

1. Build the nn~ PD external (if building mesh from source):
   ```bash
   nix run .#build-nn-tilde
   ```
   Place the resulting `nn~.pd_linux` in `effects/pd/externals/`.

2. Obtain a RAVE TorchScript model (`.ts` file) and place it in
   `effects/pd/models/`.

3. Create a PD patch that loads the model using `[nn~ model.ts]`.

See `examples/pd-effects/rave-percussion/` for a complete working example.

### PD Limitations

- Each deck gets its own isolated PD instance (via `$0-` prefix scoping).
- PD effects add some CPU overhead compared to native built-in effects.
- RAVE neural models are CPU/GPU-intensive. They work well on desktop hardware
  but may struggle on embedded or low-power systems.
- Maximum of 8 parameters per effect.

---

## Effect Presets

Mesh uses YAML preset files that are human-readable and can be edited by hand.
Presets are created in the multiband editor in mesh-cue, or by copying and
modifying existing preset files.

### Stem Presets

A stem preset saves the complete effect chain for a single stem.

- **Location**: `presets/stems/*.yaml`
- **Contains**: Band configuration, crossover frequencies, the effects in each
  band with their parameter values, and dry/wet settings.

### Deck Presets

A deck preset saves the complete effect setup for all 4 stems on a deck, plus
the macro knob configurations.

- **Location**: `presets/decks/*.yaml`
- **Contains**: References to stem presets by name (not copies of them), plus the
  4 macro knob mappings.

Because deck presets reference stem presets by name, you can reuse the same stem
preset across different deck configurations. Changing a stem preset file updates
every deck preset that references it.

### Global FX Presets

Global FX presets apply a deck preset to all 4 decks simultaneously. This is
useful for applying a consistent effect setup across your entire mix.

- Accessible from the header dropdown in mesh-player.
- Scrollable via the MIDI FX encoder.
- The "No FX" option removes all effects from all decks.

---

## Using Effects in Performance (mesh-player)

In mesh-player, effects are designed for fast, hands-on control during a live
set:

1. **DJ Filter**: Always available per-deck via the mixer filter knob. Turn left
   for lowpass, center for flat, right for highpass.

2. **Global FX preset**: Select from the header dropdown to apply an effect
   setup to all decks at once. Navigate with the MIDI FX encoder.

3. **Macro knobs**: 4 per deck, pre-configured in your deck preset. Control
   them with MIDI knobs or the on-screen sliders. Each macro can sweep multiple
   parameters across multiple stems simultaneously.

4. **Individual effect control**: Open the multiband editor (gear icon on a deck)
   for direct access to every effect parameter.

---

## Using Effects in Preparation (mesh-cue)

In mesh-cue, you have full access to the multiband editor for building and
fine-tuning effect chains:

1. Open the multiband editor for any stem on a deck.
2. Add frequency bands by splitting the signal at crossover points.
3. Add effects to any band by clicking the **+** button and browsing the effect
   picker (built-in effects, CLAP plugins, and PD patches are all listed).
4. Drag effects to reorder them within a band's chain.
5. Adjust effect parameters using the knob interface, or open the CLAP plugin's
   native GUI window for full control.
6. Set up macro knob mappings to link parameters across stems.
7. Adjust dry/wet controls at the effect, chain, and global levels.
8. Save your work as a stem preset or deck preset for use in performance.
