# Smart Suggestions — Similarity Search & Scoring

Technical documentation for Mesh's track recommendation system. This system powers the suggestion panel in mesh-player's collection browser.

---

## Overview

When suggestions are enabled, Mesh analyzes the tracks loaded on your decks and recommends what to play next. The system combines **audio fingerprint similarity** (HNSW vector search), **harmonic compatibility** (Camelot/Krumhansl key analysis), **ML-derived musical characteristics** (danceability, approachability, tonal/timbre contrast), and **tempo proximity** into a single score per candidate track. An energy direction fader lets you steer results toward higher-energy or cooler tracks.

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
4. Compute seed averages (BPM, danceability, approachability, timbre, tonal)
    │
    ▼
5. Score each candidate:
   ┌──────────────────────────────────────────────────────────┐
   │ score = w_hnsw     × hnsw_distance                       │
   │       + w_key      × key_penalty                          │
   │       + w_key_dir  × key_direction_penalty                │
   │       + w_bpm      × bpm_penalty                          │
   │       + w_dance    × danceability_direction_penalty        │
   │       + w_approach × approachability_direction_penalty     │
   │       + w_contrast × tonal_timbre_contrast_penalty         │
   └──────────────────────────────────────────────────────────┘
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

### Search Scope

The HNSW query searches the **entire collection** (k=10,000, effectively unlimited for DJ libraries). The search beam width (`ef`) is set to match `k` for full recall. This ensures the scoring pipeline has maximum diversity in the candidate pool — the energy fader can steer results toward any track in the collection, not just the closest HNSW neighbors.

### Candidate Merging

Each seed track queries the full collection independently. When multiple seeds return the same candidate, the **minimum** (best) distance is kept. Seed tracks themselves are excluded from results.

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

## Stage 3: ML Score Penalties

When ML analysis data is available (from EffNet classification heads), each track has **danceability**, **approachability**, **timbre** (brightness), and **tonal** (tonality) scores — all 0.0–1.0 floats. These feed three penalty terms that activate proportionally with fader movement.

### ML Scores (from EffNet)

| Score | Range | Meaning |
|-------|-------|---------|
| **Danceability** | 0.0–1.0 | How danceable the track is |
| **Approachability** | 0.0–1.0 | How accessible/approachable the music is |
| **Timbre** | 0.0–1.0 | Brightness: 0.0 = dark, 1.0 = bright |
| **Tonal** | 0.0–1.0 | Tonality: 0.0 = atonal, 1.0 = tonal |

These are batch-fetched via `get_ml_scores_batch()` for all candidate + seed track IDs.

### Seed Averages

For each ML score, the **average across seed tracks** is computed (with 0.5 fallback when no data exists). This provides the baseline against which candidates are compared.

### Direction Penalty (Danceability & Approachability)

Both danceability and approachability use the same **direction penalty** formula. When raising energy (positive bias), candidates with higher values than the seed average get lower penalties; when dropping energy, lower values are preferred:

```
direction_penalty(cand_val, seed_avg, energy_bias) =
    clamp(0.5 - (cand_val - seed_avg) × energy_bias, 0.0, 1.0)
```

- **Fader center (bias=0)**: All tracks get 0.5 (no differentiation)
- **Fader right (bias=+1.0)**: Tracks with higher values → low penalty (good)
- **Fader left (bias=-1.0)**: Tracks with lower values → low penalty (good)

Tracks without ML data use 0.5 as `cand_val`, producing a neutral 0.5 penalty.

### Contrast Penalty (Timbre & Tonal)

At energy extremes, DJs often want **contrasting** characteristics — follow a dark, atonal track with a bright, tonal one for maximum impact. The contrast penalty rewards candidates whose timbre and tonal scores differ from the seed averages:

```
contrast_penalty(cand_timbre, cand_tonal, seed_timbre, seed_tonal) =
    1.0 - (|seed_timbre - cand_timbre| + |seed_tonal - cand_tonal|) / 2.0
```

Returns 0.0 (maximum contrast = best) to 1.0 (identical = worst). The weight itself (`w_contrast = 0.12 × |bias|`) is zero at center, so this term only matters when the fader is off-center.

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

All seven terms are combined with dynamic weights that shift as the fader moves:

```
score = w_hnsw     × hnsw_distance
      + w_key      × (1.0 - key_score)
      + w_key_dir  × key_direction_penalty
      + w_bpm      × bpm_penalty
      + w_dance    × danceability_direction_penalty
      + w_approach × approachability_direction_penalty
      + w_contrast × tonal_timbre_contrast_penalty
```

Lower score = better match.

### Dynamic Weights

