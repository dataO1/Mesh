# Surge XT 1.3.4 technical reference

Facts verified against source at `/nix/store/clagjpx1mslrpvxyj3v4kl7kgixyjmqr-source` (call it `$SRC`). Paths below are relative to `$SRC`. "Stored" = the value in XML `value=` and the unit `surgepy.setParamVal` takes. "Display" = what the Surge UI shows. Items marked (unverified) were inferred, not read from code.

Conventions: `a_`/`b_` = scene A/B. Parameter streaming names are `{scene}_{group}{n}_{name}` (`src/common/Parameter.cpp: get_prefix`). Osc, filter, env groups are 1-based in names (`a_osc1_`, `a_filter2_`, `a_env1_`). LFOs are 0-based (`a_lfo0_` .. `a_lfo11_`). FX are `fx1_` .. `fx16_`.

Sections: 1 patch structure, 2 oscillators, 3 mixer and filter configs, 4 filters and waveshapers, 5 envelopes and LFOs, 6 modulation and macros, 7 effects, 8 surgepy, 9 conversion formulas, 10 file layout and wavetable blobs.

## 1. Patch structure

Sources: `src/common/SurgeStorage.h` (enums, counts), `src/common/SurgePatch.cpp` (parameter creation, `init_default_values`, `save_xml`/`load_xml`).

Counts: 2 scenes, 3 oscs/scene, 2 filter units/scene, 2 EGs/scene (env1 = amp EG, env2 = filter EG), 12 LFOs/scene (6 voice + 6 scene), 8 macros, 16 FX slots, 12 params/FX slot, 7 params/osc. Internal block size 32 samples (build default `SURGE_COMPILE_BLOCK_SIZE=32`, `src/CMakeLists.txt`). Current streaming revision `ff_revision = 24`.

### Global parameters (XML names, order as saved)

| XML name | ctrltype | stored range | default | notes |
|---|---|---|---|---|
| `volume_FX1`..`volume_FX4` | ct_amplitude | 0..1 | 1 | send FX 1..4 return level (dB = 18*log2(v)) |
| `volume` | ct_decibel_attenuation_clipper | -48..0 dB | 0 | global volume, stored in dB |
| `scene_active` | int | 0..1 | 0 | 0 = A, 1 = B. In `single` mode only this scene sounds |
| `scenemode` | int | 0..3 | 0 | see below |
| `splitkey` | int | 0..127 | 60 | key split point, or channel*8 in chsplit |
| `fx_disable` | int bitmask | - | 0 | bit n = FX slot n (slot enum order) bypassed |
| `polylimit` | int | 2..64 | 16 | |
| `fx_bypass` | int | 0..3 | 0 | 0 all FX, 1 no send FX, 2 scene FX only (no send, no global), 3 all FX off |
| `character` | int | 0..2 | 1 | 0 Warm, 1 Neutral, 2 Bright (last param in file) |

### scenemode (`scene_mode`)

| value | name | behavior (`SurgeSynthesizer.cpp` ~361-400) |
|---|---|---|
| 0 | Single | only `scene_active` scene is played |
| 1 | Key Split | key < `splitkey` -> scene A, key >= `splitkey` -> scene B |
| 2 | Dual | both scenes play every note |
| 3 | Channel Split | MIDI channel < splitkey/8 + 1 -> A, else B |

### polymode (`{a,b}_polymode`, `play_mode`)

| value | surgepy const | name | meaning |
|---|---|---|---|
| 0 | pm_poly | Poly | |
| 1 | pm_mono | Mono | retriggers EGs on every note |
| 2 | pm_mono_st | Mono (Single Trigger) | legato: EGs not retriggered while a note is held |
| 3 | pm_mono_fp | Mono (Fingered Portamento) | portamento only on overlapped (legato) notes |
| 4 | pm_mono_st_fp | Mono (Single Trigger and Fingered Portamento) | |
| 5 | pm_latch | Latch (Monophonic) | |

There is no "legato" enum. Legato = `pm_mono_st` (2) or `pm_mono_st_fp` (4).

Non-parameter mono options are saved in `<nonparamconfig>`: `monoVoicePrority_{0,1}` (sic) v = 0 NOTE_ON_LATEST_RETRIGGER_HIGHEST, 1 ALWAYS_LATEST, 2 ALWAYS_HIGHEST, 3 ALWAYS_LOWEST; `monoVoiceEnvelope_{0,1}` v = 0 RESTART_FROM_ZERO, 1 RESTART_FROM_LATEST; `polyVoiceRepeatedKeyMode_{0,1}` v = 0 new voice every note-on, 1 one voice per key; `hardclipmodes` global/sc0/sc1 = 1 clip at +18 dBFS, 2 clip at 0 dBFS, 3 bypass (scene only).

### Scene-level pitch/amp parameters (per scene, prefix `a_`/`b_`)

| XML name | ctrltype | stored range | default | meaning |
|---|---|---|---|---|
| `octave` | ct_pitch_octave | int -3..3 | 0 | scene octave shift |
| `pitch` | ct_pitch_semi7bp | -7..7 semitones (extend_range: x12 = +-84) | 0 | scene transpose |
| `portamento` | ct_portatime | -8..2 (log2 s) | -8 | time = 2^v s, -8 = off |
| `fm_switch` | ct_fmconfig | int 0..3 | 0 | see section 3 |
| `fm_depth` | ct_decibel_fmdepth | -48..24 dB | -24 | |
| `drift` | ct_percent_oscdrift | 0..1 | 0 | oscillator drift |
| `noisecol` | ct_noise_color | -1..1 | 0 | noise colour (unverified: 0 = white), attr `deform_type` 0 = stereo noise, 1 = mono |
| `ktrkroot` | ct_midikey | int 0..127 | 60 | key where keytrack = 0 |
| `volume` | ct_amplitude_clipper | 0..1 | 0.8909 (-3 dB) | linear gain = v^3 |
| `pan` | ct_percent_bipolar_pan | -1..1 | 0 | |
| `pan2` | ct_percent_bipolar | -1..1 | 0 | this is **Width** (renamed from Pan 2 at rev 8) |
| `send_fx_1`..`send_fx_4` | ct_sendlevel | 0..1.5874 (+12 dB) | 0 | dB = 18*log2(v) |
| `pbrange_up`, `pbrange_dn` | ct_pbdepth | int 0..24 semitones | 2 | |
| `vca_level` | ct_decibel | -48..48 dB | 0 | "VCA Gain" |
| `vca_velsense` | ct_decibel_attenuation | -48..0 dB | 0 | velocity -> gain |
| `lowcut` | ct_freq_hpf | -72..15 (6.9 Hz..1046 Hz) | -72 | scene **highpass**; attr `deform_type` 0..3 = 12/24/36/48 dB/oct. `deactivated="1"` bypasses |
| `level_pfg` | ct_decibel | -48..48 dB | 0 | pre-filter gain |

### Portamento (`{a,b}_portamento`)

Stored log2 seconds, `t = 2^v` s (range 3.9 ms .. 4 s). Extra XML attributes on this element: `porta_const_rate` (0/1), `porta_gliss` (0/1), `porta_retrigger` (0/1), `porta_curve` (-1 log, 0 linear, 1 exp), `temposync="1"` allowed. Source: `SurgePatch.cpp` save/load, `Parameter.cpp: can_temposync`.

## 2. Oscillators

Sources: `src/common/SurgeStorage.h` (`osc_type`), `src/common/dsp/oscillators/*.cpp` (`init_ctrltypes`, `init_default_values`), `src/common/globals.h` (`MAX_UNISON = 16`).

Per osc (n = 1..3) XML params: `type` (int), `octave` (int -3..3), `pitch` (ct_pitch_semi7bp_absolutable, -7..7 semitones, extend x12, `absolute="1"` = Hz mode), `param0`..`param6`, `keytrack` (bool, default 1), `retrigger` (bool, default 0).

### ot_* enum

| idx | name | uses wavetable blob | unison | surgepy const |
|---|---|---|---|---|
| 0 | Classic | no | p5 detune, p6 voices | ot_classic |
| 1 | Sine | no | p5, p6 | ot_sine |
| 2 | Wavetable | yes | p5, p6 | ot_wavetable |
| 3 | S&H Noise | no | p5, p6 | ot_shnoise |
| 4 | Audio In | no | none | ot_audioinput |
| 5 | FM3 | no | none | ot_FM3 |
| 6 | FM2 | no | none | ot_FM2 |
| 7 | Window | yes | p5, p6 | ot_window |
| 8 | Modern | no | p5, p6 | (none, use 8) |
| 9 | String | no | none | (none, use 9) |
| 10 | Twist | no | none | (none, use 10) |
| 11 | Alias | no | p5, p6 | (none, use 11) |

Unison (all types that have it): `voices` int 1..16 (ct_osccount), `detune` ct_oscspread stored 0..1, display cents = 100*v (extend_range: x12 -> up to 1200 cents; `absolute="1"` = Hz). Voices are spread evenly from -detune to +detune (semitone units, `OscillatorCommonFunctions.h: UnisonSetup`), per-voice gain 1/sqrt(n), pan alternates L/R. Default detune 0.1 (Classic, Sine, WT, S&H, Window), 0.2 (Modern, Alias).

Param 6 is stored as `type="0"` (int) in XML where it is unison voices.

### Classic (0): `co_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Shape | percent_bipolar | -1..1 | 0 |
| 1 | Width 1 | percent | 0..1 | 0.5 |
| 2 | Width 2 | percent | 0..1 | 0.5 |
| 3 | Sub Mix | percent | 0..1 | 0 |
| 4 | Sync | ct_syncpitch | 0..60 semitones above osc pitch, >0 enables hard sync | 0 |
| 5 | Unison Detune | oscspread | 0..1 | 0.1 |
| 6 | Unison Voices | osccount | 1..16 | 1 |

Shape morph (unverified from code, matches Surge manual): -1 pulse, 0 saw, +1 dual saw; Width 1/2 set pulse width / second saw phase.

### Sine (1): `sine_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Shape | ct_sineoscmode | int 0..27 ("Wave 1".."Wave 28"; 0..7 are also "TX 1..8") | 0 |
| 1 | Feedback | osc_feedback_negative | -1..1 (extend x4); attr `deform_type` 0 Surge, 1 Vintage FM | 0 |
| 2 | Behavior | ct_sinefmlegacy | int 0 = legacy (<1.6.2), 1 = same as FM2/3 | 1 |
| 3 | Low Cut | freq_audible_deactivatable_hp | -60..70 | -60, deactivated |
| 4 | High Cut | freq_audible_deactivatable_lp | -60..70 | 70, deactivated |
| 5 | Unison Detune | oscspread | 0..1 | 0.1 |
| 6 | Unison Voices | osccount | 1..16 | 1 |

