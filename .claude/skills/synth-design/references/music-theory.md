# Music theory for DnB riffs, mid basses and leads

Knowledge for designing and evaluating the Mesh live-mode riff engine (bass
and lead lanes: motif clips, scale degrees in key and mode, density,
complexity, variation, spread, register, forms). Researched 2026-10-16.

How to read this file:

- Grid: 16 steps per bar, step 0 = beat 1, 4 = beat 2, 8 = beat 3, 12 = beat 4.
  At 174 BPM one step = 86 ms, one bar = 1.38 s, 64 bars = 88 s.
- Riff notation: `step:degree/length`, length in 16ths. Degrees are 1-based
  over the clip root with `b`/`#` (`1 b2 2 b3 4 #4 b5 5 b6 6 b7 7 8 b9`),
  `,` = octave down, `'` = octave up (same tokens as
  `crates/mesh-core/src/live/motif/starter.rs`). A rhythm line uses the
  starter syntax: `x` short, `X` held, `-` continues, `.` rest.
- Drum reference line under every grid (two-step):
  ```
  step   0 1 2 3 4 5 6 7 8 9 A B C D E F
  kick   K . . . . . . . . . K . . . . .
  snare  . . . . S . . . . . . . S . . .
  ```
- Example riffs are constructed from the cited rules. They are not
  transcriptions of records.
- "Ref" numbers are medians over the user's reference sets
  (`~/Music/mesh-live-dev/references/analysis/<set>/*.melody-{bass,other}.json`,
  produced by `scripts/live-analysis/melody.py`, pyin on htdemucs stems). The
  "other" stem holds leads and, in neuro, part of the mid bass.

## 0. Reference numbers (measured on the user's sets)

| set | stem | median MIDI | range st (p5-p95) | notes / active bar | unison | step (1-2 st) | 3rd | 4th/5th | 6th/7th | octave+ | len median (16ths) | long (>=4) | offbeat share | downbeat share | active share |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| pendulum | bass | 32.7 | 13.9 | 2.2 | .12 | .39 | .13 | .15 | .06 | .03 | 5 | .65 | .71 | .04 | .71 |
| pendulum | other | 46.0 | 26.1 | 2.4 | .32 | .36 | .11 | .08 | .04 | .08 | 3 | .35 | .74 | .07 | .52 |
| metal-step | bass | 31.7 | 13.0 | 2.3 | .19 | .34 | .17 | .09 | .09 | .04 | 5 | .64 | .75 | .07 | .72 |
| metal-step | other | 46.9 | 24.2 | 3.1 | .27 | .35 | .10 | .08 | .04 | .07 | 2 | .27 | .75 | .06 | .49 |
| techstep | bass | 31.9 | 12.1 | 2.0 | .32 | .32 | .08 | .03 | .02 | .03 | 4 | .61 | .77 | .04 | .69 |
| techstep | other | 47.3 | 29.9 | 2.8 | .33 | .32 | .08 | .08 | .04 | .12 | 2 | .29 | .73 | .06 | .49 |
| neurofunk-classics | bass | 29.9 | 9.5 | 2.1 | .41 | .33 | .10 | .02 | .01 | .05 | 4 | .57 | .71 | .09 | .66 |
| neurofunk-classics | other | 46.1 | 33.6 | 2.8 | .32 | .33 | .10 | .05 | .03 | .07 | 2 | .23 | .74 | .07 | .46 |
| jumpup | bass | 29.1 | 15.0 | 1.7 | .19 | .23 | .19 | .03 | .04 | .12 | 7 | .66 | .61 | .18 | .67 |
| jumpup | other | 52.1 | 33.4 | 2.6 | .37 | .27 | .12 | .08 | .04 | .11 | 2 | .36 | .76 | .06 | .43 |
| jungle-tek | bass | 32.1 | 13.9 | 2.4 | .19 | .32 | .11 | .14 | .03 | .05 | 4 | .56 | .85 | .06 | .63 |
| jungle-tek | other | 51.8 | 27.8 | 2.7 | .38 | .25 | .11 | .07 | .03 | .06 | 2 | .33 | .75 | .07 | .38 |

What the table says, across every set:

1. Notes are few: 2-3 onsets in a bar that has notes. Leads play in about
   half the bars (call and response at bar scale).
2. About 3/4 of onsets are off the quarter-note beat. The downbeat (step 0)
   holds only 4-9% of onsets (jump-up bass 18%, the one exception: the
   jump-up bass hits with the kick and holds).
3. Intervals: repeated notes plus steps are 60-70% of all moves. 4ths/5ths
   are 2-15% (neuro and techstep bass 2-3%, Pendulum/jungle bass 14-15%),
   octave leaps 3-12%.
4. Bass notes are long (median 4-7 16ths, 56-66% of notes >= 4 16ths).
   Lead notes are short (median 2 16ths) with about 1/3 staccato.
