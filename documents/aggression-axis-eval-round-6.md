# Aggression-axis evaluation, round 6 — single regression head on raw embeddings

**Status: PLAN (not yet executed). Companion to round 5.**

Round 5 hit the structural ceiling of hand-blended axes at ~63% pairwise
agreement against the BT priors. The variant Spearman dropped uniformly
from round 4 because the BT ground truth got more nuanced (within-cluster
gradient now visible) and no linear blend of the 6 named axes can follow
sub-cluster ordering. Round 6 is the first attempt to break that ceiling
by replacing the 6-axis projection with a learned non-linear head.

## TL;DR (planned)

Train a small MLP head: 768-dim MuQ-MuLan embedding → 1-dim intensity
score, supervised by the round-5 BT priors via pairwise margin loss
(RankNet / LambdaRank). Expected outcome: ~75–80% pairwise agreement
(vs ~63% for V6/V11). Loses interpretability — the head is opaque vs
the 6 named axes, which we keep alongside as separate sub-controls.

## Why this exists

- The 6 named axes (`aggression, distortion, density, darkness,
  noisiness, atonality`) are themselves a fixed handcrafted projection
  of the 768-dim MuQ-MuLan space. Variants V1–V13 explored linear
  blends *of those 6 axes*; we exhausted that search.
- A learned head consumes the **full 768-dim embedding** and emits a
  single intensity score, with ~100k learned weights instead of 6
  hand-tuned ones. Non-linear interactions ("high distortion AND high
  density together is much more intense than either alone") become
  representable.
- The supervisory signal (round-5 BT priors) is already produced. No
  new LLM-judge runs required for round 6.

## Inputs (already exist)

- **X**: 909 MuQ-MuLan embeddings, 768-dim each. Cached on disk in
  the project's existing embedding store. Need to dump to `/tmp/`
  for spike training.
- **y**: round-5 BT priors at
  `documents/axis-eval-results/llm-pair-priors-r5.txt` — 909 tracks,
  each with a 0-10 intensity score derived from 10,450 pair judgments.
- **Validation set**: 47 round-2 hand-anchors at `/tmp/anchors50.txt`
  (kept held out — never used for training).

## Architecture

```
MuQ-MuLan embedding (768 floats)
            │
            ▼
   ┌─────────────────┐
   │  Linear 768→128 │
   │  ReLU           │
   │  Dropout 0.2    │
   │  Linear 128→64  │
   │  ReLU           │
   │  Linear 64→1    │
   └────────┬────────┘
            ▼
       intensity ∈ ℝ   (then sigmoid+rescale → 0-10 at inference)
```

~100k parameters. Trains in seconds on 909 examples. Linear-probe
baseline (single Linear 768→1) trained as a sanity check.

## Loss

**Pairwise margin loss (RankNet)**: for every pair (i, j) where BT
prior delta |y_i − y_j| > 0.5, the head's predictions should rank them
in the same order, with margin proportional to the BT delta.

```
loss(i, j) = max(0, margin − sign(y_i − y_j) * (s_i − s_j))
```

Sum over all eligible pairs. Omits pairs too close to call (BT delta
< 0.5) since those are within ground-truth noise.

Optional upgrade: **LambdaRank weighting** — multiply each pairwise
loss by Δ-NDCG (the swap penalty if these two were swapped in the
ranked list). Bigger weight on pairs near the top of the ranking,
where ranking errors hurt most.

## Training procedure

1. **Dump 909 embeddings** to `/tmp/track-grading/embeddings.npz`
   via a small Rust binary (or reuse existing inference path).
2. **5-fold cross-validation** stratified by BT prior bucket. For
   each fold: train on 727 tracks, validate on 182.
3. **Optimizer**: AdamW, lr=1e-3, weight_decay=1e-4, batch size = full
   batch (909 fits trivially). 200 epochs.
4. **Early stopping** on validation pairwise agreement %.
5. **Final model**: retrain on full 909 tracks at the best epoch, save
   to `documents/axis-eval-results/intensity-head-r6.pt`.

## Validation

Three metrics, each computed on the held-out fold and aggregated:

1. **Pairwise agreement % vs round-5 BT priors** (the headline metric).
   Compare to V6 round-5 baseline of 63.5%.
2. **Spearman ρ vs 47 round-2 hand-anchors** (held out from training).
   Compare to V6 round-5 baseline of +0.39.
3. **Band-hit rate** (right percentile bucket). Compare to V6 round-5
   baseline of 427/909.

If any of these fails to clearly beat the V6 baseline, the head training
didn't help — investigate before round 7.

## Expected results (working theory)

| Metric | V6 (round 5) | Head (round 6 expected) |
|---|---|---|
| Pairwise agreement vs BT | 63.5% | 75-80% |
| Spearman vs hand-anchors | +0.39 | +0.50-0.55 |
| Band-hit rate | 427/909 | 550-600/909 |

If actual results are within 2pp of V6, the head isn't extracting
anything new from the embedding — the 6 axes already capture
everything. If results are dramatically better, that's evidence the
embedding has intensity signal V11 was throwing away.

## Risks / unknowns

- **Overfitting on 909 tracks.** 100k parameters / 909 examples = 110
  params per example. Heavy regularisation (dropout, weight decay,
  early stopping) is essential. May need to start with linear probe
  (single Linear) and grow only if validation supports it.
- **BT priors as labels carry their own bias.** Particularly the
  Switch Disco "loud bass" over-rating (round-4/5 disagreement with
  hand-priors). The head will learn that bias. Whether that's the
  right call depends on which interpretation of "intensity" we want.
- **The 6 named axes lose informational role.** They stay available
  for per-axis sub-controls but the headline intensity number stops
  being a transparent blend.

## Implementation effort

- Embedding dump: 1 hour (small Rust binary or re-use inference path)
- Training script: 2 hours (PyTorch, ~150 lines)
- Validation harness: 1 hour (extend existing pairwise-agreement scripts)
- Report writing: 1 hour
- **Total: ~half day end-to-end. Trivial GPU time (<1 min training).**

## Files / artifacts (planned)

- `spike/track-grading/dump_embeddings.py` (or `.rs`) — dump all 909
  MuQ-MuLan embeddings to `.npz`
- `spike/track-grading/train_head_r6.py` — training script with CV
- `spike/track-grading/eval_head_r6.py` — evaluation harness
- `documents/axis-eval-results/intensity-head-r6.pt` — trained model
- `documents/axis-eval-results/V14_head_r6.csv` — predictions per track,
  in the same format as V*.csv so `compare-variants.py` works
- `documents/aggression-axis-eval-round-6.md` — final results report
  (replacing this plan section)

## Follow-on: how round 6 feeds round 7

- Round 6 validates the **infrastructure** (embedding dump, pairwise
  loss, validation harness) before round 7 invests in the bigger
  per-axis LLM tournaments.
- If round 6 hits the expected ~78% pairwise agreement, that's the
  ceiling of the *intensity-only* signal. Round 7 (axis discovery)
  could push higher by surfacing additional dimensions.
- If round 6 disappoints, that informs round 7 — maybe the embedding
  is just not as rich as we hoped, and axis discovery won't help
  either.

## Cross-references

- [Round 5](aggression-axis-eval-round-5.md) — sampling design that
  produced the BT priors used as supervision here
- [Round 7 plan](aggression-axis-eval-round-7.md) — axis discovery,
  the more ambitious successor
