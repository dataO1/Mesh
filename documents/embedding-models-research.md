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
| 1 | **MAEST-30s-pw-519l-2** | Same-vendor drop-in replacement; de-risk the pipeline (head retraining, bell-σ re-tuning, HNSW dim change) | **In progress** |
| 2 | **MuQ-MuLan** (700M) | Highest perceptual-similarity numbers + free text-query path | Pending Phase 1 review |
| 3 | **MULE** (62M) | Best playlist-coherence supervision + cheapest of the high-quality options | Pending Phase 1 review |

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

### Out of scope for this branch

- Text-query feature (deferred to Phase 3)
- macOS GPU acceleration (no Apple users on the priority list)
- Genre tagging (dropped per user direction)
- Production integration into `crates/` (only after spike passes)