5. Register: bass MIDI 29-33 (F1-A1), lead/mid MIDI 46-52 (A#2-E3). Leads
   sit low (see also `sound-design.md`: Pendulum "other" median ~MIDI 46).

Caveat: the grid phase is estimated, so a one-step offset is possible in the
per-step histograms (not used here). The off-beat share is robust to that.

## 1. Modes, harmony and dark intervals

### 1.1 Modes in use

| mode | semitones | colour note | where it shows up |
|---|---|---|---|
| aeolian (natural minor) | 0 2 3 5 7 8 10 | b6 | default for all DnB; Pendulum *Witchcraft* (F minor, i-VI-VII-iv) [1] |
| phrygian | 0 1 3 5 7 8 10 | b2 | neuro, techstep, metal riffs; Noisia & The Upbeats *Dead Limit* is F phrygian [2]; Pendulum *Blood Sugar* solo is G phrygian [3] |
| harmonic minor | 0 2 3 5 7 8 11 | 7 (leading tone), b6-7 augmented 2nd | the major V chord in minor: Pendulum *Propane Nightmares* i-V-VI-III-V [4] |
| dorian | 0 2 3 5 7 9 10 | 6 | brighter, rolling minor; *Propane Nightmares* chorus i-IV in G dorian [4], *The Vulture* i-VII-III-VII in F# dorian [5] |
| phrygian dominant | 0 1 4 5 7 8 10 | b2 + major 3rd | mode 5 of harmonic minor, "exotic"/aggressive bass music [6] |
| locrian-ish b5 colour | (as phrygian + b5) | b5 | not a key centre but a riff colour: diminished power chord in *Blood Sugar* [3] |

Mesh already weights the minor family as aeolian 0.5, dorian 0.2, phrygian
0.15, harmonic 0.15 (`harmony.rs`, `MINOR_MODES`).

Phrygian vs minor: the only difference is the b2. Producer theory courses
name the b2 as the source of the dark sound and advise keeping several notes
on the root so the mode stays anchored [7].

### 1.2 Dark intervals and how they are used

- **b2 (minor 2nd above the root)**: the strongest dark colour. Use it as a
  short neighbour note that falls back to 1 (1-b2-1), or as a pickup from
  above into the root. Over a long root pedal the clash creates tension
  without a chord change. *Dead Limit*'s whole melody spans 4 semitones,
  with chord-melody tension rated 95/100 [2]: the darkness comes from a
  narrow cluster around the root, not from range.
- **b6 (minor 6th)**: the aeolian/phrygian colour above the 5th. Typical
  move: 5-b6-5 upper neighbour (Mesh starter `lead-upper-neighbour`). Over
  the VI chord it is the root.
- **b5 / #4 (tritone)**: the most unstable interval. Use it as a short hit
  between roots (1-b5-1), as the diminished power chord (1+b5), or as a
  chromatic passing note 4-b5-5. Never hold it at the end of a phrase.
- **7 (leading tone, harmonic minor)**: the strongest pull back to 1. Use it
  as a pickup on step 14/15 into the next bar's root, or on the V chord in
  a turnaround.
- **b3 vs 3**: the minor 3rd marks minor mode. Power chords (1+5, no 3rd)
  leave it ambiguous, which suits metal-step (*The Tempest*: i5-iv5 [8]).

Expectation research supports this. Stable degrees (1, 5) read as calm and
happy, unstable ones (b2, 7, b5) as tense, and listeners agree strongly on
those qualities (Huron's "scale-degree qualia") [9].

### 1.3 Pedal tones and drones

- A neuro/techstep mid bass often stays on the root for a whole bar or
  phrase and moves by rhythm, timbre and pitch bends (ref: neuro bass repeats
  the same note 41% of the time, techstep 32%).
- A sub pedal under a moving lead is the default harmony carrier: the
  sub plays the chord root (or the key root as a pedal), and the lead and mid
  imply the chords.
- Moving chords over a static root (VI or bII over a root pedal) give
  "suspended" tension, used in breakdowns and builds.

### 1.4 Chromatic neighbour and passing notes

- Neighbour: 1-b2-1, 5-b6-5, 1-7-1 (harmonic minor), 5-#4-5. One step,
  short (1-2 16ths), on a weak step (odd 16ths), resolving on the next
  stronger step.
- Passing: 4-b5-5 (blues/metal), b7-7-1 (chromatic run up into the root),
  b3-2-b2-1 (chromatic fall into the root, techstep/dark).
- Rule: chromatic notes are never held, never land on a phrase end, and
  never exceed about 1 in 6 notes. Hooktheory measures Pendulum leads as 85-100%
  diatonic [1][3][4][10].

### 1.5 Implying harmony with few notes

- Root + rhythm implies the chord root. Root + b3 or root + b6 implies
  i or VI. Root + 5 is neutral (power chord).
- Two-note riffs work because the second note picks the chord: 1-b6 implies
  i moving to VI, 1-b7 implies VII, 1-b2 implies the phrygian bII.
- Pendulum leads use 48-91% chord tones (*Witchcraft* 48%, *Encoder* 91%) [1][10].
  A strong rule for the engine: notes on anchor steps are chord tones,
  colour notes go on weak steps.

### 1.6 Minor-key chord loops in DnB

| loop | example | character |
|---|---|---|
| i-VI-VII-iv | Pendulum *Witchcraft* (F minor) [1] | epic, driving |
| i-V-VI-III-V | Pendulum *Propane Nightmares* verse (G minor, major V) [4] | dramatic, harmonic minor |
| VI-III-V-i-VI | *Propane Nightmares* pre-chorus [4] | rising |
| i-iv (power chords) | *The Tempest* i5-iv5 [8], *Slam* verse i-iv [11] | heavy, static |
| i-IV (dorian) | *Propane Nightmares* chorus [4] | bright minor, lift |
| i-VII-III-VII | *The Vulture* (dorian) [5] | rock |
| i-VI-v / i-VI-iv | general dark-progression guides [12] | dark, sad |
| i-bII (phrygian) | phrygian progressions use the bII major chord [2] | dark, menacing |
| i pedal only | neuro, techstep | all tension from rhythm and timbre |

Chord rhythm: one chord per bar or per 2 bars at 174 BPM (a 4-bar loop =
5.5 s). Faster chord changes read as busy.

### Mapping to Mesh

- **Mode** (`harmony.rs`): keep aeolian as the default. Phrygian and harmonic
  minor make "dark" and "dramatic" harmony seeds. A style option to raise the
  phrygian weight for neuro/techstep styles (e.g. aeolian 0.4, phrygian 0.35,
  harmonic 0.15, dorian 0.1) and the dorian weight for jungle/rollers.
- **Progressions**: offer i-VI-VII-iv, i-V-VI-III-V (harmonic), i-iv, i-bII,
  i-VI-iv-v and a pure pedal (no progression) as named progressions. Chord
  duration 1 or 2 bars. Neuro and techstep styles default to the pedal or i-bII.
- **Scale degrees in clips**: write chord tones on anchor steps, colour
  notes (b2, b6, b5, 7) on weak steps with length 1-2, always resolved by
  step to a chord tone. Chromatic offsets in clips: at most about 1 in 6 notes.
- **Evaluation rule**: a rendered bar fails if a chromatic or tritone note is
  the last note of a phrase or is held for 4+ 16ths.

## 2. Riff construction

### 2.1 Building blocks

- **Motif**: 2-4 notes, 1-2 beats long, one rhythm figure and one contour.
  Hooks in the references are tiny (*Watercolour*'s lead spans 5
  semitones [13], *Dead Limit*'s 4 [2]).
- **Repetition**: the motif repeats unchanged at least 2-3 times before it
  varies. The Hooked study found repetitiveness among the predictors of
  catchiness [14].
- **Call and response**: within a bar (call in steps 0-7, answer in 8-15)
  or across bars (bar 1 call, bar 2 answer). The answer keeps the call's
  rhythm and changes its last 1-2 pitches (ends on 1 vs ends on 5/b7). In
  neuro the "response" is often a different bass sound or a pitch-shifted
  edit of the same loop [15].
- **Sequence**: repeat the motif's rhythm and contour from another degree
  (one step down per bar over i-VI-VII-iv).
- **Inversion**: flip the contour (up becomes down) for the B section or
  the response.
- **Rhythmic displacement**: move the whole motif by 1, 2 or 3 16ths
  (e.g. from step 0 to step 3) to make it syncopated without changing notes.
- **Hook vs filler**: the hook is the 1-2 bar figure that returns at the
  same place in every phrase. Filler is the variation bar, the turnaround
  and the fills. A riff with no stable hook sounds like noodling. A riff
  that is only hook sounds like a loop.

### 2.2 Heavy and energetic vs childish and carnival

| trait | heavy / dark / energetic | childish / carnival |
|---|---|---|
| pitch set | root-heavy, b2 / b6 / b5 colours, minor or power chord | major or major pentatonic, major 3rd and 6th prominent |
| intervals | repeats and steps (60-70%), octave drops on weak steps | 4ths/5ths/octaves up and down every note |
| rhythm | anticipations (step 15, 7, 10), 3-3-2, notes in the drum gaps | equal notes on the beats (0, 4, 8, 12) or straight 8ths |
| length | short pickups into long held notes, staccato stabs against held roots | all notes the same length |
| space | rests, especially under the snare (4, 12) | no rests, every step filled |
| range | narrow (bass 1 octave, lead hook within a 5th or 6th) | wide jumps over 1.5+ octaves |
| contour | flat with one gesture (a fall to the root, a lift and back) | zigzag, triad arpeggios up and down |
| pitch movement | glides and bends into notes | discrete hops |

Sources for the right column: emotion research associates major mode, high
pitch and fast tempo with happiness [16]. Pentatonic material avoids every
clashing note, so it cannot create tension [17]. The reference statistics in
section 0 show 4th/5th moves at 2-15% and off-beat onsets at about 74%. The
"carnival" quality is the combination of happy cues (major, high, bouncy)
with no tension or space.

### 2.3 Example riffs (grids)

Each riff lists the steps, a rhythm line, the style and character, and why
it works. Drum reference: kick 0 and 10, snare 4 and 12.

**R1. Neuro mid bass, root pedal with b2 flick** (neurofunk, dark, heavy, 1 bar)
```
0:1/3 3:1/2 6:b2/1 7:1/3 11:1/1 14:,b7/2
rhythm  X--X-xX--.x..X-.
```
Root pedal in 3-3-2, the b2 is one 16th on a weak step and falls back to 1.
The note at 11 ends on 12, so the snare is clear. The pickup `,b7` on 14 leads
into the next downbeat.

**R2. Techstep one-note riff** (techstep, dark, minimal, 2 bars)
```
bar 1: 0:1/6 7:1/1 10:1/4 15:1/1
bar 2: 0:1/6 7:1/1 10:1/2 12:b2/1 14:b5/2
rhythm 1: X-----.x..X---.x
rhythm 2: X-----.x..X-x.X-
```
All rhythm in bar 1: anticipation on 15 into the downbeat. Bar 2 answers with
b2 and the tritone at the phrase end of the 2-bar unit. It never resolves in
the bar, but the next bar's root resolves it.

**R3. Neuro call and response inside the bar** (neurofunk, energetic, 1 bar)
```
call:     0:1/2 3:1/1 6:b2/2
response: 10:1/1 11:b3/1 13:1/3
rhythm  X-.x..X-..xxX--.
```
The call sits between kick 0 and snare 4 and the gap before beat 3. The
response sits after the second kick. Each half is 3 notes. Swap the
response's sound (Mesh `*` swap) for the classic neuro edit.

**R4. Metal-step chug** (metal-step / Pendulum, energetic, heavy, 1 bar, power chords)
```
0:1/2 3:1/1 4:1/2 6:1/1 7:1/1 10:b6/2 12:b7/2 14:b7/1 15:,b7/1
rhythm  X-.xX-xxx.X-X-xx
```
A guitar-style gallop on the root, then VI and VII roots. Density near the
upper end (9 notes) is for peak energy only. Play it as power chords (1+5)
with short, palm-mute-like gates.

**R5. Pendulum lead hook as a sequence over i-VI-VII-iv** (Pendulum, hooky, 4 bars, lead)
```
bar 1 (i):   2:5/1 3:b6/1 4:5/6 10:b3/2 12:4/4
bar 2 (VI):  2:b3/1 3:4/1 4:b3/6 10:1/2 12:,b7/4
bar 3 (VII): 2:4/1 3:5/1 4:4/6 10:2/2 12:b3/4
bar 4 (iv):  2:1/1 3:2/1 4:b3/4 8:4/2 10:5/4 14:.
rhythm 1-3: ..xxX-----X-X---
rhythm 4:   ..xxX---X-X---..
```
Two-16th pickup, a long note held through the snare, then an answer. Each bar
is the same figure a step lower (sequence). Bar 4 breaks the pattern and
climbs to 5 to restart the loop. Narrow range (an octave).

**R6. Jump-up bass, long note and octave drop** (jump-up, heavy, hooky, 1 bar)
```
0:1/6 6:1/2 10:8/2 12:1/3 15:,b7/1
rhythm  X-----X-..X-X--x
```
Matches the jump-up exception in the refs: hits with the kick on the
downbeat (jump-up bass downbeat share 18%) and holds (median 7 16ths). Glide
or pitch bend on the long note. The octave jump on 10 lands on the second
kick.

**R7. Jungle sub, dub root-fifth** (jungle / jungle-tek, rolling, 2 bars)
```
bar 1: 0:1/6 6:1/2 10:,5/4 14:,b7/2
bar 2: 0:1/6 6:1/2 10:b3/2 12:4/2 14:5/2
rhythm: X-----X-..X---X-
```
Reggae/dub bass: long sliding notes answering the kick rather than doubling it,
root and 5th as the pillars [18]. The jungle-tek refs show 14% 4ths/5ths in
the bass, the highest of all sets.

**R8. Techstep tritone stab** (techstep, dark, 1 bar)
```
0:1/2 3:1/1 6:1/1 10:b5/2 14:1/1 15:b2/1
rhythm  X-.x..x...X-..xx
```
Sparse stabs. The b5 hits with the second kick for maximum weight. The b2
anticipates the next downbeat from above.

**R9. Phrygian lead line** (neuro / techstep, dark, 1 bar, lead)
```
0:5/2 2:b6/1 3:5/3 8:b2'/2 10:1'/4 14:b7/2
rhythm  X-xX--..X-X---X-
```
The upper neighbour 5-b6-5, then b2 resolving down to 1 on the second kick.
Range a 5th plus a step.

**R10. Harmonic-minor leading-tone hook** (Pendulum / metal-step, dramatic, 1 bar, lead)
```
0:1/2 2:,7/1 3:1/3 6:b3/2 8:5/2 10:b6/2 12:5/2 14:,7/2
rhythm  X-xX--X-X-X-X-X-
```
The leading tone 7 below the root works as a neighbour (step 2) and as the
pickup into the next bar (step 14). The climb 1-b3-5-b6-5 is a chord-tone
outline with a b6 colour.

**R11. Dorian roller lead** (jungle / roller, energetic, brighter, 1 bar, lead)
```
0:1/2 3:b3/1 4:4/2 6:6/2 8:5/4 13:4/1 14:b3/2
rhythm  X-.xX-X-X---.xX-
```
The natural 6 (dorian) gives lift without turning major. Long note on beat 3,
a short descent at the end.

**R12. Metal-step tritone power riff** (metal-step, heavy, 1 bar, mid bass)
```
0:1/3 3:1/3 6:b5/2 8:1/2 10:b2/3 13:1/3
rhythm  X--X--X-X-X--X--
```
3-3-2-2-3-3. The b5 on step 6 and the b2 on the second kick are the
dissonances. Play as power chords. Like *Blood Sugar*'s diminished power
chord [3].

**R13. Neuro octave-displacement edit** (neurofunk, energetic, 1 bar, mid bass)
```
0:1/1 1:8/1 3:1/2 7:1/1 11:8/2 14:1/2
rhythm  xx.X-..x...X-.X-
```
One pitch class, three octave placements: a rhythm-only riff that relies on
the sound and the edit. Fits the ref neuro bass profile: 41% repeated
notes, 2% 4ths/5ths.

**R14. Pendulum mid bass under the chord loop** (Pendulum, energetic, 1-bar clip, chord-relative)
```
0:1/3 3:1/3 6:1/2 8:1/3 11:1/3 14:8/2
rhythm  X--X--X-X--X--X-
```
Written chord-relative, so the harmony moves it through i-VI-III-VII. The
3-3-2 + 3-3-2 groupings push against the kick (0, 10). The octave on 14 is
the pickup.

**R15. Lead long note with pickup** (Pendulum / liquid-edge, hooky, 2 bars, lead)
```
bar 1: 0:5/8 10:4/1 11:b3/1 12:4/2 14:5/1 15:b6/1
bar 2: 0:5/6 6:b3/2 10:1/4 14:,b7/1 15:1/1
rhythm 1: X-------..xxX-xx
rhythm 2: X-----X-..X---xx
```
A held 5, a turn (4-b3-4) and a 2-note pickup (5-b6) back into 5. Bar 2
falls to the root. Range a 6th.

**R16. Lead stabs answering the bass's call** (neuro, energetic, 1 bar, lead over R3's call)
```
8:5/1 9:5/1 11:b6/2 14:5/2
rhythm  ........xx.X-.X-
```
Silent while the bass calls (steps 0-7), then answers in the bass's gap.
Same pitch twice, a b6 neighbour, back to 5. Plays in the upper octave of
the lead register.

**R17. Jump-up talking two-note riff** (jump-up, hooky, 1 bar)
```
0:1/3 3:b3/3 6:1/2 10:1/3 13:,b7/3
rhythm  X--X--X-..X--X--
```
Two pitches plus glides between them ("talking" bass). The rest on
steps 8-9 leaves room before the second kick.

**R18. Dotted-8th chain for the C/D filler** (any style, filler, 1 bar)
```
0:1/2 3:b3/2 6:4/2 9:5/2 12:b6/2 15:5/1
rhythm  X-.X-.X-.X-.X-.x
```
Groups of 3 16ths cut across the 4/4 pulse (pseudo-triplet, 3 against 4).
For true 8th-note triplets the engine's 384 ticks per bar allows onsets every
32 ticks (12 per bar).

### 2.4 Counter-examples (childish / carnival)

**X1. Major-pentatonic quarter bounce** (childish)
```
0:1/2 4:3/2 8:5/2 12:6/2
rhythm  X-..X-..X-..X-..
```
Why: major 3rd and 6th (major pentatonic), equal notes exactly on the beats,
no syncopation. A nursery-rhyme shape. Every onset sits on a beat (refs:
about 74% of onsets are off the beat).

**X2. Root-octave-fifth oompah** (carnival)
```
0:1/1 2:8/1 4:5/1 6:8/1 8:1/1 10:8/1 12:5/1 14:8/1
rhythm  x.x.x.x.x.x.x.x.
```
Why: straight 8ths with octave and 4th/5th jumps on every note (100% leaps vs
3-15% in the refs), no rests, no held note. Sounds like a fairground organ.

**X3. Triad arpeggio up and down on the beats** (childish, even in minor)
```
bar 1: 0:1/4 4:b3/4 8:5/4 12:8/4
bar 2: 0:5/4 4:b3/4 8:1/8
rhythm 1: X---X---X---X---
rhythm 2: X---X---X-------
```
Why: equal on-beat quarters, a pure triad arpeggio, symmetric up and down.
Minor mode alone does not save it. Mesh's starter `lead-triad-up` has this
shape.

**X4. Wide zigzag** (circus)
```
0:1/2 2:5'/2 4:b3/2 6:1''/2 8:5/2 10:b3'/2 12:1/2 14:8'/2
rhythm  X-X-X-X-X-X-X-X-
```
Why: range over 2 octaves inside one bar (refs: lead p5-p95 range 24-34
semitones over the whole track, bass 9-15), alternating jumps of 7-12+
semitones, constant 8ths.

**X5. Major root-octave bounce into the triad** (carnival)
```
bar 1: 0:1/1 3:8/1 6:1/1 8:8/1
bar 2: 0:3/4 4:5/4 8:8/2
rhythm 1: x..x..x.x.......
rhythm 2: X---X---X-......
```
Why: major mode, octave bounces, then a major triad climbing to the top. Bright,
upward, resolved. It has the energy cues without the tension. Mesh's starter
`lead-major-bounce` has this shape.

**X6. Ascending major scale run every bar** (childish)
```
0:1/2 2:2/2 4:3/2 6:4/2 8:5/2 10:6/2 12:7/2 14:8/2
rhythm  X-X-X-X-X-X-X-X-
```
Why: a scale exercise with no motif, no rest and no rhythm figure. Moving by
step is fine (refs: 25-39%), but a step run with nothing else is a scale, not
a riff.

### Mapping to Mesh

- **Motif clips**: curate clips to the "heavy" column. Hard limits per clip:
  4th/5th moves at most about 15% of moves (bass), octave+ at most about 15%,
  at least one rest of 2+ 16ths per bar, at least one off-beat onset per beat
  pair, range at most 12 semitones for bass and 9 for a hook (lead).
- **Starter clips to revise or relabel**: `lead-triad-up` (X3),
  `lead-major-bounce` (X5), `lead-octave-call`, `bass-eighths-triad`.
  Keep them only as deliberate "bright" options, not as defaults.
- **Variation** (`motif.rs`): the variation bar keeps the rhythm and changes
  only the last 1-2 notes (call vs answer), or applies sequence (whole bar
  +/- 1 degree), inversion or displacement by 1-3 steps. Never re-roll the
  whole bar.
- **Complexity**: map low to rhythm-only riffs (R2, R13), mid to 2-3 pitch
  classes with one colour note (R1, R3, R17), high to more colour notes and
  displacement (R8, R12), capped by the hard limits above.
- **Lead `melody: 1` cells**: build cell palettes from the R5/R9/R15/R16
  figures (pickup into a held note, neighbour turn, fall to the root, answer
  stabs). Order them A A B A or A B A C.
- **Evaluation**: run the X1-X6 detectors on every rendered 4-bar phrase:
  share of onsets on beats > 0.6, 4th/5th/octave share > 0.4, range > 19
  semitones, no rest of 2+ 16ths in a bar, or a major-3rd-and-6th pitch
  set without b2/b6/b3. Each one flags the phrase as "carnival".

## 3. Rhythm at 170-175 BPM

### 3.1 The drum grid

- Two-step: kick on steps 0 and 10, snare on steps 4 and 12, 168-178 BPM [19].
  The second kick is pushed to the 8th before the second snare, which gives
  the "stepping" feel [20]. Mesh `TWO_STEP_KICK`/`TWO_STEP_SNARE` match this.
- Half-time feel: the snare backbeat at 174 is heard as a half-time pulse
  (about 87 BPM). Pendulum's Hooktheory tempi are often written at 86-87 BPM
  for that reason [4][11].
- Common tempo: 172-176, 174 is the most common [19][21].

### 3.2 Where bass and lead notes sit

| step(s) | role for bass/lead | why |
|---|---|---|
| 0 | the root hits with the kick (bass) or stays silent (lead) | refs: only 4-9% of onsets (jump-up bass 18%) |
| 2, 3 | after the first kick | first gap. 3 is the 3-3-2 point |
| 4, 12 | mostly rest or held note | snare. A new onset here masks the snare |
| 6, 7 | second gap. 7 = anticipation of beat 3 | 3-3-2 point (6), anticipation (7) |
| 8 | beat 3: a held note from 6/7, or a new note if the riff wants weight | |
| 10 | second kick: heavy hits (b5, octave drop) | weight on the step |
| 11, 13 | after the kick, before and after the snare | answer phrase |
| 14, 15 | pickup / anticipation into the next downbeat | the strongest drive point |

- **Anticipation**: a note one 16th (step 15, 7, 3, 11) or one 8th (14, 6)
  before a beat, held across it. This is the main source of push in the
  references (about 74% of onsets off the beat).
- **3-3-2 / tresillo**: onsets 0, 3, 6 (and 8, 11, 14 in the second half).
  This is the Euclidean rhythm E(3,8) [22]. 3-3-3-3-4 (0, 3, 6, 9, 12) spreads
  the same feel over a whole bar.
- **Dotted patterns**: dotted 8ths (every 3 steps) cut across the beat (R18).
  Dotted-8th delays are a standard bass trick [23].
- **Triplets**: 8th triplets (12 per bar, 32 ticks in Mesh) for fills and
  C/D fillers, not for the main hook.

### 3.3 Note length and gate

- Long notes (4+ 16ths) carry weight and harmony: bass median 4-7 16ths.
- Short notes (1-2 16ths) carry drive: lead median 2 16ths, about 1/3 staccato.
- The energetic profile is short pickups into long notes (R5, R15). Equal
  lengths everywhere read as mechanical (X1, X2).
- Gate as energy: at high energy shorten gates (stabs) but keep 1-2 long
  anchor notes per bar. At low energy lengthen notes and drop pickups.

### 3.4 Density vs space

- Refs: 2-3 onsets per active bar. The lead plays in about half the bars,
  the bass in about 2/3.
- Groove research: desire to move and pleasure peak at medium syncopation
  (an inverted U) [24]. Highly syncopated rhythms get re-heard as less
  syncopated and are remembered worse [25]. Keep the hook moderately syncopated
  and put the extreme displacement in fillers.

### Mapping to Mesh

- **Density thinning** (`motif.rs`, `blocks.rs METRIC_WEIGHT`): today weak
  notes drop by metric weight, so downbeats survive and anticipations
  die. Replace with an **anchor flag per clip note** (set at load: the
  longest note, the first note after a rest, notes on 3-3-2 points and pickups
  on 14/15). Thinning drops non-anchors first, whatever their metric weight.
- **Density targets**: density 0.5 should land at 2-3 onsets per bar
  (bass) and 2.5-3 (lead). Density 1.0 caps at about 8 per bar (R4 is the
  ceiling). Density 0 keeps 1-2 anchors per bar.
- **Syncopation (complexity)**: low = anchors on 0, 6/7 and 10.
  Mid = 3-3-2 and pickups on 14/15. High = displacement by 1-3 steps and
  dotted chains. Keep the hook's LHL syncopation in the middle band
  (section 6.2).
- **Note length**: the current `LENGTH_SCALE (2.0, 0.55)` follows the
  refs if it keeps 1-2 notes per bar at their written length (anchors) and
  only scales the rest.
- **Rule A7 (sparse against the drums)**: also mask onsets on steps 4 and 12
  (snare) for the lead unless the note is held from earlier.

## 4. Mid-bass vs lead roles

### 4.1 Register (MIDI)

| role | range | ref median | notes |
|---|---|---|---|
| sub | 24-40 (C1-E2) | (in the bass stem) | root only, mono, one register [26] |
| mid bass / reese | 36-55 | bass stem 29-33 is mostly sub + low mid | octave+ above the sub on the same roots [26] |
| lead (Pendulum, metal-step, neuro) | 40-64 | 46-47 | leads sit low, user heard +1 octave as too high (`sound-design.md`) |
| lead (jump-up, jungle) | 44-70 | 52 | higher |
| top doubling | +12 over the lead | | octave doubling for bite |

### 4.2 Roles

- **Sub**: plays the chord root or the key root as a pedal, follows the mid
  bass's rhythm in simplified form (long notes). Never plays the mid bass's
  octave jumps (an octave jump leaves the sub band) [26].
- **Mid bass (neuro, techstep)**: carries the riff. It moves by rhythm, edits,
  bends and timbre more than by pitch (ref neuro bass: 41% repeated notes,
  range 9.5 semitones). Call and response between two bass sounds is the
  neuro core [15].
- **Mid bass (Pendulum, metal-step)**: power-chord riff, often layered with
  guitar or bass guitar ("adds a subharmonic that gets lost otherwise") [27].
- **Lead**: answers in the bass's gaps (R16), holds long notes over busy bass
  edits, or doubles the bass riff in unison/octave for the hook's climax.
- **Unison**: the lead plays the bass riff's rhythm and pitches (+12 or
  +24). Maximum weight, used for the drop's last 4-8 bars or the hook bar.
- **Octave doubling**: the lead doubles itself +12 at high energy (Mesh
  `spread`).

### 4.3 Articulation and sound

- Stab (sharp attack, 1-2 16ths): drives and syncopates. Needs rests around it.
- Sustained reese (detuned, moving filter): carries harmony. The movement
  is in the filter, not the notes ("static detuned saws are nothing") [26].
- Pitch bend at note start (neuro stab with a harsh bend) to play off kick
  and snare [28]. Glide/portamento for leads [21].
- Rule of thumb: the busier the sound's own movement (mudpies), the fewer
  notes the riff needs.

### Mapping to Mesh

- **Register**: bass lane floor at degree -3 (`BASS_FLOOR_DEGREE`) keeps the
  bass near the refs. Lead base register should resolve to MIDI 44-52 (Pendulum,
  metal-step, neuro) and 50-56 (jump-up, jungle).
- **Spread**: bass octave lifts only on weak, non-anchor notes and at most
  about 15% of notes (refs: octave+ 3-12%). Lead doubling +12 from spread
  0.3 is consistent with the octave+ share.
- **Unison mode**: a lead option that copies the bass clip's onsets
  (+12/+24) for the last bar of 4 or the last 4 of 16. Useful as a "climax"
  letter (B or `'`).
- **Interlock**: the lead picks a clip whose onsets fall in the bass clip's
  rests (already in `lead.rs` via the drum-gap approximation). Score
  interlock as the share of lead onsets not within 1 step of a bass onset,
  target > 0.6.
- **Mudpie lanes**: lower complexity (fewer pitch classes) when the sound is
  a mudpie. The sound carries the movement.

## 5. Development over 16/32/64 bars

### 5.1 Structure

- DJ-friendly DnB: 16 or 32 bar intro, phrases of 16/32 bars. A common plan
  is intro 16, build 16, drop 32, breakdown 16, second drop 32 [29].
- Arrangement: intro (atmosphere), build (drums and bass filter in), drop
  (full), breakdown (stripped, melodic), second drop (variation), outro [21].
- Swire wanted to move past "drop, 32 bars of breakdown" towards song parts [27].
  Pendulum tracks have verse/chorus sections at 87 BPM half-time [4][11].

### 5.2 How riffs evolve

- Every 4 bars: the last bar varies (fill, turnaround, different ending).
- Every 8 bars: the answer changes (A A A A' becomes A B A B').
- Every 16 bars: a filler bar or half-bar (break, triplet run, sound swap).
- Every 32 bars: a new section or a new riff letter.
- Second drop: a switch-up, the same drums with a new or varied bass
  riff. Producers make variation by re-cutting the bass loop into new
  patterns and adding call-and-response layers [15].

### 5.3 Tension and release

- Tension builders: pedal under moving chords, chromatic climbs (b6-6-b7-7-1),
  rising register, shorter gates, more onsets, filter opening, the b2/b5
  colours, rising onset frequency [30].
- Release: return to the root on the downbeat of a new phrase, longer notes,
  the hook's return.
- Farbood's model: perceived tension follows pitch height, loudness, onset
  frequency, harmony, syncopation and rhythmic regularity, with trends weighted
  over a moving window [30].

### Mapping to Mesh

- **Forms** (`form.rs`): the default forms already place C/D at the 16-bar
  points and breakouts at 64. Keep the hook letter (A) for at least 8
  bars before the first change. Primes (A') vary only the second half.
- **Variation knob**: 0 = only the 16-bar fillers, 0.5 = last bar of every
  4, 1 = also the 8-bar answer change. Never more than one change per
  4-bar phrase outside fillers.
- **Second pass (bars 65-128)**: switch the main letter's motif clip (new
  clip, same rhythm family) or swap A/B order. This is the "second drop
  switch-up".
- **Energy curve**: map the energy slider to density, gate shortness and
  register together, and ramp them over 8-16 bars, not per bar.

## 6. Melodic quality metrics

All of these can run on the engine's symbolic output (degrees, onsets in
ticks, lengths), which is more exact than the pyin metrics on audio.

