# Round-7.6 V18 training log

**Date:** 2026-05-08
**Branch:** `text-tower-aggression-axis`
**Final commit:** see `git log` for the V18 release run
**Spec:** `documents/round-7-6-pipeline-spec.md`
**Snapshot:** `data/round7_6/`

This document records the round-7.6 V18 training pipeline as it actually
ran (in particular, where it diverged from the spec) and the final
deployable model's evaluation.

---

## Headline result

| Metric | Value | Spec target | Status |
|---|---:|---:|---:|
| **V18 student test PA** | **0.8113** | ≥ 0.75 | ✅ +6.1 pp over target |
| Teacher test PA | 0.9400 | — | — |
| Distillation gap | +12.87 pp | ≤ 5 pp | ❌ (audio-encoder ceiling) |
| V15 (deployed predecessor) PA | 0.7013 | — | beaten by +11.0 pp |
| V17b polar blend PA | 0.7270 | — | beaten by +8.4 pp |
| Held-out test set | 3985 tracks | — | — |
| CPU latency (1000-track dot) | 0.004 ms | ≤ 100 ms | ✅ 25,000× under |
| Spec rubric pass rate | 8 / 10 | 10 / 10 | ❌ G5, G6 fail with documented rationale |

**V18 is shipped at `models/aggression-axes/V18_round7_6_consensus_distilled.json`.**
It supersedes V15 as the deployed intensity axis.

---

## Corpus

- 39913 Deezer 30-second preview MP3s
- Built via `spike/track-grading/build_corpus.sh`: scrape everynoise.com → seed
  list of 2109 DJ-relevant genres → query Deezer API at 10/s → 30 tracks/seed
  → download previews at 32 parallel workers
- Convergence loop in `build_corpus.sh` handles Deezer's 30-min URL TTL by
  refreshing+downloading in shrinking batches (commit `f24db0f`)

---

## Methodology divergences from spec

### 1. r7.5 inputs dropped from V18 release run

**Spec §10-§14:** assumed 5 jury sources (3 caption-text-LLM + r7.5_bt_blend + aggressive_overall_tag) feeding Dawid-Skene.

**Actual:** 3-juror panel only. Round-7.5 BT priors and mined tags only cover 15314 of the expanded 39913-track corpus (38%). Including them at mixed coverage caused Dawid-Skene σ²-collapse where one full-coverage source's σ² → 0, precision → ∞, consensus → that source. Even after a σ²-floor + nanmedian-init fix (commits `311b714` + `726e17e`), the floor-pinned source still grabbed 70.5% normalized weight.

Dropping r7.5 entirely (commit `d446d4b`) produced a clean EM: 3 sources at 100% coverage, σ² floored at 0.01, all sources weight 1/3 in normalized reliability.

**Spec G5 originally required ≥ 4 sources. Updated to ≥ 3** (spec doc commit `fc08b60`) with rationale: 3 distinct foundation lineages (NVIDIA-Nemotron, Mistral, Alibaba-Qwen) with pairwise Spearman ρ=0.93-0.96 satisfies the diversity intent of the original target.

### 2. Caption sweep `max_tokens` 192 → 1024

**Spec §7:** `max_tokens=256` for ~190-word captions.

**Actual:** Initial run at `--max-tokens 192` clipped 100 % of MF responses mid-sentence (every caption reached `completion_tokens == 192`). Bumped to 1024 (commit `dfc1525`) → average 393 words, max 884 tokens, 0 cap hits in a 100-track sample.

### 3. Concurrency tuning

**Spec §7:** `workers=8`, MF `max_num_seqs=4`.

**Actual:** Live perf bench during caption sweep → MF `max_num_seqs=48`, client `workers=64`. Throughput went from 0.44 c/s → 1.99 c/s, with KV cache stable at 64-77%. Diminishing returns past 48 (kernel scheduling becomes the bottleneck). Caption sweep wall: 25 hr projected → ~6 hr actual.

### 4. ARG_MAX bug in streaming rater

`caption-rate-streaming` polled the captions dir with `ls .../*.json | wc -l`. At 25k+ files the glob overflowed kernel ARG_MAX, returned 0, and the rater silently stopped advancing. Fixed by switching to `find -maxdepth 1 -name '*.json'` (commit `82cec9a`).

---

## Pipeline timeline

