# Live Mode — Research

**Date:** 2026-10-13
**Status:** Research complete, design grilling in progress
**Branch:** `live-performance-mode`
**Companion:** design document (to follow after grilling)

This document records the research behind a new **live mode** for Mesh: a deck that
generates breakbeat-focused music live from parameters (drum layers + 3 instrument
layers, one global key) instead of playing tracks. It covers the fit with the
existing codebase, the live-coding and hardware landscape, the build-vs-adopt
question for the pattern engine, and the visual/output/plugin constraints on the
Orange Pi target.

Labels: **[F]** verified fact (source or file reference), **[I]** inference.

---

## 1. Fit with the Mesh codebase

### 1.1 What exists

| Area | State | Reference |
|---|---|---|
| Engine | 4 decks × 4 stems as fixed arrays, 48 kHz, 256-sample buffers, rtrb SPSC commands, atomics back, basedrop for RT-safe frees | `crates/mesh-core/src/types.rs:10-21`, `engine/engine.rs` |
| Deck signal path | stems → linked-stem swap → slicer → per-stem multiband FX → latency comp → sum → **one time-stretcher per deck** → mixer | `engine/engine.rs:1401-1445` |
| Slicer | 1/4/8/16-bar window cut into 16 slices, 16-step queue, 2 layers per step, 8 collection-wide presets in `slicer-presets.yaml`, beat-quantized enable, no crossfade between slots | `engine/slicer.rs` |
| Effects | per-stem multiband (8 bands × 8 FX), native filter/delay/reverb/gain, CLAP via clack-host, Pure Data via libpd, 4 macros per deck | `docs/effects.md` |
| Key | Essentia key detection, Camelot math, `are_compatible`, `semitones_to_match`; follower decks transposed toward the **master deck** | `crates/mesh-core/src/music/mod.rs` |
| Controllers | mesh-midi (MIDI + HID), NI Kontrol F1 HID profile: 4 knobs, 4×4 RGB pads, 4 faders, push encoder, 7-segment display; two F1s distinguished by serial | `docs/midi-controllers.md` |
| USB export | Phase 0 copies presets + `slicer-presets.yaml`, then FLACs + `mesh.db` | `mesh-core/src/export/service.rs` |

### 1.2 What does not exist [F]

- No synthesis: no oscillators, no sampler, no note-to-pitch handling. MIDI notes are only control addresses.
- No sequencer or pattern code beyond the slicer.
- No embedded scripting language (no rhai/mlua/wasmtime/boa in any crate).
- No global key. Key handling is relative to the master deck.
- **The CLAP host is effects-only:** fixed stereo in/out, input events carry only `ParamValueEvent`, extensions limited to `params, gui, latency`. Instrument hosting needs note events, the `note-ports` and `audio-ports` extensions, an instrument path with no audio input, and transport info (`crates/mesh-core/src/effect/mod.rs:156`, `clap/plugin.rs:333-334, 1167-1191`, `Cargo.toml:45`).
- Pd patches receive only `paramN` floats and `bypass`. They get no clock or notes.
- MIDI mappings are not exported to USB.

### 1.3 Collision points [I]

1. The fixed 4×4 arrays favour a live deck that exposes exactly 4 groups as its "stems".
2. One stretcher per deck after the sum rules out per-layer pitch through the existing path. Live layers need their own pitch handling: resample-repitch for short samples, extra stretcher instances for long phrases.
3. CPU headroom on the board is limited: 4 decks use about 40-60% CPU. The GPU is near its limit with 8 waveform views (`documents/opi5-performance-analysis.md`).

### 1.4 Embedded platform facts that differ from the docs [F]

- The device has one 2880×864@120 Hz internal screen on `HDMI-A-1` (`nix/embedded/kiosk.nix:37`).

- The kiosk runs **GL/Panfrost with Fifo present mode**, not Vulkan/PanVK, because PanVK crashed on the 2880×864 mode (`nix/embedded/kiosk.nix:110-115`). The 120 Hz plan still describes Vulkan + Mailbox.
- The flake builds the **`orangepi5`** board module and DTB (`flake.nix:61`, `nix/embedded/hardware.nix:13`), while `docs/embedded.md` names the Orange Pi 5 **Pro**. The two boards differ in display outputs (§5.3).
- Cage runs with `-m last`, which drives a single output.

