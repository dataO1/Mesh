# Smart Track Suggestions — Research & Design Document

## Executive Summary

Mesh has a production-ready neural suggestion system using 1280-dim Discogs-EffNet
embeddings, Goldilocks HNSW similarity, harmonic scoring, and aggression-based energy
direction. The system is now user-configurable via four settings (Sound Target, Sound
Focus, Key Filter, Stem Complement), and is being evaluated through a planned feedback
collection mechanism.

---

## 1. Original Infrastructure (Pre-Neural)

### 1.1 Existing 16-Dimensional Feature Vector

Location: `crates/mesh-cue/src/features/extraction.rs`, stored in `audio_features` CozoDB relation.

| Category | Dimensions | Features |
|----------|-----------|----------|
| **Rhythm** (4) | bpm_normalized | `(bpm - 60) / 140` |
| | bpm_confidence | Detection confidence |
| | beat_strength | From `Danceability` algo |
| | rhythm_regularity | From `RhythmDescriptors.first_peak_weight` |
| **Harmony** (4) | key_x, key_y | Circular encoding on 12-semitone wheel |
| | mode | 1.0=major, 0.0=minor |
| | harmonic_complexity | From `SpectralComplexity` |
| **Energy** (4) | lufs_normalized | EBU R128 loudness |
| | dynamic_range | From `DynamicComplexity` |
| | energy_mean | Mean RMS per 1s segments |
| | energy_variance | Energy volatility |
| **Timbre** (4) | spectral_centroid | Brightness |
| | spectral_bandwidth | Frequency spread |
| | spectral_rolloff | High-frequency content |
| | mfcc_flatness | Noisiness vs tonality |

**HNSW Index**: dim=16, m=16, ef_construction=200, Cosine distance.
Kept as silent fallback for tracks not yet re-analysed with EffNet.

### 1.2 Existing Harmonic Mixing

Location: `crates/mesh-core/src/music/mod.rs`

- `MusicalKey` struct with `root` (0-11) and `minor` (bool)
- Camelot wheel conversion (position 1-12, A/B letter)
- Relative key detection (Am <-> C major)
- Semitone distance calculation
- `is_harmonically_compatible()` check

### 1.3 CozoDB Capabilities for Recommendations

#### Vector Search (HNSW)

Already implemented. Key capabilities:

```datalog
// Find 10 tracks similar to seed track
?[dist, track_id, title] :=
    *tracks{track_id: $seed_id, embedding: v},
    ~tracks:audio_sim{
        track_id, title |
        query: v, k: 10, ef: 50,
        bind_distance: dist
    }
```

**In-search filtering** (scalar constraints during HNSW traversal):
```datalog
~tracks:audio_sim{
    track_id, title, bpm |
    query: v, k: 20, ef: 100,
    bind_distance: dist,
    filter: bpm >= seed_bpm * 0.95 && bpm <= seed_bpm * 1.05
}
```

#### Graph Queries

**Built-in graph algorithms** (with `graph-algo` feature):
- CommunityDetectionLouvain — find clusters of similar tracks
- PageRank — track importance in the "played together" graph
- ShortestPath — minimum-cost path between tracks
- RandomWalk — stochastic playlist generation
- ConnectedComponents — identify isolated track groups

**Unique feature**: CozoDB exposes the **HNSW proximity graph itself** as a queryable
relation. Community detection can run directly on the audio similarity graph:
```datalog
?[community, track_id] <~ CommunityDetectionLouvain(
    *tracks:audio_sim[fr_track_id, to_track_id, dist],
    undirected: true
)
```

#### Hybrid Queries (Vector + Scalar + Graph)

The killer feature: combine all three in a single Datalog query:

```datalog
// Tracks commonly played after seed (graph signal) + audio similarity
graph_candidates[track_id, play_count] :=
    *played_together{track_a: $seed_id, track_b: track_id, count: play_count},
    play_count >= 3

?[score, track_id, title, play_count] :=
    *tracks{track_id: $seed_id, embedding: v},
    graph_candidates[track_id, play_count],
    *tracks{track_id, title, embedding: v2},
    dist = cos_dist(v, v2),
    score = play_count * 0.3 + (1.0 - dist) * 0.7
:order -score
:limit 10
```

