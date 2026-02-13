# Drop Detection in EDM — Research Report

## Executive Summary

There is **no widely-available dedicated "drop detection" library** in any language. The most practical approach combines a pre-trained music structure analysis model with a bass-energy heuristic in Rust. This report surveys the landscape of structural analysis, characterizes drops via measurable audio features, and recommends implementation paths for Mesh.

**Key finding**: The **All-In-One** model (WASPAA 2023) provides SOTA structure analysis using source-separated spectrograms as input — directly leveraging Mesh's existing stem separation. Combined with a simple bass-energy-ratio heuristic in Rust, this can reliably identify drops without training a custom model.

---

## 1. What Is a "Drop" Technically?

A **drop** in EDM is the moment where built-up musical tension is dramatically released, typically marked by the reintroduction of a full beat (kick drum + bass) after a breakdown or buildup section.

In DnB specifically: "the point in a track where a switch of rhythm or bassline occurs and usually follows a recognisable build section and breakdown."

### 1.1 Production Techniques at a Drop

Based on Solberg (2014) — *"Waiting for the Bass to Drop"*:

1. **Bass and kick removal then reintroduction**: Buildup strips away kick/bass, drop reintroduces at full force → massive jump in low-frequency energy (20–150 Hz)
2. **Uplifters / risers**: Rising noise sweeps → increasing spectral centroid during buildup
3. **Drum roll / snare build**: Increasingly dense percussive patterns → rising onset density
4. **Frequency range contraction → expansion**: Buildup narrows spectrum, drop opens it → spectral bandwidth / contrast changes
5. **Dynamic compression → expansion**: Buildup compresses dynamics, drop has strong transients → RMS energy changes

### 1.2 Measurable Feature Signatures

| Feature | Before Drop (Buildup) | At Drop | Change |
|---------|----------------------|---------|--------|
| **RMS Energy** | Decreasing or moderate | Sudden spike | Large positive |
| **Low-freq energy ratio** | Low (bass removed) | High (bass reintroduced) | **Large positive** |
| **Onset density** | High & increasing (snare rolls) | Moderate & steady | Variable |
| **Spectral centroid** | High (risers, noise) | Lower (bass-heavy) | Negative |
| **Spectral flux** | Very high (rapid changes) | Moderate (steady groove) | Peak then settle |
| **Spectral contrast** | Low (compressed, narrow) | High (full spectrum) | Positive |
| **Spectral flatness** | Higher (noise-like) | Lower (more tonal) | Negative |

### 1.3 Why Drops Are Hard to Detect

A drop is **not just "high energy"** — it is specifically a **transition point** with simultaneous changes across multiple features. Other high-energy sections (second verse, variation) share high energy but lack the characteristic bass reintroduction pattern.

**The distinguishing factors**:
- What **precedes** it: low-energy section with specific buildup characteristics
- **Bass reintroduction**: bass energy from near-zero to maximum (unique to drops)
- **Spectral centroid shift**: downward from high-frequency buildup content to bass-heavy drop

**Practical heuristic**: A drop is a structural boundary where the bass energy ratio increases by more than X% within Y beats, and the preceding N bars had below-average bass energy.

---

## 2. DnB-Specific Considerations

- **Tempo**: 165–185 BPM. 16-bar phrase ≈ 21 seconds; 32-bar phrase ≈ 42 seconds
- **Half-time drops**: Some DnB uses "drumstep" half-time feel — onset pattern changes but tempo stays the same
- **Breakbeat patterns**: Broken, syncopated drums vs. four-on-the-floor — onset analysis must handle irregularity
- **Bass ranges**: Sub-bass (30–80 Hz sine/triangle) AND mid-range bass (reese/neurofunk, 100–500 Hz). Detection should check both
- **Multiple drops**: Typically 2 per track (sometimes 3), second often a variation. Each preceded by a breakdown
- **Common keys**: E minor, F minor, F# minor — bass fundamentals at 40–90 Hz

---

## 3. Existing Approaches

### 3.1 Academic Papers

#### Yadati et al. (ISMIR 2014) — "Detecting Drops in Electronic Dance Music"
- **Paper**: https://archives.ismir.net/ismir2014/paper/000297.pdf
- **Approach**: Two-stage — segment the track, then classify each boundary as "drop" or "not drop"
- **Features**: Spectrogram statistics (mean/stddev), MFCC, rhythm features from fixed-length window around boundary
- **Classifier**: Binary SVM
- **Dataset**: 100 mainstream EDM tracks with 225 time-coded SoundCloud comments for ground truth
- **Results**: F1 = 0.71 at 15-second tolerance; F1 > 0.6 at 3-second tolerance
- **Status**: Not publicly released as a tool