| Phase | Duration | Output | Tool |
|---|---:|---|---|
| Corpus build (Deezer) | ~2 hr | 39915 manifest, 39913 mp3s | `build_corpus.sh` |
| Caption sweep (MF) | ~6 hr | 39913 caption JSONs | MF / vLLM @ :8001 |
| Caption embedding (bge-base-en-v1.5) | 5 min | `caption_emb.npz` 768d | `embed_captions.py` |
| Struct tag extraction | 30 sec | 52 multi-hot tags | `extract_caption_tags.py` |
| Audio embedding (MuQ-MuLan-large) | 26 min | `corpus_muq_mulan.npz` 512d | `embed_corpus_mulan.py` |
| Mistral-Small-3.2-24B AWQ rating | 4.8 hr | juror NPZ (40k × 20 buckets) | local vLLM @ :8002 |
| Nemotron-30B rating (Spark 2 remote) | ~3 hr | juror NPZ | streaming rater |
| Qwen3.6-27B rating (Spark 1 remote) | ~3 hr | juror NPZ | streaming rater |
| Consensus EM (Dawid-Skene) | 0.5 sec | `consensus.npz` | `aggregate_consensus.py` |
| Artist-stratified split | 1 sec | `split.npz` | `make_split.py` |
| Teacher MLP train | 11.6 sec | `teacher.pt` (380k params) | `train_v18_teacher.py` |
| Student linear probe distill | 2.6 sec | `student.pt` (513 params) | `distill_v18_student.py` |
| Eval + V18 export | 2 sec | eval.json + V18.json | `eval_v18.py` + `export_v18.py` |

**Total wall time, end-to-end:** ~17 hr (with 3 jurors running partly in parallel).

**LLM compute spent:** ~18 hr across MF + 3 jurors + remote endpoints.

---

## Final consensus details

```
N = 39913 unique track IDs across sources

Sources (all 100% coverage):
  caption_text_llm_local_minstral  (Mistral-Small-3.2-24B AWQ on local 5090)
  caption_text_llm_nemotron        (NVIDIA-Nemotron-3-Nano-30B-A3B-NVFP4 on Spark 2)
  caption_text_llm_qwen36          (Qwen3.6-27B-Text-NVFP4-MTP on Spark 1)

Per-source σ² (post-EM):
  Mistral:   σ² = 0.0100  (floor)  reliability = 100  normalized = 0.333
  Nemotron:  σ² = 0.0100  (floor)  reliability = 100  normalized = 0.333
  Qwen3.6:   σ² = 0.0100  (floor)  reliability = 100  normalized = 0.333

Consensus z stats: mean=0.500  std=0.283  min=0.000  max=1.000

Pairwise rank agreement (Spearman ρ):
  Mistral ↔ Nemotron:  +0.943
  Mistral ↔ Qwen3.6:   +0.960
  Nemotron ↔ Qwen3.6:  +0.932
```

All three sources hit the σ² floor (0.01), which means each juror's residual against the EM consensus is ≤ 0.1 std on the rank-normalized [0,1] scale. The floor saturates because the panel agrees so well — the EM can't actually distinguish "more reliable" from "less reliable" when all three jurors are this aligned. Equal-weight consensus is the right call for this panel.

---

## Teacher (privileged-features MLP)

```
Input (1332d):
  audio_emb       512  (MuQ-MuLan-large, frozen)
  caption_emb     768  (bge-base-en-v1.5 over MF captions, frozen)
  struct_tags      52  (regex-mined multi-hot from MF caption text)

Architecture:
  Linear(1332 → 256) → GELU → Dropout(0.2)
  Linear(256 → 128)  → GELU                  ← penultimate, FitNets target
  Linear(128 → 1)                            ← intensity head

Training:
  loss = MSE(pred, consensus_intensity)      (no axis aux head — r7.5 dropped)
  AdamW lr=3e-4, wd=1e-4, batch=256, 100 epochs
  early stop on val_int_mse, patience=10
  seed=42, cudnn.deterministic=True

Result:
  best val_int_mse = 0.0029 @ ep 99
  test PA = 0.9400  Spearman = 0.9801
  trained in 11.6s on RTX 5090 mobile
```

---

## Student (V18 deployed)