| Term | Center (bias=0) | Extreme (\|bias\|=1) | Purpose |
|------|----------------|---------------------|---------|
| **HNSW distance** | 0.45 | 0.00 | Audio fingerprint similarity — drops to zero at extremes |
| **Key penalty** | 0.25 | 0.25 | Harmonic compatibility — constant because safety always matters |
| **Key direction** | 0.15 | 0.25 | Emotional energy direction of the key change |
| **BPM penalty** | 0.15 | 0.10 | Tempo proximity (10 BPM difference = max penalty) |
| **Danceability** | 0.00 | 0.15 | Danceability alignment with fader direction |
| **Approachability** | 0.00 | 0.13 | Music approachability alignment with fader direction |
| **Tonal/timbre contrast** | 0.00 | 0.12 | Rewards opposite tonal + timbre characteristics |
| **Total** | **1.00** | **1.00** | |

### Weight Interpolation

Weights interpolate linearly with `|energy_bias|`:

```
w_hnsw     = 0.45 - 0.45 × |bias|     // 0.45 → 0.00
w_key      = 0.25                       // constant
w_key_dir  = 0.15 + 0.10 × |bias|     // 0.15 → 0.25
w_bpm      = 0.15 - 0.05 × |bias|     // 0.15 → 0.10
w_dance    = 0.15 × |bias|             // 0.00 → 0.15
w_approach = 0.13 × |bias|             // 0.00 → 0.13
w_contrast = 0.12 × |bias|             // 0.00 → 0.12
```

At center (bias=0): all three new weights are zero, producing identical behavior to the original 4-factor formula.

### Design Rationale

**Why does HNSW weight drop to zero at extremes?** HNSW similarity inherently finds tracks that "sound like" the seeds — similar genre, energy, timbre. This is ideal at center (find harmonically compatible, similar tracks) but counterproductive at extremes where the DJ wants *contrasting* energy levels. At full fader, the user is explicitly asking for energy-directed tracks, not just similar-sounding ones. The freed weight budget flows into the new ML signals that better capture the user's intent.

**Why is key weight constant?** Harmonic safety matters regardless of energy direction. A semitone-up transition is always risky during a melodic blend, even when it's the desired energy direction. The energy modifier already adjusts which transitions are *allowed* through the filter; the constant key weight maintains sorting by harmonic safety within each tier.

**Why are there two key-related terms?** They solve different problems:
- `w_key × key_penalty` — **gating**: should this transition be considered at all? (harmonic compatibility)
- `w_key_dir × key_direction_penalty` — **ranking**: among compatible transitions, which direction does the DJ want? (emotional energy)

A transition can be harmonically safe (low key_penalty) but in the wrong energy direction (high key_dir_penalty), or vice versa.

**Why contrast instead of direction for timbre/tonal?** Unlike danceability (where "more = higher energy" is intuitive), timbre and tonality don't have a linear energy axis. Instead, the DJ value at extremes is *variety* — following a dark, atonal breakdown with a bright, tonal peak. The contrast penalty rewards this without imposing a fixed "bright = more energy" assumption.

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
| `per_seed_limit` | 10,000 | HNSW candidates per seed (effectively entire collection) |
| `total_limit` | 30 | Maximum suggestions returned after scoring |
| `energy_direction` | 0.5 | Fader position: 0.0 (max drop) → 0.5 (center) → 1.0 (max raise) |
| `key_scoring_model` | Camelot | Key scoring algorithm (Camelot or Krumhansl) |

### Internal Constants

| Constant | Value | Description |
|----------|-------|-------------|
| Energy bias | `(energy_direction - 0.5) × 2` | Maps fader [0,1] to bias [-1,+1] |
| BPM penalty scale | 10 BPM | BPM difference that produces maximum penalty |
| Default seed ML scores | 0.5 | Fallback when no ML data exists (danceability, approachability, timbre, tonal) |
| Default seed BPM | 128.0 | Fallback when seed tracks have no BPM |
| Neutral key score | 0.3 | Score for tracks with no key metadata |
| Neutral candidate ML | 0.5 | Fallback `cand_val` when candidate has no ML data |

---

## Source Files

| File | Contents |
|------|----------|
| `crates/mesh-player/src/suggestions.rs` | Scoring engine, transition classification, key models |
| `crates/mesh-core/src/db/schema.rs` | AudioFeatures struct, HNSW index definition |
| `crates/mesh-core/src/db/queries.rs` | SimilarityQuery (HNSW search) |
| `crates/mesh-core/src/db/service.rs` | `find_similar_tracks()`, `get_ml_scores_batch()`, `MlScores` |
| `crates/mesh-core/src/music.rs` | `MusicalKey`, Camelot wheel encoding |
| `crates/mesh-cue/src/ml_analysis/` | ML pipeline (EffNet, arousal/genre classification) |