#### van den Brink (2020) — "Finding 'The Drop'"
- **Thesis**: https://essay.utwente.nl/82333/
- **Approach**: Compared SVM and CNN classifiers on spectrograms
- **Dataset**: ~500 EDM songs with manually labeled drop onsets

#### Solberg (2014) — "Waiting for the Bass to Drop"
- **Paper**: https://www.researchgate.net/publication/273928761
- **Focus**: Correlations between production techniques in buildup/drop sections and intense emotional experiences
- **Value**: Defines the audio feature characteristics of drops (see Section 1 above)

### 3.2 Music Structure Analysis Tools

#### All-In-One (WASPAA 2023) — SOTA Multi-Task Structure Analysis

- **Repo**: https://github.com/mir-aidj/all-in-one
- **Paper**: https://arxiv.org/abs/2307.16425
- **What it does**: Jointly performs beat tracking, downbeat tracking, structure segmentation, and labeling
- **Architecture**: Source-separated spectrograms → dilated neighborhood attention (long-range temporal) + non-dilated attention (local instrumental)
- **Labels**: start, end, intro, outro, break, bridge, inst, solo, verse, chorus
- **Trained on**: Harmonix Set with 8-fold cross-validation
- **Performance**: SOTA on all 4 tasks; 10 songs (33 min) in 73 seconds on RTX 4090
- **Pre-trained models**: `harmonix-all` ensemble available
- **Key advantage for Mesh**: Uses source-separated spectrograms as input — we already have stem separation, so we can feed drums/bass/vocals/other directly
- **Limitation**: Labels are pop-music-centric (verse/chorus) rather than EDM-centric (buildup/drop/breakdown). Mapping required.

#### CUE-DETR (ISMIR 2024) — EDM Cue Point Detection

- **Repo**: https://github.com/ETH-DISCO/cue-detr
- **Paper**: https://arxiv.org/abs/2407.06823
- **Approach**: Object detection on Mel spectrograms using DETR transformer
- **Dataset**: EDM-CUE — 21k manually annotated cue points from 4 DJs, ~5k EDM tracks
- **Checkpoints**: HuggingFace `disco-eth/cue-detr`
- **Inference**: Sliding window on full tracks
- **Limitation**: Cue points include mix-in/mix-out, not exclusively drops
- **Advantage**: Trained specifically on EDM with a large dataset

#### MSAF (Music Structure Analysis Framework)

- **Repo**: https://github.com/urinieto/msaf
- **Algorithms**: Foote's novelty, C-NMF, 2D-FMC, OLDA, SF, and others
- **Uses**: librosa for feature extraction
- **Provides**: Structural boundaries (not labels)
- **Status**: Maintained, pip installable

#### Rekordbox Phrase Analysis (Pioneer DJ)

- **Labels**: INTRO, UP, DOWN, CHORUS, OUTRO (for EDM/dance tracks)
- **Status**: Commercial, closed-source
- **Relevance**: Demonstrates the concept of EDM-specific labeling including buildup ("UP") and drop ("CHORUS")

### 3.3 Classical Signal Processing: Novelty Functions

Foote (2000) — "Automatic Audio Segmentation Using a Measure of Audio Novelty":

1. Extract frame-wise features (MFCC, chroma, spectral)
2. Compute **Self-Similarity Matrix (SSM)** — similarity between all time frame pairs
3. Convolve a **checkerboard kernel** along the SSM diagonal
4. The resulting **novelty function** peaks at structural boundaries
5. Peak-pick for boundary locations

**Implementations**:
- librosa: `segment.recurrence_matrix()`, `segment.agglomerative()`
- MSAF: Multiple algorithms
- Essentia: `NoveltyCurve`, `SBic` (Bayesian Information Criterion segmentation)

### 3.4 Deep Learning Approaches

#### SegmentationCNN
- **Repo**: https://github.com/mleimeister/SegmentationCNN
- CNN on log-scaled Mel spectrograms, 16-bar context window
- F-measure: 59% at 2-beat tolerance on SALAMI

#### MusicBoundariesCNN
- **Repo**: https://github.com/carlosholivan/MusicBoundariesCNN
- Compares MLS, SSM, SSLM as inputs
- Finding: Mel spectrograms alone performed best

#### Foundational Audio Encoders (Dec 2025)
- **Paper**: https://arxiv.org/abs/2512.17209
- Benchmarked MusicFM, MERT, AudioMAE on music structure
- MusicFM (30-second context) performed best for boundary detection
- Suggests fine-tuning foundational models as future path

### 3.5 Dedicated Drop Detection (or Lack Thereof)

**No dedicated open-source drop detection tool exists.** The closest:

