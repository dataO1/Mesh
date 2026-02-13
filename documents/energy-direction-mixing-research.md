# Energy-Aware Harmonic Mixing — Research & Design Document

**Date**: February 2026
**Status**: Phase 1 (unified scoring) implemented. Phases 2-3 pending.
**Depends on**: Smart Suggestions v1 (HNSW-based), Energy Direction Fader v1 (basic bonuses)

---

## 1. Problem Statement

Smart suggestions v1 uses a simple `harmonic_score()` that treats key compatibility as a binary window — everything beyond ±2 Camelot steps scores 0.0 and is filtered out. The `directional_harmonic_bonus()` added in the energy fader v1 only nudges ±1 step preferences by ±0.15.

This means:
- **+2 energy boost** (a bread-and-butter DJ technique) barely survives the filter
- **+7 semitone lift** (classic "final chorus" key change) is completely invisible
- **±6 tritone** (useful for drastic resets at extreme fader positions) is impossible
- **A↔B mood shifts** are not direction-aware (minor→major lift vs major→minor darken)
- The fader can't "unlock" dramatic transitions — it only nudges within the existing narrow window

---

## 2. Research Findings

### 2.1 Camelot Wheel = Circle of Fifths

Each clockwise step on the Camelot wheel is a perfect fifth (7 semitones). The number of shared scale tones between two keys is directly determined by their Camelot distance:

| Camelot Step | Semitones (step×7 mod 12) | Musical Interval | Shared Notes (of 7) |
|---|---|---|---|
| 0 (same key) | 0 | Unison | 7 |
| ±1 | 7 / 5 | Perfect 5th / Perfect 4th | 6 |
| ±2 | 2 / 10 | Major 2nd / Minor 7th | 5 |
| ±3 | 9 / 3 | Major 6th / Minor 3rd | 4 |
| ±4 | 4 / 8 | Major 3rd / Minor 6th | 3 |
| ±5 | 11 / 1 | Major 7th / Minor 2nd | 2 |
| ±6 | 6 | Tritone ("Devil's interval") | 2 |

**Key insight**: Shared notes decrease monotonically with Camelot distance. At ±6 (the tritone, maximally distant on the wheel), only 2 notes overlap — the most dissonant possible key relationship.

### 2.2 Directional Energy Effects

Professional DJs use Camelot wheel direction to control perceived energy:

| Movement | Camelot | Effect | DJ Use |
|---|---|---|---|
| **Clockwise +1** | e.g., 8A→9A | Subtle energy **lift** | Standard build technique, barely noticeable |
| **Counter-clockwise -1** | e.g., 8A→7A | Subtle energy **release** | Natural cool-down, creating space |
| **Energy Boost +2** | e.g., 8A→10A | Dramatic energy **surge** | "Hands in the air" moments, every 20-30 min |
| **Energy Cool -2** | e.g., 8A→6A | Dramatic cool-**down** | Post-peak recovery |
| **Semitone Up +7** | e.g., 8A→3A | Pop key change **lift** | Climactic moments, risky (3 shared notes) |
| **Semitone Down -5** | e.g., 8A→3A alt | Descending semitone | Same interval as +7 but descending perception |
| **Mood Lift A→B** | e.g., 8A→8B | Minor→Major **euphoria** | Emotional climax, all 7 notes shared |
| **Mood Darken B→A** | e.g., 8B→8A | Major→Minor **depth** | Introspective moments |
| **Diagonal Up** | e.g., 8B→9A | Compound mood+energy shift | Safe (B(n)→A(n+1) only) |
| **Tritone ±6** | e.g., 8A→2A | Maximum **dissonance** | Total tonal reset, almost never used |

**Important**: The +2 energy boost is **safer than +7** (5 shared notes vs ~3), despite being less dramatic. Mixed In Key recommends +2 every 20-30 minutes; +7 only at strategic peaks.

**Diagonal safety rule**: From B, you can safely go to A(n+1). From A, you can safely go to B(n-1). The reverse diagonals clash.

### 2.3 Professional DJ Set Structure (Five Phases)

