# Live Mode — Design

**Date:** 2026-10-13
**Status:** Approved for implementation (grilling complete)
**Branch:** `live-performance-mode`
**Research:** [`2026-10-13-live-mode-research.md`](2026-10-13-live-mode-research.md). References like "R§7.3" point to sections in that document.

---

## 1. Goal

Live mode is a second operating mode of mesh-player. Instead of playing tracks, it **generates breakbeat music live from parameters**: DnB, jungle, hardcore and midtempo breakbeat. There is one multi-layer drum lane and three instrument lanes, all locked to one master clock and one global key.

The performer plays it like an instrument on two Kontrol F1s. Nothing is pre-arranged. A set is one continuous piece that is shaped live. The prepared material is only sound design (kits, samples, synth presets, FX), packed into a **style** that is authored on the PC and carried on the USB stick.

The open question this design has to answer early is whether the result **is fun to play and interesting to listen to**. The phases are built so that each one reaches the hardware for an ear test as soon as it works.

### Non-goals (v1)

- Mixing live lanes with track decks in the same session. The architecture leaves room for it later (§4.1).
- Typing code on the device.
- Pre-arranged automation, song structures or follow-actions.
- Projector or stream output. v1 targets the performer's main screen only.
- A style editor GUI. In v1 styles are edited by hand.

---

## 2. Decisions

Codes are the ones used during the grilling. Superseded decisions are kept, with a pointer to what replaced them.

### 2.1 Product and scope

| ID | Decision | Rationale |
|---|---|---|
| D1 | ~~One deck switches into live mode~~ → **superseded by D21** | — |
| D21 | Live mode is a **full alternative mode**, not a deck. It is built so live and track decks can be mixed later | Avoids colliding with the fixed 4×4 deck/stem arrays and the master-deck sync model now. Mixing both stays a later option |
| D35 | DJ or live mode is chosen **at startup** (config or boot menu). There is no runtime switch | No hand-over of state between modes, and simpler engine ownership |
| D8 | The Orange Pi is the target from day one. CPU, GPU and core pinning are budgeted up front | The board is already near its GPU limit (R§1.3) |
| D18 | The board is the **Orange Pi 5** (not Pro), with the internal 2880×864@120 screen on HDMI-A-1 | `flake.nix:61`, `kiosk.nix:37` |
| D24 | v1 renders only the performer's main screen | A second output needs compositor work (R§5.3) |
| D12 | No throwaway prototype. Build into the real architecture on researched facts | User decision |
| D38 | Core code is unit-tested. **Every musical decision** (mappings, generators, presets, sounds) is ear-tested on the hardware as soon as something works | Research can't answer "does it sound good" |
| D39 | Phases P1 to P6 (§10) | Each phase builds only on parts already verified |

### 2.2 Sound

| ID | Decision | Rationale |
|---|---|---|
| D9 | One multi-layer **drum lane** plus **3 instrument lanes** | Covers drums, bass, lead/arp and an extra layer |
| D3 / D34 | A lane is one **instrument** (sampler or CLAP synth) plus a **CLAP/native FX chain**. Each lane has **4 performance controls** that can map to any parameter in the lane | The performer chooses the sound. The controls drive generator and sound parameters the same way |
| D31 | Stem material (for example a lead phrase) is a source for the **parametrized sampler** | One sampler serves one-shots, multisamples and stem phrases |
| D32 | At most **2 CLAP instrument lanes** in v1. Final voice budgets come from a benchmark on the board | No measured per-voice ARM cost exists (R§5.4) |
| D13 / D20 | **Level matching is automatic.** Each instrument gets a predetermined per-role LUFS target, and a **mix/master compression chain** runs on top. No fader is a level fader | No level matching during the set, full creative freedom |
| D27 | **Fixed native glue chain**, with its parameters in the style (§5.6) | Glue is as decisive as the patterns (R§7.1.9) |
| D28 | The sound engine is signed off by ear **before** any generator work | The earlier Tidal engine failed on timbre, not patterns (R§2.3) |

### 2.3 Patterns and harmony

| ID | Decision | Rationale |
|---|---|---|
| D5 | Nothing is pre-arranged. Patterns are seeded and generated live. Arrangement is a live-edited chain of slots | Live performance only |
| D17 | **Own pattern engine, written from scratch.** The earlier `breaks.hs`/`motion.hs` engine is not ported | Rust engines are immature and allocate (R§4.1). The earlier engine never sounded good |
| D25 / D42 | Generators sit behind one interface. **Procedural on-the-fly generation comes first.** Curated idiom data (Grids-style maps, rhythm libraries) is the fallback. Both are compared by ear | User decision: "test first, presets only if nothing comes out of it" |
| D26 | Variation is **built into the generated patterns** (phrase-aware). Transitions are always performed by hand | No automatic arrangement. Still open for refinement |
| D14 / D23 | The drum lane has one **major style** control (half-time, buildup, two-step, rolling, …) plus **minor overlays** (hats, ghosts/fills, percussion) with density. This is a **first draft** | User's control idea. To be refined on the hardware |
| D6 | Tempo is a performable control with bar-quantized ramps, plus a **half-time** switch | Moves between 170 and roughly 85 feel without changing tempo |
| D15 | Everything is **auto-pitched to the global key** and **locked to the master clock** | Key changes are free when content is stored as scale degrees (R§2.1) |
| D4 / D22 | **Key intent slider**: the position picks a target from Mesh's energy-ordered Camelot transitions. A button commits it at the next phrase. The separate major/minor switch is dropped because MoodLift/MoodDarken are on the slider | Reuses `suggestions/scoring.rs` TransitionType. Every move is compatible by construction |
| D30 | Each style defines **chord progressions in scale degrees**. One is chosen on the Shift layer and applied at the phrase. This is a starting point | Roots move every 1–2 bars in DnB. Degrees stay valid in any key |

### 2.4 Performance and UI

| ID | Decision | Rationale |
|---|---|---|
| D7 / D19 | Each F1 hosts 2 lanes. A lane gets 2 knobs, 2 faders, a 2×4 pad block and 2 play buttons. Global controls sit on a Shift layer and the function buttons (§7) | Every lane is one gesture away, no menu diving |
| D40 / D41 | Lanes **toggle on and off** like stem mutes. The default lands on the next bar. With Quant off it is immediate with a short fade | Needed for transitions |
| D36 | Kit and instrument swaps: hold the lane button and turn the encoder, release to load at the next bar. The whole style is preloaded in RAM | No menus and no USB reads during the set |
| D16 | **Capture** saves a lane's chain or the whole state to a bank and persists it to USB | Keeps good chains reproducible from their seeds |
| D37 | Set recording reuses Mesh's existing recorder on the master bus | No new work |
| D10 / D33 | The screen is a Strudel-style visual: **4 lanes**, each with a code readout with active highlights, a scope and its chain, plus a header strip | It shows the audience what each control does |

### 2.5 Styles

| ID | Decision | Rationale |
|---|---|---|
| D10/D11 | A style is **prepared sound design only**: kits, breaks, samples, synth presets, FX chains, control mappings, value ranges, progressions. It never holds an arrangement | Sound is prepared, music is live |
| D29 | A style is a folder `styles/<name>/` with `style.yaml` and its assets, copied to USB in export Phase 0 | Same pattern as `slicer-presets.yaml`, and self-contained |

---

## 3. Concepts

| Term | Meaning |
|---|---|
| **Lane** | One instrument with its FX chain, its generator, 4 mapped controls (plus a Shift layer), a master seed, a form and an on/off state. There are 4 lanes: Drums, L1, L2, L3 |
| **Role** | A lane's mix role (Drums, Bass, Lead, Extra). Selects its glue treatment and LUFS target |
| **Step** | A 16th note. A bar has 16 steps |
| **Master seed** | A lane's one seed. Four **letter seeds** derive from it: A and B (main), C and D (filler) |
| **Form** | Which letter plays in which bar, as segments: `A7 B1 A'7 C1`. 4 to 64 bars, aligned to the global bar. 4 bars = 1 **phrase** |
| **Generator** | A pure function that turns (seed, position, parameters, harmony) into one bar of events |
| **Style** | The loaded sound-design bundle |
| **Capture** | A saved snapshot of seeds and control values |
| **Global key** | The current key on the Camelot wheel, plus the progression and its current degree |

---

## 4. Architecture

### 4.1 Process and engine ownership

```
mesh-player (mode chosen at startup: dj | live)
├── dj mode:   existing AudioEngine (4 decks × 4 stems)          — unchanged
└── live mode: LiveEngine (4 lanes)                               — new, in mesh-core
               shares: JACK backend, master limiter/clipper, effect host
               (CLAP + native + PD), mesh-midi, set recorder, USB manager
```

