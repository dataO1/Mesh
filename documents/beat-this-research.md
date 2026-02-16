# Beat This! — Integration Research for Phase 3 Beat Detection

**Research date:** February 2026
**Paper:** "Beat This! Accurate, Fast, and Lightweight Beat Tracking" (ISMIR 2024, CPJKU)
**Repository:** https://github.com/CPJKU/beat_this
**License:** MIT

## Why Beat This!

Beat This! is the current state-of-the-art for offline beat tracking. It directly addresses every known limitation of our current Essentia-based pipeline:

| Problem | Essentia (current) | Beat This! |
|---|---|---|
| Half-tempo on DnB | Frequent (DBN post-processing causes this) | **Solved** — no DBN required |
| Phase alignment | Only uses first detected beat | Outputs **all** beat positions with high accuracy |
| Downbeat detection | Not supported (Essentia issue #253, open since 2015) | **Built-in** — separate beat + downbeat outputs |
| Beat F1 (GTZAN) | ~78–80 | **89.1** (full) / **88.8** (small) |
| Downbeat F1 | N/A | **78.3** (full) / **77.2** (small) |

## Architecture

### Model Family

Two variants are available:

| Variant | Parameters | Dimensions | Size (ONNX est.) | Beat F1 | Downbeat F1 |
|---|---|---|---|---|---|
| **Full** | ~20M | 512-dim, 16 heads | ~78 MB | 89.1 | 78.3 |
| **Small** | ~2M | 128-dim, 8 heads | ~8 MB | 88.8 | 77.2 |

**The small variant is the target for Mesh.** At 2M parameters and ~8 MB, it's practical for CPU-only inference while achieving nearly identical accuracy to the full model (-0.3 beat F1, -1.1 downbeat F1).

### Network Structure

```
Audio (mono, 22050 Hz)
    │
    ▼
Mel Spectrogram (128 bins, hop=441 → 50 fps)
    │
    ▼
Frontend: Stem + 3 Conv Blocks
  └─ Each block: Conv layers alternating with "partial transformers"
     over frequency and time axes
    │
    ▼
Main Transformer: 6 blocks
  └─ Rotary positional embedding (RoPE)
  └─ Flash attention (or standard attention)
  └─ Small variant: 128-dim, 8 heads
  └─ Full variant: 512-dim, 16 heads
    │
    ▼
Linear heads → beat activation + downbeat activation (per-frame sigmoid)
    │
    ▼
Peak picking (simple argmax-based, no DBN)
    │
    ▼
Beat positions (seconds) + Downbeat positions (seconds)
```

### Key Design Choices (from ablation study)

Most impactful training decisions:
- **Pitch augmentation** (+4.3 beat F1) — critical for generalization
- **Shift-tolerant loss** (+1.4 beat F1) — max-pooling over ±3 frames before BCE loss
- **No DBN post-processing** — the model directly outputs clean beat activations

### Input Specification

- **Sample rate:** 22050 Hz mono
- **Spectrogram:** 128 mel bands, hop size 441 (= 50 frames/second)
- **Chunk size:** 1500 frames = 30 seconds (with overlap for longer tracks)
- **Overlap handling:** Cosine-weighted blending at chunk boundaries

## Integration Path with Mesh

### Preprocessing Pipeline Compatibility

Mesh already has mel spectrogram computation for ML analysis (`mesh-cue/src/ml_analysis/preprocessing.rs`). However, the existing pipeline targets EffNet at 16 kHz / 96 bands. Beat This! requires:

- **Resample to 22050 Hz** (vs 16 kHz for EffNet)
- **128 mel bands** (vs 96 for EffNet)
- **Hop size 441** (vs EffNet's different hop)
- **50 fps output** (vs EffNet's different rate)

A separate preprocessing path would be needed, but the pattern is identical — just different parameters to the mel spectrogram computation.

### ONNX Export

Beat This! is implemented in PyTorch. ONNX export is not officially provided but is straightforward:

```python
import torch
from beat_this.model import BeatThis

model = BeatThis.from_pretrained("small")
model.eval()

# Input: [batch, 1, n_frames, 128] mel spectrogram
dummy = torch.randn(1, 1, 1500, 128)
torch.onnx.export(
    model, dummy,
    "beat_this_small.onnx",
    input_names=["mel_spectrogram"],
    output_names=["beat_activation", "downbeat_activation"],
    dynamic_axes={
        "mel_spectrogram": {0: "batch", 2: "time"},
        "beat_activation": {0: "batch", 1: "time"},
        "downbeat_activation": {0: "batch", 1: "time"},
    },
    opset_version=17,
)
```

**Potential issues:**
- Flash attention may not export cleanly — may need to fall back to standard attention
- Rotary positional embedding should export fine (it's just sin/cos multiplication)
- The `from_pretrained("small")` loads the ~2M parameter variant

### Runtime via `ort`

Mesh already uses `ort` for EffNet inference. The pattern would be identical:

```rust
use ort::session::Session;

let session = Session::builder()?
    .with_optimization_level(GraphOptimizationLevel::Level3)?
    .with_intra_threads(4)?  // CPU parallelism
    .commit_from_file("beat_this_small.onnx")?;

// Input: mel spectrogram [1, 1, n_frames, 128]
let input = Tensor::from_array(([1, 1, n_frames, 128], mel_data))?;
let outputs = session.run(inputs![input]?)?;

let beat_activation: &[f32] = outputs[0].try_extract_tensor()?;
let downbeat_activation: &[f32] = outputs[1].try_extract_tensor()?;
```

### Post-Processing (Peak Picking)

The model outputs per-frame activation values (sigmoid, 0–1). Peak picking is simple:

1. Find local maxima in the activation curve
2. Apply a minimum threshold (0.5 typical)
3. Apply minimum inter-beat distance (based on expected tempo range)
4. Convert frame indices to seconds: `time = frame_index * hop_size / sample_rate`

For downbeats, the same process applies to the downbeat activation output.

This is **dramatically simpler** than Essentia's Viterbi/DBN decoding and eliminates the half-tempo problem entirely.

### CPU Performance Estimate

The small model at ~2M parameters with 128-dim attention:
- EffNet (17MB, ~5M params) processes a 5-minute track in ~2-3 seconds on CPU
- Beat This! small (~8MB, ~2M params) should be comparable or faster
- 50 fps output means a 5-minute track = 15,000 frames
- Expected: **1-5 seconds per track on CPU** (conservative estimate)

This is acceptable for import-time analysis. Essentia's `RhythmExtractor2013` currently takes ~2-4 seconds per track.

## Integration Architecture

### Recommended Approach

```
                    ┌─────────────────────┐
                    │   Import Pipeline    │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │  Drum Stem (mono)    │
                    │  Resample → 22050 Hz │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │  Mel Spectrogram     │
                    │  128 bands, hop=441  │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │  Beat This! (ONNX)  │
                    │  Small variant       │
                    │  via ort Session     │
                    └──────┬────────┬─────┘
                           │        │
                    ┌──────▼──┐ ┌───▼─────────┐
                    │  Beats  │ │  Downbeats   │
                    │  (secs) │ │  (secs)      │
                    └──────┬──┘ └───┬─────────┘
                           │        │
                    ┌──────▼────────▼─────┐
                    │  Grid Construction   │
                    │  Median BPM from IBIs│
                    │  Phase from downbeat │
                    │  Fixed grid output   │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │  DB: first_beat +    │
                    │       bpm + downbeat │
                    └─────────────────────┘
```

### Key Advantages Over Current Pipeline

1. **BPM from inter-beat intervals**: Median of all consecutive beat intervals gives robust BPM without octave errors
2. **Phase from downbeat**: The first downbeat in a non-silent region gives the correct bar alignment
3. **No procspawn needed**: `ort` is thread-safe (unlike Essentia's C++ globals), so no subprocess isolation required
4. **Confidence from activation strength**: Mean peak activation value serves as natural confidence metric

### Model Distribution

Options:
1. **Bundle with binary** — adds ~8 MB to distribution (acceptable)
2. **Download on first use** — like current EffNet model caching in `~/.cache/mesh-cue/ml-models/`
3. **Both** — bundle small variant, download full variant on demand

Option 2 (download on first use) matches the existing pattern for EffNet models and keeps the binary small.

### Migration Strategy

1. Export small variant to ONNX, test locally
2. Add `BeatDetectionBackend` enum to config: `Essentia` (default) | `BeatThis`
3. Implement Beat This! path alongside Essentia (feature flag or config toggle)
4. Compare results on test library, tune peak picking threshold
5. Once validated, make Beat This! the default, keep Essentia as fallback

## Training Data & Generalization

Beat This! was trained on **4,556 tracks across 18 datasets**, covering:
- GTZAN (various genres)
- Ballroom (dance styles)
- SMC (challenging/non-standard meters)
- HJDB (DnB-heavy, important for our use case)
- Various others covering rock, pop, jazz, world music

The HJDB (DnB) dataset inclusion is particularly relevant — the model has seen DnB training data, unlike many beat trackers that struggle with half-time patterns.

## Comparison: Small vs Full Variant

| Metric | Small (target) | Full |
|---|---|---|
| Parameters | ~2M | ~20M |
| Transformer dim | 128 | 512 |
| Attention heads | 8 | 16 |
| ONNX size (est.) | ~8 MB | ~78 MB |
| Beat F1 (GTZAN) | 88.8 | 89.1 |
| Downbeat F1 | 77.2 | 78.3 |
| CPU inference | ~1-3s/track | ~3-8s/track |
| RAM usage | ~50 MB | ~200 MB |

The small variant loses only 0.3 points on beat F1 — well within noise. For CPU-only deployment, the 10x smaller model with near-identical accuracy is the clear choice.

## References

- Foscarin, De Berardinis, et al. "Beat This! Accurate, Fast, and Lightweight Beat Tracking." ISMIR 2024. https://arxiv.org/abs/2407.21658
- GitHub: https://github.com/CPJKU/beat_this
- Pre-trained weights: https://github.com/CPJKU/beat_this/releases
- Related work from same group (CPJKU): madmom, SuperFlux, TCN beat tracker
