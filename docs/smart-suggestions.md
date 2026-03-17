# Smart Suggestions

## Overview

When toggled on in the collection browser, mesh analyzes the tracks currently
playing on your decks and recommends what to play next. Suggestions update
automatically when you load a new track, change volume, or adjust the energy
fader.

## How It Works

1. **Seed tracks** -- Currently playing decks (above a volume threshold) become
   "seed" tracks.
2. **Candidate retrieval** -- HNSW vector search finds the 50--100 most
   sonically similar tracks using 16-dimensional audio fingerprints extracted
   during import.
3. **Re-scoring** -- Each candidate is scored against all seeds using multiple
   factors (see below).
4. **Filtering** -- Tracks below a dynamic score threshold are removed. Tracks
   already played this session are excluded and dimmed in the browser.
5. **Ranking** -- Top results are displayed with reason tags explaining why each
   was recommended.

## Scoring Factors

The suggestion score combines multiple factors. The weights adapt based on the
energy direction fader position:

| Factor                       | Center (Neutral) | Extreme (Full Energy Bias) |
|------------------------------|:-----------------:|:--------------------------:|
| Audio similarity (HNSW)      | 42%               | 0%                         |
| Key compatibility            | 25%               | 15%                        |
| Key energy direction         | 15%               | 22%                        |
| Genre-normalized aggression  | 0%                | 30%                        |
| Danceability                 | 0%                | 10%                        |
| Approachability              | 0%                | 6%                         |
| Tonal/timbre contrast        | 0%                | 4%                         |
| Production match             | 3%                | 3%                         |
| BPM proximity                | 15%               | 10%                        |

At center, audio similarity dominates for safe, harmonically compatible
matches. At extremes, audio similarity drops to zero and energy-aware scoring
takes over, allowing bolder transitions.

## Energy Direction Fader

The fader controls what kind of energy transition you want:

- **Left (Drop / Cool)** -- Favor lower-energy, cooler tracks. Prefer
  transitions that reduce intensity.
- **Center (Maintain)** -- No energy bias. Audio similarity and harmonic safety
  dominate.
- **Right (Peak / Build)** -- Favor higher-energy tracks. Prefer transitions
  that build intensity.

The fader continuously adjusts the scoring weights -- there are no discrete
modes. Moving it slightly right gently biases toward energy-raising transitions.
Moving it fully right makes genre-normalized aggression the dominant signal.

### Adaptive Filter Threshold

The minimum score required to include a suggestion relaxes as the fader moves
away from center:

| Fader Position | Minimum Score | Effect                               |
|----------------|:-------------:|--------------------------------------|
| Center         | 0.50          | Strict -- only safe matches          |
| Moderate       | 0.35          | Slightly relaxed                     |
| Strong         | 0.20          | Permissive                           |
| Extreme        | 0.10          | Lenient -- allows bold transitions   |

This prevents "dead zones" where no suggestions appear at extreme fader
positions.

## Key Compatibility Scoring

Two algorithms are available in Settings > Display > Key Matching:

### Camelot Wheel (default)

Classic DJ wheel with 12 positions and A (minor) / B (major) modes. Compatible
transitions are scored by distance on the wheel:

- Same key: best score
- Adjacent (+/-1 on wheel): very compatible
- Relative major/minor (same position, different mode): compatible
- Diagonal: moderately compatible
- Far steps: less compatible but sometimes musically interesting

### Krumhansl Perceptual Model

Based on Krumhansl-Kessler (1982) music psychology research. A 24x24 matrix of
perceptual key distances derived from listener probe-tone ratings. More nuanced
than Camelot for:

- Cross-mode transitions (C major to C minor)
- Distant key relationships that still sound good perceptually
- Subtle quality differences between "adjacent" transitions

## Transition Types

Each key relationship is classified into a named transition type with an
associated emotional direction:

| Transition    | Direction | Description                                          |
|---------------|-----------|------------------------------------------------------|
| SameKey       | Neutral   | Same key -- safest possible                          |
| AdjacentUp    | Raise     | +1 on Camelot wheel -- subtle energy lift             |
| AdjacentDown  | Cool      | -1 on Camelot wheel -- subtle energy drop             |
| EnergyBoost   | Raise     | Major key shift that raises energy                   |
| EnergyCool    | Cool      | Minor key shift that lowers energy                   |
| MoodLift      | Raise     | Minor to major -- emotional brightening              |
| MoodDarken    | Cool      | Major to minor -- emotional darkening                |
| DiagonalUp    | Raise     | B(n) to A(n+1) -- upward diagonal                   |
| DiagonalDown  | Cool      | A(n) to B(n-1) -- downward diagonal                 |
| SemitoneUp    | Raise     | +7 clockwise (wraps to -5) -- dramatic energy raise  |
| SemitoneDown  | Cool      | -7 clockwise (wraps to +5) -- dramatic energy drop   |
| FarStep       | Neutral   | Large wheel distance, same mode                     |
| FarCross      | Neutral   | Large wheel distance, cross mode (incl. reverse diagonals) |
| Tritone       | Cool      | 6 semitones apart -- maximum tension                 |

The energy direction fader rewards transitions matching the requested direction
and penalizes opposing ones.

## Reason Tags

Each suggested track shows a colored pill tag indicating how it relates to what
is currently playing.

### Arrow Direction

The arrow is per-track and based on the key transition type, not the fader
position:

- **Up arrow** -- energy-raising transition (AdjacentUp, EnergyBoost, MoodLift,
  DiagonalUp, SemitoneUp)
- **Down arrow** -- energy-cooling transition (AdjacentDown, EnergyCool,
  MoodDarken, DiagonalDown, SemitoneDown, Tritone)
- **Dash** -- neutral transition (SameKey, FarStep, FarCross)

### Color Coding

Color is based on the key compatibility score:

- **Green** -- score >= 0.7 (excellent harmonic match)
- **Amber** -- score >= 0.4 (acceptable match)
- **Red** -- score < 0.4 (risky match -- may clash harmonically)

Note: A track can show a down-arrow even when the fader is pushed right. The
arrow reflects the key transition itself, not the fader direction. That track may
still have been suggested for other energy-related reasons (arousal, genre
aggression, danceability).

## ML Analysis Integration

If ML analysis was run during import (genre, arousal, danceability), the
suggestion system uses these signals:

- **Arousal** replaces LUFS for energy direction scoring. Arousal is more
  perceptually accurate than raw loudness for judging a track's energy.
- **Danceability** aligns with energy fader direction at extremes. Higher
  danceability is favored when pushing toward peak energy.
- **Genre-normalized aggression** -- Intensity is scored relative to the
  track's own genre, so a house track and a DnB track can both register as
  "high energy for their genre" without DnB always dominating.

If no ML data is available (for example, older imports before ML was added),
fallback weights are used: 45% similarity, 35% key, 20% BPM.

To run ML analysis on existing tracks, right-click in the collection browser and
select Re-analyse Metadata > ML Tags.

## Suggestion Refresh Behavior

Suggestions automatically refresh when:

- A new track is loaded onto a deck
- A deck's volume crosses the activity threshold (becomes audible or goes
  silent)
- The energy direction fader is moved
- Play/pause state changes on a deck

A debounce timer prevents excessive re-queries during rapid changes. Only one
pending refresh runs at a time.

## Tips for Effective Use

- **Start at center.** Let audio similarity find safe matches, then explore with
  the fader.
- **Push the fader gradually.** Small movements make subtle changes. Reserve
  extremes for dramatic set direction shifts.
- **Trust the arrows.** A green up-arrow track is both harmonically safe and
  energy-raising -- ideal for building a set.
- **Amber is fine.** Amber-tagged tracks are harmonically acceptable and often
  more interesting than an all-green set.
- **Red tracks -- use with caution.** The harmonic clash may be audible, but
  some DJs deliberately use tension for effect.
- **Re-analyze for better results.** Run ML analysis on your collection to give
  the system arousal and genre data. Suggestions improve significantly with ML
  features available.

## Known Limitations

- Suggestions require at least one playing deck with a loaded track above the
  volume threshold.
- The first query after loading a collection may take 1--2 seconds as the HNSW
  index warms up.
- Very small collections (under 50 tracks) may not produce diverse suggestions.
- Tracks must have been analyzed during import. Manually added database entries
  without audio features will not appear in suggestions.