### 6.1 Pitch and interval

- **Interval histogram** (consecutive notes within 2 beats): unison, step
  (1-2), third (3-4), 4th/5th (5-7), 6th/7th (8-11), octave+ (>= 12). Compare to
  the section 0 table. Carnival flag: 4th/5th + octave+ > 0.4.
- **Range**: p5-p95 semitones per 4-bar phrase. Bass at most 12-15, hook at most 9.
- **Pitch-class entropy**: H = -sum p(pc) log2 p(pc) over the 12 PCs,
  weighted by duration. Theory: one-note riff 0 bits, 3 equally used PCs
  1.58, 5 PCs 2.32, all 7 diatonic 2.81. Expected targets (not yet measured
  on the refs): neuro/techstep bass < 1.5, leads 1.5-2.5.
- **Contour**: Huron contour classes (convex, concave, ascending,
  descending, flat) per phrase via FANTASTIC [31]. Earworms have common
  global contours with unusual gradients between turning points [32].
- **Chord-tone share**: notes on anchor steps that are chord tones.
  Pendulum leads 48-91% chord tones overall [1][10].

### 6.2 Rhythm and syncopation

- **Longuet-Higgins & Lee (LHL)**, as used by Fitch & Rosenfeld [25] and
  adapted by Witek [24]. Metric weights on the 16-step grid: step 0 = 0,
  step 8 = -1, steps 4 and 12 = -2, other even steps = -3, odd steps = -4.
  For every note N followed by a rest or tie at a position R with
  w(R) > w(N) (before the next onset), add w(R) - w(N). Sum per bar.
  Examples: R1 has notes ending before stronger silent positions on 3 → 4,
  7 → 8 and 14 → 0 (next bar).
