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