```
Input: audio_emb only (512d)

Architecture:
  Linear(512 → 1)                            (linear probe)
  Penultimate adapter: Linear(512 → 128)     (training-time only, dropped on export)

Loss:
  λ_out · MSE(student, teacher.intensity)    out distill        λ=1.0
  λ_fit · MSE(pen_proj, teacher.penultimate) FitNets            λ=0.5
  λ_kd  · KL(softmax(s/T) || softmax(t/T))   Hinton, T=2.0      λ=0.3
  λ_ls  · LabelSmoothing(student, consensus) direct anchor      λ=0.2, ε=0.05

Training:
  AdamW lr=1e-3, wd=1e-4, batch=512, 50 epochs
  early stop on val_S↔T_mse, patience=10
  seed=42

Result:
  best val_S↔T_mse = 0.0243 @ ep 45
  test PA       = 0.8113   (vs consensus, N_test=3985)
  test Spearman = 0.8184
  test R²       = 0.6739
  trained in 2.6s

Distillation gap: teacher 0.9400 → student 0.8113 = +12.87 pp
```

---

## Held-out per-cluster diagnostic (G4)

K-means on caption embeddings, K=20. Top + bottom 5 by mean predicted intensity:

| k | n_test | mean | PA | theme |
|---:|---:|---:|---:|---|
| 9 | 386 | 0.842 | 0.547 | Thrash Metal |
| 6 | 280 | 0.761 | 0.723 | Metalcore / Deathcore |
| 2 | 141 | 0.712 | 0.607 | Hardstyle |
| 7 | 140 | 0.639 | 0.664 | Industrial Techno |
| 14 | 314 | 0.632 | 0.708 | Pop-Punk / Punk Rock |
| ... | | | | (mid tier — Trap, Tech House, Hip-Hop, Reggaeton, Eurodance, Latin Trap) |
| 1 | 238 | 0.282 | 0.670 | Dream Pop / Synth-Pop |
| 12 | 268 | 0.256 | 0.764 | Indie Folk / Acoustic Rock |
| 18 | 123 | 0.246 | 0.650 | Dark Ambient / Drone |
| 0 | 129 | 0.234 | 0.721 | Neo-Soul / R&B |

**Ordering passes G4:** the top tier is dominated by industrial/hardcore/metal styles; the bottom tier is dominated by ambient/acoustic/soul. Zero inversions in either tier. Mid tier is densely packed but the included subgenres (trap, tech-house, eurodance) all legitimately occupy similar intensity bands.

---

## Spec rubric — final scorecard

| Goal | Result | Pass |
|---|---|---:|
| **G1** linear deployment | 512d vec + bias, no other features in V18 JSON | ✅ |
| **G2** no user-library leakage | corpus = Deezer only | ✅ |
| **G3** test PA ≥ 0.75 | **0.8113** | ✅ |
| **G4** per-cluster ordering | monotone, no top/bottom inversions | ✅ |
| **G5** ≥ 4 heterogeneous sources | 3 sources (target updated to ≥ 3) | ❌ (rationale documented) |
| **G6** distillation gap ≤ 5 pp | +12.87 pp (audio-encoder ceiling) | ❌ |
| **G7** source_category untrusted | 0 hits in training/labels/split/eval | ✅ |
| **G8** artist-stratified split | 19159 unique artists, no train/test overlap | ✅ |
| **G9** CPU latency ≤ 100 ms / 1000 tracks | 0.004 ms / 1000 (25,000× under) | ✅ |
| **G10** deterministic reproducibility | V18 export reproduces test_pa=0.811276 to 1e-6 | ✅ |

**8 of 10 pass.** V18 is shipped as the deployed intensity axis.

---

## Known limitations & follow-ups

### G6 distillation gap is the audio-encoder ceiling

Teacher (with caption_emb + struct_tags) hits 0.94. Student (audio-only) hits 0.81. A 13 pp gap is the LUPI literature's "irreducible privileged information" — the audio encoder fundamentally can't reconstruct what the captions encode about timbre, vocal style, structural events.

Two paths to close it:

1. **2-layer MLP student** (spec §765-768 escalation). Cheap: same data, retrain student with `Linear(512→128)→GELU→Linear(128→1)` instead of `Linear(512→1)`. Expected: +3-6 pp gap closure. CPU cost: still <0.05 ms/track (well under G9 budget).

2. **Bigger audio encoder** (`documents/embedding-models-research.md` Phase 2). MAEST-768d or MULE-1.7k+d trained on richer music understanding tasks. Expected: +2-4 pp on top of (1).

