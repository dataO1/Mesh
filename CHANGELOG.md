# Changelog

All notable changes to Mesh are documented in this file.

## Release Packages

| Package | Platform | Description |
|---------|----------|-------------|
| `mesh-cue_amd64.deb` | Linux (Debian/Ubuntu) | Full DJ application with stem separation (CPU) |
| `mesh-cue-cuda_amd64.deb` | Linux (Debian/Ubuntu) | Full DJ application with NVIDIA CUDA acceleration |
| `mesh-cue_win.zip` | Windows 10/11 | Full DJ application with DirectML GPU acceleration |
| `mesh-player_amd64.deb` | Linux (Debian/Ubuntu) | Lightweight stem player |
| `mesh-player_win.zip` | Windows 10/11 | Lightweight stem player |

### Installation

**Linux (.deb):**
```bash
sudo dpkg -i mesh-cue_amd64.deb      # or mesh-cue-cuda_amd64.deb for NVIDIA GPUs
sudo dpkg -i mesh-player_amd64.deb   # optional: lightweight player
```

**Windows (.zip):**
1. Extract the zip file to a folder (e.g., `C:\Program Files\Mesh`)
2. Run `mesh-cue.exe` or `mesh-player.exe`

> **GPU Notes:** The CUDA build requires NVIDIA driver 525+ and CUDA 12. The Windows build uses DirectML which works with any DirectX 12 capable GPU (AMD, NVIDIA, Intel) without additional drivers.

---

## [Unreleased]

### Added

- **Smart track suggestions** — The collection browser now recommends tracks
  based on what's loaded across all 4 decks. Combines audio fingerprint
  similarity (HNSW index), harmonic key compatibility, BPM proximity, and
  loudness alignment into a unified score. Toggle suggestions on/off from the
  browser toolbar.

- **Energy direction fader** — A horizontal slider in the suggestions panel
  steers recommendations toward higher-energy tracks (right) or cooler tracks
  (left). At center, suggestions prioritize safe harmonic transitions. Moving
  the fader unlocks progressively bolder key changes — energy boosts, semitone
  lifts, and even tritone drops become available at extreme positions. Uses a
  5-term scoring formula (HNSW distance, key compatibility, key emotional
  direction, ML arousal alignment, BPM proximity) with dynamic weight
  interpolation — HNSW weight fades from 0.40→0.15 at extremes while arousal
  and key direction weights increase, letting the fader genuinely reshape results.

- **Key transition emotional impact** — Each of the 14 key transition types
  (SameKey, AdjacentUp/Down, EnergyBoost/Cool, MoodLift/Darken, etc.) has a
  research-calibrated energy direction value based on DJ mixing theory and music
  psychology. These values influence both the harmonic filter (which transitions
  are allowed) and the scoring ranking (which direction is preferred). For
  example, semitone-up transitions (+0.70) are strongly boosted when raising
  energy, while energy-cool transitions (-0.50) are penalized.

- **Similarity search documentation** — Comprehensive technical documentation at
  `documents/similarity-search.md` covering the full suggestion pipeline: HNSW
  vector search, transition classification, base scores, energy modifiers,
  adaptive filter, dynamic weights, reason tags, and parameters reference.

- **Krumhansl key scoring model** — Alternative harmonic matching algorithm
  based on the Krumhansl-Kessler probe-tone research (1982). Computes a 24×24
  perceptual key distance matrix using Pearson correlations between pitch-class
  profiles. Selectable in Settings → Display → Key Matching. Compared to the
  default Camelot model, Krumhansl rates parallel-key transitions (e.g., C major
  to C minor) significantly higher, matching real-world DJ experience where
  parallel keys mix well despite being far apart on the Camelot wheel.

- **ML-enhanced audio analysis** — Automatic genre classification and mood
  tagging during track import using ONNX neural network models from the Essentia
  model hub (~20 MB, downloaded on first use). The pipeline extracts a mel
  spectrogram, runs it through EffNet to produce a 1280-dimensional audio
  embedding, then classifies against 400 Discogs genre labels and 56 Jamendo
  mood/theme tags. Vocal/instrumental detection uses RMS energy on the separated
  vocal stem (no model needed). Results are stored in the database and
  auto-populate the track tag system with colored genre (blue) and mood (purple)
  pills. Mood classification and arousal/valence derivation require enabling
  "ML Analysis (Experimental)" in Settings.

- **Arousal-based energy direction** — Smart suggestions now use perceptual
  arousal (derived from mood predictions) instead of LUFS loudness for the
  energy direction fader. Arousal captures musical energy from content
  (energetic, fast, heavy vs. calm, relaxing, soft) rather than just volume
  level, producing more musically meaningful energy-steering. Tracks without
  arousal data fall back to a redistributed scoring formula.

- **Track tags** — Color-coded tag pills in the collection browser. Tags are
  displayed between the track name and artist columns. ML analysis auto-generates
  genre and mood tags; suggestion results include reason tags showing the key
  relationship to currently playing tracks (e.g., "▲ Adjacent", "━ Same Key")
  with traffic-light coloring (green/amber/red) based on harmonic compatibility.

- **Re-analyse Similarity** — Context menu option to re-run ML analysis (genre,
  mood, vocal presence, arousal/valence) on existing tracks without re-importing.
  Works on single tracks, multi-selections, playlists, or the entire collection.
  Clears old ML-generated tags before re-tagging, so genre and mood labels stay
  current. Uses the ort-based pipeline (no subprocess), separate from the
  Essentia-based BPM/key/LUFS reanalysis.

