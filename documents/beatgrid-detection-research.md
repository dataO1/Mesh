# Beatgrid Detection — Research Report

## Executive Summary

Mesh currently uses Essentia's `RhythmExtractor2013` (multifeature method) for beat tracking. While functional, it requires frequent manual beatgrid adjustments — especially for DnB/breakbeat tracks where the half-tempo problem (170 BPM → 85 BPM) is pervasive. This report surveys the landscape of open-source beat tracking solutions, evaluates their accuracy and Rust compatibility, and recommends an upgrade path.

**Key finding**: The field has moved decisively toward deep learning. **Beat This!** (CPJKU, ISMIR 2024) achieves new state-of-the-art accuracy by removing the Dynamic Bayesian Network (DBN) that causes half-tempo errors on complex rhythms. It is ONNX-exportable and can run in Rust via the `ort` crate.

---

## 1. Current Implementation

### 1.1 How It Works Today

- **Crate**: `essentia` v0.1.5 (Rust FFI bindings to Essentia C++)
- **Algorithm**: `RhythmExtractor2013` with `method("multifeature")`
- **File**: `crates/mesh-cue/src/analysis/bpm.rs`
- **Tempo range**: 40–208 BPM (configurable via `BpmConfig`)
- **Post-processing**: `fit_bpm_to_range()` applies octave/triplet fitting
- **Beat grid**: Fixed-interval grid generated from first detected beat + BPM
- **Confidence**: Hardcoded at `0.8` (not extracted from Essentia)

### 1.2 Known Issues

1. **Half-tempo on DnB**: 170 BPM tracks detected as 85 BPM — the algorithm locks onto the snare (every other beat) instead of the kick pattern. From Pioneer DJ forums: "95% of drum and bass tracks get set to 86-88 BPM."
2. **Phase errors**: First beat position may be off, causing the entire grid to shift.
3. **Breakbeat confusion**: Syncopated patterns (Amen break, Think break) confuse onset detection.
4. **Fixed grid assumption**: The grid is a fixed interval from beat 1 — any tempo variation causes cumulative drift.

### 1.3 Quick Wins Within Essentia

These improvements require no new dependencies:

1. **Extract real confidence** — `RhythmExtractor2013` returns a confidence value; using it would flag tracks needing manual review instead of hardcoding `0.8`.
2. **HPSS preprocessing** — Run Essentia's harmonic-percussive separation before beat detection. Detecting beats on only the percussive component avoids bass synths confusing onset detection.
3. **Spectral whitening** — Essentia's `SpectralWhitening` equalizes harmonic energies, helping onset functions focus on rhythmic transients.
4. **Genre-aware tempo priors** — For DnB, setting `minTempo=80, maxTempo=200` reduces octave errors while still allowing half-time detection.

---

## 2. Essentia's Beat Tracking Algorithms

Essentia offers two main paths exposed via `RhythmExtractor2013`:

### BeatTrackerMultiFeature (current — `method("multifeature")`)

- Computes 5 onset detection functions simultaneously:
  - Complex spectral difference
  - Energy flux
  - Spectral flux in Mel-frequency bands
  - Beat emphasis function
  - Spectral flux via modified information gain
- Selects best candidates using `TempoTapMaxAgreement`
- ~80% AMLt accuracy
- Returns confidence values
- Slower but more accurate

### BeatTrackerDegara (`method("degara")`)

- Uses only complex spectral difference onset detection
- Faster but lower accuracy
- No confidence estimation (always returns 0)

### RhythmExtractor2013 Parameters

| Parameter | Type | Range | Default | Current |
|-----------|------|-------|---------|---------|
| `minTempo` | integer | [40, 180] | 40 | 40 |
| `maxTempo` | integer | [60, 250] | 208 | 208 |
| `method` | string | {multifeature, degara} | multifeature | multifeature |

### Fundamental Limitation

Essentia's beat tracking is signal-processing-based with no modern deep learning. For complex rhythms (breakbeats, polyrhythms, tempo changes), it will inherently struggle compared to transformer-based approaches. The DBN-like assumptions embedded in `TempoTapMaxAgreement` enforce tempo continuity that breaks on breakbeats.

