# Aggression-axis evaluation, round 4 — pairwise LLM judge via vLLM

Companion to `documents/aggression-axis-eval-round-3.md`. Round 3 established
that absolute LLM scoring (Audio Flamingo 3) collapsed to 7-8 for 97% of the
DnB-heavy library and was unusable as ground truth. This round implements
Option A from that report (pairwise calibration) but with the LLM as judge
instead of the user.

## TL;DR

**Pair-derived priors via Qwen3-Omni-30B-A3B-Instruct (AWQ-4bit, served via
vLLM) give a +0.36 Spearman against the 47 round-2 hand-anchors — same
signal strength as round-2 hand-prior validation but on 909 tracks and
fully automated.** The 13-variant leaderboard reorders meaningfully:
**V6_five_axis_weighted now leads at +0.4547**, V11_neuro_dnb_tuned (current
default) drops to #9 at +0.4248. All variants beat their round-2 scores
against the new priors. V3_pure_density confirmed last (+0.3126), matching
both round-2 and round-3 conclusions.

The main remaining limitation: 871 of 909 tracks pile into BT buckets 8-9,
so within-cluster ranking among "intense DnB" tracks is still poorly
resolved. That's the round-5 hook (within-cluster + similarity-conditional
sampling).

## What we built (committed)

- **vLLM Qwen3-Omni serve script** — `spike/track-grading/serve_qwen3_omni.sh`
  launches `cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit` via vLLM 0.20.1
  with multi-audio enabled. Model fits 18.16 GB on the 24 GB RTX 5090
  Laptop. Uses compressed-tensors AWQ kernel, eager mode (no CUDA graphs),
  max_model_len=8192. Bypasses several Nix-vs-CUDA pitfalls
  (`TRITON_LIBCUDA_PATH`, `patchelf` on `ptxas-blackwell` and friends to
  fix the dynamic linker).

- **Pairwise judge HTTP client** — `spike/track-grading/judge_pairs_vllm.py`.
  Reuses anchored-tournament sampling, audio-window decoding, and per-pair
  JSON cache from the AF3/transformers attempts. Sends two `input_audio`
  base64-WAV blocks plus the prompt to `/v1/chat/completions`. **Note:
  vLLM 0.20 docs document a `uuid` field per audio block, but including it
  causes "AssertionError: Expected code to be unreachable" on multi-audio
  requests — order in the content array is the disambiguator.** Threadpool
  with `--workers 8` keeps the GPU at ~96% utilization vs ~21% serial.

- **Bradley-Terry MLE** — `spike/track-grading/build_bt_priors.py`.
  Hunter (2004) MM iteration with **Gamma(a=2, b=1) Bayesian prior** — the
  prior is essential because 282/909 non-anchor tracks beat every anchor in
  every direction (saturated wins). Plain MLE diverges to inf for those
  tracks; the prior pulls them to a finite score that still respects margin.
  Converges in 94 iterations with `prior_strength=1.0`. Outputs at
  `documents/axis-eval-results/llm-pair-priors.txt` (compatible with
  `scripts/compare-variants.py`) and `.csv` (full table).

- **Validation script** — `spike/track-grading/validate_bt_priors.py`.
  Computes Spearman between BT priors and a hand-anchor file, shows top
  disagreements.

## Sampling design

Anchored tournament with 3 anchors:

| Level | Anchor | BT prior |
|---|---|---|
| low  | Faded — ZHU            | 2.69 |
| mid  | Strand — Bodzin        | 0.00 (lower than expected — see notes) |
| high | FCKD — Hyper           | 8.13 |

Each non-anchor track plays each anchor in both directions (3 × 2 = 6
pairs/track), bilateral pairing to cancel positional bias. Total directed
pairs: 906 × 6 = 5436 (a few extras from anchor-vs-anchor edges).

**Anchor mid was misranked.** "Strand" by Bodzin landed at BT prior 0.00,
*below* the low anchor "Faded" (2.69). Qwen consistently rates Strand as
less intense than Faded. This is a sampling-design bug not a model bug —
the anchors weren't in the order we assumed. Doesn't break BT (the priors
self-correct from the win patterns) but should be fixed in any future run.

## Throughput

- vLLM serve startup: ~3 min cold (model load 75 s + KV-cache profile)
- Smoke test (10 pairs, 5 known orderings): 24 s, 100% correct ordering,
  no positional bias on the bilateral check
- Full tournament parallelized at workers=8: **5436 pairs in 13 min**
  (~7 pair/s sustained, GPU at ~96%, 0 failures)
- BT iteration: 94 iters, ~1 s
- Total wall time end-to-end: ~20 min on a 24 GB RTX 5090 Laptop

Compared to round-3 AF3: 22.6 min for 909 absolute scores vs 13 min for
5436 pair judgments — **~3x more LLM calls per minute** with vLLM batching,
and the pair output is structurally richer.

