# Essentia ML Audio Analysis Capabilities

Research date: February 2026

## Context

The mesh project already uses Essentia (via `essentia-rs` FFI) for:
- **BPM/beat detection** — `RhythmExtractor2013`
- **Key detection** — `KeyExtractor`
- **LUFS measurement** — `LoudnessEBUR128`
- **Energy statistics** — `Energy` algorithm (sum of squares)
- **Dynamic complexity** — `DynamicComplexity`
- **Danceability** — `Danceability` (Detrended Fluctuation Analysis)

Essentia is built with `USE_TENSORFLOW=0`, so TensorFlow-based algorithms
(`TensorflowPredictMusiCNN`, `TensorflowPredictEffnetb0`, etc.) are **not**
available through the FFI. However, all Essentia ML models are published in
**ONNX format** and can be run directly via the `ort` crate (already in the
project for Demucs stem separation).

---

## Architecture: Two-Stage Pipeline

All Essentia ML classifiers use the same pattern:

1. **Audio → Mel Spectrogram** (Essentia DSP, available via FFI)
2. **Mel Spectrogram → Embedding** (EffNet CNN, 17.2 MB ONNX model, outputs 1280-dim vectors)
3. **Embedding → Classification** (tiny head models, ~500 KB each)

The EffNet embedding is the bottleneck. Once computed, all classification
heads are near-instant. The entire model set (embedding + all 36+
classification heads) totals under 40 MB.

### Integration Path (Recommended: Option B — ONNX via `ort`)

```
Essentia MelBands (16kHz) → ort: EffNet ONNX (17.2 MB) → 1280-dim embedding
                                                        ↓
                              ort: genre head (2 MB)    → 400 genre activations
                              ort: mood heads (~500 KB) → binary mood scores
                              ort: arousal/valence      → 2 continuous floats
                              ort: voice/instr (~500 KB)→ binary detection
```

Pros: No TensorFlow dependency, `ort` already proven in project,
`ort::Session` is `Send + Sync` (unlike `essentia-rs`), small models.

Cons: Need to replicate Essentia's mel spectrogram preprocessing for the
EffNet input format (16kHz sample rate, specific mel-band configuration).

---

## Available Models (Priority Ranked for DJ Use)

### 1. Voice/Instrumental Detection (HIGH priority)

- **Model**: `voice_instrumental-discogs-effnet-1.onnx` (~514 KB head)
- **Output**: Binary — `instrumental` / `voice` with softmax probabilities
- **Accuracy**: 96%
- **DJ value**: Knowing if a track has vocals is critical for transition
  planning. Vocal tracks should avoid overlapping vocal sections.

### 2. Genre Classification — Discogs400 (HIGH priority)

- **Model**: `genre_discogs400-discogs-effnet-1.onnx` (~2 MB head)
- **Output**: 400 sigmoid activations (multi-label, NOT mutually exclusive)
- **Accuracy**: ROC-AUC 0.954 on 3.3M tracks
- **Taxonomy**: 15 top-level genres with granular subgenres:
  - **Electronic** (106 subgenres): Acid House, Ambient, Breakbeat, Deep
    House, Deep Techno, Disco, Drum n Bass, Dub Techno, Dubstep, Electro,
    Garage House, Hardcore, House, IDM, Jungle, Minimal, Progressive House,
    Progressive Trance, Psytrance, Techno, Trance, UK Garage, etc.
  - **Rock** (99), **Latin** (34), **Hip Hop** (24), **Jazz** (24),
    **Folk/World/Country** (23), etc.
- **DJ value**: Library organization, filtering, tag-based playlists. The 106
  electronic subgenres alone are extremely relevant for DJ software.
- Multi-label means a track can be "House" AND "Deep House" simultaneously.

### 3. Arousal/Valence Regression (HIGH priority)

- **Models**: `deam_arousal_valence-*` (multiple embedding variants)
- **Output**: 2 continuous floats — (valence, arousal)
  - **Arousal**: calm (low) → excited (high) — maps to DJ "energy"
  - **Valence**: sad/dark (low) → happy/uplifting (high) — maps to DJ "mood"
- **Accuracy**: Pearson r=0.738 (valence), r=0.773 (arousal)
- **DJ value**: Unlike LUFS (which is uniform after normalization), arousal
  captures perceived energy from musical content (rhythm, timbre, harmony).
  Could replace/complement the LUFS-based energy scoring in suggestions.

### 4. Binary Mood Classifiers (MEDIUM priority)

| Model | Classes | Accuracy |
|-------|---------|----------|
| `mood_aggressive` | aggressive / non_aggressive | varies |
| `mood_happy` | happy / non_happy | 87% |
| `mood_party` | party / non_party | varies |
| `mood_relaxed` | relaxed / non_relaxed | varies |
| `mood_sad` | sad / non_sad | varies |

- Each ~514 KB classification head
- DJ value: Quick tags for set building and filtering

