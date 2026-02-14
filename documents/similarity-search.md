# Smart Suggestions — Similarity Search & Scoring

Technical documentation for Mesh's track recommendation system. This system powers the suggestion panel in mesh-player's collection browser.

---

## Overview

When suggestions are enabled, Mesh analyzes the tracks loaded on your decks and recommends what to play next. The system combines **audio fingerprint similarity** (HNSW vector search), **harmonic compatibility** (Camelot/Krumhansl key analysis), **perceived energy** (ML arousal), and **tempo proximity** into a single score per candidate track. An energy direction fader lets you steer results toward higher-energy or cooler tracks.

### Pipeline

```
Loaded decks (seed tracks)
    │
    ▼
1. Resolve seed paths → track IDs
    │
    ▼
2. HNSW vector search per seed → candidate tracks with distances
    │
    ▼
3. Merge candidates (keep minimum distance per track)
    │
    ▼
4. Compute seed averages (BPM, arousal)
    │
    ▼
5. Score each candidate:
   ┌─────────────────────────────────────────────────────┐
   │ score = w_hnsw    × hnsw_distance                   │
   │       + w_key     × key_penalty                      │
   │       + w_key_dir × key_direction_penalty             │
   │       + w_arousal × arousal_direction_penalty         │
   │       + w_bpm     × bpm_penalty                       │
   └─────────────────────────────────────────────────────┘
    │
    ▼
6. Filter by adaptive harmonic threshold
    │
    ▼
7. Sort by score (lower = better), return top N
```

---

## Stage 1: HNSW Vector Search

### Audio Feature Vector (16 dimensions)

Each track is represented by a 16-dimensional feature vector stored in a CozoDB HNSW index. Features are organized into four groups:

| Group | Dimensions | Features |
|-------|-----------|----------|
| **Rhythm** | 4 | BPM (normalized), BPM confidence, beat strength, rhythm regularity |
| **Harmony** | 4 | Key X (cosine encoding), Key Y (sine encoding), mode (major/minor), harmonic complexity |
| **Energy** | 4 | LUFS (normalized), dynamic range, mean RMS energy, energy variance |
| **Timbre** | 4 | Spectral centroid, spectral bandwidth, spectral rolloff, MFCC flatness |

**Key encoding**: The musical key is encoded as a point on the unit circle using `cos(key × 2π / 12)` and `sin(key × 2π / 12)`, which captures the circular nature of keys (C and B are adjacent, not distant).

### Index Configuration

- **Algorithm**: HNSW (Hierarchical Navigable Small World)
- **Dimensions**: 16
- **Connections per node (m)**: 16
- **Construction ef**: 200
- **Database**: CozoDB (embedded)

### Candidate Merging

Each seed track queries for its nearest neighbors independently. When multiple seeds return the same candidate, the **minimum** (best) distance is kept. Seed tracks themselves are excluded from results.

---

## Stage 2: Key Transition Scoring

### Transition Classification

Every pair of keys maps to one of 14 transition types based on their position on the Camelot wheel:

| Transition | Camelot Steps | Shared Notes | Example |
|-----------|--------------|-------------|---------|
| Same Key | 0, same mode | 7/7 | Am → Am |
| Adjacent Up | +1, same mode | 6/7 | Am → Em |
| Adjacent Down | -1, same mode | 6/7 | Am → Dm |
| Diagonal Up | +1, cross-mode (B→A) | ~5-6/7 | C → Dm |
| Diagonal Down | -1, cross-mode (A→B) | ~5-6/7 | Am → G |
| Energy Boost | +2, same mode | 5/7 | Am → Bm |
| Energy Cool | -2, same mode | 5/7 | Am → Gm |
| Mood Lift | A→B same position | 7/7 | Am → C |
| Mood Darken | B→A same position | 7/7 | C → Am |
| Semitone Up | +7 (wraps to -5) | ~3/7 | Am → Bbm |
| Semitone Down | -7 (wraps to +5) | ~3/7 | Am → Abm |
| Far Step | ±3 to ±5, same mode | 3-4/7 | Am → Cm |
| Far Cross | ±2+, cross-mode | varies | Am → Bb |
| Tritone | ±6 (maximum distance) | ~2/7 | Am → Ebm |