---

## 2. Live-coding prior art

### 2.1 Strudel / TidalCycles model [F]

- A pattern is a pure function from a time span (in cycles, rational) to events. Transformations wrap the query function. ([Strudel technical manual](https://strudel.cc/technical-manual/patterns/))
- Mini-notation: rest `~`, subdivide `[ ]`, alternate `< >`, speed `* /`, replicate `!`, elongate `@`, random drop `?`, random choice `|`, stack `,`, Euclid `(k,n,r)`, polymeter `{ }`. ([Tidal mini-notation](https://tidalcycles.org/docs/reference/mini_notation/))
- Sample cutting:
  - `chop` cuts each sample into equal parts.
  - `striate` interleaves those parts across events.
  - `slice` and `splice` pick parts by index; `splice` also repitches each part to fit its step.
  - `loopAt` and `fit` set playback speed so the sample matches the cycle or event length.
  - `scrub` plays from a position.

  ([Strudel samples](https://strudel.cc/learn/samples/))
- Tonal: `n(...).scale("C:minor")`, `scaleTranspose`, `chord(...).voicing()`, `rootNotes` (tonal.js). ([Strudel tonal](https://strudel.cc/learn/tonal/))
- Live input: `midin` → `cc(n)`, gamepad. ([Strudel I/O](https://strudel.cc/learn/input-output/))

**[I]** Storing melodic content as scale degrees and resolving them through one key at render time makes a global key change free: every layer re-voices on the next bar with no pattern edits.

### 2.2 Switch Angel break chopping [F]

Source: vault note *Breakbeat Choppage — switchangel* (frame-by-frame transcription of
[Chopping Breaks & Samples in STRUDEL Pt. 1](https://www.youtube.com/watch?v=dcmwqqzJubA)),
implemented in `~/Projects/tidal/lib/breaks.hs`.

- Core move: quantized random chops (`irand(16).seg(8)`), frozen by seed and window (`rib(seed, len)`). Randomness becomes a repeatable riff.
- Seeds are auditioned live and kept by ear. The saved seed arrays are the composition.
- Other moves:
  - rare stutter (`almostNever(ply("2|4"))`)
  - patterned break switching
  - 5+3 phrasing shared by the chops and the bass
  - one shared distortion on breaks and bass, to glue them together
  - white-noise hats
  - a buildup made by shrinking a ribbon over the whole mix
  - a master LPF on a squared curve
- Chop rate 8 per cycle is the classic feel. 16 is for fills.
- Sample pack: [switchangel/breaks](https://github.com/switchangel/breaks) (Unlicense).

### 2.3 Earlier Tidal work by the user [F]

`~/Projects/tidal` holds a generative arrangement framework (PRD accepted 2026-07-03):
- an instrument registry
- seam devices (Fill, Riser, Void, …)
- breakpoint automation lanes
- a contour-algebra motion engine (`lib/motion.hs`, 671 lines) that is deterministic and does no search

The engine worked mechanically but was **rejected by ear on sound design** ("synths bland/clownish, sub unimpactful"), not on pattern logic (PRD §5b). DnB layering rules are in the vault note *DnB Bass & Melody Design*:
- the sub locks to the chord root
- the reese plays the roots an octave up through a moving filter
- the lead is a sparse pentatonic answer

**[I]** Sound quality is the highest-risk area of the live mode, not pattern generation.

---

## 3. Hardware performance prior art

| System | Mechanism worth reusing [F] | Source |
|---|---|---|
| Elektron | Trig conditions (`%`, `A:B`, FILL, 1ST, PRE, NEI), parameter locks, Octatrack scenes interpolated on the crossfader | [CDM](https://cdm.link/new-elektron-octatrack-os-trig-conditions-awesome/), [Octatrack](https://elektron.se/explore/octatrack-mkii) |
| Torso T-1 | Per-track steps / pulses (Euclid) / rotate / repeats / accent / random velocity / scale-quantized voicing and range. No screen: the pulses light up and move as the knob turns | [Torso](https://torsoelectronics.com/pages/t-1), [MusicRadar](https://musicradar.com/reviews/torso-electronics-t-1-algorithmic-sequencer) |
| Mutable Grids | 5×5 map of drum patterns taken from real loops. X/Y blends the 4 neighbouring patterns; density, chaos and accent thresholds shape the result | `pichenettes/eurorack grids/pattern_generator.cc` (**GPL-3.0**) |
| Mutable Marbles | "Déjà vu": a 16-value loop buffer. Knob 0-0.5 sets the chance of fresh randomness, 0.5 is fully locked, above 0.5 the loop is jumped around randomly. Weighted scale quantizer | `marbles/random/random_sequence.h` (MIT) |
| Polyend Play | Per-step Chance and Action. Fill by density, Euclid, or drum role. Randomize targets | [Sound On Sound](https://www.soundonsound.com/reviews/polyend-play?amp) |
| OP-Z | Step components: ratchet, random-in-scale, ramps, jump | [TE guide](https://teenage.engineering/guides/op-z/step-components) |
| Ableton | Quantized launch, follow actions with chance, legato clip switching | [Live manual](https://www.ableton.com/en/manual/launching-clips/) |
| Novation Circuit | One scale and key shared by all tracks | [Elektronauts](https://www.elektronauts.com/t/a-circuit-tracks-change-between-two-scales-for-more-notes-access-with-2-patterns/219224) |

**[I]** These devices do not expose pattern code on the hardware. Code or data is authored elsewhere, and the device exposes a small set of continuous controls (density, variation/seed, morph position) plus quantized discrete launches.

**Criticism [F]:** generative sequencers are seldom designed for real-time improvisation ([NIME 2026](https://nime.org/web_archive/2026/proceedings/604.html)), and audiences can hear generative output as random ([Igloo Mag](https://igloomag.com/features/synthetic-ecology-how-the-how-affected-the-what)). Freezing seeds is the established answer.

---

## 4. Pattern engine: build vs adopt

### 4.1 Rust landscape [F]

| Project | License | Maturity | RT-relevant facts |
|---|---|---|---|
| [renoise/pattrns](https://github.com/renoise/pattrns) | AGPL-3.0 | Most mature. Ships in Renoise 3.5. v0.9.3, 2 contributors | Rhythm → Gate → Emitter model. Lua via mlua. Mini-notation only, not Tidal's transform library. `Rc<RefCell>`, so not `Send`. Event output allocates `Vec`s. Example player uses 250 ms lookahead on its own thread. Xoshiro256++ RNG seeded per cycle |
| [tzfm/rustel](https://github.com/tzfm/rustel) | AGPL-3.0+ | Created 2026-09, 1 contributor | Full Strudel port. Full scores need QuickJS. Queries on a worker thread with a host-set horizon. `Vec<Hap>` |
| [nukleas/cycletron](https://github.com/nukleas/cycletron) (strudel-rs) | AGPL-3.0 | Public alpha | Documented Strudel subset. Crates on Codeberg (`strudel-dsl`, `strudel-mini`, `strudel-markov`) |
| [femtomc/crumble](https://github.com/femtomc/crumble) | **No license** | One commit burst | Cannot legally be reused |
| cycles (mitchmindtree) | GPL-3.0 | Inactive since 2025-03 | The only iterator-based design, so allocation-free queries are possible in principle |
| [glicol](https://github.com/chaosprint/glicol) | MIT | Last push 2025-04 | Sequencer runs inside the DSP graph, sample-accurate, but its sequencing features are thin |
| megra.rs | GPL-3.0+ | Active | Markov-chain live-coding language |

No engine evaluates patterns on the audio thread without allocating. **[I]** Mesh's real-time design is the same whatever we choose: pattern logic runs off the JACK thread with lookahead, and fixed-size event buffers cross over through the existing rtrb + basedrop path.

### 4.2 Authoring language options [F]

script-bench-rs (Apple M5 Max, Oct 2026, ms; lower is faster):

| Engine | ms |
|---|---|
| wasmtime | 2.7 |
| wasmi | 7.0 |
| mlua-Luau | 11.8 |
| roto | 12.0 |
| mlua-Lua 5.5 | 15.5 |
| rquickjs | 36.4 |
| Koto | 49.3 |
| boa | 80.0 |
| Rhai | 112.0 |

No aarch64 numbers exist. ([script-bench-rs](https://github.com/khvzak/script-bench-rs))

- Lua is the most-used option in music tools: pattrns/Renoise and norns.
- Rhai offers sandboxing and a no-panic guarantee.
- wasmtime on aarch64 Linux is Tier 2.

**[I]** A generator runs about once per bar per layer. A 16th note at 174 BPM lasts about 86 ms, so any of these interpreters fits the budget on an A55 core. Sandboxing and authoring ergonomics separate the options, not speed.

### 4.3 Algorithms ready to port [F]

- **Break chops:** `breaks.hs` (`rndChops`, `pickN`, `stutt`, `fig53`, `twoStep`). Each maps directly to a 16-step function of (seed, window).
- **Drum density maps:** Grids (GPL-3.0, compatible with Mesh's AGPL-3.0).
- **Locked randomness:** Marbles déjà vu, plus a weighted quantizer (MIT).
- **Conditions:** Elektron trig conditions.
- **Euclid:** Bjorklund's algorithm ([Toussaint 2005](https://cgm.cs.mcgill.ca/~godfried/publications/banff.pdf)).
- **Derived voices:** `motion.hs` contour algebra (carrier → contrariness → register fold → scale snap → consonance nudge), O(n) per bar.

### 4.4 Options

- **O1 Adopt a Strudel port (rustel / strudel-rs).**
  - Gains: the text language and Strudel's transform library.
  - Costs: the projects are weeks old with one contributor each, need a JS runtime, allocate on every query, and include a synth Mesh would not use. Pad and knob performance would use little of the text language.
- **O2 Adopt pattrns as a library.**
  - Gains: it is mature, has Lua authoring and seeded RNG.
  - Costs: it is not `Send`, it allocates, and its pulse/gate/emitter model needs adapting to "a 16-step bar from a seed".
- **O3 Build.**
  - Port: Grids, Marbles déjà vu, `breaks.hs`, `motion.hs`, Euclid, trig conditions, and optionally Tidal's time-hash RNG so seeds auditioned in Tidal reproduce in Mesh.
  - Write: step and bar types, the bar renderer, the RT step player, the CLAP note path, and the style-file schema.
- **O4 Hybrid on top of O3.** Optionally add the pattrns mini-notation grammar for typed fixed patterns, and/or a Lua layer for user-written generators that runs on the bar-ahead thread.

**[I] Assessment:** the live mode is performed with pads, knobs and seeds, not text. A generator is a pure function `f(seed, bar, step, params, key) → [Step; 16]`. The parts that make the music are small, deterministic, and already known (§4.3). Strudel's limited sound engine does not affect the choice, because Mesh supplies its own sampler and CLAP instruments in every option. O3 is the lowest-risk path, with O4 extensions kept open.

### 4.5 Real-time architecture [I]

1. A control thread on an A55 core renders each layer's next bar into a fixed `Copy` array `[Step; 16]` (trigger, slice or degree, velocity, micro-offset, length, condition), at least one bar ahead. It pushes the result through rtrb.
2. The JACK thread walks a step cursor from the deck's beat clock and fires sampler voices and CLAP note events at sample offsets.
3. Structural changes are quantized to a bar or step boundary by re-rendering and swapping: seed, chain edits, kit swap, key.
4. Continuous parameters apply immediately at block rate: velocity scale, filter, mutes, macro depth.
5. Rendering keeps no state (f(seed, bar, params)). That is what makes seeds freeze, repeat, and re-render after a key change.

Scheduling references:
- Strudel: 100 ms latency ([cyclist.mjs](https://codeberg.org/uzu/strudel/raw/branch/main/packages/core/cyclist.mjs)).
- Web audio pattern: 25 ms timer and 100 ms lookahead ([web.dev](https://web.dev/articles/audio-scheduling)).
- pattrns: 250 ms.

---

## 5. Visuals, display outputs, plugins

### 5.1 Live-coding visual vocabulary [F]

- **Strudel highlighting.** Every event carries the source ranges of the tokens that produced it. On each frame, the active ranges get a 2 px outline in the pattern's colour ([highlight.mjs](https://codeberg.org/uzu/strudel/raw/branch/main/packages/codemirror/highlight.mjs)).
- **Strudel inline widgets.**
  - 64 px sliders placed at their text position ([slider.mjs](https://codeberg.org/uzu/strudel/raw/branch/main/packages/codemirror/slider.mjs)).
  - `_scope`, `_pianoroll`, `_punchcard` (post-transform) and `_spiral`, plus `_pitchwheel` and spectrum.
  - All are redrawn each frame. The pianoroll puts the playhead at the centre and uses two flat colours, with gain mapped to alpha ([visual feedback](https://strudel.cc/learn/visual-feedback/)).
- **Feedforward (Tidal).** ncurses on a Pi Zero, with active-step highlights and a VU meter per pattern ([NIME 2020](https://nime.org/proc_music/nime2020_music_mclean/)).
- **Gibber.** Rewrites the source live with annotations showing each sequence's phase and latest output ([NIME 2015](https://nime.org/proc/nime2015_croberts/)).
- **Audience research.** Flashing the instruction that triggered a sound makes the code-to-sound link clearer than detailed execution views ([McLean et al. 2010](https://slab.org/2010/05/29/visualisation-of-live-code/)). The audience does not need to read the code, only see that it is live ([TOPLAP manifesto](https://tidalcycles.org/docs/around_tidal/toplap_manifesto)).
- **Hardware.**
  - Torso T-1 pulses move across the steps as the knob turns.
  - Elektron users ask for conditional and probability steps to be visually distinct ([elektronauts](https://www.elektronauts.com/t/trig-conditions-locked-on-digitakt-2/224126)).

**[I]** Since Mesh owns the generator, every step can carry its source span the way Strudel events do. That gives exact highlight sync without parsing.

The highest-value elements are also the cheapest to draw:
- an outline on the active token
- one colour per layer shared by code, scope and roll
- inline value readouts
- Euclid and step rings
- probability shown as fill, with the outcome flashed when the step fires

### 5.2 Rendering on Mali-G610 [F]

- The waveform shader is limited by ALU throughput. Eight views put Mali at about 76-105% load. Edge anti-aliasing is the most expensive per-pixel step (`documents/opi5-performance-analysis.md:38, 228`).
- Mali is tile-based. Clear attachments at the start of each pass, batch or instance draws, and keep varying data small ([Arm TBR](https://developer.arm.com/community/arm-community-blogs/b/mobile-graphics-and-gaming-blog/posts/the-mali-gpu-an-abstract-machine-part-2---tile-based-rendering)).
- iced_wgpu 0.14 renders text through cryoglyph and cosmic-text. A cached text group is re-prepared only when its version changes (`iced_wgpu-0.14.0/src/text.rs:139-171`).

**[I]** Untested techniques:
- Put static code text in a long-lived cache and live values in small separate caches.
- Use a monospace font with fixed-width digits so layout never shifts.
- Draw highlights as quads.
- Draw scopes as one custom shader primitive over a 256-512 point ring buffer.
- Run visuals at 30 Hz, decoupled from audio.

All of these need measuring on the board before we commit to them.

### 5.3 Display and stream outputs [F]

- **Orange Pi 5 Pro:**
  - HDMI 2.1 (8Kp60)
  - a second HDMI 2.0 through an LT8711UXD DP-to-HDMI bridge, with mainline support still pending ([CNX](https://www.cnx-software.com/2024/04/12/orange-pi-5-pro-low-cost-rockchip-rk3588s-sbc-16gb-lpddr5-ram-dual-hdmi/), [linux-rockchip 2026-04](https://lists.infradead.org/pipermail/linux-rockchip/2026-April/070325.html))
  - MIPI DSI
  - USB-C carries power only.
- **Orange Pi 5 (non-Pro):** HDMI plus USB-C DisplayPort alt mode.
- **Cage:** `extend` stretches one app across all outputs, `last` uses only the last one. Cage exposes screencopy and export-dmabuf, so its output can be captured ([cage(1)](https://man.archlinux.org/man/cage.1)).
- **RK3588 encoding:**
  - The hardware encoder does H.264/HEVC up to 1080p@480 or 4K@120 ([ffmpeg-rockchip](https://github.com/nyanmisaka/ffmpeg-rockchip/wiki/Encoder)).
  - It needs the vendor kernel's MPP, because mainline has no `rkvenc` driver.
  - Mesh already builds on the vendor kernel (`flake.nix:41`).
- **NDI on ARM:** historically send-only and CPU-heavy (older sources).

**[I] Output options:**
- **O1** One window under `cage -m extend`, split into a performer half and a projector half.
- **O2** A compositor that can place two windows (sway or labwc).
- **O3** Render the projector view offscreen and send it to `appsrc ! mpph264enc ! rtmpsink` (stream instead of a second display).

None of these has been measured.

### 5.4 CLAP instruments on aarch64 [F]

| Plugin | nixpkgs aarch64-linux | Other aarch64 sources |
|---|---|---|
| Surge XT 1.3.4 | Yes, CLAP built | Upstream nightlies, ALARM |
| Dexed 1.0.1 | Yes | ALARM |
| Cardinal 26.02 | Yes | — |
| Odin 2 2.4.1 | No | ALARM 2.4.1-2 |
| Six Sines 1.2.1 | No | Upstream nightly arm64 |
| Vital 1.5.5 | No | Vitalium (2021, no CLAP) |
| ZynAddSubFX | `platforms.all` | No CLAP build found |

I found no measured CPU cost per voice on A76 cores for any of them.

---

## 6. Event and voice model

### 6.1 Strudel / superdough [F]

- A **Hap** has these fields:
  - `whole` and `part` spans
  - an open key/value `value` map
  - `context`, which holds the source locations used for highlighting

  Only events where `whole.begin == part.begin` are onsets that trigger sound. Duration is `value.duration`, or the `whole` span if unset, multiplied by `clip`/`legato` ([hap.mjs](https://codeberg.org/uzu/strudel/raw/branch/main/packages/core/hap.mjs)).
- **Controls**: about 300 untyped keys ([controls.mjs](https://codeberg.org/uzu/strudel/raw/branch/main/packages/core/controls.mjs)):
  - Source: `s`, `n`, `note`, `freq`
  - Level: `gain`, `velocity`, `postgain`
  - Time: `clip`, `duration`, `nudge`
  - Amp ADSR
  - Pitch envelope and vibrato
  - Sample playback: `begin`, `end`, `loop*`, `speed`, `cut`, `stretch`
  - Oscillators: `unison`, `spread`, wavetable, FM
  - LP/HP/BP filters, each with its own envelope and LFO
  - Dirt: `distort`, `shape`, `crush`, `coarse`
  - Space: `pan`, delay and room sends, `orbit`, duck
- **Per event vs per orbit**: filters, dirt and pan are part of each voice. Each orbit has one delay and one reverb. `delay` and `room` set on an event are only send levels. The last event to set the orbit's delay or room parameters wins.
- **Sample pitch**: `rate = |speed| * 2^((note - 36)/12)`. With no note given, the sample plays at its original pitch. A negative `speed` reverses it. If `clip`, `loop` and `release` are all unset, the voice length is the slice length.
- **Cut groups**: a new hit fades the previous voice in the same group out over 10 ms.
- **Voices**: polyphony is global, capped at 128, and the oldest voice is stolen first. There is **no legato, mono mode or glide**: `slide` is a TODO and `portamento` is commented out.

**[I]** superdough is a good reference for parameter names and sample pitch math. It is not a model for voice behaviour.

### 6.2 CLAP note path [F]

- `clap_event_note` carries `note_id`, port, channel, key and velocity (f64). The event types are NOTE_ON, NOTE_OFF, NOTE_CHOKE and NOTE_END (sent from plugin to host). Events are sample-timed ([events.h](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/events.h)).
- Note expressions: VOLUME, PAN, TUNING (±120 semitones), VIBRATO, EXPRESSION, BRIGHTNESS, PRESSURE. Values are absolute and sample-accurate.
- Per-note parameter modulation uses `PARAM_MOD` with a `note_id`. It only works on parameters the plugin flags `MODULATABLE_PER_NOTE_ID` ([params.h](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/params.h)).
- **CLAP has no glide event.** Synths glide when notes overlap in mono or legato mode, as in Surge XT Mono FP. The alternative is for the host to ramp TUNING on the held note ID.

### 6.3 Groovebox voices [F]

- **Digitakt**: tune, play mode (including reverse), start, length, loop, filter with its own envelope, amp ADSR, 3 LFOs, sends. Tracks are monophonic.
- **Octatrack Flex**: pitch, start, length, rate, retrig. Start and slice can be set per step.
- **Polyend Tracker** step FX: tune, micro-tune, glide, slide, reverse, position, slice, gate, micro-move, chance, roll, filter, overdrive, sends, velocity, pan.
- **Ableton**: 16 choke groups.

Sources: [Polyend](https://backstage.polyend.com/t/what-step-effects-are-available-in-the-polyend-tracker/262), [SOS DT2](https://www.soundonsound.com/reviews/elektron-digitakt-ii).

### 6.4 Proposed event schema [I]

A fixed-size, `Copy` event of about 24 bytes:
- tick within the bar
- kind: Trigger / NoteOn / NoteOff / Choke / Expr
- flags: reverse, tie, accent, loop
- target slot or slice
- choke group
- key, resolved from the scale degree at render time
- cents
- velocity
- start offset
- gate in ticks (0 = one-shot)
- decay override
- cutoff and pan offsets
- glide time
- CLAP `note_id`

**Per layer, updated once per audio block:**
- volume and base pan
- filter base
- envelope defaults
- drive
- send levels
- swing
- repitch vs stretch mode
- CLAP `PARAM_VALUE`

Delay and reverb are shared per deck, the same way superdough shares them per orbit.

**Tie** is played as overlapping NOTE_ON events on CLAP layers, and as a playback-rate ramp in the sampler. **Choke** is NOTE_CHOKE on CLAP layers, and a 10 ms fade in the sampler.

---

## 7. What makes generated breakbeat music sound good

### 7.1 Evidence [F]

1. **Corpus beats rules for drums.** Grids looks up per-step "levels" learned from real loops and blends the 4 nearest maps. A step fires if `level + chaos*rand > ~density`, so density reveals steps from most to least important ([manual](https://pichenettes.github.io/mutable-instruments-documentation/modules/grids/manual/), [pattern_generator.cc](https://github.com/pichenettes/eurorack/blob/master/grids/pattern_generator.cc)).
2. **Metric weighting is the cheap version of a corpus.** The Endless Acid Banger fires a note with probability `density × (0.6 on step%4, 0.5 on step%3, 0.3 on even steps, 0.1 otherwise)`. Its note pools are root-heavy (unison and octaves). About 30% of notes are accented and 10% slide ([pattern.ts](https://raw.githubusercontent.com/vitling/acid-banger/main/src/pattern.ts)).
3. **Repetition reads as human.**
   - Adding repetition raised how much listeners liked the music and how often they judged it human-made ([Margulis via NPR/WNYC](https://wnyc.org/story/play-it-again-and-again-sam/)).
   - Human drumming varies little inside a pattern and a lot at fills. Random generators vary everywhere ([arXiv 2301.01162](https://arxiv.org/pdf/2301.01162)).
4. **Syncopation has a sweet spot.** Medium syncopation maximizes groove ([Witek 2014](https://research.birmingham.ac.uk/en/publications/syncopation-body-movement-and-pleasure-in-groove-music/)). Syncopation and density predict groove. Leftover microtiming does not ([Senn 2018](https://pmc.ncbi.nlm.nih.gov/articles/PMC6025871)).
5. **Quantized timing is safe and jitter is useless.**
   - Fully quantized patterns rated highest ([Davies 2013](https://repositorio.inesctec.pt/handle/123456789/3901?mode=full)).
   - Gaussian jitter has no effect, while microtiming learned from real drummers does ([GrooVAE](https://ar5iv.labs.arxiv.org/html/1905.06118)).
   - Jungle's swing lives inside the break audio.
6. **Velocity carries the ghost and accent layer.** Ghost notes sit at about 30-40% velocity. Hats at a constant velocity sound robotic.
7. **Commercial bass generators** take rhythm from curated libraries and derive pitch from harmony with one "shape" control: Captain Deep, Scaler, Orb, RapidComposer. None of them generate free-form rhythm.
8. **Break resequencing in practice:**
   - Producers tighten kick and snare first and keep each slice's trailing sound.
   - Stepping slices **in order** and editing selected groups produced the best breaks ([Polyend 2023](https://polyend.com/whats-up/aisjam-polyend-play-breakbeat-jungle-drums-using-tracker-style-slicing/)).
   - BBCut cuts at phrase level with statistical feedback ([Collins 2001](https://www.generativeart.com/on/cic/ga2001_PDF/collins.pdf)).
9. **Sound is decisive.**
   - The earlier Tidal engine failed on timbre, not on pattern (vault PRD §5b, reese failure analysis).
   - On the amen break, "timbre is doing as much musical work as the placement" ([Hein](https://ethanhein.substack.com/p/building-the-amen-break)).
   - Jungle glue: the break sidechained to the kick, a drum bus with glue compression, and a kick-keyed dynamic-EQ cut at 50-70 Hz on the bass ([Attack 2023](https://www.attackmagazine.com/technique/beat-dissected/slicing-breakbeats-like-nia-archives/), [Music Guy Mixing](https://www.musicguymixing.com/how-to-mix-drum-and-bass/)).

### 7.2 Drum archetypes [F, partly unverified]

| Archetype | Backbone (16 steps, 1-indexed) |
|---|---|
| Two-step DnB | K 1, 11 / S 5, 13 |
| Rolling DnB | Two-step plus ghost snares, 8th ride and offbeat open hats. Swing 50-60% (one source) |
| Half-time | K 1 / S 9 (drumstep: K 1, S 9, K 15) |
| Hardcore | 4x4 kick with a break on top |
| Midtempo | Ambiguous: 4x4 with snare on 2/4, or half-time K 1 / S 3, depending on source |
| Buildup | Snare roll speeding up from quarters to 8ths, 16ths, then 32nds over 2/4/8/16 bars. One beat of silence, then the drop |

The exact amen slice positions are not verified against a primary source.

### 7.3 Design implications [I]

- **A1** Archetypes are data: per-step importance levels for each role, Grids-style. Density reveals steps in order of importance.
- **A2** The backbone is locked. The style control owns kick and snare. Overlays may only add events where the backbone level is low.
- **A3** Variation is shaped by phrase, not by bar: A A A' B, with mutation only at phrase ends.
- **A4** Slices stay mostly in order. The default is the original slice for each step. A mutation swaps in a slice of the same type. Fully random slice order is for fills only.
- **A5** Feel comes from velocity tiers per metric level, not from timing jitter. Swing defaults to 0 for DnB.
- **A6** Bass rhythm comes from a curated library per archetype or from the gaps in the drums. Bass pitch comes from a root-heavy weighted pool. Accent and slide flags drive the synth.
- **A7** Leads are sparse: a 2-4 note motif placed in the bass's rests.
- **A8** Mix glue is on by default: trims per role, a low-band duck keyed by the kick, a drum bus, a mono sub, shared drive, and a high-pass filter per role.
- **A9** Sound comes before code. Each role's timbre gets ear sign-off before generator tuning starts.

---

## 8. Open questions carried into the design

1. CPU cost on A76 cores of sampler voices plus 1-3 CLAP instruments. This needs measuring on the board.
2. Whether the target CLAP synths (Surge XT, Dexed, Odin 2, Six Sines) support per-note-ID modulation and TUNING expressions on aarch64. This needs a clack-host scan.
3. Exact amen and classic-break slice positions, to be checked against the waveforms.
4. Whether a 1D style axis can be ordered so that neighbouring archetypes sound alike. This needs an ear test.
5. Whether this is fun to play and sounds good. Research can't answer that. It gets answered by building and listening.

Resolved since the first draft:
- The board is the Orange Pi 5, not the 5 Pro. The internal 2880×864@120 screen is on HDMI-A-1.
- v1 targets only the performer's main screen.