---

## 2. Audio Embedding Models — Evaluation

### 2.1 Neural Embedding Models (Deep Learning)

| Model | Dims | Compute | Music-Specific | ONNX | Best For |
|-------|------|---------|---------------|------|----------|
| **Discogs-EffNet** | 1280 | Low | Yes (400 styles) | Yes | Genre/style similarity ✅ chosen |
| **MAEST** | 768-2304 | Medium | Yes (transformer) | Yes | Multi-scale temporal |
| **MSD-MusiCNN** | 200 | Very Low | Yes (790k params) | Convertible | Timbre/temporal |
| **VGGish** | 128 | Low | No (general) | Yes | General audio |
| **OpenL3** | 512/6144 | Low-Med | Yes (music mode) | Convertible | Acoustic texture |
| **CLAP** | 512 | Medium | Partial | Yes | Text-based search |
| **MERT** | 768 | High | Yes (strong) | Convertible | Pitch/harmony/rhythm |
| **PANNs CNN14** | 2048 | Medium | No (general) | Yes | General audio |

**Chosen: Discogs-EffNet**
- 1280-dim embeddings trained on 400 Discogs music styles via contrastive learning
- ONNX model directly available: `discogs-effnet-bsdynamic-1.onnx`
- EfficientNet architecture = low compute cost
- Embedding space naturally clusters by genre/subgenre

**EffNet cosine distance empirical zones:**
- `d < 0.15`: near-identical recordings, remixes, alternate masters
- `d ≈ 0.20–0.40`: same subgenre, shared production aesthetic (DJ sweet spot)
- `d ≈ 0.40–0.60`: genre-adjacent, shared era/energy
- `d > 0.65`: cross-genre

### 2.2 Integration Path: ONNX in Rust

**`ort` crate** (v2.0.0-rc.11):
- Wraps ONNX Runtime v1.24.1
- Execution providers: CPU, CUDA, TensorRT, CoreML, DirectML
- Used by Google Magika, HuggingFace Text Embeddings Inference

---

## 3. Real-World DJ Software Approaches

### 3.1 Commercial Software

| Software | Feature | Approach |
|----------|---------|----------|
| **Rekordbox** (Pioneer) | Related Tracks | Key + BPM + genre tag matching |
| **djay Pro** (Algoriddim) | AI Match | Neural audio analysis, "Automix AI" |
| **VirtualDJ** | Smart Suggestions | BPM + key + genre + energy matching |
| **Mixed In Key** | Energy Level System | 1-10 energy scale + Camelot key |
| **Serato** | SmartCrates | Metadata rule-based filtering |
| **Beatport** | Recommendations | Collaborative filtering + audio features |
| **DJ.Studio** | Auto-mix | Harmonic + tempo + energy curve planning |

### 3.2 Spotify

Spotify's core is **collaborative filtering** — embedding tracks into a latent space where
proximity reflects co-listening behavior (people who played X also played Y). Audio CNNs
are used only for **cold-start** (new/obscure tracks with no listen history). Their audio
model is similar in spirit to EffNet but trained end-to-end on engagement signals (saves,
playlist adds, skip rate), not genre classification.

Key insight: **their metric is re-listen probability, not transition smoothness**. Their
"audio2vec"/"track2vec" embeddings are learned from *user behavior*. We have acoustic
content (EffNet) but no behavior data — which is why the feedback collection system is the
right direction.

### 3.3 Academic DJ Research

Several papers (notably DJMD 2019, automatic DJ systems at ISMIR) converge on a similar
feature set for transition quality:
1. **Beat alignment** — grid phase match
2. **Harmonic compatibility** — Camelot/circle of fifths
3. **Timbral similarity** — CNN embeddings (same as EffNet)
4. **Energy curve** — LUFS/arousal trajectory

The ML-based ones find that **timbral similarity + harmonic compatibility** are the strongest
predictors of a "good transition" rated by human DJs. **BPM** is a **hygiene factor**
(must be within ±8%) not a scoring signal. **Mood/aggression** features appear in none of
the well-performing systems as primary signals — they are typically used only when no
audio embedding is available.

### 3.4 Camelot Wheel Reference