### Wavetable (2): `wt_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Morph | countedset_percent_extendable | 0..1; frame position = v*(n_tables-1). `extend_range="1"` = interpolate between frames, `0` = stepped | 0, extend_range=1 |
| 1 | Skew Vertical | percent_bipolar | -1..1 | 0 |
| 2 | Saturate | percent | 0..1 | 0 |
| 3 | Formant | ct_syncpitch | 0..60 semitones | 0 |
| 4 | Skew Horizontal | percent_bipolar | -1..1 | 0 |
| 5 | Unison Detune | oscspread | 0..1 | 0.1 |
| 6 | Unison Voices | osccount | 1..16 | 1 |

Wavetable data: see section 10. `wt_params` names in Surge UI: p1 "Skew Vertical", p4 "Skew Horizontal".

### S&H Noise (3): `shnoise_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Correlation | percent_bipolar | -1..1 | 0 |
| 1 | Width | percent | 0..1 | 0.5 |
| 2 | Low Cut | freq_audible_deactivatable_hp | -60..70 | -60, deactivated |
| 3 | High Cut | freq_audible_deactivatable_lp | -60..70 | 70, deactivated |
| 4 | Sync | ct_syncpitch | 0..60 | 0 |
| 5, 6 | Unison Detune / Voices | | | 0.1 / 1 |

### Audio In (4)

p0 channel (percent_bipolar), p1 gain (dB +-48), p2-p4 scene-A-in-B mix controls, p5 low cut, p6 high cut. Needs host input audio; not useful for offline patch authoring.

### FM3 (5): `fm3_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | M1 Amount | percent | 0..1 | 0 |
| 1 | M1 Ratio | ct_fmratio | 0..32 float. non-extended: ratio C:v. extend_range: v<=16 -> 1/(1+(16-v)*31/16), v>16 -> 1+(v-16)*31/16. `absolute="1"` = Hz | 1 (16 if extended/absolute) |
| 2 | M2 Amount | percent | 0..1 | 0 |
| 3 | M2 Ratio | ct_fmratio | as above | 1 (16) |
| 4 | M3 Amount | percent | 0..1 | 0 |
| 5 | M3 Frequency | freq_audible_fm3_extendable | -60..70 | 0 |
| 6 | Feedback | osc_feedback_negative | -1..1 (x4 extended), attr `deform_type` 0/1 | 0 |

M3 is a ring/amplitude modulator in absolute frequency. DSP uses `pitch_to_omega(60 + v)` (261.6 Hz at v=0, `FM3Oscillator.cpp:126`) while the display formula is 440*2^(v/12) (440 Hz at v=0): 9 semitone display/DSP mismatch in source, trust the DSP.

### FM2 (6): `fm2_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | M1 Amount | percent | 0..1 | 0 |
| 1 | M1 Ratio | ct_fmratio_int | int 1..32 ("C : n") | 1 |
| 2 | M2 Amount | percent | 0..1 | 0 |
| 3 | M2 Ratio | ct_fmratio_int | int 1..32 | 1 |
| 4 | M1/2 Offset | ct_freq_fm2_offset | -10..10 Hz (extend_range: x100 -> +-1000 Hz) | 0 |
| 5 | M1/2 Phase | percent | 0..1 | 0 |
| 6 | Feedback | osc_feedback_negative | -1..1; `deform_type` 0 Surge / 1 Vintage FM | 0 |

### Window (7): `window_params` (needs wavetable blob)

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Morph | countedset_percent_extendable | 0..1 | 0 |
| 1 | Formant | ct_pitch | -60..60 semitones | 0 |
| 2 | Window | ct_wt2window | int 0..8: Triangle, Cosine, Blend 1, Blend 2, Blend 3, Sawtooth, Sine, Square, Rectangle | 0 |
| 3 | Low Cut | freq_audible_deactivatable_hp | -60..70 | -60, deactivated |
| 4 | High Cut | freq_audible_deactivatable_lp | -60..70 | 70, deactivated |
| 5, 6 | Unison Detune / Voices | | | 0.1 / 1 |

### Modern (8): `mo_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Sawtooth | percent_bipolar | -1..1 | 0.5 |
| 1 | Pulse | percent_bipolar | -1..1 | 0 |
| 2 | Triangle/Square/Sine | ct_modern_trimix | -1..1; attr `deform_type` 0 triangle, 1 square, 2 sine | 0 |
| 3 | Width | percent | 0..1 | 0.5 |
| 4 | Sync | ct_syncpitch | 0..60 | 0 |
| 5, 6 | Unison Detune / Voices | | | 0.2 / 1 |

### String (9): `str_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Exciter | stringosc_excitation_model | int 0..14: 0 Burst Noise, 1 Burst Pink Noise, 2 Burst Sine, 3 Burst Triangle, 4 Burst Ramp, 5 Burst Square, 6 Burst Sweep, 7 Constant Noise, 8 Constant Pink Noise, 9 Constant Sine, 10 Constant Triangle, 11 Constant Ramp, 12 Constant Square, 13 Constant Sweep, 14 Audio In | 0 |
| 1 | Exciter Level | percent_with_string_deform_hook | 0..1; attr `deform_type` bit flags: 2 = 1x, 4 = 2x oversample; 8 ZOH, 16 linear, 32 sinc interpolation | 1; deform 2\|32 |
| 2 | String 1 Decay | percent_with_extend_to_bipolar_static_default | 0..1 (extend_range: v -> 2v-1) | 0.95 |
| 3 | String 2 Decay | same | 0..1 | 0.95 |
| 4 | String 2 Detune | oscspread_bipolar | -1..1, cents = 100*v | 0.1 |
| 5 | String Balance | percent_bipolar_stringbal | -1 = string 1, +1 = string 2 | 0 |
| 6 | Stiffness | percent_bipolar_with_string_filter_hook | -1..1; `deform_type` 64 fixed, 128 keytrack, 256 compensate | 0; deform 64 |

No unison. Output decays after trigger; set the amp EG sustain/release accordingly.

### Twist (10): `twist_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Engine | ct_twist_engine | int 0..15: 0 Waveforms, 1 Waveshaper, 2 2-Operator FM, 3 Formant/PD, 4 Harmonic, 5 Wavetable, 6 Chords, 7 Vowels/Speech, 8 Granular Cloud, 9 Filtered Noise, 10 Particle Noise, 11 Inharmonic String, 12 Modal Resonator, 13 Analog Kick, 14 Analog Snare, 15 Analog Hi-Hat | 0 |
| 1 | Harmonics | percent_bipolar_w_dynamic_unipolar_formatting | -1..1 stored (shown 0..100% for unipolar engines, label changes per engine) | 0 |
| 2 | Timbre | same | -1..1 | 0 |
| 3 | Morph | same | -1..1 | 0 |
| 4 | Aux Mix | ct_twist_aux_mix | -1..1 | -1 |
| 5 | LPG Response | percent_deactivatable | 0..1 | 0, deactivated |
| 6 | LPG Decay | percent | 0..1 | 0 |

No unison.

### Alias (11): `ao_params`

| p | name | ctrltype | stored range | default |
|---|---|---|---|---|
| 0 | Shape | ct_alias_wave | int 0..17 (stream ids): 0 Sine, 1 Ramp, 2 Pulse, 3 Noise, 4 Alias Mem, 5 Osc Mem, 6 Scene Mem, 7 DAW Chunk Mem, 8 Step Seq Mem, 9 Audio In, 10..16 TX 2..TX 8, 17 Additive | 0 |
| 1 | Wrap | percent | 0..1 | 0 |
| 2 | Mask | ct_alias_mask | 0..1; `deform_type` selects mask mode | 0 |
| 3 | Threshold | countedset_percent | 0..1 in 1/255 steps | 0.5 |
| 4 | Bitcrush | ct_alias_bits | 1..8 bits | 8 |
| 5, 6 | Unison Detune / Voices | | | 0.2 / 1 |

## 3. Oscillator section (mixer, FM routing, filter configuration)

Sources: `SurgePatch.cpp` lines ~200-440, `SurgeStorage.h` (`filter_config`, `fm_routing`), `src/common/dsp/QuadFilterChain.cpp`.

### Mixer parameters (per scene)

| XML name | ctrltype | stored range | default | notes |
|---|---|---|---|---|
| `level_o1`, `level_o2`, `level_o3` | ct_amplitude | 0..1 | 1 | dB = 18*log2(v), gain = v^3 |
| `mute_o1`..`mute_o3`, `mute_noise`, `mute_ring12`, `mute_ring23` | bool | 0/1 | o1 = 0, all others = 1 | **o2, o3, noise, rings are muted by default** |
| `solo_*` | bool | 0/1 | 0 | |
| `route_o1`..`route_o3`, `route_noise`, `route_ring12`, `route_ring23` | ct_oscroute | int 0..2 | 1 | 0 = Filter 1, 1 = Both, 2 = Filter 2 |
| `level_ring12`, `level_ring23` | ct_amplitude_ringmod | 0..1 | 1 | ring mod of osc1 x osc2 / osc2 x osc3. attr `deform_type` = combinator mode (0 = plain ring mod, see enum below) |
| `level_noise` | ct_amplitude | 0..1 | 1 | noise level, colour via `noisecol` |
| `level_pfg` | ct_decibel | -48..48 dB | 0 | pre-filter gain |

Combinator enum (`CombinatorMode`): 0 ring, 1 cxor43_0, 2 cxor43_1, 3 cxor43_2, 4 cxor43_3_legacy, 5 cxor43_4_legacy, 6 cxor43_3, 7 cxor43_4, 8..12 cxor93_0..4.

### FM routing

| `fm_switch` | surgepy const | name | meaning |
|---|---|---|---|
| 0 | fm_off | Off | |
| 1 | fm_2to1 | 2 > 1 | osc 2 frequency-modulates osc 1 |
| 2 | fm_3to2to1 | 3 > 2 > 1 | |
| 3 | fm_2and3to1 | 2 > 1 < 3 | both modulate osc 1 |

`fm_depth` is dB (-48..24, default -24). Whether a muted osc still acts as an FM modulator is not confirmed in source (`SurgeVoice::set_path` takes the FM mode separately from the mute flags); verify by ear.

### Filter configuration `fb_config` (`filter_config`)

There is **no "parallel" or "dual-sum" enum name** in 1.3.4. Order and behavior from `QuadFilterChain.cpp` (A = filter 1, B = filter 2, WS = waveshaper):

| value | const | name | signal flow |
|---|---|---|---|
| 0 | fc_serial1 | Serial 1 | A -> WS -> B, no feedback |
| 1 | fc_serial2 | Serial 2 | A -> WS -> B with feedback |
| 2 | fc_serial3 | Serial 3 | A -> WS, B only in the feedback path |
| 3 | fc_dual1 | Dual 1 | A and B in parallel (osc routes pick which gets what), balance-mixed, then WS |
| 4 | fc_dual2 | Dual 2 | WS on branch A only, then A and B balance-mixed |
| 5 | fc_stereo | Stereo | A processes left, B right, WS on both |
| 6 | fc_ring | Ring | A x B ring-modulated, then WS |
| 7 | fc_wide | Wide | 4 filters: A and B on each channel (stereo osc) |