- `LiveEngine` lives in a new module `mesh-core/src/live/`. It owns the JACK process callback in live mode. The DJ `AudioEngine` is not touched.
- **Room for mixing later [I]:** `LiveEngine` outputs 4 lane buses into a summed stereo master. Later it can be wrapped as a deck source that exposes its 4 lanes as the deck's 4 "stems", which matches `NUM_STEMS = 4`. None of that is built in v1.

### 4.2 Threads and cores

| Thread | Core class | Work | RT-safe |
|---|---|---|---|
| JACK process | A76 (pinned, as today) | Step player, sampler voices, CLAP instrument + FX processing, glue chain, master | Yes: no locks, no allocation |
| Lane workers | A76 (rayon pool, like stem processing in `deck.rs:1781`) | Per-lane voice rendering and FX in parallel | Yes |
| Bar renderer | A55 | Runs generators and renders the next bar (and its fill variant) for each lane, at least 1 bar ahead | No: may allocate, never blocks the RT thread |
| MIDI/HID | A55 | F1 input, direct command ring (existing `direct_dispatch`) | — |
| UI | A55 + GPU | iced UI, visuals at 30 Hz | — |
| Loader | A55 | Style loading into RAM, kit preload | — |

Communication reuses Mesh's patterns: `rtrb` SPSC rings for commands and rendered bars, atomics for state read by the UI, and `basedrop` for freeing on the RT side (R§4.5).

### 4.3 Master clock

- `LiveClock` holds the tempo in BPM, a sample-accurate position (bar, step, sample offset within the step), the phrase index, and an active tempo ramp (target, length in bars).
- Tempo changes are bar-quantized ramps. Half-time is **not** a clock change. It is a generator context flag, so the grid stays at the set tempo and the patterns render at half-time feel.
- Event times are `tick: u16` within the bar, at **384 ticks per bar** (96 per beat). The step player converts a tick to a sample offset with the current tempo when the event fires.

### 4.4 Rendering pipeline

```
 controls / pads ──► LiveState (A55)  ──►  Bar renderer (A55)
                     seeds, params,         for each lane:
                     key, progression,        gen.render_bar(ctx) → BarEvents
                     toggles                  gen.render_bar(ctx+fill) → FillEvents
                                                     │  rtrb (≥1 bar ahead)
                                                     ▼
                     JACK thread (A76): StepPlayer
                       applies momentary moves (stutter, double, void) to the
                       rendered bar → fires events at sample offsets →
                       Sampler voices / CLAP note events → lane FX → glue → master
```

**Change latency:**

| Change type | Examples | When it takes effect |
|---|---|---|
| Structural | Seed, slot selection, style/kit swap, key commit, progression change | The next bar boundary. Key and progression wait for the next phrase. The renderer re-renders the pending bar, and the swap happens at the boundary |
| Continuous | The 4 mapped controls when bound to sound or FX parameters | At block rate, immediately |
| Mapped to a generator parameter | — | The next bar is re-rendered. The rest of the current bar keeps playing |
| Momentary | Stutter/roll, double rate, void | Applied on the RT side to the already-rendered bar, so the response is instant |
| Fill | — | Uses the fill bar pre-rendered next to every bar |

Generators keep no state: `render_bar` is a pure function of its context. The RNG is a counter-based hash of `(seed, lane, bar_in_phrase, step, stream)`, so a seed always reproduces the same bar. Captures and re-renders after a key change depend on that.

### 4.5 Event schema

A fixed-size `Copy` struct of about 24 bytes, `#[repr(C)]` (R§6.4):

```rust
pub struct LiveEvent {
    pub tick: u16,       // 0..384 within the bar
    pub kind: EventKind, // Trigger | NoteOn | NoteOff | Choke
    pub flags: u8,       // REVERSE | TIE | ACCENT | LOOP
    pub target: u8,      // sampler slot / slice / drum voice
    pub choke: u8,       // 0 = none, else lane-local group
    pub key: u8,         // MIDI key, resolved from scale degree at render time
    pub vel: u8,
    pub cents: i16,      // fine pitch + sample root correction
    pub start: u16,      // start offset within slot/slice (0..=65535)
    pub gate: u16,       // length in ticks, 0 = one-shot
    pub note_id: u16,    // CLAP note_id
    pub decay: u8,       // 0 = lane default
    pub cutoff: i8,      // bipolar offset to the lane filter
    pub glide: u8,       // used with TIE
    pub pan: i8,         // offset to lane pan
}
pub struct BarEvents { len: u16, events: [LiveEvent; MAX_EVENTS_PER_BAR] }
```

**Per-lane parameters** (applied at block rate, not stored in events):
- volume trim from the LUFS calibration
- base pan
- filter base
- envelope defaults
- drive
- send levels
- swing
- CLAP `PARAM_VALUE`

`MAX_EVENTS_PER_BAR` is set in P2. It has to cover drum rolls at 32nds across several sub-voices.

### 4.6 Harmony

- `GlobalKey { camelot: (u8, Mode), progression: usize, degree_index: u8 }`.
- Generators emit **scale degrees plus an octave**. The renderer resolves them to MIDI keys with the key active **for that bar**. A pending key or progression change applies from the first bar of the next phrase, and because rendering is at least one bar ahead, that bar is rendered with the new key.
- **Key intent**: the slider position is mapped onto the energy-ordered transition list from `suggestions/scoring.rs`:

  | Direction | Transitions (closest to centre first) |
  |---|---|
  | Up | AdjacentUp → DiagonalUp → MoodLift → EnergyBoost → SemitoneUp |
  | Down | the mirrored moves down |
  | Centre | stay |

  The header shows the target. The commit button arms the change for the next phrase.
- Sample root keys come from the style metadata, or from detection in the style tool for stem sources. Sampler pitch = target key − root, done by resampling (R§6.1).

---

## 5. Sound engine (P1)

### 5.1 Sampler

One sampler engine serves every sample-based lane: one-shots, drum kits, break slices, multisampled instruments and stem phrases (D31).

- **Per-voice parameters**: all fields of `LiveEvent`.
  - Playback runs forward or reverse from `start`.
  - `gate` 0 plays the slot to its end. Otherwise the voice is held for the gate length and then released.
  - Amp envelope with a `decay` override.
  - Per-voice SVF filter offset by `cutoff`.
  - Pan.
- **Pitch**: resampling with band-limited interpolation. The interpolator's quality and cost get measured in the P1 benchmark. There is **no per-voice time-stretching**, because of the CPU budget.
- **Choke**: a 10 ms fade for voices in the same lane-local group. A "mono" group gives Digitakt-style one-voice tracks.
- **Tie**: ramps the playback rate over `glide`.
- **Voice pool**: fixed size per lane, pre-allocated. When the pool is full, the oldest voice in the lane is stolen. Pool sizes are set from the benchmark.
- **Drum lane**: one lane holding several sub-voices (backbone kit, break slices, hats, percussion), each with its own choke group. That is how a 4-on-the-floor kick and a chopped break play together.
- **Sample data**: decoded into RAM when the style loads and shared through `basedrop::Shared`, the same way stems are handled.

### 5.2 CLAP instrument hosting

This extends the effects-only host (`crates/mesh-core/src/clap/`, R§1.2):

1. The `note-ports` and `audio-ports` extensions, so instruments can have 0 audio inputs and a stereo output.
2. NOTE_ON, NOTE_OFF and NOTE_CHOKE events with sample offsets in the input `EventBuffer`, and NOTE_END handling.
3. Transport info (tempo, bar position) from `LiveClock`.
4. Tie: send the new NOTE_ON before the previous NOTE_OFF, so the synth's own portamento kicks in (CLAP has no glide event).
5. Discovery already tags the `Instrument` category (`discovery.rs:477`). Live mode lists only instrument plugins for lane instruments.

Candidates on aarch64: Surge XT, Dexed and Cardinal (packaged in nixpkgs), plus Odin 2 and Six Sines from other sources (R§5.4). Per-note modulation support is checked with a clack-host scan in P1.

### 5.3 Lane FX chain

- Each lane runs one FX chain through Mesh's existing effect host (CLAP + native + PD). v1 uses a **single-band** chain per lane.
- **[I]** The multiband container is available, but its CPU cost on top of the instruments is not budgeted. Using it is decided after the benchmark.
- The lane's 4 performance controls map to any parameter of the lane:
  - instrument parameters
  - FX parameters
  - generator parameters (§6)
  - glue parameters exposed by the style

  Each mapping has a range and a curve, using the same mechanism as Mesh's deck macros.

### 5.4 Delay and reverb