- **Beat-synced LED feedback** — Play button LEDs now pulse to the beat when a
  deck is playing, using the master beatgrid phase. Loop button LEDs use a
  compound state: green pulse when playing (no loop), red pulse when loop is
  active and playing, steady red when loop is active but stopped. Both behaviors
  are automatically configured during MIDI learn.

- **Note-offset LED color mode** — Support for controllers that use MIDI note
  number offsets for LED colors (e.g., Allen & Heath Xone K series: red=+0,
  amber=+36, green=+72). The MIDI output handler tracks per-LED color state
  to ensure clean color transitions. Known controllers are auto-detected from
  a built-in device database during MIDI learn — no manual configuration needed.

- **Mirrored deck layout** — Bottom-row decks (3, 4) now render with overview
  waveforms on top and zoomed waveforms below, mirroring the top-row layout.
  All four overview waveforms cluster in the center of the 2x2 grid, reducing
  eye travel when comparing track positions. Grid gap reduced from 10px to 4px
  to keep the overviews visually grouped. Click-to-seek and drag-to-zoom
  hit-testing updated for the mirrored regions.

- **HID device auto-reconnection** — When an HID controller disconnects (USB
  hub reset, cable issue, power management), the I/O thread exit is now detected
  via a health check every 2 seconds. The device is re-enumerated by VID/PID
  (not path, since `/dev/hidraw*` can change on reconnect) and automatically
  reconnected with preserved MIDI profile, shift/layer state, and LED feedback.
  Gives up after 30 failed attempts (~60 seconds).

### Fixed

- **HID udev rules in .deb packages** — The mesh-player .deb now installs
  `/lib/udev/rules.d/99-mesh-hid.rules` automatically, granting user access to
  USB HID controllers (Kontrol F1). Previously the F1 silently failed on
  Debian/Pop!_OS because `/dev/hidraw*` devices require explicit udev rules for
  non-root access. Connection failures are now logged at warn level with a hint
  to check udev rules, instead of being silently swallowed at debug level.

- **MIDI learn 4-deck load buttons** — When using 4 physical decks without layer
  toggle, the browser phase now includes 4 dedicated DeckLoad steps (one per
  deck) after the browse encoder/select mapping. Previously only the global
  BrowserSelect was available, which could only load to the focused deck.

- **procspawn subprocess library resolution** — Force-linked `libopenmpt` and
  `libmpg123` as direct binary NEEDED dependencies via build.rs (with
  `--no-as-needed` to prevent linker stripping), and added to the Nix devshell
  runtime inputs. Without direct linkage, the deep transitive chain
  (Essentia → FFmpeg → libopenmpt → mpg123) failed lazy PLT symbol resolution
  in the procspawn subprocess on NixOS. Also uses `--disable-new-dtags` for
  DT_RPATH instead of DT_RUNPATH.

- **Essentia build pinning** — Pinned Essentia to commit `17484ff` (FFmpeg 4.x
  compatible) in Debian and Windows build scripts. Essentia master now requires
  FFmpeg 5.1+ `ch_layout` API, breaking builds against FFmpeg 4.4.x.

- **ML model input tensor names** — Fixed EffNet input name from
  `serving_default_melspectrogram` to `melspectrogram` and Jamendo mood input
  from `model/Placeholder` to `embeddings`, matching the actual ONNX model
  tensor names (TF SavedModel prefixes get stripped during ONNX conversion).

- **JACK client name collision** — mesh-cue and mesh-player now register with
  distinct JACK client names (`mesh-cue` / `mesh-player`) instead of both using
  `mesh-player`, preventing connection failures when both apps run simultaneously.

- **CPAL audio fallback** — When JACK server is unavailable, the audio system
  now falls back to CPAL (ALSA/PulseAudio) instead of failing outright. Both
  backends are always compiled; JACK is tried first, CPAL used as fallback.

- **Discogs genre tag splitting** — ML genre tags in the `SuperGenre---SubGenre`
  format (e.g., "Electronic---Breakcore") are now split into separate super-genre
  (dark blue) and sub-genre (light blue) tags, with super-genre deduplication.

- **HNSW full collection search** — Similarity search now queries the entire
  library (k=10,000 with dynamic beam width) instead of only the 30 nearest
  neighbors. This gives the energy direction fader a genuinely diverse candidate
  pool to work with — previously the top-30 were so similar that fader movement
  had negligible effect on results.

- **Energy fader responsiveness** — Reduced the energy direction debounce
  threshold from 0.05 to 0.02 and removed the dead zone in the energy modifier
  function, so small fader movements now produce visible changes in suggestions.

- **Debian build hidapi** — Added `libudev-dev` to the container build packages.
  The hidapi crate's `linux-static-hidraw` backend needs udev headers at compile
  time, causing build failures in the Ubuntu 22.04 container.

- **Windows cross-compilation hidapi** — Made the `hidapi` dependency
  target-conditional in mesh-midi: Linux uses `linux-static-hidraw` (static
  hidraw backend), Windows uses default features (native Windows HID API via
  `hid.dll`). The previous unconditional `linux-static-hidraw` feature would
  fail when cross-compiling to `x86_64-pc-windows-gnu`.

- **Xone K3 LED color model** — Corrected to 3 discrete layer offsets (red=+0,
  amber=+36, green=+72) instead of a continuous gradient. The K3 has only 3 LED
  color states unlike the K2's gradient.