Related params:

| XML name | ctrltype | stored range | default | meaning |
|---|---|---|---|---|
| `feedback` | ct_filter_feedback | -1..1 (extend x4) | 0 | filter-chain feedback (not used in serial1) |
| `f_balance` | ct_percent_bipolar | -1..1 | 0 | -1 = filter 1 only, +1 = filter 2 only (dual/stereo/ring) |
| `f2_cf_is_offset` | ct_bool_relative_switch | 0/1 | 0 | 1: filter 2 cutoff/envmod/keytrack become offsets from filter 1 (cutoff then ct_freq_mod, +-96 semitones). Toggling rewrites values in the synth (adds/subtracts F1 values) |
| `f2_link_resonance` | ct_bool_link_switch | 0/1 | 0 | filter 2 resonance follows filter 1 |

## 4. Filters and waveshaper

Sources: `libs/sst/sst-filters/include/sst/filters/FilterConfiguration.h`, `src/common/FilterConfiguration.h`, `libs/sst/sst-waveshapers/include/sst/waveshapers/WaveshaperConfiguration.h`.

### Per-filter parameters (`{a,b}_filter{1,2}_*`)

| name | ctrltype | stored range | default | units / meaning |
|---|---|---|---|---|
| `type` | int | 0..33 | 0 (Off) | see table |
| `subtype` | int | 0..15, clamped to `fut_subcount[type]-1` | 0 | see table. Set **after** `type`: changing type restores the remembered subtype for that type |
| `cutoff` | freq_audible_with_tunability | -60..70 | 3 | semitones relative to **440 Hz**: Hz = 440*2^(v/12). 0 = 440 Hz, 3 = 523 Hz. Filter 2 in offset mode: ct_freq_mod +-96 |
| `resonance` | ct_percent | 0..1 | 0 | filter 2 may be linked |
| `envmod` | ct_freq_mod | -96..96 semitones | 0 | cutoff shift at full FEG (env2) output; modulation of cutoff is additive in semitones |
| `keytrack` | percent_bipolar | -1..1 | 0 | 1 = cutoff follows pitch 1:1, relative to `ktrkroot` (default key 60) |

All five float params are modulatable (cutoff/res/envmod/keytrack). Cutoff modulation depth is in semitones (stored) = depth01 * 130.

### fut_* index -> name (stream order; `FilterType` enum)

| idx | enum | UI name | subtypes (count: names) |
|---|---|---|---|
| 0 | fut_none | Off | 0 |
| 1 | fut_lp12 | LP 12 dB | 3: Standard, Driven, Clean |
| 2 | fut_lp24 | LP 24 dB | 3: Standard, Driven, Clean |
| 3 | fut_lpmoog | LP Legacy Ladder | 4: 6, 12, 18, 24 dB |
| 4 | fut_hp12 | HP 12 dB | 3 |
| 5 | fut_hp24 | HP 24 dB | 3 |
| 6 | fut_bp12 | BP 12 dB | 3 |
| 7 | fut_notch12 | N 12 dB | 2: Standard, Mild |
| 8 | fut_comb_pos | FX Comb + | 2: 50% wet, 100% wet |
| 9 | fut_SNH | FX Sample & Hold | 0 |
| 10 | fut_vintageladder | LP Vintage Ladder | 4: Type 1, Type 1 Compensated, Type 2, Type 2 Compensated |
| 11 | fut_obxd_2pole_lp | LP OB-Xd 12 dB | 2: Standard, Pushed |
| 12 | fut_obxd_4pole | LP OB-Xd 24 dB | 4: 6, 12, 18, 24 dB |
| 13 | fut_k35_lp | LP K35 | 5: No, Mild, Moderate, Heavy, Extreme Saturation |
| 14 | fut_k35_hp | HP K35 | 5 (same) |
| 15 | fut_diode | LP Diode Ladder | 4: 6, 12, 18, 24 dB |
| 16 | fut_cutoffwarp_lp | LP Cutoff Warp | 12 (see below) |
| 17 | fut_cutoffwarp_hp | HP Cutoff Warp | 12 |
| 18 | fut_cutoffwarp_n | N Cutoff Warp | 12 |
| 19 | fut_cutoffwarp_bp | BP Cutoff Warp | 12 |
| 20 | fut_obxd_2pole_hp | HP OB-Xd 12 dB | 2 |
| 21 | fut_obxd_2pole_n | N OB-Xd 12 dB | 2 |
| 22 | fut_obxd_2pole_bp | BP OB-Xd 12 dB | 2 |
| 23 | fut_bp24 | BP 24 dB | 3 |
| 24 | fut_notch24 | N 24 dB | 2 |
| 25 | fut_comb_neg | FX Comb - | 2 |
| 26 | fut_apf | FX Allpass | 0 |
| 27 | fut_cutoffwarp_ap | FX Cutoff Warp AP | 12 |
| 28 | fut_resonancewarp_lp | LP Res Warp | 8 |
| 29 | fut_resonancewarp_hp | HP Res Warp | 8 |
| 30 | fut_resonancewarp_n | N Res Warp | 8 |
| 31 | fut_resonancewarp_bp | BP Res Warp | 8 |
| 32 | fut_resonancewarp_ap | FX Res Warp AP | 8 |
| 33 | fut_tripole | MULTI Tri-pole | 12 |

Nonlinear-feedback subtype index: Cutoff/Resonance Warp: `i & 3` = stages-1 (1..4 stages), `(i >> 2) & 3` = saturator 0 tanh, 1 Soft Clip, 2 OJD (3 = Sine, not reachable: 12 subtypes = 3 saturators x 4 stages; Res Warp has 8 = tanh, Soft Clip). Tri-pole: `i & 3` = LLL, LHL, HLH, HHH; `(i >> 2) & 3` = output stage (first, second, third).

Old (rev <15) enums are remapped on load; always write rev 24 indices above.

### Waveshaper (`{a,b}_ws_type`, `{a,b}_ws_drive`)

`ws_type` int 0..42 (43 types, default 0 = off). `ws_drive` ct_decibel_narrow_short_extendable: -24..24 dB (extend x2). Position in the filter chain: see section 3 table (after A in serial configs).

| idx | enum | name | idx | enum | name |
|---|---|---|---|---|---|
| 0 | wst_none | Off | 22 | wst_addsaw3 | Additive Saw 3 |
| 1 | wst_soft | Soft | 23 | wst_addsqr3 | Additive Square 3 |
| 2 | wst_hard | Hard | 24 | wst_fuzz | Fuzz |
| 3 | wst_asym | Asymmetric | 25 | wst_fuzzsoft | Fuzz Soft Clip |
| 4 | wst_sine | Sine | 26 | wst_fuzzheavy | Heavy Fuzz |
| 5 | wst_digital | Digital | 27 | wst_fuzzctr | Fuzz Center |
| 6 | wst_cheby2 | Soft Harmonic 2 | 28 | wst_fuzzsoftedge | Fuzz Soft Edge |
| 7 | wst_cheby3 | Soft Harmonic 3 | 29 | wst_sinpx | Sin+x |
| 8 | wst_cheby4 | Soft Harmonic 4 | 30 | wst_sin2xpb | Sin 2x + x |
| 9 | wst_cheby5 | Soft Harmonic 5 | 31 | wst_sin3xpb | Sin 3x + x |
| 10 | wst_fwrectify | Full Wave | 32 | wst_sin7xpb | Sin 7x + x |
| 11 | wst_poswav | Half Wave Positive | 33 | wst_sin10xpb | Sin 10x + x |
| 12 | wst_negwav | Half Wave Negative | 34 | wst_2cyc | 2 Cycle |
| 13 | wst_softrect | Soft Rectifier | 35 | wst_7cyc | 7 Cycle |
| 14 | wst_singlefold | Single Fold | 36 | wst_10cyc | 10 Cycle |
| 15 | wst_dualfold | Double Fold | 37 | wst_2cycbound | 2 Cycle Bound |
| 16 | wst_westfold | West Coast Fold | 38 | wst_7cycbound | 7 Cycle Bound |
| 17 | wst_add12 | Additive 1+2 | 39 | wst_10cycbound | 10 Cycle Bound |
| 18 | wst_add13 | Additive 1+3 | 40 | wst_zamsat | Medium |
| 19 | wst_add14 | Additive 1+4 | 41 | wst_ojd | OJD |
| 20 | wst_add15 | Additive 1+5 | 42 | wst_softfold | Soft Single Fold |
| 21 | wst_add12345 | Additive 12345 | | | |

surgepy only exports `wst_none..wst_digital` (0..5) as constants. The FX waveshaper (`fxt_waveshaper`) uses the same 0..42 indices. Distortion and Rotary FX use an 8-entry subset (section 7).

## 5. Envelopes and LFOs

Sources: `SurgePatch.cpp` (parameter creation ~455-690, defaults ~880-960), `src/common/Parameter.cpp` (envtime/lforate), `src/common/dsp/modulators/*`.

### Envelopes (`{a,b}_env1_*` = amp EG, `{a,b}_env2_*` = filter EG)

`adsr_ampeg = 0` (xml env1), `adsr_filteg = 1` (xml env2). Mod sources ms_ampeg (15) and ms_filtereg (16), unipolar 0..1. env2 drives filter `envmod`.

| XML name | ctrltype | stored range | init default | units |
|---|---|---|---|---|
| `attack` | ct_envtime | -8..5 | -8 | log2 seconds: t = 2^v s (3.9 ms .. 32 s). At -8 shows 0 |
| `attack_shape` | ct_envshape_attack | int 0..2 | 1 | 0 Convex, 1 Linear, 2 Concave |
| `decay` | ct_envtime | -8..5 | -2 | 2^v s (0.25 s default) |
| `decay_shape` | ct_envshape | int 0..2 | 1 | 0 Linear, 1 Quadratic, 2 Cubic |
| `sustain` | ct_percent | 0..1 | 1 | linear level |
| `release` | env1: ct_envtime_deformable; env2: ct_envtime | -8..5 | -5 | 2^v s (31 ms default). env1 attr `deform_type` 1 = "freeze release at sustain level" (rev 23+) |
| `release_shape` | ct_envshape | int 0..2 | 2 | 0 Linear, 1 Quadratic, 2 Cubic |
| `mode` | ct_envmode | int 0..1 | 0 | 0 Digital, 1 Analog |