Delay and reverb are **shared sends** in live mode, one of each, the same way superdough shares them per orbit (R§6.1). Each lane gets a send level. This saves CPU compared with an instance per lane.

### 5.5 Loudness calibration (D20)

- A style tool on the PC (CLI first, mesh-cue later) renders each instrument option through a reference pattern and measures integrated LUFS. It writes a **trim per option** into `style.yaml`.
  - Sample options: the sample itself.
  - CLAP options: the preset played through the reference pattern.
- Each role has a target loudness that the style can override:

  | Role | Example target |
  |---|---|
  | Drums | −12 LUFS |
  | Bass | −14 |
  | Lead | −16 |
  | Extra | −18 |

- On the device, trims are static gains. There is no real-time gain riding. The dynamic stage is the glue and master chain (§5.6).

### 5.6 Glue chain (D27)

All of these are native, fixed in topology, with parameters in the style:

1. **Role high-pass filter** on every non-bass lane, for example lead at about 500 Hz and extra at about 300 Hz.
2. **Bass mono below about 120 Hz.**
3. **Low-band duck** on the bass, lead and extra lanes, triggered by the drum lane's kick sub-voice. It is a dynamic cut at about 50–120 Hz with a tempo-relative release.
4. **Drum bus**: compressor plus saturation.
5. **Shared drive** option: one saturation stage feeding both the drum and bass buses. This is Switch Angel's glue technique.
6. **Master**: bus compressor, then the existing master limiter/clipper.

---

## 6. Generators (P3)

### 6.1 Interface

```rust
pub trait LaneGenerator: Send {
    fn render_bar(&self, ctx: &BarCtx, out: &mut BarEvents);
}
pub struct BarCtx<'a> {
    pub seed: u32,
    pub bar_in_phrase: u8,      // 0..4
    pub phrase: u32,
    pub fill: bool,
    pub half_time: bool,
    pub params: &'a [f32; MAX_GEN_PARAMS], // normalized, from mapped controls
    pub harmony: HarmonyCtx,    // key, chord degree this bar, scale
    pub drum_mask: &'a [u8; 16], // drum lane's rendered onsets (for bass gap-following)
}
```

- Generators are pure and deterministic. They allocate only into `out` and are unit-tested with snapshots (D38).
- The drum lane is rendered first each bar, so its onset mask can feed the other lanes. This is how the bass avoids the kick or follows the gaps.

### 6.2 Families (D42)

| Family | When | Building blocks |
|---|---|---|
| **Procedural** (first) | Default in P3 | Metric-weighted step probability, Euclid/Bjorklund, density thresholds, seeded break chops that keep slices mostly in order, root-weighted note pools, motif repetition with phrase-aware variation, velocity tiers per metric level |
| **Idiom data** (fallback) | Only if the ear tests show procedural output sounds random | Grids-style per-step importance maps per role, curated rhythm-cell libraries per archetype. Stored in the style |

Both families sit behind `LaneGenerator`. Switching or blending them is a style setting, not an engine change.

### 6.3 Research-backed rules applied by default (R§7.3)

| Rule | What it means |
|---|---|
| A2 | The drum backbone (kick and snare) is locked to the major style. Overlays only add events where the backbone is empty |
| A3 / D26 | Variation is phrase-shaped: bars 1–3 repeat and bar 4 varies. This is part of the pattern, and transitions stay manual |
| A4 | Break slices default to their original position. A mutation swaps in a slice of the same type. Fully random order only happens in the fill bar |
| A5 | Feel comes from velocity tiers (accent / normal / ghost at 30–40%). There is no timing jitter. Swing applies only to synthesized hats and ghosts, and defaults to 0 |
| A6 | Bass pitch is root-heavy (root, octave, 5th, b7, b3) and follows the progression. Bass rhythm follows the gaps in the drums. Accent and slide flags drive the synth |
| A7 | Leads are 2–4 note motifs placed in the bass's rests |

Each rule is a parameter of the generator, so an ear test can switch it off.

### 6.4 Drum lane: major style and overlays (D23, first draft)

- **Major style** is an ordered set of backbone archetypes defined in the style. The research ordering is half-time → two-step → rolling → jungle → hardcore, with buildup as a special case (R§7.2). The style control moves along that order, and the change lands at the bar.
- Whether the control blends neighbouring archetypes (Grids-style) or switches between them is decided by ear test in P3.
- **Overlays**: hats, ghosts/fills, ride (crash on the phrase start), shaker, percussion and a chopped top loop. One density control brings them in, in an order the style sets (`enter_*` entry points, `overlay_*` amounts). See the 2026-10-15 change log entry.

---

## 7. Control surface (P4)

These are starting assignments. Every mapping is a musical decision and gets ear- and hand-tested on the hardware (D38).

### 7.1 Per lane

Left F1 hosts Drums and L1. Right F1 hosts L2 and L3.

| Control | Lane A (columns 1–2 / left half) | Lane B (columns 3–4 / right half) |
|---|---|---|
| Knobs | knob 1, 2 | knob 3, 4 |
| Faders | fader 1, 2 | fader 3, 4 |
| Pads (2×4, row-major 1 2 / 3 4 / 5 6 / 7 8) | Drums, bottom left to top right: kick, snare, break, perc, hats, ride, top, shaker, each turning its voice on or off against the seed's arrangement. Shift + bottom row: form ◀/▶. Pitched: oct−, oct+ (both = 0), half rate, double rate, form ◀, form ▶, 7–8 free | same |
| Pads with Sync held | momentary: stutter · fill / double · void | same |
| Play button A | lane on/off, landing on the next bar (immediate with a fade when Quant is off) | same |
| Play button B | hold + encoder: reroll the master seed. Tap: back to the previous seed | same |
| Size / Browse held + encoder | browse kit/instrument, release loads at the bar | same |

> Superseded layouts: v1 (4 chain slots), v2 (8 bar pads, change log 2026-10-14). Current layout: change log 2026-10-15.

**Default control mappings** (each style can override them):

| Lane | Knob A | Fader A | Knob B, Fader B |
|---|---|---|---|
| Drums | major style | density | sound/FX |
| L1–L3 | complexity | density | knob B: sound/FX, fader B: note length. Shift + fader B: what fader B held before |

### 7.2 Global (Shift layer and function buttons)

| Control | Function |
|---|---|
| Shift + left fader 1 | Key intent (soft takeover) |
| Shift + left fader 2 | Tempo (ramp target) |
| Shift + encoder (left) | Select progression, applied at the phrase |
| Encoder push (left, with Shift) | Commit the key intent |
| Quant | Toggles quantized vs immediate lane toggles |
| Capture | Capture the selected lane's chain. Shift + Capture captures the whole state |
| Reverse | Half-time toggle |
| Browse / Size / Type / Sync | Kept free for P4 decisions: capture recall, master FX, page |

The 7-segment displays show the key on the left and the tempo on the right.

---

## 8. Screen (P5)

The screen is 2880×864. It is built as its own view in iced and uses the GL/Panfrost backend (R§1.4).

```
┌────────────────────────────────────────────────────────────────────────────────────┐
│ KEY 8A Am → 9A Em (next phrase) │ 172 BPM │ ½T off │ prog i–VI–iv–v │ ▮▮▮▯ phrase  │
├────────────────────────────────────────────────────────────────────────────────────┤
│ drums  .style("rolling").dens(0.62).seed[50 50 51 ⟨77⟩]   ~~scope~~   [■][■][■][□] │
│ bass   .synth("surge:reese").cx(0.4).dens(0.5).seed[...]  ~~scope~~   [■][■][□][□] │
│ lead   .sampler("pendulum-lead").cx(0.2).seed[...]        ~~scope~~   [■][□][■][□] │
│ extra  .sampler("pad-dark").dens(0.3) ░muted░             ~~scope~~   [□][□][□][□] │
└────────────────────────────────────────────────────────────────────────────────────┘
```

- **Code readout**: each lane's state is rendered as Strudel-like text. Every value that is moving outlines with the lane colour while its control moves. The active slot's seed and the firing step are highlighted. Each event carries its "source span" (the lane field that produced it), so highlighting needs no parsing (R§5.1).
- **Scope**: one per lane, in the lane colour, drawn from a decimated ring buffer of 256–512 points the RT thread fills.
- **Chain**: the 4 slots with a playhead, with fill flashes and probability shown as fill level.
- **Rendering budget**:
  - Visuals run at 30 Hz, separate from audio.
  - Static text sits in long-lived caches and live values in small separate caches.
  - Monospace font with fixed-width digits.
  - Highlights are drawn as quads.
  - Scopes use one custom shader primitive.
  - All of this is measured on the board before it is accepted (R§5.2).
