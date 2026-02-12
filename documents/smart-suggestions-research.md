# Smart Track Suggestions System — Research Report

## Executive Summary

Mesh already has a **production-ready foundation** for smart track suggestions:
- 16-dimensional audio feature vectors extracted via Essentia
- HNSW vector index in CozoDB with cosine similarity search
- Camelot wheel key compatibility logic
- BPM detection and normalization
- Graph relation tables defined (similar_to, harmonic_match, played_after) but **not yet populated**
- Database query infrastructure (SimilarityQuery) ready but unused in UI

The gap is: building the **recommendation engine** on top of this infrastructure, adding **richer embeddings** (neural), and creating the **UI/UX** for suggestion triggers.

---

## 1. What We Already Have

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

### 1.2 Existing Harmonic Mixing

Location: `crates/mesh-core/src/music/mod.rs`

- `MusicalKey` struct with `root` (0-11) and `minor` (bool)
- Camelot wheel conversion (position 1-12, A/B letter)
- Relative key detection (Am <-> C major)
- Semitone distance calculation
- `is_harmonically_compatible()` check

### 1.3 Existing Database Schema

Defined but **empty** graph relations:
- `similar_to(from_track, to_track, similarity_score)` — computed from vector search
- `harmonic_match(from_track, to_track, match_type)` — Same/Adjacent/EnergyBoost/EnergyDrop
- `played_after(from_track, to_track, count, avg_transition_quality)` — DJ history

### 1.4 Existing Query Infrastructure

- `SimilarityQuery::find_similar(track_id, limit)` -> `Vec<(Track, f32)>`
- `SimilarityQuery::upsert_features(track_id, features)`
- `SimilarityQuery::has_features(track_id)` -> bool
- HNSW search with ef=50, auto-excludes query track

---

## 2. Audio Embedding Models — What's Available

### 2.1 Neural Embedding Models (Deep Learning)

| Model | Dims | Compute | Music-Specific | ONNX | Best For |
|-------|------|---------|---------------|------|----------|
| **Discogs-EffNet** | 1280 | Low | Yes (400 styles) | Yes | Genre/style similarity |
| **MAEST** | 768-2304 | Medium | Yes (transformer) | Yes | Multi-scale temporal |
| **MSD-MusiCNN** | 200 | Very Low | Yes (790k params) | Convertible | Timbre/temporal |
| **VGGish** | 128 | Low | No (general) | Yes | General audio |
| **OpenL3** | 512/6144 | Low-Med | Yes (music mode) | Convertible | Acoustic texture |
| **CLAP** | 512 | Medium | Partial | Yes | Text-based search |
| **MERT** | 768 | High | Yes (strong) | Convertible | Pitch/harmony/rhythm |
| **PANNs CNN14** | 2048 | Medium | No (general) | Yes | General audio |

**Recommended for Mesh: Discogs-EffNet**
- 1280-dim embeddings trained on 400 Discogs music styles via contrastive learning
- ONNX model directly available: `discogs-effnet-bsdynamic-1.onnx`
- EfficientNet architecture = low compute cost
- Embedding space naturally clusters by genre/subgenre
- Already available through Essentia's model repository

**Secondary: CLAP (optional future enhancement)**
- 512-dim shared audio-text embedding space
- Enables natural language queries: "find me dark atmospheric techno with heavy bass"
- Useful for tag-based browsing without manual tagging

### 2.2 Traditional Feature Vectors

**bliss-audio** (Rust crate, pure feature extraction):
- 20 dimensions: tempo(1) + timbre(7) + loudness(2) + chroma(10)
- Euclidean distance (customizable via Mahalanobis)
- Uses aubio + librosa-style chroma extraction
- Lightweight, no neural network needed
- Similar approach to our existing 16-dim vector but with different feature selection

**Our existing 16-dim vector** covers similar ground to bliss-audio but with different emphasis (more energy/dynamics, less chroma). Could be **extended or replaced** rather than adding bliss as a dependency.

### 2.3 Integration Path: ONNX in Rust

**`ort` crate** (v2.0.0-rc.11) — primary recommendation:
- Wraps ONNX Runtime v1.24.1
- Execution providers: CPU, CUDA, TensorRT, CoreML, DirectML
- Used by Google Magika, HuggingFace Text Embeddings Inference
- Apache-2.0 / MIT dual license