| Code | Minor Key | | Code | Major Key |
|------|-----------|---|------|-----------|
| 1A | Abm | | 1B | B |
| 2A | Ebm | | 2B | F#/Gb |
| 3A | Bbm | | 3B | Db |
| 4A | Fm | | 4B | Ab |
| 5A | Cm | | 5B | Eb |
| 6A | Gm | | 6B | Bb |
| 7A | Dm | | 7B | F |
| 8A | Am | | 8B | C |
| 9A | Em | | 9B | G |
| 10A | Bm | | 10B | D |
| 11A | F#m | | 11B | A |
| 12A | C#m/Dbm | | 12B | E |

---

## 4. Research Papers & Notable Implementations

| Paper | Year | Key Contribution |
|-------|------|-----------------|
| **"Automatic DJ Transitions with Differentiable Audio Effects and GANs"** (Chen et al.) | ICASSP 2022 | GAN-based transition generation using differentiable EQ + fader |
| **"Cue Point Estimation using Object Detection"** (Argüello et al.) | 2024 | Object detection transformer for DJ cue points, 21k annotated dataset |
| **"Zero-shot DJ Tool Retrieval"** | 2024 | CLAP embeddings for classifying vocal hooks, drum breaks, etc. |
| **"Enhancing Sequential Music Recommendation with Personalized Popularity Awareness"** | RecSys 2024 | Transformer + popularity awareness, 25-70% improvement |
| **Discogs-EffNet** (MTG/UPF) | — | EfficientNet trained on 400 Discogs styles, contrastive learning |
| **MAEST** (MTG/UPF) | — | Music Audio Efficient Spectrogram Transformer, multi-scale |
| **CLAP** (LAION/Microsoft) | — | Contrastive Language-Audio Pretraining, text-audio shared space |

---

## 5. Current Scoring System (as implemented, v0.9.13+)

### 5.1 HNSW Routing

Primary: EffNet 1280-dim HNSW (`ml_embeddings:similarity_index`, Cosine, m=32, ef=300).
Fallback (silent, for tracks not yet re-analysed): 16-dim audio features HNSW.

### 5.2 Simplified Weight Formula

Removed components after analysis: BPM, production, danceability, approachability,
contrast/timbre. These were found to be either double-counted by HNSW or insufficiently
predictive relative to their weight budget. The weight set is now:

| Component | Formula | Center (bias=0) | Extreme (|bias|=1) | Notes |
|-----------|---------|----------------|-------------------|-------|
| `w_hnsw` | see below | 0.58 (stem off) / 0.33 (stem on) | 0.40 | Self-normalizing remainder |
| `w_key` | 0.30 | 0.30 | 0.30 | Constant — harmonic quality never trades off |
| `w_key_dir` | 0.12 − 0.07·b | 0.12 | 0.05 | Key energy direction match |
| `w_aggression` | 0.25·b | 0.00 | 0.25 | Genre-normalized energy direction |
| `w_vocal_compl` | 0.15·(1−b) if on | 0.15 | 0.00 | Stem complement — vocal |
| `w_other_compl` | 0.10·(1−b) if on | 0.10 | 0.00 | Stem complement — melodic/lead |

`w_hnsw` is computed as `1.0 − w_key − w_key_dir − w_aggression − w_vocal − w_other`
so the sum is always exactly 1.00 regardless of stem complement toggle state.

**Budget verification:**
- stem OFF, center: 0.58 + 0.30 + 0.12 = **1.00** ✓
- stem ON,  center: 0.33 + 0.30 + 0.12 + 0.15 + 0.10 = **1.00** ✓
- either,   extreme: 0.40 + 0.30 + 0.05 + 0.25 = **1.00** ✓

### 5.3 Goldilocks HNSW Formula (Configurable)

```
gold_target  (from SuggestionSimilarityTarget)  default 0.35
gold_sigma2  (from SuggestionSimilarityFocus)   default 0.08 (2σ², σ=0.20)

goldilocks     = exp(−(norm_dist − gold_target)² / gold_sigma2)
hnsw_component = goldilocks × (1 − bias_abs) + (1 − norm_dist) × bias_abs
```

At center, rewards tracks at `gold_target` normalized distance from the seed. At extremes,
blends fully to `1 − norm_dist` (diversity reward) for transition mode.