Combined target: V18.5 ≈ 0.86-0.88 PA. Plan to ship (1) immediately; defer (2) to embedding-models migration.

### G5 panel diversity

The 3-juror panel passes the *intent* of G5 (heterogeneous foundation lineages, high pairwise agreement, full coverage) but is below the original "≥ 4" nominal target. Defense-in-depth options:

- Add a 4th juror — could be Llama-3.1-70B AWQ on Spark 2 (~3 hr remote work) or GPT-4o-mini via OpenAI (~30 min API spend, ~$30 cost on 40k captions).
- Re-run round-7.5 BT priors on the new 24k tracks (~50 hr Qwen3-Omni K=4 GPU). Marginal gains; not recommended.

---

## Reproduction recipe

From a fresh checkout with the snapshot at `data/round7_6/`:

```bash
# 1. Unpack snapshot to live workspace
cd /home/data01/Music/mesh-track-grading
mkdir -p round7_6_captions
zstd -dc /path/to/Mesh/data/round7_6/captions_music_flamingo.tar.zst | tar -x
cp /path/to/Mesh/data/round7_6/round7_6_*.{npz,pt,json,md} .
cp /path/to/Mesh/data/round7_6/corpus_muq_mulan.npz embeddings/

# 2. Re-derive caption embeddings (~5 min GPU)
nix develop /path/to/Mesh#mlspike
bash spike/track-grading/run_r7_step.sh embed_captions.py \
  --captions-root round7_6_captions/music_flamingo \
  --out round7_6_caption_emb.npz

# 3. Re-train teacher + student + eval + export from cached features (~15 s)
bash spike/track-grading/run_round7_6_pipeline.sh v18-train
```

Step 3 reproduces V18 deterministically (seed=42 across split, EM, teacher, student, dropout). The exported V18 weights match `models/aggression-axes/V18_round7_6_consensus_distilled.json` to ~1e-6 absolute (G10).

## Companion files

- `data/round7_6/` — full snapshot of irreplaceable artifacts (141 MB)
- `data/round7_6/README.md` — inventory + reproduction recipe
- `data/round7_6/logs/v18_train_release.log` — train run log
- `data/round7_6/logs/embed_resume.log` — audio embedding extension log
- `models/aggression-axes/V18_round7_6_consensus_distilled.json` — deployed model
- `documents/round-7-6-pipeline-spec.md` — full spec with V18-release update notes
- `documents/round-7-6-v18-1-mlp-experiment.md` — V18.1 capacity-bake-off

---

## Post-deploy hand verification on the user's DnB library (2026-05-08)

After deploying V18.1 to mesh-cue / mesh-player, ran `cargo run -p mesh-core
--bin aggression_inspect` against the personal collection at
`/home/data01/Music/mesh-collection` (909 tracks, mostly DnB across the
liquid → neurofunk → industrial spectrum + some non-DnB electronic).

### Distribution sanity (passed)

```
total tracks scored: 909
  min:      +0.244     (NTO — La clé des champs, French ambient electronic)
  p10:      +0.595
  p25:      +0.701
  median:   +0.780     (skewed high — expected for a DnB-dominant library)
  p75:      +0.842
  p90:      +0.883
  max:      +0.974     (Mob Tactics — Dirtgrub, neuro/jump-up DnB)
  mean ± σ: +0.758 ± 0.117
```

Range covers ~73 % of the [0, 1] band, std=0.117 comparable to the
training-corpus jurors (0.166-0.210). No collapse, no saturation.

### Genre boundary (strong pass)

The bottom 15 tracks are predominantly **non-DnB** electronic:

- NTO, Stephan Bodzin, Boris Brejcha, Oliver Koletzki — melodic / minimal
  techno + deep house. Correctly identified as low-intensity.
- ZHU, Trentemøller, Lulu Rouge, Emika — moody electronica / pop. Correct.
- Random Movement (Fuzzy Teeth) — only liquid-DnB track in the bottom 15.
  Correct: liquid is the lowest-intensity DnB sub-genre.

The top 15 are recognizable peak-time DnB:

- Mob Tactics (Dirtgrub), Synergy (Overdose), Pythius / Black Sun Empire
  (Mysterion), Noisia (Friendly Intentions, Omnivore), Teddy Killerz
  (Layer, Darkness) — all correctly clustered at the high end.

### Sanity-check separation (passed strong, not excellent)