| Project | URL | Status |
|---------|-----|--------|
| Yadati et al. approach | (paper only, no code released) | Academic |
| Uptake Bass Drop Predictor | (internal corporate demo) | Not released |
| edm-segmentation | https://github.com/mixerzeyu/edm-segmentation | Student project, limited |
| DJ-LLM | https://github.com/themreza/DJ-LLM | WIP, not released |

---

## 4. Feature Engineering for Drop Detection

### 4.1 Primary Features (Strongest Signal)

1. **Low-frequency energy ratio** — energy in 20–200 Hz / total energy. The single most distinguishing feature. Essentia: `EnergyBandRatio(startFreq=20, stopFreq=200)`
2. **RMS energy / loudness** — sudden jump at drop boundary
3. **Onset strength / spectral flux** — peak at transition, then stabilization

### 4.2 Secondary Features (Refinement)

4. **Spectral centroid** — drops shift lower; buildups are high
5. **Spectral contrast** — drops have wide distribution; breakdowns are narrow
6. **Spectral flatness** — buildups trend noise-like; drops are more tonal
7. **Onset density / rate** — buildups accelerate (snare rolls); drops regularize

### 4.3 Temporal Context Features

8. **Feature deltas** — rate of change over 2–8 bars preceding boundary
9. **Buildup detection** — increasing spectral centroid + decreasing bass + increasing onset density = buildup
10. **Bass energy ratio delta** — magnitude of bass reintroduction (before vs. after boundary)

### 4.4 Distinguishing Drops from Other High-Energy Sections

A practical multi-feature check at each structural boundary:

```
is_drop(boundary) =
  bass_energy_ratio_delta > threshold_bass  AND
  preceding_N_bars_bass_energy < average_bass_energy  AND
  rms_delta > threshold_rms  AND
  spectral_centroid_delta < 0  (shift downward)
```

---

## 5. Implementation Paths for Mesh

### Path A: Essentia Features + Custom Heuristics (No ML)

1. Extract per-beat features using Essentia:
   - RMS energy, low-band energy ratio, spectral centroid, onset strength, spectral contrast
2. Compute novelty function from bass energy ratio (first derivative of smoothed signal)
3. Apply heuristics at novelty peaks:
   - Bass energy ratio delta > threshold?
   - Preceding section low-energy / low-bass?
   - Following section high-energy?
   - If all yes → "drop"

**Pros**: No ML, runs natively in Rust, interpretable, fast
**Cons**: Requires genre-specific threshold tuning, may miss subtle drops

**Effort**: ~3–5 days
**Accuracy**: Moderate — works well for obvious drops, struggles with subtle ones

### Path B: All-In-One Model (Best Quality)

1. Run All-In-One as Python subprocess during track import
2. Get segment boundaries + labels (intro, chorus, break, etc.)
3. Map to EDM concepts: "chorus" after "break" = likely drop
4. Optionally refine with bass energy check in Rust

**Pros**: SOTA accuracy, pre-trained, handles diverse music. Can feed separated stems directly.
**Cons**: Python dependency, pop-centric labels need mapping

**Effort**: ~1 week
**Accuracy**: High

### Path C: Train Small ML Classifier

1. Extract Essentia features at structural boundaries (from novelty function)
2. Train small classifier (random forest / SVM) on "drop" vs "not drop"
3. Training data: manually annotate 50–100 DnB tracks

**Feature vector per boundary**:
- Bass energy ratio: mean + delta (before/after)
- RMS energy: mean + delta
- Spectral centroid: mean + delta
- Onset rate: mean + delta
- Spectral contrast: mean + delta
- Duration of preceding low-energy section
- BPM (context)

**Pros**: Small model, fast inference, specializes for DnB
**Cons**: Requires manual annotation, won't generalize without retraining

**Effort**: ~1–2 weeks (including annotation)
**Accuracy**: High for annotated genres, low generalization

### Path D: CUE-DETR for EDM Cue Points

1. Run CUE-DETR inference (pre-trained HuggingFace checkpoints)
2. Get cue points → filter by audio features to identify drops

**Pros**: Trained on 5k EDM tracks, 21k annotations
**Cons**: Cue points ≠ drops (includes mix-in/mix-out); Python-only

### Path E: Hybrid (Recommended)

1. **Offline (Python, at import time)**: Run All-In-One for structural segments + labels. Store in CozoDB.
2. **Rust heuristic**: At each boundary, compute bass energy ratio delta. Confirm "break" → "chorus" transitions as drops.
3. **Store drop timestamps** in CozoDB alongside track metadata.
4. **Long-term**: Export All-In-One or CUE-DETR to ONNX → run via `ort` → no Python at runtime.