| Phase | Energy Level | Key Strategy |
|---|---|---|
| **Warm-Up** | Low-Medium (5-6) | Stay narrow (same-key, -1), minor keys, long blends |
| **Build** | Medium-Rising (6-7) | Steady +1 clockwise, introduce A→B mood lifts |
| **Peak** | High (7-8+) | Energy boost +2, semitone lift +7, major keys |
| **Release** | Medium (6-7) | -1 counter-clockwise, B→A mood darkens |
| **Finale** | Declining (5-6) | Return to minor keys, -1 moves, emotional material |

The energy direction fader maps naturally: left=warm-up/finale, center=build/release, right=peak.

### 2.4 Key Distance Models (Academic)

#### Krumhansl-Kessler Probe-Tone Profiles (1982)

Empirically measured perceptual key distances using listener experiments. Produced 12-value "key profiles" for each major/minor key:

```
C Major: [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88]
C Minor: [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17]
```

Pearson correlation between any two key profiles gives a continuous perceptual distance. The resulting 24×24 matrix maps onto a **torus** (4D) where both relative keys (Am↔C) and parallel keys (C↔Cm) appear nearby.

Key correlations (approximate):
- Same key: r = 1.0
- Relative major/minor (Am↔C): r ≈ 0.65
- Perfect fifth (C↔G): r ≈ 0.55-0.60
- Parallel major/minor (C↔Cm): r ≈ 0.50-0.55
- Whole tone (C↔D): r ≈ 0.35
- Tritone (C↔F#): r ≈ -0.10 to 0.10

**Advantage over Camelot distance**: Captures that relative keys are perceptually very close and that parallel keys are moderately close, providing continuous (not discrete) distance values.

**TODO**: Embed the 24×24 Krumhansl correlation matrix as a `const` lookup table for an optional Tier 2 scoring enhancement. Requires computing all 24 profiles by transposing the major/minor templates and calculating Pearson correlations.

#### Gebhardt et al. — Psychoacoustic Consonance (DAFx-15, 2015)

Measured actual consonance between two simultaneously-playing tracks using:
- **Roughness model** (Plomp & Levelt): beating/interference between close partials
- **Pitch commonality model** (Terhardt): shared virtual pitches

**Critical finding**: Listeners preferred consonance-optimized transitions over commercial Camelot-based matching. The optimal pitch shift depends on the specific harmonic content of both tracks, not just their key labels.

**Implication**: Simple key matching is a useful heuristic but not optimal. Future enhancement could analyze actual spectral content for consonance scoring.

Sources:
- Gebhardt et al., "Psychoacoustic Approaches for Harmonic Music Mixing", Applied Sciences 6(5):123, 2016
- Gebhardt et al., "Harmonic Mixing Based on Roughness and Pitch Commonality", DAFx-15 Proceedings, 2015

#### Vande Veire & De Bie — Automatic DJ System (2018)

Built a fully integrated automatic DJ system for Drum & Bass:
- Key compatibility as a **hard filter** (go/no-go), not a continuous score
- Energy management via 3D "theme descriptor" capturing energy and spectral content
- Three transition types: double drop (climactic), rolling (energetic), relaxed (calm)
- 91% accuracy on 160-song corpus

**Takeaway**: The hierarchical approach (hard key filter → soft energy ranking) works well. Our system can use the adaptive threshold approach instead.

Source: Vande Veire & De Bie, "From Raw Audio to a Seamless Mix", EURASIP J. Audio Speech Music Process., 2018

#### ML Models for DJ Transitions

| Model | Year | Approach |
|---|---|---|
| **DJ-MC** | 2014 | Reinforcement learning MDP for playlist recommendation |
| **DeepFADE** | 2019 | Three-tiered RL (song selection → timing → transition generation) |
| **DJ AI** | 2024 | Transformer models for playlist sequencing + transition generation |
| **Spotify AH-DQN** | 2023 | Modified deep Q-Network for large-scale playlist generation |
| **Deej-AI** | Open source | Deep learning playlists from audio embeddings |

Sources:
- Liebman et al., "DJ-MC: A Reinforcement-Learning Agent for Music Playlist Recommendation", arXiv:1401.1880, 2014
- Nazzaro, "Mixing Music Using Deep Reinforcement Learning", DiVA 2019
- Spotify Research, "Automatic Music Playlist Generation via Simulation-based RL", 2023

---

## 3. Current Implementation (v1)

### 3.1 harmonic_score() — Binary Window

```rust
match (diff, same_letter) {
    (0, true)  => 1.0,   // Same key
    (0, false) => 0.85,  // Relative major/minor
    (1, true)  => 0.9,   // Adjacent, same mode
    (1, false) => 0.6,   // Adjacent, different mode
    (2, _)     => 0.6,   // Two steps
    _          => 0.0,   // CLIFF: everything else = incompatible
}
```

### 3.2 directional_harmonic_bonus() — Small Nudge

Only considers ±1 same-mode steps. Returns ±0.15 bonus/penalty max. Does not handle +2 energy boosts, semitone lifts, A↔B mood shifts, or larger jumps.

### 3.3 Four Separate Scoring Modes

Similar, HarmonicMix, EnergyMatch, Combined — each with different weight distributions. Redundant once unified scoring is implemented.

---

## 4. Proposed Design: Unified Key Transition Scoring

### 4.1 Single Scoring Function

Replace both `harmonic_score()` and `directional_harmonic_bonus()` with:

```rust
fn key_transition_score(
    seed_key: &MusicalKey,
    cand_key: &MusicalKey,
    energy_bias: f32,  // -1.0 (drop) to +1.0 (peak)
) -> f32  // 0.0 (worst) to ~1.0 (best)
```

The energy_bias **reshapes the entire scoring landscape**, not just nudges within it.

### 4.2 Transition Classification

Every key relationship maps to a named type:

```rust
enum TransitionType {
    SameKey,          // 0 steps, same mode
    Relative,         // 0 steps, diff mode (Am↔C)
    AdjacentUp,       // +1, same mode (energy lift)
    AdjacentDown,     // -1, same mode (energy cool)
    DiagonalUp,       // safe cross-mode diagonal (B(n)→A(n+1) or A(n)→B(n-1))
    DiagonalDown,     // safe cross-mode diagonal (reverse direction)
    EnergyBoost,      // +2, same mode
    EnergyCool,       // -2, same mode
    MoodLift,         // A→B same number (minor→major)
    MoodDarken,       // B→A same number (major→minor)
    SemitoneUp,       // +7 same mode (pop key change up)
    SemitoneDown,     // -5 same mode (= semitone down)
    ParallelMode,     // same root, diff mode (C↔Cm, 3-step cross)
    FarStep(i8),      // ±3, ±4, ±5 same mode
    FarCross(i8),     // ±2+ cross-mode (non-diagonal)
    Tritone,          // ±6
}
```

### 4.3 Base Compatibility Scores (energy_bias = 0)

These produce behavior nearly identical to v1 at fader center:

| Transition | Base Score | Shared Notes | Rationale |
|---|---|---|---|
| SameKey | 1.00 | 7/7 | Perfect blend, zero risk |
| Relative | 0.90 | 7/7 | All notes shared, different tonal center |
| AdjacentUp | 0.85 | 6/7 | Bread-and-butter DJ transition |
| AdjacentDown | 0.85 | 6/7 | Same quality, opposite direction |
| DiagonalUp | 0.75 | ~6/7 | Safe compound mood+energy |
| DiagonalDown | 0.75 | ~6/7 | Safe compound (reverse) |
| MoodLift (A→B) | 0.70 | 7/7 | Powerful emotional shift |
| MoodDarken (B→A) | 0.70 | 7/7 | Same quality, opposite mood |
| EnergyBoost (+2) | 0.50 | 5/7 | Noticeable but workable |
| EnergyCool (-2) | 0.50 | 5/7 | Same quality, opposite direction |
| ParallelMode | 0.40 | varies | Same tonic, different color |
| FarStep(±3) | 0.25 | 4/7 | Getting risky, needs careful execution |
| SemitoneUp (+7) | 0.20 | ~3/7 | High risk, high reward |
| SemitoneDown (-5) | 0.20 | ~3/7 | Same interval, descending |
| FarStep(±4) | 0.15 | 3/7 | Dramatic, rarely appropriate |
| FarCross(any) | 0.10 | varies | Generally risky cross-mode |
| FarStep(±5) | 0.08 | 2/7 | Very risky |
| Tritone (±6) | 0.03 | 2/7 | Maximum dissonance, almost never |

**Key difference from v1**: Nothing is 0.0. Every transition has some score, just very small for distant keys. At fader center, the filter threshold ensures only safe transitions pass.

### 4.4 Energy-Dependent Modifiers

The modifier scales linearly with `|energy_bias|`. All modifiers are 0.0 at center.

#### Raising Energy (bias > 0, fader right of center)

| Transition | Modifier at bias=1.0 | Resulting Score |
|---|---|---|
| AdjacentUp (+1) | +0.10 | 0.95 |
| EnergyBoost (+2) | +0.30 | **0.80** (competitive with adjacent!) |
| SemitoneUp (+7) | +0.35 | **0.55** (viable peak option) |
| MoodLift (A→B) | +0.20 | **0.90** (euphoric shift) |
| DiagonalUp | +0.15 | 0.90 |
| AdjacentDown (-1) | -0.15 | 0.70 (penalized) |
| EnergyCool (-2) | -0.20 | 0.30 (penalized) |
| MoodDarken (B→A) | -0.15 | 0.55 (penalized) |
| FarStep(±3) | +0.05 | 0.30 (slight unlock at extreme) |
| Tritone | 0.00 | 0.03 (no help when raising) |

#### Dropping Energy (bias < 0, fader left of center)

| Transition | Modifier at bias=-1.0 | Resulting Score |
|---|---|---|
| AdjacentDown (-1) | +0.10 | 0.95 |
| EnergyCool (-2) | +0.25 | **0.75** (dramatic cooldown) |
| MoodDarken (B→A) | +0.20 | **0.90** (emotional depth) |
| DiagonalDown | +0.15 | 0.90 |
| SemitoneDown (-5) | +0.20 | **0.40** (noticeable drop) |
| Tritone (±6) | +0.15 | **0.18** (creative "total reset" at extreme) |
| FarStep(±3) | +0.10 | **0.35** (opens up at extremes) |
| FarStep(±4,±5) | +0.08 | ~0.23 (barely viable outliers) |
| AdjacentUp (+1) | -0.15 | 0.70 (penalized) |
| EnergyBoost (+2) | -0.20 | 0.30 (penalized) |
| MoodLift (A→B) | -0.15 | 0.55 (penalized) |

#### Extreme Creative Outliers

At maximum fader deflection (|bias| > 0.9), some normally-unusable transitions become viable creative options:

**Peak mode (bias ≈ 1.0):**
- +7 Semitone Up: 0.55 — genuine suggestion for climactic moments
- +2 Energy Boost: 0.80 — primary dramatic option
- FarStep(+3): 0.30 — bold outlier for adventurous DJs

**Drop mode (bias ≈ -1.0):**
- Tritone (±6): 0.18 — "total reset" option, extreme contrast
- FarStep(±4): 0.23 — dramatic tonal departure
- -5 Semitone Down: 0.40 — striking descending shift

These outliers appear at the bottom of suggestion lists but give adventurous DJs creative options they wouldn't find in any Camelot-only system.

### 4.5 Adaptive Filter Threshold

The harmonic filter threshold relaxes as the fader moves from center:

```
|energy_bias| < 0.1  →  threshold = 0.50  (strict: same-key, adjacent, relative only)
|energy_bias| < 0.4  →  threshold = 0.35  (moderate: +2 energy boosts pass)
|energy_bias| < 0.7  →  threshold = 0.20  (strong: semitone lifts, ±3 pass)
|energy_bias| >= 0.7 →  threshold = 0.10  (extreme: nearly everything except tritone)
```

This is the mechanism by which the fader "unlocks" dramatic transitions — it lowers the quality floor.

### 4.6 A↔B Mood Shift Asymmetry

**Current design**: Asymmetric scoring:
- A→B (minor→major) treated as "lift" — boosted when raising energy
- B→A (major→minor) treated as "darken" — boosted when dropping energy

**Note for future consideration**: Symmetry might be appropriate because:
- Some DJs use B→A for intimacy/depth during builds (not just cool-downs)
- A→B can create a "release" feeling (relief, not just lift)
- The direction of mood change may be more context-dependent than energy-determined

If we make this symmetric, both A→B and B→A would get the same base score, and neither would be penalized by energy direction. The energy fader would only affect same-mode directional movements.

---

## 5. Unified Scoring Mode

### 5.1 Rationale: Single Mode Replaces Four

With energy-aware key transition scoring, a single composite mode handles all use cases:

| Old Mode | How Unified Handles It |
|---|---|
| **Similar** | Fader at center → key_transition returns safe scores, HNSW dominates |
| **HarmonicMix** | Same as above, key_transition naturally prioritizes harmonic compatibility |
| **EnergyMatch** | Fader away from center → lufs_direction_bonus + key_transition energy modifiers |
| **Combined** | The unified score IS the combined approach |

### 5.2 Unified Scoring Formula

```
score = w_hnsw * hnsw_dist
      + w_key  * (1.0 - key_transition_score)
      + w_lufs * lufs_direction_bonus
      + w_bpm  * bpm_penalty
```

Where weights are:
- `w_hnsw = 0.40` — audio similarity (genre/style)
- `w_key  = 0.30` — harmonic compatibility (energy-direction-aware)
- `w_lufs = 0.15` — loudness alignment with energy intent
- `w_bpm  = 0.15` — tempo compatibility

Lower score = better match (consistent with existing convention).

At fader center: `key_transition_score` returns base compatibility (identical to v1 behavior for safe transitions), `lufs_direction_bonus` returns 0. Net effect = v1-equivalent scoring.

---

## 6. Example Scenarios

### 6.1 Fader at Center (0.5) — "Play It Safe"

Currently playing: Am (8A), -9.0 LUFS, 126 BPM

| Candidate | Key Score | LUFS Bonus | BPM Pen | Behavior |
|---|---|---|---|---|
| Em (9A) | 0.85 → pen 0.045 | 0.0 | low | Top pick (adjacent) |
| C (8B) | 0.90 → pen 0.030 | 0.0 | low | Strong (relative) |
| Am (8A) | 1.00 → pen 0.000 | 0.0 | low | Same key, safe |
| Bm (10A) | 0.50 → pen 0.150 | 0.0 | low | Visible but ranked low |
| F#m (11A) | 0.25 → pen 0.225 | 0.0 | low | Below filter threshold |

**Result**: Nearly identical to v1.

### 6.2 Fader at 0.85 — "Raise Energy"

energy_bias = +0.70

| Candidate | Base | Modifier | Final Key Score | Net Effect |
|---|---|---|---|---|
| Em (9A, +1 up) | 0.85 | +0.07 | 0.92 | Top subtle lift |
| C (8B, mood lift) | 0.70 | +0.14 | 0.84 | Euphoric shift |
| Bm (10A, +2 boost) | 0.50 | +0.21 | **0.71** | Energy boost unlocked! |
| Dm (7A, -1 down) | 0.85 | -0.11 | 0.74 | Still present, ranked lower |

### 6.3 Fader at 1.0 — "PEAK"

energy_bias = +1.0

| Candidate | Base | Modifier | Final Key Score | Net Effect |
|---|---|---|---|---|
| Bm (10A, +2 boost) | 0.50 | +0.30 | **0.80** | Ranks alongside adjacent! |
| Bbm (3A, +7 semi) | 0.20 | +0.35 | **0.55** | Viable peak moment! |
| C (8B, mood lift) | 0.70 | +0.20 | **0.90** | Euphoric climax |
| Em (9A, +1 up) | 0.85 | +0.10 | 0.95 | Still top safe option |

Filter threshold drops to 0.10 — dramatic transitions visible.

### 6.4 Fader at 0.0 — "DROP"

energy_bias = -1.0

| Candidate | Base | Modifier | Final Key Score | Net Effect |
|---|---|---|---|---|
| Dm (7A, -1 down) | 0.85 | +0.10 | 0.95 | Top cooling pick |
| Gm (6A, -2 cool) | 0.50 | +0.25 | **0.75** | Dramatic cooldown |
| Fm (4A, ±4) | 0.15 | +0.08 | 0.23 | Drastic outlier |
| Ebm (2A, tritone) | 0.03 | +0.15 | **0.18** | "Total reset" at extreme |

---

## 7. Implementation Checklist

### Phase 1: Core Scoring — COMPLETE
- [x] Implement `classify_transition()` → `TransitionType`
- [x] Implement `key_transition_score(seed, cand, energy_bias)` with base + modifiers
- [x] Implement adaptive filter threshold
- [x] Replace `harmonic_score()` and `directional_harmonic_bonus()` in `suggestions.rs`
- [x] Replace four scoring modes with unified scoring formula
- [x] Remove `SuggestionMode` enum (settings UI section, config field, browser state)
- [x] Add comprehensive tests (24 tests: classification, scoring, thresholds, LUFS)
- [x] Verify zero-regression at fader center (base scores match v1 for safe transitions)

### Phase 2: Krumhansl Enhancement (future)
- [ ] Embed 24×24 Krumhansl correlation matrix as `const` lookup
- [ ] Add `krumhansl_distance(key1, key2) -> f32` function
- [ ] Option to blend Camelot-based and Krumhansl-based scoring
- [ ] A/B comparison testing against base system

### Phase 3: Audio-Based Consonance (future, research)
- [ ] Investigate Gebhardt roughness model for actual transition quality
- [ ] Could score consonance at specific transition points (intro/outro overlap)
- [ ] Requires audio buffer access, significantly more complex

---

## 8. References

### Academic Papers
- Gebhardt, Goeltze, Hlatky, "Psychoacoustic Approaches for Harmonic Music Mixing", Applied Sciences 6(5):123, 2016
- Gebhardt, Goeltze, Hlatky, "Harmonic Mixing Based on Roughness and Pitch Commonality", DAFx-15, 2015
- Vande Veire & De Bie, "From Raw Audio to a Seamless Mix", EURASIP J. Audio Speech Music Process., 2018
- Krumhansl & Kessler, "Tracing the Dynamic Changes in Perceived Tonal Organization", Psychological Review 89(4), 1982
- Liebman et al., "DJ-MC: A Reinforcement-Learning Agent for Music Playlist Recommendation", arXiv:1401.1880, 2014
- Lerdahl, "Tonal Pitch Space", Oxford University Press, 2001

### Professional DJ Resources
- Mixed In Key, "Harmonic Mixing Guide" — https://mixedinkey.com/harmonic-mixing-guide/
- Mixed In Key, "Energy Boost DJ Mixing Tutorial" — https://mixedinkey.com/harmonic-mixing-guide/energy-boost-dj-mixing-tutorial/
- Mixed In Key, "Sorting Playlists by Energy Level" — https://mixedinkey.com/harmonic-mixing-guide/sorting-playlists-by-energy-level/
- DJ TechTools, "Advanced Key Mixing Techniques for DJs", 2013
- DJ TechTools, "Controlling the Dancefloor: Organizing Playlists by Energy", 2022
- DJ.Studio, "Anatomy of a Great DJ Mix: Structure, Energy Flow, Transition Logic"
- DJoid, "How to Prepare a House DJ Set in Chapters"

### Key Distance Theory
- Krumhansl & Kessler probe-tone key profiles: implementation at https://gist.github.com/bmcfee/1f66825cef2eb34c839b42dddbad49fd
- Weber's Torus / Neo-Riepelian key distance: Springer, "A Neo-Riepelian Key-Distance Theory"
- Plomp & Levelt roughness model (1965) — foundation of consonance measurement
- Terhardt virtual pitch model (1982) — pitch commonality measurement
