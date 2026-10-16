---
name: synth-design
description: Design synth sounds (leads, mid basses, stabs, plucks, hoovers) for Mesh live mode as declarative Surge XT specs, audition them through the engine in context with their 4 controls swept, score them against the reference sets, and iterate with the user's feedback. Use when asked to create, tune or compare a lead/mid-bass/stab sound or its macros for a live-mode lane, or to plan how a mudpie should be played.
---

# Synth design for Mesh live mode

> Direction 2026-10-16 (`docs/plans/2026-10-16-live-sound-packs.md`): the user makes
> the sounds and riffs (SFZ + MIDI packs); the engine remixes them. Use this skill to
> check and tune packs against the references, and for open-synth (Surge XT) sounds the
> user asks for. Do not generate riffs as material.

You design sounds as YAML specs, compile them into Surge XT patches plus a
lane block, render them through the real engine, measure them, and iterate
until they pass the numbers and the user's ear. Read
`references/sound-design.md` before designing anything; it holds the user's
practice and the style hints. Read `sounds/feedback.md` (the user's verdicts)
before every iteration.

## Scope

- **Synth mode**: typical synth sounds, processed heavily in stages (see the
  knowledge file). Everything is Surge XT (wavetable, FM, unison, layers,
  modulation) plus a lane FX chain of native, Airwindows or CLAP effects.
- **Mudpie mode**: for neuro and techstep the source is the user's mudpie
  sample. Design the per-note shape (ADSR), pitch automation, filter and drive
  movement and the processing chain. Do not build complex neuro sounds from
  scratch.
- Not this skill: riff and form composition (the lane generator and forms),
  the glue, the drum lane.

## Loop

1. **Brief**: read `references/music-theory.md` for the riff side; style, role (lead, mid bass, stab), articulation, what the
   reference set does (`references/sound-design.md`, the set's score
   numbers), and the user's notes for this style in `sounds/feedback.md`.
2. **Spec**: write `sounds/<style>/<name>.yaml` (format: `references/spec.md`).
   Design every layer, envelope, modulation and stage on purpose; name the
   4 controls and make each move the sound musically. Parameter names and
   ranges: `references/surge-params.md`; enums and effect layouts:
   `references/surge.md`; wavetables: `references/wavetables.md`.
3. **Compile**: `scripts/synth-design.sh compile sounds/<style>/*.yaml --out <dir>`.
4. **Audition**: `scripts/synth-design.sh audition <dir> <name>... --style
   ~/Music/mesh-live-dev/styles/starter-breaks --out <out> [--lane lead]
   [--bars 64]`. It renders each design in context, a showcase in context
   where each control sweeps 0 -> 1 in turn, solo, and morph 0 / 0.5 / 1, into
   `<out>/audition.wav` with `<out>/index.txt`.
5. **Score**: `scripts/synth-design.sh score <out> --set <set> [--stem other|bass]`
   writes `<out>/score.md`: spectrum against the set's stem (flag > 3 dB off
   the median), melody and rhythm against the set's interquartile range,
   morph loudness spread (flag > 3 LU).
6. **Fix what the numbers flag**, recompile, re-render, until the hard rules
   hold:
   - spectrum within 3 dB of the set median per band (on the bands the role
     owns: a lead is judged above ~200 Hz, a mid bass above ~60 Hz),
   - melody and rhythm metrics inside the set's interquartile range where the
     sound can influence them (articulation, length, register),
   - every control position usable; morph loudness spread within 3 LU.
7. **Listening is mandatory**: hand the user `audition.wav` + `index.txt` +
   `score.md` with design codes (e.g. P1, P2 for Pendulum). Their verdict goes
   into `sounds/feedback.md` (date, code, what they said, what changed). Never
   call a design done without it.

## Rules

- Measure loudness in LUFS, never peak.
- A mid bass gets no simple high-pass (`lane.hpf: 0`): the sub lane gives way
  band by band (glue band duck). Leads keep a high-pass only if the design
  needs it.
- Designs own their envelope (`own_envelope: true`, set by the compiler).
- Keep layers purposeful: each oscillator, scene and stage must change the
  sound audibly; remove what does not.
- CPU: develop on the host without limits, but note heavy patches (16-voice
  unison x several oscillators) in the spec comment; they may be trimmed for
  the board later.

## Tools (scripts/synth-design/)

| tool | what |
|---|---|
| `compile.py` | spec -> `.fxp` (through surgepy, Surge's own code) + `.lane.yaml` |
| `audition.py` | style copy with the designs on one lane, loudness calibration, renders, index |
| `score.py` | design lane vs reference set (spectrum, melody, morph loudness) |
| `surge_dump.py` | regenerates `references/surge-params.md` |
| `surgepy-mesh.patch` | adds `loadWavetable` and `setMacro` to surgepy |

`scripts/synth-design.sh setup` builds surgepy and the Python environment
once (cache `~/.cache/mesh-synth-design`).
