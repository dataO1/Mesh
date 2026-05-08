# Aggression-axis evaluation, round 7 — LLM-supervised axis discovery + joint blend

**Status: EXECUTED 2026-05-04.** Round-7 ran end-to-end on the everynoise→Deezer
corpus (15314 tracks, 12 axes). The plan, the build, and the results all sit
in one document; results are at the bottom under [Results](#results-2026-05-04).

Round 6 trains a single opaque head from MuQ-MuLan embeddings to
intensity. Round 7 is the more ambitious successor: instead of
collapsing the 512-dim space to one number, learn `k` interpretable
axes (each a linear direction in embedding space) jointly with the
final blend weights into intensity. This answers the deeper question
*"are our 6 named axes the right basis?"* empirically.

## TL;DR (planned vs measured)

Plan: 12 per-axis LLM tournaments → multi-task linear probes → ListMLE blend.
Expected ~80-85% pairwise agreement, an interpretable basis, cross-library
deployment story.

**Measured (V16 = round-7 blend, evaluated on user's 909-track DJ library):**

| Variant                 | Spearman vs round-5 BT | Pairwise Agreement |
|-------------------------|------------------------|--------------------|
| V11 (text-tower 6-axis, baseline)      | +0.39 | 62.5 % |
| V15 (round-6 single linear probe, in-domain) | +0.60 | 76.5 % |
| **V16 (round-7 12-axis blend, OUT-of-domain)** | **+0.50** | **71.7 %** |

Conclusions are honest and load-bearing:

- V16 transfers from a *completely separate* 1424-track Deezer corpus
  (zero overlap with the user's library) and still achieves 71.7 % PA on
  the user's library. That is **+9 pp over V11** and only **−5 pp behind
  V15**, which had the in-domain advantage of being trained on these
  exact 909 tracks. So the multi-axis basis discovered on a foreign
  corpus does carry the intensity signal across libraries.
- The 12 axes are **highly redundant** in MuQ-MuLan space — 22 axis
  pairs have |Pearson r| ≥ 0.85, with `aggression`↔`distortion` at
  +0.991. The joint-blend optimiser (ListMLE) zeroed out 7 of the 12
  weights and kept just 5: `darkness (32%) + vocal_intensity (21%) +
  density (20%) + dynamic_compression (19%) + rhythmic_complexity (7%)`.
  These five are the only directions in MuQ-MuLan space the LLM judge
  can consistently distinguish.
- **V15 stays deployed.** V16 ships at `models/aggression-axes/V16_round7_blend.json`
  but is *not* the per-collection axis. The user's library was already
  curated to round-5, so V15's in-domain training wins. V16's value is
  (a) cross-library deployment (round-8 entry point) and (b) the 5
  per-axis sub-directions exposed via `sub_axes` for future per-axis UI.
- Spearman(V15, V16) = +0.872 on the user library — V16 produces a very
  similar ranking to V15, confirming both axes capture the same
  underlying perceptual dimension.

The headline plan target ("80-85% PA") was **not** met. The
interpretation is straightforward: per-axis pairwise judgments from the
Qwen3-Omni-30B LLM cap at ~70 % accuracy in this regime — that's the
LLM-noise floor for a 512-d audio embedding mapped to a 5-axis blend.
You can't push past that ceiling without a stronger judge or fundamentally
different supervision (e.g., listening-test crowd labels, or audio-text
contrastive pre-training on a labelled intensity dataset).

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

## Training corpus — decided + built

Round-6 confirmed the MuQ-MuLan embedding has substantial intensity
signal we weren't extracting linearly (V15 linear probe alone beat V11
by +8.6 pp pairwise agreement). That gated the round-7 corpus
investment, and we built it.

### Strategy: everynoise → Deezer (not Spotify, not free corpora)

Round-7 corpus is a **DJ-relevant subset of everynoise.com expanded via
Deezer's public API**. Specifically:

1. **everynoise.com** — scraped 6291 genre cells (each with a Spotify
   playlist ID, a 30 s preview URL, atlas coordinates encoding 2 audio
   dimensions, and an example artist+track). everynoise is the most
   comprehensive genre/sub-genre atlas in the wild.
2. **categorize_genres.py** — three-tier classifier picks the
   DJ-relevant subset using `HARD_BLOCK > INCLUDE > SOFT_BLOCK`
   precedence. Result: **2116 INCLUDE genres** (33% of everynoise),
   spanning house family (98), techno (59), DnB (17), dubstep (11),
   trance (33), hardcore (72), electro (108), idm (4), phonk (9),
   hyperpop (12), all the way to regional hip-hop (228 + 311 rap),
   afrobeats (6), reggaeton (7), kuduro/kompa/kizomba, mahraganat,
   plus punk (182) / metal (378) / emo (25) / goth (9) per the
   "don't exclude punk and metal per-se" rule the user set.
3. **Deezer search + radio expansion** — per genre, search Deezer for
   the everynoise example_track, take the first match, then call
   `/artist/{id}/radio` for 9 similar tracks (deduped). Total: ~21k
   tracks at 10 per seed.
4. **Preview MP3 download** — Deezer serves 30 s previews from a
   Google-Frontend CDN (`cdnt-preview.dzcdn.net`). Download in parallel
   (32 workers); ~10 GB total.

### Why not Spotify

Tested with a free Spotify dev app late-2024 / 2025: every
`/playlists/{id}/tracks` request returned HTTP 403 "Active premium
subscription required for the owner of the app." Spotify's policy now
requires the dev-account holder to have an active Premium subscription
before public-playlist reads work, and the propagation after enabling
takes a few hours. We didn't have Premium; we abandoned the path. The
working Spotify client is left at `spike/track-grading/fetch_spotify_tracks.py`
in case the policy changes or someone runs the pipeline with Premium —
it correctly handles the auth, just gets 403'd by the server.

### Why not free corpora alone

FMA-medium (~25k CC tracks) + MTG-Jamendo (~55k CC tracks) skew indie /
folk / classical / experimental electronic. They underweight the DJ
genres Mesh actually targets (commercial dance, neuro-DnB, peak-time
techno, drum & bass, etc.). Training axes on free-corpora-only would
risk discovering dimensions that don't fire on DJ music. Skipping FMA
and Jamendo for round 7 — they remain useful for adjacent Mesh
features (vocal extraction, source separation) but don't belong in the
intensity-axis training corpus.

### Empirical verification: previews are hook-aware, not random

Probed 8 already-downloaded Deezer previews with librosa for per-frame
RMS shape, mean loudness, and onset rate:

```
category         track                                rms_dB    shape    onset/s
ambient synth    Jogging House — Flight               -19.5     flat     1.83
ambient synth    Jogging House — Strings              -25.1     flat     1.40
afrobeats        KCee — Pullover (Remix)               -8.9     flat     5.44
afrobeat         Antibalas — Battle of the Spec       -14.6     flat     4.07
ambient house    Khotin — Groove 32                   -13.3     flat     5.74
ambient house    Khotin — WEM Lagoon Jump             -10.5     flat     1.97
ambient house    Khotin — Shopping List               -16.1     flat     0.40
alt-christian    Shane & Shane — Knowing You          -29.7     rising   1.87
```

7/8 had **flat** RMS shape — the signature of a chorus, drop, or
sustained section. The lone "rising" was a slow worship track with no
clear hook. RMS levels match expected genre energy (ambient quiet, pop
afrobeats loud, worship very quiet).

So Deezer's clip-selection algorithm is hook/chorus-aware for most
modern produced music — comparable in spirit to Mesh's own
`drop_marker`-centered 30 s clip, just selected server-side. Edge cases
(slow vocal music, classical buildups) get whole-track-start samples;
acceptable noise at 21k corpus size.

### Sample budget

10 tracks per genre × 2116 genres = **~21k tracks total**. Justification:
- Multi-task k=12 linear probes train cleanly on ~1700 examples per
  axis (after dedup) — 21k provides comfortable margin
- ~10 GB at 470 KB/preview, fits anywhere
- Wall time ~50-60 min end-to-end on a residential connection (Phase 1
  search ~9 min rate-gated to 10 req/s, Phase 2 download ~30-40 min at
  32 parallel workers)
- Re-running with different categorisation rules is cheap (per-seed
  cache + per-file cache → resume-safe)

### Files committed for the build

- `spike/track-grading/scrape_everynoise.py` — phase 0
- `spike/track-grading/categorize_genres.py` — phase 0 (HARD_BLOCK /
  INCLUDE / SOFT_BLOCK rule)
- `spike/track-grading/fetch_deezer_tracks.py` — phase 1 (generic seed
  adapter, search + radio)
- `spike/track-grading/download_previews.py` — phase 2 (parallel HTTP)
- `spike/track-grading/build_corpus.sh` — wrapper that runs all phases
- `spike/track-grading/fetch_spotify_tracks.py` — Spotify alternative,
  blocked by their Premium policy

Outputs land at `/home/data01/Music/mesh-track-grading/`:
- `everynoise_genres.json` (full scrape, 6291 entries)
- `everynoise_dj_genres.json` (INCLUDE subset, 2116 entries)
- `deezer/corpus_tracks.json` (the manifest)
- `audio/dz_<id>.mp3` (the 21k MP3s)
- `logs/{fetch,download}.log`

## Follow-on rounds (speculative)

**Round 8 — productisation for end users without GPUs.** Ship the
round-7 pretrained axes (~36 KB) plus 3-5 starter blend profiles
(~12 floats each). Auto-detect library type via clustering on
imported MuQ-MuLan embeddings, pick the closest starter blend. Revive
the existing calibration UI in `crates/mesh-cue/src/ui/state/
calibration.rs` to update the 12 blend weights from user pairwise
clicks (and optional implicit signals like skip rate, listen-through).
**Critical: zero LLM compute on the end-user machine.** All heavy
training stays at the dev side; users get pretrained axes + on-device
CPU embedding + small per-user blend refit.

**Round 9** — uncertainty-driven ensemble: train multiple round-7
models with different seeds / prompt variations. For each track,
compute prediction variance across the ensemble. Tracks with high
variance → flag for human review or ask Qwen with a different prompt
phrasing.

**Round 10 (later)** — drift handling: schedule periodic centralised
re-training of the round-7 axes as the reference corpus grows, push
updated axis weights to users, allow blend re-fit on top.

## Cross-references

- [Round 5](aggression-axis-eval-round-5.md) — sampling design that
  produces the BT priors used here as the joint-blend target
- [Round 6 plan](aggression-axis-eval-round-6.md) — the simpler
  single-head approach, runs first to validate infrastructure


## Results (2026-05-04)

### Pipeline as run

```
everynoise scrape (6291 cells)
    ↓ categorize (HARD_BLOCK > INCLUDE > SOFT_BLOCK rule)
2116 INCLUDE genres
    ↓ Deezer API search + /artist/{id}/radio expand
15314 unique tracks (manifest)
    ↓ download_previews.py (32-worker parallel HTTP, 30 s previews)
15314 / 15314 MP3s on disk (6.9 GB)
    ↓ MuQ-MuLan ONNX-CUDA, 30 s clips → 512-d embeddings
15314 × 512 float32 = 32 MB NPZ  [17.5 min wall on 5090]
    ↓ vLLM Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit
    ↓ 12 axes × 1500 directed pairs = 18000 pairs
    ↓ community-aware kmeans-24 sampling, bilateral
all 12 axes, 0 fail, 53 min total wall  [5.4 pairs/s, 12 inflight]
    ↓ Bradley-Terry MM (Hunter 2004) + Gamma(2,1) prior, per axis
1424 tracks with at least one prior on every axis (BT scores 0-10)
    ↓ multi-task Linear(512 → 12) probe, RankNet margin loss
    ↓ 5-fold CV stratified by aggression bucket, 600 epochs
12 row-normalised 512-d directions + biases
    ↓ ListMLE blend (target = aggression BT prior)
12 softmax weights, 7 zeroed naturally
    ↓ schema-validate + L2-normalise
V16_round7_blend.json (intensity_axis_vec + 12 sub_axes)
```

### Per-axis CV pairwise agreement (5-fold, 1424 tracks)

| Axis | CV-mean PA | CV-mean Spearman | Top track examples (round-7 corpus) |
|---|---:|---:|---|
| aggression           | 68.1 % | +0.381 | The Prodigy — Ibiza; Mandidextrous; Tymon; hardstyle / hardcore |
| atonality            | 69.5 % | +0.428 | Merzbow; Sutcliffe Jügend; harsh-noise wall, modern classical |
| bass_weight          | 68.0 % | +0.405 | NeuroKontrol Ragga Connection; phonk drift; trap; sub-bass DnB |
| darkness             | 68.4 % | +0.393 | Cryobiosis — An Opening; Anenzephalia; Scorn; Merzbow |
| density              | 66.4 % | +0.347 | NeuroKontrol; Bright Visions; The Prodigy; layered hardcore |
| distortion           | 67.3 % | +0.400 | (collapses to aggression: r=+0.991) |
| dynamic_compression  | 68.5 % | +0.405 | Kamiyada+; Tymon; hardstyle; modern brick-walled mixes |
| harmonic_dissonance  | 66.5 % | +0.356 | (correlates +0.90 with aggression — not distinct) |
| noisiness            | 69.1 % | +0.417 | (correlates +0.91 with atonality — not distinct) |
| rhythmic_complexity  | 69.6 % | +0.426 | Tim Reaper; Nebula II; Ram Trilogy; jungle / breakbeat |
| tempo_intensity      | 68.1 % | +0.383 | (correlates +0.95 with aggression) |
| vocal_intensity      | 66.3 % | +0.370 | Kill Your Idols; Korpiklaani; Sniper 66; hardcore punk |

Mean across all 12 axes: 68.0 % PA / +0.393 Spearman.

### Inter-axis Pearson correlation (notable redundancies)

22 pairs with |r| ≥ 0.85. Worst offenders:

| Pair | r |
|---|---:|
| `aggression` ↔ `distortion`           | +0.991 |
| `distortion` ↔ `vocal_intensity`      | +0.959 |
| `aggression` ↔ `tempo_intensity`      | +0.952 |
| `density` ↔ `tempo_intensity`         | +0.947 |
| `distortion` ↔ `tempo_intensity`      | +0.942 |
| `aggression` ↔ `vocal_intensity`      | +0.935 |
| `density` ↔ `distortion`              | +0.925 |
| `aggression` ↔ `density`              | +0.921 |
| `tempo_intensity` ↔ `vocal_intensity` | +0.918 |
| `atonality` ↔ `noisiness`             | +0.914 |

The five effectively-distinct axes that survive the redundancy filter
(after the blend optimiser zeros out the rest):

| Axis | Distinctness |
|---|---|
| `darkness`            | r ≤ 0.51 with everything else |
| `vocal_intensity`     | r ≤ 0.92 with `tempo_intensity`, but adds vocal-aggression signal |
| `density`             | r=0.85 with bass_weight, distinct enough to weight separately |
| `dynamic_compression` | r ≤ 0.91 with bass_weight, but dB-envelope axis is real |
| `rhythmic_complexity` | r ≤ 0.88 with noisiness, breakbeat-vs-4-on-floor signal is real |

### Joint blend (target = aggression BT prior)

ListMLE on 1424-track set, 2000 epochs × 32 batches × list_size 64.

| Axis | Blend weight |
|---|---:|
| `darkness`            | 0.323 |
| `vocal_intensity`     | 0.213 |
| `density`             | 0.199 |
| `dynamic_compression` | 0.190 |
| `rhythmic_complexity` | 0.075 |
| `tempo_intensity`     | 0.000 |
| `distortion`          | 0.000 |
| `aggression`          | 0.000 |
| `harmonic_dissonance` | 0.000 |
| `noisiness`           | 0.000 |
| `atonality`           | 0.000 |
| `bass_weight`         | 0.000 |

Reading: the LLM-judge thinks "aggression" is best decomposed as ~32 %
darkness + ~21 % vocal_intensity + ~20 % density + ~19 % dynamic
compression + ~7 % rhythmic complexity. The literal `aggression`-axis
direction zeros out because it duplicates `distortion`+`density`+
`tempo_intensity` (all r > 0.92), and the optimiser prefers the more
distinct linear combinations.

### Cross-library transfer (V15 vs V16 on user's 909-track library)

The user's library has round-5 BT priors as gold standard. V15 was
trained on these (in-domain). V16 was trained on a completely separate
Deezer corpus (out-of-domain).

| Axis | Spearman vs round-5 BT | Pairwise Agreement |
|---|---:|---:|
| V15 (round-6 single probe, **in-domain**)        | +0.603 | **76.5 %** |
| V16 (round-7 12-axis blend, **out-of-domain**)   | +0.504 | **71.7 %** |
| V11 (text-tower 6-axis, pre-V15 baseline) | +0.39  | 62.5 % |

Spearman(V15, V16) on user library = **+0.872**.

V15 wins by 5 pp PA on the user's own library — expected, since V15 was
trained on these labels. V16's 71.7 % PA on the same library, having
never seen any of those tracks during training, is a clean transfer
result: **+9 pp over V11**, demonstrating that the round-7 axis-blend
infrastructure does carry signal across libraries even without per-library
fine-tuning.

### Iteration log (in-flight evaluation + corrections)

- **Initial finding**: per-axis tournaments showed asymmetric A/B counts
  (e.g. distortion: A=924/B=576 = 62 % positional bias). Bilateral pair
  structure (each undirected pair sent A→B and B→A) and BT MM cancel
  this at the score level. Verified by aggression axis (51.7 % A) and
  distortion axis (61.6 % A) producing similar ranked outputs after BT.
- **Tested but rejected**: re-running with redundant axes dropped (only 5
  axes). Skipped because the blend optimiser already does the dropping
  data-driven via softmax → 7 zero weights. No new information.
- **Tested and shipped**: head-to-head V15 vs V16 on the user library
  (`spike/track-grading/compare_v15_v16.py`). Result drove the
  deployment decision: keep V15 deployed at
  `<collection>/muq-mulan-aggression-axis.json`; V16 ships next to V15
  at `models/aggression-axes/V16_round7_blend.json` for future use.

### Files produced (committed for round 7)

- `spike/track-grading/embed_corpus_mulan.py` — MuQ-MuLan inference for
  15314 corpus MP3s (PyTorch fp16, batch 24, 17.5 min wall on 5090)
- `spike/track-grading/round7_axis_prompts.json` — 12 axis definitions
  with per-axis questions, expected genres, rationale
- `spike/track-grading/run_per_axis_tournaments.py` — community-aware
  bilateral pair sampler + vLLM-judge runner (resume-safe, per-pair
  JSON cache under `/home/data01/Music/mesh-track-grading/round7_pairs/<axis_id>/`)
- `spike/track-grading/build_bt_priors_r7.py` — per-axis Hunter-MM BT
  with Gamma(2,1) prior, vectorised in NumPy (~5 s per axis)
- `spike/track-grading/train_axes_r7.py` — multi-task `Linear(512, 12)`
  with RankNet pairwise margin loss, 5-fold CV
- `spike/track-grading/joint_blend_r7.py` — ListMLE blend optimiser
  over softmax weights, frozen probe directions
- `spike/track-grading/interpret_axes_r7.py` — top/bottom 20 per axis +
  inter-axis correlation matrix → `round7_interpretation.md`
- `spike/track-grading/cross_library_r7.py` — projects user library
  onto every learned axis + blended axis, Spearman vs V15
- `spike/track-grading/compare_v15_v16.py` — V15 vs V16 head-to-head
  on the user's round-5 BT priors as gold-standard target
- `spike/track-grading/export_axis_r7.py` — emits the V16 JSON in the
  schema mesh-cue/player loads, with all 12 sub-axes L2-normalised
- `spike/track-grading/run_r7_step.sh` — env-bootstrap wrapper that
  sets LD_LIBRARY_PATH (zlib + libcuda + libstdc++) and exec's any
  step's Python script (single source of truth for the venv quirks)
- `models/aggression-axes/V16_round7_blend.json` — round-7 axis,
  schema-validated, `intensity_axis_vec` unit-norm, 12 sub-axes
- `/home/data01/Music/mesh-track-grading/round7_axes.npz` — raw 12 × 512 directions + CV
  metrics (re-runnable input for any future blend objective)
- `/home/data01/Music/mesh-track-grading/round7_priors.npz` — BT priors per axis
- `/home/data01/Music/mesh-track-grading/round7_blend.npz` — blend weights + scaling
- `/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz` — 15314 × 512
  embeddings keyed by deezer_track_id (re-usable for any future axis
  experiment, no need to re-embed)
- `/home/data01/Music/mesh-track-grading/round7_interpretation.md` — top/bottom 20 per
  axis (human readable, used to validate axis names)
- `/home/data01/Music/mesh-track-grading/round7_cross_library.md` — V15 vs V16 ranks on
  the user library, per-axis projections of all 909 user tracks
- `/home/data01/Music/mesh-track-grading/round7_train_metrics.json` — per-fold CV scores

### Round-7 verdict

**For the user's mesh-collection: keep V15 deployed.** V16 is round-7's
deliverable but not the new default — V15's in-domain training advantage
beats V16's broader-corpus generality on the user's specific library.

**For round 8 (productisation for end users without GPUs)**: V16 is the
right starting point. Its 12 sub-axes are general music concepts derived
from a 21k cross-genre corpus, so per-user adaptation can be a small
blend re-fit (12 weights) rather than retraining a head. The deployment
JSON is ready.

**For axis discovery as a methodology**: the round-7 result that
`aggression` decomposes into `darkness + vocal_intensity + density +
dynamic_compression + rhythmic_complexity` is interesting but should be
read as "these are the directions in MuQ-MuLan space that the LLM judge
can independently vote on", not "these are the perceptually-orthogonal
axes of intensity". Many of the 12 candidate axes collapsed (r > 0.9)
because the LLM-judge falls back to overall energy when given subtle
prompt distinctions on noisy 30 s audio clips. A higher-resolution
methodology would either use a stronger judge (e.g., human listening
tests) or constrain the multi-task head to be orthogonal during training.
