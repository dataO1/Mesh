# Aggression-axis evaluation, round 6 — single regression head on raw embeddings

**Status: COMPLETED. Companion to round 5.**

Round 5 hit the structural ceiling of hand-blended axes at ~63% pairwise
agreement against the BT priors. Round 6 trains a learned head directly on
the 512-dim MuQ-MuLan embedding (correcting an earlier note that said
768-dim — MuQ-MuLan emits 512). Two models trained: a single 512→1 linear
probe and a 512→128→64→1 MLP. Both are supervised by round-5 BT priors via
RankNet pairwise margin loss.

## TL;DR

**The MLP head beat the V6 ceiling by +10.7 percentage points pairwise
agreement (74.2% CV vs V6 round-5 63.5%) and +0.30 Spearman (CV +0.67 vs
V6 +0.39).** Validated independently against the held-out 47 hand-anchors:
V14 MLP head Spearman +0.50 vs V6 +0.36 (+0.14 lift over the best
hand-blended variant). The single linear probe alone hit 71.1% pairwise
agreement / +0.60 Spearman — meaning the MuQ-MuLan embedding has
substantial intensity signal that the V11 polar axes were throwing away.

The round-7 axis-discovery direction is justified: there's clearly more in
the embedding than the 6 hand-named axes capture. Round 6 also de-risks
the round-7 pipeline (label loading, pair-margin loss, validation harness
all work cleanly).

## What we built (committed)

- **`spike/track-grading/dump_embeddings.py`** — dumps the 512-dim
  MuQ-MuLan embeddings for the 909 round-5 tracks from the mesh CozoDB
  to `/home/data01/Music/mesh-track-grading/embeddings.npz`. Uses the `pycozo` Python SDK.
- **`spike/track-grading/train_head_r6.py`** — trains both the linear
  probe and the MLP via RankNet pairwise margin loss. 5-fold CV
  stratified by BT-prior bucket. Pure CPU — training is <30 sec total
  for both models including CV. Outputs predictions in V*.csv format
  for `compare-variants.py`.

## Architecture

```
MuQ-MuLan embedding (512 floats)
            │
            ├──── Linear 512→1                  (linear probe baseline)
            │     [V15_linear_probe_r6.csv]
            │
            └──── Linear 512→128 → ReLU → Dropout 0.2
                  → Linear 128→64 → ReLU
                  → Linear 64→1                  (MLP head, V14_mlp_head_r6.csv)
```

MLP: ~75k parameters total. Linear probe: 513 parameters. Both trained
with AdamW (lr=1e-3, weight_decay=1e-4), 400 epochs, full-batch.

## Loss

Pairwise margin (RankNet-style):
```
loss(i, j) = max(0, |y_i - y_j| - sign(y_i - y_j) * (s_i - s_j))
```
for all pairs (i, j) with `|y_i - y_j| > 0.5` (skips noise-level
ground-truth pairs). Margin is proportional to the BT delta, so the
model is penalized more for swapping high-confidence pairs.

## Throughput

- Embedding dump: 2 seconds (909 × 512 = 1.8 MB)
- Training (full CV + retrain, both models): 25 seconds on a laptop CPU
- Total round-6 wall time: ~30 seconds

No GPU, no LLM calls. Reuses round-5's already-cached BT priors as labels.

## Results

### Headline metrics — pairwise agreement (the most readable number)

| Variant | Pairwise agreement | Δ vs V6 |
|---|---|---|
| **V14 MLP head (r6, CV)** | **74.2%** | **+10.7 pp** |
| V15 Linear probe (r6, CV) | 71.1% | +7.6 pp |
| V6 five_axis_weighted (round 5) | 63.5% | baseline |
| V11 neuro_dnb_tuned (current default) | 62.5% | −1.0 pp |
| V3 pure_density (worst variant) | 59.4% | −4.1 pp |

**74.2% pairwise agreement means: shown two random tracks, the MLP head
ranks them in the same order as the LLM judge ~3 times out of 4.** vs V6's
2 in 3. Closing 11 of the 36 percentage points that V6 left on the table.

### Cross-validation Spearman (5-fold, held out within training set)

| Model | CV pa | CV ρ | In-sample ρ |
|---|---|---|---|
| MLP head | 74.2% | +0.6710 | +0.7925 |
| Linear probe | 71.1% | +0.6002 | +0.6037 |

The MLP shows ~0.12 in-sample/CV gap → mild overfitting (expected at
75k params / 727 train examples). Linear probe has near-zero gap
(513 params / 727 examples) — well under-parameterised. The fact that
even the linear probe at 71.1% massively beats V6's 63.5% says the
MuQ-MuLan embedding is doing most of the work; the MLP only adds the
non-linear interactions.

### Spearman vs 47 hand-anchors (truly held-out, never seen during training)

| Variant | ρ vs hand |
|---|---|
| **V14 MLP head (r6)** | **+0.4997** |
| V15 Linear probe (r6) | +0.4294 |
| V11 neuro_dnb_tuned | +0.3703 |
| V6 five_axis_weighted | +0.3640 |
| V7 dark_noisy_emphasis | +0.3624 |
| V3 pure_density | −0.0265 |

The hand-anchors were never used during round-6 training. **MLP head
beats V6 by +0.14 Spearman against an external test set** — the gain is
not just a fit-to-BT-priors artifact; it transfers to human ground truth.

### Full leaderboard (Spearman vs round-5 BT priors)