### Base Compatibility Score

At fader center (no energy bias), each transition has a **base score** reflecting pure harmonic compatibility:

| Transition | Base Score | Rationale |
|-----------|-----------|-----------|
| Same Key | 1.00 | Perfect harmonic match |
| Adjacent Up/Down | 0.85 | One shared note difference, very safe |
| Diagonal Up/Down | 0.75 | Cross-mode adds complexity but still related |
| Mood Lift/Darken | 0.70 | All 7 notes shared but tonal center shifts |
| Energy Boost/Cool | 0.50 | Two shared note differences, noticeable change |
| Far Step (3) | 0.25 | Getting risky |
| Semitone Up/Down | 0.20 | Only ~3 shared notes, high harmonic risk |
| Far Step (4) | 0.15 | Quite distant |
| Far Cross | 0.10 | Distant cross-mode |
| Far Step (5) | 0.08 | Very distant |
| Tritone | 0.03 | Maximum dissonance |

### Key Scoring Models

Two algorithms compute the base score:

**Camelot** (default): Hand-tuned scores per transition category as shown above. Predictable and well-understood by DJs.

**Krumhansl**: Uses the Krumhansl-Kessler (1982) probe-tone profiles to compute a 24×24 Pearson correlation matrix between all major and minor keys. This gives continuous perceptual similarity scores rather than discrete categories. Better at rating cross-mode transitions (e.g., C major to A minor = relative key = high correlation) where Camelot uses coarse bucketing.

The model is selectable in **Settings → Display → Key Matching**.

### Energy Modifier

When the energy direction fader is off-center, each transition type receives a **bonus or penalty** to its base score based on its inherent emotional energy direction. This modifies which transitions pass the adaptive filter threshold.

Each transition has a research-calibrated energy direction:

| Transition | Energy Direction | Emotional Effect |
|-----------|-----------------|-----------------|
| Semitone Up | **+0.70** | Visceral pitch lift — strongest energy surge. Classic pop final-chorus key change (Whitney Houston, Michael Jackson). Triggers physiological arousal from pitch increase. |
| Energy Boost | **+0.50** | Dramatic whole-step lift — "hands in the air" moment. Mixed In Key's signature technique. More reliable than semitone (+5/7 vs +3/7 shared notes). |
| Mood Lift | **+0.30** | Minor→major brightening — "the sun coming out." Same 7 notes, but recentered tonal gravity transforms melancholy into celebration. |
| Adjacent Up | **+0.20** | Gentle forward momentum — modulation to the dominant key. The Circle of Fifths' most natural-sounding progression. Bread-and-butter DJ technique. |
| Diagonal Up | **+0.15** | Complex lift — combines energy direction with mood color shift. Sophisticated narrative transition. |
| Same Key | **0.00** | Perfectly neutral — maintains current energy exactly. Maximum harmonic safety. |
| Diagonal Down | **-0.15** | Complex cooldown — energy drop with mood shift. Atmospheric transition. |
| Adjacent Down | **-0.20** | Gentle relaxation — subdominant modulation ("plagal" direction). Associated with rest and resolution. |
| Mood Darken | **-0.30** | Major→minor darkening — shifts from celebration to introspection. Same notes, different emotional gravity. |
| Energy Cool | **-0.50** | Strong energy drain — whole-step descent. Signals lower-energy territory. |
| Semitone Down | **-0.50** | Settling, sinking sensation — dramatic pitch drop. Rare in pop music (downward modulations lack "lift"). |
| Tritone | **-0.80** | Maximum dissonance — the "Devil's Interval." Chaotic, unstable, destructive to dancefloor flow. Only viable as deliberate shock. |

The modifier formula:

```
alignment = energy_direction × sign(energy_bias)
modifier = alignment × |energy_bias| × 0.80
final_score = clamp(base_score + modifier, 0.0, 1.0)
```