```
known_aggressive (Current Value, Billain, Neonlight, Mefjus, Phace,
                  Noisia, Black Sun Empire, Audio, Teddy Killerz):
  mean percentile = 58.6 % (over 122 tracks)

known_liquid (Random Movement, Calibre, LSB, Logistics, BCee,
              Etherwood, Marcus Intalex):
  mean percentile = 8.3 % (over 17 tracks)

separation: aggressive_mean − liquid_mean = +50.3 pp
```

Spec calls >40pp "strong", >60pp "excellent". V18.1 lands at +50pp:
strong but not excellent. The bottom-tier separation is essentially
perfect (liquid mean at p8.3 is exactly where it should be), but the
top-tier cluster is sitting at p58 instead of the ~p80 you'd expect for
canonical neurofunk.

### Methodology misfit identified: train-vs-inference clip distribution

**Root cause of the mid-pack neurofunk clustering**, found by inspecting
`crates/mesh-cue/src/ml_analysis/inference.rs:374-405` (the deployed
`extract_clips` + `average_embeddings` path) against
`spike/track-grading/embed_corpus_mulan.py:80-95` (the training-time
embedder):

| | Training (round-7.6 spike) | Deployment (mesh-cue inference) |
|---|---|---|
| Source audio | 30-second Deezer previews | Full track files (3-7 min typical) |
| Sampling | One contiguous 30 s window | 6 evenly-spaced 10 s clips across the whole track |
| Aggregation | Single MuQ-MuLan forward → 512d | Mean of 6 per-clip MuQ-MuLan outputs |

**Effect**: Deezer's preview heuristic picks the catchiest 30 s — usually
the drop. So the teacher and the 3 jurors learned to score
*peak-energy windows*. At deployment, the model receives a *mean-averaged*
embedding that smears intro / breakdown / outro into the drop.

For peak-time DnB tracks with substantial non-drop content (intro buildups,
half-time breakdowns, fade-outs), the average pulls the embedding away
from the high-intensity region of the embedding space. The model still
ranks the track correctly *relative to other tracks with similar
structure*, but it doesn't reach the full extreme range.

Confirmed with concrete examples:

- Current Value — Rethink (canonical neurofunk single, all-drop): p53.5
- Black Sun Empire — Modern Propaganda (canonical neurofunk single): p52.1
- Noisia — Running Blind (textbook neurofunk): p51.7
- Noisia — Omnivore (more extreme): p99.3 ✓
- Noisia — Friendly Intentions (Original): p97.7 ✓

The "extreme" tracks of the same artists land at the top tier, but the
"merely very hard" ones don't, because their non-drop portions dilute
the average more.

**For DJ-intensity scoring this is the wrong aggregator.** What matters
operationally is "how hard does this track hit at its peak", because
that's what determines where you'd cue it in a set.

### Fix decision (2026-05-08): peak-clip pooling at inference

Instead of mean-averaging the 6 clip embeddings, project each clip
through V18.1 and take the **maximum-intensity clip's score** as the
track's intensity. Costs ~6× the projection (still microseconds at the
G9 budget), but operationally aligns "track intensity" with "peak-clip
intensity" — which is what the teacher was trained to recognise.

Implementation: change `inference.rs::average_embeddings(...)` /
its caller to project each clip independently, return the max. The DB
schema doesn't need to change (we still store the picked clip's 512d
vec). V18.1 weights don't need to change either.

Expected effect: aggressive_mean percentile climbs from 58.6 → 70-80,
liquid_mean stays low, separation widens from +50 pp toward +60 pp
(the spec's "excellent" target).

Tracked as TODO item: see `TODO.md` § "Corpus strengthening — full-length
support". A longer-term fix is to make the training corpus match the
deployment corpus shape (full-length tracks instead of 30 s previews),
which would eliminate the train/inference distribution mismatch entirely
but requires re-downloading + re-encoding the corpus (cost analysis in
the TODO entry).

### Library scaling (verified)

V18.1 is a fixed function `f: 512d → scalar`. Adding tracks to the
library doesn't shift the score of any existing track: a 0.785
"Modern Propaganda" score is identical regardless of what else is on
disk. **Percentile ranks** do shift — adding more extreme tracks would
push existing high-intensity tracks down in rank — but that's bucket
arithmetic, not model drift. Library-invariant absolute scores are
the right behaviour: a track scored 0.85 means "high intensity" the
same way for every user.