- **MIDI learn loop control addressing** — Loop toggle, halve, and double
  controls now use `deck_index` (direct addressing) in the generated MIDI config
  instead of `physical_deck` (layer-resolved). This matches the stem mute
  pattern and ensures loop controls always target the correct virtual deck.

---

## [0.8.1] - 2026-02-15

### Added

- **Browser overlay in performance mode** — The collection browser is now hidden
  by default in performance mode, giving waveforms full-screen height. The browser
  appears as a modal overlay when triggered by MIDI browse mode, encoder scroll, or
  encoder select, and auto-hides after 5 seconds of inactivity or when a track is
  loaded. Click the dark backdrop to dismiss manually. Mapping mode is unchanged.

- **Dynamic waveform canvas height** — The 4-deck waveform canvas now fills all
  available vertical space instead of using a fixed 350px height. Header and
  overview waveforms keep their fixed sizes while the zoomed waveform absorbs all
  extra space, giving significantly taller waveforms on larger displays.

### Fixed

- **HID display feedback channel spam** — The 7-segment display update now skips
  sending when the text hasn't changed, preventing "Display feedback channel full"
  log spam that occurred when identical loop-length or layer text was sent every
  tick at 30Hz.

---

## [0.8.0] - 2026-02-15

### Added

- **Momentary mode overlay for compact controllers** — MIDI Learn now offers a
  choice between permanent and momentary mode buttons. With momentary mode, hold
  a mode button (e.g., Hot Cue or Slicer) to temporarily overlay pad functions,
  then release to return to the default performance mode (stem mutes, transport).
  Ideal for compact controllers that share buttons between transport and
  performance pads.

- **Per-side mode buttons (4-deck)** — On 4-deck setups without layer toggle,
  mode buttons work per side: the left mode button controls decks 1 and 3, the
  right mode button controls decks 2 and 4. Each side can independently enter
  Hot Cue or Slicer mode.

- **Dual browse encoders (4-deck)** — 4-deck non-layered setups now support
  two browse encoders (left and right) during MIDI Learn, one per physical side.

- **State-aware HID LED feedback** — HID controllers with RGB pads (e.g.,
  Kontrol F1) now show distinct colors per function: green for play (pulsing
  when playing), orange for cue, green/red for loop (green when playing, red
  when loop is active), amber for hot cues, cyan for slicer, blue/purple for
  mode buttons, and per-stem colors for mute toggles. LEDs automatically switch
  between performance, hot cue, and slicer color schemes when mode buttons are
  pressed.

- **Per-deck load buttons (4-deck)** — The browser phase now includes individual
  load buttons for all 4 decks when using a non-layered 4-deck setup.

### Fixed

- **HID feedback channel overflow** — Increased HID feedback buffer and added
  RGB-aware change detection to prevent "channel full" warnings and LED
  flickering.

---

## [0.6.16] - 2026-02-12

### Added

- **HID controller support** — Native USB HID device support alongside MIDI,
  starting with the Native Instruments Traktor Kontrol F1. HID devices are
  auto-discovered via hidapi, with dedicated I/O threads for low-latency input
  parsing and LED/RGB feedback output. A protocol-agnostic abstraction layer
  (`ControlEvent`, `ControlAddress`, `ControlValue`) allows the mapping engine,
  feedback system, and learn mode to work identically for MIDI and HID devices.

- **Multi-HID-device support** — Multiple identical HID devices (e.g., two
  Kontrol F1s) are now independently addressable. Each device is identified by
  its USB serial number (`device_id`), which is embedded into every
  `ControlAddress::Hid` to produce distinct HashMap keys in the mapping engine.
  Device profiles can target specific physical units via the new `hid_device_id`
  config field, with automatic fallback to product-name matching for single-device
  setups. MIDI learn captures the serial and writes it into the generated config.

- **Kontrol F1 HID driver** — Full protocol implementation for the NI Kontrol F1:
  16 RGB grid pads, 4 play buttons with LEDs, 9 function buttons (8 with LEDs),
  4 analog knobs, 4 analog faders, 1 rotary encoder with push, and a 4-digit
  7-segment display. Input uses delta detection with analog deadzone filtering;
  output uses BRG byte-order RGB and brightness-scaled single-color LEDs.

- **Per-physical-deck MIDI mapping** — Complete rework of MIDI controller mapping
  for 2-deck controllers with layer toggle controlling 4 virtual decks:
  - **Per-deck shift buttons** — Each physical deck side has its own shift button
    instead of a single global shift. Shift state is tracked per-deck via
    `SharedMidiState`, and the mapping engine resolves shift based on the
    mapping's physical deck association.
  - **Layer toggle wiring** — Layer toggle buttons are now learned during MIDI
    learn (no more placeholder config). Toggle detection happens in the input
    callback and updates shared state directly.
  - **Per-deck browser encoders** — In layer mode, each physical deck side gets
    its own browser encoder and select button, loading tracks to the active
    virtual deck on that side.
  - **Stem mute direct mapping** — Stem mute buttons use `deck_index` (direct
    deck addressing) instead of `physical_deck` (layer-resolved), so the physical
    4x4 button matrix always maps to fixed virtual decks.
  - **Layer toggle LED colors** — `FeedbackMapping` gains `alt_on_value` for
    Layer B color differentiation (e.g., red for Layer A, green for Layer B).
  - **UI layer indicators** — Deck labels are colorized based on active MIDI
    layer (red = Layer A, green = Layer B, white = not targeted).
  - **Thread-safe shared state** — New `SharedMidiState` (`Arc`-wrapped) shared
    between midir input callback and mapping engine, using `AtomicBool` for shift
    and `RwLock` for layer/deck target state.
  - **Breaking:** MIDI config format changed — `shift` field replaced with
    `shift_buttons: Vec<ShiftButtonConfig>`. Old configs must be re-learned.