At full fader (+1.0), a semitone-up gets `+0.70 × 1.0 × 0.80 = +0.56` bonus, lifting it from 0.20 to 0.76 — competitive with adjacent transitions. An energy-cool transition gets `-0.50 × 1.0 × 0.80 = -0.40` penalty, dropping it from 0.50 to 0.10.

### Adaptive Filter Threshold

Candidates whose best key score falls below a threshold are filtered out entirely. The threshold relaxes as the fader moves to extremes:

| Fader Region | Threshold | What Passes |
|-------------|-----------|-------------|
| Center (< 0.1) | 0.50 | Same key, adjacent, relative, mood, diagonal |
| Moderate (0.1 – 0.4) | 0.35 | + energy boost/cool |
| Strong (0.4 – 0.7) | 0.20 | + semitone, far steps (±3) |
| Extreme (> 0.7) | 0.10 | Nearly everything except tritone at center |

---

## Stage 3: Arousal Direction Penalty

When ML analysis data is available (from EffNet + Jamendo mood model), each track has an **arousal** value (0.0–1.0) representing perceived energy/excitement level.

The arousal direction penalty measures how well a candidate's energy aligns with the fader direction:

```
diff = candidate_arousal - avg_seed_arousal
alignment = diff × energy_bias
penalty = 0.5 - 0.5 × clamp(alignment, -1.0, 1.0)
```

- **Fader center (bias=0)**: All tracks get 0.5 (no differentiation)
- **Fader right (bias=+1.0)**: Tracks with higher arousal than seeds → low penalty (good); lower arousal → high penalty
- **Fader left (bias=-1.0)**: Tracks with lower arousal than seeds → low penalty (good); higher arousal → high penalty

Tracks without arousal data receive a neutral 0.5 penalty — neither boosted nor penalized.

---

## Stage 4: Key Direction Penalty

Separate from the key compatibility score, this term independently rewards transitions whose **emotional energy direction** aligns with the fader. It uses the same energy direction values as the energy modifier but produces a 0-1 penalty for use as its own scoring term:

```
alignment = energy_direction × energy_bias
penalty = 0.5 - 0.5 × clamp(alignment, -1.0, 1.0)
```

Example at full raise (bias=+1.0):
- Semitone Up (dir=+0.70): penalty = 0.5 - 0.5 × 0.70 = **0.15** (excellent)
- Same Key (dir=0.00): penalty = **0.50** (neutral)
- Energy Cool (dir=-0.50): penalty = 0.5 - 0.5 × (-0.50) = **0.75** (poor)

This gives the scoring formula an independent signal about whether the key transition *direction* is emotionally appropriate, separate from whether the key transition is *harmonically safe*.

---

## Stage 5: Unified Scoring Formula

All five terms are combined with dynamic weights that shift as the fader moves:

```
score = w_hnsw    × hnsw_distance
      + w_key     × (1.0 - key_score)
      + w_key_dir × key_direction_penalty
      + w_arousal × arousal_direction_penalty
      + w_bpm     × bpm_penalty
```

Lower score = better match.

### Dynamic Weights

| Term | Center (bias=0) | Extreme (\|bias\|=1) | Purpose |
|------|----------------|---------------------|---------|
| **HNSW distance** | 0.40 | 0.15 | Audio fingerprint similarity (genre, style, timbre) |
| **Key penalty** | 0.25 | 0.25 | Harmonic compatibility — constant because safety always matters |
| **Key direction** | 0.10 | 0.20 | Emotional energy direction of the key change |
| **Arousal** | 0.15 | 0.30 | Perceived energy alignment (from ML analysis) |
| **BPM penalty** | 0.10 | 0.10 | Tempo proximity (10 BPM difference = max penalty) |
| **Total** | **1.00** | **1.00** | |

### Weight Interpolation

Weights interpolate linearly with `|energy_bias|`:

```
w_hnsw    = 0.40 - 0.25 × |bias|
w_key     = 0.25                   (constant)
w_key_dir = 0.10 + 0.10 × |bias|
w_arousal = 0.15 + 0.15 × |bias|
w_bpm     = 0.10                   (constant)
```

### Design Rationale