### 5. MTG-Jamendo Mood/Theme — 56 Tags (MEDIUM priority)

- Multi-label with 56 mood/theme tags: action, adventure, calm, dark, deep,
  dramatic, dream, emotional, energetic, epic, fast, film, fun, groovy,
  happy, heavy, hopeful, inspiring, love, meditative, melancholic, melodic,
  motivational, nature, party, positive, powerful, relaxing, retro, romantic,
  sad, sexy, slow, etc.
- Uses same two-stage pipeline with multiple embedding options.

### 6. ML Danceability (LOW priority)

- **Model**: `danceability-discogs-effnet-1.onnx` (~514 KB head)
- **Output**: Binary — `danceable` / `not_danceable`
- **Accuracy**: 97%
- Already have DFA-based danceability (continuous 0-1). ML version is more
  robust but binary instead of continuous.

### 7. Instrument Detection — 40 Classes (LOW priority)

- Multi-label: accordion, bass, beat, bell, brass, cello, drums,
  drummachine, electricguitar, flute, guitar, keyboard, orchestra, organ,
  pad, percussion, piano, rhodes, sampler, saxophone, strings, synthesizer,
  trombone, trumpet, viola, violin, voice, etc.
- DJ value: Could tag tracks with instrumentation for filtering/search.

### 8. Other Descriptors (LOW priority)

| Descriptor | Output | DJ Relevance |
|-----------|--------|-------------|
| Approachability | mainstream vs niche (2/3-class or regression) | Low-medium |
| Engagement | active vs background listening | Low |
| Timbre | bright vs dark | Medium |
| Tonal/Atonal | binary | Low |
| Acoustic/Electronic | binary | Low |

---

## Model Download URLs

All hosted at `https://essentia.upf.edu/models/`:

```
# EffNet embedding (run once per track)
models/music-style-classification/discogs-effnet/discogs-effnet-bsdynamic-1.onnx  (17.2 MB)

# Classification heads (run after embedding)
models/classification-heads/genre_discogs400/genre_discogs400-discogs-effnet-1.onnx
models/classification-heads/mood_happy/mood_happy-discogs-effnet-1.onnx
models/classification-heads/mood_aggressive/mood_aggressive-discogs-effnet-1.onnx
models/classification-heads/mood_party/mood_party-discogs-effnet-1.onnx
models/classification-heads/mood_relaxed/mood_relaxed-discogs-effnet-1.onnx
models/classification-heads/mood_sad/mood_sad-discogs-effnet-1.onnx
models/classification-heads/danceability/danceability-discogs-effnet-1.onnx
models/classification-heads/voice_instrumental/voice_instrumental-discogs-effnet-1.onnx
models/classification-heads/approachability/approachability_*-discogs-effnet-1.onnx
models/classification-heads/engagement/engagement_*-discogs-effnet-1.onnx

# Arousal/Valence (uses different embedding — MusiCNN or VGGish)
models/classification-heads/deam_valence/deam_valence-*-1.onnx
```

Metadata JSON files alongside each model contain class labels, input/output
node names, and performance metrics.

**License**: CC BY-NC-SA 4.0 (non-commercial). Proprietary license available
on request from MTG/UPF.

---

## Implementation Notes

### Preprocessing for EffNet

The EffNet model expects mel-band frames computed from 16kHz mono audio:
1. Resample to 16kHz mono
2. Compute mel spectrogram using Essentia's `MelBands` algorithm
3. Frame into patches of the expected shape (check model JSON metadata)

The `MelBands` algorithm is available in the DSP-only build (no TensorFlow
needed). The specific configuration (number of bands, hop size, window size)
must match what the model was trained with — check the model's JSON config.

### Storage Considerations

- Embeddings (1280-dim float32) = 5 KB per track
- Could store embeddings in CozoDB for fast retrieval
- Run classification heads on-demand or during import analysis
- Genre/mood results could populate the tag system automatically

### Relation to Smart Suggestions

The arousal value from arousal/valence regression could replace the
LUFS-based energy scoring in the suggestions system. Unlike LUFS (which is
uniform after loudness normalization), arousal captures perceived energy
from musical content — rhythm intensity, harmonic tension, timbral
brightness. This would make the energy direction fader actually meaningful
for steering suggestions toward higher or lower perceived energy tracks.

---

## Sources

- [Essentia Models Documentation](https://essentia.upf.edu/models.html)
- [Essentia Algorithms Overview](https://essentia.upf.edu/algorithms_overview.html)
- [Essentia ML Tutorial](https://essentia.upf.edu/machine_learning.html)
- [TensorFlow Models in Essentia (paper)](https://ar5iv.labs.arxiv.org/html/2003.07393)
- [Discogs-EffNet Model Files](https://essentia.upf.edu/models/music-style-classification/discogs-effnet/)
- [Classification Heads Directory](https://essentia.upf.edu/models/classification-heads/)
- [essentia-rs GitHub](https://github.com/lagmoellertim/essentia-rs)