- Until P5 a minimal debug view stands in: lane state as plain text.

---

## 9. Styles, captures, export (P6)

```
<collection>/styles/<name>/
├── style.yaml        # lanes, roles, instrument options, FX chains, mappings,
│                     # ranges, archetype order, progressions, glue params,
│                     # LUFS trims, sample root keys
└── assets/           # samples, breaks (+ slice markers), stem phrases, CLAP presets
<collection>/captures/<style>/<timestamp>.yaml   # seeds + control values
```

- `config.yaml` gains `mode: dj | live` and `live.style: <name>` (D35).
- On load, every asset in the style is decoded into RAM. Style size is capped: the cap is set from measured RAM use, and the 8 GB board already uses about 1.5 GB in DJ mode.
- USB export Phase 0 copies `styles/` next to `slicer-presets.yaml`. Captures are written to the USB collection during a set and travel back like the play history does.
- Break slicing reuses Mesh's beat-grid/slice analysis offline. Slice markers and slice types (kick, snare, ghost, hat) are stored in the style. Manual correction is allowed, since the exact amen positions are unverified (R§7.2).

---

## 10. Phases and hardware test gates

| Phase | Scope | Automated checks | Hardware ear/hand gate (D38) |
|---|---|---|---|
| **P1 Sound engine** | Sampler, CLAP instrument hosting, lane FX, sends, glue, LUFS tool, `LiveClock`, hand-written fixed patterns, startup mode switch, **board benchmark** | Unit tests on voice DSP. Offline WAV renders of fixed patterns. Benchmark binary on the board | Do drums, bass and lead sound finished through the glue? Sign off each role's timbre. Set voice and CLAP budgets |
| **P2 Pattern core** | `LiveEvent`, `BarEvents`, bar renderer thread, step player, quantized structural changes, momentary moves, harmony resolution | Timing tests (sample-accurate onsets, bar-boundary swaps, no RT allocation), key-change re-render tests | Is the timing tight on the board? Do key changes land cleanly? |
| **P3 Generators** | Procedural families for drums (major style + overlays), bass, lead. Progressions | Seed-determinism snapshot tests | Does the output sound musical rather than random? Procedural vs idiom-data A/B. Archetype order |
| **P4 Performance surface** | F1 live layout, chains, seeds, toggles, kit browse, capture, key intent, tempo, half-time | Mapping and dispatch tests | Can a 30-minute set be played without menus? Revise mappings |
| **P5 Visual UI** | Code-readout lanes, scopes, chain grid, header | GPU frame-time measurement on the board | Is it readable from the booth? Does it look the part? |
| **P6 Styles & export** | Style bundle schema, USB export, captures persisted, first breakbeat style | Export round-trip tests | A full set from USB on the board |

Each gate's outcome is recorded in this document's change log before the next phase starts.

---

## 11. Risks

| ID | Risk | Mitigation |
|---|---|---|
| R1 | **Musical feasibility.** Procedural output sounds random, or playing it isn't fun | The fallback idiom-data generator family (D42). Ear gates in P1 and P3 come before the UI investment |
| R2 | **CPU.** 2 CLAP synths, sampler voices and FX don't fit on the A76 cores | P1 benchmark before budgets are fixed. Cheaper interpolation. Fewer CLAP lanes. Sampler-based synth presets |
| R3 | **CLAP instrument support on aarch64.** Missing per-note modulation, crashes, poor builds | Scan the candidate plugins in P1. Sampler lanes work without CLAP |
| R4 | **GPU.** The code readout plus 4 scopes exceed the Mali budget | 30 Hz cap, cached text, a single scope shader, measured in P5 |
| R5 | **Sound design quality** decides the result (R§7.1.9) | Sounds are signed off first (D28). Glue is on by default |
| R6 | **Control density.** 4 controls per lane aren't expressive enough, or the Shift layer gets too busy | Mappings are data in the style. Revised at the P4 gate |
| R7 | **Repitch artefacts** on stem phrases moved far from their root | Limit the range per sampler option in the style. Allow multiple root versions of a phrase |
| R8 | **Break slicing accuracy** | Offline slicing with manual correction. Slice types stored in the style |

---

## 12. Open questions

These have no answer yet and are expected to be settled during the phases:

1. The exact pad geometry for the 2×4 lane blocks, and how captures are recalled (P4).
2. Whether major style archetypes blend or switch, and their order (P3).
3. How variation and transitions should feel beyond "phrase-shaped variation plus manual transitions" (D26, P3–P4).
4. How progressions are chosen and how many there are per style (D30, P3).
5. The multiband FX budget per lane (P1 benchmark).
6. A second display or stream output, and mixing live lanes with track decks (after v1).

---

## Change log

- 2026-10-13: initial design from grilling rounds 1–5 (D1–D42).
- 2026-10-14: §4.2 core pinning confirmed (user). Live mode inverts the DJ layout: the live audio thread pins itself to A76 cores 5–7 on its first block, the bar renderer runs on A55 cores 1–3, and core 4 stays with UI rendering (`rt::live_cores`). **Open (P1 board test):** the NixOS image tunes the A55 cluster for audio at boot (`nohz_full=0-3`, `rcu_nocbs=0-3`, `irqaffinity=4-7`, audio IRQs on CPU 0, deep idle disabled on 0–3; `nix/embedded/hardware.nix`, `audio.nix`). In live mode that tuning protects the wrong cluster. Measure `live-bench` on the board first, then decide whether the irq-affinity/cpu-idle services become mode-aware (boot parameters can't change per mode).
- 2026-10-14: the live audio thread flushes denormals (FTZ/DAZ on x86, FPCR.FZ on aarch64). On x86 this took the fixture style's glue stage from 0.80 to 0.04 ms per block.
- 2026-10-14: All non-musical code for P1–P6 is on `live-performance-mode`: engine, sampler, CLAP instruments, glue, generators, F1 surface, LiveApp screen, styles export, captures. Tests: mesh-core lib 547, mesh-player 45, mesh-midi 58, mesh-widgets 7, all passing.

  Not verified yet:
  - on-screen visuals
  - the aarch64 build of `rt::flush_denormals`
  - anything on the Orange Pi or with real F1s

  Tools for the hardware gates:
  - `live-bench` — CPU budget on the board
  - `live-render` — offline WAV renders
  - `live-patterns` — ASCII pattern grids
  - `style-calibrate` — LUFS trims
  - `clap-scan` — CLAP instrument capabilities on aarch64

  Implementation deviations from this document:
  - **Key intent:** 4 moves per side, ordered by `transition_energy_direction`. Only 4 moves are reachable from any one key.
  - **Pad geometry:** columns 1–2 control lane A and columns 3–4 lane B. Rows 1–2 are chain slots, rows 3–4 are momentary moves.
  - **7-segment key display:** Camelot positions 10–12 show as A/B/C, so 10A reads `AA`.
- 2026-10-14: F1 layout v2 (user). Chains have 8 slots and each lane has a loop length of 1, 2, 4 or 8 bars (default 4). A lane plays slot `bar % loop_len` of the global bar, so lanes line up at phrase boundaries. Generators still get `bar_in_phrase = bar % 4`, so harmony and phrase shaping keep 4-bar phrases. All 8 pads of a lane's half are bar pads, row-major (1 2 / 3 4 / 5 6 / 7 8). Tap selects the bar, hold + encoder reseeds it, and the encoder alone reseeds all 8. Shift (same unit) + bar pad n sets the loop length to the largest of 1/2/4/8 ≤ n, landing on the next bar. Pads past the loop are very dim but stay editable. Holding Sync switches that unit's pads to moves (row 1 stutter/fill, row 2 double/void, rows 3–4 dark). Releasing Sync releases any held moves. Sync no longer recalls captures and no F1 control recalls. Captures are now version 2 (8 seeds + `loop_len`). Version-1 files load with their 4 seeds repeated and length 4. Keyboard: `y u i o` select bars 5–8, and Shift + a bar key sets the loop length.
- 2026-10-14: Per-lane seeded chords and new knobs (user). This supersedes D30's global progression for sound. Each pitched lane draws the chord root of every chain slot from that slot's seed (`harmony::seeded_chord`: diatonic, no diminished chord, slot 1 leans to the tonic). The lane's loop is therefore its own progression. Reseeding a bar changes its chord, and reseeding the lane changes all of them. All lanes stay in the one global key. The style's `progressions` and the global progression state remain but no longer reach the generators. Left Shift + encoder and the `p` key are unassigned, and the header shows each pitched lane's current chord. New generator params: `spread` on bass and lead (pitch range: few notes near the root at 0, wide moves and octave jumps at 1) and `mutate` on drums (seeded displaced, extra and dropped backbone hits; the bar's first kick and the backbeat stay). `MAX_GEN_PARAMS` is now 12. Starter style knobs: drums style/dens/mutate/verb, bass cx/dens/spread/duck, lead spread/dens/cutoff/delay, extra spread/dens/reso/verb.
- 2026-10-14: Glue dynamics (user). Master compressor: 4:1, 25 ms attack, 10 ms release. The starter style's threshold is −8 dB, which gives about 4 dB peak reduction over 32 bars at default controls (`live-render` now prints it). The duck is now full band and fixed per style, with no control. Only the main kit's kick and snare key it (backbone targets 0 and 1), never break slices. Bass: 8 dB on the kick, 2 dB on the snare, 50 ms release. Lead and extra: 2 dB on the kick, 4 dB on the snare, 120 ms release. Attack is 0.5 ms. The `duck_depth` glue control is removed, and `glue.duck` takes `attack_ms`, `bass` and `others` (`kick_db`, `snare_db`, `release_ms`). The starter style's bass fader 2 is now `gate`.
- 2026-10-14: Per-lane mix processing (user). Bass, lead and extra each get a compressor then a saturator before the duck: 3:1, 10/80 ms, 6 dB knee, 1 dB makeup, about 2–3.5 dB peak reduction. The bass threshold is −5 dB because bass lanes peak near full scale; the other lanes use −21 dB. Saturation is drive 3 dB (bass 4 dB) at 50 % wet. The drum bus has two hard-knee compressors before its saturation. Comp 1 is 4:1, 15/59 ms, threshold −8.5 dB, giving 3.8 dB peak reduction. Comp 2 is 2:1, 30/20 ms, threshold −12 dB, giving 2.0 dB. The master runs compressor → −4 dB output gain → saturator (drive 2 dB, 40 % wet) → limiter (about 1.8 dB peak) → clipper. Thresholds were measured with `live-render` (per-stage peak reduction) on the starter style at default controls. The new stages are runtime defaults and are not in the style schema yet. The starter style no longer overrides the drum-bus compressor. Engine cost: 56.6× realtime on the PC.
- 2026-10-14: Sounds, FX slots and plugins (user).
  - Drum kits come from kick/snare folders (kit n = kick n + snare n) and a break folder (1/2/4-bar loops), via `scripts/live-starter-style.sh --kicks/--snares/--breaks`. The 808/909 kits are removed.
  - Bass is the round sub plus five Surge XT reese/hoover patches. Extra is four Surge pads. CLAP instruments load `.fxp` chunk presets.
  - Every FX chain takes native or CLAP effects in any order. The chains are lane `fx`, the drum lane's `break_fx` (break slices render on their own sampler bus), `glue.drum_bus_fx` (end of the drum bus) and `glue.master_fx` (before the limiter). `source: airwindows` selects an Airwindows Consolidated algorithm by name, starting from its defaults.
  - New native effects: `compressor`, `eq` (low cut plus two bells) and `transient` (SPL-style differential envelopes).
  - Starter break chain: EQ (120 Hz 12 dB/oct cut, −3 dB at 200 Hz, +3 dB at 3.5 kHz) → compressor (4:1 hard knee, 50/50 ms, −18 dB) → transient (+4 dB attack). Starter drum bus: Pop2 → ButterComp2, as 1176/LA-2A stand-ins. No FOSS 1176/LA-2A plugin exists for Linux/aarch64.
  - Live mode scans `<collection>/effects/clap/` and `effects/clap/<arch>/` of the style's collection. Export copies `effects/clap` to the stick, following symlinks.
  - Open: aarch64 builds of Surge XT and Airwindows Consolidated for `effects/clap/aarch64/` (nixpkgs marks airwin2rack x86_64-only).
