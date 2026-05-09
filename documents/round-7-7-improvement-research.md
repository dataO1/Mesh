# Round-7.7 — Intensity Axis Improvement Research

**Date:** 2026-05-09
**Branch:** `text-tower-aggression-axis`
**Status:** Final (post-review)
**Predecessors:** `round-7-6-pipeline-spec.md`, `round-7-6-training-log.md`,
`round-7-6-v18-1-mlp-experiment.md`
**Working draft (with critique annotations):** `round-7-7-improvement-research-DRAFT.md`

This document enumerates and reasons about every plausible avenue for
improving the deployed intensity-axis pipeline beyond the current
**V18.1 MLP + peak-clip-pool** baseline. Each suggestion includes a
hypothesis, a causal chain explaining why it should help, an
expected-gain estimate, cost, risks, and self-critique.

A draft of this doc was reviewed by an independent assistant. **Specific
sections of the original draft were factually wrong** — most importantly
B3 (Music Flamingo direct rating) was designed against the wrong
calibration mode for MF, and the σ²-floor consensus saturation was
under-described as a virtue when it's actually evidence of a degenerate
panel. The reviewer's critique is folded into every section below.

The final ranked recommendation in §11 is the reviewer's re-ranking,
not the draft's.

---

## 0. Current baseline

| Component | Current state |
|---|---|
| Corpus | 39 913 Deezer 30 s previews, scraped via everynoise (~2 100 DJ-relevant seed genres × 30 tracks/seed) |
| Audio encoder | **MuQ-MuLan-large 512d** (frozen, music-text contrastive pretraining, 700M params) |
| Caption gen | **Music Flamingo 7B** (NVIDIA), `T=0.7, top_p=0.9, max_tokens=1024`, ~393 words avg, **1 caption per track** |
| Caption embedder | bge-base-en-v1.5 (768d) |
| Caption struct tags | ~50 regex-mined multi-hot tags |
| Jurors | **3** text-LLM (Mistral-Small-3.2-24B AWQ, Nemotron-30B, Qwen3.6-27B), 20-bucket two-token logprob recovery |
| Pairwise juror agreement | Spearman ρ=0.93-0.96 — **σ²-floor degenerate**, see below |
| Consensus | Continuous Dawid-Skene EM, σ²-floor=0.01, nanmedian-init → all jurors weighted **equally (1/3)** because no juror's residuals fall below the floor |
| Teacher | MLP `1332 → 256 → 128 → 1`, MSE on consensus, **PA = 0.94** on 3985 held-out |
| Student (V18.1) | MLP `512 → 128 → 1` over MuQ-MuLan, FitNets + Hinton + LS distillation, **PA = 0.8174** |
| Distillation gap | **+12.87 pp** |
| Library hand-eval | **+55.8 pp** aggressive_mean − liquid_mean separation on 909-track DnB library |
| Inference embedding | 6 × 10 s evenly-spaced clips, project each through V18.1, take peak |
| Inference compute | ~1.4 µs/track for V18.1; MuQ-MuLan ONNX ~1.7 s dominant cost |

### 0.1 Critical baseline observations (revised after review)

**(a) The σ²-floor saturation is the biggest methodological issue.**
All 3 jurors hit σ² = 0.01 (the floor) ⇒ Dawid-Skene **cannot
distinguish their reliabilities**. The "DS reliability scores 0.333
each" is artifactual, not a finding about the jurors. With pairwise
ρ = 0.93-0.96, the panel is effectively **one juror's vote with 3×
the false-confidence inflation**. Reporting "consensus from a 3-juror
DS panel" gives a misleading sense of robustness. The right re-frame:
*we have one effective independent vote*. Every "add a juror"
suggestion should be valued by how much that juror **breaks** σ²-floor
saturation (i.e., how genuinely uncorrelated it is with the existing
panel).

**(b) Train-vs-deploy clip distribution mismatch — clarified.**
- **Training**: `MuQMuLan.from_pretrained()` over the 30 s Deezer
  preview internally splits into **3 × 10 s contiguous non-overlapping
  clips** and averages (per `muq/muq_mulan/muq_mulan.py`). Deezer's
  preview heuristic typically picks the catchiest 30 s of the track,
  so the training distribution is "3-clip mean of a drop-rich 30 s
  window".
- **Deployment** (post 2026-05-08, `inference.rs:309-326`): pick **1
  of 6 evenly-spaced** 10 s clips by V18.1 score; discard the other 5.
- The deploy distribution is *more peaky* than training was.
  V18.1's weights were learned on 3-clip-mean inputs but inference
  delivers single-clip-pick inputs.

**(c) Hand-eval is single-genre, single-user.** The +55.8 pp library
separation is on 909 DnB tracks for one user. The 39 913-track training
corpus spans ~2 100 genre seeds, so training is broad — but we have
**no evaluation evidence** for tech-house, ambient, hyperpop, hip-hop,
or any non-DnB DJ-relevant collection. Before optimizing further we
need at least one non-DnB hand-eval baseline.

**(d) MF licensing is an existential risk for shipping.**
Per `judges/music_flamingo.py:18-21`, MF is licensed under NVIDIA
OneWay Noncommercial Academic — research use only. V18 weights
distill through MF-derived consensus (captions → consensus → student).
Whether V18 weights are a **derivative work** of MF outputs is a
legal question, not an engineering one. The current spec says "MF
caption used at training time only; deployed weights are linear
over MuQ-MuLan; user-redistributable subject to MuQ-MuLan license"
but this hasn't been legally vetted. **Action:** get a legal read
before any commercial release of V18.1; alternatively, scope an
MF-free training pipeline as the user-facing model.

---

## 1. Inference-time changes (no retraining)

These are the cheapest improvements: deploy-side code only.

### A1. Realign deploy clip strategy with training

**Hypothesis.** Replace 1-of-6-peak with: (1) score 6 evenly-spaced
10 s clips, (2) find the highest-scoring contiguous 30 s window (the
3 adjacent clips with maximum sum-intensity), (3) mean those 3 clip
embeddings, (4) project through V18.1.

