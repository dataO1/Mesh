# Sound design knowledge for Mesh live mode

How the user works and what a design must reach. Style hints are hints of the
detail and complexity required, not rules. When this file and the user's
feedback log (`sounds/feedback.md`) disagree, the feedback log wins.

## What the skill builds, and what it does not

- **Synth mode**: typical synth sounds, not resampled, processed heavily in
  stages: leads, mid basses, stabs, plucks, hoovers, pads for jumpup, Pendulum,
  metal-step, jungle-tek, and the stab leads of the neuro-hybrid sets.
- **Mudpie mode**: complex neuro and techstep basses come from the user as
  resampled samples (mudpies), already full of movement, pitch bends and
  modulation. The skill designs how each note plays them: envelope shape
  (ADSR), pitch automation on top (bends, drops, slides), filter and drive
  movement, and the processing chain after the sampler. It never tries to
  build a neuro bass from scratch, and never scans the playhead.
- Generating mudpies is a later experiment, not this skill's job yet.

## Signal flow of a lane

1. Surge XT (one instance per lane): up to 2 scenes (Dual mode = layered) x 3
   oscillators, per scene: oscillator mix -> filter 1/2 (+ config) ->
   waveshaper -> amp, then Surge's own FX slots.
2. Lane FX chain (style `fx`): native effects (`eq`, `multiband` 6-band up/down,
   `compressor`, `drive`, `clipper`, `filter`, `delay`, `reverb`, `gain`,
   `transient`, `room`), Airwindows Consolidated by name (`source: airwindows`),
   other CLAP effects (Surge XT Effects).
3. Glue: LUFS trim, role high-pass (set `hpf` per design: 0 for a mid bass),
   lane compressor and saturation, kit duck (lead: a band duck around 900 Hz
   on kick and snare), sends.
4. The bass lane (the sub) is ducked band by band by lead + extra (`glue.band_duck`,
   last stage of the bass lane). A mid bass therefore needs no high-pass: the
   sub gives way where the synths are loud. Never add a simple high-pass to
   make room for the sub.

## Sources

- **Wavetable mixing**: two or three wavetable oscillators with different
  tables or morph positions (one bright, one hollow or vocal), detuned against
  each other; morph position, skew and formant moved by envelopes or LFOs.
  Factory tables: `references/wavetables.md`; user `.wav` tables work.
- **Unison and detune**: 3-9 voices per oscillator. Supersaw leads: 7-9 voices,
  detune 15-30 cents, spread wide, with an undetuned or lightly detuned centre
  layer so the pitch stays clear. Basses and mid basses: 2-4 voices, small
  detune (5-15 cents), or a detuned pair of separate oscillators (reese). Wide
  unison below ~150 Hz smears the low end: keep the low layer narrow or mono.
- **Octaves of a stack**: layers rarely sit on the same octave (user). Spread a stack over
  octaves (played octave, +1, -1) or a fifth; detune within a layer, not between
  same-octave layers.