`temposync="1"` is allowed on attack/decay/release (time = value as note-length, see section 9). Quick time table: -8 3.9 ms, -6 15.6 ms, -5 31 ms, -4 62 ms, -3 125 ms, -2 250 ms, -1 0.5 s, 0 1 s, 1 2 s, 2 4 s, 3 8 s, 4 16 s, 5 32 s.

Note: XML shows `a_env1_attack_shape value=1` etc. in factory patches. The `val_default` of EG release is -5 for env1 and 0 for env2, but the loaded init value is -5 for both.

### LFO shapes (`lfoN_shape`, `lfo_type`)

| idx | const | name | `deform_type` choices (`lt_num_deforms`) |
|---|---|---|---|
| 0 | lt_sine | Sine | 3 |
| 1 | lt_tri | Triangle | 3 |
| 2 | lt_square | Square | 0 |
| 3 | lt_ramp | Sawtooth | 3 |
| 4 | lt_noise | Noise | 0 |
| 5 | lt_snh | Sample & Hold | 2 |
| 6 | lt_envelope | Envelope | 3 |
| 7 | lt_stepseq | Step Sequencer | 2 |
| 8 | lt_mseg | MSEG | 0 |
| 9 | lt_formula | Formula | 3 |

surgepy exports every `lt_*` except `lt_stepseq`.

### Voice vs scene LFOs

XML index `lfoN`: **N = 0..5 are voice LFOs** (`ms_lfo1..6`, ids 17..22, one instance per voice, key-triggered by default), **N = 6..11 are scene LFOs** (`ms_slfo1..6`, ids 23..28, one instance per scene). In surgepy `patch['scene'][s]['lfo'][i]` uses the same 0..11 index.

Scene LFO parameters accept only scene-level modulation sources (`isValidModulation` rejects voice-level sources for `ctrlgroup_entry >= ms_slfo1`). Scene LFO params are not per-voice.

### LFO parameters (`{a,b}_lfoN_*`)

| name | ctrltype | stored range | init default | meaning |
|---|---|---|---|---|
| `shape` | int | 0..9 | 0 | |
| `rate` | ct_lforate_deactivatable | -7..9 | 0 | log2 Hz: f = 2^v Hz (0.0078 .. 512 Hz). `temposync="1"`: f = 2^v * (bpm/120) Hz. `deactivated` attr exists; Envelope-type LFOs force it inactive |
| `phase` | ct_lfophaseshuffle | 0..1 (extend_range: v -> 2v-1) | 0 | start phase; "Shuffle" for step seq with rate active |
| `magnitude` | ct_lfoamplitude | 0..1 (extend: 2v-1, allows inversion) | 1 | amplitude |
| `deform` | ct_lfodeform | -1..1 | 0 | meaning depends on shape; `deform_type` attr picks variant |
| `trigmode` | ct_lfotrigmode | int 0..2 | 1 | 0 Freerun, 1 Keytrigger, 2 Random phase |
| `unipolar` | bool | 0/1 | 0 | 1 = output 0..1 instead of -1..1 |
| `delay` | ct_envtime_deactivatable | -8..5 | -8 | |
| `attack` | ct_envtime | -8..5 | -8 | |
| `hold` | ct_envtime | -8..5 | -8 | |
| `decay` | ct_envtime | -8..5 | 0 | |
| `sustain` | ct_percent | 0..1 | 1 | |
| `release` | ct_envtime_lfodecay | -8..5 | 5 | max = "Forever" |

The DAHDSR group shapes any LFO output (defaults are transparent: instant attack, sustain 1). For `lt_envelope` the DAHDSR is the output.

Step sequencer data: `<stepsequences><sequence scene="0" i="6" s0=".." s1=".." ... loop_start="0" loop_end="15" shuffle="0" trigmask=".." trigmask_0to15=".." trigmask_16to31=".." trigmask_32to47=".."/>`. Steps s0..s15 are -1..1 (omitted when 0), only written for LFOs whose shape is lt_stepseq. MSEG: `<msegs><mseg scene i activeSegments endpointMode editMode loopMode loopStart loopEnd hSnap vSnap axisWidth axisStart><segments><segment duration v0 nv1 cpduration cpv type useDeform invertDeform retriggerFEG retriggerAEG/>...`. Formula: `<formulae><formula scene i formula="base64 Lua" interpreter="0"/>`. `extralfo/lfo extraAmplitude` is also saved.

## 6. Modulation and macros

Sources: `src/common/ModulationSource.h`, `SurgePatch.cpp` (save/load modrouting, customcontroller), `Parameter.cpp: get_modulation_f01`, `SurgeSynthesizer.cpp: setModDepth01, isValidModulation`.

### modsources enum (numeric ids used in XML `source=`)

| id | const | name | polarity | level |
|---|---|---|---|---|
| 0 | ms_original | (none) | | |
| 1 | ms_velocity | Velocity | uni 0..1 | voice |
| 2 | ms_keytrack | Keytrack | bi, (note - ktrkroot)/12 (1.0 per octave) | voice |
| 3 | ms_polyaftertouch | Poly AT | uni | voice |
| 4 | ms_aftertouch | Channel AT | uni | scene |
| 5 | ms_pitchbend | Pitch Bend | bi | scene |
| 6 | ms_modwheel | Mod Wheel | uni | scene |
| 7..14 | ms_ctrl1..ms_ctrl8 | Macro 1..8 | uni or bi (per macro) | scene |
| 15 | ms_ampeg | Amp EG | uni | voice |
| 16 | ms_filtereg | Filter EG | uni | voice |
| 17..22 | ms_lfo1..ms_lfo6 | Voice LFO 1..6 | bi (uni if `unipolar` or Envelope) | voice |
| 23..28 | ms_slfo1..ms_slfo6 | Scene LFO 1..6 | same | scene |
| 29 | ms_timbre | Timbre (MPE) | bi | voice |
| 30 | ms_releasevelocity | Release Velocity | uni | voice |
| 31 | ms_random_bipolar | Random | bi | voice |
| 32 | ms_random_unipolar | Rand Uni | uni | voice |
| 33 | ms_alternate_bipolar | Alternate | bi | voice |
| 34 | ms_alternate_unipolar | Alternate Uni | uni | voice |
| 35 | ms_breath | Breath | uni | scene |
| 36 | ms_expression | Expression | uni | scene |
| 37 | ms_sustain | Sustain Pedal | uni | scene |
| 38 | ms_lowest_key | Lowest Key | bi | scene |
| 39 | ms_highest_key | Highest Key | bi | scene |
| 40 | ms_latest_key | Latest Key | bi | scene |

Rules (`isValidModulation`): only float params are modulatable (int/bool/discrete are not); params with `per_voice_processing == false` (FX params, scene highpass `lowcut`, send levels, scene LFO params) accept only scene-level sources and channel aftertouch; an LFO's own parameters cannot be modulated by itself; EG params cannot be modulated by EGs; keytrack cannot modulate scene `pitch`. Changing an osc type clears that osc's modulation routings.

### Modulation routing in XML

Child elements of the target parameter element:

```xml
<a_filter1_cutoff type="2" value="-14.3">
  <modrouting source="16" depth="34.4" muted="0" source_index="0"/>
</a_filter1_cutoff>
```

- `depth` is in the **target parameter's stored (native) units**, not normalized. Cutoff: semitones. Percent params: fraction. Osc `param6`-style ints cannot be modulated.
- Effective value = base + sum(depth * source_value), clamped to the param range. Bipolar source swings +-depth, unipolar source 0..depth.
- Normalized API depth (surgepy `setModDepth01`) = depth / (val_max - val_min). Examples: cutoff range 130 -> 12 semitones = 0.0923; `envmod` range 192; ct_percent range 1 -> depth 0.5 = 0.5 -> 50 %; ct_lforate range 16.
- With `extend_range` the displayed depth is scaled by the extension factor, but the stored depth stays in the base range (`getModDepth` applies `get_extended`).
- `muted` 0/1, `source_index` int (for sources with sub-indices, 0 otherwise). For **global (scene-0) params** (all FX params, `volume`, `volume_FXn`, ...) add `source_scene="0|1"`: which scene the source belongs to. Scene params imply source scene = own scene.
- Voice-level and scene-level routings are separated internally by source type; the XML does not distinguish them.
- Revision < 9 patches shift source ids > 13 by +1 on load (macro 8 insertion). Always write revision 24 ids as above.

### Macros (`<customcontroller>`)

```xml
<customcontroller>
  <entry i="0" bipolar="0" v="0.00000000000000" label="-"/>   <!-- i = 0..7 = Macro 1..8 -->
  ...
</customcontroller>
```

`bipolar` 0/1, `v` initial value (0..1, or -1..1 if bipolar), `label` text shown in the UI (default `-`). Written once for scene 0 and shared by both scenes. A macro is a modulation source id 7..14 used in `<modrouting source="7" .../>`. Macros have no XML parameter element of their own.

Also stored: `<modwheel s0="0.0" s1="0.0"/>` (initial mod wheel per scene), `<lfobanklabels>` (UI only).

## 7. Effects

Sources: `src/common/SurgeStorage.h` (`fx_type`, `fxslot_*`), `src/common/dsp/effects/*.cpp|h`, `libs/sst/sst-effects/include/sst/effects/*.h`, `libs/airwindows`.

### Slots