**SuggestionSimilarityTarget values:**
| Setting | gold_target | EffNet zone |
|---------|------------|-------------|
| Tight    | 0.20 | Near-clone, same subgenre |
| Balanced | 0.35 | Same genre, different texture (default) |
| Wide     | 0.45 | Genre-adjacent |
| Open     | 0.55 | Cross-genre |

**SuggestionSimilarityFocus values:**
| Setting | gold_sigma2 | σ | Effect |
|---------|-----------|-----|--------|
| Sharp  | 0.02 | ≈0.10 | Tight bell, strongly rewards the exact target distance |
| Normal | 0.08 | ≈0.20 | Balanced (default) |
| Broad  | 0.16 | ≈0.28 | Wide plateau, forgiving of distance variation |

### 5.4 Harmonic Filter (Configurable)

The dual-layer harmonic gate uses thresholds from `SuggestionKeyFilter`:

| Setting | harmonic_floor | blended_threshold | Effect |
|---------|---------------|-------------------|--------|
| Strict  | 0.45 | 0.65 | Blocks Semitone, FarStep, FarCross, Tritone (default) |
| Relaxed | 0.20 | 0.45 | Allows semitone and cross-key moves for atonal/mashup |
| Off     | 0.00 | 0.00 | No filter — all key relationships scored |

Preferred tracks (user-curated playlist) receive 50% leniency on the blended threshold
layer only (not the base floor).

### 5.5 Stem Complement Formula

Bipolar scoring normalized to [0, 1]:

```
stem_complement(seed, cand) = (|seed − cand| − min(seed, cand) + 1) / 2
```

| seed | cand | result | meaning |
|------|------|--------|---------|
| 1.0 | 0.0 | 1.0 | fully complementary → max boost |
| 0.0 | 1.0 | 1.0 | fully complementary → max boost |
| 1.0 | 1.0 | 0.0 | clashing → max penalty |
| 0.0 | 0.0 | 0.5 | both silent → neutral |

Applied independently to vocal and "other" (melody/lead) stems. Fades to zero at extreme
bias. Toggleable via Settings — default on.

**Status: unvalidated.** The hypothesis (fill vocal/melodic gaps) is musically plausible
but RMS energy density is a coarse proxy. Many great mashups layer two vocal tracks
intentionally. Feedback data will determine whether this component helps or hurts.

### 5.6 TransitionType Scores and Energy Directions

```
SameKey     base=1.00  energy=0.00
AdjacentUp  base=0.85  energy=+0.20
AdjacentDn  base=0.85  energy=−0.20
DiagonalUp  base=0.75  energy=+0.15
DiagonalDn  base=0.75  energy=−0.15
MoodLift    base=0.70  energy=+0.30
MoodDarken  base=0.70  energy=−0.30
EnergyBoost base=0.50  energy=+0.50
EnergyCool  base=0.50  energy=−0.50
SemitoneUp  base=0.20  energy=+0.70
SemitoneDown base=0.20 energy=−0.50
FarStep(±n) base=0.25/0.15/0.08/0.05  energy=±0.10
FarCross    base=0.10  energy=±0.05
Tritone     base=0.03  energy=−0.80
```

---

## 6. User-Configurable Parameters

All settings live in Settings › Browser › Suggestions.

| Setting | Type | Options | What it controls |
|---------|------|---------|-----------------|
| Sound Target | Button group (4) | Tight / Balanced / Wide / Open | Center of Goldilocks bell (GOLD_TARGET) |
| Sound Focus | Button group (3) | Sharp / Normal / Broad | Width of Goldilocks bell (GOLD_SIGMA2) |
| Key Filter | Button group (3) | Strict / Relaxed / Off | Harmonic floor + blended threshold |
| Stem Complement | Toggle | on/off | Enable vocal/lead gap-fill scoring |
| Playlist Split | Toggle | on/off | 15 playlist + 15 global vs 30 any |

**Design rationale for the choices made:**

- **BPM removed**: Modern DJs pitch-correct freely; ±15% BPM is trivially matched. At 13%
  weight it excluded excellent spectral candidates. Reduced to zero.
- **Production/Timbre removed**: Double-counted by HNSW. Minimal unique signal.
- **Dance/Approach/Contrast removed**: Small weights (0.01–0.05) at extremes only; three
  ML heads from non-DJ-context models summing to 8% of the score. Noise floor.