- **Register**: leads sit low. Pendulum's "other" stem has a median pitch around
  MIDI 46 (A#2); the user heard round 1 at +1 octave (and its bite layer at +2) as one
  or two octaves too high. Start at the generator's base register.
- **Stacking**: octave-down layer for weight, octave-up or fifth layer for
  bite (often faded in by a macro), a sine sub-oscillator only when the design
  must carry its own low end (normally the sub lane does).
- **FM**: FM2 / FM3 oscillators or scene FM routing (fm_2to1 etc.). Ratios: 1:1
  brassy, 2:1 hollow / square-ish, 3:1 and up bell, metallic, growl. FM depth on
  an envelope gives a bright attack that settles; FM depth on an LFO gives
  growl and talk. No unison on FM oscillators: layer for width.
- **Sync, ring mod, noise**: hard sync swept by an envelope for the classic
  sync lead; ring mod 1x2 for metallic edges; a short noise burst through the
  amp envelope as a transient layer.
- **Classic rave sources** (jungle-tek): hoover / Mentasm (detuned pulse or saw
  stack with pitch dive and chorus), organ stabs (M1 style: square + octaves,
  fast decay), piano and chord stabs (filter envelope, short), bleeps (sine or
  square, short, pitched high).

## Envelopes and articulation

| articulation | amp A | amp D | amp S | amp R | notes |
|---|---|---|---|---|---|
| stab | 0-2 ms | 80-200 ms | 0-30 % | 30-80 ms | filter env with fast decay does the punch |
| pluck | 0-1 ms | 150-400 ms | 0 % | 50-150 ms | decay to silence; filter env follows |
| sustained lead | 2-10 ms | 300 ms+ | 80-100 % | 80-250 ms | vibrato delayed 200-400 ms |
| legato / glide | 2-10 ms | - | 100 % | 100-300 ms | mono, portamento 30-80 ms, legato retrigger off |
| mid bass | 1-5 ms | 200 ms+ | 70-100 % | 40-120 ms | mono, envelope or LFO on the timbre per note |

- Each sound states its articulation in the spec (`articulation:`): kind,
  attack, useful register, typical note length. The riff generator will read
  it (stabs get short notes and more syncopation; sustained and reese sounds
  long notes and fewer onsets; glide sounds overlapping notes).
- **Pitch automation**: a blip on the attack (+12 to +24 st over 10-30 ms) for
  punch; dives on stabs and hoovers; portamento between notes; slow rises at
  phrase ends come from the generator (pitch bend), not the patch.
- Envelope retrigger per note on timbre modulators (wavetable position, FM,
  filter) makes each note speak; free-running LFOs make notes differ.
- The lane must not override the envelope: designed options set
  `own_envelope: true` (the compiler does).

## Movement

- **Vibrato**: delayed LFO to pitch, 5-6 Hz, 10-30 cents, faded in 200-400 ms.
  On held notes only; "here and there", not on every note.
- **Tempo-synced LFOs** (1/8, 1/16, triplets) on filter cutoff, formant,
  wavetable position or FM depth: the wobble / talk of jumpup and neuro.
- **Random per note** (random source, alternate) for small differences per
  note: detune drift, morph offset, pan.
- **Moving EQ before distortion**: a resonant peak or notch swept by an LFO or
  envelope before a drive or fold highlights a moving band of the sound (a
  formant-like movement). Very common in the user's practice.
- **Macros**: the 4 lane controls are Density, Length, Rhythm and Morph. Each
  moves the generator and the sound together (`also:` in the control). Morph
  is Surge macro 1 (CLAP param 0) and must sweep the sound's main character
  from dark and clean to screaming and distorted: cutoff, FM depth, wavetable
  position, drive and fold together. Every macro position must be usable: no
  silence, no blow-up; loudness within 3 LU across the sweep.

## Distortion types

| type | Surge | harmonics | use |
|---|---|---|---|
| soft / tanh | waveshaper soft, Distortion FX | odd, gentle | warmth, glue, before filter |
| asymmetric | waveshaper asym, zamsat, ojd | even + odd, DC shift | warmth, grit, "analog" body |
| hard clip | waveshaper hard / digital | dense odd | aggression, screams; cleans peaks |
| fuzz | fuzz variants | dense, gated feel | metal-step, screaming leads |
| wavefold | singlefold, dualfold, westfold, sinefold, linearfold | moving, complex | movement when the drive is modulated; neuro-ish talk |
| rectify | fwrectify, softrect | octave up | bite, octave layer without an oscillator |
| Chebyshev | cheby2..cheby5 | one target harmonic | adding a specific overtone |
| bitcrush / downsample | Bonsai, Distortion bitcrush, airwindows | aliasing grit | digital edge, jungle-tek retro |

- Symmetric clipping makes odd harmonics (aggressive, hollow); asymmetric adds
  even harmonics (warm, full). Pre-filter drive changes the source; post-filter
  drive adds edge on top of the filtered shape.
- Modulating drive or fold amount per note is a main source of movement.
- Several moderate stages sound better than one extreme stage.

## Processing chain (default order, switch stages per style)

1. Drive (soft or asymmetric) before the filter.
2. Filter: moving cutoff (envelope per note, LFO, macro), formant or comb.
3. Moving EQ peak or notch (before the next distortion).
4. Fold, clip or fuzz for edge.
5. OTT-style multiband (native `multiband`: upward and downward per band).
6. 76-style compression (fast FET-like: attack 0.1-1 ms, release 50-100 ms,
   4:1-8:1); works well on leads.
7. EQ for the mix: shape the sound to the curve its type needs (see the set's
   spectrum in the score); cut mud 200-400 Hz on leads, presence 1-3 kHz.
8. Second saturation or exciter.
9. Notch or dynamic EQ to tame harsh resonances (2-5 kHz).
10. Stereo: chorus, ensemble or Haas only above ~300 Hz; mono below.
11. Sends (delay, reverb): high in breaks, low in drops (set by the form).
12. Clipper on the lane for the last peaks.

Metal-step adds an amp and cabinet stage (Surge Chow / Neuron, airwindows
amp sims) and a second multiband with heavy upward compression.

## Style hints (detail and complexity required)

- **Pendulum**: never a plain supersaw. Several wavetables mixed, distortion,
  OTT, filter and ADSR automation, pitch automation, vibrato here and there.
  Two families: a distorted growling or screaming mono lead (mid-high), and a
  bright detuned saw hook, sometimes gated to the break. Octave double, a
  detuned copy, a sub oscillator; pads behind. Riffs of 2-8 bars (mostly 4)
  with a fixed pitch contour, variation from rhythm, bends and filter; drops
  often hold a pedal note with ornaments. Lead enters 4-8 bars into a section.
- **Metal-step**: screaming lead with an octave double, descending minor riffs,
  densest lead of all sets; the FX chain is more complex than it looks
  (amp/cab, fuzz, multiband, layered distortions, filters).
- **Techstep (Disphonia, early Black Sun Empire)**: mudpie mode. Dark, gritty
  mid bass with long notes and few onsets (1.1 notes per bar), repeated notes
  (32 %), trading bars with the lead (alternation 0.51). The automation of the
  mudpie playback (shape, pitch, processing) is what matters.
- **Neurofunk**: mudpie mode. The distorted sliding bass is the main melodic
  voice; repeated notes (41 %); short, bright stab lead answers it.
- **Jumpup**: synthesized. A distorted mid-range riff bass (wobble, hoover,
  square, talking filter) on one note or interval, long notes (median 7 16ths),
  many on the downbeat; few notes, big character.
- **Jungle-tek**: retro classic stabs and sounds (rave stabs, hoovers, organ
  and piano stabs, bleeps) plus modern character sounds; stabby sounds
  usually; not just saws and heavy processing. Sub and breakbeat carry the
  track.
- **Neuro-hybrid sets (detox-new, mystic-image, mindart, project-52-new)**: a
  distorted wavetable mid bass (mudpie mode) answered by short bright stab
  leads, 4-bar loops, some bit-crushed screams and glitch stabs.

## Numbers from the reference study

Medians per set are in `~/Music/mesh-live-dev/references/analysis/compare.md`;
`scripts/synth-design.sh score` compares a design against one set (spectrum of
the set's separated stem over the main drop, melody, rhythm, repetition).
Mesh's gaps (doc "Reference tracks vs Mesh live mode"): too little repetition
(lead pitch match after 4 bars 0.22 vs 0.33-0.47), too many leaps, 2-8 kHz
3-5 dB too hot, nothing ever drops out.