- **Standalone CLAP plugin support** — CLAP plugins can now bundle their runtime
  dependencies in a `lib/` subdirectory for fully portable operation. Essential
  for NixOS and other non-FHS Linux distributions.
  - Libraries are automatically discovered via `$ORIGIN/lib` RPATH
  - `LD_LIBRARY_PATH` fallback for maximum compatibility
  - Includes 194 LSP plugins (compressors, EQs, reverbs, etc.)

- **LSP plugin setup script** — `scripts/setup-lsp-plugins.sh` downloads and
  optionally bundles dependencies for LSP plugins.

- **Effects documentation** — Comprehensive guide at `docs/effects.md` covering:
  - PD effect creation with metadata.json examples
  - CLAP plugin installation and dependency bundling
  - Multiband processing workflow

- **Example effects collection** — `collection/effects/pd/` includes:
  - `test-gain/` — Simple gain utility for testing
  - `rave-percussion/` — Neural audio synthesis via nn~ external

- **Effects editor modal (mesh-cue)** — New full-featured effects preset editor
  accessible via the "FX Presets" button. Create multiband processing chains
  with complete plugin parameter control:
  - **Multiband splitter** — Up to 8 frequency bands with adjustable crossovers
  - **Pre/Post-FX chains** — Apply effects before or after the band split
  - **Per-band effect chains** — Independent effect stacks for each frequency band
  - **8 macro knobs** — Map macros to any parameter across all effect chains
  - **Real-time audio preview** — Toggle preview to hear changes live on any stem

- **CLAP parameter learning (mesh-cue)** — Click the label under any effect knob
  to enter learning mode. Open the plugin's native GUI and adjust any parameter —
  it will be automatically assigned to that knob. This allows controlling any of
  a plugin's parameters, not just the default first 8.

- **Macro-to-parameter mapping** — Click the "Map" button on any macro knob, then
  click a parameter knob to create a mapping. Macros use bipolar modulation where
  50% is neutral: turning below 50% subtracts from the base value, above 50% adds.
  This enables expressive live control of multiple parameters simultaneously.

- **Macro modulation range indicators** — Visual mini-bars above each macro knob
  show which parameters are mapped and their modulation depth. Drag indicators
  up/down to adjust the modulation range from -1 (fully inverted) through 0
  (no modulation) to +1 (full modulation). Hover over an indicator to highlight
  the corresponding parameter knob in the effect chain.

- **Stem/deck preset split** — Replaced the flat single-preset system with a
  two-level hierarchy. **Stem presets** (`presets/stems/`) save one stem's effect
  chain. **Deck presets** (`presets/decks/`) reference 4 stem presets by name plus
  4 shared macro knobs, enabling reuse of the same stem preset across multiple
  decks. A `preset_type` field in YAML prevents accidentally loading the wrong
  type. Auto-generates stem preset names when saving a deck preset.

- **Non-destructive stem switching** — Switching between stems (VOC/DRM/BAS/OTH)
  in the effects editor no longer destroys CLAP plugin instances. Plugin GUIs are
  hidden (not destroyed) and effect state is snapshotted/restored via
  `StemEffectData`. Stem-indexed CLAP instance IDs (`_cue_s{N}_`) prevent handle
  collisions between stems.

- **Two-row effects toolbar** — The effects editor toolbar is now split into a
  deck row (deck preset name, New/Load/Save, audio preview toggle) and a stem row
  (stem tabs with data indicators, stem preset name, Load/Save). Separate browser
  and save dialogs for stem-level and deck-level presets.

- **Full plugin state in presets** — Presets now capture ALL plugin parameter
  values, not just the 8 mapped to UI knobs. Settings made via the plugin's
  native GUI (e.g., reverb mode, filter type) are preserved across save/load.

- **Multiband latency compensation** — Ring-buffer delay lines at every dry/wet
  blend point inside `MultibandHost` eliminate phase cancellation when mixing dry
  and wet signals from plugins that report non-zero latency. Compensation operates
  at four levels:
  - **Per-effect** — Dry signal delayed by individual plugin latency before blending
  - **Per-chain** — Pre-chain snapshot delayed by total chain latency
  - **Inter-band alignment** — Shorter bands padded to match the longest band,
    preventing frequency-region time smearing at crossover points
  - **Global** — Unprocessed input delayed by full pipeline latency
  Internal compensation is invisible to the external `LatencyCompensator`; reported
  `latency_samples()` is unchanged so stem-level alignment across decks is unaffected.

- **Parallel band processing** — Multiband bands are processed in parallel via
  Rayon `par_iter_mut` when more than one band is active, distributing effect
  processing across CPU cores. Band alignment is folded into the parallel loop
  since each band's delay line only touches its own buffer.

- **Dry/wet mix controls** — Comprehensive parallel processing support at three
  levels for precise blend control:
  - **Per-effect dry/wet** — A 9th knob (D/W) on each effect card controls the
    blend between the unprocessed and processed signal for that effect
  - **Chain dry/wet** — Each chain section (Pre-FX, each band, Post-FX) has a
    dedicated D/W knob to blend the entire chain's output with the original
  - **Global dry/wet** — Master D/W knob in the macro bar blends the entire
    effect rack output with the completely unprocessed signal
  - All dry/wet controls are macro-mappable via drag-and-drop with ±50% modulation
    range, enabling expressive performance control
  - Values are saved and loaded with presets (existing presets default to 100% wet)