16 slots. XML names `fxN_type`, `fxN_p0`..`fxN_p11` with **N = slot enum value + 1**. Slots whose type is Off are omitted from the XML entirely (type element included). Processing order (`fxslot_order`): A1..A4 on scene A, B1..B4 on scene B, then send S1..S4 (fed by each scene's `send_fx_N`, returned by `volume_FXn`), then global G1..G4.

| slot enum | XML N | name | chain | processing order |
|---|---|---|---|---|
| 0 | fx1 | A1 (A Insert FX 1) | scene A | 1 |
| 1 | fx2 | A2 | scene A | 2 |
| 2 | fx3 | B1 | scene B | 5 |
| 3 | fx4 | B2 | scene B | 6 |
| 4 | fx5 | S1 (Send FX 1) | send | 9 |
| 5 | fx6 | S2 | send | 10 |
| 6 | fx7 | G1 (Global FX 1) | global | 13 |
| 7 | fx8 | G2 | global | 14 |
| 8 | fx9 | A3 | scene A | 3 |
| 9 | fx10 | A4 | scene A | 4 |
| 10 | fx11 | B3 | scene B | 7 |
| 11 | fx12 | B4 | scene B | 8 |
| 12 | fx13 | S3 | send | 11 |
| 13 | fx14 | S4 | send | 12 |
| 14 | fx15 | G3 | global | 15 |
| 15 | fx16 | G4 | global | 16 |

surgepy: `patch['fx'][i]` uses the slot enum index; constants only exist for `fxslot_ains1..global2` (0..7).

### fxt_* enum

| idx | const | UI name | idx | const | UI name |
|---|---|---|---|---|---|
| 0 | fxt_off | Off | 15 | fxt_neuron | Neuron |
| 1 | fxt_delay | Delay | 16 | fxt_geq11 | Graphic EQ |
| 2 | fxt_reverb | Reverb 1 | 17 | fxt_resonator | Resonator |
| 3 | fxt_phaser | Phaser | 18 | fxt_chow | CHOW |
| 4 | fxt_rotaryspeaker | Rotary Speaker | 19 | fxt_exciter | Exciter |
| 5 | fxt_distortion | Distortion | 20 | fxt_ensemble | Ensemble |
| 6 | fxt_eq | EQ (3-band parametric) | 21 | fxt_combulator | Combulator |
| 7 | fxt_freqshift | Frequency Shifter | 22 | fxt_nimbus | Nimbus |
| 8 | fxt_conditioner | Conditioner | 23 | fxt_tape | Tape |
| 9 | fxt_chorus4 | Chorus (4 voice) | 24 | fxt_treemonster | Treemonster |
| 10 | fxt_vocoder | Vocoder | 25 | fxt_waveshaper | Waveshaper |
| 11 | fxt_reverb2 | Reverb 2 | 26 | fxt_mstool | Mid-Side Tool |
| 12 | fxt_flanger | Flanger | 27 | fxt_spring_reverb | Spring Reverb |
| 13 | fxt_ringmod | Ring Modulator | 28 | fxt_bonsai | Bonsai |
| 14 | fxt_airwindows | Airwindows | 29 | fxt_audio_input | Audio Input |

surgepy only exports `fxt_off..fxt_neuron` (0..15); use integers for the rest.

### FX parameter storage

Params are positional: `fxN_p0`..`fxN_p11` (12 floats; ints for choice params, written `type="0"`). Meaning depends on `fxN_type`. Unused trailing params are `ct_none` ints. Per-param XML attributes: `temposync`, `extend_range`, `deactivated`, `deform_type`, `absolute`.

**XML-load defaults:** when `deactivated` is absent on a deactivatable param (FX low/high cut, EQ gains, Combulator tone, delay right time, ...) the loader sets it **deactivated = true** (`SurgePatch.cpp` ~1650). Write `deactivated="0"` to enable a cut filter. When `extend_range` is absent it is set to 0.

In surgepy, setting `fxN_type` queues a reload: call `process()` before setting `p0..p11` (sections 8, 10), and the new type loads its own defaults.

Shorthand below: `freqA` = ct_freq_audible family, Hz = 440*2^(v/12), stored -60..70; `cut_hp`/`cut_lp` = deactivatable freq (default -60 / 70, normally deactivated); `dBn` = ct_decibel_narrow +-24 dB; `pct` = 0..1 shown as %; `pctB` = -1..1; `env` = ct_envtime log2 s (-8..5). See section 9 for all ctrltypes.

### Tabulated effects (most useful for leads and basses)

Distortion (5), `dist_params`:

| p | name | type | range / unit | default |
|---|---|---|---|---|
| 0 | Pre-EQ Gain | ct_decibel_extendable | +-48 dB (extend x3) | 0 |
| 1 | Pre-EQ Frequency | freqA | -60..70 | 0 |
| 2 | Pre-EQ Bandwidth | ct_bandwidth | 0..5 octaves | 2 |
| 3 | Pre-EQ High Cut | cut_lp | -60..70 | active |
| 4 | Drive | ct_decibel_narrow_extendable | +-24 dB (x5) | 0 |
| 5 | Feedback | percent_bipolar | -1..1 | 0 |
| 6 | Post-EQ Gain | decibel_extendable | dB | 0 |
| 7 | Post-EQ Frequency | freqA | | 0 |
| 8 | Post-EQ Bandwidth | bandwidth | | 2 |
| 9 | Post-EQ High Cut | cut_lp | | active |
| 10 | Gain | dBn | | 0 |
| 11 | Model | ct_distortion_waveshape | int 0..7 -> ws index: 0 Soft(1), 1 Hard(2), 2 Asymmetric(3), 3 Sine(4), 4 Digital(5), 5 OJD(41), 6 Full Wave(10), 7 Fuzz Soft Clip(25) | 0 |

Waveshaper (25), `wsfx_params`:

| p | name | type | range / unit | default |
|---|---|---|---|---|
| 0 / 1 | Pre Low Cut / High Cut | cut_hp / cut_lp | | deactivated |
| 2 | Shape | ct_wstype | int 0..42 (section 4) | 0 |
| 3 | Bias | percent_bipolar | -1..1 | 0 |
| 4 | Drive | ct_decibel_narrow_short_extendable | +-24 dB (x2) | 0 |
| 5 / 6 | Post Low Cut / High Cut | cut_hp / cut_lp | | active |
| 7 | Gain | decibel_narrow_short_extendable | dB | 0 |
| 8 | Mix | percent | | 1 |

EQ 3-band (6), `eq3_params`:

| p | name | type | range / unit | default |
|---|---|---|---|---|
| 0, 3, 6 | Gain 1/2/3 | ct_decibel_deactivatable | +-48 dB | 0 (active) |
| 1, 4, 7 | Frequency 1/2/3 | freqA | semitones from 440 Hz | -30 (78 Hz), 6 (622 Hz), 54 (9.9 kHz) |
| 2, 5, 8 | Bandwidth 1/2/3 | ct_bandwidth | 0..5 octaves | 2 |
| 9 | Gain | ct_decibel | +-48 dB | 0 |
| 10 | Mix | percent_bipolar | -1..1 | 1 |

Delay (1), `delay_params` (sst-effects enum; p9 is a reserved int):

| p | name | type | range / unit | default |
|---|---|---|---|---|
| 0 | Left time | ct_envtime | log2 s | -2 |
| 1 | Right time | ct_envtime_linkable_delay | log2 s; `deactivated=1` = linked to left | -2, active |
| 2 | Feedback | ct_dly_fb_clippingmodes | 0..1 (extend_range: -1..1); `deform_type` 0 off, 1 soft cubic, 2 tanh, 3 hard 0 dBFS, 4 hard +18 dBFS | 0.5; deform 1 |
| 3 | Crossfeed | percent_with_extend_to_bipolar | 0..1 (extend: -1..1) | 0 |
| 4 / 5 | Low Cut / High Cut | cut_hp / cut_lp | | -24 / 30 (active) |
| 6 | Mod Rate | ct_lforate | log2 Hz | -2 |
| 7 | Mod Depth | ct_detuning | 0..2, cents = 100*v (extend x6) | 0 |
| 8 | Input Channel | percent_bipolar_stereo | -1 left .. 1 right | 0 |
| 10 | Mix | pct | | 0.5 |
| 11 | Width | dBn | | 0 |

Reverb 2 (11), `rev2_params`:

| p | name | type | range / unit | default |
|---|---|---|---|---|
| 0 | Pre-Delay | ct_reverbpredelaytime | -8..1 log2 s | -4 |
| 1 | Room Size | percent_bipolar | -1..1 | 0 |
| 2 | Decay Time | ct_reverbtime | -4..6, t = 2^v s | 0.75 |
| 3 / 4 / 5 | Diffusion / Buildup / Modulation | pct | | 1 / 1 / 0.5 |
| 6 / 7 | LF Damping / HF Damping | pct | | 0.2 / 0.2 |
| 8 | Width | dBn | | 0 |
| 9 | Mix | pct | | 0.33 |

Chorus (9), 4-voice, `chorus_params`: p0 Time (ct_chorusmodtime, -11..-3 log2 s, -6), p1 Rate (ct_lforate, -2), p2 Depth (pct, 0.3), p3 Feedback (pct, 0.5), p4 Low Cut (cut_hp, -36 active), p5 High Cut (cut_lp, 36 active), p6 Mix (pct, 1), p7 Width (dBn, 0).

### Other effects (compact: `pN` name (type, range, default))

- **Reverb 1 (2)**: p0 Pre-Delay (ct_envtime, -4), p1 Shape (int 0..3 = Type 1..4, 0), p2 Room Size (pct, 0.5), p3 Decay Time (ct_reverbtime -4..6, 2^v s, 1), p4 Damping (pct, 0.2), p5 Low Cut (cut_hp, -24 active), p6 EQ Frequency (freqA, 0), p7 EQ Gain (ct_decibel, 0), p8 High Cut (cut_lp, 72 active), p9 Mix (pct, 0.5), p10 Width (dBn, 0).
- **Phaser (3)** (sst-effects order, not UI order): p0 Center (pctB), p1 Feedback (pctB), p2 Sharpness (pctB), p3 Rate (ct_lforate_deactivatable, log2 Hz), p4 Depth (pct), p5 Stereo (pct), p6 Mix (pct), p7 Width (dBn), p8 Count (int 1..16, 1 = legacy 4 stages), p9 Spread (pct, inactive when Count = 1), p10 Waveform (ct_fxlfowave int 0..5 = Sine, Triangle, Sawtooth, Noise, S&H, Square), p11 Tone (pctB deactivatable).
- **Rotary Speaker (4)**: p0 Horn Rate (ct_lforate), p1 Doppler (pct), p2 Tremolo (pct), p3 Rotor Rate (ct_percent200 0..2, 0.7), p4 Drive (ct_rotarydrive 0..1), p5 Model (0..7 same map as Distortion), p6 Width (dBn), p7 Mix (pct).
- **Frequency Shifter (7)**: p0 Left shift (ct_freq_shift, stored -10..10, Hz = 10*v, extend_range Hz = 1000*v), p1 Right (pctB, ratio of left), p2 Time (ct_envtime), p3 Feedback (pct), p4 Mix (pct).
- **Conditioner (8)**: p0 Bass, p1 Treble (ct_decibel_extra_narrow_deactivatable, +-12 dB), p2 Width (pctB), p3 Balance (pctB), p4 Threshold (ct_decibel_attenuation -48..0), p5 Attack Rate (pctB), p6 Release Rate (pctB), p7 Gain (ct_decibel_attenuation), p8 Side Low Cut (cut_hp).
- **Vocoder (10)**: p0 Gain (dB), p1 Gate (ct_decibel_attenuation_large), p2 Env Follow (pct), p3 Q (pctB), p4 Shift, p5 Bands (int 4..20), p6 Min Freq (-36..36), p7 Max Freq (0..60), p8 Modulator input (int 0 Monosum, 1 Left, 2 Right, 3 Stereo), p9 Range, p10 Center, p11 Mix. Needs a modulator input.
- **Ring Modulator (13)**: p0 Carrier Shape (ct_ringmod_sineoscmode int 0..28, 28 = audio in), p1 Carrier Frequency (ct_freq_ringmod 0..127, Hz = 8.175798*2^(v/12), 60 = 261.6 Hz), p2 Unison Detune (0..1), p3 Unison Voices (1..16), p4 Diode Forward Bias (pct), p5 Diode Linear Region (pct), p6 Low Cut (cut_hp), p7 High Cut (cut_lp), p8 Mix (pct).
- **Airwindows (14)**: p0 effect id (int 0..76), p1..p11 up to 11 raw params stored 0..1 (`ct_airwindows_param`; integral params are ints). Ids in `libs/airwindows/src/AirWinBaseClass_pluginRegistry.cpp` (append-only): 0 AD Clip, 1 Block Party, 2 Butter Comp, 3 Compresaturator, 4 Logical, 5 Mojo, 6 One Corner Clip, 7 Point, 8 Pop, 9 Pressure, 10 Pye Wacket, 11 Surge, 12 Vari-Mu, 13 Bit Glitter, 14 Crunchy Groove Wear, 15 DeRez, 16 Deck Wrecka, 17 Dust Bunny, 18 Groove Wear, 19 Noise, 20 Voice Of The Starship, 21 Bright Ambience, 22 Hombre, 23 Melt, 24 Pocket Verbs, 25 Star Child, 26 Apicolypse, 27 Bass Drive, 28 Cojones, 29 Density, 30 Drive, 31 Focus, 32 Fracture, 33 Hard Vacuum, 34 Loud, 35 NC-17, 36 Spiral, 37 Unbox, 38 De-Bess, 39 (NoOp), 40 Single-Ended Triode, 41 Iron Oxide, 42 To Tape, 43 Air, 44 Buss Colors, 45 Drum Slam, 46 Capacitor, 47 Slew 1, 48 Slew 2, 49 Galactic, 50 Infinity, 51 MatrixVerb, 52 Verbity, 53 Triple Spread, 54 Chamber, 55 Mackity, 56 MackEQ, 57 (NoOp), 58 Cabs, 59 Chrome Oxide, 60 Dub Sub, 61 Dub Center, 62 Fire Amp, 63 Glitch Shifter, 64 Nonlinear Space, 65 Pafnuty, 66 Power Sag, 67 Tape Dust, 68 To Vinyl, 69 YLowpass, 70 YBandpass, 71 YHighpass, 72 YNotch, 73 ZLowpass, 74 ZBandpass, 75 ZHighpass, 76 ZNotch. Per-effect param names: read `getParameterName` after setting p0 and calling `process()`.
- **Neuron (15)**: p0 Drive (dBn, 0), p1 Squash (pct, 0.5), p2 Stab (pct, 0.5), p3 Asymmetry (pct, 1), p4 Bias (pct, 0), p5 Comb Frequency (freqA, 70), p6 Comb Separation (ct_freq_mod +-96 st, 0), p7 LFO Waveform (ct_fxlfowave int 0..5), p8 LFO Rate (ct_lforate_deactivatable), p9 LFO Depth (pct), p10 Width (dBn), p11 Gain (dBn).
- **Graphic EQ (16)**: p0..p10 = 30, 60, 120, 250, 500, 1k, 2k, 4k, 8k, 12k, 16k Hz (ct_decibel_narrow_deactivatable, +-24 dB), p11 Gain (ct_decibel).
- **Resonator (17)**: three bands, each (Frequency, Resonance, Gain) at p0-2, p3-5, p6-8. Frequency ranges: band 1 -34.49..-6.63 (60..300 Hz, default -18.63), band 2 -6.63..21.23 (300..1500 Hz, 8.04), band 3 21.23..49.10 (1.5..7.5 kHz, 35.9). Resonance pct (0.75). Gain ct_amplitude (0.7937 = -6 dB). p9 Mode (int 0 Lowpass, 1 Bandpass, 2 Bandpass+Notch, 3 Highpass; default 1), p10 Gain (ct_decibel), p11 Mix (pct, 1).
- **CHOW (18)**: p0 Threshold (ct_decibel_attenuation_large, -96..0 dB), p1 Ratio (ct_chow_ratio 1..20, default 10), p2 Flip (bool), p3 Mix (pct, 1).
- **Exciter (19)**: p0 Drive (pct, 0.5), p1 Tone (pct, 0.5), p2 Attack (ct_comp_attack_ms 0..1, ms = 100^v, 0.5), p3 Release (ct_comp_release_ms 0..1, ms = 10*100^v, 0.5), p4 Mix (pct, 0.5).
- **Ensemble (20)**: p0 Anti-Alias Filter (freqA), p1 LFO Freq 1 (ct_ensemble_lforate, Hz = 2^v, range log2(0.01)..log2(20)), p2 Depth 1 (pct), p3 LFO Freq 2, p4 Depth 2, p5 Delay Type (int 0 Digital, 1..6 BBD 128/256/512/1024/2048/4096 stages), p6 Clock Rate (ct_ensemble_clockrate 1.5..100 kHz, 40), p7 Saturation (pct, 0), p8 Feedback (pct, 0), p9 Width (pctB, 1), p10 Mix (pct, 1), p11 Reconstruction Filter (freq_audible_deactivatable).
- **Combulator (21)**: p0 Extra Noise (pct, 0), p1 Center (ct_freq_audible_very_low_minval -117.4..70 = 0.5 Hz..25 kHz, default -9), p2 / p3 Comb 2 / 3 offset (ct_pitch_extendable_very_low_minval, semitones from Center, 0.25 / -0.25), p4 Feedback (pctB, 0.75), p5 Tone (pctB deactivatable, 0), p6-p8 Comb 1/2/3 level (ct_amplitude, 0.7937), p9 / p10 Pan 2 / 3 (percent_bipolar_pan, 0.25 / -0.25), p11 Mix (pct, 1).
- **Nimbus (22)**: p0 Mode (int 0 Granularizer, 1 Pitch Shifter, 2 Looping Delay, 3 Spectral Madness), p1 Quality (int 0..3: 32k 16-bit stereo, 32k mono, 16k 8-bit stereo, 16k mono), p2 Position (pct), p3 Size (-1..1 stored, default 0.5), p4 Pitch (ct_pitch4oct +-48 st), p5 Density (-1..1), p6 Texture (-1..1), p7 Spread (pct), p8 Freeze (ct_float_toggle 0..1), p9 Feedback (pct), p10 Reverb (pct), p11 Mix (pct, 0.5). Needs input audio.
- **Tape (23)**: p0 Drive (ct_tape_drive 0..1, 0.85; `deform_type` 0..3 = Normal, Medium, High, Very High precision), p1 Saturation (pct, 0.5), p2 Bias (pct, 0.5), p3 Tone (pctB), p4 Speed (ct_tape_speed 1..50 ips, 30), p5 Gap (ct_tape_microns 0..500 um, 10), p6 Spacing (um, 0.1), p7 Thickness (um, 0.1), p8 Degrade Depth (percent_deactivatable, deactivated), p9 Degrade Amount (pct), p10 Degrade Variance (pct), p11 Mix (pct, 1).
- **Treemonster (24)**: p0 Threshold (ct_decibel_attenuation_large, -24), p1 Speed (pct, 0.5), p2 Low Cut (cut_hp), p3 High Cut (cut_lp), p4 Pitch (ct_pitch +-60 st), p5 Ring Modulation (pct, 0.5), p6 Width (pctB), p7 Mix (pct, 1).
- **Mid-Side Tool (26)**: p0 Matrix (ct_mscodec int 0 "L-R > M-S > L-R", 1 "L-R > M-S", 2 "M-S > L-R"), p1-p4 Mid (Low Cut, Gain ct_decibel_narrow_deactivatable, Frequency freqA, High Cut), p5-p8 Side (same four), p9 Mid Gain, p10 Side Gain (ct_decibel_attenuation_plus12, -48..12), p11 Balance (percent_bipolar_stereo).
- **Spring Reverb (27)**: p0 Size (pct, 0.5), p1 Decay (ct_spring_decay 0..1, t = 0.5*9^v s, 0.5), p2 Reflections (pct, 1), p3 HF Damping (pct, 0.5), p4 Spin (pct, 0.5), p5 Chaos (pct, 0), p6 Knock (ct_float_toggle), p7 Mix (pct, 0.5).
- **Bonsai (28)**: p0 Input Gain (dBn), p1 Bass Boost Amount (ct_bonsai_bass_boost +-24 dB, extend x3, 0), p2 Bass Distort (pct, 0.25), p3 Bias Filter (int 0 Tilt, 1 Pull Mids), p4 Saturation Type (int 0 Gentle, 1 Normal, 2 Hard, 3 Harsh; ctrltype default 1, sst metadata default 0), p5 Saturation Amount (pct, 0.25), p6 Noise Sensitivity (pct, 0.25), p7 Noise Gain (dBn, -12), p8 Dull (pct, 0), p9 Output Gain (dBn), p10 Mix (pct, 1).
- **Flanger (12)**: p0 Mode (int 0 Dry + Combs, 1 Combs Only, 2 Dry + Arp Combs, 3 Arp Combs Only), p1 Waveform (int 0..5), p2 Rate (ct_lforate, -2), p3 Depth (pct, 1), p4 Count (float 1..4, 4), p5 Base Pitch (float 0..127 MIDI note, 60), p6 Spacing (float 0..12 st, 0), p7 Feedback (pct, 0), p8 HF Damping (pct, 0.1), p9 Width (dBn), p10 Mix (percent_bipolar, 0.8).


## 8. surgepy API

Source: `src/surge-python/surgepy.cpp`, `surgepy/__init__.pyi`, `tests/test_surgepy.py`.

**The nix package `surge-xt-1.3.4` does not contain surgepy.** It ships only `bin/{Surge XT,surge-xt-cli,Surge XT Effects}` and clap/lv2/vst3. surgepy must be built from `$SRC` with `-DSURGE_BUILD_PYTHON_BINDINGS=ON` (`src/CMakeLists.txt:4`), and it locates factory data (wavetables, skins) through the install data path (`getFactoryDataPath()`).

### Module

| call | notes |
|---|---|
| `surgepy.createSurge(sampleRate)` | returns SurgeSynthesizer; sets tempo 120, ppq 0; block size = 32 |
| `surgepy.getVersion()` | |
| `surgepy.constants.*` | cg_*, ms_*, ot_* (classic..window only), adsr_*, fxt_* (off..neuron only), fxslot_* (ains1..global2 only), pm_*, sm_*, fc_*, fm_*, lt_* (no stepseq), `WaveshaperType.wst_none..wst_digital`, `FilterType.fut_*` (all) |

### SurgeSynthesizer methods

| method | behavior |
|---|---|
| `getPatch()` | dict: `scene[0..1]` {`osc[0..2]` {type, pitch, octave, keytrack, retrigger, `p[0..6]`}, `filterunit[0..1]` {type, subtype, cutoff, resonance, envmod, keytrack}, `wsunit` {type, drive}, `adsr[0..1]` {a, d, s, r, a_s, d_s, r_s, mode}, `lfo[0..11]` {rate, shape, start_phase, magnitude, deform, trigmode, unipolar, delay, hold, attack, decay, sustain, release}, pitch, octave, fm_depth, fm_switch, drift, noise_colour, keytrack_root, level_/mute_/solo_/route_ for o1, o2, o3, ring_12, ring_23, noise, level_pfg, vca_level, pbrange_dn/up, vca_velsense, polymode, portamento, volume, pan, width, `send_level[0..1]` (only 2 of 4), f2_cutoff_is_offset, f2_link_resonance, feedback, filterblock_configuration, filter_balance, lowcut}, `fx[0..15]` {type, return_level, `p[0..11]`}, plus scene_active, scenemode, splitpoint, volume, polylimit, fx_bypass, fx_disable, character. Every leaf is a `SurgeNamedParamId`. Not present: `send_fx_3/4`, `volume_FX1..4` (only via `fx[i]['return_level']`, valid for send slots 4,5,12,13), `ktrkroot` is `keytrack_root` |
| `getControlGroup(cg)` | `.getEntries()` -> entries with `.getEntry()`, `.getScene()` (0 global, 1 A, 2 B), `.getParams()` -> `SurgeNamedParamId` with `.getName()` (full UI name like "A Osc 1 Shape") |
| `getParamVal / getParamMin / getParamMax / getParamDef(p)` | float in **stored units** (ints returned as floats) |
| `getParamValType(p)` | "float" / "int" / "bool" |
| `getParamDisplay(p)`, `getParamInfo(p)` | UI string with units (use to confirm conversions) |
| `setParamVal(p, v)` | **stored units**. Internally `setParameter01(id, value_to_normalized(v))`; int params round, bool > 0.5 = true. Ranges are the param's *current* min/max (extend_range changes them) |
| `getParameterName(id)` | |
| `getModSource(ms)` | wraps a `surgepy.constants.ms_*` int; valid for velocity, releasevelocity, keytrack, lowest/highest/latest key, poly/channel AT, modwheel, breath, expression, sustain, pitchbend, timbre, alternate/random, EGs, LFO/SLFO 1-6, ctrl1-8 |
| `setModDepth01(target, source, depth, scene=0, index=0)` | **normalized** depth: fraction of (val_max - val_min), e.g. cutoff 0.1 = 13 semitones. `scene` = source scene for global params (0 = A, 1 = B); ignored for scene params |
| `getModDepth01 / isValidModulation / isActiveModulation / isBipolarModulation / getAllModRoutings` | query |
| `playNote(ch, note, vel, detune=0)`, `releaseNote(ch, note, relVel=0)`, `pitchBend`, `polyAftertouch`, `channelAftertouch`, `channelController(ch, cc, val)`, `allNotesOff()` | |
| `createMultiBlock(nBlocks)` | float32 array shape (2, nBlocks*32), zeroed |
| `processMultiBlock(arr, startBlock=0, nBlocks=-1)` | renders; advances ppq. `process()` + `getOutput()` for one block |
| `processMultiBlockWithInput(inArr, outArr, ...)` | audio-in effects |
| `loadPatch(path)` | loads `.fxp` (raises if missing); patch name = file name |
| `savePatch(path)` | writes `.fxp` (60-byte header + `sub3` payload) |
| `loadSCLFile / loadKBMFile / retuneToStandardTuning / retuneToStandardScale / remapToStandardKeyboard`, `mpeEnabled`, `tuningApplicationMode` | tuning |

Typical flow:

```python
import surgepy, numpy as np
s = surgepy.createSurge(48000)
p = s.getPatch()
a = p['scene'][0]
s.setParamVal(a['osc'][0]['type'], 0)            # classic (osc type changes apply on the next process(), see G1)
s.setParamVal(a['filterunit'][0]['type'], 2)     # LP 24 dB (set type first)
s.setParamVal(a['filterunit'][0]['subtype'], 1)
s.setParamVal(a['filterunit'][0]['cutoff'], -12) # semitones from 440 Hz
s.setModDepth01(a['filterunit'][0]['cutoff'], s.getModSource(surgepy.constants.ms_modwheel), 0.25)  # 0.25 * 130 = 32.5 semitones
s.setParamVal(a['filterunit'][0]['envmod'], 24)
buf = s.createMultiBlock(int(2*48000/s.getBlockSize()))
s.playNote(0, 48, 100); s.processMultiBlock(buf)
s.savePatch('/tmp/x.fxp')
```

### Gotchas (all verified in source)

| # | issue | source |
|---|---|---|
| G1 | **Osc type change is queued** (`queue_type`) and applied inside `process()` (`loadOscalgos`). Applying it resets all 7 osc params to the new type's defaults and clears that osc's modulation. Order: set type -> `process()` once -> set osc params -> set mod routings | `SurgeSynthesizer.cpp:2814, 3189` |
| G2 | **FX type change is queued** (`load_fx_needed`), applied in `process()`; before that, the slot's `p[i]` still have the old type's ranges (or `ct_none` int ranges), so `setParamVal` on them mis-scales. Order: set `fx[i]['type']` -> `process()` -> set `p` | `SurgeSynthesizer.cpp:2863, 2991` |
| G3 | Filter `type` change is immediate but also overwrites `subtype` from per-type memory. Set `type` before `subtype` | `SurgeSynthesizer.cpp:2798` |
| G4 | `setParamVal` on `f2_cutoff_is_offset` mutates filter-2 cutoff/envmod/keytrack | `SurgeSynthesizer.cpp` (ct_bool_relative_switch case) |
| G5 | No setter for per-param flags: `extend_range`, `temposync`, `absolute`, `deactivated`, `deform_type`, portamento options. Effects: FX cut filters, Sine/S&H/Window osc low/high cuts, Delay right-time link, LFO rate deactivation, unison/FM3 `absolute`, scene `lowcut` slope, ring-mod combinator mode, noise mono/stereo, String/Tape/Delay `deform_type` stay at their init state | `surgepy.cpp` has no such binding |
| G6 | No macro API: cannot set macro value, bipolar flag, or label. Macros stay 0 (routing from `ms_ctrl1..8` works but contributes nothing) unless the XML is edited | `getModSource` only; `customcontroller` not exposed |
| G7 | No wavetable API. A Wavetable/Window osc uses whatever table is loaded (first factory table on a fresh synth). Embed tables by editing the fxp (section 10) | `perform_queued_wtloads` not exposed |
| G8 | No step sequencer / MSEG / Formula editing | |
| G9 | `getPatch()['scene'][s]['send_level']` has only the first 2 sends | `surgepy.cpp` `makeScene` |
| G10 | Missing constants (use ints): ot 8..11, fxt 16..29, fxslot 8..15, lt_stepseq, wst 6..42 | |
| G11 | `loadPatch` on a patch with different streaming revision re-maps legacy values; for authored XML write `revision="24"` | `SurgePatch.cpp:1203` |
| G12 | `createSurge` loads the template patch named by user default `InitialPatchName` (default "Init Saw" in category Templates) when the factory patch list is found (`SurgeSynthesizer.cpp` ctor). Init Saw (rev 22): classic saw on osc 1, filters Off, `fb_config` 7 (Wide), osc2/osc3/noise/rings muted, amp EG attack -8 decay -2 sustain 1 release -5, scene volume 0.8909. Other templates: Init Sine, Square, Wavetable, FM2, Modern, Karplus-Strong, Duophonic, Paraphonic, Env Per Osc, Experimental | `patches_factory/Templates/` |

## 9. Value conversion formulas

Source: `src/common/Parameter.cpp: set_type (ranges, display info), get_display, get_extended, value_to_normalized`; `src/common/dsp/utilities/DSPUtils.h` (amp_to_db).

`normalized = (v - min)/(max - min)` for floats, ints likewise, bool `v > 0.5`. Modulation depth01 = depth / (max - min).

### Core formulas

| quantity | stored -> display | display -> stored |
|---|---|---|
| frequency (ATwoToTheBx, a = 440, b = 1/12) | Hz = 440 * 2^(v/12) | v = 12 * log2(Hz/440) |
| ring mod / MIDI-note frequency | Hz = 8.175798 * 2^(v/12) | v = 12*log2(Hz/8.175798) |
| time (ct_envtime etc., a=1, b=1) | s = 2^v (shown ms if < 1 s) | v = log2(seconds) |
| LFO / FX rate (ct_lforate, a=1, b=1) | Hz = 2^v | v = log2(Hz) |
| amplitude (ct_amplitude*) | dB = 18*log2(v); linear gain = v^3 | v = 2^(dB/18) |
| decibel types | stored dB directly | |
| percent types | display % = 100*v (bipolar: -100..100) | v = %/100 |
| semitones (pitch types, freq_mod) | stored = semitones | |
| detune (oscspread) | cents = 100*v (extend x12) | v = cents/100 |
| detuning (delay mod depth) | cents = 100*v (extend x6) | |
| syncpitch | semitones above osc pitch, 0..60, > 0 enables | |
| keytrack source | (key - ktrkroot)/12 octaves | |
| tempo sync | rate params: Hz = 2^v * (bpm/120); time params: s = 2^v * (120/bpm) | value is snapped so frac(v) is 0, log2(4/3) (triplet) or log2(3/2) (dotted) |

Reference values: 55 Hz = -36, 110 Hz = -24, 220 Hz = -12, 261.6 Hz = -9, 440 Hz = 0, 523.3 Hz = 3 (default cutoff), 880 Hz = 12, 1 kHz = 14.21, 2 kHz = 26.21, 5 kHz = 42.08, 10 kHz = 54.08, 20 kHz = 66.08, range ends 13.75 Hz (-60) and 25088 Hz (70). 100 Hz = -25.65, 200 Hz = -13.65, 500 Hz = 2.23.

dB amplitude table: 0 dB = 1.000, -1.5 dB = 0.944, -3 dB = 0.891, -6 dB = 0.794, -12 dB = 0.630, -18 dB = 0.500, -24 dB = 0.397, -36 dB = 0.250, -48 dB = 0.157.

### ctrltype dictionary (stored ranges)

| ctrltype | type | min | max | default | notes |
|---|---|---|---|---|---|
| ct_percent | f | 0 | 1 | 0 | |
| ct_percent_bipolar (+ _pan, _stereo, _stringbal, _with_string_filter_hook, _deactivatable, _w_dynamic_unipolar_formatting) | f | -1 | 1 | 0 | |
| ct_percent200 | f | 0 | 2 | 0 | |
| ct_percent_with_extend_to_bipolar, ct_dly_fb_clippingmodes | f | 0 | 1 | 0 | extend_range: v -> 2v-1 |
| ct_amplitude, _clipper, _ringmod, ct_lfoamplitude | f | 0 | 1 | 1 | |
| ct_sendlevel | f | 0 | 1.5874 | 0 | +12 dB max |
| ct_decibel (+ _extendable, _deactivatable) | f | -48 | 48 | 0 | extendable x3 |
| ct_decibel_narrow (+ _deactivatable) | f | -24 | 24 | 0 | |
| ct_decibel_narrow_extendable / _short_extendable | f | -24 | 24 | 0 | extend x5 / x2 |
| ct_decibel_extra_narrow (+ _deactivatable) | f | -12 | 12 | 0 | |
| ct_decibel_attenuation, _clipper | f | -48 | 0 | 0 | |
| ct_decibel_attenuation_large | f | -96 | 0 | 0 | |
| ct_decibel_attenuation_plus12 | f | -48 | 12 | 0 | |
| ct_decibel_fmdepth | f | -48 | 24 | 0 | |
| ct_freq_audible (+ _deactivatable, _fm3_extendable, _with_tunability) | f | -60 | 70 | 3 | |
| ct_freq_audible_deactivatable_hp / _lp | f | -60 | 70 | -60 / 70 | |
| ct_freq_audible_very_low_minval | f | -117.3763 | 70 | 3 | 0.5 Hz min |
| ct_freq_hpf | f | -72 | 15 | -72 | |
| ct_freq_mod | f | -96 | 96 | 0 | semitone offset |
| ct_freq_ringmod | f | 0 | 127 | 60 | MIDI-note scale |
| ct_freq_fm2_offset | f | -10 | 10 | 0 | Hz (extend x100) |
| ct_freq_shift | f | -10 | 10 | 0 | Hz = 10*v (extend: 1000*v) |
| ct_pitch (+ _extendable_very_low_minval) | f | -60 | 60 | 0 | semitones |
| ct_pitch4oct | f | -48 | 48 | 0 | |
| ct_pitch_semi7bp (+ _absolutable) | f | -7 | 7 | 0 | extend x12; absolutable: Hz x10 |
| ct_syncpitch | f | 0 | 60 | 0 | |
| ct_pitch_octave | i | -3 | 3 | 0 | |
| ct_fmratio | f | 0 | 32 | 1 | extend: see FM3 |
| ct_fmratio_int | i | 1 | 32 | 1 | |
| ct_oscspread | f | 0 | 1 | 0.1 | cents = 100*v |
| ct_oscspread_bipolar | f | -1 | 1 | 0 | |
| ct_osccount | i | 1 | 16 | 1 | |
| ct_detuning | f | 0 | 2 | 0 | cents = 100*v |
| ct_bandwidth | f | 0 | 5 | 1 | octaves |
| ct_envtime (+ _deformable, _deactivatable, _linkable_delay, _lfodecay) | f | -8 | 5 | 0 | 2^v s |
| ct_portatime | f | -8 | 2 | -8 | |
| ct_lforate (+ _deactivatable) | f | -7 | 9 | 0 | 2^v Hz |
| ct_ensemble_lforate | f | log2(0.01) | log2(20) | 0 | |
| ct_ensemble_clockrate | f | 1.5 | 100 | 40 | kHz |
| ct_delaymodtime, ct_chorusmodtime | f | -11 | -3 | -6 | |
| ct_reverbtime | f | -4 | 6 | 1 | |
| ct_reverbpredelaytime | f | -8 | 1 | -2 | |
| ct_lfodeform | f | -1 | 1 | 0 | |
| ct_lfophaseshuffle | f | 0 | 1 | 0 | extend -1..1 |
| ct_osc_feedback (0..1), ct_osc_feedback_negative, ct_filter_feedback | f | 0 or -1 | 1 | 0 | extend x8 / x4 / x4 |
| ct_envshape(_attack), ct_oscroute | i | 0 | 2 | 0 / 1 (route) | |
| ct_envmode, ct_character, ct_scenesel, ct_sinefmlegacy | i | 0 | 1 / 2 / 1 / 1 | | |
| ct_lfotype / ct_lfotrigmode | i | 0 | 9 / 2 | 0 | |
| ct_wstype / ct_filtertype / ct_filtersubtype | i | 0 | 42 / 33 / 15 | 0 | |
| ct_fbconfig / ct_fmconfig / ct_osctype / ct_fxtype / ct_fxbypass | i | 0 | 7 / 3 / 11 / 29 / 3 | 0 | |
| ct_polymode / ct_scenemode / ct_polylimit / ct_midikey / ct_pbdepth | i | 0 / 0 / 2 / 0 / 0 | 5 / 3 / 64 / 127 / 24 | 0 / 0 / 16 / 60 / 2 | |
| ct_bool* | b | 0 | 1 | 0 | |

## 10. Patch file layout

Sources: `src/common/SurgeSynthesizerIO.cpp` (`savePatchToPath`, `loadPatchByPath`), `src/common/PatchFileHeaderStructs.h`, `src/common/SurgePatch.cpp` (`save_patch`, `load_patch`, `save_xml`, `load_xml`), `src/common/dsp/Wavetable.{h,cpp}`, `resources/data/wavetables/WT fileformat.txt`.

### Binary layout of an .fxp

| offset | size | content |
|---|---|---|
| 0 | 4 | `CcnK` (big-endian magic) |
| 4 | 4 | byteSize, BE (Surge writes 0; reader ignores) |
| 8 | 4 | `FPCh` |
| 12 | 4 | version = 1 (BE) |
| 16 | 4 | fxID `cjs3` (reader rejects other IDs) |
| 20 | 4 | fxVersion = 1 |
| 24 | 4 | numPrograms = 1 |
| 28 | 28 | program name, NUL-padded |
| 56 | 4 | chunkSize (BE) = size of everything after this field |
| 60 | 4 | `sub3` (patch payload start) |
| 64 | 4 | xmlsize, **little-endian** uint32 |
| 68 | 24 | `wtsize[scene][osc]` 6 x uint32 LE: order A.osc1, A.osc2, A.osc3, B.osc1, B.osc2, B.osc3. 0 = no blob |
| 92 | xmlsize | XML (UTF-8) |
| 92+xmlsize | sum(wtsize) | wavetable blobs in order scene A osc1..3, scene B osc1..3, only for entries with non-zero wtsize |

Patch name on load comes from the file name. XML is parsed by TinyXML; element/attribute lookup is by name, so parameter order and omitted parameters are fine (missing = default).

### XML skeleton (rev 24)

Practical start for programmatic edits: take `patches_factory/Templates/Init Saw.fxp` (rev 22, complete XML, other `Init *` templates for Sine, Square, Wavetable, FM2, Modern), cut the XML out at offset 92, edit attributes by name, rebuild the 92-byte header with the new `xmlsize`, keep `wtsize` 0 for non-wavetable patches.

```xml
<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>
<patch revision="24">
  <meta name="" category="" comment="" author="" license=""><tags/></meta>
  <parameters>
    <volume_FX1 type="2" value="1.0"/> ... <volume type="2" value="0.0"/>
    <scene_active type="0" value="0"/> <scenemode type="0" value="0"/> <splitkey type="0" value="60"/>
    <fx_disable type="0" value="0"/> <polylimit type="0" value="16"/> <fx_bypass type="0" value="0"/>
    <fx1_type type="0" value="5"/> <fx1_p0 type="2" value="0.0" extend_range="0"/> ... (12 per slot)
    <a_octave .../> ... <a_osc1_type type="0" value="0"/> ... <a_osc1_param6 type="0" value="1"/> ...
    <a_filter1_cutoff type="2" value="3.0"><modrouting source="16" depth="24.0" muted="0" source_index="0"/></a_filter1_cutoff>
    ... <b_*/> ... <character type="0" value="1"/>
  </parameters>
  <nonparamconfig>...</nonparamconfig>
  <extraoscdata><osc_extra_sc0_osc0 scene="0" osc="0" extra_n="0" wavetable_display_name="..."/>...</extraoscdata>
  <stepsequences/><msegs/><formulae/><extralfo/>
  <customcontroller><entry i="0" bipolar="0" v="0.0" label="-"/>...(8)</customcontroller>
  <lfobanklabels/><modwheel s0="0.0" s1="0.0"/>
  <compatability><correctlyTunedCombFilter v="1"/></compatability>   <!-- optional, absent = true for rev >= 14 -->
  <dawExtraState populated="0"/>
</patch>
```

`type` attribute: 0 = int/bool, 2 = float (`vt_int=0, vt_bool=1, vt_float=2`; bools are written as type 0). Modulation is only read for float-typed params. Float values written with 14 decimals. Param element attributes (all optional): `temposync`, `extend_range`, `absolute`, `deactivated`, `deform_type`, `porta_*`. **Absent-attribute load defaults**: `extend_range` -> 0; `deactivated` -> 1 for every deactivatable param except LFO group, scene `lowcut`, filter/waveshaper `type`, scene `volume`; `deform_type` -> 0 (noise colour: 0 stereo).

### Wavetable blobs (osc types 2 and 7)

`wtsize[sc][osc]` must equal `12 + n_samples * n_tables * (2 if int16 else 4)`; the blob is:

| bytes | field | notes |
|---|---|---|
| 0..3 | tag | `vawt` in files; Surge zeroes it when saving a patch; the loader ignores it in a patch |
| 4..7 | n_samples | uint32 LE, per-frame size, power of 2, 2..4096 |
| 8..9 | n_tables | uint16 LE, 1..512 frames |
| 10..11 | flags | uint16 LE: 0x1 is sample, 0x2 loop sample, 0x4 int16 data (else float32), 0x8 int16 uses full 16-bit range (else 15-bit: 16384 = 1.0) |
| 12.. | frames | all frame 0 samples first, then frame 1, ...; int16 LE or float32 LE |

Surge saves blobs as int16 (flag 0x4 always set, 15-bit range, 16384 = +1.0). The loader (`Wavetable::BuildWT`) honors the flags, so an existing factory `.wt` file (`share/surge-xt/wavetables/**/*.wt`, same 12-byte header with tag `vawt`) can be spliced in unchanged as the blob. The file `.wav` variants (10 in the factory set) use a different container and are not blob-compatible.

The XML does not reference the blob. The only link is the position in the `wtsize` table. `<extraoscdata>` entry for that osc may carry `wavetable_display_name` (shown in the UI; the loader also uses it to look up the table in the factory list for `current_id`), `wavetable_formula` (base64 Lua, for scripted tables), `wavetable_formula_nframes`, `wavetable_formula_res_base`. Without a blob a WT osc plays the table that happens to be loaded in the synth instance (first factory table on a fresh instance; `perform_queued_wtloads`).

Morph interpolation requires `extend_range="1"` on `oscN_param0`. Window osc morph is the same.

Factory Leads folder: 128 patches are rev 20, 3 are rev 9. Lower revisions load fine, absent attributes follow the rev-dependent fixups in `load_xml`.