```
rank  variant                        spearman  band hits
   1  V14_mlp_head_r6               +0.7925    566/909   ← in-sample, overfit
   2  V15_linear_probe_r6           +0.6037    495/909   ← in-sample, well-calibrated
   3  V6_five_axis_weighted         +0.3887    427/909   ← V6 ceiling
   4  V7_dark_noisy_emphasis        +0.3881    425/909
   5  V4_blend_equal_3              +0.3839    427/909
   6  V12_peak_techno_tuned         +0.3777    425/909
   7  V9_v6_with_atonal             +0.3775    424/909
   8  V8_v7_with_distortion_bump    +0.3760    419/909
   9  V10_balanced_six_axis         +0.3750    420/909
  10  V5_aggression_led             +0.3676    424/909
  11  V11_neuro_dnb_tuned (default) +0.3646    421/909
  12  V13_distortion_atonal_dominant +0.3554   418/909
  13  V2_pure_distortion            +0.3474    406/909
  14  V1_pure_aggression            +0.3372    420/909
  15  V3_pure_density               +0.2716    392/909
```

V14's in-sample Spearman is meaningfully overfit; the CV value (+0.67)
is the honest generalisation estimate.

## What this tells us

### About the MuQ-MuLan embedding

- **The 6 hand-named axes (`aggression, distortion, density, darkness,
  noisiness, atonality`) capture only a fraction of the embedding's
  intensity-relevant signal.** A single linear projection (513 params)
  learned directly from the embedding extracts ~+7.6 pp more pairwise
  agreement than the best hand-blended variant. The 6-axis projection
  was throwing away usable signal.
- **Non-linear interactions matter, but only modestly.** MLP beats
  linear by ~+3.1 pp pairwise agreement. The bulk of the lift is from
  using more dimensions linearly, not from non-linearity.
- **Ceiling is now ~75% pairwise agreement** with intensity-only signal
  from MuQ-MuLan. The remaining 25% is some mix of (a) intensity signal
  the embedding doesn't carry, (b) BT-prior label noise, (c) genuinely
  ambiguous tracks where a single 1-d intensity score is the wrong
  representation (e.g., "mid-tempo with high distortion" might be
  "intense" or "chill" depending on listener context).

### About the variants

- **V6/V7/V11 are confirmed at their ceiling of ~63%.** No amount of
  hand-tuning the 6-axis blend can get past this — the limit is
  structural (only 6 dimensions used, hand-picked, linear blend).
- **V3 is decisively bad** across all rounds, all metrics. Confirmed.
- **The whole 13-variant exercise (rounds 1-5) was useful as data
  validation but has now been superseded.** V14 (MLP) should be the
  new default if a single intensity score is what we want.

### About round 7

Round 7's premise — that learned axes can outperform hand-named axes —
is now empirically supported. Whether round 7 (multi-task per-axis
discovery) beats round 6 (single intensity head) by the additional
~5-10 pp the round-7 plan estimates is the open question. The pipeline
infrastructure (embedding dump, pair-margin loss, validation harness)
is now battle-tested and ready for that work.

## Recommendation

**Ship V14 (MLP head) as the new default for the intensity ranking.**
Either:
- **Replace V11 entirely** with the MLP head — biggest improvement,
  clean model. Lose interpretability of the 6 axes for the headline
  intensity number (but keep them as separate sub-controls — the per-
  axis projection still works for "make this set less distorted"-type
  controls).
- **Or hybrid**: keep V11 for the 6-axis sub-controls, add V14 as the
  default `intensity` column. Two parallel projections of the same
  embedding, ~75 KB extra storage per shipped model.

Production checklist before shipping:
1. Persist the trained MLP weights (`.pt` or `.safetensors`) to
   `models/intensity-head/r6_mlp.pt` once the model artifact format
   is decided.
2. Add an inference path in the Rust ML analysis crate
   (`crates/mesh-cue/src/ml_analysis/`) — torch via `tch-rs` or ONNX
   via `ort`. Both work on CPU at <1ms per track for a 512→128→64→1 MLP.
3. Migration: re-rank existing libraries on first launch after upgrade.
   No re-embedding needed — the 512-dim vectors stay, only the
   projection changes.

## Risks / known gaps

- **Overfitting on 909 examples.** CV/in-sample gap is +0.12 Spearman
  for the MLP. Heavy regularisation (dropout 0.2, weight decay 1e-4,
  early-stopping eligible) but more training data would help. The
  round-7/8 multi-genre corpus directly addresses this.
- **BT-prior bias propagates.** Switch Disco "Bass So Loud" was
  rated 7.9 by Qwen vs 4 by hand-priors; the head will learn that
  bias. Whether that's the right intensity definition depends on
  use case. Round 8 (productisation) will need a calibration UI to
  let users tune away from biases that don't match their preference.
- **Interpretability lost** in V14 vs V11. The 6-axis sub-controls
  remain available (driven by V11) but the headline intensity number
  becomes opaque. Round 7 (axis discovery) reclaims interpretability
  if we want to ship learned-but-named axes.

## Files / artifacts

- `documents/axis-eval-results/V14_mlp_head_r6.csv` — committed,
  per-track MLP intensity predictions in V*.csv format
- `documents/axis-eval-results/V15_linear_probe_r6.csv` — committed,
  linear probe baseline
- `spike/track-grading/dump_embeddings.py` — embedding dumper
- `spike/track-grading/train_head_r6.py` — training + CV harness
- `/home/data01/Music/mesh-track-grading/embeddings.npz` — 909 × 512 float32 (regen'able
  in 2 sec from the mesh DB)
- `/home/data01/Music/mesh-track-grading/round6_metrics.json` — full CV scores

## Cross-references

- [Round 5](aggression-axis-eval-round-5.md) — produced the BT priors
  used as supervision here
- [Round 7 plan](aggression-axis-eval-round-7.md) — the next direction:
  multi-task axis discovery for interpretability + cross-library
  generalisation