- **Reworked shader-based knob visuals** — Complete overhaul of the GPU-accelerated
  knob rendering for better visual feedback:
  - **Arc-based value display** — Proper 270° arc showing value from min to max
  - **Modulation range indicators** — Orange outer arcs on effect knobs show the
    possible modulation swing when mapped to a macro
  - **Separate base/display values** — The value arc shows the base parameter value,
    while the white indicator dot shows the actual modulated position
  - **Fixed fullscreen triangle rendering** — Corrected viewport coordinate handling
    for iced's `draw()` method, fixing clipping issues at screen edges
  - **Improved arc calculations** — Fixed angle normalization for arcs crossing 0°

- **Visual effect drag feedback** — Dragging effects between chains now shows a
  floating effect card following the cursor for clear visual feedback. The card
  displays the effect name with a styled border and drop shadow, making it easy
  to see what's being moved during drag-and-drop reordering.

- **Improved crossover bar interaction** — The frequency crossover dividers now
  track mouse movement relatively rather than using absolute positioning, providing
  smoother and more intuitive drag behavior regardless of window size. Single-band
  mode is now clickable to split into multiple bands.

- **Macro name inline editing** — Click directly on a macro name to rename it.
  A dedicated drag handle (⠿) next to the name initiates drag-to-map operations,
  separating editing from mapping interactions.

- **Global FX preset dropdown (mesh-player)** — Replaced per-deck FX preset
  dropdowns with a single centralized dropdown in the header bar (next to BPM
  slider). Selecting a preset applies it to all 4 decks simultaneously, matching
  the typical live performance workflow where all decks share the same FX chain.

- **MIDI FX encoder browsing** — A dedicated FX encoder (separate from the
  browser encoder) can now scroll through and select FX presets for all decks
  during MIDI exploration. The FX encoder rotation scrolls the preset list with
  wrapping, and the encoder press confirms the selection. The global FX dropdown
  renders as a floating overlay and auto-opens when the MIDI encoder scrolls,
  showing the currently hovered preset name in the header button.

- **MIDI FX macro knobs** — 4 macro knob mappings per deck are now captured
  during the MIDI learn Mixer phase (10 steps per deck: 6 mixer controls + 4
  FX macros). Mapped knobs send continuous CC values to the shared deck macro
  sliders for real-time effect control.

- **"No FX" encoder selection** — The global FX dropdown now includes "No FX"
  as the first item reachable via MIDI encoder scroll. Selecting it clears the
  FX preset from all decks, returning them to passthrough mode.

- **Loop length indicator in waveform header** — The canvas deck header now
  displays the current loop length (e.g., "↻4", "↻1/2") next to the BPM
  indicator. The text turns green when the loop is active, gray when inactive.

- **Volume-based waveform dimming** — Waveforms now visually dim based on the
  channel volume fader position. At full volume no dimming is applied; as
  the fader lowers, a semi-transparent overlay gradually darkens the waveform
  area, providing instant visual feedback of relative deck volumes.

- **Two-stage master bus protection** — Prevents digital overs and protects PA
  systems with a clipper → limiter chain on the master output:
  - **ClipOnly2-style safety clipper** — Stateful clipper based on the Airwindows
    algorithm using Dottie number interpolation. Shaves transient peaks cleanly
    with ~1 sample latency and zero processing when below threshold (−0.3 dBFS).
  - **Transparent lookahead limiter** — Feed-forward limiter with 1.5 ms lookahead
    (72 samples at 48 kHz). Smoothly reduces gain for sustained overs using
    sliding-window peak detection and exponential attack/release envelope.
    100 ms release time-constant prevents audible pumping.
  - **Clip indicator** — The "Audio Connected" dot in the header flashes red when
    the clipper engages (~150 ms hold), providing real-time visual feedback of
    master output clipping via a lock-free `AtomicBool` from the audio thread.

### Fixed

- **Container .deb build fixes** — Fixed two build failures in the Ubuntu 22.04
  container: added Kitware APT repository for CMake 3.25+ (Ubuntu 22.04 ships
  3.22, but libpd requires 3.25), and added missing `libx11-xcb-dev` and
  `libxcb1-dev` dependencies needed by recent iced/wgpu versions.

- **mesh-cue CLAP latency display** — CLAP plugin latency is now shown in the
  effect card header in mesh-cue (e.g., "Compressor (2.3ms)"). Previously always
  displayed 0 because `EffectUiState.latency_samples` was never set after plugin
  creation, even though the value was available from the plugin info.

- **Dry/wet phase misalignment** — Dry/wet blending at all levels (per-effect,
  per-chain, global) no longer causes comb filtering when plugins introduce latency.
  Multiband frequency regions no longer exhibit time smearing when bands have
  different total chain latencies.

- **Effects editor preset loading** — Loading a preset now properly clears stale
  UI state (drag handles, hover state, effect knobs) before applying the new
  configuration. Previously, stale references to old effects could cause crashes.

- **CLAP plugin latency compensation** — CLAP plugins now properly report their
  processing latency via the CLAP latency extension. This fixes audio alignment
  issues when playing multiple decks with effects that introduce latency (e.g.,
  lookahead limiters, linear-phase EQs). Previously, all CLAP plugins reported
  0 latency, causing drum tracks to drift out of sync across decks.