- 2026-10-14: Readable FX settings and multiband (user).
  - FX entries take `settings`: parameter name → real units (`low_cut: 120`, `threshold: -18`). This works for native effects and CLAP plugins, including Airwindows names read after the algorithm loads. An unknown name warns with the list of known names.
  - New native `multiband`: 6 LR4 bands with allpass compensation (flat when idle), downward and upward compression per band, band gain, `up max` and `up gate`.
  - The starter style puts a multiband first in the bass (neuro smiley: sub lightly held, high bass firm, mids 8:1 and −3 dB, top lifted), lead (lighter) and extra (gentle) chains.
  - LSP's CLAP multiband compressor/dynamics processor was rejected. Each compressor band is up or down, not both. Most parameters are exposed only as normalized log values, and it carries FFT analysis.
  - Engine cost on the PC is now 17× realtime (worst block 29 %). The Surge pad (extra lane) dominates, so OPi headroom must be checked (G2).
  - Future, not scheduled: map the 4 lane macros (the performance controls) onto many effect and generator parameters with ranges, like Mesh's multiband FX macros.
- 2026-10-14: Loudness after processing (user), plus drum EQ and balance changes.
  - `style-calibrate` runs two passes. Pass 1 keeps the input trims (`lufs_trim_db` to `loudness`), so every option drives its lane processing at the same level.
  - Pass 2 measures each option / drum kit after the lane's whole processing: break bus, lane FX, glue high-pass, mono-below, compressor and saturation. No duck and no buses. It plays the lane's own generator and writes `output_trims_db` to reach `output_loudness`.
  - The glue applies the output trim right after the lane dynamics, ramped per block. It is linear, so one pass is exact.
  - Starter output targets are the balance approved by ear, then drums +1 dB and bass −1.5 dB. Drum-bus thresholds rose 1 dB (`comp` −4, new style key `comp2` −11), so the drum increase survives the bus.
  - Drum lane EQ: 40 Hz low cut, +1.5 dB at 100 Hz, −2 dB around 320 Hz. Breaks get more highs: +4.5 dB broad around 5 kHz instead of +3 dB at 3.5 kHz. The EQ's second bell now reaches down to 200 Hz.
  - The starter script always runs the bins through `direnv`. A shell with `IN_NIX_SHELL` set but no devshell environment made CLAP fail to load during calibration.
- 2026-10-15: Drum pools, forms and lane pads (user grill Q1–Q33). Supersedes F1 layout v2, the 8-slot chains and loop length.
  - **Drum voices**: kick, snare (with ghosts), hats (closed / open, choked), ride (crash on the phrase start), shaker, percussion, top loop, break. New sub-voices `ride`, `shaker`, `top`. Target layout: backbone 0–7, hats 8–39, ride 40–55, shaker 56–71, perc 72–103, break 104–167, top 168–199 (`generator::layout`).
  - **Pools** (Q4): hats, ride, shaker, perc and top keep their default option when the kit is browsed. The bar's sound seed (the master seed in main bars, the letter seed in filler bars) picks one closed and one open hat, one ride and crash, one shaker, a few percussion hits and one top loop. Kit browsing steps only the kick/snare pair and the break. Samples carry `tag: closed | open | ride | crash`, top loops `bars`.
  - **Top loops** (Q2): one target per loop, chopped in 16ths through the event `start`, in order across bars. The density drops chops (each held one step), `chop` stutters and swaps. Loops more than 10 % off the style tempo are left out by the style builder.
  - **One density** (Q5): `overlay_density` takes an entry point per voice. Starter: hats, ghosts, perc from 0, shaker 0.35, ride 0.5, top 0.6.
  - **Mutes** (Q6, Q33): the 8 drum pads toggle voices (`DrumVoice`). RT-side filter in `Lane::fire`, landing on the next bar or immediately when Quant is off; a muted break or top loop fades its choke group.
  - **Master seed and forms** (Q16, Q21–Q30): each lane has one master seed. Letter seeds derive from it (A = the master seed). A form is segments of a letter and a bar count, 4–64 bars, aligned to the global bar; a form change lands on the next phrase (4 bars). `A'` plays A with the second half from a derived seed. Starter forms: `form::DEFAULT_FORMS` (style key `forms`, `default_form`).
  - **Seed classes** (Q23): A/B are main (mostly straight, sometimes light swing, ±1 tick drift). C/D are filler (40 % triplet reflow, heavy swing, ±3 ticks, generator parameters pushed up by `FILLER_BOOSTS`, drum fill in the last filler bar). Applied in the renderer (`form::Feel`).
  - **Chords** (Q24, Q31, Q32): the master seed draws a 4-chord progression (1 or 2 bars per chord). It restarts at every main segment, runs on through filler segments (35 % passing chord), and the bar before the form returns to A plays IV or V. Supersedes the per-slot seeded chords.
  - **Seeds on Play B** (Q10, Q17, Q18): hold + encoder rerolls; the seed before the edit becomes the previous seed; a tap swaps back. The encoder alone does nothing.
  - **Pitched pads** (Q12, Q14): octave ±1 per press (±2, both pads = 0, applies from the next note, RT-side). Half / double rate latched, exclusive, next bar (renderer). Pads 5/6 step the form.
  - **Note length** (Q13): fader B maps to the generator `gate`. At 0.9 and above every note ties into the next one and single notes glide (renderer). Starter: lead delay and extra reverb moved (Shift + fader B, knob B).
  - **Shift layer**: Shift + any knob / fader drives the lane's `shift_controls` (left Shift + faders 1/2 stay key and tempo).
  - **Captures** (Q15, Q20): version 3 stores the master seed, form, Shift-layer positions, octave and drum mutes. Versions 1 and 2 load with their first slot seed.
  - **Style builder**: `--library DIR` (Kicks, Snares, Breaks, Cymbals, Percussion) sorts every file into a pool by name, length and decay, writes `sample-classes.txt`, and reads `sample-overrides.yaml`. Undecodable files are skipped.
  - Open: chord / letter interplay beyond the turnaround (Q24), pitched pads 7/8, capture recall, form list tuning by ear (Q26).
