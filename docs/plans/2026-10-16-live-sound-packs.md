# Live mode: user sound packs instead of generated riffs

Status: approved direction (user, 2026-10-16). Supersedes the riff and lead
generation work of 2026-10-15/16 for the synth lanes.

## The shift

**Before:** the engine composed the synth lanes. Generators wrote riffs and
melodies from rules and seeds, and the synth-design skill designed Surge XT
patches for them.

**Now:** the user creates the sounds and the musical material in their own
DAW. The engine only plays and remixes that material live, fitted to the
other lanes, the same way it already treats breaks and mudpies.

## Why

- Eight listening rounds on 2026-10-16 did not reach quality.
  - The starter clips sounded like "carnival or a kids toys preset".
  - Rule-generated riffs were rated mostly "random", "bullshit", "no". Only 2 of
    16 were close.
  - Tuned Surge patches kept artefacts the user heard at once (an 8 kHz
    "laser", a washy delay sound).
- The user's conclusion: "ai currently has not the understanding of what makes
  music great at all. this is just random."
- Measurement still helps, but only to check and to remix. A model can tell
  what the user's material does (off-beat share, pickups, note lengths,
  repetition). It cannot invent material that is great.
- Technically there is no other way to carry a producer's sound to the board.
  - No open standard moves an instrument and FX chain between DAWs. DAWproject
    is unsupported in Ableton, FL and Reaper, and plugin state is
    vendor-bound.
  - Commercial synths (Serum, Phase Plant) do not run on ARM.
  - Rendered audio is the only portable carrier.

## Principle

The files define the material and the nuances a sound can have. The 4 knobs
decide how much of it plays.

- **MIDI clip** (up to 64 bars): notes, rhythm, note lengths, velocity, pitch
  bend (slides, bends, performed vibrato), controller curves (filter, macro
  movement over the phrase), glide (CC65 portamento on, CC5 portamento time,
  overlapping notes = legato).
- **SFZ instrument**: which sample plays for which notes and velocities, loops,
  round robin, amp, filter and pitch envelopes, LFOs (vibrato), controller
  mappings (macros), the start offset for mudpies.
- **The 4 knobs** remix the material, never invent it. At 0.5 on all four the
  clip plays exactly as written.

| knob | 0 | 0.5 | 1 |
|---|---|---|---|
| Density | anchor notes only (long notes, pickups, phrase starts) | as written | written plus repeats in the gaps |
| Length | stabs | written lengths | held, legato with glide |
| Expression | no bends, vibrato, glide or filter movement | as written (MIDI + SFZ) | exaggerated depths |
| Tone / Morph | SFZ macro at one end | middle | other end |

The forms still decide which phrase plays in which section and where the
lane rests (`-` rests, `~` break-only, synth forms after `|`). Key and tempo
follow the track: clips are moved to the lane's key (`motif_exact`), samples
are re-pitched by the sampler. The bass lane gives way to the pack band by
band (glue band duck).

## Pack format

```
packs/<name>/
  sound.sfz            # SFZ subset (see below), paths relative to the pack
  samples/*.wav        # the rendered sounds and mudpies
  phrases/*.mid        # Standard MIDI files, any key, up to 64 bars each
  pack.yaml            # role (lead / bass / extra), optional FX chain,
                       # knob ranges, which CC the Tone knob drives,
                       # glide default
```

SFZ subset to support first:

- headers `<control> <global> <group> <region>`;
  `sample`, `default_path`, `lokey`/`hikey`/`pitch_keycenter`,
  `lovel`/`hivel`
- `loop_mode`/`loop_start`/`loop_end`, `seq_length`/`seq_position`
  (round robin), `trigger=release`
- `ampeg_*`, `fileg_*` with `cutoff`/`resonance`/`fil_type`, `pitcheg_*`
- `lfoNN_*` (pitch, cutoff, amplitude), `*_onccN` (macros),
  `label_ccN` (macro names)
- `offset`, `offset_onccN` (mudpie start position)

Opcodes outside the subset are ignored and listed as load warnings.

## User workflow