- **Aggression raised to 0.25 at extreme**: Stronger energy-direction signal for
  transition mode (was 0.15). Linearly scales from 0 at center.
- **Harmonic key kept constant at 0.30**: Harmonic quality is always relevant. The
  original design rationale stands.

---

## 7. Signal Noise Assessment

Ordered by confidence that the signal is primarily noise:

**High confidence noise (removed):**
- BPM penalty — hygiene factor, not a scoring dimension
- Production match — double-counted by EffNet HNSW
- Danceability/approachability/contrast at extremes — non-DJ-context ML heads at small weights

**Uncertain (kept, needs feedback data):**
- Stem complement (vocal/other) — theoretically motivated, empirically unvalidated
- Aggression at extremes — increased weight but still from a non-DJ-context model

**Likely signal (kept):**
- EffNet HNSW distance — strongest signal; validated by academic research
- Key transition score — industry standard; validated by all commercial DJ software
- Key direction (energy direction via Camelot) — validated by energy arc research

---

## 8. Planned Feedback Collection System

### 8.1 What to record

For each suggestion selection event:

```
suggestion_feedback {
  session_id:       String    -- groups events within one session
  timestamp:        Int       -- unix seconds
  energy_bias:      Float     -- fader position at time of selection (−1.0..1.0)

  -- Track identifiers (for manual review)
  seed_title:       String    -- seed track title
  seed_artist:      String    -- seed track artist
  selected_title:   String    -- selected suggestion title
  selected_artist:  String    -- selected suggestion artist

  -- Acoustic relationship
  hnsw_dist_norm:   Float     -- normalized EffNet cosine distance (0..1)
  key_relation:     String    -- TransitionType name ("Adjacent", "SameKey", etc.)
  key_score:        Float     -- blended harmonic score (0..1)
  bpm_ratio:        Float     -- candidate_bpm / seed_bpm

  -- Component scores at time of selection
  score_hnsw:       Float
  score_key:        Float
  score_aggression: Float
  score_stem:       Float     -- (vocal + other combined)
  score_total:      Float

  -- Stem densities (for complement validation)
  seed_vocal_den:   Float
  seed_other_den:   Float
  cand_vocal_den:   Float
  cand_other_den:   Float

  -- Algorithm config snapshot (for cross-config comparisons)
  config_gold_target:  Float
  config_gold_sigma2:  Float
  config_key_filter:   String
  config_stem_on:      Bool
}
```

### 8.2 Signals NOT recorded

- Negative signals (session ended without loading a suggestion) — too ambiguous
- Skipped position in list — user browses freely, rank ≠ preference
- Playback duration after load — out of scope for now

### 8.3 Analysis questions the data answers

1. **Is GOLD_TARGET calibrated?** Histogram of `hnsw_dist_norm` for selected tracks.
   Peak should be at 0.35 (Balanced). If it's at 0.20 or 0.50, adjust the default.

2. **Does BPM matter?** Correlation between `abs(bpm_ratio − 1.0)` and selection. If
   selected tracks uniformly span ±15% BPM, the removal was correct.

3. **Does stem complement predict selection?** Compare `score_stem` distribution for
   selected vs non-selected candidates at the same fader position. If distributions
   overlap, the feature is noise.

4. **Is aggression predictive at extremes?** Filter `|energy_bias| > 0.7`, check if
   `score_aggression` correlates with selection.

5. **Is the harmonic floor too strict?** What fraction of sessions used Key Filter:
   Strict but switched to Relaxed? User migration pattern is its own signal.

### 8.4 Implementation plan

Storage: CozoDB relation in the mesh-collection database. One row per selection.
After 200–500 events (~5–10 active sessions), run correlation analysis.
Export via `:put suggestion_feedback` CSV for external analysis if needed.

---

## 9. Storage Estimates

| Data | Size per Track | 50k Tracks |
|------|---------------|-----------|
| 16-dim features (existing) | 64 bytes | 3.2 MB |
| 1280-dim Discogs-EffNet | 5,120 bytes | 256 MB |
| HNSW index overhead (~4x) | ~20 KB | ~1 GB |
| Metadata (BPM, key, etc.) | ~200 bytes | 10 MB |
| Transition graph edges | ~50 bytes/edge | ~50 MB (1M edges) |
| **Total** | | **~1.3 GB** |