- **CLAP `request_restart()` handling** — Plugins that change latency dynamically
  (e.g., LSP compressors adjusting lookahead) now trigger a deactivate → reactivate
  cycle with latency re-query. Previously, `request_restart()` was ignored, so
  plugins reporting 0 at activation but changing latency later (common with LSP
  plugins) would cause phase misalignment at partial dry/wet settings.

- **mesh-player preset loading** — Presets loaded in mesh-player now correctly
  apply ALL plugin parameters to the audio engine. Previously only the 8
  knob-mapped parameters were applied, ignoring settings made via the plugin's
  native GUI (e.g., reverb mode, filter type). Also fixed bypass state not
  being applied when loading presets.

- **MIDI learn phase-skipping** — Fixed phases being silently skipped during
  MIDI learn when controls were pressed within the 1-second capture debounce
  window. The debounce timer is now reset at each phase transition so subsequent
  captures are accepted immediately.

- **MIDI learn encoder press detection** — Encoder button presses (BrowserSelect,
  FxSelect) are now mapped as standalone learn steps rather than being auto-detected
  after encoder rotation. The old `awaiting_encoder_press` sub-flow was fragile
  and often missed presses due to debounce timing. Browser phase now has 7+N steps
  (FxEncoder, FxSelect, BrowserEncoder, BrowserSelect, master controls, deck loads).

- **MIDI learn loop size encoder** — Replaced separate loop halve/double button
  mappings with a single loop size encoder target. Previously, mapping an encoder
  to both halve and double resulted in only double working (second mapping overwrote
  the first). Now a single `DeckLoopEncoder` target uses encoder rotation direction:
  negative = halve, positive = double.

- **mesh-player macro modulation** — Macro sliders in the deck view now properly
  modulate effect parameters. Previously, moving a macro slider only updated the
  UI value without actually changing the audio. Fixed by implementing direct
  parameter modulation: when a preset is loaded, all macro-to-parameter mappings
  are extracted and stored in the UI state. When a macro slider is moved, the
  modulated parameter values are computed and sent directly to the audio engine,
  matching the proven approach used in mesh-cue.

- **Engine macro mappings dropped during preset loading** — Deck presets with
  macro mappings now load correctly in mesh-player. Previously, bulk preset
  loading sent 300-500+ commands in a tight burst that overflowed the 64-slot
  `rtrb` ring buffer. `AddMultibandMacroMapping` commands near the end of each
  stem's burst were silently dropped, resulting in `(has 0 mappings)` for all
  stems. Fixed by increasing queue capacity to 1024 and adding retry-with-backoff
  logic so the UI thread briefly yields when the queue is full instead of
  discarding commands.

### Changed

- **Unified effects directory** — Each effect type has its own subfolder:
  - PD effects: `effects/pd/<effect-name>/`
  - PD externals: `effects/pd/externals/`
  - PD models: `effects/pd/models/`
  - CLAP plugins: `effects/clap/`
  - CLAP libs: `effects/clap/lib/`

- **Improved CLAP discovery logging** — Better error messages when plugins fail
  to load due to missing dependencies, with actionable guidance.

- **Effects editor architecture (mesh-cue)** — Complete rewrite of the effects
  editing system. The editor now maintains its own UI state separate from audio,
  with changes synced to the audio engine only when preview is enabled. This
  provides a responsive editing experience without audio glitches.

- **Preset format (breaking)** — Removed legacy flat preset I/O and the
  `MultibandPresetConfig` type alias. Presets now use `StemPresetConfig` and
  `DeckPresetConfig` with explicit `preset_type` validation. Old presets in
  `presets/` must be moved to `presets/stems/` to be discovered.

### Known Limitations

- **libpd parallel processing** — Multiple PD effects process in parallel (not
  series) due to libpd's single global DSP graph architecture. A warning is now
  shown when adding multiple PD effects.

---

## [0.6.0] - 2026-02-03

### Added

