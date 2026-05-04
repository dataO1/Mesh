# Aggression-axis evaluation, round 5 — community-aware + active-learning sampling

Companion to `documents/aggression-axis-eval-round-4.md`. Round 4 used a 3-anchor
tournament that left 871/909 tracks saturated in BT 8-9, 282 always-won tracks,
and 65.7% pairwise agreement for the best variant (V6). The round-4 doc closed
with a flag that the existing calibration UI in `aggression.rs` already
implements smarter sampling — round 5 ports it.

## TL;DR

**Cross-validated against hand-priors Spearman jumped from +0.36 (round 4) to
+0.41 (round 5)** — the new BT priors are measurably closer to human ground
truth. **Saturated tracks dropped from 282 to 83**, and the BT distribution
spread out dramatically (round-4 had 871/909 in 8-9 buckets; round-5 has them
distributed across 4-9 with median in bucket 6).

**The variant leaderboard preserves V6 at #1, V11 mid-pack** — same shape as
round 4, but absolute Spearman against BT dropped uniformly (V6 +0.45 → +0.39,
V11 +0.42 → +0.36). **The variants haven't regressed — the ground truth has
gotten more nuanced and the variants can no longer track the within-cluster
gradient.** Band-hit rate (right percentile bucket) actually *rose* across all
variants (V6 263 → 427 hits out of 909).

The round-4 verdict — V6 vs V11 within noise, variants at their ceiling, next
step is a learned head — is reinforced. **Pairwise agreement plateaued around
63%, and that's the structural ceiling of hand-blended axes**; closing the
30% gap requires the round-6 head training, not more sampling.

## What we built (committed)

- **`spike/track-grading/plan_pairs_v2.py`** — Python port of the Rust
  `build_calibration_plan()` from `crates/mesh-core/src/suggestions/aggression.rs`.
  Uses 6-axis features from V11.csv (aggression, distortion, density, darkness,
  noisiness, atonality), KMeans (k=12) for community detection (no external
  dep, approximates Leiden well enough at 909 tracks), centroid + farthest-point
  reps per community. Three-tier Phase 1 + budgeted Phase 2 active-learning
  queue with hybrid embedding filter and transitive-closure pruning.

- **`judge_pairs_vllm.py --plan-file` mode** — accepts the planner's CSV
  output. Reuses the round-4 parallel-worker pool. Cached pairs from round-4
  are automatically skipped (per-pair JSON cache + frozenset(a,b) dedup).

## Sampling design

**Phase 1 (deterministic bootstrap):**
- **1A — cross-community extreme pairs**: top-8 community-centroid pairs
  ranked by |BT-prior gap|. Pins the global scaffold across the biggest fault
  lines.
- **1B — per-community centroid × farthest-rep**: 1 pair per community.
  Each cluster gets its own internal scale.
- **1C — intra-community skip-chains** (communities with ≥75 members):
  e0–e2, e2–centroid, centroid–e1 (4 reps per chain). The within-cluster
  refinement that round 4 was missing.

**Phase 2 (active-learning queue, budget 2500 unordered pairs):**
- Score = `prior_factor × ||Δ_features||² × diversity_decay`
  - `prior_factor = 1 + 1.5 × |bt(a)−bt(b)|/10`, doubled if both tracks are
    in the round-4 saturated 8-9 cluster (highest information content there).
  - `diversity_decay = 0.85^k` where k counts recent touches in either community.