**Causal chain.** V18.1's weights were learned on 3-clip-mean inputs.
Single-clip pick is brittle: if the chosen clip lands on a tom fill,
a vocal hook, or a brief breakdown inside the drop, we get a non-
representative embedding. The 3-clip mean of the densest 30 s
recovers exactly what training saw and is robust to within-30 s
variation.

**Expected gain.** **+0-2 pp library separation.** Lower than the
draft estimated because the per-track variance from clip-pick is
not the dominant error source — the encoder's representation of
intensity is. (The draft said +1-3 pp; corrected after self-
critique.)

**Cost.** ~30 lines in `inference.rs::analyze` + a re-analysis pass
on the user's library (~5 min wall on 909 tracks).

**Risks.**
- On long tracks where the catchy 30 s isn't the actual peak (e.g.,
  tracks with a 30 s ambient passage followed by a 10 s drop), the
  contiguous-window heuristic might pick the longer ambient.
- Slightly more compute per track (still microseconds).

**Prerequisites.** **A4 (decoupled intensity vs similarity
embeddings)** must land first. Without A4, A1's peak-region
embedding becomes the same vector that powers similarity / PCA,
shifting similarity semantics from "tracks with similar overall arc"
to "tracks with similar drops" — which may not be what we want for
DJ track-suggestion.

**Self-critique.** Worth doing. Cheap, principled, easy to revert.
Pairs cleanly with A4.

### A2. Top-k peak pooling instead of single peak

**Hypothesis.** Score 6 clips; mean the embeddings of the top-K by
intensity score; project. K=3 calibrated.

**Verdict (revised).** **Subsumed by A1** (K=3 over 6 evenly-spaced
clips ≈ contiguous 30 s window when peaks cluster). Drop as a
separate item.

### A3. Anchor-relative percentile display

**Hypothesis.** Add a thin display-layer wrapper that maps raw V18.1
scores to percentiles **of a hand-picked anchor set** (~30 tracks
spanning the spectrum) rather than percentiles of the live library.
"p80" then means "as intense as the 80th percentile of canonical
references" instead of "as intense as 80 % of *this user's* library".

**Causal chain.** A DnB-only library reads as "80 % are high
intensity" because the whole library is in the high tail. Anchor-
relative percentiles read as "this DnB library's median is at p70
of the global scale" — which matches a DJ's mental model.

**Expected gain.** Doesn't move PA. **UX-defining win** for users
with stylistically-narrow libraries (i.e., most users).

**Cost.** ~50 lines of Rust.

**Risks.**
- **Licensing (CRITICAL).** Cannot ship Deezer-derived audio
  embeddings — Deezer audio is proprietary. **Three options:**
  (a) anchor tracks with redistribution-licensed audio (CC-licensed,
  Free Music Archive, MTG-Jamendo subset), (b) ship **only** the
  V18.1 scalar score per anchor (one float per anchor), (c) re-derive
  anchors per-user from a deterministic hash of the user's library.
  Option (b) is simplest: ship a 30-element table of `{anchor_name,
  scalar_score}` and let mesh-cue/player do percentile lookup.
- Anchor selection is opinionated.

**Self-critique.** This is one of the highest-leverage moves for
*user experience* even though it's PA-neutral. Combined with
options-(b) above, it ships in days. Move higher in priority than
draft listed.

### A4. Decouple intensity vs similarity embeddings (PREREQUISITE FOR A1)