**Pipeline for Discogs-EffNet:**
1. Load audio (already have this in Mesh — mono mixdown at 48kHz)
2. Compute mel spectrogram (128 mel bands, matching Essentia's `TensorflowInputMusiCNN` preprocessing)
3. Run inference via `ort` on `discogs-effnet-bsdynamic-1.onnx`
4. Extract 1280-dim embedding from penultimate layer (`PartitionedCall:1`)
5. Store in CozoDB HNSW index

**Mel spectrogram crate**: `mel_spec` — aligned to whisper.cpp/librosa, streaming support, 480x faster than realtime.

**Alternative: `tract`** (Sonos) — pure Rust ONNX inference, no C dependency, supports ~140 ONNX operators. Good fallback if `ort` is too heavy.

---

## 3. CozoDB Capabilities for Recommendations

### 3.1 Vector Search (HNSW)

Already implemented in our schema. Key capabilities:

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

### 3.2 Graph Queries

**Built-in graph algorithms** (with `graph-algo` feature):
- CommunityDetectionLouvain — find clusters of similar tracks
- PageRank — track importance in the "played together" graph
- ShortestPath — minimum-cost path between tracks
- RandomWalk — stochastic playlist generation
- ConnectedComponents — identify isolated track groups

**Unique feature**: CozoDB exposes the **HNSW proximity graph itself** as a queryable relation. You can run community detection directly on the audio similarity graph:
```datalog
?[community, track_id] <~ CommunityDetectionLouvain(
    *tracks:audio_sim[fr_track_id, to_track_id, dist],
    undirected: true
)
```

### 3.3 Hybrid Queries (Vector + Scalar + Graph)

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

### 3.4 Additional CozoDB Indexes

- **FTS (Full-Text Search)**: For searching tracks by title/artist text
- **MinHash-LSH**: For finding near-duplicate tracks (remixes, alternate versions)

### 3.5 Performance at Our Scale

For 10k-100k tracks:
- HNSW search: single-digit milliseconds
- Graph traversal (2-hop): sub-millisecond
- Point reads: 100K+ QPS
- Batch import: ~150K rows/second (RocksDB)

---

## 4. Real-World DJ Software Approaches

### 4.1 Commercial Software

| Software | Feature | Approach |
|----------|---------|----------|
| **Rekordbox** (Pioneer) | Related Tracks | Key + BPM + genre tag matching |
| **djay Pro** (Algoriddim) | AI Match | Neural audio analysis, "Automix AI" |
| **VirtualDJ** | Smart Suggestions | BPM + key + genre + energy matching |
| **Mixed In Key** | Energy Level System | 1-10 energy scale + Camelot key |
| **Serato** | SmartCrates | Metadata rule-based filtering |
| **Beatport** | Recommendations | Collaborative filtering + audio features |
| **DJ.Studio** | Auto-mix | Harmonic + tempo + energy curve planning |

### 4.2 Key Mixing Criteria (What Real DJs Use)

**Primary filters (hard constraints):**
1. **BPM compatibility**: ±5% range (e.g., 128 BPM matches 122-134)
2. **Key compatibility**: Camelot wheel — same number, ±1 number, or A<->B (relative major/minor)

**Secondary ranking (soft scoring):**
3. **Energy flow**: Progressive build (70->80->90) vs maintain vs cool-down (90->80->70)
4. **Genre/style similarity**: Neural embedding distance
5. **Timbral similarity**: Similar spectral characteristics for smooth blends
6. **Mood consistency**: Maintain or deliberately shift atmosphere

**Tertiary (context-aware):**
7. **Already played**: Don't repeat tracks
8. **Transition history**: Tracks that have worked together before (played_after graph)
9. **Set position**: Warm-up tracks early, bangers at peak, cool-down at end

### 4.3 Harmonic Mixing — Camelot Wheel

Full mapping (already implemented in `mesh-core/src/music/mod.rs`):

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

**Compatible moves:**
- Same key (8A -> 8A) — score 1.0
- Adjacent number (8A -> 7A, 8A -> 9A) — score 0.9
- Relative major/minor (8A -> 8B) — score 0.85
- ±2 numbers (8A -> 6A, 8A -> 10A) — score 0.6 (energy boost/drop)

### 4.4 Energy Flow Patterns

Professional DJ sets follow a narrative arc:
1. **Warm Up** (low-medium energy): Deep, spacious tracks, long blends
2. **Build** (medium-rising): Stronger drums, layered percussion
3. **Peak** (high energy): Anthemic tracks, quick transitions — typically ~2/3 through the set
4. **Release** (medium): Melodic, breakdowns
5. **Finale** (decreasing): Familiar/emotional tracks

**For suggestion system**: Allow user to specify desired energy direction (build/maintain/cool-down) as a filter parameter.

---

## 5. Research Papers & Notable Implementations

### 5.1 DJ-Specific Research

| Paper | Year | Key Contribution |
|-------|------|-----------------|
| **"Automatic DJ Transitions with Differentiable Audio Effects and GANs"** (Chen et al.) | ICASSP 2022 | GAN-based transition generation using differentiable EQ + fader |
| **"Cue Point Estimation using Object Detection"** (Argüello et al.) | 2024 | Object detection transformer for DJ cue points, 21k annotated dataset |
| **"Zero-shot DJ Tool Retrieval"** | 2024 | CLAP embeddings for classifying vocal hooks, drum breaks, etc. |
| **"Enhancing Sequential Music Recommendation with Personalized Popularity Awareness"** | RecSys 2024 | Transformer + popularity awareness, 25-70% improvement |
| **"Beyond Collaborative Filtering: Using Transformers"** | 2024 | Transformer models for playlist continuation |

### 5.2 Audio Embedding Research

| Paper/Model | Key Contribution |
|-------------|-----------------|
| **Discogs-EffNet** (MTG/UPF) | EfficientNet trained on 400 Discogs styles, contrastive learning |
| **MAEST** (MTG/UPF) | Music Audio Efficient Spectrogram Transformer, multi-scale |
| **CLAP** (LAION/Microsoft) | Contrastive Language-Audio Pretraining, text-audio shared space |
| **MERT** (m-a-p) | Self-supervised music transformer, CQT + codebook teachers |
| **PANNs** (Kong et al.) | Pretrained Audio Neural Networks, AudioSet, CNN14 architecture |

### 5.3 Key Insight from Research

> "Strict song order appears less crucial than previously thought" for playlist generation. What matters most is **local compatibility** (the next track fits the current one) and **global trajectory** (the set follows a coherent energy arc).

---

## 6. Rust Crates & Tools

### 6.1 ML Inference

| Crate | Purpose | Notes |
|-------|---------|-------|
| **`ort`** (v2.0.0-rc.11) | ONNX Runtime wrapper | Primary choice, CUDA support, 2k+ stars |
| **`tract`** (v0.22.0) | Pure-Rust ONNX inference | No C dependency, good for embedded, supports streaming (tract-pulse) |
| **`candle`** (HuggingFace) | Rust ML framework | Whisper, encodec support; CPU/CUDA/Metal backends |
| **`rten`** | Pure-Rust ONNX runtime | Lightweight alternative to ort |

### 6.2 Audio Analysis

| Crate | Purpose | Notes |
|-------|---------|-------|
| **`mel_spec`** | Mel spectrogram computation | 480x realtime, streaming, aligned to librosa |
| **`bliss-audio`** (v0.9) | 20-dim audio features + playlist | Tempo, timbre, chroma, loudness; Euclidean/Mahalanobis distance |
| **`rusty-chromaprint`** | Audio fingerprinting | Pure Rust, for remix/duplicate detection |
| **`Spectrograms`** | Linear/Mel/CQT/MFCC | Multiple spectrogram types |

### 6.3 Vector Similarity

| Crate | Purpose | Notes |
|-------|---------|-------|
| **CozoDB HNSW** | Already integrated | Cosine/L2/IP, disk-based, MVCC |
| **`fast_vector_similarity`** | CPU-optimized similarity | ndarray + rayon |
| **`ndarray`** | Manual cosine similarity | Trivial to implement |

---

## 7. Proposed Architecture

### 7.1 Two-Tier Embedding System

**Tier 1: Existing 16-dim features** (fast, already computed)
- Used for initial candidate filtering and basic similarity
- No additional compute needed — already in database
- Good for BPM/key/energy matching

**Tier 2: Neural 1280-dim Discogs-EffNet** (rich, compute at import time)
- Used for deep style/genre similarity ranking
- Computed during track import (background, like current analysis)
- Stored in a second HNSW index in CozoDB

### 7.2 Query Pipeline

```
User triggers "Suggest Next Track"
    |
    v
[1] Get seed track(s) metadata
    - Currently playing track(s) BPM, key, energy, embeddings
    |
    v
[2] Hard filters (BPM ±5%, compatible key via Camelot)
    - CozoDB in-search filter on HNSW query
    - Exclude already-played tracks
    |
    v
[3] Vector similarity ranking (Discogs-EffNet cosine distance)
    - Top-K nearest neighbors from HNSW
    |
    v
[4] Re-rank with composite score:
    - 0.4 * audio_similarity (Discogs-EffNet cosine)
    - 0.2 * key_compatibility (Camelot score)
    - 0.2 * energy_direction (matches desired build/maintain/cool)
    - 0.1 * bpm_closeness (normalized BPM distance)
    - 0.1 * transition_history (played_after graph weight, if available)
    |
    v
[5] Return top 10 suggestions with explanations
    - "Similar style, compatible key (8A -> 9A), energy builds"
```

### 7.3 Suggestion Modes

| Mode | Description | Primary Signal |
|------|-------------|---------------|
| **Similar** | "More like this" | Vector similarity only |
| **Harmonic** | Key-compatible tracks | Camelot + energy |
| **Energy Build** | Increase energy | Energy > current + vector sim |
| **Energy Cool** | Decrease energy | Energy < current + vector sim |
| **Surprise** | Intentional contrast | Random from compatible key/BPM |
| **History** | "Worked before" | played_after graph + vector sim |

### 7.4 Data Flow

```
Track Import
    |
    +-> Essentia analysis (existing: BPM, key, LUFS, 16-dim features)
    +-> Discogs-EffNet via ort (new: 1280-dim neural embedding)
    +-> Store both vectors in CozoDB HNSW indexes
    +-> Pre-compute Camelot compatibility relations
    |
During Playback
    |
    +-> Record transitions in played_after graph
    +-> Track energy curve position
    |
On "Suggest" Trigger
    |
    +-> Query CozoDB with hybrid query (vector + scalar + graph)
    +-> Return ranked suggestions to UI
```

---

## 8. Implementation Priority

### Phase 1: Activate Existing Infrastructure (Low Effort)
- [ ] Wire up `SimilarityQuery::find_similar()` to a UI suggestion panel
- [ ] Populate `harmonic_match` table during track import
- [ ] Add "Suggest Similar" button/MIDI trigger
- [ ] Basic results: similar tracks by 16-dim features + BPM + key filter

### Phase 2: Neural Embeddings (Medium Effort)
- [ ] Add `ort` dependency, download Discogs-EffNet ONNX model
- [ ] Add mel spectrogram preprocessing (128 mel bands)
- [ ] Compute 1280-dim embeddings during import (subprocess, like existing analysis)
- [ ] Create second HNSW index for neural embeddings (dim=1280)
- [ ] Update suggestion query to use neural embeddings as primary ranker

### Phase 3: Smart Ranking (Medium Effort)
- [ ] Composite scoring function (similarity + key + energy + BPM + history)
- [ ] Energy direction parameter (build/maintain/cool)
- [ ] Exclude already-played tracks
- [ ] Suggestion explanation text ("Compatible key, similar style, energy builds")
- [ ] Record transitions in `played_after` graph during playback

### Phase 4: Advanced Features (Higher Effort)
- [ ] Community detection on similarity graph (auto-genre clusters)
- [ ] CLAP text embeddings for natural language search ("dark minimal techno")
- [ ] Set position awareness (warm-up vs peak vs cool-down)
- [ ] Transition quality feedback loop (rate transitions, improve `played_after` weights)
- [ ] MinHash-LSH for remix/edit detection

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

Comfortable for any modern machine. HNSW search at 50k tracks with 1280 dims: ~5-10ms.

---

## 10. Key Resources

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
- Performance: https://docs.cozodb.org/en/latest/releases/v0.3.html

### Research
- Music Collection Analyzer (Essentia embeddings): https://github.com/Masetto96/music-collection-analyzer
- Cue Point Estimation (2024): https://arxiv.org/abs/2407.06823
- Auto DJ Transitions (ICASSP 2022): https://arxiv.org/abs/2110.06525
- Sequential Music Recommendation: https://arxiv.org/html/2409.04329v1
- Spotify User Embeddings: https://research.atspotify.com/contextual-and-sequential-user-embeddings-for-music-recommendation/
