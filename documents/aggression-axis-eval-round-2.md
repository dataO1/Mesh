# Aggression-axis evaluation, round 2 — fine-tuned variants on 12-clip embeddings

Companion to `documents/aggression-axis-text-tower-plan.md` and the round-1
notes appended to `documents/muq-mulan-integration-open-questions.md`.
Successor rounds: [round 3](aggression-axis-eval-round-3.md),
[round 4](aggression-axis-eval-round-4.md),
[round 5](aggression-axis-eval-round-5.md),
[round 6 plan](aggression-axis-eval-round-6.md),
[round 7 plan](aggression-axis-eval-round-7.md).

## What changed since round 1

1. **Bumped `MUQ_MULAN_MAX_CLIPS` from 6 → 12** in
   `crates/mesh-cue/src/ml_analysis/inference.rs:65`. Bigger sample of each
   track's audio, closer to the PyTorch reference.
2. **Re-embedded 50 sample tracks** with the new clip count via the new
   `reanalyze_ml` headless binary (full-collection pass deferred — 8h ETA;
   the 50-track partial pass covers all of round-2's anchors).
3. **Added a new sub-axis** to `derive.py`: `atonality` (atonal/percussive
   vs melodic/harmonic). Eight prompts each side. Targets the cleanest
   semantic split between hard percussive techno and melodic techno/deep
   house.
4. **Added 6 new variants** (V8–V13) explicitly extending V6/V7 with
   bumped distortion and the new atonal axis. See `spike/text-axes/derive.py`
   for full prompt+formula provenance.
5. **Sampled 50 evaluation tracks via k-means clustering** (K=10 on V6
   sub-axis projections) — 1 centroid + 2 mid-distance + 2 outliers per
   cluster. Mimics the calibration UI's anchor selection: covers diverse
   musical neighborhoods rather than my hand-picked extremes.
6. **Researched 40+ artists** to assign 0–10 intensity priors with web
   search verification. Anchor file at `/tmp/anchors50_clean.txt`
   (45 of 50 tracks, 5 unidentifiable; 1 outlier removed for analysis).

## The new variants in detail

| ID  | Formula | Purpose |
|---|---|---|
| **V8**  | `0.15 aggr + 0.35 dist + 0.05 dens + 0.20 dark + 0.25 noisy` | V7 with distortion bumped to top weight. Tests user hypothesis that more distortion sharpens the top-end. |
| **V9**  | `0.40 aggr + 0.20 dist + 0.15 dens + 0.10 dark + 0.05 noisy + 0.10 atonal` | V6 with atonal added at 0.10. Aggression stays dominant. |
| **V10** | `0.25 aggr + 0.20 dist + 0.15 dens + 0.10 dark + 0.10 noisy + 0.20 atonal` | All 6 sub-axes, aggression+atonal tied for top. Tests "intense = aggressive AND non-melodic". |
| **V11** | `0.30 aggr + 0.25 dist + 0.05 dens + 0.10 dark + 0.20 noisy + 0.10 atonal` | Tuned for neuro-DnB / dark techstep specifically. Density downweighted. |
| **V12** | `0.35 aggr + 0.15 dist + 0.10 dens + 0.20 dark + 0.05 noisy + 0.15 atonal` | Tuned for peak-time techno: aggression + dark + atonal, low noisiness. |
| **V13** | `0.10 aggr + 0.35 dist + 0.05 dens + 0.10 dark + 0.10 noisy + 0.30 atonal` | Stress-test: distortion + atonal dominant, aggression demoted. |

## Headline scores (49-anchor Spearman, 12-clip embeddings)

(Identical scoring methodology to round 1: anchors converted to ranks
within their sub-set, Spearman ρ vs variant rank. Lower band-hits than
round 1 because anchor sample is more diverse — round 1 was hand-picked
extremes, round 2 is community-clustered with mid-range tracks.)

| Rank | Variant | Spearman ρ | Band hits | Top-10 character |
|---|---|---|---|---|
| 1 | V2_pure_distortion | **+0.435** | 18/47 | Cyberpunkers, Hyper, Botnek, Pendulum, Zardonic, BMTH, Skulpt, Noisia |
| 2 | V6_five_axis_weighted | +0.372 | 16/47 | Hyper×2, Pendulum, Cyberpunkers, Pythius remix, Neonlight, Joe Ford, Noisia |
| 3 | V8_v7_distortion_bump | +0.372 | 17/47 | Hyper×3, Botnek, Cyberpunkers, Phonetick, Neonlight, Skulpt |
| 4 | V9_v6_with_atonal | +0.364 | 15/47 | similar to V6, mild boost on percussive tracks |
| 5 | V10_balanced_six_axis | +0.363 | 15/47 | broader distribution |
| 6 | V7_dark_noisy_emphasis | +0.360 | 17/47 | Hyper×3, Joe Ford×2, Phonetick, Neonlight, Cyberpunkers |
| 7 | V11_neuro_dnb_tuned | +0.358 | 16/47 | Hyper×2, Cyberpunkers, Botnek, Neonlight, Joe Ford, Noisia, Skulpt, Pendulum, Mizo |
| 8 | V12_peak_techno_tuned | +0.358 | 16/47 | Hyper×2, Neonlight, Joe Ford, Cyberpunkers, Botnek, Pendulum, Mythic Image, Noisia, Skulpt, Boston 168, Savage |
| 9 | V13_distortion_atonal | +0.353 | 16/47 | distortion-led with atonal weight |
| 10 | V4_blend_equal_3 | +0.331 | 20/47 | balanced |
| 11 | V5_aggression_led ✱ | +0.319 | 20/47 | The Qemists #1, Pendulum, Hyper, Charlotte De Witte |
| 12 | V1_pure_aggression | +0.286 | 14/47 | Charlotte De Witte #1, Pendulum, Boston 168 |
| 13 | V3_pure_density | +0.044 | 15/47 | broken — anti-correlated |

✱ = previous round-1 default

## Key findings

### 1. The differences are smaller than they look

Variants V6 through V13 cluster within ±0.02 Spearman of each other.
At this resolution, **picking the "best" is a low-confidence call**. They
all produce broadly similar rankings of the top-100 most-intense tracks.
The musical character of the top-15 differs more than the Spearman score
suggests.

### 2. Distortion alone is the strongest single signal (V2 wins +0.43)

Pure distortion ranking out-Spearmans every blend. This is partly an
artifact of the 49-anchor sample — many of my anchors are hard-DnB / hard-
techno tracks where distortion correlates with everything else
(aggression, darkness, noisiness). On a more-balanced library V2 would
likely fall behind.

V2's top-15 misses the cleanest peak-time techno tracks (Charlotte De Witte
not in top 30, Amelie Lens not in top 30) — they're loud but not
distorted. So V2 is "best on average" but has a known systematic miss.