**Hypothesis.** Store **two** 512d vectors per track in the DB:
(a) **peak-region embedding** (A1's output) for intensity scoring;
(b) **mean-of-all-clips embedding** for similarity / PCA / clustering.
Intensity head reads (a); similarity index reads (b).

**Causal chain.** Currently both downstream uses share one 512d.
A1 will shift that vector toward "drop semantics" — improving
intensity but possibly degrading "similar overall vibe" similarity.
A4 surfaces both representations so each downstream gets the right
geometry.

**Expected gain.** 0 PA. Enables A1 to land without similarity
regression.

**Cost.** New DB column, second pooling pass at analysis time, ~50
lines of Rust. Re-analysis pass.

**Risks.** Existing rows need re-analysis to fill the new column.
Slightly more storage (~4 MB extra at 909 tracks; ~40 MB at 10k).

**Self-critique.** Strict prerequisite for A1. Architectural cleanup
that we'd want anyway.

### A5. Energy-prefiltered clip selection

**Hypothesis.** Before MuQ-MuLan, compute cheap energy stats (RMS,
spectral flux, kick-band energy) on a sliding window across the full
track. Centre the 6 clips on the **top-6 energy peaks**, not on
evenly-spaced positions.

**Causal chain.** Even spacing can miss a short drop in a long
track. Energy-based pre-selection guarantees the drop is sampled.

**Expected gain.** +1-3 pp library separation, mostly on tracks with
long quiet intros (jungle, post-rock-style buildups).

**Cost.** ~50 lines of Rust + librosa-equivalent. RMS and onset
density are cheap from existing mel data.

**Risks.** Energy ≠ intensity (loud lo-fi distorted intros, busy
hi-hats in non-drop sections). Needs care in feature selection
(kick-band 50-100 Hz is a better proxy than broadband RMS).

**Self-critique.** Combine with A1: use energy peaks as clip *centres*
(replacing even spacing), then proceed with A1's contiguous-30 s-
window selection. Solid mid-priority item.

---

## 2. Caption-side improvements

### B1. Caption prompt redesign for intensity-relevant aspects

**Hypothesis.** Rewrite the MF prompt to elicit information specifically
useful for intensity discrimination: drop architecture, bass weight,
distortion character, vocal aggression, BPM perception, peak-time
suitability. Current prompt is generic.

**Specific prompt sketch:**
```
You are a careful music analyst. Describe the audio specifically
for DJ-set intensity assessment. Cover all of:
1. Genre and sub-genre
2. Bass character (sub, reece, distorted) and weight
3. Drum/percussion intensity, kick character, density
4. Distortion, saturation, harshness in the mix
5. Vocal style if any (clean, harsh, sampled, shouted)
6. Drop architecture: when does the energy peak, how dense
7. Compression / mastering loudness
8. BPM range estimate
9. Mood: dark, uplifting, menacing, melancholic, euphoric
10. Closest comparable artists or sub-genre tag
Write 200-400 words. Use specific musical vocabulary.
```

**Expected gain.** **+0.5-2 pp student PA** (revised down from draft's
+2-5 pp). The student is bottlenecked by the audio encoder, not by
caption quality. A sharper teacher (~+2-3 pp on teacher PA) cascades
to ~+0.5-2 pp on student PA after the +12.87 pp irreducible gap.

**Cost.** **10-15 hr MF re-run** (revised up from 6 hr — list-format
prompts produce longer outputs at ~1.5-2× the per-call wall) + ~9 hr
juror compute (3 jurors in parallel) + 30 s consensus + teacher +
student.

**Risks.**
- Structured prompts can cause MF to template-fill (e.g., "no
  distortion present" rather than skipping). Validate on a 100-track
  sample before the full sweep.
- Longer outputs → more tokens → slower MF.

**Self-critique.** Strong mid-priority item. Better captions help
the *teacher* substantially, but the student gain is bounded by the
encoder ceiling. Combine with B6 (caption stability check) before
deciding.

### B2. Multiple captions per track (T-ensemble)

**Hypothesis.** Generate 3 captions per track at T=0.7, embed each,
mean the bge embeddings; score each caption with each juror, mean
the 3 scores per juror.

**Verdict.** **Gated on B6** (caption stability check). If B6 measures
ρ > 0.90 between paired re-captioning runs, B2 adds nothing. If B6
measures ρ < 0.80, B2 becomes high-value. **Don't run B2 blind.**

### B3. MF-as-juror via existing Likert path (REWRITTEN AFTER REVIEW)

**Original draft was wrong.** I proposed using MF for direct 20-bucket
intensity rating via two-token logprobs. But `judges/music_flamingo.py:226-235`
explicitly documents:

> Music Flamingo was trained on captions + MCQ + classification, NOT
> scalar rating, so a "0-100 integer" prompt at T=0 collapses to
> the modal "50" token on subjective axes. Replacing the ask with
> "pick one of {1,2,3,4,5}" puts the model in its strong
> categorical mode

The pipeline spec also references *"Mesh — Music Flamingo Pointwise
Findings — why pointwise rating fails"* as a primary doc.

**Corrected hypothesis.** Use the **existing 16-axis Likert path**
(`load_mf_likert_intensity` in `aggregate_consensus.py`) which already
does 5-bucket Likert with logprob recovery, summed over 8 pro-high
axes (`LIKERT_PRO_HIGH`). MF rates each (track, axis) cell on 1-5;
z-mean across the pro-high axes gives a per-track scalar; this
becomes the 4th juror.

**Causal chain.** MF sees the audio directly (not just a caption of
it). Adding it to the panel is the *most genuinely heterogeneous*
juror addition possible: a different *modality* (audio) not just a
different LLM lineage. Pairwise ρ with the text jurors should be
substantially lower than the existing 0.93-0.96, **breaking
σ²-floor saturation** and giving Dawid-Skene actual reliability
information to work with.

**Expected gain.** +2-4 pp student PA, primarily by giving the
consensus an audio-grounded signal. **Larger than any single text-
juror addition would provide**, because of (0.1.a) σ²-floor saturation.

**Cost.** **17-25 hr MF compute** (revised up from draft's 5.5 hr).
Computation: 39 913 tracks × 8 axes = 319 304 cells. At sustained
5.2 c/s (per existing benchmarks in the runbook), that's ~17 hr.
At slower throughput it's 25 hr. Plus ~30 s for consensus rerun.

**Risks.**
- MF Likert calibration on this corpus has only been smoke-tested
  on 200 tracks. Need to run a 1 000-track validation pass first
  to confirm the bucket distribution is non-degenerate before
  committing to the full 39 913.
- The MF licensing concern (§0.1.d) is now even more relevant —
  more MF involvement in training means more derivative-work
  exposure.

**Self-critique.** This is the strongest single panel-side addition
*because* of σ²-floor saturation, not despite it. Re-prioritised in
§11. The re-scoping (existing 16-axis path, not new 20-bucket prompt)
removes the "MF can't do this" objection.

### B4. Sub-genre specialist jurors

**Hypothesis.** Train (or in-context-tune) per-genre juror prompts
with anchor examples ("Black Sun Empire 'Lights Out' = 17/19 in DnB
context"). At consensus time, weight specialists by the genre-cluster
membership of the track.

**Causal chain.** General-purpose LLMs cluster genres correctly but
have weak intra-genre discrimination. A specialist with anchored
in-context examples has sharper sub-genre calibration.

**Expected gain.** +1-3 pp library separation on intra-genre ranking.
Not much on cross-genre PA (which is what test set measures).

**Cost.** Curate ~50 anchor tracks/genre × ~5 genres = 250 hand-rated
tracks (~12-25 hr human time depending on rater rigour). Then ~3-4
hr juror re-run with longer in-context prompts.

**Risks.**
- Genre detection is itself error-prone; mis-routed tracks get worse
  ratings.
- Specialists over-anchor on their examples ("Random Movement = liquid"
  even when a specific track is heavier).
- Hand-rating intersects MF licensing concerns if anchors come from
  user library (G2 leakage).

**Self-critique.** Promising but high-design-cost. Don't gate between
specialists; *blend* via soft genre-cluster weights. Treat as
**additional** juror on top of general 3, not replacement. Lower
priority than B1/B3 because the design risks are real.

### B5. Re-run round-7.5 BT priors on the new 24 599 tracks

**Verdict (revised).** Draft estimated +2-4 pp; reviewer correctly
notes this is overestimated. With current σ²-floor saturation, adding
a 4th text-derived source that probably correlates ρ ≈ 0.85+ with
the existing panel adds **~+0-1 pp** for **~50 hr GPU**. Poor ROI.

**Move down in priority.**

### B6. Caption stability sanity check

**Hypothesis.** Re-caption 50 random tracks at T=0.7, measure
caption-embedding cosine ρ between paired runs.

**Causal chain.** Pure validation. Tells us whether B2 is worth
running. Spec calls for ρ > 0.85.

**Expected gain.** Information, not PA.

**Cost.** ~5 minutes of MF + 30 sec of bge embedding.

**Self-critique.** Should have been done day 1. Trivial.

---

## 3. Juror panel improvements

### C1. Add 1-2 frontier-API jurors (RE-PRICED)

**Hypothesis.** Add jurors from **genuinely different lineages**:
- Claude Haiku 4.5 (Anthropic Constitutional AI training)
- GPT-4o-mini (OpenAI RLHF)
- Optionally: Gemini Flash (Google data)

**Why these specifically:** Llama-3.1-70B (which the draft proposed)
shares pretraining data overlap with Mistral and Qwen via Common
Crawl. The genuinely *novel-lineage* additions are GPT, Claude, and
Gemini — different RLHF and instruction data.

**Causal chain.** Adding heterogeneous jurors with low pairwise
correlation (target: ρ < 0.85 against the existing 3) **breaks
σ²-floor saturation**. Once any juror has materially different
residuals, Dawid-Skene assigns differential weights and the consensus
gets actual robustness instead of degenerate equal-weight averaging.

**Expected gain.** +1-3 pp student PA, **conditional on σ²-floor
breaking**. Combined with D2 (Snorkel) below, +2-4 pp.

**Cost.** **~$5 GPT-4o-mini + ~$15 Claude Haiku** (revised down
from draft's $80 — 30M input tokens × $0.15/M ≈ $4.50 for GPT-4o-
mini; Claude Haiku 4.5 ≈ $0.80/M input). Plus ~30 min API time.

**Risks.**
- Hosted-API jurors mean ongoing cost for re-runs.
- If new jurors agree perfectly with the existing 3 (ρ > 0.95),
  gain is 0 — but this is unlikely given the lineage diversity.
- Privacy: captions don't contain PII but do reveal which Deezer
  tracks we're rating.

**Self-critique.** Cheaper than I thought. Move *up* in priority.

### C2. Reasoning-juror with structured rubric

Subsumed by C1 if we add a reasoning model (DeepSeek-R1-Distill,
QwQ-32B). Drop as separate item.

---

## 4. Consensus / aggregation

### D1. Multi-axis consensus (intensity + complementary axes)

Round-7.5 already tried 16-axis. Results were okay but not
transformative. Don't redo unless we identify a specific axis the
single-target is missing. **Drop.**

### D2. Snorkel correlation-aware label model (PAIRED WITH C1)

**Hypothesis.** Replace continuous Dawid-Skene with Snorkel's
LabelModel that handles correlations between sources. Currently
DS assumes conditional independence given z, violated by 3 jurors
reading the same caption.

**Causal chain.** With C1 landing 1-2 new jurors, the panel becomes
4-5 sources with known correlation structure (3 caption-LLM jurors
correlated with each other, MF-Likert audio juror separate, GPT/
Claude API jurors orthogonal). Snorkel deflates the correlated trio's
joint weight, giving the heterogeneous sources room to contribute.

**Expected gain.** +0-2 pp **conditional on C1 + B3 having broken
σ²-floor saturation**. With current 3-juror saturated panel, Snorkel
produces the same equal-weight output as the floored DS.

**Cost.** ~1 day to swap in Snorkel's LabelModel call + verify outputs.

**Self-critique.** Becomes valuable *with* C1, not after. Pair them.

---

## 5. Teacher and student

### E1. Multi-task teacher with auxiliary heads (REVISED EXPECTATION)

**Hypothesis.** Teacher predicts intensity AND auxiliary targets
(BPM bin, danceability proxy, valence proxy, genre cluster). Aux
heads regularize the shared backbone, enriching the penultimate that
FitNets matches.

**Causal chain.** Predicting BPM forces the representation to encode
BPM, which is intensity-correlated within DJ genres. Student then
gains BPM-aware penultimate via FitNets.

**Expected gain.** **+0-1 pp student PA** (revised down from draft's
+1-3 pp). The student is already saturated against MuQ-MuLan-512d's
intensity signal (the V18.1 h=128/256/512 sweep showed h=512 added
+0.07 pp). Adding regularization to the teacher doesn't add new
information to the encoder the student reads. Most of the gain would
have to come via FitNets penultimate enrichment, which has a small
ceiling here.

**Prerequisites.**
- **Validate BPM detection accuracy** on a hand-rated 50-track EDM
  subset before training. Naive `librosa.beat.tempo` halves/doubles
  routinely on DnB and half-time tracks. If accuracy < 90 % on EDM,
  drop the BPM head.
- BPM extraction over 39 913 × 30 s previews at ~6 hr CPU or ~2 hr
  GPU.
- Spotify Audio Features for danceability/valence requires ISRC
  lookup over the corpus (free API, ~2 hr).

**Cost.** **2-3 hr setup + 30 s teacher rerun**, plus prerequisite
work above (8-10 hr total).

**Risks.** Bad auxiliary labels inject gradient noise.

**Self-critique.** Cheap and worth doing for *diagnostic* value
(does the encoder learn BPM-aware features?), but expect modest
direct gains. The information-bottleneck is upstream of the teacher.

### E2. Ordinal regression head — drop. Tiny gain, complication.

### E3. Quantile head (uncertainty-aware) — defer. UX, not PA.

### E4. LoRA fine-tune MuQ-MuLan (RE-COSTED — ONNX BLOCKING)

**Hypothesis.** Fine-tune MuQ-MuLan with LoRA adapters on the
consensus targets. The encoder learns intensity-discriminative
features that aren't present in its pretraining geometry.

**Causal chain.** MuQ-MuLan was trained for music-text similarity;
its embedding axis is dominated by genre/mood similarity, not
intensity. LoRA adapters re-shape the embedding so intensity
becomes a more dominant axis without erasing the music-text
geometry.

**Expected gain.** +3-6 pp student PA. The encoder ceiling is the
G6 distillation gap; this is the cheapest path to lifting it
without swapping encoders entirely.

**Cost (REVISED).** Draft said "6 hr GPU + ONNX export". Reality:
- **6 hr GPU training** (LoRA over 700M params on 39 913 tracks,
  bf16, A100/5090).
- **1-3 days re-validating the ONNX export pipeline.** Per
  TODO.md:336, MuQ-MuLan has **no official ONNX**; the existing
  `convert-muq-mulan/export.py` is custom. MuQ-MuLan uses
  `flash_conformer` layers that may not have stable ONNX
  equivalents. After LoRA, the export needs to:
  - Merge LoRA adapters into base weights
  - Re-export ONNX
  - Verify parity with PyTorch on the 3 985-track held-out set
    (target: bit-identical or PA delta ≤ 0.1 pp)
  - Regenerate the `*.norm.json` mel sidecar

**Risks.**
- Catastrophic forgetting of music-text similarity if over-trained;
  hurts similarity downstream.
- ONNX export instability — may need to keep PyTorch inference
  alongside ONNX as a fallback during validation.
- Doubles MuQ-MuLan storage if we keep base + LoRA-merged separately.

**Self-critique.** Highest-impact single audio-encoder change short
of swapping to MAEST, but the realistic cost is **1-2 weeks**, not
"a day". Properly priced this drops in priority below cheaper wins.

### E5. Encoder swap (MAEST-768d / MULE-1.7k+d)

Already in TODO and `documents/embedding-models-research.md`.
Highest expected gain (~+5-10 pp combined with everything else)
but multi-week effort. Defer to its own round (V19+).

**Note added after review:** The reviewer flagged that the
"MAEST > MuQ-MuLan for intensity" claim in the existing TODO is
**speculative**. MAEST (87M params, 768d/2304d concat) is trained
on AudioSet with patch-out spectrogram transformer; whether its
geometry is genuinely more intensity-discriminative than MuQ-MuLan
needs an actual probe-and-compare experiment, not assertion.
**Add to embedding-models-research:** before committing weeks to
MAEST migration, run a comparable linear-probe on V18 corpus
embeddings to measure the actual upgrade ceiling.

### E6. Multi-encoder ensemble

Concat MuQ-MuLan + MAEST + handcrafted features (BPM, RMS, spectral
centroid, onset density, key, tempo stability, dynamic range, LUFS).

Strong direction once MAEST migration lands. **Defer to post-E5.**

### E7. Stem-separated encoding — defer (Demucs is too slow at deploy).

### E8. Distillation method upgrade (CRD) — drop. Student is saturated.

---

## 6. Corpus-side improvements

### F1. Full-length tracks vs 30 s previews (RE-SCOPED)

**Critical correction (per review):** Re-encoding audio at full-length
while keeping the 30 s-preview-derived consensus labels means the
labels still reflect the catchy 30 s. The model would learn to
extract drop-features from a longer track, but the supervision is
still drop-derived. To realize the full gain, **labels must be
redone on full-length audio too** — which approximately doubles
the cost.

- **Audio-only rebuild:** +0-1 pp (small; encoder learns more
  context but supervision is unchanged).
- **Audio + caption + juror rebuild:** +3-6 pp (the real win).

**Cost.** Audio-only: 50 hr GPU. Full rebuild: 100+ hr GPU + new
caption sweep + juror pass. Plus legally-licit data sourcing
(Deezer non-preview is paid; Spotify has no audio API; YouTube-dl
is iffy; Bandcamp is the cleanest legal path for selectable artists).

**Self-critique.** Real ceiling but expensive. Defer until V18.1
truly saturates. This is the biggest single-investment win when
ready.

### F2. Audio-side data augmentation (pitch shift, time stretch, mixup)

Helps out-of-distribution robustness; doesn't move in-distribution
test PA much. Useful for cross-library generalization (§H4 below)
once we measure that gap. Defer.

### F3. Curated 500-track anchor set (RE-COSTED)

**Hypothesis.** Build a curated set of ~500 hand-validated tracks
spanning intensity and major sub-genres, with consensus labels
confirmed by ≥ 2 raters. Use as held-out test set + training-time
oversampling.

**Causal chain.** Provides a sharper held-out signal than
consensus-vs-consensus comparison. Anchors catch "MF captioned the
breakdown instead of the drop" errors that propagate through the
3-juror chain.

**Expected gain.** Doesn't move training PA. **Provides the eval
foundation for measuring all other gains.**

**Cost (REVISED).** Draft said 10 hr human time. Reality:
- 30 s clip + careful rating ≈ 3 min/track at the careful end
- 500 tracks × 3 min = **25 hr** for one rater
- Standard methodology = 2-3 raters per track for inter-rater
  agreement = **50-75 hr human time**
- Bootstrap option: use existing aggression_inspect output for the
  DnB slice (~50 anchor tracks already implicitly defined) +
  curate ~450 new tracks across other genres.

**Self-critique.** Unavoidable foundational work. Top priority.

### F4. Genre balance audit

Cheap to run (a few hours). Worth running once. May not need any
action depending on result.

---

## 7. Calibration / evaluation

### H1. LUPI gap decomposition (NEW — per reviewer)

**Hypothesis.** Train a sequence of teachers with progressively
smaller feature sets to **measure** the privileged-information gap
rather than just diagnose it:
- T_audio = teacher trained on audio_emb only (= what student
  *should* saturate at)
- T_audio+struct = + struct_tags
- T_audio+caption = + caption_emb
- T_full = current teacher (all)

**Causal chain.** The marginal PA gain per added feature **bounds**
each modality's contribution. If T_audio hits 0.83-0.85, then the
current student at 0.81 is 2-4 pp from the *audio-encoder* ceiling
(tractable via E1, E4). If T_audio only hits 0.78, then the current
student at 0.81 is *above* T_audio, indicating it's overfitting to
the larger teacher's penultimate via FitNets — and E4 (LoRA on the
encoder) is the only path forward.

**Expected gain.** Diagnostic, not direct PA. Tells us *which lever
to pull* with confidence.

**Cost.** ~2 days of work (4 teacher trains × 30 s each + 4 students
× 3 s each + analysis).

**Self-critique.** Should have been in the original spec. Move to
high priority — runs in parallel with F3 and is much cheaper.

### H2. DEAM external benchmark

In-spec optional. Provides external validation that V18.1 correlates
with academic-ground-truth arousal. Enables isotonic calibration.

### H3. Cross-library hand-eval (NEW — per reviewer)

**Hypothesis.** Get hand-eval data on at least one non-DnB library
before optimizing further. The current +55.8 pp metric is for one
DnB library. We have no evidence about model quality on tech-house,
ambient, hyperpop, hip-hop, etc.

**Cost.** Find a willing user with a different DJ-relevant library
(~few hours to set up the eval).

**Self-critique.** Without this we're optimizing single-genre slice.
Important.

### H4. MTG-Jamendo / FMA external validation

Free open datasets with mood/genre/instrument tags. Compute V18.1 on
tracks tagged "aggressive" / "dark" / "energetic" vs "calm" / "soft"
/ "peaceful" — separation should be large. Cheap external benchmark
that's redistribution-licensed (CC).

### H5. MARBLE-2 benchmark

Universal music embedding evaluation including arousal/valence tasks
(2024 update). Larger N than DEAM, more recent baselines.

---

## 8. Novel methodology directions

### I1. Audio-text contrastive pretraining → subsumed by E4.

### I2. Test-time per-collection adaptation — drop. Breaks library invariance.

### I3. Multi-clip transformer-pool — couples to F1, defer.

### I4. Self-supervised intensity discovery (SimCLR-style) (NEW — per reviewer)

**Hypothesis.** Take pairs of clips from the same track (positive)
and from different tracks of distinct intensity tier (negative).
Train a head over MuQ-MuLan-512d via contrastive loss. Cheaper
than E4 LoRA, doesn't touch the encoder.

**Causal chain.** SimCLR-style training yields representations that
respect the positive/negative structure — here, "clips from the
same track look similar; clips from very different intensity tracks
look different". This sharpens the intensity axis without supervised
labels.

**Expected gain.** +1-2 pp probably. Less than supervised E4 but
no encoder modification.

**Cost.** ~1-2 days of code; same data we have.

**Self-critique.** Cheaper alternative to E4 worth exploring.

### I5. Triplet loss with consensus-derived anchors (NEW)

**Hypothesis.** For every anchor track, sample (similar-intensity,
dissimilar-intensity) triplets; train a head to satisfy the triplet
inequality. Often outperforms direct regression for monotone-rank
targets.

**Cost.** ~1 day code change to the student trainer.

**Expected gain.** +0-2 pp. Worth ablating against current MSE.

### I6. Listwise learning-to-rank (ListMLE / LambdaRank) (NEW)

**Hypothesis.** Replace MSE with a listwise rank loss. The eval
metric (PA) is fundamentally a ranking metric; MSE optimizes the
wrong thing.

**Causal chain.** ListMLE optimizes the probability that the model's
ranking matches the target ranking. PA is exactly that probability,
restricted to pairs. ListMLE should directly improve PA.

**Cost.** Modest code change. Round-7.5 already used ListMLE; we
have the infrastructure.

**Expected gain.** +1-3 pp PA (literature-supported).

**Self-critique.** Worth ablating. Should have been in the original
draft.

### I7. BPM/key as model input (= I5 from draft)

Per E1 caveats — needs BPM accuracy validation first. Probably
subsumed by E1 if E1 includes BPM.

### I8. Mixup augmentation in embedding space — small ceiling, cheap. Try if time.

---

## 9. Methodology I considered and rejected (NEW)

To be transparent about the search: these were considered and
rejected with brief rationale.

| Approach | Rejected because |
|---|---|
| AST (Audio Spectrogram Transformer) | Trained on AudioSet for general audio; weaker on music-specific features than MuQ-MuLan/MAEST. |
| MusicFM (Won 2024) | Worth a probe; flag as future encoder candidate alongside MAEST (E5). |
| EnCodec features (Meta 2023) | Discrete codes for AudioLM/MusicGen; intermediate continuous reps untested for intensity. Speculative. |
| CLAP (LAION-CLAP-fusion) | Music-text contrastive like MuQ-MuLan; head-to-head with MuQ-MuLan would be a fair encoder ablation. Flag as alternative to MuQ-MuLan in E5. |
| Jukebox features (top-layer) | Historically SOTA but ~5B params; deploy infeasible. |
| HTSAT | CLAP backbone; small/fast; tag-focused not intensity-focused. |
| PaSST | MAEST backbone; standalone redundant if we go MAEST. |
| Wav2Vec2 + audio probe | Speech-pretrained; weak on music. |
| Self-Rewarding LLM (Yuan 2024) | Bootstrap MF-substitute juror; out of scope this round. |
| Constitutional AI judges (Bai 2022) | Reduces juror variance; folded into C1 by adding Claude. |
| Self-Consistency / ToT for evaluation | Cheaper than +jurors; folded into C1 reasoning models. |
| G-Eval / GPTScore (Liu 2023) | Better LLM eval design than 20-bucket digit; consider for next juror prompt iteration. |
| Pairwise vs pointwise calibration (Liusie 2024) | Pointwise + few-shot in-domain examples often beats pairwise for genre-stratified tasks; supports B4 design. |
| DistilBERT-style two-stage distillation | Multiple temperatures; modest gain; not worth complication. |
| Born-Again Networks (Furlanello 2018) | Often closes 50% of LUPI gap with no new data — **worth trying** as a cheap E4 alternative. **Add to ranking.** |
| Decoupled KD (Zhao 2022) | Helps when student head is small — **worth trying** with V18.1 MLP. **Add to ranking.** |
| NT-Xent / SimSiam representational distillation | Trains similarity geometry; relevant to A4 (decoupled embeddings). |

**Two newly-added items for the ranking:**

### I9. Born-Again Networks (BAN) self-distillation

Train V18.1 → V18.2 (same architecture, V18.1 outputs as targets) → V18.3 → ... iteratively. Often closes ~50 % of the LUPI gap with zero new data. ~1 hr per iteration.

### I10. Decoupled Knowledge Distillation (Zhao 2022)

Separate target-class KD from non-target distillation. Helps small student heads specifically. Modest code change.

---

## 10. Process / measurement

### J1. Canonical eval protocol (NEW — top of priority list)

A single eval invocation that produces all four numbers:
1. Held-out test PA (existing 3 985 tracks)
2. User library separation PA (the +55.8 pp metric)
3. Anchor-set Spearman ρ (when F3 lands)
4. DEAM correlation (when H2 lands)
5. (Optional) cross-library separation PA (when H3 lands)

Every change reports all of these. Currently we mix metrics
inconsistently across docs.

**Cost.** 1-2 days of code in `spike/track-grading/` to wrap
existing eval scripts.

### J2. A/B sweep infrastructure

Config-dict driven experiment runner. Lets us run B1 / B3 / E1 / E4
in a structured way without re-implementing plumbing each time.
~1 day on top of J1.

### J3. CI on V18

Tests in mesh-cue that load V18.1, embed a fixed reference clip,
verify projected score within tolerance of golden value. Catches
broken weights.

---

## 11. Final ranked recommendation (post-review)

Ranked by **(expected impact) × (probability of working) / (true cost)**.
This ranking incorporates the reviewer's critique. Major changes from
draft: J1/J2 first; A4 before A1; A3 higher (UX win); C1 cheaper than
thought; B3 rescoped; E4 lower (ONNX cost); B5 lower (poor ROI);
several new items (H1 LUPI decomposition, H3 cross-library, I6 ListMLE).

| Rank | Suggestion | Why this rank | Cost | Expected gain |
|---:|---|---|---|---|
| 1 | **J1+J2** Canonical eval + sweep infra | Without this every PA comparison below is apples-to-oranges | 2-3 days | Foundation |
| 2 | **F3** Curated 500-track anchor set | Eval foundation; ground truth for everything below | 25-50 hr human | 0 PA (eval) |
| 3 | **B6** Caption stability check | Trivial; tells us if B2 is worth running | 5 min | Information |
| 4 | **H1** LUPI gap decomposition (audio-only teacher) | Tells us *which lever to pull* — encoder vs distill vs labels | 2 days | Diagnostic |
| 5 | **H3** Cross-library hand-eval (non-DnB) | Without this we're optimizing one user's DnB | hours-days | Foundation |
| 6 | **A4** Decoupled intensity vs similarity embeddings | Prereq for A1; architectural cleanup | 1 day code + reanalysis | 0 PA (enables A1) |
| 7 | **A1** Realign deploy clip strategy with training | Cheap inference win, principled | 30 min code + reanalysis | +0-2 pp |
| 8 | **A3** Anchor-relative percentile display | Biggest UX win; library-invariant | 2 days code | 0 PA (UX) |
| 9 | **C1** 1-2 frontier-API jurors (Claude Haiku, GPT-4o-mini) | $20 + few hours; breaks σ²-floor saturation | $20 + 4 hr | +1-3 pp (with D2: +2-4 pp) |
| 10 | **D2** Snorkel correlation-aware label model (paired with C1) | Becomes valuable *immediately* with C1 | 1 day | (folded into C1 estimate) |
| 11 | **B1** Caption prompt redesign | Sharper inputs; teacher PA cascades modestly to student | 10-15 hr GPU | +0.5-2 pp |
| 12 | **B3** MF as 4th juror via existing 16-axis Likert path | Genuinely heterogeneous (audio modality); license risk noted | 17-25 hr GPU | +2-4 pp |
| 13 | **I6** Listwise rank loss (ListMLE) | Optimizes the actual eval metric (PA); cheap | 1 day | +1-3 pp |
| 14 | **I9** Born-Again Networks self-distillation | Often closes ~50 % of LUPI gap with no new data | 2-4 hr | +1-3 pp |
| 15 | **E1** Multi-task teacher (BPM aux head) | Cheap once BPM accuracy validated | 8-10 hr (incl. BPM gating) | +0-1 pp |
| 16 | **A5** Energy-based clip selection (combined with A1) | Mid-priority; helps long-quiet-intro tracks | 1 day | +1-3 pp |
| 17 | **F4** Genre balance audit (then decide) | Audit cheap; action conditional | hours | 0-2 pp |
| 18 | **I4** SimCLR-style self-supervised intensity head | Cheaper alternative to E4 LoRA | 1-2 days | +1-2 pp |
| 19 | **I5** Triplet loss with anchor-derived triplets | Worth ablating against MSE | 1 day | +0-2 pp |
| 20 | **I10** Decoupled KD (Zhao 2022) | Helps small student heads | 1 day | +0-1 pp |
| 21 | **B4** Sub-genre specialist jurors | Promising but design-heavy | days + hand-anchors | +1-3 pp |
| 22 | **B2** Multi-caption ensemble (gated on B6) | Only run if B6 finds caption instability | gated | gated |
| 23 | **E4** LoRA fine-tune MuQ-MuLan | High ceiling but ONNX export is 1-2 weeks | 1-2 weeks | +3-6 pp |
| 24 | **B5** Re-run round-7.5 BT on new tracks | 50 hr GPU for ~+0-1 pp; poor ROI | 50 hr GPU | +0-1 pp |
| 25 | **F1** Full-length corpus rebuild (audio + labels) | Highest single-investment win | weeks + legal | +3-6 pp |
| 26 | **E5** MAEST/MULE encoder swap | Already TODO; verify-by-probe before committing | weeks | +3-6 pp (probe-conditional) |
| 27 | **E6** Multi-encoder ensemble | Post-MAEST | weeks | +3-5 pp |
| 28 | **F2** Audio-side data augmentation | Robustness; small in-dist gain | days + heavy GPU | +1-3 pp OOD |
| 29 | **I8** Mixup augmentation | Small ceiling | 1 hr | +0.5-1 pp |
| 30 | **H2** DEAM external benchmark | External validation; calibration | hours | Diagnostic |
| 31 | **H4/H5** MTG-Jamendo / FMA / MARBLE-2 external benches | External validation | hours each | Diagnostic |

Items dropped / deferred / subsumed:
- A2 (top-k peak) — subsumed by A1
- A6 (multi-scale clips) — off-distribution for encoder
- C2 (reasoning juror) — folded into C1
- D1 (multi-axis consensus) — round-7.5 already tried
- E2 (ordinal regression) — minor win
- E3 (quantile head) — UX, defer
- E7 (stem separation) — too expensive
- E8 (CRD) — student saturated
- G2 (similarity-aware FT) — niche
- I1 (intensity-CLIP pretrain) — E4 cheaper version
- I2 (test-time adaptation) — breaks library invariance
- I3 (transformer pool) — couples to F1
- Round-7.7 user-fit — out of V18 scope per spec G2

---

## 12. Recommended Round-7.7 plan

**Phase 0 (this week, ~5 days):**
- J1+J2 — eval protocol + sweep infra (#1)
- B6 — caption stability check (#3)
- H1 — LUPI decomposition (#4)
- A4 + A1 — decoupled embeddings + clip-strategy realign (#6, #7)
- A3 — anchor-percentile display (#8)
- F4 — genre balance audit (#17)

**Phase 1 (~2 weeks, ~$20 in API, ~30 hr GPU):**
- F3 — anchor set (#2, in parallel)
- H3 — cross-library hand-eval (#5)
- C1 + D2 — 2 API jurors + Snorkel (#9, #10)
- B3 — MF Likert as 4th juror via existing path (#12)
- I6 — ListMLE rank loss (#13)
- I9 — Born-Again self-distillation (#14)
- E1 — multi-task teacher (gated on BPM accuracy) (#15)

**Phase 2 (~2-4 weeks):**
- B1 — caption prompt redesign (#11)
- A5 — energy-based clip selection (#16)
- I4 — SimCLR contrastive head (#18)
- I5 — triplet loss ablation (#19)
- I10 — Decoupled KD (#20)
- External benches H2/H4/H5 (#30, #31)

**Phase 3 (~weeks-months):**
- B4 — sub-genre specialist jurors (#21)
- E4 — LoRA fine-tune MuQ-MuLan (with ONNX validation) (#23)
- F1 — full-length corpus rebuild (#25)

**Phase 4+ (Lever 2 — already in TODO):**
- E5 — MAEST/MULE migration (#26, run ceiling probe first)
- E6 — multi-encoder ensemble (#27)

Each phase ends with **eval-on-everything** via J1 protocol.

---

## 13. Alternative: do nothing more this round

Per reviewer (H5): V18.1 hits G3 (0.81 vs 0.75), passes G4, G7, G8,
G9, G10. Two of ten goals fail (G5, G6) with documented rationale.
The marginal user-experience improvement from going 0.81 → 0.85 PA is
not obviously better than spending the same engineering time on the
dozens of other items in TODO.md (suggestion graph, set analysis,
native effects, USB sync).

**Defensible alternative path:**
1. **Phase 0 only** (J1, B6, H1, A4, A1, A3, F4) — ~1 week, captures
   most of the easy-win PA gains and the foundational measurement
   infrastructure.
2. **Defer Phase 1+** until either (a) the MAEST encoder migration
   completes (V19), or (b) user feedback identifies a specific
   intensity-axis pain point that justifies further investment.
3. Use freed engineering time on the broader Mesh roadmap.

This should be a conscious choice, not a default. **Recommendation:**
do Phase 0 unconditionally, then evaluate whether Phase 1's expected
~+3-5 pp PA gain is worth the effort vs other Mesh work.

---

## 14. Existential risks (NEW)

### 14.1 Music Flamingo licensing

Per `judges/music_flamingo.py:18-21`: NVIDIA OneWay Noncommercial
Academic license, research use only. V18 weights distill through MF-
derived consensus (captions → text-LLM ratings → consensus → student
training). **Whether V18 weights are a derivative work of MF outputs
is a legal question, not engineering.** The current spec note says
the weights are linear over MuQ-MuLan and MF is "training-time only",
but distillation through MF-derived signal arguably makes the
weights derivative.

**Action items:**
1. Get a legal read before any commercial release of V18.1+.
2. Alternatively, scope an **MF-free training pipeline** (e.g.,
   captions from a permissively-licensed audio-LLM like LP-MusicCaps
   or a different model with commercial licensing) for the user-
   facing model.
3. Keep MF in the research / internal pipeline; gate user releases
   on the MF-free version.

### 14.2 Single-genre evaluation

The +55.8 pp library-eval metric is one user, one DnB library. We
have **no evidence** about model quality on tech-house, ambient,
hyperpop, hip-hop, or any other DJ-relevant genre that isn't DnB.
H3 (cross-library hand-eval) addresses this.

### 14.3 σ²-floor consensus saturation

The current 3-juror DS panel is degenerate: all jurors hit the σ²
floor, so DS produces equal weights regardless of their actual
behaviour. Consensus reliability is artifactual. C1 + D2 are the
only path out. **Until they land, claims about "3-juror consensus
robustness" are misleading.**

---

## 15. Summary

The intensity axis as deployed (V18.1 + peak-clip) is good but not
great: 0.81 test PA, +55.8 pp DnB library separation. Three
foundational issues constrain further improvement:

1. **σ²-floor consensus saturation** means we have one effective
   independent vote, not three. Fix: C1+D2.
2. **MuQ-MuLan-512d is the encoder ceiling** (V18.1 MLP h=128/256/512
   sweep proved no more capacity helps). Fix: LUPI decomposition (H1)
   to confirm this, then E4 LoRA or E5 MAEST.
3. **Single-genre, single-user evaluation** means we don't know
   whether the model generalizes. Fix: H3 cross-library hand-eval +
   F3 anchor set + external benches H2/H4/H5.

The **highest ROI moves** are the cheap-and-foundational ones in
Phase 0 (1 week of work captures most easy gains and gives us the
measurement infrastructure to evaluate everything else honestly).
After Phase 0, the choice between Phase 1+ and "ship V18.1, work on
broader Mesh" is a deliberate trade-off, not a default.

If Phase 1+ proceeds, the highest expected value combination is:

- **C1 + D2** (~$20, ~1 day) — break σ²-floor, paired
- **B3** (existing-path, ~25 hr GPU) — heterogeneous-modality juror
- **I6** (ListMLE, ~1 day) — optimize the actual eval metric
- **I9** (BAN self-distill, ~hours) — close half the LUPI gap free
- **H1** (gap decomposition, 2 days) — confirm encoder ceiling
- **E4** (LoRA, 1-2 weeks) — only if H1 confirms it's worth the ONNX work

Combined expected gain: **+5-10 pp test PA**, **+5-8 pp library
separation** (target: clear the +60 pp "excellent" bar), and a
sharper picture of where the *next* round's investment should go.

The expensive infrastructural items (E5 MAEST, F1 full-length corpus)
remain the right destinations long-term but should not block Phase 0/1
work.