HNSW search at 50k tracks with 1280 dims: ~5–10ms.

---

## 10. Implementation Phase Status

### Phase 1: Activate Existing Infrastructure ✅ Complete
- [x] Wire up `SimilarityQuery::find_similar()` to a UI suggestion panel
- [x] Add "Suggest Similar" button/MIDI trigger
- [x] Basic results: similar tracks by 16-dim features + BPM + key filter

### Phase 2: Neural Embeddings ✅ Complete
- [x] Add `ort` dependency, download Discogs-EffNet ONNX model
- [x] Add mel spectrogram preprocessing (128 mel bands)
- [x] Compute 1280-dim embeddings during import
- [x] Create second HNSW index (dim=1280, m=32, ef=300)
- [x] Update suggestion query to use neural embeddings as primary ranker
- [x] 16-dim vector kept as silent fallback

### Phase 3: Smart Ranking ✅ Complete
- [x] Composite scoring function — unified intent slider
- [x] Goldilocks HNSW: rewards "close but not clone" at center
- [x] Stem complement scoring (vocal/melodic gap fill)
- [x] Dual harmonic filter: permanent floor + energy-blended threshold
- [x] Krumhansl–Kessler perceptual key model as alternative to Camelot
- [x] Genre-normalized aggression scoring
- [x] Suggestion reason tags with color-coded pills

### Phase 4: User Configurability ✅ Complete
- [x] Sound Target (Goldilocks center): Tight / Balanced / Wide / Open
- [x] Sound Focus (bell width): Sharp / Normal / Broad
- [x] Key Filter: Strict / Relaxed / Off
- [x] Stem Complement toggle
- [x] Playlist Split toggle
- [x] Simplified weight set (removed BPM, production, dance, approach, contrast)
- [x] Aggression raised to 0.25 at extremes

### Phase 5: Feedback & Calibration (Planned)
- [ ] `suggestion_feedback` CozoDB relation
- [ ] Record selection events (seed/candidate identities + scores + config)
- [ ] Correlation analysis: which components predict selection?
- [ ] Adjust GOLD_TARGET default based on actual selection histograms
- [ ] Validate or remove stem complement based on data

### Phase 6: Advanced Features (Future)
- [ ] Community detection on similarity graph (auto-genre clusters)
- [ ] CLAP text embeddings for natural language search ("dark minimal techno")
- [ ] Set position awareness (warm-up vs peak vs cool-down)
- [ ] MinHash-LSH for remix/edit detection
- [ ] PCA reduction of EffNet 1280-dim → ~200 dims before HNSW (noise reduction)

---

## 11. Key Resources

### Models
- Essentia Models: https://essentia.upf.edu/models.html
- Discogs-EffNet ONNX: https://essentia.upf.edu/models.html (discogs-effnet-bsdynamic-1.onnx)
- CLAP (LAION): https://github.com/LAION-AI/CLAP
- MERT: https://huggingface.co/m-a-p/MERT-v1-330M

### Crates
- ort (ONNX Runtime): https://github.com/pykeio/ort
- mel_spec: https://crates.io/crates/mel_spec
- bliss-audio: https://github.com/Polochon-street/bliss-rs
- rusty-chromaprint: https://github.com/darksv/rusty-chromaprint
- tract: https://github.com/sonos/tract
- candle: https://github.com/huggingface/candle

### CozoDB
- Vector search docs: https://docs.cozodb.org/en/latest/vector.html
- Graph algorithms: https://docs.cozodb.org/en/latest/algorithms.html

### Research
- Music Collection Analyzer (Essentia embeddings): https://github.com/Masetto96/music-collection-analyzer
- Cue Point Estimation (2024): https://arxiv.org/abs/2407.06823
- Auto DJ Transitions (ICASSP 2022): https://arxiv.org/abs/2110.06525
- Sequential Music Recommendation: https://arxiv.org/html/2409.04329v1
- Spotify User Embeddings: https://research.atspotify.com/contextual-and-sequential-user-embeddings-for-music-recommendation/