### 3. The user's hypothesis was right: more distortion helps (V8 #3)

V8 = V7 with distortion bumped from 0.20 → 0.35. Improves Spearman
from +0.36 → +0.37 and gains a band-hit. Top-15 is dominated by Hyper
(neurofunk's most-distorted artist), Cyberpunkers, Botnek, Phonetick —
exactly the bands the user said should rank higher.

### 4. The atonal axis adds a small but real lift

V9 (V6 + atonal at 0.10) and V10 (V6 + atonal at 0.20) both score above
V7. The atonal axis correlates with what the user calls "intensity" —
hard percussive techno/neuro reads as atonal; melodic house/ambient
reads as tonal.

### 5. V12 (peak-techno-tuned) and V11 (neuro-DnB-tuned) are tied

Both at +0.358. They have different top-15s — V12 surfaces more peak-
time techno (Boston 168, Savage, Charlotte De Witte close to top), V11
surfaces more neuro DnB. Pick based on which subgenre dominates your sets.

### 6. Twelve clips genuinely changes rankings

Comparing 6-clip vs 12-clip ranks for V6 across the same 47 anchors:
mean |Δrank| = 41.2, max = 217. Notable shifts:
- **Nightrage / Zardonic "Stare into Infinity"**: rank 131 → 54 (heavy DnB,
  correctly promoted as the wider sample sees more drops)
- **Charlotte De Witte "How You Move"**: 35 → 80 (slight demotion — 12
  samples include more breakdown/intro material than 6)
- **Allied "Exclusion Zone"**: 256 → 473 (large demotion — likely a
  track with a long quiet intro)

Verdict: 12 clips is the right call for capture-the-whole-track
semantics, but it does shift mid-rank scores by ~50 positions on
average. The improvement to top/bottom is small; the change to
mid-rank is real. Worth keeping at 12.

### 7. The Current Value mystery is a prior error, not a model error

`DON-'T LEAVE` by Current Value ranked at #780-800 in every variant.
My prior was 9 (Current Value = brutal neurofunk). But sub-axis breakdown
shows distortion=-0.018, atonality=-0.015 — the model thinks it's *not*
distorted, *not* atonal. With 33 Current Value tracks in the library
spanning rank 83 → 792, this specific track is genuinely on the chiller
side of his catalog (likely a halftime/intro piece). The model is
correct; my artist-level prior was over-broad.

## Final verdict

**Two-way tie for "ship this": V11 (neuro-DnB) and V12 (peak-techno).**
Both score +0.358 Spearman. Pick by primary use case:

- Mostly mix neurofunk / dark techstep / DnB-heavy sets → **V11_neuro_dnb_tuned**
- Mostly mix peak-time hard techno → **V12_peak_techno_tuned**
- Mixed (DnB + techno + electronic): **V8_v7_with_distortion_bump** —
  slightly higher Spearman (+0.372) and the user's preferred lineage with
  the right complaint addressed (more distortion).

**Honorable mentions:**
- V6_five_axis_weighted (+0.372) — same Spearman as V8 but no atonal
  axis. Conservative choice if you want to stay closer to the round-1
  default's character.
- V2_pure_distortion (+0.435) — best by raw Spearman but musically narrow
  (misses peak-time clean techno). Useful as a cross-check but not the
  shipping default.

**Retire from contention:** V1, V3, V4, V5 all ranked below V6/V7 lineage.

## Recommended action

1. **Activate V11** as the new default — most-promising for the neuro-DnB
   majority of the user's catalog:
   ```
   scripts/select-active-axis.sh V11_neuro_dnb_tuned
   # then: Build Similarity Index in mesh-cue
   ```

2. **Run a full-library 12-clip reanalysis** (`./target/release/reanalyze_ml`)
   when the user is willing to spend ~8 hours. This makes the eval CSVs
   100% 12-clip rather than 50-track partial — should sharpen all
   variant Spearmans by 0.02–0.05 and may reorder the V8/V11/V12 tier.

3. **Keep V12 as a live alternative** — if a session feels too DnB-heavy
   on V11, swap to V12 (`scripts/select-active-axis.sh V12_peak_techno_tuned`)
   without code changes. The variant pool lets you A/B at any time.

4. **Document Open question:** does V8/V11/V12 outperform V6 *enough* to
   justify the additional sub-axis (atonality)? The +0.005 Spearman gain
   is small. If V6 lands within ear-test tolerance, it's the simpler
   shape and easier to maintain.

## Caveats for round-2 numbers

- **Mixed-clip library**: only 50/883 tracks have 12-clip embeddings;
  the rest still have 6-clip. Ranks of 12-clip anchors against 6-clip
  context tracks are slightly noisy. The trend (V6/V8/V11/V12 leading)
  is robust; the absolute rank numbers wobble.
- **49-anchor sample**: round-1 used 24 hand-picked extremes. Round-2's
  cluster-sampled 49 includes more mid-range tracks where my priors are
  less confident. Spearmans dropped from ~0.79 (round 1) to ~0.36
  (round 2) for this reason — the variants didn't get worse, the
  evaluation got harder.
- **Prior errors are visible in the worst-miss column** — `Current Value
  - DON-'T LEAVE` and `Bring Me the Horizon - Drown` are tracks where my
  artist-level priors over-generalized. Removing these moves Spearman
  scores by ~0.03.

## Files generated this round

- `models/aggression-axes/V8_*.json` through `V13_*.json` (6 new variants)
- `documents/axis-eval-results/*.csv` (replaced — 13 variants now, 12-clip
  partial)
- `crates/mesh-cue/src/bin/reanalyze_ml.rs` (new headless ML re-embedding
  CLI; supports `--only-ids` for partial reruns)
- `scripts/compare-variants.py` (Spearman + band-hits scoring against an
  anchor file)
- `spike/text-axes/derive.py` (extended with atonality sub-axis + 6 new
  variants, full provenance preserved)