- 2026-10-15: Seeded arrangements, melody cells, bass jumps (user feedback after the first play; supersedes the density entry points and the mute pads of the entry above).
  - **Why**: seeds sounded alike (rhythm came from fixed importance tables), every voice was always on, and muting the kick left the break's kick slices on every kick step.
  - **Drum recipe** (`generator::recipe`): the sound seed picks 2–3 of ride / shaker / perc / top plus the break (80 %); kick, snare and hats always. Each voice plays one pattern from a small vocabulary (hats: 8ths, pickups, last-beat triplets, gallop, triplet 8ths; ride: the 1, quarters, the 1 then triplets through the second half, offbeats, 1 and 3; shaker: 16ths, offbeats, pickups, triplets; top loop: whole, second half, last bar, every other bar, offbeats; perc: seeded Euclid). Letter seeds vary the patterns, so A and B differ.
  - **Density**: 0 = basic groove (the archetype's kick anchors, lean seeds only the strongest; the backbeat, some seeds only every second snare; offbeat hats). Layers then enter one at a time in a seeded order between 0.1 and 0.9: full kick (with a seeded syncopated kick), full snare, ghosts, two hat upgrades, the seed's extra voices, the break's full roles.
  - **Break roles per bar**: gaps (free steps only), layer (through at 60 %, off the backbone), tail (second half of the phrase's last bar), combined per bar. The break never plays on a sounding kick or snare; a slice before one stops in time. Before its layer only the tail plays. `mutate` changes one voice's pattern or flips a break role per bar.
  - **Pads enable / disable** against the seed's arrangement (`voice_flips` XOR the bar's voice mask from the generator, applied RT-side). Voices the seed leaves out still render so a pad can bring them in. A new drum seed clears the flips. Pad order bottom left to top right: kick, snare, break, perc, hats, ride, top, shaker.
  - **Lead cells** (`melody: 1`, wired in the starter style; the motif mode stays): the seed builds 2–3 cells from rhythm figures (4 triplet repeats, 8th run, long note, syncopated pair, dotted, short pair, 16th roll, quarter pulses), contours (repeat, down, up, leap and back, zigzag) and articulations (staccato, even, held), ordered in call and response over the phrase. `spread` = range (a 5th to about two octaves), `complexity` = busier and more syncopated figures, `density` = 1–3 figures per bar, fader B scales every note length.
  - **Bass**: notes stay in one octave (fixed small pool weight); `spread` = size of a seeded odd block of 1–3 notes per bar (45 % of bars, never the first note): a 5th, an octave, an octave and a 3rd, up to an octave and a 5th (+19).
  - Main seeds swing more often (50 %, 0.1–0.55).

- 2026-10-15: Drum pads as overrides (user: pads did not act at once). Quant was already off by default. The cause: a voice whose density layer had not entered was not rendered, so enabling it had nothing to unmute. Now ride, shaker, perc, top loop and the break render at every density, and the generator's voice mask holds the seed's voices whose layer has entered. The pads set overrides on top of that mask (force on / force off, `layout::voice_override`, packed in a `u16`), applied RT-side, so both directions act immediately with Quant off. A press turns a playing voice off and a silent one on; a new drum seed clears the overrides. The break no longer plays the phrase tail below its layer.
- 2026-10-15: Bass note length from the density, lead lower, bass half rate (user). The bass `gate` parameter is gone: notes are held to the next note (up to 16 steps) at density 0 and cut to 30 % of the gap (at most 4 steps) at density 1. Starter bass fader B is now `slide`. The lead preset's register defaults to the bottom (an octave lower). Lanes take `base_rate` (normal | half | double), the lane's centre speed: the rate pads go one step slower / faster from it (−2..+1 octaves of speed). The starter bass's centre is half speed, so its half pad gives quarter speed and its double pad the generator's own speed.
- 2026-10-15: Parameter moves act at once (user: the density fader waited for the next bar). A generator parameter move re-renders the playing bar too, in the harmony it is playing in, and ships it as a replacement packet (`BarPacket::replace`). The RT player swaps it in on its next block and reads on from its position, so nothing repeats; the drum lane's voice mask and form position update with it. Seeds, forms, rates, key and progression still land on their boundaries.
- 2026-10-15: Synth length and attack from the density, mudpie stream samplers (user).
  - Lead, arp, stab and pad notes take their length from the density like the bass (held to the next note when sparse, 30 % of the gap when dense; pads 100 % → 60 %). The lead `gate` parameter and the renderer's legato tie are gone; the starter lead and extra lanes put the delay send on fader B.
  - Every pitched note carries an `attack` (new event field, log scale 1 ms .. 2 s): 80 ms at density 0 to 10 ms at density 1. The sampler's attack follows a quarter sine (round, eases into the top). CLAP lanes set Surge XT's "A/B Amp EG Attack" from it (log2 seconds −8..5 on 0..1, set when the attack changes); other CLAP synths keep their own attack.
  - **Stream samplers** (`stream: true` on a sample): a long rendered stream (mudpie) played as a pitched instrument. Notes start at the event's `start` (other pitched slots ignore it); generators set it from the sound seed (a region 2–85 % into the file) with ±0.4 % per note, so each hit lands slightly elsewhere and a new seed moves the region. Everything else (ADSR, density, length, attack, key) is the same as for any sampler option.
  - The style builder adds every file of the library's `Mudpies/` as a stream option of the bass and the lead lane. Root: the median YIN note of 16 windows spread over the file (voiced ones), reported with the spectral estimate. Both current mudpies measure F1.
- 2026-10-15: Bass shape, drum room, Play B, synth OTT (user).
  - **Bass**: the rhythm density has a floor (0.32 at density 0, eased in so the middle is unchanged), so a sparse bass still grooves. Each note is either a short stab (35 % of the gap, at most 2 steps) or held; density sets the share of stabs (20 % → 80 %). The pitch pool reaches the 5th and 7th below the root (never more than a 6th under it, or 4 semitones under the key's register root); spread scales how often notes leave the root (pool weight 0.2 → 1.3), how often an odd block happens (15 % → 70 % of bars) and how far it jumps (up to +19).
  - **Drum room** (`room`, new native effect): an 8-line feedback delay network (Householder mixing, damping in the loop, decay in RT60 seconds) with pre-delay and a low cut / high cut on the tail only. Sits in the drum lane FX after the EQ, so before the lane and bus compressors. Starter values from common drum-reverb guidance, at the short end for DnB: 20 ms pre-delay, tail 350 Hz – 7 kHz, damping 0.4. The density control also drives it: mix 22 % → 8 %, decay 1.4 s → 0.4 s. Drum knob B is the room size (0.6 – 1.4); the shared-reverb send is no longer on the drum controls.
  - **`also` targets**: a control can move more targets, each with its own range and curve.
  - **Play B**: a press rolls a random new master seed (lands on the next bar; the display shows it while held). Hold + encoder and the tap-to-swap are gone (keyboard Backspace still swaps back).
  - **OTT**: a 6-band multiband in OTT style (down 8:1 above −30 dB, up 4:1 below −45 dB, 4 ms attack) at 30 % mix after the lead and extra lanes' own multiband.
- 2026-10-15: Mix and room fixes (user). Output targets bass −2 dB (−11), lead and extra +2 dB (−12, −22.5). The lead preset's root sits an octave lower again (A2 at the base register; other presets unchanged). Melody cells: up to 4 figures per bar (from density 0.3, 0.55, 0.8), 12 start positions, notes no shorter than 0.8 of their articulation at full density. Bass rhythm floor 0.45 at density 0. The drum room is shorter and drier (mix 12 % → 4 %, decay 0.7 s → 0.2 s, size 0.5–1.0, 400 Hz – 6 kHz, damping 0.6) and sidechained: the drum lane keys the room on every kick and snare it fires (`ROOM_KEY_PARAM`), ducking the tail 15 dB and releasing over 180 ms.
- 2026-10-15: Immediate controls and letter variation (user). Knobs and faders act on the first move (no soft takeover, also after a Shift-layer change or a recall). Letters B, C and D move density, complexity, variation, slide, spread, accent, swing, chop and mutate by a seeded offset of 40–100 % of a per-parameter maximum (0.25–0.45), up or down from the control value (`LETTER_OFFSETS`). A plays the controls as set. Filler boosts still add on top for C and D.
- 2026-10-15: Sampler pitch envelope and sparse bass (user). Pitched sampler notes shorter than 0.6 s start with a pitch rise (4 ms up, up to +1.5 semitones at 0.12 s and shorter, fading out with length) that falls back over 70 % of the note, 30–100 ms. Surge lanes are unchanged. Bass rhythm floor 0.62 eased with density³, and at low density the off 16ths lose importance (0.35, gone by density 0.6) so sparse lines sit on the 8ths. Short notes are longer when sparse (60 % → 35 % of the gap). At slow rates (bass half by default) a note held over the middle of the rendered bar now continues from the next bar's downbeat instead of stopping at the barline.
- 2026-10-15: Mix and bass envelope (user). Letters move complexity (cx) by up to 0.6. Bass attack runs 120 ms → 20 ms over density (synths keep 80 → 10 ms), bass sampler release 150 ms, and Surge bass presets get their amp release raised to at least 150 ms. The native EQ gains a low shelf (`low_shelf_freq`, `low_shelf_gain`, Q 0.5). The drum bus ends with two shelves, +2 dB at a 200 Hz corner (kick) and +1.5 dB at 500 Hz (snare body); there is no per-voice kick or snare bus, so they act on the whole kit. The break bus ends with −5 dB. The bass FX end with a −4 dB bell at 315 Hz, Q 2.2.
- 2026-10-15: Song forms, per-note variation, a lead that follows the bass, stacks (user). Every form plays twice: as written, then a seeded variation of the same length on the same letter seeds (`Form::pos_varied`). Segments keep their bars and class and swap parts to the partner letter (A/B, C/D): whole, every second bar, the last quarter or the second half. `A3 C1` may come back as `A1 B1 A1 D1`. Every letter, A included, also takes a smaller seeded parameter move in the second pass (35 % of the letter maximum). Notes read their parameters with a seeded per-note jitter of half the letter maximum (`note_param`; bass density, complexity, spread, slide, accent; lead complexity and spread). The renderer passes the bass lane's onsets to later lanes (`BarCtx::bass_onsets`). Lead `follow: 1`: rhythm from unison with the bass (cx 0) to a counter-rhythm in its rests (cx 1), a seeded home degree (root .35, 5th .25, b7 .15, b3 .12, 4th .08, b6 .05, flattened by spread, a second home in the back half of the phrase above spread 0.5), stacks of 1–4 notes by spread (octave/5th, power/triad, rave stab/m7/4ths). No two tones closer than a 5th below C3. Rhythm density 0.42–0.75 (about 4 hits per bar at the bottom, 13 at the top). Lead fader B is cx (no delay control). New lead/stab/pad parameter `stack`: tones added root, 5th, 3rd, octave, 7th, 9th, each fading in from 35 % velocity. The extra lane's fader B is stack. Lead +6 dB (−6), extra +6 dB (−16.5). Room at full density is 7 % mix, 0.4 s.
- 2026-10-15: Bass filter and playhead motifs (user: mudpies sounded like one sound replayed). Sampler bass lanes get a lowpass whose slope morphs with the cutoff, from 6 dB/oct at 700 Hz through 12 to 24 dB/oct at 15 kHz (`LaneSamplerParams::filter_morph`), and whose cutoff glides from the previous note's over 60 ms (`cutoff_glide`). Each bass note sets its cutoff in that range from the seed's base (0.2–0.55), the density (+0.25), a seeded step motif (±0.22), the pitch above the root (+0.15 per octave) and the accent (+0.15). A step motif (`step_motif`) is a seeded value per 8th that repeats in a cycle of 2–8 8ths. Stream (mudpie) notes start at one of 2–4 seeded playhead anchors per letter seed (up to 12 % of the file around the region), picked per step by a motif, so blocks differ and bars repeat with intent. Stacked tones start slightly apart.
- 2026-10-15: Per-sample loudness (user: some hats far too loud; everything in LUFS). The loader measures every sample's integrated LUFS when the style loads (hits shorter than a 400 ms gating block padded with silence) and gains it to −12 LUFS (`SAMPLE_LOUDNESS_TARGET`, ±24 dB at most). Option trims and the lane calibration then set the balance on top. So far this runs at load time; it belongs in the analysis step later (a per-sample gain written into the style).
- 2026-10-15: 128-bar forms, structure breaks, sound swaps, playhead over the whole file (user). A written pattern repeats to 64 bars (`FORM_BARS`); over its main bars the structure puts C at bar 16 and 48, D over 31–32, C over 61–62 and D* over 63–64 (`STRUCTURE_BREAKS`). A pattern that already fills 64 bars keeps its own breaks. With the seeded second pass every form is a 128-bar cycle. `*` in a segment (`A*1`, `A'*1`) plays the lane on another loaded option or drum kit for that segment, seeded per lane, letter and written segment. The renderer renders it with that kit's generator, the packet carries it (`LaneBar::swap`) and the RT lane switches on the bar and returns to the player's option after (`Lane::set_form_option`). New built-in form `A1 A*1 A1 B*1 A'1 A'*1 A1 C*1`; the long form ends on `C*2`. The readout marks the written segment playing (none in an inserted break). Mudpie playheads: each letter seed spreads 2–4 anchors over the whole file (one per equal part, at a seeded place in it, within the first 90 %), replacing the region picked by the sound seed.
- 2026-10-15: Beat-resolution forms, composed built-ins (user: no loop-like patterns, `*` and `'` placed with intent, short swaps). Segment lengths are bars in quarters down to one beat (`C*.5`, `D*.25`); forms work in beats (`Segment::beats`, `Form::pos_at`). A bar touching several segments renders each part as a whole bar with its letter, settings, prime, feel and kit, and splices it over its ticks (`Form::parts`, `render_part`, `splice_range`). A swap inside a bar carries its tick range (`LaneBar::swap`); the RT lane switches option at that tick and back (`Lane::set_form_swap`, applied in `begin_segment`, so to the chunk). The ten built-in forms are written out at 64 bars, one 16-bar phrase each: steady, roller, B-led, two-step build, EDM build with a 16-bar breakdown, breakouts, call and answer, jungle chops (one-beat swaps), and two sound-per-bar forms. The readout shows a window of 10 segments around the playing one.
- 2026-10-15: Mix, chords, modes, bass block speed (user). Bass filter brighter and more varied (seed base 0.35–0.8, motif ±0.35). Snare −2 dB against its kick (`SampleSpec::gain_db`, written by the builder), hats −8, perc −10, break −2 dB under the drum target. Stacks build chords on the played note from a seeded family per chord of the progression (5th/3rd/7th/9th/11th, sus2, sus4, stacked 4ths, open add9; no octave doublings), for the stack knob and the follow lead. Minor chords (i, iv, v) now carry most bars of a minor key. A global harmony seed picks the mode in the key's family (minor: aeolian .5, dorian .2, phrygian .15, harmonic .15; major: ionian .5, mixolydian .3, lydian .2), seed 0 = plain minor/major; it lands on the next phrase. F1 Type flips minor/major (same root, next phrase), Shift + Type rerolls the harmony seed; keyboard `p` / `Shift+p`. The header shows the mode. Bass blocks take a seeded speed step per letter seed (main: centre .65, faster .25, slower .1; filler: .35/.45/.2) on top of which the half pad and base rate apply.
- 2026-10-15: Drum voice buses and mix (user). The sampler routes each target to an output by a map (`Sampler::set_bus_map`, contiguous bus blocks in `process_split`); the drum lane gets a bus per voice that has a chain in the new style field `voice_fx` (kick, snare, hats, ride, shaker, perc, top) plus the break bus, each processed before the lane FX. Starter: kick dbx-160 style compressor (4:1, 5/60 ms, soft knee) and +3 dB low shelf (150 Hz corner), −2 dB at 350 Hz; snare high pass 80 Hz and a 76-style compressor (2 ms attack, 250 ms release, +3 dB makeup); hats/ride/shaker/perc/top a per-type high pass (350/300/400/120/200 Hz), a small shaping EQ, 3:1 at 15/50 ms and a half-wet 3 dB drive. Drum bus compressors −7 / −14 dB thresholds (peak GR 5.0 / 3.9 dB on a 32-bar render), master −11 dB (5.7 dB). Bass multiband: lows down (band 1 3:1 at −20 dB, −1.5 dB; band 2 −1 dB), mids up (band 3 −1 dB instead of −3), 2–10 kHz +3 dB, top +2.5; bass output −1 dB (−12).
- 2026-10-15: Drum tone, quarter bass centre, riff lead (user). Kick shelf +6 dB and 35 ms compressor attack, a half-wet 4 dB drive after the kick and snare compressors; other voices 35 ms attack and 5 dB drive; drum bus compressors 35 ms attack, saturation 0.35; an API 550B style EQ last on the drum bus (+2 dB at 100 Hz, +1.5 dB at 500 Hz, +1 dB at 10 kHz); a 2.5 dB drive at 40 % before the master limiter (`master_fx`). New base rate `quarter` (`RateSpec::exponent`); the bass centre is quarter speed. The follow lead plays seeded riffs (research: DnB lead recipes): a rhythm cell (10, weighted by cx) and a chord-tone template (10, weighted by spread; passing tones only on odd steps, stepping into the next tone) per letter seed, 1- or 2-bar riffs (bar 2 answers, ending on the 5th) repeated exactly, a varied last phrase bar (16th run, octave lift, end on 5th/b7, +2 sequence), the cell joined by the bass onsets at cx 0 and kept out of them at cx 1, density thinning weak steps and above 0.45 adding repeats of riff notes (about 4 to 10 hits per bar). Chords on degree 5+ voice the riff an octave down. No per-note jitter in the lead.
- 2026-10-15: Recording from the start (user). `live.auto_record` (default on) starts the set recording when live mode runs; Ctrl+R still stops and restarts it. Next to the first recording file a `<name>.state.tsv` logs one row per bar: time, bar, tempo, key and mode, progression, and per lane on/off, sound, seed, form, letter and written segment, octave, rate and control values, so a moment in the recording maps to settings that render again offline.
- 2026-10-15: Drum tone, quarter bass centre, riff lead (user). Kick shelf +6 dB and 35 ms compressor attack, a half-wet 4 dB drive after the kick and snare compressors; other voices 35 ms attack and 5 dB drive; drum bus compressors 35 ms attack, saturation 0.35; an API 550B style EQ last on the drum bus (+2 dB at 100 Hz, +1.5 dB at 500 Hz, +1 dB at 10 kHz); a 2.5 dB drive at 40 % before the master limiter (`master_fx`). New base rate `quarter` (`RateSpec::exponent`); the bass centre is quarter speed. The follow lead plays seeded riffs (research: DnB lead recipes): a rhythm cell (10, weighted by cx) and a chord-tone template (10, weighted by spread; passing tones only on odd steps, stepping into the next tone) per letter seed, 1- or 2-bar riffs (bar 2 answers, ending on the 5th) repeated exactly, a varied last phrase bar (16th run, octave lift, end on 5th/b7, +2 sequence), the cell joined by the bass onsets at cx 0 and kept out of them at cx 1, density thinning weak steps and above 0.45 adding repeats of riff notes (about 4 to 10 hits per bar). Chords on degree 5+ voice the riff an octave down. No per-note jitter in the lead.
- 2026-10-15: Motif clips, call and response, swap partners (user: bass and lead sounded like random hits). Bass and lead play curated MIDI clips (`motifs/bass/*.mid`, `motifs/lead/*.mid`, or a lane's `motifs` list; `crate::live::motif`). The loader parses them (`midly`), detects each clip's key and mode (Krumhansl-Schmuckler on note lengths, minor when no third) and stores scale degrees plus chromatic offsets, so key, mode and chord re-pitch them. The letter seed picks a clip; rhythm and pitches play as written in every bar (no per-bar randomness; per-note jitter only moves accent, slide and the bass filter). Knobs: density thins weak-beat notes below 0.5 and adds capped repeats above, and scales note length (sparse = held); spread lifts weak bass notes an octave or doubles lead notes; variation gives bar 4 one seeded change (octave lift, end on the 5th, lead 16th run); the chord root stays nearest the key's. The lead picks a clip that interlocks with the bass clip's onsets (unison at low cx, off the bass at high cx), plays a 1-bar clip as call in bars 1 and 3 and answer in bars 2 and 4 (together in the bar from cx 0.5), drops notes on the bass's steps from cx 0.3 and rests on drum fills. Without clips, or with `motifs: 0`, the old generators run. The starter builder writes ten bass and ten lead clips (`motif::starter`, Am, Dm, Em, Fm and C major) as `starter-*.mid`. Form swaps (`*`) now play one fixed partner per lane, picked by the master seed among the other loaded options of the same kind (sampler, CLAP synth, stream sampler; any kit on the drums), `RenderLane::families`.
- 2026-10-15: Mix rework (user, analysis of a recording). Kick and hats: a fast compressor tames the peaks, then a slow one glues lightly; snare a gentle 76-style compressor; ride, shaker, perc and top keep their dynamics (high pass only); no per-voice drives. Drum bus: 3:1 at 3 ms / 175 ms, then 2:1 glue at 30 ms, saturation 0.12, parallel compression (8:1, a quarter wet), API-style EQ at 100 and 250 Hz, no top lift; Pop2 removed. Bass lane: a fast compressor before its multiband, lighter drive, high bands halved. Glue lane compressors 2:1 at 30 ms, lane saturation 1.5 dB at 30 % (bass 2 dB), master saturation 1 dB at 25 %, master compressor 2:1 with a 150 ms release. A native soft-knee `clipper` (4x oversampled, transparent below its knee) shaves the highest peaks before the drum bus compressors (+3 dBFS) and before the master limiter (0 dBFS); the live limiter holds sample peaks at −1.5 dBFS. Short hits are measured over their own length for the per-sample LUFS gain (`measure_hit_lufs`, boost capped at +12 dB). The sampler steals voices with a 5 ms fade from spare slots. On a 48-bar render the limiter now does 1.9 dB (was 7.1).
- 2026-10-15: Style pipeline in steps (user: regeneration took about 7.5 minutes). `scripts/live-starter-style.sh build|analyse|calibrate|all OUT_DIR [--lanes A,B]`: build writes the style from the library, analyse measures each sample once and stores it (`SampleSpec::lufs`, `style-calibrate --analyse`, the loader measures only samples without it), calibrate sets the input and output trims, optionally per lane. Output calibration measures each option from silence (`Lane::reset_sound`), so the trim no longer depends on the option measured before (a few drum kits moved by up to 2.2 dB), and runs lanes with 16 or more options (the 274 drum kits) on up to 8 worker threads: calibrate 434 s → 95 s. `scripts/live-analyse.sh` runs the analysis scripts (scripts/live-analysis/) in one cached numpy/scipy environment.
- 2026-10-16: Band duck on the bass lane (user: the synth lanes should carve the bass band by band, like a multiband sidechain, as the bass lane's last stage). The glue splits the bass and the key (lead + extra after their glue) into six bands (80, 400, 2000, 5000, 8000 Hz; LR24 with allpass compensation, flat with no key) and cuts each bass band where the synths are loud in it, gently (3 dB in the sub band, 4 dB elsewhere at most, 4:1 from −40 dBFS). Timing adapts per band: attack 20 ms in the sub band down to 1.5 ms in the air band, release 250 ms down to 60 ms, after multiband practice (slow low bands never modulate within a cycle). Program-dependent release: while a band's key stays over the threshold the release stretches up to 3x (a held synth note lets the bass back gently, a stab quickly). Style `glue.band_duck`, `glue/bandduck.rs`.
- 2026-10-16: Direction change for the synth lanes: user sound packs instead of generated riffs (`docs/plans/2026-10-16-live-sound-packs.md`). After eight listening rounds of generated riffs and designed Surge leads did not reach quality ("ai currently has not the understanding of what makes music great at all"), the user supplies the sounds (SFZ instruments over rendered samples and mudpies) and the material (MIDI clips up to 64 bars with lengths, velocity, pitch bend, CC curves and glide) from any DAW; the engine remixes them with 4 knobs (Density, Length, Expression, Tone), forms place the phrases and rests. Also on 2026-10-16: lead/extra synth forms after `|` with rests (`-8`) and break-only segments (`~A8`), sends raised in breaks and rests, the bass lane's band duck keyed by the synth lanes, `motif_exact` and `motif_chords` lead options, default key F minor.