---

## 3. Alternative Beat Tracking Libraries

### 3.1 Tier 1: ML-Based (Highest Accuracy)

#### Beat This! (CPJKU, ISMIR 2024) — Current State of the Art

- **Repo**: https://github.com/CPJKU/beat_this
- **Paper**: https://arxiv.org/abs/2407.21658
- **Architecture**: Alternating convolutions + transformers over frequency and time
- **Key innovation**: No DBN postprocessing — removes the tempo/meter assumptions that cause half-tempo errors on breakbeats
- **Accuracy**:
  - GTZAN: Beat F1 = 89.1 (prev SOTA: 88.7), Downbeat F1 = 78.3 (prev: 75.6)
  - Ballroom: Beat F1 = 97.5, Downbeat F1 = 95.3
  - Candombe: 99.7/99.7
- **Trained on**: 16+ datasets including solo instruments, time signature changes, and high tempo variation
- **Models**: ~78 MB (full), ~8.1 MB (small)
- **Language**: Python/PyTorch
- **License**: MIT
- **Rust path**: Export to ONNX via `torch.onnx.export()`, run via `ort` crate. The conv+transformer architecture is ONNX-compatible. No official ONNX export provided yet, but straightforward to create.
- **DnB advantage**: By removing the DBN entirely, it avoids the metrical constraints that cause half-tempo errors. This is the single most impactful improvement for DnB/jungle.

#### madmom (CPJKU) — Previous State of the Art

- **Repo**: https://github.com/CPJKU/madmom
- **Algorithms**: `DBNBeatTracker`, `CRFBeatDetector`, `MMBeatTracker`
- **Architecture**: RNNs + Dynamic Bayesian Network postprocessing
- **Accuracy**: Top-ranked in MIREX 2015–2021. In benchmarks on 500 clips across 4 genres, madmom achieved highest alignment with annotated beat positions, outperforming both librosa and Essentia
- **License**: BSD-3-Clause
- **Rust path**: Subprocess — `DBNBeatTracker single INFILE` outputs beat positions (seconds) to stdout, one per line. Parseable via `std::process::Command`.
- **Requires**: Python + numpy + scipy + cython
- **Weakness**: DBN can enforce wrong meter on breakbeats (same fundamental issue as Essentia, just less frequent)

#### BeatNet (ISMIR 2021)

- **Repo**: https://github.com/mjhydri/BeatNet
- **Architecture**: CRNN + Monte Carlo particle filtering
- **Modes**: Streaming, real-time, online, offline
- **Strengths**: Real-time capable (<50ms latency), reportedly "very high" accuracy for electronic music, joint beat+downbeat+meter tracking
- **BeatNet+ (2024)**: Updated with improved CRNN and cascade particle filter
- **Rust path**: Subprocess, or ONNX export of CRNN component (particle filter would need reimplementation)

#### BEAST (2023)

- **Paper**: https://arxiv.org/abs/2312.17156
- **Architecture**: Streaming Transformer with contextual block processing
- **Accuracy**: Beat F1 = 80.04%, Downbeat F1 = 46.78% (at <50ms latency)
- **Best for**: Live/real-time use where latency matters more than accuracy
- **Not suitable** for offline beatgrid generation

### 3.2 Tier 2: Classic Libraries

#### aubio (C)

- **Rust bindings**: https://github.com/katyo/aubio-rs (`aubio-rs` crate)
- **Also**: `bliss-audio-aubio-rs` fork used by bliss-audio
- **Beat tracking**: `Tempo` struct for real-time beat detection
- **Accuracy**: Lower than ML-based. The name literally means "audio with a typo: some errors are likely"
- **Advantage**: Pure C with Rust FFI, very fast, real-time capable, tiny footprint
- **Best for**: Real-time beat sync, not offline grid generation

#### librosa (Python)

- **Beat tracking**: `beat_track()` estimates a single tempo for the entire signal
- **Major limitation**: Assumes constant tempo — not suited for tempo changes or complex rhythms
- **Not recommended** for this use case

#### BTrack (C++)