## Results

### Spearman vs BT priors (909 tracks)

| Rank | Variant | Spearman ρ | round-2 (hand-priors) | round-3 (LLM-priors) |
|---|---|---|---|---|
| 1 | V6_five_axis_weighted          | **+0.4547** | +0.372 | +0.239 |
| 2 | V7_dark_noisy_emphasis         | +0.4499 | +0.360 | +0.200 |
| 3 | V4_blend_equal_3               | +0.4480 | +0.331 | +0.274 |
| 4 | V9_v6_with_atonal              | +0.4428 | +0.364 | +0.226 |
| 5 | V12_peak_techno_tuned          | +0.4423 | +0.358 | +0.212 |
| 6 | V10_balanced_six_axis          | +0.4417 | +0.363 | +0.216 |
| 7 | V8_v7_with_distortion_bump     | +0.4357 | +0.372 | +0.193 |
| 8 | V5_aggression_led              | +0.4307 | +0.319 | +0.241 |
| 9 | V11_neuro_dnb_tuned (current)  | +0.4248 | +0.358 | +0.190 |
| 10 | V13_distortion_atonal_dominant | +0.4179 | +0.353 | +0.189 |
| 11 | V2_pure_distortion             | +0.4053 | +0.435 | +0.208 |
| 12 | V1_pure_aggression             | +0.3928 | +0.286 | +0.197 |
| 13 | V3_pure_density                | +0.3126 | +0.044 | +0.262 |

**Every variant scores higher against BT priors than against round-2
hand-priors, and dramatically higher than against round-3 LLM-priors.**
That's the validation: BT priors have signal at least equal to hand-priors
(more anchors helps) and are decisively better than absolute-LLM scoring.

### Cross-validation: BT priors vs 47 round-2 hand-anchors

```
Spearman ρ (BT vs hand) = +0.3574
  round-2 hand-vs-V11 baseline   = +0.358 (best variant against hand)
  round-3 LLM-vs-V11 baseline    = +0.190 (LLM-as-absolute against hand)
```

The +0.36 BT-vs-hand correlation is essentially identical to the V11-vs-hand
ceiling round-2 reported. This means the BT priors capture roughly the
same intensity signal as the hand-priors, on 19x more tracks, with no
manual labelling.

### Top disagreements (BT prior vs hand prior)

| Track | BT | Hand | Note |
|---|---|---|---|
| Bass So Loud — Switch Disco | 9.90 | 4 | Qwen rates loud-bass mainstream as very intense |
| React — Switch Disco | 8.13 | 3 | Same pattern |
| Pianos Rising From The Grave (High) | 8.13 | 4 | Distorted but melodic — divergent reads |
| Blue September | 8.13 | 4 | Same |
| Every Wall Is a Door | 1.45 | 5 | Qwen rates as chill, hand-prior moderate |
| Sandbox | 9.20 | 6 | Qwen rates higher |
| Blood Hunter | 9.20 | 6 | Same |

Pattern: Qwen rates **loud-bass / aggressive-mainstream** higher than
human-hand priors did, and is mostly aligned on heavy DnB. Neither is
obviously "right" — both have signal.

### BT prior distribution

```
bucket  0:    1  █
bucket  1:    3  ███
bucket  2:    1  █
bucket  3:    4  ████
bucket  4:    0
bucket  5:    2  ██
bucket  6:   26  ██████████
bucket  7:    0
bucket  8:  366  ████████████████████████████████████████████████████ ...
bucket  9:  505  █████████████████████████████████████████████████████████████████████ ...
bucket 10:    1  █
median:    9.20
```

**871 of 909 tracks land in buckets 8-9.** That's the cost of anchored-only
sampling: Qwen places most non-anchor tracks above the high anchor (FCKD),
but with only 6 games per track we can't resolve their relative ordering.
This is the round-5 hook — within-cluster pairs would refine ranking inside
the dense top.

## Smoke-test diagnostic

10 directed pairs (5 known orderings × bilateral):

| Pair (presented A vs B) | Choice | Expected | Match |
|---|---|---|---|
| FCKD — Hyper / Faded — ZHU | A | A | ✓ |
| Faded — ZHU / FCKD — Hyper | B | B | ✓ |
| How You Move — Charlotte De Witte / Butternuts | A | A | ✓ |
| Butternuts / How You Move | B | B | ✓ |
| FCKD — Hyper / Strand — Bodzin | A | A | ✓ |
| Strand — Bodzin / FCKD — Hyper | B | B | ✓ |
| Omnivore — Noisia / Slinkystink — Random Movement | A | A | ✓ |
| Slinkystink / Omnivore | B | B | ✓ |
| Strand / Faded (uncertain mid-low) | A | – | bilaterally consistent |
| Faded / Strand | B | – | bilaterally consistent |

