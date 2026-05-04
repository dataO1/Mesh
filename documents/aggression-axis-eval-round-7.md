# Aggression-axis evaluation, round 7 — LLM-supervised axis discovery + joint blend

**Status: PLAN (not yet executed). Companion to rounds 5 and 6.**

Round 6 trains a single opaque head from MuQ-MuLan embeddings to
intensity. Round 7 is the more ambitious successor: instead of
collapsing the 768-dim space to one number, learn `k` interpretable
axes (each a linear direction in embedding space) jointly with the
final blend weights into intensity. This answers the deeper question
*"are our 6 named axes the right basis?"* empirically.

## TL;DR (planned)

Run k separate per-axis pairwise LLM tournaments ("which is more
distorted?", "which is darker?", ...) → multi-task linear probes
discover k linear directions in the 768-dim MuQ-MuLan space → joint
end-to-end training with ListMLE loss against round-5 intensity
ranking learns the per-axis blend weights. Each learned axis is
interpretable (we can read off top-scoring tracks per axis and confirm
or rename it). Per-library deployment becomes a few-shot blend-weight
fine-tune.

Expected: ~80-85% pairwise agreement (vs ~63% V6, ~78% round-6 head)
**AND** an interpretable axis basis we can defend against the current
hand-named one **AND** a clear cross-library deployment story.

## Why this exists

The user's question that triggered this: *"i'm not even sure these
axes are correct. with the current LLM as judge is it possible to
find the optimal measurable combination of axes and their weightings?"*

Yes. The current 6 axes (`aggression, distortion, density, darkness,
noisiness, atonality`) were named first then projected via the
text-tower CLAP-style pipeline. They've never been validated as the
*optimal* basis for intensity ranking. With the LLM judge, we can:

1. Ask the LLM per-axis pairwise questions to derive supervised labels
   for any candidate axis.
2. Learn linear directions in embedding space from those labels.
3. Compare learned vs hand-named axes — confirm which named axes the
   data supports, identify dimensions we missed, drop dimensions that
   collapse together.
4. Jointly optimise the blend weights so the final intensity matches
   the round-5 BT ranking.

## Inputs (some exist, some need generating)

**Already exist:**
- 909 MuQ-MuLan embeddings (768-dim) — same dump as round 6
- 909 round-5 BT priors (intensity rankings)
- Existing 6-axis projections from V11.csv (for comparison/baseline)

**Need to generate:**
- Per-axis pairwise judgments from Qwen3-Omni. For each candidate axis
  ask "which clip is more {axis_name}?" via the same vLLM pipeline as
  round 5. ~500-1000 pairs per axis × k axes = 5-15 GPU-hours total.

## Candidate axes to probe

Start with the existing 6 (test if they hold up empirically):
- aggression, distortion, density, darkness, noisiness, atonality

Add 4-6 new candidates the existing axes might miss:
- **bass weight** (sub-bass energy / kick presence)
- **tempo intensity** (faster-feeling, regardless of BPM)
- **rhythmic complexity** (breakbeat / polyrhythm vs four-on-the-floor)
- **vocal intensity** (screaming / aggressive vocal vs clean / no vocal)
- **dynamic compression** (loud-throughout vs quiet-loud-quiet)
- **harmonic dissonance** (chord-level vs the existing atonality
  which is more about pitched-vs-unpitched)

= 12 candidate axes. Per-axis tournament: 500 directed pairs (anchored
or community-sampled) → ~12 min × 12 axes = ~2.5 GPU-hours.

The LLM tournaments per axis should reuse the round-5 community-aware
planner — same `plan_pairs_v2.py` infrastructure, just swap the prompt.

## Architecture

```
                MuQ-MuLan embedding (768 floats)
                            │
              ┌─────────────┼─────────────┐
              │             │             │
              ▼             ▼             ▼
        ┌─────────┐   ┌─────────┐   ┌─────────┐
        │ axis_1  │   │ axis_2  │ ..│ axis_k  │   k learned linear probes
        │ Linear  │   │ Linear  │   │ Linear  │   (each: 768 → 1)
        │ 768→1   │   │ 768→1   │   │ 768→1   │
        └────┬────┘   └────┬────┘   └────┬────┘
             │             │             │
             ▼             ▼             ▼
         (per-axis scores, 0-1)
             │             │             │
             └──────┬──────┴──────┬──────┘
                    ▼             ▼
              ┌──────────────────────┐
              │  blend = w₁·a₁ +     │   k learned blend weights
              │          w₂·a₂ + ... │
              └──────────┬───────────┘
                         ▼
                  intensity ∈ ℝ
```

Total parameters: k × 768 + k = ~10,000 for k=12. Far smaller than
round 6's 100k MLP. Each axis is inherently interpretable because
it's a linear function of the embedding — to interpret it, project all
909 tracks onto that direction and inspect the top + bottom 20.

## Training procedure

**Phase 1 — per-axis probing (parallel)**:
- For each candidate axis i, fit a linear probe (Linear 768→1) on the
  per-axis pairwise judgments using RankNet-style margin loss.
- Output: k linear directions w_i ∈ ℝ^768.

**Phase 2 — joint blend**:
- Freeze the k axis directions, learn the blend weights v ∈ ℝ^k via
  ListMLE loss against the round-5 BT priors.
- Or jointly fine-tune both (less stable but potentially higher
  ceiling).

**Phase 3 — axis interpretation**:
- For each learned axis: list top 20 + bottom 20 tracks. Assign a
  human-readable name (or confirm the original name).
- Compute correlation between each learned axis and the existing
  named axes. Drop axes that strongly correlate (>0.85) with another
  — they're not adding information.
- Final k' ≤ k axes after deduplication.

**Phase 4 — recompose**:
- Final model = k' axes + blend → intensity.
- Generate `V15_axis_discovery.csv` for `compare-variants.py`.

## Validation

- **Pairwise agreement** vs round-5 BT priors. Target: ≥80%.
- **Spearman vs 47 hand-anchors** (held out from all training).
  Target: ≥+0.50.
- **Per-axis interpretability check**: for each learned axis, do top
  tracks share an obvious property? If not, the axis has no
  human-meaningful semantic — flag it.
- **Cross-library transfer test**: take a small (~50-100 track)
  external library snippet (e.g., a folk or jazz playlist), run the
  trained model, ask Qwen for ~50 pairwise intensity judgments,
  compute pairwise agreement. Compare to V11 zero-shot performance
  on the same external set.

## Cross-library deployment story

This is round 7's biggest advantage over round 6:

- The k learned axes (distortion, density, etc.) are **general music
  concepts**. Once trained on Mesh's DnB library, they remain
  meaningful for jazz/folk/orchestral because the underlying audio
  features (distortion, density, etc.) are universal.
- **Per-library, only the k blend weights need updating.** New
  library? Run a 100-pair LLM tournament on it (~3 min), refit
  v ∈ ℝ^k (k=12, ~12 parameters), done.
- This sidesteps the round-6 problem where the entire 100k-parameter
  head would need to be retrained on every new library, requiring a
  much bigger label set per library.

The trade-off vs round 6:
- Higher upfront cost (12 LLM tournaments instead of 1).
- Higher pairwise agreement ceiling because the model is forced to
  decompose intensity into orthogonal-ish dimensions.
- Vastly cheaper per-library adaptation (~12 weights vs ~100k).

## Risks / unknowns

- **Per-axis prompts may have their own biases.** "Which is more
  distorted?" might trigger different responses than "which has more
  distortion?". Need to A/B several prompt phrasings per axis.
- **Some candidate axes may not exist in the embedding.** MuQ-MuLan
  was trained on a specific objective; if its representation didn't
  capture (say) "rhythmic complexity" cleanly, that axis won't probe
  out. Empirically detectable via low-confidence probe accuracy.
- **k inflation**. Easy to add more candidate axes than the embedding
  can support. Use mutual information / correlation pruning post-
  training to keep only orthogonal axes.
- **LLM cost scales linearly with k**. 12 axes × 12 min = 2.5 GPU-hr.
  Acceptable. 50 axes would not be.

## Expected results (working theory)

| Metric | V6 (round 5) | Head (round 6) | Discovery (round 7) |
|---|---|---|---|
| Pairwise agreement | 63.5% | 75-80% | 80-85% |
| Spearman vs hand | +0.39 | +0.50-0.55 | +0.55-0.60 |
| Interpretability | full | none | full |
| Cross-library cost | retrain axes + blend | retrain head | refit blend (~12 weights) |
| Total params trained | 6 | ~100,000 | ~10,000 |
| LLM-judge GPU cost | already done | 0 | ~2.5 hours |

## Implementation effort

- Per-axis prompt design + tournament: 1 day (12 axes, prompt A/B,
  tournament runs are mostly compute-bound)
- Multi-task probe training: 0.5 day (PyTorch, ~200 lines)
- Joint blend optimisation: 0.5 day
- Axis interpretation + naming: 0.5 day (manual inspection of top/bottom
  tracks per axis)
- Cross-library transfer test: 0.5 day (need a small external library)
- Report writing: 0.5 day
- **Total: ~3-4 days end-to-end**

## Files / artifacts (planned)

- `spike/track-grading/per_axis_prompts.py` — 12 axis prompt templates
- `spike/track-grading/run_per_axis_tournament.py` — wrapper around
  `judge_pairs_vllm.py` for per-axis judgments
- `spike/track-grading/train_axis_probes.py` — multi-task probe
  training
- `spike/track-grading/joint_blend.py` — blend-weight optimisation
- `spike/track-grading/interpret_axes.py` — axis interpretation utility
- `documents/axis-eval-results/learned-axes-r7.npz` — k×768 axis matrix
  + k blend weights
- `documents/axis-eval-results/V15_axis_discovery.csv` — predictions per
  track in V*.csv format
- `documents/aggression-axis-eval-round-7.md` — final results report
  (replacing this plan section)

## Follow-on rounds (speculative)

**Round 8** — cross-library generalisation: pick 3-5 libraries with
distinct genre profiles (DnB, jazz, electronic, folk). Run blend-weight
fine-tune on each. Report Spearman vs hand-judged labels per library.
Validate that round-7 axes hold up across genres.

**Round 9** — uncertainty-driven ensemble: train multiple round-7
models with different seeds / prompt variations. For each track,
compute prediction variance across the ensemble. Tracks with high
variance → flag for human review or ask Qwen with a different prompt
phrasing.

## Cross-references

- [Round 5](aggression-axis-eval-round-5.md) — sampling design that
  produces the BT priors used here as the joint-blend target
- [Round 6 plan](aggression-axis-eval-round-6.md) — the simpler
  single-head approach, runs first to validate infrastructure
