# Embedding Models — Research Findings & Evaluation Log

Working document for the `embeddings-upgrade` branch. Captures the candidate
analysis, planned phases, and a rolling observation log as we test each
model against Mesh's similarity index, smart-suggestion scoring, graph view,
and probe-head pipeline.

**Branch:** `embeddings-upgrade` (cut from `main` at 5adf2a8 on 2026-05-02)

**Owner:** Daniel — runs all live A/B tests on actual library, reports back
observations under [Observation Log](#observation-log).

---

## Why we're doing this

Mesh's smart-suggestion stack today (`documents/smart-suggestions-research.md`,
`documents/similarity-search.md`) hangs entirely off Discogs-EffNet's 1280-dim
embedding:

- HNSW (cosine, m=32, ef=300) over the full library.
- PCA cosine + Barnes-Hut t-SNE for the graph view.
- Goldilocks bell-σ scoring, recently re-tuned in commits 6bd1972 / 045aba0
  because the raw distance distribution is awkward.
- Five binary classification heads (danceability, approachability, timbre,
  tonal, mood electronic/acoustic) consuming the 1280-dim vector.
- Aggression/energy axis derived from these heads + EffNet weights.

Two motivations to upgrade:

1. **Geometry** — EffNet is supervised on 400-class softmax. Its embedding
   geometry is shaped by classification loss, not similarity loss. That's
   why the bell-σ tuning is non-trivial. Contrastive / perceptual
   embeddings should produce cleaner cosine distributions and reduce
   calibration burden.
2. **Resolution** — newer models (MAEST 519 styles, MuQ-MuLan perceptual
   triplets, MULE playlist-coherence) capture finer or perceptually more
   meaningful distinctions for electronic music specifically.

---

## Plan

| Phase | Model | Goal | Status |
|---|---|---|---|
| 1 | **MAEST-30s-pw-519l-2** | Same-vendor drop-in replacement; de-risk the pipeline (head retraining, bell-σ re-tuning, HNSW dim change) | **Shipped** |
| 2 | **MuQ-MuLan** (700M) | Highest perceptual-similarity numbers + free text-query path | ONNX export spike in progress |
| 3 | **MULE** (62M) | Best playlist-coherence supervision + cheapest of the high-quality options | Pending Phase 2 review |
| 4 | **Multimodal Audio LLM** (Qwen3-Omni-Captioner / Audio Flamingo 3 / Music Flamingo) | Caption-augmented similarity + structured tag extraction; fused multi-view graph; replaces aggression placeholder with a semantic signal. **mesh-cue only, GPU-gated; player consumes precomputed vectors only.** | Design phase |

Phase 1 ships behind a config flag so EffNet stays available — A/B is the
whole point. Phases 2 and 3 don't start until the user reports back on
Phase 1 outcomes.

---

## Candidate matrix

### Quick comparison

| Model | Params | Dim | ONNX | CPU/30s | License | Verdict |
|---|---|---|---|---|---|---|
| **Discogs-EffNet** (current) | ~5M | 1280 | ✅ | 50–200ms | CC-BY-NC | Baseline |
| **MAEST-30s-pw-519l-2** | 87M | 768 | ✅ official | 1.5–4s | CC-BY-NC | Phase 1 |
| **MULE** (Pandora) | 62M | 1728 | ❌ TF only | 0.5–2s | GPL+CC-BY-NC | Phase 3 |
| **MuQ-MuLan** | 700M | 512 (joint) | ❌ | 5–10s | MIT/CC-BY-NC | Phase 2 |
| **LAION-CLAP music_audioset** | 80M | 512 | ✅ via optimum | ~1s | MIT/CC0 | Complement only |
| MERT-v1-95M | 95M | 768 | ❌ no official | 5–8s | CC-BY-NC | Skip — worse than MULE on genre |
| MERT-v1-330M | 330M | 1024 | ❌ | 3–5× MERT-95M | CC-BY-NC | Skip unless multi-task |
| MusicFM-MSD | 330M | 1024 | ❌ | 5–10s | MIT/Apache | Skip — chord/beat strength, not similarity |
| OMAR-RQ | 580M | — | ❌ | 10s+ | AGPL+CC-BY-NC | Skip — license, novel quantizers |

### Per-model tradeoffs

#### MAEST-30s-pw-519l-2 (Phase 1)