**Effort**: ~1–2 weeks
**Accuracy**: High
**Maintenance**: Low — pre-trained model, simple heuristic

---

## 6. Rust Ecosystem for Implementation

### Available Tools

| Crate/Tool | What | Use For |
|------------|------|---------|
| `essentia` (current) | Audio feature extraction | Bass energy ratio, RMS, spectral features |
| `ort` | ONNX Runtime bindings | Running exported ML models |
| `spectrum-analyzer` | FFT spectrum analysis (no_std) | Custom spectral feature extraction |
| `audio-processor-analysis` | Transient detection | Onset density |
| All-In-One (subprocess) | Structure analysis | Segment boundaries + labels |
| CUE-DETR (subprocess) | Cue point detection | EDM-specific boundaries |

### ONNX Long-Term Path

Both All-In-One and CUE-DETR are PyTorch models that could theoretically be exported to ONNX:
- Run via `ort` crate in Rust
- Hardware acceleration via CUDA/TensorRT
- Already bundling `libonnxruntime.so` in mesh-cue's deb package
- Would eliminate Python runtime dependency

**Caveat**: All-In-One uses source separation internally (via Demucs), which is itself a large model. Exporting the full pipeline to ONNX may be complex. An alternative is to perform source separation separately (which Mesh already does) and feed the stems directly.

---

## 7. Recent Developments (2023–2026)

| Year | Development | Reference |
|------|-------------|-----------|
| 2023 | All-In-One (WASPAA) — SOTA multi-task structure analysis | https://arxiv.org/abs/2307.16425 |
| 2024 | CUE-DETR (ISMIR) — transformer cue point detection for EDM | https://arxiv.org/abs/2407.06823 |
| 2024 | Self-supervised multi-level audio representations for segmentation | IEEE TASLP 2024 |
| 2025 | Foundational audio encoders for music structure (MusicFM best) | https://arxiv.org/abs/2512.17209 |
| 2025 | Beat-feature + ResNet-34 with self-attention for structure | PLOS ONE 2025 |
| 2025 | DJ-LLM — multimodal LLM for DJ tasks (WIP, includes drop detection) | https://github.com/themreza/DJ-LLM |
| 2025 | Temporal adaptation of foundation models for structure analysis | https://arxiv.org/abs/2507.13572 |

### Key Trends

1. **Foundation models emerging**: MusicFM, MERT show promise for structure analysis with fine-tuning
2. **Source separation as preprocessing**: All-In-One demonstrated that feeding separated stems significantly improves analysis — directly relevant for Mesh
3. **Object detection on spectrograms**: CUE-DETR leverages the CV model ecosystem for audio
4. **EDM-specific datasets growing**: EDM-CUE (5k tracks) and Harmonix Set provide substantial training data
5. **No Rust-native solution exists**: All cutting-edge is Python. Rust path = FFI (essentia-rs) or ONNX (ort)

---

## Sources

- [Yadati et al. — "Detecting Drops in EDM" (ISMIR 2014)](https://archives.ismir.net/ismir2014/paper/000297.pdf)
- [Solberg — "Waiting for the Bass to Drop" (2014)](https://www.researchgate.net/publication/273928761)
- [van den Brink — "Finding 'The Drop'" (2020)](https://essay.utwente.nl/82333/)
- [All-In-One Repository](https://github.com/mir-aidj/all-in-one)
- [All-In-One Paper (WASPAA 2023)](https://arxiv.org/abs/2307.16425)
- [CUE-DETR Repository](https://github.com/ETH-DISCO/cue-detr)
- [CUE-DETR Paper (ISMIR 2024)](https://arxiv.org/abs/2407.06823)
- [MSAF Repository](https://github.com/urinieto/msaf)
- [SegmentationCNN Repository](https://github.com/mleimeister/SegmentationCNN)
- [MusicBoundariesCNN Repository](https://github.com/carlosholivan/MusicBoundariesCNN)
- [Foundational Audio Encoders Paper](https://arxiv.org/abs/2512.17209)
- [Foote — Automatic Audio Segmentation (2000)](https://www.researchgate.net/publication/3863771)
- [Essentia Models Documentation](https://essentia.upf.edu/models.html)
- [ort (Rust ONNX Runtime)](https://github.com/pykeio/ort)
- [DJ-LLM Repository](https://github.com/themreza/DJ-LLM)
- [dnb-autodj-3 Repository](https://github.com/lenvdv/dnb-autodj-3)
- [edm-segmentation Repository](https://github.com/mixerzeyu/edm-segmentation)
- [Drop (music) — Wikipedia](https://en.wikipedia.org/wiki/Drop_(music))