- **CLAP plugin hosting** — Load any CLAP plugin as a stem effect. Mesh now supports the [CLAP](https://cleveraudio.org/) open-source plugin standard, giving you access to hundreds of professional effects including LSP Plugins (compressors, EQs, gates), Dragonfly Reverb, Airwindows, BYOD, and ChowTapeModel. Plugins are automatically discovered from `~/.clap/` and `/usr/lib/clap/`.

- **Unified effect picker** — The effect picker now shows both Pure Data effects and CLAP plugins in a single interface. Filter by source (All/PD/CLAP) using the new toggle buttons. Effects are grouped by category with availability status.

- **RAVE neural audio effects** — Create neural audio effects using [RAVE](https://github.com/acids-ircam/RAVE) models via the nn~ external. The included RAVE percussion example (`examples/pd-effects/rave-percussion/`) demonstrates real-time neural timbre transfer. Build nn~ with `nix run .#build-nn-tilde`.

- **Multiband effect container** — New multiband effect system (Kilohearts Multipass-style) that splits audio into frequency bands using LSP Crossover and applies separate effect chains per band. Access via the **Multiband** button on each stem. Features:
  - Up to 8 frequency bands with adjustable crossover points
  - Per-band effect chains with full CLAP and Pure Data plugin support
  - 8 macro knobs per stem for live performance control
  - Band mute/solo/gain controls for sculpting your sound
  - Parameter mapping — route any macro knob to multiple effect parameters

- **Interactive macro knobs** — Per-stem macro knobs on the deck view are now interactive sliders (not read-only). Adjust them during live performance to control multiband effect parameters in real-time.

### Changed

- **Effect architecture overhaul** — All effects now go through the multiband container system. The previous per-effect chain model has been replaced with a unified multiband approach, improving latency compensation and enabling frequency-band-specific processing.

- **Effect UI state sync** — Effect bypass, add, and remove operations now properly sync between the UI and audio engine. Bypass toggles correctly reflect current state instead of always bypassing.

- **Improved latency compensation** — Empty decks (no track loaded) now report zero latency, preventing unnecessary compensation delays on active decks. Maximum compensation buffer increased to 8000 samples (~165ms at 48kHz) for better headroom with high-latency effects.

- **RAVE latency reduced** — RAVE effects now use a 2048-sample buffer (down from 4096), reducing latency from ~85ms to ~43ms at 48kHz while maintaining stable processing.

### Technical

- **clack-host integration** — CLAP plugins are loaded via [clack-host](https://github.com/prokopyl/clack), a Rust CLAP hosting library. Effects implement the standard `Effect` trait for seamless integration with the effect chain system.

- **RT-safe CLAP processing** — Plugin audio processing uses `try_lock()` to avoid blocking the audio thread. On lock contention, frames are skipped gracefully rather than causing dropouts.

- **Single libpd instance** — Pure Data effects now share a single global PD instance with RT-safe logging, improving stability and reducing resource usage.

---

## [0.5.1] - 2026-01-30

### Added

- **Pure Data effect plugins** — Create custom audio effects using [Pure Data](https://puredata.info/) patches. Effects are loaded from `~/Music/mesh-collection/effects/` and can be added to any stem's effect chain via the new effect picker UI. Supports up to 8 parameters per effect with real-time knob control.

- **Effect picker modal** — New UI for browsing and adding effects to stems. Click the "+" button on any stem's effect chain to open the picker. Effects are grouped by category with availability status.

- **nn~ build script** — `nix run .#build-nn-tilde` builds the nn~ external for RAVE neural audio effects. Outputs to `dist/nn~/`.

- **PD effect examples** — Added `examples/pd-effects/` with templates and working examples including a simple gain effect and RAVE percussion template.

### Technical

- **libpd-rs integration** — Per-deck PD instances with thread-safe audio processing
- **Lock-free parameter control** — UI knob changes sent via command queue, no audio thread blocking
- **Effect discovery** — Automatic scanning of effects folder with dependency checking

---

## [0.5.0] - 2026-01-28

> **⚠️ Note:** Stem separation features in this release are experimental. GPU acceleration (CUDA on Linux, DirectML on Windows) is untested and may not work on all systems.

### Added

- **HTDemucs hybrid model support** — Full implementation of the Demucs v4 hybrid transformer architecture with both time-domain and frequency-domain branches. The frequency branch uses STFT/ISTFT processing to capture spectral details that the time branch might miss.

- **Shift augmentation** — Configurable random time-shifting (1-5 shifts) that averages multiple inference passes for improved separation quality. Each additional shift adds ~0.2 SDR improvement at the cost of proportionally longer processing time. Configurable in Settings under "Separation Shifts".

- **Residual "other" stem computation** — Instead of using the model's direct "other" prediction (which often contains vocal/hihat artifacts), the "other" stem is now computed as `mix - drums - bass - vocals`. This ensures perfect reconstruction and eliminates bleed from other stems.

- **50% segment overlap** — Increased from 25% to 50% overlap between processing segments. This provides better handling of transients (especially hihats) at segment boundaries, at the cost of ~2x processing time.

- **High-frequency preservation for drums** — Neural networks often attenuate frequencies above 14kHz. For the drum stem, frequencies above 14kHz are now blended from the original mix using a smooth spectral crossfade, restoring hihat and cymbal crispness without introducing bleed.

- **External data file support** — Model downloads now correctly handle ONNX external data files (`.onnx.data`). Large models store weights separately from the graph structure, and both files are now downloaded and managed together.

- **Fine-tuned model option** — Added support for `htdemucs_ft` (fine-tuned) model variant, which provides ~1-3% better SDR than the standard model with the same architecture.

- **Model selection in settings** — Choose between "Demucs 4-stem" (fast) and "Demucs 4-stem Fine-tuned" (better quality) in Settings > Separation Model.

### Changed

- **Model download URLs** — Models are now downloaded from GitHub releases (`releases/download/models/`) for faster and more reliable downloads.

- **Simplified model options** — Removed 6-stem model support. Only 4-stem models (vocals, drums, bass, other) are now available, which is the standard configuration for DJ workflows.

- **Progress reporting for downloads** — Model download progress now correctly accounts for both `.onnx` (~2MB) and `.onnx.data` (~160MB) files, with the large data file representing 98% of progress.

### Fixed

- **WAVE_FORMAT_EXTENSIBLE support** — Fixed WAV file import for files using the extensible format header (common in professional audio software).

- **STFT preprocessing** — Fixed spectrogram computation to exactly match Demucs' `standalone_spec` function, including proper reflection padding, frame cropping, and normalization.

### Build System

- **GPU-accelerated builds** — Added compile-time GPU acceleration support for stem separation:
  - `nix run .#build-deb` — CPU-only Linux build (works everywhere)
  - `nix run .#build-deb-cuda` — Linux build with NVIDIA CUDA 12 support
  - `nix run .#build-windows` — Windows build with DirectML (AMD/NVIDIA/Intel via DirectX 12)

- **DirectML for Windows** — Windows builds now include DirectML support by default. DirectML is built into Windows 10+ and provides GPU acceleration for any DirectX 12 capable GPU without additional driver installation.

- **CUDA for Linux** — Optional CUDA 12 builds available for NVIDIA GPU users. Requires NVIDIA driver 525+ and CUDA toolkit on target system.

- **Runtime DLL loading for Windows** — Windows builds now use the `load-dynamic` ort feature, which loads `onnxruntime.dll` at runtime via `LoadLibrary()` instead of linking at compile time. This bypasses MinGW/MSVC ABI incompatibility and enables MinGW cross-compiled builds to use Microsoft's pre-built DirectML binaries.

- **Fixed Windows packaging** — Zip package creation now removes old zip files before creating new ones, preventing "missing end signature" errors from corrupted partial builds.

---

## [0.4.4] - 2026-01-26

### Added

- **Automatic stem separation** — Import regular audio files (MP3, FLAC, WAV, OGG, M4A) and mesh-cue will automatically separate them into stems using the Demucs neural network. No external tools required!

- **Import mode toggle** — Switch between "Pre-separated Stems" and "Mixed Audio" modes in the import modal. Mixed audio mode handles stem separation automatically.

- **Separation progress display** — During mixed audio import, the progress bar shows real-time separation progress (e.g., "Track Name (separating 45%)").

- **Modular separation backend** — Architecture supports swappable backends (ONNX Runtime, future Charon). Models are downloaded automatically on first use (~171MB, cached in `~/.cache/mesh-cue/models/`).

### Changed

- **Import modal redesigned** — New dual-mode UI with toggle buttons for selecting import type. Mixed audio mode shows detected audio files with format indicators (MP3, FLAC, etc.).

- **README Quick Start updated** — Now explains both import options (automatic separation vs. pre-separated stems) for new users.

---

## [0.4.3] - 2026-01-25

### Added

- **Multi-device MIDI support** — Connect multiple MIDI controllers simultaneously. All configured devices are connected at startup, not just the first match.

- **Drag-and-drop on file list** — Tracks can now be dropped onto the file list panel (right column) in mesh-cue, not just playlist labels.

- **Shift+click to delete stem links** — Hold Shift and click a linked stem button to remove the link.

- **Drag indicator** — When dragging tracks, a semi-transparent label appears near the cursor showing the track name (or "name..." for multiple tracks).

- **Drop zone highlight** — The file browser shows a teal border outline when hovering over it while dragging tracks, indicating where tracks will be dropped.

- **Drag threshold** — Dragging only starts after moving 8 pixels from the initial click, preventing accidental drags and preserving double-click to load tracks.

- **Automatic port name capture** — During MIDI learn, the actual system port name is captured and stored as `learned_port_name` for precise device matching on reconnection.

- **Port name normalization** — Hardware IDs like `[hw:3,0,0]` are stripped from port names, so devices match regardless of which USB port they're connected to.

- **Device matching with fallback** — Exact match against `learned_port_name` is tried first, falling back to substring match against `port_match` for backwards compatibility.

- **New MidiController methods** — `connected_count()`, `connected_device_names()`, `first_connected_port()`, and `drain_raw_events_with_source()` for multi-device management.

- **Dynamic audio output switching (JACK)** — On Linux with JACK, audio outputs can now be changed in settings without restarting the app. Works in both mesh-player (master/cue outputs) and mesh-cue (single output). Device selection is saved to config.

- **Zoomed waveform scrubbing** — Click and drag horizontally on the zoomed waveform to scrub through the track. Drag direction is auto-detected: horizontal = scrub, vertical = zoom. Waveform moves 1:1 with mouse like grabbing vinyl.

- **BPM adjustment buttons** — Plus and minus buttons next to the BPM field for quick ±1 BPM adjustments. Beat grid is automatically recalculated.

- **Beat grid align button** — New "│" button next to grid nudge controls sets the current playhead position as a downbeat. Scrub to where you hear the "1" of the bar, click the button (or press "m"), and the grid aligns to that position.

- **Vinyl-style scratch audio** — During zoomed waveform scrubbing, audio plays based on mouse velocity like real vinyl. Moving the mouse plays audio at proportional speed; stopping outputs silence (not a looped buffer). Backward movement plays audio in reverse. Previous play state is restored when scrubbing ends.

- **Scratch interpolation setting** — Choose between Linear (fast, acceptable quality) and Cubic (Catmull-Rom, smoother audio) interpolation for scratch audio in Settings > Audio Output. Cubic interpolation provides higher quality variable-speed playback at the cost of slightly more CPU usage.

### Changed

- **MidiController architecture** — Refactored from single-device (`Option<MidiInputHandler>`) to multi-device (`HashMap<String, ConnectedDevice>`) support.

- **Global shift state** — Shift state is now shared across all connected MIDI devices for consistent behavior.

- **LED feedback** — Feedback is now sent to all connected devices, not just the first one.

### Fixed

- **Cross-system MIDI compatibility** — Devices now connect correctly when hardware enumeration differs between systems (e.g., `hw:1,0,0` on Pop!_OS vs `hw:3,0,0` on NixOS).

- **Collection auto-refresh after import** — Newly imported tracks now appear in the file browser immediately after analysis completes.

- **Text overlap in file browser** — Long track names and labels are now clipped instead of overlapping adjacent columns.

- **File browser layout** — Editor and browser panels now use proportional sizing (3:1 ratio) to prevent the browser from squeezing hot cue buttons.

- **Beat grid preserved on BPM change** — Changing BPM now anchors the grid on the beat nearest to the playhead instead of the first beat. This preserves nudge adjustments — the beat you're listening to stays in place while others recalculate.

---

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