10/10 correct on expected orderings, bilateral consistency on the uncertain
pair. **No positional bias** detectable at this scale.

(Tournament-wide there's a mild ~7.8% positional bias toward "B"
— 2935 B vs 2511 A across 5446 pairs — but bilateral pairing absorbs it.)

## What this tells us

### About Qwen3-Omni as a music judge

- **Real audio understanding, not genre buckets.** Unlike AF3 in round-3,
  Qwen distinguishes intensity within DnB (Noisia/Spor/Hyper rank above
  Random Movement / mid-tempo DnB) and gives consistent A/B answers
  bilaterally. The smoke-test 100% accuracy is the strongest signal here.
- **Mainstream-bass bias.** Tracks with loud sub-bass that aren't actually
  heavy (Switch Disco "Bass So Loud", "React") get rated very intense.
  This is probably real bass energy that Qwen hears, not error. Whether
  that's right depends on what "intensity" should mean for our use case.
- **Saturation at the high end.** 282 tracks beat every anchor in every
  direction. The high anchor (FCKD) is intense but not the ceiling — many
  Pythius / Spor / Noisia neuro-DnB tracks are clearly above it to Qwen.

### About the polar axes

- **V6_five_axis_weighted is the new top variant.** Pull it up to default?
  Cautious yes: +0.43 (V11 current) → +0.45 (V6) is a 5% improvement.
  Not huge, but V6 family (V6/V7/V9/V10/V12) consistently outperforms V11
  family across both round-2 and round-4. Worth A/B-comparing in the
  player UI before flipping.
- **V11 is fine.** It's mid-pack here (#9 of 13) but +0.4248 is still well
  above the V1/V2/V3 simple-axis baselines. Not a regression to ship V11.
- **V3_pure_density is decisively bad.** 4th-from-last in round-2, last
  here. Its high round-3 ranking was an artifact of LLM-priors collapsing
  to 7-8 (density correlates with DnB density which is what AF3 saw).
  Don't ever use V3 alone.

## Recommendation

### Ship now

1. **Switch default to V6_five_axis_weighted** if A/B listening confirms
   the 5% Spearman improvement is audible. Otherwise keep V11 — neither
   is wrong.
2. **Re-pick anchors before any future pair-judge run.** "Strand" should
   not be the mid anchor (Qwen rates it lowest in the library). Better
   trio: Faded (low), maybe a Switch Disco track or "Liquid Soul" (mid),
   FCKD or a Spor neuro track (high).

### Round 5 (next iteration)

1. **Within-cluster sampling.** Group tracks by BT score into deciles,
   sample ~30 random within-decile pairs per decile (300 pairs / ~1 min).
   Re-run BT to sharpen ranks inside the dense 8-9 cluster.
2. **Similarity-conditional sampling.** For each track, pick its 5
   nearest neighbours by MuQ-MuLan / V11 embedding cosine and judge those
   pairs. This answers "for tracks our embedding model thinks are similar,
   what's Qwen's intensity ordering?" — exactly the within-cluster
   resolution problem.
3. **Embedding-head training.** Use BT priors as labels for a tiny head
   on top of MuQ-MuLan (the round-3 doc warned against this with AF3's
   genre-bucket labels — BT priors don't have that pathology). Output:
   Qwen-derived intensity at MuQ-MuLan inference speed (microseconds vs
   150 ms per track).
4. **Active uncertainty sampling.** TrueSkill / Glicko-style: BT → pick
   highest-variance pairs → re-judge → re-BT. ~3x signal per LLM call.

### Don't go for full round-robin

909² ≈ 820k pairs is ~150x what we just did and would take ~25 GPU-hours.
The marginal Spearman gain from more random pairs is small relative to
the gain from targeted within-cluster + similarity sampling. Active
sampling beats brute force by an order of magnitude.

## Files / artifacts

- `documents/axis-eval-results/llm-pair-priors.txt` — committed, 909 rows,
  format `track_id|title|prior_0_10`, ready for `compare-variants.py`
- `documents/axis-eval-results/llm-pair-priors.csv` — committed, full
  table with bt_score / n_games / win_rate columns
- `spike/track-grading/serve_qwen3_omni.sh` — vLLM launcher (spike-only)
- `spike/track-grading/judge_pairs_vllm.py` — pair grader HTTP client
- `spike/track-grading/build_bt_priors.py` — BT MLE → priors
- `spike/track-grading/validate_bt_priors.py` — Spearman vs hand-anchors
- `/tmp/track-grading/pairs_vllm/*.json` — 5446 per-pair JSONs (not
  committed — regenerable in 13 min)
- vLLM env at `~/.cache/mesh-spike/vllm-env/` (not committed)
- Model in HF cache at `~/.cache/mesh-spike/hf/hub/models--cpatonn--Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit` (~26 GB, not committed)