- **Repo**: https://github.com/adamstark/BTrack
- **From**: Queen Mary University of London
- **License**: GPLv3
- **Dependencies**: libsamplerate + FFTW or Kiss FFT
- **Accuracy**: Below ML-based methods
- **Best for**: Real-time where latency > accuracy

### 3.3 Tier 3: Rust-Native

#### stratum-dsp

- **Repo**: https://github.com/HLLMR/stratum-dsp
- **Pure Rust**, zero FFI
- **Approach**: HMM-based beat tracking with tempo drift correction, dual tempogram (FFT + autocorrelation)
- **BPM accuracy**: 87.7% within ±2 BPM on 155 DJ tracks (vs Mixed-in-Key's 98.1%)
- **Known issue**: ~12% metrical-level confusion (octave errors) — exactly the DnB problem
- **Performance**: ~200–210ms per track
- **Verdict**: Promising for pure Rust but not competitive with ML approaches

#### beat-detector

- **Repo**: https://github.com/phip1611/beat-detector
- **Pure Rust**, `no_std` compatible
- **Designed for**: Live audio beat detection only (0.05ms per step)
- **Not suitable** for offline beatgrid generation

---

## 4. Accuracy Comparison

| Library | Architecture | Beat F1 (GTZAN) | DnB Half-Tempo Issue | Rust Path |
|---------|-------------|-----------------|---------------------|-----------|
| **Beat This!** | Conv+Transformer, no DBN | **89.1** | Solved (no DBN) | ONNX → `ort` |
| **madmom** | RNN+DBN | ~86–88 | Reduced (better RNN) | Subprocess |
| **BeatNet** | CRNN+Particle Filter | ~85–87 | Reduced | Subprocess |
| **Essentia** (current) | Signal processing | ~78–80 | Frequent | Already integrated |
| **aubio** | Signal processing | ~70–75 | Frequent | `aubio-rs` |
| **stratum-dsp** | HMM | ~75 | Frequent (~12%) | Direct Rust dep |
| **librosa** | Onset+DP | ~72–76 | Frequent | N/A |

*F1 values are approximate from published benchmarks. Direct comparison across different evaluation sets is imperfect.*

---

## 5. The Half-Tempo Problem in Detail

### Why It Happens

Beat trackers compute an onset detection function, then search for periodic patterns. In DnB at 170 BPM:
- The **kick** hits on beats 1 and 3 → 170 BPM periodicity
- The **snare** hits on beats 2 and 4 → also 170 BPM periodicity
- But the **snare is louder/more prominent** in the mix → the algorithm often locks onto beats 2 and 4 as "the beats", effectively halving to 85 BPM

The DBN postprocessing (used in madmom, Essentia) then enforces tempo continuity, locking in the wrong tempo.

### Solutions

1. **Beat This!**: Removes DBN entirely → no forced tempo continuity → finds the correct tempo more often
2. **Genre-aware priors**: When genre = DnB, constrain min tempo to 150+ BPM
3. **Drum stem analysis**: Run beat tracking on the separated drum stem (which has clearer kick patterns). Mesh already has stem separation.
4. **Post-hoc octave correction**: The existing `fit_bpm_to_range()` approach — works but loses beat phase information
5. **TU Wien (2015)**: "Addressing Tempo Estimation Octave Errors in Electronic Music" — uses genre classification to select appropriate tempo octave

### Reference
- Pioneer DJ Forums: https://forums.pioneerdj.com/hc/en-us/community/posts/203054289
- TU Wien paper: https://www.ifs.tuwien.ac.at/~knees/publications/hoerschlaeger_etal_smc_2015.pdf
- ISMIR 2012 — Downbeat Detection in Jungle/DnB: https://ismir2012.ismir.net/event/papers/169_ISMIR_2012.pdf

---

## 6. The ONNX Integration Path

The `ort` crate (https://github.com/pykeio/ort) provides production-ready Rust bindings to ONNX Runtime:

### Workflow

1. **One-time (Python)**: Export Beat This! model to ONNX format using `torch.onnx.export()`
2. **Ship**: Include the ONNX model file (~8–78 MB) with the application
3. **Runtime (Rust)**: Load model via `ort`, run inference on audio spectrograms
4. **Post-process (Rust)**: Simple peak-picking on frame-wise beat/downbeat logits

### Advantages

- Hardware acceleration (CUDA, TensorRT, OpenVINO) via ONNX Runtime
- No Python runtime dependency
- 5–10x faster inference than Python for the same model
- Already bundling `libonnxruntime.so` in mesh-cue's deb package

### `ort` Crate Details

- **Version**: 2.0.0-rc.11 (as of Feb 2026)
- **License**: MIT / Apache-2.0
- **Status**: Production-ready, actively maintained
- **Features**: Dynamic batch sizes, multiple execution providers, GPU support

---

## 7. Recommended Strategy

### Short-Term (minimal effort, immediate improvement)

1. Extract real confidence from `RhythmExtractor2013` (stop hardcoding 0.8)
2. Add madmom as a subprocess fallback for tracks where Essentia confidence < 0.6
3. Compare Essentia and madmom results; take the higher-confidence one
4. Consider running beat detection on the drum stem when available

**Effort**: ~1–2 days
**Impact**: Moderate — fewer tracks need manual correction

### Medium-Term (best accuracy/effort ratio)

1. Run madmom's `DBNBeatTracker` via subprocess for all tracks
2. Use madmom beat positions for grid generation instead of Essentia's
3. Keep Essentia for key detection and audio features (it performs well there)
4. Integration: `std::process::Command` → parse stdout float values

**Effort**: ~3–5 days
**Impact**: Significant — madmom consistently outperforms Essentia on beat tracking

### Long-Term (highest accuracy, no Python dependency)

1. Export Beat This! model to ONNX (one-time Python task)
2. Add `ort` as a dependency (already bundling ONNX Runtime)
3. Implement spectral feature extraction in Rust (or reuse Essentia for mel spectrogram)
4. Run inference via `ort`; implement peak-picking post-processing in Rust
5. Remove madmom/Python dependency entirely

**Effort**: ~1–2 weeks
**Impact**: SOTA accuracy, eliminates half-tempo problem, no external dependencies

---

## Sources

- [Beat This! Paper (ISMIR 2024)](https://arxiv.org/abs/2407.21658)
- [Beat This! Repository](https://github.com/CPJKU/beat_this)
- [madmom Repository](https://github.com/CPJKU/madmom)
- [madmom Documentation](https://madmom.readthedocs.io/en/v0.16/modules/features/beats.html)
- [BeatNet Repository](https://github.com/mjhydri/BeatNet)
- [BEAST Paper](https://arxiv.org/abs/2312.17156)
- [Dual-Path Beat Tracking (MDPI 2024)](https://www.mdpi.com/2076-3417/14/24/11777)
- [BIFF.ai — Rundown of Open Source Beat Detection Models](https://biff.ai/a-rundown-of-open-source-beat-detection-models/)
- [stratum-dsp](https://github.com/HLLMR/stratum-dsp)
- [aubio-rs](https://github.com/katyo/aubio-rs)
- [essentia-rs](https://github.com/lagmoellertim/essentia-rs)
- [ort (Rust ONNX Runtime)](https://github.com/pykeio/ort)
- [Essentia RhythmExtractor2013 Docs](https://essentia.upf.edu/reference/std_RhythmExtractor2013.html)
- [Essentia BeatTrackerMultiFeature Docs](https://essentia.upf.edu/reference/std_BeatTrackerMultiFeature.html)
- [MIREX 2025 Audio Beat Tracking](https://www.music-ir.org/mirex/wiki/2025:Audio_Beat_Tracking_Results)
- [Pioneer DJ Forums — DnB Half-Tempo](https://forums.pioneerdj.com/hc/en-us/community/posts/203054289)
- [TU Wien — Tempo Octave Errors in Electronic Music (2015)](https://www.ifs.tuwien.ac.at/~knees/publications/hoerschlaeger_etal_smc_2015.pdf)
- [ISMIR 2012 — Downbeat Detection in Jungle/DnB](https://ismir2012.ismir.net/event/papers/169_ISMIR_2012.pdf)