- **Weighted note-to-beat distance (WNBD)** [33]: beats = quarters (every 4
  steps). For each onset x not on a beat, T = distance to the nearest beat
  in beats (0.25 or 0.5). D = 1/T if the note ends at or before the next
  beat, 2/T if it ends after the next beat but at or before the one after,
  1/T if longer. WNBD = sum D / number of notes.
- **Off-beat share** and **downbeat share**: as in `melody.py`. Refs about
  0.74 off-beat and 0.04-0.09 downbeat.
- **Targets**: groove peaks at medium syncopation [24]. Calibrate LHL and WNBD
  bands on the curated clips the user likes and keep the hook in the middle
  tercile.

### 6.3 Repetition and self-similarity

- Onset overlap (Jaccard) and pitch agreement between consecutive 1, 2 and
  4-bar blocks (as `melody.py` rep1/2/4). Refs: rep4_pitch 0.33-0.63
  (jungle-tek highest).
- Best repetition lag in bars: the hook period (expect 2, 4 or 8).
- Catchiness predictors: repetitiveness and conventionality of the melody
  (Hooked) [14]. Liking peaks at moderate information content (IDyOM) [34].
  A hook should repeat exactly but contain one unexpected event (a b2, a
  displaced note, an interval leap).

### 6.4 Tonal tension