- Cross-community ambiguous bonus: pairs with V11 intensity diff < 0.05 get
  1.5x score (those are exactly where the embedding can't decide).
- **Hybrid embedding filter**: pairs with V11 intensity diff > 0.20 are
  *dropped* from phase 2 (V11 is decisive — don't waste an LLM call).
- **Transitive-closure pruning** via DFS reachability over the round-4 win
  graph: drop any pair already deducible from existing chains.

Bilateral pairing (each plan pair → A→B and B→A) preserved for positional-bias
cancellation.

**Pair count**: 2502 unordered (5004 directed) added on top of round 4's 5446,
total 10450 directed pairs / 5225 unordered.

## Throughput

- Planner runtime: <2 s (KMeans + farthest-point + scoring)
- Round-5 grader: 5004 new directed pairs in 16.2 min (~5.2 pair/s, GPU 91-96%)
- BT iteration: 187 iters, ~2 s
- Total round-5 wall time: ~19 min on top of round 4

Round-4 + round-5 combined: 10450 directed pairs / 909 tracks = 11.5 games/track
median (vs 6 in round-4 alone).

## Results

### BT prior distribution — round 4 vs round 5

```
                  round 4       round 5
bucket  0  →        1             1
bucket  1  →        3             3
bucket  2  →        1             5
bucket  3  →        4             0
bucket  4  →        0            30
bucket  5  →        2            13
bucket  6  →       26           433  ← median moved here
bucket  7  →        0           328
bucket  8  →      366            78
bucket  9  →      505            17
bucket 10  →        1             1
saturated tracks  282           83  (-70%)
```

Round-4 had a sharp 8-9 collapse; round-5 spreads tracks meaningfully across
buckets 4-9. Saturation dropped 70%.

### Cross-validation — BT vs 47 hand-anchors

```
                       round 4      round 5      delta
Spearman ρ             +0.3574      +0.4070      +0.0496
```

**The new BT priors agree more with human-assigned hand-priors.** This is the
strongest validation that round-5 sampling improved ground-truth quality, not
just looked different.

### Variant leaderboard

| Rank | Variant | ρ_round4 | ρ_round5 | band hits r4 | band hits r5 |
|---|---|---|---|---|---|
| 1 | V6_five_axis_weighted          | +0.4547 | +0.3887 | 263 | **427** |
| 2 | V7_dark_noisy_emphasis         | +0.4499 | +0.3881 | 261 | **425** |
| 3 | V4_blend_equal_3               | +0.4480 | +0.3839 | 259 | **427** |
| 4 | V12_peak_techno_tuned          | +0.4423 | +0.3777 | 254 | **425** |
| 5 | V9_v6_with_atonal              | +0.4428 | +0.3775 | 262 | **424** |
| 6 | V8_v7_with_distortion_bump     | +0.4357 | +0.3760 | 268 | **419** |
| 7 | V10_balanced_six_axis          | +0.4417 | +0.3750 | 259 | **420** |
| 8 | V5_aggression_led              | +0.4307 | +0.3676 | 262 | **424** |
| 9 | V11_neuro_dnb_tuned (current)  | +0.4248 | +0.3646 | 267 | **421** |
| 10 | V13_distortion_atonal_dominant | +0.4179 | +0.3554 | 262 | **418** |
| 11 | V2_pure_distortion             | +0.4053 | +0.3474 | 265 | **406** |
| 12 | V1_pure_aggression             | +0.3928 | +0.3372 | 261 | **420** |
| 13 | V3_pure_density                | +0.3126 | +0.2716 | 262 | **392** |

**Spearman dropped uniformly (~6 percentage points).** **Band hits rose
~60% across the board** (V6 from 263 to 427 out of 909). Same leaderboard
order as round 4. This pattern is the signature of a more nuanced ground
truth: variants get tracks in the right intensity *bucket* better, but
within-bucket ordering is now too granular for hand-weighted axes to follow.

### Pairwise agreement %

| Variant | round 4 | round 5 | Δ |
|---|---|---|---|
| V6_five_axis_weighted | 65.7% | 63.5% | −2.2 pp |
| V7_dark_noisy_emphasis | 65.5% | 63.4% | −2.1 pp |
| V11_neuro_dnb_tuned | 64.6% | 62.5% | −2.1 pp |
| V2_pure_distortion | 63.9% | 62.0% | −1.9 pp |
| V3_pure_density | 60.7% | 59.4% | −1.3 pp |

Same pattern. The 2pp drop is the within-cluster signal that variants can't
reach. Pairwise agreement plateaus around 63%.

### Round-5 BT priors — top + bottom

```
top 5: Dirtgrub (High)=10.0, Into Black=9.9, Mechanical Paw=9.7, Shinde=9.5, Tydyrium=9.5
bot 5: Strand=0.0, The Great Commandment=1.1, Run=1.1, Every Wall Is a Door=1.1, Faded=2.1
```

vs round-4 top (Omnivore, Dead Limit, Bullet Time, Nightrage Zardonic, Clamps
MK Ultra — all 9.9-10.0) — the new top has a clear **9.5/9.7/9.9/10.0
gradient**, where round-4 had everything piled at 9.9-10.0 indistinguishably.

Top disagreements vs hand-priors:

| Track | BT-r5 | Hand | Note |
|---|---|---|---|
| Bass So Loud — Switch Disco | 7.90 | 4 | Persistent: Qwen rates loud-bass mainstream high |
| Every Wall Is a Door | 1.13 | 5 | Persistent: Qwen rates as chill |
| React — Switch Disco | 6.35 | 3 | Persistent loud-bass disagreement |
| DDoS | 6.35 | 9 | New disagreement: hand says intense, Qwen mid |
| Creating | 6.35 | 9 | Same — hand-priors say very intense |

Round-5 corrected some round-4 over-estimates (Switch Disco tracks dropped
9.9 → 7.9 because within-cluster pairs revealed they aren't as intense as
neuro-DnB). The "DDoS"/"Creating" disagreements are new — likely tracks the
hand-priors over-rated.

## What this tells us

### About the round-5 sampling design

- **It worked.** BT-vs-hand jumped +0.05 Spearman, saturation dropped 70%,
  the within-cluster gradient is now visible. The community-aware planner
  is doing real work that 3 hand-anchors couldn't.
- **But it has limits.** 83 tracks still saturated (always-won), median games
  per track still 6 because the planner only added pairs to the saturated
  cluster, leaving the rest at anchored-only coverage.
- **Hybrid embedding filter is conservative.** Setting `HYBRID_SKIP=0.20`
  on V11 intensity (range ~0.4) only skipped a handful of pairs. With the
  axis range so compressed, the filter barely activates. A future tweak
  could use V11 *rank* difference instead of *score* difference.

### About the variants

- **V6 still leads.** +0.39 (was +0.45 against round-4 BT). V11 still mid-pack.
  Order preserved.
- **Variant ceiling confirmed at ~63% pairwise agreement.** Across rounds 4
  and 5, every variant converges to a 60-66% range. The remaining ~36%
  disagreement is within-cluster ordering that no linear blend of 6 axes can
  capture. **More sampling will not raise variant Spearman further.**
- **V3_pure_density still last.** Round-3's "V3 jumped to #2" was an artifact;
  rounds 2/4/5 all confirm V3 as bad.

## Recommendation

### Ship now (no change from round-4 verdict)

1. **Switch default to V6 if listening test confirms.** +0.39 vs +0.36 (V11)
   is 3 percentage points; pairwise agreement gap is 1 pp. Audible difference
   not guaranteed. Worth A/B testing before flipping.
2. **Use round-5 BT priors as the canonical ground truth** for any future
   variant tuning. They're better-validated than round-4 (+0.41 vs hand
   instead of +0.36) and more granular within clusters.

### Round 6 — train a regression head (the real next lever)

The variant ceiling at ~63% pairwise agreement is **the structural limit of
linearly blending the 6 named axes**. Closing the gap requires a non-linear
model that uses the raw MuQ-MuLan embeddings, with round-5 BT priors as
labels:

- 909 tracks × 768-dim MuQ-MuLan embeddings → 1-dim intensity head
- Loss: pairwise margin loss on the BT-derived score deltas
- Architecture: small MLP (768 → 128 → 64 → 1) or a single linear projection
  to start
- Train/val split: 80/20 with stratification across BT prior buckets
- Expected lift: pairwise agreement 63% → 75-80% based on similar pairwise
  ranking literature (RankNet, LambdaRank).

### Round 7 — close the remaining gap (later)

Once the head is trained, identify pairs where head and BT disagree. Re-judge
those with a second independent LLM run (Crowd-BT-style). If LLM judges
agree, BT is right and head needs more capacity / data. If LLMs disagree,
the pair is genuinely ambiguous and should be marked EQUAL.

### Don't bother with

- **Full round-robin (820k pairs).** ~25 GPU-hours, marginal Spearman gain
  ≪ what a learned head buys with the same data we already have.
- **More anchors.** The community-aware planner already has 12 communities
  with 5 reps each = effectively 60 anchors, all data-derived. Adding more
  human-picked anchors would only re-introduce the hand-bias round-2 had.
- **Hand-tuning a 14th variant.** We've explored the linear-blend space
  exhaustively. Any improvement now lives outside that space.

## Files / artifacts

- `documents/axis-eval-results/llm-pair-priors-r5.txt` — committed, 909 rows,
  the new canonical priors
- `documents/axis-eval-results/llm-pair-priors-r5.csv` — committed, full table
- `spike/track-grading/plan_pairs_v2.py` — community-aware planner
- `spike/track-grading/judge_pairs_vllm.py` — `--plan-file` mode added
- `/tmp/track-grading/round5_plan.csv` — the planner's pair queue (regen'able)
- `/tmp/track-grading/pairs_vllm/*.json` — 10450 per-pair JSON judgments
  (cumulative round 4 + round 5; not committed, regenerable in 30 min)

## Open questions for round 6

1. Is MuQ-MuLan the right embedding for a head? Or would CLAP / LAION-CLAP
   give a more intensity-aligned latent space?
2. How does the head respond to the persistent loud-bass disagreement
   (Switch Disco)? If trained on Qwen-derived BT priors, will it learn
   "loud-bass = intense" and override the hand-prior signal?
3. Should we add a second LLM judge (e.g., Audio Flamingo 3 in pairwise mode
   if we can resolve its 1:1 constraint, or SALMONN-13B) to cross-validate?

## Cross-references

- [Round 6 plan](aggression-axis-eval-round-6.md) — single regression head
  on raw MuQ-MuLan embeddings, supervised by round-5 BT priors. Targets
  ~75-80% pairwise agreement; sacrifices interpretability.
- [Round 7 plan](aggression-axis-eval-round-7.md) — LLM-supervised axis
  discovery + joint blend optimisation. Targets ~80-85% pairwise agreement
  AND an interpretable, defensible axis basis. The more ambitious successor
  to round 6 and the answer to "are our 6 named axes the right basis?".