**Why does HNSW weight decrease at extremes?** HNSW similarity inherently finds tracks that "sound like" the seeds — similar genre, energy, timbre. This is ideal at center (find harmonically compatible, similar tracks) but counterproductive at extremes where the DJ wants *contrasting* energy levels. Reducing HNSW weight lets dissimilar-but-energetically-appropriate tracks surface.

**Why is key weight constant?** Harmonic safety matters regardless of energy direction. A semitone-up transition is always risky during a melodic blend, even when it's the desired energy direction. The energy modifier already adjusts which transitions are *allowed* through the filter; the constant key weight maintains sorting by harmonic safety within each tier.

**Why are there two key-related terms?** They solve different problems:
- `w_key × key_penalty` — **gating**: should this transition be considered at all? (harmonic compatibility)
- `w_key_dir × key_direction_penalty` — **ranking**: among compatible transitions, which direction does the DJ want? (emotional energy)

A transition can be harmonically safe (low key_penalty) but in the wrong energy direction (high key_dir_penalty), or vice versa.

---

## Reason Tags

Each suggestion displays a colored tag pill showing the key relationship:

### Direction Symbols

| Symbol | Meaning |
|--------|---------|
| **▲** | Transition moves clockwise on the Camelot wheel (raises musical tension) |
| **▼** | Transition moves counter-clockwise (releases musical tension) |
| **━** | Same key (no movement) |

### Tag Labels

| Tag | Transition Type |
|-----|----------------|
| ━ Same Key | Same key |
| ▲ Adjacent / ▼ Adjacent | ±1 step, same mode |
| ▲ Diagonal / ▼ Diagonal | ±1 step, cross-mode (safe diagonal) |
| ▲ Boost | +2 steps, same mode |
| ▼ Cool | -2 steps, same mode |
| ▲ Mood Lift | Minor → major, same position |
| ▼ Darken | Major → minor, same position |
| ▲ Semitone / ▼ Semitone | ±7 steps (one semitone pitch change) |
| ▲/▼ Far | ±3-5 steps, same mode |
| ▼ Tritone | ±6 steps (maximum dissonance) |

### Color Coding

Traffic-light colors indicate harmonic compatibility:

| Color | Key Score | Meaning |
|-------|----------|---------|
| Green (#2d8a4e) | ≥ 0.70 | Excellent — harmonically safe |
| Amber (#c49a2a) | ≥ 0.40 | Acceptable — use with care |
| Red (#a63d40) | < 0.40 | Risky — dramatic effect only |

---

## Parameters Reference

### Query Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `per_seed_limit` | 50 | Maximum HNSW results per seed track |
| `total_limit` | 30 | Maximum suggestions returned |
| `energy_direction` | 0.5 | Fader position: 0.0 (max drop) → 0.5 (center) → 1.0 (max raise) |
| `key_scoring_model` | Camelot | Key scoring algorithm (Camelot or Krumhansl) |

### Internal Constants

| Constant | Value | Description |
|----------|-------|-------------|
| Energy bias | `(energy_direction - 0.5) × 2` | Maps fader [0,1] to bias [-1,+1] |
| BPM penalty scale | 10 BPM | BPM difference that produces maximum penalty |
| Default seed arousal | 0.5 | Fallback when no ML arousal data exists |
| Default seed BPM | 128.0 | Fallback when seed tracks have no BPM |
| Neutral key score | 0.3 | Score for tracks with no key metadata |

---

## Source Files

| File | Contents |
|------|----------|
| `crates/mesh-player/src/suggestions.rs` | Scoring engine, transition classification, key models |
| `crates/mesh-core/src/db/schema.rs` | AudioFeatures struct, HNSW index definition |
| `crates/mesh-core/src/db/queries.rs` | SimilarityQuery (HNSW search) |
| `crates/mesh-core/src/db/service.rs` | `find_similar_tracks()`, `get_arousal_batch()` |
| `crates/mesh-core/src/music.rs` | `MusicalKey`, Camelot wheel encoding |
| `crates/mesh-cue/src/ml_analysis/` | ML pipeline (EffNet, arousal/genre classification) |