Same vendor and lineage as the current EffNet. PaSST/AST transformer
trained on 4M tracks across 519 Discogs styles (vs EffNet's 3.3M / 400).
Output is `[3, 768]`: CLS token, DIST token, mean of input tokens.
Standard recipe is to use CLS (768-dim) — we'll start there.

**Geometry:** still classification-loss shaped. So the bell-σ tuning won't
disappear, just shift. The win is taxonomy granularity (519 vs 400) and
30s context window (vs ~3s) — captures structure EffNet's short patches
can't.

**Realistic gain:** modest on similarity geometry, larger on
within-electronic-genre granularity. Communities will look very similar
to current EffNet communities, just split a bit finer (~30% more
potential micro-genres but most additions are long-tail).

**Storage:** 1280 → 768-dim drops ~40% (256MB → 154MB at 50K tracks).

**Integration cost:** ~1 day. Same `ort` runtime, same CC-BY-NC license,
official ONNX with dynamic batch.

**Costs:**
- 10–20× slower per-track inference than EffNet (1.5–4s vs 50–200ms) —
  still inside Mesh's seconds-per-track import budget.
- All 5 binary heads need retraining on top of MAEST embeddings
  (labels carry over, 2-class probes train in minutes).
- HNSW + PCA + t-SNE indices need rebuilding.
- Bell-σ Goldilocks zones need re-calibration on a representative
  library.

#### MuQ-MuLan (Phase 2)

SSL Conformer (MuQ) with Mel-RVQ self-supervised target, then a
contrastive stage (MuLan) aligning audio to text via dual towers. The
audio embedding is the projected joint-space vector — *not* the raw SSL
hidden state.

**Why it's the quality leader:** the contrastive joint-space objective
is the closest published analog to what a similarity index actually
wants. CLIP-family geometry is isotropic and cosine-friendly. The 72.4%
Inst-Sim-ABX number is on full mixes (not on artist/album leakage
shortcuts) so it's measuring something close to perceptual similarity.

**Realistic similarity gain:** likely meaningful — perceptual triplets
are a better proxy for "DJ would mix these together" than Discogs
taxonomy hits. Should reduce EffNet/MAEST's failure mode of
over-clustering by *label* (same Discogs sub-style → high similarity
even if the tracks sound nothing alike).

**Realistic community detection gain:** mixed. Joint-space models tend
to produce *fewer, broader* communities because the text alignment
pulls songs toward semantic anchors ("techno", "ambient") rather than
splitting on production fingerprint. Different communities, not
strictly better.

**Costs:**
- 700M params, no ONNX, fp32-only (NaN at fp16), 24kHz input.
- ~10s CPU per 30s clip → 50K-track import goes from hours to ~6 days.
- **Only sane integration is two-stage retrieval** — cheaper recall
  model over the full library, MuQ-MuLan rerank on the top 50–100.
- PyTorch only; either ship a Python sidecar or do `optimum-cli`
  export work.

#### MULE (Phase 3)

NFNet-F0 backbone, 62M params, contrastively trained on Pandora's
MusicSet for **playlist-coherence similarity**. 1728-dim track
embedding pooled over 3s windows / 2s stride.

**Why it ranks high:** ties Jukebox-5B (5 *billion* params) on
MTG-Jamendo genre and beats every MERT variant — at 62M params. The
training objective is essentially "embed tracks such that tracks from
the same hand-curated Pandora playlist land near each other," which is
almost literally what Mesh's suggestion graph wants.

**For similarity:** very strong fit. Contrastive on playlist
co-occurrence produces clean cosine geometry *and* the supervision
signal directly maps to "would a human put these in the same set."
Distance distributions should be cleaner than SSL models.

**For community detection:** likely the best of the bunch for
DJ-relevant communities. Cluster boundaries fall along human-curated
lines.

**Critical caveat:** the playlist supervision is from *Pandora*, not
from DJ sets. "Same playlist" ≠ "would mix together." Coherent
communities, but per-pair similarity for *transition* fitness may be
no better than EffNet.

**Costs:**
- TF SavedModel only; NFNet weight-standardization layers don't always
  trace cleanly through tf2onnx.
- License (GPL-3.0 code + CC-BY-NC weights) is *worse* than current
  EffNet for any future commercial path.
- MusicSet skews Western popular/curated — electronic underground
  coverage is weaker than MAEST's Discogs lineage.

#### LAION-CLAP (complementary, not replacement)

512-dim joint text/audio space. Audio tower is well-trained but
optimized for text-audio matching, not audio-audio fine-grained
similarity. Audio-only HNSW on CLAP underperforms music-specialized
models in published comparisons.

**Use case:** add as a *parallel* index for text → audio query ("dark
hypnotic techno around 132 BPM"). Doesn't disturb the primary
similarity pipeline. The distilled DCLAP variant (~30MB ONNX) is
attractive — 5–6× faster than LAION-CLAP, 0.884 cosine to teacher.

#### Skipped models (with reasoning)

- **MERT-v1-95M / 330M** — MARBLE genre numbers worse than MULE at 5×
  the cost. SSL anisotropy needs whitening before HNSW. Pick only if
  multi-task (genre + chord + beat probes) amortizes the backbone.
- **MusicFM-MSD** — best on chords/beats/structure, not similarity.
  Worth considering only as part of a single-backbone consolidation
  (replace Beat This! + key + similarity in one shot — much bigger
  decision).
- **OMAR-RQ** — research artifact. AGPL, novel RVQ/FSQ ops don't trace
  to ONNX, no probe heads shipped. Months of integration for SSL
  features that still need whitening for HNSW use.
- **Jukebox / OpenL3 / PaSST / AST / BEATs** — out of scope for
  reasons covered in initial research.

---

## Phase 1: MAEST drop-in details

### Migration scope

- `crates/mesh-cue/src/ml_analysis/models.rs` — add `MaestEmbedding`
  variant alongside `EffNetEmbedding`. Both stay in `base_models()` so
  we can A/B without re-downloading.
- `crates/mesh-cue/src/ml_analysis/inference.rs` — add MAEST loading
  path, MAEST forward call (input shape, output extraction), feature
  flag / config switch to choose which embedder feeds the downstream
  heads + HNSW.
- `crates/mesh-cue/src/ml_analysis/preprocessing.rs` — verify mel
  parameters match MAEST's training (16 kHz, 96 mel bands — same as
  EffNet — confirm hop size + frame count for 30s clips).
- HNSW index in `mesh-core` — accept 768-dim alongside existing
  1280-dim during the transition window.
- Probe heads (timbre / tonal / acoustic / electronic / danceability /
  approachability / voice / Jamendo mood) — these are 2-class softmax
  / regression heads trained on top of EffNet embeddings. They will
  *not* work on MAEST embeddings without retraining. Phase 1 either:
  - (a) Keeps EffNet running in parallel for the heads while MAEST
    drives only similarity/HNSW, or
  - (b) Disables heads when MAEST is selected and we retrain
    afterwards.
  Decision pending — option (a) is the lower-risk path for A/B.

### Confirmed ONNX I/O signature (verified 2026-05-02 from Essentia metadata JSON)

**ONNX:** `https://essentia.upf.edu/models/feature-extractors/maest/discogs-maest-30s-pw-519l-2.onnx`
(348 MB, version 2, released 2025-01-22, sample_rate 16 kHz).

**Sibling metadata JSON:**
`https://essentia.upf.edu/models/feature-extractors/maest/discogs-maest-30s-pw-519l-2.json`
(contains the full 519-class label list — bring it into Mesh as a
sibling fixture, don't hardcode a 519-name table by hand).

**Input:**

| field | value |
|---|---|
| name | `melspectrogram` |
| dtype | `float32` |
| shape | `[1, 1876, 96]` (batch × time × mel_bands) |
| sample rate | 16 000 Hz |

Same mel band count as EffNet (96), same sample rate (16 kHz). The only
preprocessing change vs EffNet is the time dimension: **1876 frames per
inference call instead of 128.** Mesh's current
`preprocessing::MelSpectrogramResult` produces a 96-band mel — reusable.
We just extract a 1876-frame window instead of stacking 128-frame patches.

For tracks shorter than ~30s of mel frames: pad with zeros to 1876.
For longer tracks: extract multiple stridden 1876-frame windows and
average the resulting embeddings (same averaging strategy as EffNet,
just with bigger windows and far fewer of them per track).

**Outputs (the model exposes 14 of them):**

| index | name | shape | role |
|---|---|---|---|
| 0 | `PartitionedCall/Identity` | `[1, 519]` | logits (style classifier) |
| 1–12 | `PartitionedCall/Identity_1` … `_12` | `[1, n_tokens, 768]` | per-layer embeddings, layers 1–12 |
| 13 | `PartitionedCall/Identity_13` | `[1, 519]` | sigmoid predictions |

The metadata flags **`PartitionedCall/Identity_7`** (layer 7) with
`output_purpose: "embeddings"` — that's the canonical embedding output.

`n_tokens` is the dynamic transformer token count after AST patching
(includes CLS at index 0, DIST at index 1, then signal tokens). Per
the MAEST paper, the recommended pooled embedding is
`stack(CLS, DIST, mean(signal tokens))` at layer 7 → **2304-dim**
(3 × 768).

**ort tip:** request only the outputs we use to avoid the model
returning all 14 every call. We need:
- `PartitionedCall/Identity` (519 logits — for genre)
- `PartitionedCall/Identity_13` (519 sigmoid — alternative for genre)
- `PartitionedCall/Identity_7` (layer-7 embeddings — for similarity + heads)

If `ort` doesn't easily filter outputs we just ignore the other tensors
in extraction.

### Resolved design choices

- **Pooling:** stack `[CLS; DIST; mean(signal)]` at layer 7 → 2304-dim.
  This is the paper-recommended recipe and matches what Essentia uses
  internally. Storage at 50K tracks: 2304 × 4 bytes = 9.0 KB/track →
  450 MB total. Larger than EffNet (256 MB) and *larger* than CLS-only
  (154 MB), but the quality justifies it for the A/B. We can fall back
  to CLS-only later if benchmarks show no difference.
- **Window strategy:** single 1876-frame (30s of mel) window per
  inference call. Multiple stridden windows for longer tracks, then
  average. Pad to 1876 for short clips.
- **Genre classifier:** keep using the model — 519 sigmoid output at
  `Identity_13` is a direct upgrade over EffNet's 400 softmax. New
  label list goes into a sibling fixture file.

### Phase 1 design — final

- **EffNet is gone.** Branch is MAEST-only; no parallel A/B in code.
  Goldilocks bell-σ tuning will need to be redone end-to-end against
  the new distance distribution.
- **All EffNet classification heads removed wholesale** at user request
  — `vocal_presence`, `mood_themes`, `binary_moods`, `danceability`,
  `approachability`, `reverb`, `timbre`, `tonal`, `mood_acoustic`,
  `mood_electronic`. The `MlAnalysisData` struct is now just
  `top_genre` + `genre_scores`. Track table loses Timbre and
  Danceability columns; auto-tag emits genre tags only; aggression
  scoring drops the mood-tag contribution (constant 0.0 placeholder
  until re-derived from the MAEST embedding).
- **Embedding pooling:** stack `[CLS; DIST; mean(signal)]` at layer 7
  → 2304-dim. Paper-recommended recipe.
- **Window strategy:** 1876-frame (30s of mel) windows, 50% overlap,
  zero-pad short tracks, average embeddings + sigmoid genre
  predictions across windows.
- **DB migration:** the legacy `ml_embeddings` relation
  (`<F32; 1280>`) is dropped + recreated at `<F32; 2304>`; the
  `ml_analysis` relation drops its 11 head columns. Existing user DBs
  re-run analysis to repopulate. No rollback path on this branch.

---

## Observation Log

Append-only. User reports go here. Format: timestamp, model under test,
setup, observation, action.

### 2026-05-02 — Phase 1 kickoff

- Branch `embeddings-upgrade` cut from `main` (5adf2a8).
- Research findings doc created.
- Memory persisted (`memory/project_embedding_upgrade.md`).
- TODO.md "Embedding Model Upgrade" section added.
- MAEST ONNX I/O signature verified against Essentia's metadata JSON
  (input `melspectrogram[1,1876,96]`, embedding output
  `PartitionedCall/Identity_7[1,n_tokens,768]`, genre output
  `Identity_13[1,519]` sigmoid). 519-class label list extracted from
  the JSON metadata, not hand-typed.

### 2026-05-02 — MAEST drop-in landed

- `MlModelType` collapsed to single `MaestEmbedding519l` variant.
  Download URL: Essentia model hub
  (`discogs-maest-30s-pw-519l-2.onnx`, ~348 MB).
- `MlAnalyzer` rewritten: only loads MAEST, runs 30s windows
  (50% overlap), pools to `[CLS|DIST|mean(signal)]` at layer 7
  → 2304-dim. Genre decoded from sigmoid output, top-10 above 0.05.
- All 9 EffNet classification heads removed from the codebase
  (Jamendo mood, voice/instrumental, timbre, tonal, mood_acoustic,
  mood_electronic, danceability, approachability, NSynth reverb).
- `MlAnalysisData` now just `{ top_genre, genre_scores }`. DB schema
  collapsed to match. Migration triggers when the legacy
  `vocal_presence` column is detected.
- `ml_embeddings` Cozo relation widened from `<F32; 1280>` to
  `<F32; 2304>` with drop+recreate migration. HNSW index rebuilt.
- Track table drops Timbre + Danceability columns. Auto-tag emits
  genre tags only. Aggression scoring's mood-tag contribution
  replaced with 0.0 placeholder (will be re-derived from MAEST
  embedding in a follow-up).
- `cargo check --workspace --no-default-features --all-targets`
  passes (warnings only, none from the modified files).

**Next user step (manual):**
1. Run mesh-cue, trigger ML analysis or re-analysis on a small set of
   tracks (10–50). First run downloads the 348 MB MAEST ONNX.
2. Verify embeddings populate (`db_inspect` or query
   `ml_embeddings` directly). Expected `vec.len() == 2304`.
3. Test smart-suggestion lookups against the new HNSW. Note any
   surprising rankings, distance-distribution oddness, or import-time
   regressions.
4. Report back observations here for Phase 2/3 planning.

<!-- Append observations below this line. -->

### 2026-05-02 — Phase 1 evaluation closed

Real-world results on a 910-track electronic library (DnB/techno/house mix):

- **PCA build:** 854 → 131-dim @ 95 % variance (vs EffNet's 128/95 %). Higher
  PCA dim = more semantic signal surviving compression. Good.
- **Smart-suggestion top-10 from a liquid-DnB seed**: 7 of 10 are correct
  DnB neighbours (Bad Robot, About U, Back in Those Days, Hard Noize,
  Phaselock, Forgo, Just a Feeling). MAEST is locking the right
  neighbourhood. Strong.
- **Score distribution on first run is bottom-heavy** — 244/342 tracks
  sit at the 0.044 floor. Cause: the harmonic Camelot Strict filter +
  bell-σ curves were tuned against EffNet's distance distribution and
  no longer fit MAEST's cleaner cosine geometry.
  - vec component median = 0.20 (the floor) → bell σ wants to be ~0.25–0.30
    instead of 0.18.
  - aggr component median = 0.25 (the floor) → expected: the mood-tag
    term is at the constant-0 placeholder until re-derived from MAEST.
  - key=0.93 vs key=0.59 split with nothing in between → Camelot Strict
    plus harmonic gate is doing what it's told but masking candidates.
- **Genre tagging quality**: 519-class taxonomy returns recognisable
  labels (Drum n Bass, Disco, Experimental, African) on real tracks.
  Discogs-style taxonomy is qualitatively better than EffNet's 400.
- **Performance after the per-thread analyzer + window-cap fix**:
  854-track reanalysis in ~26 minutes on a 24-core machine. CPU peak
  ~70 %. Acceptable for a one-time pass.
- **Two known regressions, both calibration not architecture**:
  bell-σ retune is needed; aggression mood-term re-derivation is
  needed. The pipeline itself is sound.

Phase 1 verdict: **MAEST is shipped, working, and producing meaningful
similarity / clustering / suggestions. Branch left alive on
`embeddings-upgrade` for any further calibration.** Phase 2 (MuQ-MuLan)
proceeds on a fresh branch off `embeddings-upgrade`.

---

## Phase 2: MuQ-MuLan ONNX export spike

Branch `muq-mulan-eval` (off `embeddings-upgrade` at 7790c0c, 2026-05-02).

### Why MuQ-MuLan

Highest published perceptual-similarity numbers in the open-source music
embedding space (MuLan paper, ICLR 2025):

- 72.4 % Inst-Sim-ABX (full mix), 90.4 % with stem-separation reweighting
- 79.3 % MagnaTagATune zero-shot ROC-AUC
- Joint 512-dim audio+text space → unlocks text-query for free
  ("dark hypnotic techno around 132 BPM")

Headline trade-offs vs MAEST:

| Axis | MAEST | MuQ-MuLan |
|---|---|---|
| Total params | 87 M | **630 M** (310 M audio tower + 320 M text tower) |
| Embedding dim | 2304 | **512** (78 % storage win) |
| Model file | 348 MB | **2.65 GB** |
| CPU per-window | ~1.5 s | ~4–6 s desktop / ~10–15 s laptop |
| Reanalysis 854 tracks (24-core) | ~6 min | ~14 min |
| GPU (RTX 3060 / 4070) | n/a | **<2 min** |
| VRAM @ fp32 | n/a | ~3.3 GB (fits 6 GB cards) |
| License | CC-BY-NC | MIT code, CC-BY-NC weights |

**Hard constraints from upstream:**
- fp32 only — fp16 produces NaN, no bfloat16 path
- 24 kHz mono input (vs MAEST's 16 kHz)
- 128-band mel preprocessor (vs MAEST's 96-band)
- No genre logits — emits 512-d audio embedding only
- Open-source weights are MSD-trained variant; paper numbers are upper
  bounds for the actual checkpoint

**Decisions locked in for this branch (user-confirmed 2026-05-02):**
- Genre tagging is **not required** — drop it cleanly when MuQ-MuLan
  replaces MAEST. `genre_map.rs` and `auto_tag_from_ml`'s genre block
  become dead code or get repurposed.
- macOS / Apple Silicon is **not a priority** — accept CPU-only on Mac;
  no Python sidecar fallback specifically for MPS.
- Text-query feature: **deferred** — get audio embedding working first;
  add text encoder + query path as a follow-up.

### Architecture decision: ONNX export first, sidecar fallback only if it fails

Per the integration-options analysis (full table in the prior research),
the ONNX path is the only one that preserves Mesh's single-binary
distribution + per-thread `ort` session pattern + free CPU/CUDA branching
via execution providers. Cost: 1-day spike to validate. Upside: drop-in
replacement at the `MlAnalyzer` level.

**Spike pass criteria:**
1. Audio tower exports cleanly to ONNX opset ≥17 with dynamic batch axis.
2. Inference output cosine-matches PyTorch reference within 1e-4 on the
   same input mel.
3. CPU inference time per 30-s window is within 1.5× of native PyTorch
   (sanity check that `ort` isn't bottlenecking).

**Spike fail → fallback plan:** scope the Python sidecar (option B) —
adds Python runtime to the distribution but unblocks GPU paths via the
upstream torch.cuda. This is a multi-week packaging refactor.

### Spike implementation plan — *isolated from `crates/`*

The spike must not touch `crates/mesh-cue/src/ml_analysis/` or any other
production code. Everything lives in the existing `nix/apps/` conversion
pattern that's already used for EffNet-head / Beat This! / Demucs ONNX
exports.

**Files added (all outside `crates/`):**
- `nix/apps/convert-muq-mulan-model.nix` — flake app following the same
  template as `convert-ml-model.nix` and `convert-beat-model.nix`. Wires
  Python deps via `pip install --target tmp_dir` (no permanent venv,
  cached between runs in `/tmp` like the existing apps).
- `nix/apps/convert-muq-mulan/` (subdirectory if helpful) — actual
  Python script(s) the shell wrapper invokes:
  - `download.py` — `huggingface_hub.snapshot_download("OpenMuQ/MuQ-MuLan-large")`
  - `export.py` — instantiate the MuQ-MuLan audio tower, trace via
    `torch.onnx.export(..., opset_version=17, dynamic_axes={...})`,
    save to `models/muq-mulan-audio-tower.onnx`
  - `validate.py` — load both PyTorch and ONNX models, run on a 30-s
    test clip, assert cosine ≥ 0.9999. Print pass/fail + actual cosine.
  - `bench.py` — wall-clock timing per window on CPU; print N samples'
    quartiles. (No GPU bench in the spike — that's a follow-up after
    integration.)
- `flake.nix` — register `convert-muq-mulan-model` flake app the same
  way `convert-ml-model` is wired (lines 285–290).

**Python deps to add (via pip in the conversion app's tmp_dir):**
- `torch>=2.2,<2.6` — **GPU-accelerated by default if CUDA is detected.**
  The conversion app probes for `nvidia-smi` (or `CUDA_VISIBLE_DEVICES` /
  `--gpu` flag) and installs from the appropriate PyPI index:
  - GPU detected → `pip install torch --index-url https://download.pytorch.org/whl/cu124`
  - No GPU → `pip install torch --index-url https://download.pytorch.org/whl/cpu`
  The user's dev box has a CUDA GPU so the spike will use it. Tracing
  a 630M model on GPU is seconds vs ~10–20 minutes on CPU.
- `transformers>=4.40` (xlm-roberta tokenizer + base model)
- `huggingface_hub>=0.20` (snapshot_download)
- `librosa>=0.10` (24 kHz resample, 128-band mel)
- `numpy<2.0` (PyTorch wheel compatibility)
- `onnx>=1.15`
- `onnxruntime>=1.18` for CPU validation; **`onnxruntime-gpu>=1.18`** if
  we also want to validate the ONNX runs on GPU during the spike (free
  signal that production CUDA path will work end-to-end)
- `optimum[exporters]>=1.20` (alternative export pathway if `torch.onnx`
  can't handle the Conformer)

**ONNX file is device-agnostic** — exported once on GPU, runs on CPU OR
GPU at inference time depending on the `ort` execution provider. The
GPU-accelerated tracing is purely a developer-convenience win; doesn't
change what we ship to users.

**Sequence:**
1. **Pre-flight**: shell wrapper detects CUDA via `nvidia-smi -L` (or
   `--cpu` flag override) and picks the appropriate PyTorch wheel index.
   Logs `Using CUDA device: <name>` or `No CUDA detected — using CPU`.
2. `nix run .#convert-muq-mulan-model` invokes `download.py` →
   pulls the ~2.65 GB checkpoint into a Nix-store-friendly cache
   (`$XDG_CACHE_HOME/mesh-spike/muq-mulan/`).
3. `export.py` instantiates `MuQMuLan.from_pretrained(...).to(device)`
   where `device` is `cuda` if available else `cpu`, isolates the audio
   tower (`.audio_encoder` or equivalent — confirm exact attr name from
   the HF model card), and runs `torch.onnx.export` with:
   - dynamic axes `{0: "batch"}` on input
   - opset_version=17
   - `do_constant_folding=True`
   - input shape `[1, 720000]` (24 kHz × 30 s) OR `[1, 750, 128]` mel
     depending on where in the pipeline we cut the export boundary
   - tracing happens on GPU when available (seconds vs ~10–20 minutes
     on CPU); the resulting ONNX file is device-agnostic
4. `validate.py` round-trips a sine-wave + a real audio clip through
   both PyTorch (on the same device used for export) and ONNX (on CPU
   via `onnxruntime`), asserts cosine ≥ 0.9999 per output dim. If
   `onnxruntime-gpu` is installed, also validates the CUDA execution
   provider produces matching output — confirms the production CUDA
   path will work without surprises.
5. If steps 1–4 succeed, output is `models/muq-mulan-audio-tower.onnx`
   (~1.2 GB expected). Document the I/O signature in
   `documents/muq-mulan-onnx-export.md` (input tensor name + shape,
   output tensor name + shape) — same template MAEST got from the
   Essentia metadata JSON.
6. **Stop here for the spike.** Do not touch `crates/`. The spike's
   only deliverable is "does the audio tower export cleanly to ONNX,
   and does it produce numerically-correct embeddings on both CPU and
   GPU runtimes?"

**Devshell impact:** the existing `nix/devshell.nix` already has a
commented-out `pythonEnv` block (lines 9–17). The spike doesn't need to
uncomment it — the conversion app brings its own Python via the
`writeShellScriptBin` + `pkgs.python311.withPackages` pattern, exactly
like `convert-ml-model.nix` does. Devshell stays lean.

**Time budget:** 1 day for the export attempt. If it fails, the failure
mode is documented (which op didn't trace, what error message) and
becomes input to the sidecar-fallback scoping doc.

### Open tasks (Phase 2)

- [ ] Add `nix/apps/convert-muq-mulan-model.nix` (template:
  `nix/apps/convert-ml-model.nix`). Includes CUDA auto-detection via
  `nvidia-smi -L` and `--cpu`/`--gpu` flag overrides.
- [ ] Wire `convert-muq-mulan-model` into `flake.nix` apps list
- [ ] Write `nix/apps/convert-muq-mulan/download.py` —
  `huggingface_hub.snapshot_download("OpenMuQ/MuQ-MuLan-large")`
- [ ] Write `nix/apps/convert-muq-mulan/export.py` — picks
  `cuda`/`cpu` device, traces on GPU when available; tries
  `torch.onnx.export` first; falls back to `optimum-cli export onnx`
  if traces fail
- [ ] Write `nix/apps/convert-muq-mulan/validate.py` (cosine ≥ 0.9999
  PyTorch vs ONNX-CPU on synthetic + real audio; also validate
  ONNX-CUDA EP if `onnxruntime-gpu` available — confirms the
  production GPU path won't surprise us at runtime)
- [ ] Write `nix/apps/convert-muq-mulan/bench.py` (wall-clock per window
  on CPU + GPU if present, n=20 samples)
- [ ] Run the spike end-to-end on the user's GPU box. Capture pass/fail.
- [ ] Document the resulting I/O signature in
  `documents/muq-mulan-onnx-export.md` if the export succeeds
- [ ] **Decision gate:** if spike passes → scope the production
  integration (new `MlModelType::MuQMuLanLarge` variant, schema
  migration `<F32; 2304>` → `<F32; 512>`, preprocessing rewrite for
  24 kHz / 128-band mel, drop genre handling cleanly, `ort` CUDA
  feature flag for users with GPUs). If spike fails → scope the
  Python sidecar fallback architecture in a separate doc.

### Spike findings — STFT export blocker + resolution (2026-05-02)

First export attempt failed with `STFT does not currently support
complex types` from the legacy TorchScript exporter. The dynamo
fallback also failed because `torch.onnx.dynamo_export` was removed
in PyTorch 2.6 (replaced by `torch.onnx.export(..., dynamo=True)`).

Root cause: `MuQModel.preprocessor_melspec_2048` is a
`torchaudio.transforms.MelSpectrogram` which calls `torch.stft`, and
STFT returns complex tensors that the legacy exporter cannot represent.
Same class of failure as the prior MAEST mel-frontend export.

**Resolution:** cut the export boundary above the mel — same split we
already use for MAEST. The wrapper monkey-patches
`MuQModel.get_predictions` so it accepts a pre-computed mel directly
and skips both `preprocessing` (the STFT) and `normalize` (a dict
comprehension that doesn't trace cleanly either).

Producer responsibilities now split:
- **Python (export side):** writes a sidecar `<onnx>.norm.json` with
  the model's `melspec_2048_mean`/`melspec_2048_std` plus the MelSTFT
  parameters (`sample_rate=24000`, `n_fft=2048`, `hop_length=240`,
  `n_mels=128`, `is_db=true`, `trim_last_frame=true`, `clip_secs=10`).
- **Rust (inference side, when integrated):** computes mel matching
  those params, applies `(mel - mean) / std`, feeds the (1, 128, 1000)
  tensor to ONNX. Exact same shape pattern as MAEST mel.

Single-clip semantics: the ONNX wraps ONE 10 s clip's worth of mel and
returns a 512-d embedding. PyTorch's native `extract_audio_latents`
chops a long waveform into 10 s clips and averages — Rust will do the
same chop+average outside ONNX (mirrors MAEST's window averaging).

Validate / bench scripts updated to feed mel; `validate.py` computes the
reference mel by calling the loaded `MuQModel.preprocessor_melspec_2048`
+ stats, so a passing cosine gate also confirms Rust's eventual mel
implementation must match those exact params.

### Out of scope for this branch

- Text-query feature (deferred to Phase 3)
- macOS GPU acceleration (no Apple users on the priority list)
- Genre tagging (dropped per user direction)
- Production integration into `crates/` (only after spike passes)

---

## Phase 4: Multimodal Audio LLM Layer (caption-augmented, fused-graph)

**Status:** Design phase. No branch cut yet. Independent of Phase 2 — can land
on top of either MAEST-only or MuQ-MuLan-only baselines.

### Premise

Pure audio embeddings (MAEST, MuQ-MuLan, MULE) capture timbre and structure
but cannot describe a track in DJ-relevant terms — production qualities,
mix-friendliness, vocal type, structural cues, blend descriptors. Adding a
**second modality** (free-text caption from an audio LLM, embedded with a
sentence encoder) and fusing the two graphs gives:

- A semantic signal to replace the constant-0 aggression mood-tag placeholder.
- A natural way to surface genre blends that the 519-class taxonomy can't
  represent.
- A cross-modal disagreement signal for outlier / QC detection — tracks
  whose audio neighbours and caption neighbours disagree are interesting.
- Free-text user queries against the library without retraining anything.

**Hard architectural constraint:** all heavy work happens in `mesh-cue` at
import / reanalysis time. `mesh-player` never runs an LLM, never even loads
one. Player consumes precomputed float vectors + small JSON tag bundles —
exactly the shape it consumes today, just with better numbers in it. USB
sync ships the same artifact format.

### Model selection

User profile target: strong consumer GPU (24 GB VRAM class). H100/A100 not
assumed. Three tiers:

| Tier | Model | VRAM | Role | Verdict |
|---|---|---|---|---|
| Default | **Qwen3-Omni-30B-A3B-Captioner-AWQ-4bit** | ~20 GB | Caption generator (audio-in → text-out only, ≤30 s clip, no prompt accepted) | **Recommended baseline.** Apache-style weights, vLLM serving, fits 24 GB. Caption only — no Q&A. |
| Default Q&A path | **Audio Flamingo 3** (~7–8 B) | ~16–20 GB | Prompt-driven Q&A, multi-audio in single call, JSON-mode tag extraction, pairwise comparisons | **Recommended companion.** Easier to deploy than Music Flamingo. Apache-2.0. |
| Aspirational | **Music Flamingo** (8 B / ~36 GB FP16) | 36 GB+ | Strongest musical literacy (harmony / structure / theory), 20-min audio context | Out of reach on 24 GB without aggressive quantisation. Research-only license. Watch for 4-bit release. |
| Skip | **Nemotron 3 Nano Omni** | — | Audio path is Parakeet (speech-tuned). Not music-aware enough. |
| Watch | **MOSS-Audio** (OpenMOSS, Apr 2026) | varies | Time-aware QA is interesting for cue-point suggestions. Less battle-tested. |

The Captioner is the simplest first step because it has zero prompt design
surface — pass audio, get a caption. A second pass with Audio Flamingo 3
(or Qwen3-Omni-Instruct) extracts structured fields via JSON-mode prompt.

### Two embedding spaces per track

After this phase the system has **two parallel float vectors per track**:

1. **Audio embedding** — MAEST 2304-dim today, MuQ-MuLan 512-dim if Phase 2
   ships. Captures sound.
2. **Caption embedding** — caption text passed through a small sentence
   encoder. Captures description.

Recommended sentence encoder: **BGE-M3** (BAAI, ~570 MB, 1024-dim) — current
SOTA general-purpose, strong on long descriptive text. Run on GPU during
import; the resulting vectors are CPU-cheap forever after. Alternative
fallback: `all-MiniLM-L6-v2` (~90 MB, 384-dim) if storage matters.

Both vectors live in CozoDB. Both feed HNSW / PCA / t-SNE / Louvain / HDBSCAN
identically — every algorithm currently in the pipeline works on either.

### Pipeline (mesh-cue, import-time, GPU-gated)

Behind a `StrongGpuProfile` config flag — default off. When enabled, after
MAEST analysis runs but before HNSW index build:

1. **Caption pass** — Qwen3-Omni-Captioner-AWQ-4bit on the same 30 s window
   MAEST already extracts. ~5–15 s/track on a 24 GB GPU via vLLM. Output:
   one ~200-word free-text caption.
2. **Structured tag pass** — Audio Flamingo 3 (or Qwen3-Omni-Instruct) with
   a JSON-mode prompt against the same window. Output:
   `{aggression, dark, hypnotic, euphoric, mix_friendly_intro, vocal_type,
   structure_timestamps, has_voice_tag, ...}` — scalars in [0,1] plus
   enums / timestamps. ~3–8 s/track.
3. **Caption text embedding** — BGE-M3 over the caption string.
   1024-dim vector. ~30 ms/track on GPU.
4. **Persist** — new CozoDB relations:
   - `track_captions { track_id, caption_text, caption_vec[1024], tags_json,
     llm_model_id, captioner_version, generated_at }`.
   - HNSW index on `caption_vec` (same pattern as `ml_embeddings`).
5. **Multi-view graph build** — see fusion strategy below. Outputs the fused
   PCA reduction + the fused community graph + the refit aggression axis.
   Same artifacts the current pipeline ships, just better numbers.

Cost at scale: ~10–20 GPU-hours one-time for a 5 000-track library. Fine
for re-analysis, must not block batch import. Implement as a separate
post-MAEST stage that can also run later on demand.

Determinism: pin model + temperature=0 + seed. Captions are still
generative — we treat them as advisory data, not ground truth, and never
overwrite user-set fields (cue points, drop marker, tags) with LLM output.
LLM-derived suggestions surface as *proposals*, the user accepts or
ignores them.

### Fusion strategy

Three patterns, in increasing sophistication. Spike (a) first; only escalate
to (b) or (c) if needed.

**(a) Concatenation + re-PCA.** Stack `[α·MAEST_vec | β·caption_vec]`,
re-PCA to 95 % variance. Tunable α/β (start at α=β=1, normalise each block
by its mean L2 norm). Cheapest. The aggression PCA axis you already have
generalises directly to the new fused space — refit using the LLM-derived
`aggression` scalar as the target.

**(b) Multi-view k-NN graph (recommended).** Build *two* k-NN graphs — one
from MAEST, one from captions — and combine:

- *Edge-weight blend:* `w_ij = λ·sim_audio(i,j) + (1−λ)·sim_text(i,j)`
  with λ exposed as a slider in the graph view.
- *Top-k intersection:* a track is a "real" neighbour only if it appears in
  both modalities' top-k. This is the version that lights up outliers
  automatically — disagreement between modalities is the signal.

Run Louvain / HDBSCAN on the fused graph as today. No algorithmic changes
needed downstream.

**(c) Cross-modal alignment via a learned projection.** Fit a small linear
map (or 2-layer MLP, ~1k params) that pulls each track's MAEST vec toward
its caption vec — a tiny CLIP-style alignment trained on the user's own
library. Optional. Only worth the complexity if (a) and (b) leave obvious
geometry holes in real usage.

### Aggression axis — replace the placeholder

Two routes, not exclusive:

1. **Direct LLM scalar.** Use the `aggression` field from the JSON tag
   bundle as the aggression value. Simplest. Caveat: LLM scalars are
   generations, not calibrated regressions — noisy in absolute terms.
2. **LLM-as-labeller, MAEST-as-scorer.** Use LLM-derived coarse buckets
   (low/med/high) as supervised labels and fit a regression head on top of
   the MAEST embedding. Output is calibrated, runtime is fast vector
   math. **This is the cleaner pattern** — LLM provides semantics, MAEST
   provides calibration, player runtime stays cheap.

Both refit cleanly into the existing `aggression_axis` PCA artifact.

### What ships to player / USB

Identical artifact shape to today, just better numbers:

- Fused PCA-reduced vector (~50 floats after 95 %-variance reduction).
- Aggression scalar (now derived from a real semantic signal).
- Structured tag JSON (~200 bytes/track).
- Optional: caption text itself (~1 KB/track) if surfaced in player UI;
  skip if USB size matters.

Player runtime cost is **identical to today** — cosine over a small float
vector. No LLM at mix time, ever. No model loading. No extra dependencies
in the player binary.

### Open tasks (Phase 4)

- [ ] Spike: cut a `multimodal-llm-eval` branch off the latest `embeddings-
  upgrade` head. Capture caption + tags for a 100-track sample of the
  user's library. Manual quality eval — does Qwen3-Omni-Captioner produce
  useful descriptions of techno/DnB specifically (the published benchmarks
  lean pop/rock).
- [ ] Wire vLLM serving in a separate Python sidecar process — `mesh-cue`
  posts audio chunks via local HTTP. Sidecar lifecycle: spin up on
  StrongGpuProfile-enabled batch import, shut down after.
- [ ] Add CozoDB relation `track_captions { track_id, caption_text,
  caption_vec[1024], tags_json, llm_model_id, captioner_version,
  generated_at }`. HNSW index on `caption_vec`.
- [ ] Implement fusion pattern (a) — concatenation + re-PCA. Compare
  community structure against MAEST-only baseline on the user's library.
- [ ] Implement fusion pattern (b) — multi-view k-NN with intersection
  rule. Add λ slider to graph view. Surface cross-modal disagreement as
  an "outlier" overlay.
- [ ] Refit aggression axis via "LLM-as-labeller, MAEST-as-scorer" pattern.
  Replace the constant-0 placeholder.
- [ ] Decision gate: ship behind `StrongGpuProfile`. If quality wins are
  marginal on real techno/DnB content, scope down to *only* the
  aggression-relabelling use case (smallest blast radius, biggest
  immediate fix to a known bug).

### Out of scope for Phase 4

- Player-side LLM inference (architecturally rejected — player is CPU /
  precomputed vectors only).
- Live caption generation during mixing (same reason).
- Replacing MAEST or MuQ-MuLan with the LLM. Captions augment audio
  embeddings; they don't substitute for them. MAEST is deterministic,
  fast, and captures timbral nuance below the lexicon. Captions are
  generative and language-shaped. Both modalities together beat either
  alone — that's the entire premise of (b).

### LLM-as-validator for the audio embeddings

A natural follow-on question: can the LLM directly validate the audio
embedding's similarity calls — i.e. judge whether MAEST / MuQ-MuLan got a
neighbour right or wrong?

**Yes, in three concrete forms:**

1. **Pairwise similarity judgement.** For a candidate seed→neighbour pair
   from MAEST's HNSW top-k, prompt Audio Flamingo 3 (multi-audio in a
   single call) with: *"Are these two tracks similar in mood, energy, and
   production? Answer yes/no/borderline plus a one-sentence reason."* Run
   on a stratified sample of N=200–500 pairs, compute agreement rate vs
   MAEST's distance ranking. Disagreement clusters identify systematic
   geometry failures — e.g. same Discogs label but very different sound.
   This is essentially **using the LLM as a cheap human evaluator**, no
   labels needed.

2. **Triplet probe.** Sample (anchor, positive, negative) where positive
   is a top-k MAEST neighbour and negative is mid-rank. Ask the LLM
   *"Which of B or C is more similar to A?"* Compute the rate at which the
   LLM agrees with MAEST's ordering. Low agreement = MAEST geometry
   doesn't reflect perceptual similarity in that region. This is the
   exact methodology used to evaluate MuLan / CLAP in the academic
   literature, just with an LLM standing in for crowd workers.

3. **Caption-distance vs audio-distance correlation.** For all pairs in a
   sample, compute cosine in MAEST space and cosine in caption-embedding
   space. The Spearman correlation between the two is a global
   embedding-quality signal. Locally, pairs where the two distances
   diverge by more than k σ are candidates for manual review — either
   MAEST is over-clustering by label without semantic justification, or
   the caption missed something the audio captured. **Both directions are
   useful**: the first finds "false positives" in the suggestion graph,
   the second finds "false negatives" the LLM is blind to.

**Limits worth flagging:**
- LLM judgements are not ground truth. The Audio Flamingo 3 / Music
  Flamingo training distributions skew Western pop/rock. On underground
  techno/DnB the LLM's "similarity" notion may be coarser than MAEST's
  timbral resolution — i.e. MAEST is right and the LLM is the one
  hallucinating disagreement.
- Pairwise judgements are O(n²). Limit to top-k MAEST neighbours per
  seed; never run all-pairs.
- Costs: ~3–10 s per pair on 24 GB GPU. A 500-pair audit is 30 min — fine
  for periodic geometry checks, not for every track.

**Practical use:** add an offline "geometry audit" command to mesh-cue
(`mesh-cue audit-similarity --sample 500`) that runs the LLM-validator
sweep, dumps a CSV of disagreement cases, and ranks them by severity. Run
periodically against the user's library — especially after embedding-model
changes (Phase 2 / Phase 3 ship dates) — to spot regressions empirically
rather than from spot-checks alone. This is also the cleanest way to
*compare* MAEST vs MuQ-MuLan in production: which model's neighbours does
the LLM agree with more often, on the user's actual library?

This validator path does not require shipping the multimodal layer in Phase
4 — it can run as a one-off audit tool the moment captions exist in the
DB. So even if fusion / aggression-relabel turns out to be marginal, the
LLM still earns its place as an evaluation harness.

### Architecture options — the full design space

The audit/validator workflow above is **only an evaluation harness** — its
purpose is to confirm the LLM produces meaningful results on this library
before committing to an architecture. The end goal is a **fully automatic
system for the end user**: no per-track hand review, no manual fixture
curation in production, no human-in-the-loop at user import time. The
validator pass is a one-off developer-side sanity check.

Given that target, the design space is broader than just "caption-fusion vs
direct-LLM-comparison". Seven distinct options, evaluated against the
constraint *"runs once at import in mesh-cue, player consumes precomputed
vectors only"*:

#### Option 1 — Caption-embedding fusion

Audio LLM → free-text caption → sentence encoder (BGE-M3) → caption vector.
Fuse with the audio embedding via concat-PCA or multi-view k-NN. Aggression
refit on the fused space using LLM-derived scalars as labels.

- **Strength:** captions describe DJ-relevant qualities (production, vocal
  type, mix-friendliness) that the audio embedding can't see.
- **Weakness:** caption similarity reflects what the LLM *chose to write
  down*. Two sonically distinct tracks can land near each other if vocab
  overlaps; two similar tracks can drift apart if the LLM described them
  at different levels of detail. Vocabulary-shaped, not sound-shaped.
- **Player cost:** standard cosine over PCA vector. Free.
- **Per-track LLM calls:** 1 (caption) + 1 (tag pass).
- **GPU at user box:** required at import time only.

#### Option 2 — LLM-as-scorer comparing audio directly

Audio LLM ingests two audio clips, outputs similarity / aggression
directly. No caption layer, no sentence encoder. At import: pairwise
comparisons over the audio-embedding's top-k → store LLM verdicts as the
neighbour table. Aggression: LLM pairwise comparisons → reconstructed
global ordering → stored as scalar.

- **Strength:** LLM compares *sound to sound*, not vocabulary to
  vocabulary. No caption-vocabulary bias.
- **Weakness:** k LLM calls per track instead of 1 — much more expensive.
  ~30–60 GPU-min per 100 tracks at k=10.
- **Player cost:** free — reads precomputed neighbour tables and scalar.
  LLM expense is paid once at import; output is crystallised into the
  same artifact shape MAEST already produces.
- **Per-track LLM calls:** 0 unary, k pairwise.
- **GPU at user box:** required, more of it than Option 1.

#### Option 3 — LLM-as-labeller, embedding-as-scorer (hybrid) — *recommended candidate*

LLM produces structured tags + scalars at import; they're used as
**training labels** for tiny classifier/regression heads on top of the
audio embedding. Heads run at HNSW-rerank time on the audio vector only.

```
LLM (offline, import) → {aggression, dark, hypnotic, mix_friendly, ...}
                        ↓ used as labels to fit
                        a few small heads on the audio embedding
                        ↓
Player: reads audio vec, runs cheap heads, reranks
```

- **Strength:** combines LLM semantics with embedding calibration and
  speed. Heads are deterministic, fast, stable across LLM model swaps.
  This is the original EffNet probe-head pattern — just relabelled by an
  LLM instead of by MTG-Jamendo / NSynth.
- **Weakness:** quality capped at how well the audio embedding encodes the
  axis the LLM is labelling. If the embedding is blind to "vocal-presence",
  no head fit on it fixes that.
- **Player cost:** a few tiny matmuls per track. Negligible.
- **Per-track LLM calls:** 1 (tag pass).
- **GPU at user box:** required at import time only, briefly.

This is the strongest single option in our current view — LLM provides the
semantic axes the audio embedding is missing without ever shipping
caption-vocabulary geometry, and player runtime stays fully
embedding-based.

#### Option 4 — LLM as graph-edge weighter (no captions, no labels)

Don't store captions, don't fit heads. Use the LLM offline to score *edges
in the suggestion graph*: for each MAEST top-k pair, ask "are these
similar?" and store the verdict as the edge weight. The graph itself
becomes the artifact.

- **Strength:** simplest schema. Most of MAEST's wrongly-tight neighbours
  get pruned, most of its right neighbours kept. No caption-space
  hallucinations.
- **Weakness:** brittle to library additions (new tracks have no
  LLM-validated edges until reanalysis). Doesn't help with aggression.
- **Player cost:** edge lookup. Free.
- **Per-track LLM calls:** 0 unary, k pairwise.
- **GPU at user box:** required at import time, equivalent to Option 2.

#### Option 5 — LLM-driven re-ranker on top of audio cosine

Audio embedding does cheap recall (HNSW top-50). At import, an LLM
pairwise rerank reorders those top-50. Final stored neighbour list is
LLM-ordered, not cosine-ordered. Same shape as Option 2 but explicitly
framed as "embedding recalls, LLM reranks".

- **Strength:** industry-standard retrieval pattern (BM25 → cross-encoder
  rerank). Output matches the existing neighbour-table structure exactly.
- **Weakness:** same expense profile as Option 2; only fixes ordering,
  not aggression.
- **Player cost:** precomputed lookup. Free.

#### Option 6 — LLM-distilled lightweight student model

Run the LLM on a representative sample on the dev box. Distill its
judgements into a small student model (~10M params, audio embedding in
→ similarity score / aggression scalar out). Ship the student, not the
LLM. User imports run the student — no LLM ever runs on the user's
machine.

- **Strength:** the only option that doesn't require a strong GPU at user
  import time. Removes the StrongGpuProfile gate entirely.
- **Weakness:** student quality capped by sample diversity and
  distillation loss. Updating the LLM requires re-distillation on the dev
  box and a new Mesh release.
- **Player cost:** student is small enough to run on CPU at import time
  (or even player time if needed).
- **Per-track LLM calls at user box:** 0.
- **GPU at user box:** **not required.**

This is the only option fully aligned with "fully automatic for *every*
end user". It turns the LLM layer from "power-user feature behind a GPU
gate" into "default for everyone".

#### Option 7 — Joint audio-text embedding (MuQ-MuLan / CLAP)

Already in the plan as Phase 2. The model produces audio and text
embeddings *in the same space* — no separate sentence encoder, no LLM
captioning step. Text queries become a free byproduct. Sits orthogonal
to Options 1–6: MuQ-MuLan replaces MAEST as the audio embedding;
the LLM layer (1–6) sits on top of *whichever* audio embedding is in
use. Strongest combinations: **MuQ-MuLan + Option 3** for self-host
power users; **MuQ-MuLan + Option 6** for default-on automatic.

#### Comparison matrix

| Option | Caption vec stored | Per-track LLM calls (unary / pairwise) | Player runtime | Quality vs MAEST-alone | GPU required at user box |
|---|---|---|---|---|---|
| 1 — caption fusion | yes | 1 / 0 | cosine on PCA vec | better on semantic axes, vocab-biased | yes (import only) |
| 2 — LLM scorer | no | 0 / k | precomputed lookup | best on sound-similarity | yes, lots (import only) |
| **3 — LLM labeller + heads** | no | 1 / 0 | tiny heads | very good on labelled axes, fast | yes (import only, brief) |
| 4 — LLM edge weighter | no | 0 / k | edge lookup | better edges only, no aggression fix | yes, lots (import only) |
| 5 — LLM rerank | no | 0 / k | precomputed lookup | better ordering, same recall | yes, lots (import only) |
| **6 — LLM-distilled student** | no | 0 / 0 (at user box) | student inference | good if distillation holds | **no** |
| 7 — MuQ-MuLan joint embedding | n/a (text encoder is the LLM substitute) | 0 / 0 | cosine | best audio geometry + free text | no extra |

#### Current leaning

**Near-term, single-user GPU profile (ships fastest): Option 3** — LLM
labeller, heads on the audio embedding. Fixes the aggression placeholder,
adds the axes the audio embedding is blind to, keeps player runtime
trivial, no caption-vocabulary geometry to defend. Captions can still be
generated and stored as a side effect for explainability ("suggested
because: dark hypnotic groove, similar mix-friendly intro") — but the
*similarity and aggression numbers* come from heads, not captions.

**Long-term, true zero-GPU end user: Option 6 layered on Option 3.** Run
the LLM on a representative corpus on the dev machine, distill into a
student model, ship the student with Mesh. Any user — strong GPU or not —
gets the LLM-derived semantic axes for free. This is the only path that
turns the LLM layer from "power-user feature" into "default for
everyone".

**Least preferred as primary mechanism: Option 1** (caption fusion).
Vocabulary-shaped geometry is harder to reason about and harder to
evaluate than the alternatives. Captions remain useful as
*explainability* output, but probably shouldn't sit in the
similarity-distance critical path.

#### Decision is deferred until after Phase 2

Path forward locked in 2026-05-02:

1. Finish Phase 2 (MuQ-MuLan ONNX export spike, integration if it
   passes). This decides the **audio embedding** that everything else
   sits on top of.
2. Run the validator/audit harness against whichever embedding wins
   Phase 2. This produces the empirical signal — does the LLM actually
   produce meaningful judgements on the user's library?
3. *Then* pick between Options 1–6 (or a combination). The decision is
   evidence-driven, not architectural-from-principle. The audit numbers
   will tell us whether to invest in caption-space geometry, LLM-labelled
   heads, distillation, or some mix.

Until step 1 lands, no work happens on Options 1–6. They're all written
down here so the option space is preserved when the decision point
arrives.