- **Spiral array** (Herremans & Chew): cloud diameter (spread of the notes
  in tonal space), cloud momentum (movement of the centre of effect between
  windows), tensile strain (distance from the key's centre) [35].
  `partitura.musicanalysis.tonal_tension` implements them [36].
- **Lerdahl & Krumhansl**: prolongational structure, pitch-space distance,
  surface dissonance and attraction [37]. Heavier to implement. The spiral
  array is the practical choice.
- Use: compute tensile strain per beat. The phrase should peak in bar 4 or 8
  (turnaround) and drop at the next downbeat.

### Mapping to Mesh

- Add a symbolic metric pass to the scoring tools (`scripts/synth-design/`
  score, `scripts/live-analysis/`) that reads the engine's note events and
  reports: interval histogram, range, PC entropy, LHL, WNBD, off-beat share,
  rep1/2/4, chord-tone share on anchors, carnival flags, and tension per beat.
- Calibrate the bands on the reference sets by running the same symbolic
  metrics on transcribed or curated clips. The audio metrics in section 0 are
  the cross-check.
- Use the metrics as **filters at load time** (reject or flag clips) and as
  **regression tests** for generator changes, not as real-time objectives.

## 7. Generative approaches

### 7.1 Options

- **Curated motifs plus transformations** (current Mesh): clips as the
  source, with sequence, inversion, displacement, octave placement,
  thinning and repeats as transformations. Best fit for a live tool: the
  musical quality lives in the clips, and the knobs reshape them predictably.
- **Markov chains with constraints**: train interval and rhythm transitions
  on the curated clips. Constraints (start/end on the root, max leap, rest
  quota, anchors) are added as Pachet & Roy's Markov constraints, which keep
  the Markov probabilities while satisfying the rules [38].
- **Grammars**: rewrite rules from phrase to bar to cell (e.g. `phrase →
  A A A' F`, `bar → call response`, `call → pickup held`). Probabilistic
  temporal graph grammars (Quick & Hudak, Kulitta) generate structure,
  and the post-processing turns it into notes [39].
- **Euclidean rhythms**: E(k,16) and rotations for rhythm cells. E(3,8) =
  3-3-2, E(5,16) = 3-3-3-3-4 [22]. Cheap and stylistically right for bass
  rhythms.
- **Tension-targeted generation**: MorpheuS fits repeated patterns to a
  tension profile from the spiral array [40]. Useful offline to generate
  clip candidates for curation.

### 7.2 Rule set for the engine (synthesised from sections 1-6)

1. Anchors: 1-3 per bar, chord tones, at least one off the beat
   (pickup 14/15, 3-3-2 point, 7).
2. Rests: at least one rest of 2+ 16ths per bar, preferably around the snare.
3. Intervals: steps and repeats at least 55% of moves. 4ths/5ths at most 15%,
   octaves at most 15% and only on weak steps.
4. Colour notes (b2, b6, b5, 7) on weak steps, length 1-2, resolved by step.
5. Range: bass at most 12 semitones per phrase, hook at most 9, lead phrase at
   most 19.
6. Phrase: 4 bars, bars 1-3 the hook (exact or call/answer), bar 4 the
   variation. Fillers at 16-bar boundaries.
7. Density: 2-3 onsets per bar at density 0.5, at most 8 at 1.0.
8. Length: 1-2 long notes per bar (4+ 16ths), the rest short.
9. Interlock: lead onsets in the bass's rests, except unison climax bars.
10. Surprise: one unexpected event per hook (a colour note, a displacement,
    a leap), never more than two per bar.

### Mapping to Mesh

| knob | maps to | values |
|---|---|---|
| `density` | onsets per bar, non-anchors thinned first | 0 → 1-2 anchors, 0.5 → 2-3, 1 → at most 8 |
| `complexity` | syncopation band + colour notes + displacement | low: anchors only, mid: 3-3-2 + pickups + one colour note, high: displacement 1-3 steps, dotted chains, two colour notes |
| `variation` | how often and how strongly the last bar varies | see section 5 mapping |
| `spread` | octave placement and range | bass lifts at most 15% of notes, lead doubling from 0.3 |
| `register` | base octave | lead MIDI 44-52 base for Pendulum, metal-step, neuro |
| note length | gate scale with anchors kept | anchors keep written length |
| mode (harmony seed) | aeolian / phrygian / harmonic / dorian | style weights per section 1 mapping |
| forms / letters | A/B hook letters, C/D filler, primes | section 5 mapping |
| motif clips | curated against the X1-X6 detectors and the section 7.2 rules | |

## Sources

1. Hooktheory, Pendulum *Witchcraft* (undated, user transcription): https://www.hooktheory.com/theorytab/view/pendulum/witchcraft
2. Hooktheory, Noisia & The Upbeats *Dead Limit* (undated): https://www.hooktheory.com/theorytab/view/noisia-and-the-upbeats/dead-limit
3. Hooktheory, Pendulum *Blood Sugar* (undated): https://www.hooktheory.com/theorytab/view/pendulum/blood-sugar
4. Hooktheory, Pendulum *Propane Nightmares* (undated): https://www.hooktheory.com/theorytab/view/pendulum/propane-nightmares
5. Hooktheory, Pendulum *The Vulture* (undated): https://www.hooktheory.com/theorytab/view/pendulum/the-vulture
6. Unison, Phrygian dominant scale (undated): https://unison.audio/phrygian-dominant-scale/
7. CreativeLive, Tomas George, Phrygian lesson (undated): https://firehose.creativelive.com/class/music-theory-for-electronic-producers-tomas-george/lessons/phrygian
8. Hooktheory, Pendulum *The Tempest* (undated): https://www.hooktheory.com/theorytab/view/pendulum/the-tempest
9. Huron, *Sweet Anticipation*, MIT Press 2006: https://discover.musikverket.se/bib/1255655
10. Hooktheory, Pendulum *Encoder* (undated): https://www.hooktheory.com/theorytab/view/pendulum/encoder
11. Hooktheory, Pendulum *Slam* (undated): https://www.hooktheory.com/theorytab/view/pendulum/slam
12. eMastered, dark chord progressions (undated): https://emastered.com/blog/dark-chord-progressions
13. Hooktheory, Pendulum *Watercolour* (undated): https://www.hooktheory.com/theorytab/view/pendulum/watercolour
14. Van Balen, Burgoyne et al., Hooked, ISMIR 2013: https://archives.ismir.net/ismir2013/paper/000101.pdf
15. Loopmasters, Neurofunk DnB drop in Ableton (2015): https://www.loopmasters.com/articles/4270-How-To-Make-a-Neuro-Drum-and-Bass-Drop-in-Ableton-Live
16. Juslin/Lindström review on emotion and musical structure, Frontiers 2013: https://www.frontiersin.org/journals/psychology/articles/10.3389/fpsyg.2013.00487/full
17. Slooply, melody tips (undated): https://slooply.com/blog/10-tips-tricks-for-making-better-melodies/
18. futureproof genre explorer, Jungle/DnB (undated): https://genreexplorer.futureproofmusicschool.com/genre/jungle-dnb
19. Attack Magazine, Raw Drum & Bass (2012): https://www.attackmagazine.com/technique/beat-dissected/raw-drum-bass/
20. Attack Magazine, beat tutorials (search summary, undated): https://www.attackmagazine.com/technique/beat-dissected/incessant-drum-bass-beat/
21. Beatportal, DnB like Sub Focus, A.M.C, Delta Heavy (2025): https://www.beatportal.com/articles/818379-step-by-step-guide-to-producing-drum-and-bass-like-sub-focus-a-m-c-and-delta-heavy
22. Toussaint, The Euclidean Algorithm Generates Traditional Musical Rhythms (2005): https://cgm.cs.mcgill.ca/~godfried/publications/banff.pdf
23. Attack Magazine, eight common bassline styles (2015): https://www.attackmagazine.com/technique/tutorials/low-end-theory-exploring-eight-common-bassline-styles/
24. Witek et al., Syncopation, body-movement and pleasure in groove music, PLOS ONE 2014: https://journals.plos.org/plosone/article?id=10.1371/journal.pone.0094446
25. Fitch & Rosenfeld, Perception and production of syncopated rhythms, Music Perception 2007: https://research-portal.st-andrews.ac.uk/en/publications/perception-and-production-of-syncopated-rhythms/
26. User note "DnB Bass & Melody Design" (2026-07-02), citing MusicRadar, EDMProd, NI: /home/data01/Notes/🗂️ Collection/DnB Bass & Melody Design.md
27. Sound On Sound, Pendulum (June 2008): https://www.soundonsound.com/people/pendulum
28. Point Blank bassline masterclass (search summary, 2023): https://magneticmag.com/2023/03/mastering-basslines-with-stevie-r-point-blank-online-masterclass/
29. Lost Stories Academy, DJ-friendly tracks (undated): https://loststoriesacademy.com/blogs-and-tutorials/what-makes-a-track-dj-friendly
30. Farbood, A parametric, temporal model of musical tension, Music Perception 2012: https://bpb-us-e1.wpmucdn.com/wp.nyu.edu/dist/f/11865/files/2020/08/Farbood_2012_Musical_Tension.pdf
31. Whyatt & Harrison, melody-features (FANTASTIC and others), arXiv 2026: https://arxiv.org/abs/2608.19061
32. Jakubowski et al., Dissecting an earworm, 2017: https://research.gold.ac.uk/19405
33. Gómez, Melvin, Rappaport, Toussaint, Mathematical measures of syncopation, Bridges 2005: https://archive.bridgesmathart.org/2005/bridges2005-73.html
34. Gold et al. 2019, predictability and uncertainty in liking (IDyOM): https://pmc.ncbi.nlm.nih.gov/articles/PMC6867811
35. Herremans & Chew, Tension ribbons, TENOR 2016: https://dorienherremans.com/content/tension-ribbons-quantifying-and-visualising-tonal-tension
36. partitura tonal_tension source: https://partitura.readthedocs.io/en/v1.2.0/_modules/partitura/musicanalysis/tonal_tension.html
37. Lerdahl & Krumhansl, Modeling tonal tension, Music Perception 2007: https://sonicfield.org/library/modeling-tonal-tension
38. Pachet & Roy, Markov constraints, Constraints 2011: https://link.springer.com/article/10.1007/s10601-010-9101-4
39. Music as Language: Probabilistic Temporal Graph Grammars, FARM 2019: https://icfp19.sigplan.org/details/farm-2019-papers/9/Music-as-Language-Putting-Probabilistic-Temporal-Graph-Grammars-to-Good-Use
40. Herremans & Chew, MorpheuS: https://arxiv.org/pdf/1812.04832

## 8. What the approved riffs do (user-approved, round 3, 2026-10-16)

Seven Pendulum drop riffs (*Blood Sugar*, *Granite*, *Hold Your Colour*,
*Slam*, *Witchcraft*, *Propane Nightmares*, *Watercolour*), transcribed from
the bass stems with basic-pitch (`ref-riffs-bp/bass-stem`), played 1:1 in F
minor. The user: "the melody and movement is much much much much better"
than the starter clips. Measured with `scripts/synth-design/riff_stats.py`
(medians; starter clips in brackets):

| feature | approved | starter | rule (section 7.2) |
|---|---|---|---|
| onsets off the beat | 0.75 | 0.41 | about 0.74 in the refs: holds |
| onsets on the downbeat | 0.00 | 0.27 | refs 0.04-0.09: holds, even stricter |
| onsets on pickups (3, 7, 14, 15) | 0.27 | 0.07 | anticipations drive: holds |
| onsets on the snare (4, 12) | 0.09 | 0.17 | mask the snare: holds |
| repeated notes | 0.31 | 0.15 | repeats + steps >= 55%: repeats carry it |
| 4th/5th moves | 0.16 | 0.28 | <= 15%: at the limit |
| octave+ moves | 0.00 | 0.00 | <= 15% |
| range (semitones) | 10 | 12 | hook <= 9, bass <= 12: 7-12 typical, Witchcraft 21 |
| notes per bar | 4.25 | 3.75 | 2-3 at density 0.5: the approved riffs are busier |
| notes >= 4 16ths | 0.27 | 0.18 | 1-2 long notes per bar |
| bars with a rest >= 2 16ths | 0.50 | 1.00 | "one rest per bar": the approved riffs flow more |
| exact repeat after 1 bar / 2 bars | 0.00 / 0.11 | 0.00 / 0.00 | the phrase is 4 bars long, not a 1-bar loop |
| LHL syncopation per bar | 0.50 | 0.50 | medium |
| b5 (tritone) present | 5 of 7 | 0 of 10 | the signature colour |

What makes them good, in the terms of sections 1-3:

1. **Nothing lands on beat 1.** The riff enters a 16th or 8th early
   (anticipation) or answers after the kick. The starter clips put 27% of
   their notes on the downbeat; the approved riffs none.
2. **The tritone is the colour.** b5 appears in five of seven riffs (Blood
   Sugar 1-b3-4-b5, Slam 1-b3-b5, Propane, Hold Your Colour), always short
   and between roots. b2, b6 and the leading tone 7 add a second colour.
3. **Repetition on the pitch, variety in the rhythm.** A third of the moves
   repeat the note (a pedal), the rest are mostly steps; leaps are rare and
   never octave zigzags.
4. **A 4-bar sentence, not a 1-bar loop.** No bar repeats the previous one
   exactly; the phrase is call, answer, variation, turnaround (rep1 0.00,
   rep2 0.11). The starter clips are 1-2 bar loops.
5. **Minor and power-chord degrees only.** 1, b3, 4, 5, b7 plus the colours;
   major 3rd and 6th appear only in Propane (harmonic-minor 7 and the
   dorian 6) and never as a resting note.
6. **Busier than the rule said, but syncopated.** About 4 notes per bar
   with half the bars playing through: the energy comes from off-beat
   16ths, not from rests. The rest rule applies to the lead over a busy bass,
   not to the drop riff itself.

### Mapping to Mesh

- Clip generation (`scripts/synth-design/riff_gen.py`): weight onsets by the
  approved riffs' step histogram, forbid step 0 except as a held note, put a
  pickup on 14/15 in the turnaround bar, build 4-bar call/answer sentences,
  pitch from 1 b3 4 5 b7 with b5/b2/b6/7 as short colours, repeats 25-40%.
- Scoring: `riff_stats.py` features against the approved medians; reject
  clips with downbeat share > 0.15, 4th/5th > 0.25, major 3/6 on long notes.