1. **Sound design** (any DAW, any plugins, on the user's PC): build the sound
   and resample it, either as several notes across the range or as one long
   processed take (a mudpie).
2. **Instrument** (Linux or any OS): map the WAVs in an SFZ file with
   Polyphone (open source, on Linux; imports WAVs, sets key zones, loops and
   envelopes, exports SFZ), ConvertWithMoss (free, Java, any OS), or by hand
   (a few lines of text).
3. **Phrases**: load the SFZ in sfizz (open source; VST3, AU and LV2 on Windows,
   macOS and Linux, standalone too) or Sforzando (free, Windows and macOS),
   write riffs against it, export each as a `.mid`.
4. **Pack**: put the folder on the USB stick.
5. **Play**: the pack is a lane option. Forms place the phrases, the knobs
   remix them.

The user's split: sound design sessions on the PC (any DAW, any plugins) produce
raw WAVs and mudpies with enough variation to build instruments from. On the
Linux laptop: Polyphone maps the WAVs to an SFZ, a MIDI editor that hosts sfizz
(Reaper, Qtractor, Ardour or Bitwig) writes the phrases, and the folder loads
straight into Mesh. The PC with a full DAW can do the same steps.

## Engine work

1. SFZ subset loader into the native sampler (Rust, no sfizz: its library
   repository was archived in 2026-06, it is C++, and its CPU cost on the
   board is unmeasured).
2. MIDI clips up to 64 bars (today 4), with pitch bend, CC curves (CC1, CC74,
   the Tone CC, CC65/CC5 glide) played as per-note and per-lane modulation.
3. The 4 knobs as in the table. Density thins by anchor importance (long notes,
   pickups, phrase starts), not by metric weight: thinning by metric weight
   removes the anticipations that make riffs work (`music-theory.md`
   section 3 and 8).
4. Pack discovery on the stick, packs as lane options, phrase choice per form
   letter.

No pack editor in mesh-cue (user, 2026-10-16): packs are made with existing
tools, SFZ and MIDI are the editing formats.

## What stays, what is retired

| kept | retired or demoted |
|---|---|
| motif clips with key mapping, `motif_exact`, `motif_chords` | rule-based riff generation (`riff_gen.py`) as a source of material |
| | a pack editor in mesh-cue (dropped) |
| forms with rests, break-only and synth forms, break sends | starter riff clips as defaults |
| glue band duck, lane FX, Surge XT hosting | the synth-design skill as the way to make sounds (it becomes a checker for packs and stays available for open synths) |
| analysis tools (`riff_stats.py`, score, references) to check packs | AI transcription of reference leads as material |

## Open questions

- How the user's DAW exports pitch bend and CC curves into `.mid`
  (Ableton, Bitwig): needs a test clip and a checker.
- Voice and memory limits of large SFZ libraries on the ARM board (cap voices,
  stream mudpies from disk).
- Where the Tone knob's CC comes from when a pack defines none.

## Implementation plan (2026-10-16)

From a survey of the engine (sampler, loader, motif clips, generators, render):
the sampler has key zones but no velocity zones, round robin, loops, vibrato or
pitch bend; clips are capped at 4 bars and only notes are parsed; there is no
modulation event; per-option generators exist (`RenderLane::kits`) but only
drums use them; sampler gates may run past the bar line.

| stage | what | where |
|---|---|---|
| S1 | SFZ subset parser: headers with inheritance, `default_path`, `#define`; zones (`lokey hikey key pitch_keycenter lovel hivel`), `seq_length seq_position`, `loop_mode loop_start loop_end`, `offset`, `tune transpose volume pan`, `ampeg_*`, `cutoff resonance fil_type`, `pitchlfo_depth pitchlfo_freq` and `lfoNN_freq lfoNN_pitch` (vibrato), `*_onccN` for cutoff, volume, pitch-lfo depth and offset, `label_ccN`; unknown opcodes listed | `live/sfz.rs` |
| S2 | Sampler: velocity zones, round robin, loop points, per-slot tune and start offset; per-option base params (envelope, filter, vibrato, CC map) applied when the option becomes active | `sampler/*`, `lane.rs` |
| S3 | Modulation: event kinds `Bend` (cents, lane-wide, smoothed) and `Cc` (value per CC number); sampler vibrato LFO, CC state driving cutoff, volume and vibrato depth through the SFZ map; lane params `Tone` (the pack's tone CC) and `Expression` (scales bends, vibrato, CC movement and glide) as instrument controls | `event.rs`, `sampler/*`, `command.rs` |
| S4 | Phrases: MIDI parse with pitch bend and CC; clips up to 64 bars with absolute pitch, tonic detection, bends, CC curves (downsampled to a 64th grid) | `motif/midi.rs`, `live/phrase.rs` |
| S5 | `phrase` generator: plays the bar of the clip at the global bar (clip bar = bar mod clip length), moved to the lane's key by the shortest transposition; Density thins by note importance (long notes, pickups, first after a rest) or adds repeats; Length scales gates (stab .. written .. legato with glide ties); emits bends and CCs; letter seed picks the phrase | `generator/phrase.rs`, registry |
| S6 | Packs: `pack.yaml`, `sound.sfz`, `phrases/*.mid`; lane option `pack: DIR` and lane `pack_dir: DIR` (every sub-folder with a `sound.sfz` becomes an option); per-option phrase generator through `RenderLane::kits`; option family `Pack` for form swaps | `live/pack.rs`, `style.rs`, `loader.rs` |
| S7 | Starter style: lead and extra lanes read `packs/lead` and `packs/extra` with the phrase generator and the 4 controls (Density, Length, Expression, Tone) when the folders hold packs | `live_starter_style.rs` |
| S8 | Tests per stage, a fixture pack, docs (`live-style-reference.yaml`, this file), a render check with the user's first pack | |

## Status (2026-10-16)

- S1–S6 done: SFZ and SF2 parsers, sampler (velocity zones, round robin,
  loops, bend, CC, vibrato, per-note mod envelope and mod LFO), phrases,
  `phrase` generator, `pack` / `pack_dir` loading with per-option sound
  shapes and phrase generators.
- SF2 is the primary format (Polyphone). Read: instrument-level generators
  (zones, tuning, attenuation, pan, loops, volume envelope, filter, vibrato
  LFO, mod envelope to pitch / filter, mod LFO to pitch / filter / volume)
  and CC modulators with a plain unipolar source (to cutoff, attenuation,
  vibrato depth, start offset, coarse start offset). Preset-level
  generators, velocity / key sources and reversed or bipolar CC modulators
  are listed as load warnings.
- S7 done: the starter builder switches a lane to its packs when
  `packs/<role>` holds a pack with phrases (controls dens / len / expr /
  tone); lanes without phrases keep their generated material.
- First pack: `styles/starter-breaks/packs/bass/other-dimensions/` (the
  user's Other Dimensions mudpie, modulation added; original in `source/`).
  Waiting for phrases before the render check (S8).
- Layers and crossfades (2026-10-16): a note plays every zone whose key and
  velocity ranges hold it (up to 4). Zone-level CC modulators stay with their
  zone (SF2 instrument-zone modulators: to attenuation, filter, start; SFZ
  `xfin_locc/hicc`, `xfout_locc/hicc` equal-power fades); the global zone's
  and the preset's apply to the whole instrument. CC sources may be reversed
  and use the SF2 curves (linear, concave, convex, switch). A Tone crossfade:
  zone A CC74 -> attenuation, zone B reversed CC74 -> attenuation, both
  concave.
- Knobs: Tone sets the pack's tone CC, or shifts the phrase's curve of it
  when the phrase draws that CC. Expression scales bends, vibrato, glide and
  the mod envelope and mod LFO depths.
- Knob layout (2026-10-16, user): Tone and Expression merged into one
  Character macro (sampler param 9): 0 = stabby and heavily modulated
  (decay 0.2x, no sustain, modulation x2, tone CC 0), 0.5 = the pack as
  designed, 1 = sustained, open and still (decay 3x, full sustain, no
  modulation, tone CC 127). The freed knob is Morph (sampler param 10): the
  pack's morph CC (`morph_cc` in pack.yaml, default CC20), for layer
  crossfades or a mudpie scrub. Starter pack lanes: dens, len, char, morph.
- Knob layout, revised (2026-10-16, user): Character and Length merged into
  one shape control (sampler param 9 plus the generator's `length`
  0.1..0.9 and `expression` 1..0.15): 0 = stabby, accented, heavy filter
  bite (half decay, 60 % sustain, modulation x2, tone CC 0.1), 0.5 = as
  designed, 1 = sustained and calm (double decay, full sustain, modulation
  x0.3, tone CC open). The envelope never drops to silence; stabs come
  from the gate and the filter. Pack lanes: dens, shape, morph, spread.
  The bass pack's filter floor is 400 Hz (CC74 +5000 cents).
- Bass line generated again (2026-10-17, user): the bass lane keeps the
  pack as its sound but plays the `bass` generator (`motifs: 0`, register
  up an octave to the F2 range the pack was tuned on). The bass generator
  takes `length` and `expression` like `phrase`, so dens / shape / morph /
  spread work as before. Packs on a lane whose generator is not `phrase` are
  sounds only.
