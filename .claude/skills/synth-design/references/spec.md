# Sound spec format

One YAML file per sound in `sounds/<style>/<name>.yaml`. `compile.py` turns it
into `<name>.fxp` (Surge XT patch) and `<name>.lane.yaml` (the lane option,
FX, controls and high-pass that `audition.py` merges into a style).

```yaml
name: pendulum-p1            # unique; file names and the lane option use it
style: pendulum              # reference set it is scored against
role: lead                   # lane role: lead | extra
articulation:                # read by the riff generator (trial)
  kind: sustained            # stab | pluck | sustained | legato | midbass
  attack_ms: 4
  register: [55, 79]         # useful MIDI note range
  length_16ths: [1, 4]       # typical note lengths
notes: |                     # design intent, layer by layer (for the user and the next iteration)
  Scene A: bright 2-table wavetable stack ... Scene B: distorted saw octave down ...

surge:
  base: Init Wavetable       # optional: patches_factory/Templates/<base>.fxp, else Surge's init
  set:                       # Surge parameters by their full name (references/surge-params.md)
    Scene Mode: dual         # sm_*: single | split | dual | chsplit
    A Play Mode: mono_st     # pm_*: poly | mono | mono_st | mono_fp | mono_st_fp | latch
    A Portamento: 40 ms
    A Osc 1 Type: wavetable  # ot_*: classic, sine, wavetable, shnoise, audioinput, FM3, FM2, window, modern, string, twist, alias
    A Osc 1 Unison Voices: 7
    A Osc 1 Unison Detune: 0.22      # raw value (0..1 = 0..100 cents for most oscillators)
    A Osc 1 Volume: -3 dB
    A Filter 1 Type: <name or index> # see references/surge.md for fut_* names
    A Filter 1 Cutoff: 2400 Hz
    A Filter 1 Resonance: 25 %
    A Filter 1 FEG Mod Amount: 24 st
    A Waveshaper Type: <name or index>
    A Waveshaper Drive: 9 dB
    A Amp EG Attack: 4 ms
    A Amp EG Release: 150 ms
    A LFO 1 Type: sine       # lt_*: sine, tri, square, ramp, noise, snh, envelope, stepseq, mseg, formula
    A LFO 1 Rate: 5.5 Hz
    A LFO 1 Delay: 300 ms
  wavetables:                # scene letter + oscillator number -> factory path (wavetables.md) or absolute .wt/.wav
    A1: Basic/Sine To Sawtooth HQ.wav
  macros: [Morph, Dirt, Move, Size]   # labels of Surge macros 1..4 (= CLAP params 0..3)
  mod:                       # [source, destination, depth]; depth -1..1 of the destination's range
    - [macro1, A Filter 1 Cutoff, 0.35]
    - [lfo1, A Osc 1 Pitch, 0.01]     # vibrato
    - [filtereg, A Osc 1 Morph, 0.4]
  # sources: macro1..8, lfo1..6 (voice), slfo1..6 (scene), ampeg, filtereg,
  # velocity, keytrack, modwheel, pitchbend, random, random_uni, alternate

lane:
  hpf: 0                     # role high-pass of the lane in Hz; 0 = off (mid basses)
  fx:                        # lane FX chain, style FxSpec entries, in order
    - {id: multiband, source: native, settings: {band_3_up_ratio: 3}}
    - {id: Pop2, source: airwindows}
  generator:                 # lane generator parameters for this sound (register, mode, ...)
    register: 0.5
  controls:                  # exactly 4: Density, Length, Rhythm, Morph
    - label: dens
      target: {type: generator, param: density}
      default: 0.5
    - label: len
      target: {type: generator, param: <length param>}
      default: 0.5
    - label: rhy
      target: {type: generator, param: complexity}
      default: 0.4
    - label: morph
      target: {type: instrument, param: 0}   # Surge macro 1
      default: 0.3
      also:                                  # one fader can move more targets
        - {target: {type: fx, slot: 0, param: 12}, min: 0.2, max: 0.8}
```

Values: plain numbers are Surge's own units (`setParamVal`); strings with a
unit convert: `Hz`/`kHz` (cutoffs as notes from A440, LFO rates as log2 Hz),
`ms`/`s` (envelope and portamento times, log2 seconds), `dB` (amplitudes
0..1 become 2^(dB/18)), `%` (/100), `st` (semitones). Enum parameters take
names (`wavetable`, `mono_st`, `dual`, `sine`) or indices.

Types are set first and applied before everything else, so the oscillator's
parameter names after a type change are the new type's (an FM2 oscillator
has `M1 Amount`, not `Morph`).

Mudpie mode (planned): the option is a sampler with the user's mudpie
(`sampler: samples: [...]`, `stream: true`) instead of `surge:`, plus a
per-note shape and pitch automation block; needs engine support first.
