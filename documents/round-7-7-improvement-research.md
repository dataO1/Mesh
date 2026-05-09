# Round-7.7 — Intensity Axis Improvement Research

**Date:** 2026-05-09 (rev. 2 — after user feedback + re-validation)
**Branch:** `text-tower-aggression-axis`
**Status:** Final
**Predecessors:** `round-7-6-pipeline-spec.md`, `round-7-6-training-log.md`,
`round-7-6-v18-1-mlp-experiment.md`, Obsidian `Mesh — Pushing Past the
70% PA Ceiling — Research` (2026-05-07)

This document enumerates and reasons about every plausible avenue for
improving the deployed intensity-axis pipeline beyond the current
**V18.1 MLP + peak-clip-pool** baseline. Each suggestion includes a
hypothesis, a causal chain explaining why it should help, an
expected-gain estimate, cost, risks, and self-critique.

A first draft was reviewed by an independent assistant. Several of
the reviewer's claims were factually wrong on re-check (notably on
LoRA cost and ONNX availability — verified against the existing
`nix/apps/convert-muq-mulan/export.py` and the Obsidian research
note that already plans this work). User pushed back on several
priorities; the final ranking in §11 reflects the post-feedback
re-evaluation.

---

## 0. Current baseline

| Component | Current state |
|---|---|
| Corpus | 39 913 Deezer 30 s previews, scraped via everynoise (~2 100 DJ-relevant seed genres × 30 tracks/seed) |
| Audio encoder | **MuQ-MuLan-large 512d** (frozen, music-text contrastive pretraining, 700M params). **ONNX-exported via `nix/apps/convert-muq-mulan/export.py` and integrated.** |
| Caption gen | **Music Flamingo 7B** (NVIDIA), `T=0.7, top_p=0.9, max_tokens=1024`, ~393 words avg, **1 caption per track** |
| Caption embedder | bge-base-en-v1.5 (768d) |
| Caption struct tags | ~50 regex-mined multi-hot tags |
| Jurors | **3** text-LLM (Mistral-Small-3.2-24B AWQ, Nemotron-30B, Qwen3.6-27B), 20-bucket two-token logprob recovery |
| Pairwise juror agreement | Spearman ρ=0.93-0.96 — **σ²-floor degenerate**, see §0.1 |
| Consensus | Continuous Dawid-Skene EM, σ²-floor=0.01, nanmedian-init → all jurors weighted **equally (1/3)** |
| Teacher | MLP `1332 → 256 → 128 → 1`, MSE on consensus, **PA = 0.94** on 3985 held-out |
| Student (V18.1) | MLP `512 → 128 → 1` over MuQ-MuLan, FitNets + Hinton + LS distillation, **PA = 0.8174** |
| Distillation gap | **+12.87 pp** |
| Library hand-eval | **+55.8 pp** aggressive_mean − liquid_mean separation on 909-track DnB library |
| Inference embedding | 6 × 10 s evenly-spaced clips, project each through V18.1, take peak |

### 0.1 σ²-floor consensus saturation — explained

This is one of the most important methodological observations in the
current pipeline, and the original draft buried it. Plain explanation:

**What Dawid-Skene does:** for each juror it tries to learn how noisy
that juror is — its noise variance σ². A juror with low σ² (its
ratings are tight against the consensus) gets high reliability
(weight ∝ 1/σ²); a juror with high σ² gets low reliability. Then the
consensus is a precision-weighted average.

**The σ² floor:** we added a hard floor σ² ≥ 0.01 to prevent
runaway-feedback: without it, a juror whose σ² happens to round to ~0
in iteration 1 gets infinite precision, pins the consensus to itself,
and starves the others. The floor caps max precision at 100, just
below the healthiest observed juror's natural precision.

**What happened in V18:** **all 3 jurors hit the floor.** Their
residuals against the EM consensus are so small (because they all
agree very strongly with each other — pairwise ρ = 0.93-0.96) that
DS can't distinguish them. Result: each juror gets weight 1/3.

**Why this is a problem (the part that needs unpacking):**
- It's NOT that we're "only valuing one judge". The consensus IS the
  3-juror equal-weighted average, so all 3 do contribute equally.
- It IS that we're getting **much less independent information than
  3 jurors should provide**. In Bayesian terms, 3 jurors with
  pairwise ρ ≈ 0.95 give you maybe **1.5× the precision of one juror**,
  not 3×. The "3-juror robust consensus" framing is misleading —
  we have something closer to 1.5 effective independent votes wearing
  a 3-juror costume.
- And worse: if the 3 correlated jurors share a **systematic bias**
  (e.g., they all under-rate dark-ambient-with-distorted-vocals
  because that combination is rare in their training data), the
  consensus inherits that bias unchallenged. With only correlated
  jurors, DS can't tell us this is happening.

**Does the user's "local sub-genre specialist juror" idea help here?
YES, dramatically.** This is the right intuition. The whole point is
to introduce jurors **with uncorrelated errors** — not just more
jurors:
- A DnB-specialist juror that knows "neurofunk = peak-time ≈ 17/19"
  has different errors than a generalist Qwen — its errors will be
  on intra-genre micro-distinctions, not on coarse genre placement.
- An audio-grounded juror (MF Likert via the existing 5-bucket path)
  has errors uncorrelated with text-LLM jurors because it sees a
  different modality entirely.
- A frontier-API juror with different RLHF training (Claude, GPT-4)
  has errors driven by different pretraining biases than the existing
  open-weight panel.

When *any* of those land, pairwise ρ drops below 0.85 against the
existing trio, σ² of the new juror differs from the floor, DS starts
distinguishing reliabilities, and we get genuine multi-vote
consensus. Adding *another generalist text-LLM* (e.g., Llama-3.1-70B
on the same captions) only modestly helps because it shares too much
training-data overlap with the existing trio.

### 0.2 Train-vs-deploy clip distribution — clarified

- **Training**: `MuQMuLan.from_pretrained()` over the 30 s Deezer
  preview internally splits into **3 × 10 s contiguous non-overlapping
  clips** and averages. Deezer's preview heuristic typically picks
  the catchiest 30 s, so training distribution = "3-clip mean of a
  drop-rich 30 s window".
- **Deployment** (post 2026-05-08, `inference.rs:309-326`): pick **1
  of 6 evenly-spaced** 10 s clips by V18.1 score; discard the other 5.
- The deploy distribution is *more peaky* than training was.

### 0.3 Hand-eval is single-genre, single-user

The +55.8 pp library separation is on 909 DnB tracks for one user.
The 39 913-track training corpus spans ~2 100 genre seeds, so
training is broad — but we have **no evaluation evidence** for
tech-house, ambient, hyperpop, hip-hop, or any non-DnB collection.

### 0.4 Encoder is the bottleneck (per existing research)

Per the user's Obsidian note `Mesh — Pushing Past the 70% PA Ceiling
— Research` (2026-05-07): the V15/V17/V18 architecture (frozen
MuQ-MuLan + linear/MLP probe) **is the literature-correct shape**
(direct precedent: BATON IJCAI 2024, "Aligning Audio Captions"
arXiv 2509.14659 — both use frozen-encoder + small-MLP-head for
exactly this kind of preference-ranking task). The bottleneck is
not the architecture. It's the encoder's representation of intensity-
discriminative features, because MuQ-MuLan was trained for music-text
similarity, not for intensity. The note prescribes:

1. **E1: judge swap** (Music Flamingo + ensemble) — **DONE**. We did
   this in round 7.6: replaced the V17.5 BT-priors-from-Qwen3-Omni
   with the MF-caption + 3-text-LLM-juror pipeline. V15 (0.70 PA)
   → V18.1 (0.81 PA) on the user library. Predicted gain was +3-7
   pp; actual was ~+11 pp. **The judge swap delivered.**
2. **E2: LoRA on MuQ-MuLan** with end-to-end pairwise/rank-supervised
   loss — **NOT DONE**. Predicted gain on top of E1: +2-4 pp PA.
3. **E3: RINCE rank-supervised contrastive** (skip BT MLE entirely) —
   **NOT DONE**. High-novelty alternative.

**Implication for this round:** the next high-ROI move is **E2
(LoRA on MuQ-MuLan)**, not "more jurors". More-jurors only helps
the consensus quality marginally; the student is encoder-bound.

---

## 1. Inference-time changes (no retraining)

### A1. Realign deploy clip strategy with training

**Hypothesis.** Replace 1-of-6-peak with: (1) score 6 evenly-spaced
10 s clips, (2) find the highest-scoring contiguous 30 s window (the
3 adjacent clips with maximum sum-intensity), (3) mean those 3 clip
embeddings, (4) project through V18.1.

**Causal chain.** V18.1 was trained on 3-clip-mean inputs; deploy
gives it single-clip inputs. Single-clip is brittle (a tom fill or
a vocal hook can be the chosen clip). The 3-clip mean of the densest
30 s recovers exactly what training saw.

**Expected gain.** +0-2 pp library separation.

**Cost.** ~30 lines in `inference.rs::analyze` + reanalysis pass
(~5 min wall on 909 tracks).

**Prerequisites.** **A4** (decoupled intensity vs similarity
embeddings) should land first.

### A2. Top-k peak pooling — subsumed by A1.

### A3. Anchor-relative percentile display

**Hypothesis.** Display layer maps raw V18.1 scores to percentiles
of a **hand-picked anchor set** (~30 tracks spanning the spectrum)
rather than percentiles of the live library. "p80" then means "as
intense as the 80th percentile of canonical references", not "as
intense as 80 % of *this user's* library".

**Expected gain.** Doesn't move PA. Significant UX win.

**Cost.** ~50 lines of Rust.

**License note.** Don't ship Deezer-derived embeddings. Two options:
ship only V18.1 *scalar scores* per anchor (one float per anchor,
no audio/embedding data), or use redistribution-licensed audio
sources (CC, MTG-Jamendo subset). Scalar-only is simplest.

### A4. Decouple intensity vs similarity embeddings (PREREQUISITE FOR A1 *AND* E4)

**Hypothesis.** Store **two** 512d vectors per track:
(a) **peak-region embedding** (A1's output) for intensity scoring;
(b) **mean-of-all-clips embedding** for similarity / PCA / clustering.

**Expected gain.** 0 PA. Enables A1 without similarity regression.

**Cost.** New DB column, second pooling pass at analysis time, ~50
lines of Rust.

**Why this is also a prereq for E4 (added in rev. 2):** E4 LoRA-tunes
the MuQ-MuLan encoder for intensity-rank loss. The same encoder
output goes to mesh-cue similarity (`MlAnalysisResult.embedding` at
`inference.rs:42-47` feeds both intensity scoring AND similarity
search). LoRA fine-tuning rotates the 512d geometry toward
intensity-discrimination, which can degrade music-text similarity
unnoticed. With A4 in place, the *intensity* embedding column gets
the LoRA-tuned encoder; the *similarity* embedding column keeps the
pretrained MuQ-MuLan. Without A4 we ship a similarity regression
masked by an intensity win.

### A5. Energy-prefiltered clip selection

**Hypothesis.** Use cheap energy stats (RMS, kick-band 50-100 Hz
energy, spectral flux) to pick clip *centres* on the 6 highest
peaks instead of evenly-spaced positions.

**Expected gain.** +1-3 pp library separation, mostly on tracks
with long quiet intros.

**Cost.** ~50 lines of Rust + use existing mel data.

**Combine with A1**: use energy peaks as clip centres, then proceed
with A1's contiguous-30 s-window selection.

---

## 2. Caption-side improvements

### B1. Caption prompt redesign for intensity

Rewrite MF prompt to elicit drop architecture, bass weight, distortion
character, vocal aggression, BPM perception, peak-time suitability.
Current prompt is generic.

**Expected gain.** **+0.5-2 pp student PA**. The student is
encoder-bottlenecked; sharper teacher inputs help marginally.

**Cost.** **10-15 hr MF re-run** (longer outputs from list-format
prompts) + ~9 hr juror compute (parallel).

### B2. Multi-caption ensemble — gated on B6 (caption stability check).

### B3. MF as 4th juror via 5-bucket Likert path (RESCOPED + LIKERT VALIDATION REQUIRED)

**Original draft was wrong.** I proposed direct 20-bucket integer
rating for MF. Per Obsidian `Mesh — Music Flamingo Pointwise
Findings` and `judges/music_flamingo.py:226-235`: **Music Flamingo
collapses to modal "50" on integer-rating prompts** — never trained
for that task. Confirmed empirically on a 200-track smoke: 3 of 16
axes returned exactly 50 for every clip; only 5 axes had std > 12.

**Corrected.** Use the **5-bucket Likert path** (`score_likert()`
in `judges/music_flamingo.py:213-287` and the
`load_mf_likert_intensity()` reader in `aggregate_consensus.py`)
over 8 pro-high axes (`LIKERT_PRO_HIGH`). MF rates each (track,
axis) on 1-5 with logprob recovery; z-mean across pro-high axes
gives a per-track scalar.

**Critical caveat (rev. 2 — caught by reviewer):** the Likert *code
path* exists but has **never been smoke-tested at scale**. The 200
tracks of MF data on disk at `/home/data01/Music/mesh-track-grading/round7_6_likert/`
appear to be from the failed 0-100 pointwise smoke, not from the
Likert path. The Pointwise Findings doc *proposes* Likert as the
fix but never reports a Likert smoke result.

**Required prerequisite for B3:** run a 200-track Likert smoke
across the 8 pro-high axes BEFORE committing to the full 39 913-
track sweep. Win/fail signal:
- **Win:** the dead axes from the 0-100 smoke (dynamic_envelope,
  melodic_complexity, harmonic_motion, melodic_anchoring) now have
  std > 0.5 on the 1-5 scale.
- **Fall back to Path B:** if Likert also collapses, fall back to
  Path B from the Pointwise Findings — MF-Think (chain-of-thought
  caption with reasoning) → text-LLM rerate. Adds ~1 day of work
  but stays in MF's strong mode.

**Causal chain (against σ²-floor saturation).** MF sees the audio
directly (not just a caption). Its errors are uncorrelated with
text-LLM jurors → pairwise ρ should drop below 0.85 → σ²-floor
breaks → DS gets actual reliability information.

**Expected gain.** +2-4 pp student PA. Larger than any text-juror
addition because of the modality difference.

**Cost.** **17-25 hr MF compute** (39 913 tracks × 8 axes ÷ 5.2
c/s) + 30 s consensus rerun.

### B4. Sub-genre specialist local jurors (USER'S IDEA — promoted, with caveats)

**Hypothesis.** In-context-tune juror prompts with sub-genre anchor
examples. E.g., DnB-specialist gets ~16-32 anchor tracks ("Black Sun
Empire 'Lights Out' = 17/19", "Random Movement 'Slinkystink' =
4/19") in the prompt; rates new tracks against those anchors.
Apply per-genre via soft genre-cluster weights from caption-emb
K-means.

**Anchor source (rev. 2 — caught by reviewer):** the anchor labels
**must come from F3 (curated 500-track set), not the user library.**
Using user-library tracks as anchors would violate G2 (no user-
library leakage) and turn this into a per-user calibration tool
disguised as a general-purpose juror — exactly the failure mode in
the user-memory `feedback_intensity_axis_general_purpose.md`. This
makes B4 *strictly downstream* of F3 in the pipeline DAG; it cannot
land in Phase 1 if F3 is still in progress.

**In-context anchor count:** the draft said "~50 anchors per genre".
Per ICL literature, calibration plateaus at ~10-20 in-context
examples and degrades past ~50 due to context dilution.
**Sweep K = 8, 16, 32 anchors** on a held-out F3 genre subset
before committing to one count. Likely sweet spot is 16-24.

**Genre-routing soft weights:** uses caption-emb K-means cluster
membership. Round-7.5 used K-means clusters as a *diagnostic*, not
as a stable per-track classifier. Boundary tracks (techno-leaning
DnB, ambient-leaning electronica) have unstable cluster assignments
between runs. Mitigation: bootstrap-resample the K-means assignment
to get a confidence interval; if no cluster has > 0.6 weight,
fall back to the general-purpose 3-juror panel for that track.

**Calibration verification:** anchors used IN the prompt cannot also
be used to verify calibration (that's memorisation check, not
calibration). Need a *held-out* set per genre — effectively
doubling the human-rating burden from F3's 500 to ~1 000 tracks
total if both prompt-anchor and verification sets must be curated.
Or: split F3's 500 tracks 50/50 between training-anchor and
verification-anchor pools.

**Causal chain (against σ²-floor saturation).** A specialist's
errors are systematically different from a generalist's. The
generalist might rate every DnB track in the same narrow band
because it can't distinguish sub-genres; the specialist
discriminates fine within DnB but is genre-anchored. Their
disagreement encodes intra-genre information the panel currently
lacks.

**Expected gain.** +1-3 pp library separation (especially on
intra-genre ranking, which is what user libraries care about).
Less on test PA (test set is dominated by cross-genre
discrimination).

**Cost.** Curate ~50 anchors per genre × ~5 genres = 250 hand-rated
tracks (~12-25 hr human time depending on rater rigour). Then
~3-4 hr juror re-run with longer in-context prompts. Implementation:
a single juror script with a config-driven anchor bank per genre.

**Risks.**
- Genre detection is itself error-prone; mis-routed tracks get
  worse ratings. Use **soft genre weights** (track classified 70%
  DnB / 20% techno / 10% jungle gets a weighted average of all 3
  specialists), not hard routing.
- Specialists over-anchor on examples. Calibrate by checking that
  specialist scores on the anchor tracks themselves match the
  anchor labels.

**Self-critique.** This is a strong addition specifically because
it directly attacks σ²-floor saturation in a way that adds new
information rather than just "more general jurors". Promote in
ranking.

### B5. Re-run round-7.5 BT priors on new 24 599 tracks

**Verdict.** ~+0-1 pp for ~50 hr GPU. Poor ROI per σ²-floor
analysis (Qwen3-Omni-derived BT will correlate ρ ≥ 0.85 with the
existing text-LLM panel). **Skip unless** a future round needs
multi-axis BT for orthogonal reasons.

### B6. Caption stability sanity check — 5 min, do it.

---

## 3. Juror panel — frontier-API additions

### C1. 1-2 frontier-API jurors (PRICED CORRECTLY THIS TIME)

**Hypothesis.** Add jurors from genuinely different lineages:
- **Claude Haiku 4.5** (Constitutional AI training)
- **GPT-4o-mini** (different RLHF)

**Causal chain.** Same as B3 — break σ²-floor saturation. But
weaker than B3 because these are still text-on-caption jurors,
just with different RLHF biases. Expected pairwise ρ with the
existing trio: 0.85-0.92 (lower than open-weight ρ but higher
than MF Likert's audio-modality ρ).

**Expected gain.** **+1-3 pp** student PA conditional on σ²-floor
breaking (paired with D2 Snorkel, +2-4 pp).

**Cost.** **~$5 GPT-4o-mini + ~$15 Claude Haiku** (30M input tokens
× $0.15/M ≈ $4.50 GPT-4o-mini; ~$0.80/M Claude Haiku) + ~30 min
API time.

### C2-C3. Reasoning model variants — fold into C1.

---

## 4. Consensus / aggregation

### D1. Multi-axis consensus — round-7.5 already tried. Drop.

### D2. Snorkel correlation-aware label model (PAIR WITH C1, B3, B4)

**Hypothesis.** Replace continuous DS with Snorkel's LabelModel
that handles correlations between sources. Currently DS assumes
conditional independence given z, violated when 3 jurors read the
same caption.

**Causal chain.** Once C1 / B3 / B4 land 1-3 new jurors with
genuinely different error structure, Snorkel deflates the
correlated trio's joint weight, giving heterogeneous sources room
to contribute.

**Expected gain.** +0-2 pp **conditional on** C1+B3+B4 having
broken σ²-floor saturation. With current saturated 3-juror panel,
Snorkel produces same equal-weight output as floored DS.

**Cost.** ~1 day to swap in Snorkel's LabelModel.

---

## 5. Teacher and student model

### E1. Multi-task teacher (BPM aux head)

**Hypothesis.** Teacher predicts intensity AND BPM bin / genre cluster.
Aux heads regularize the shared backbone, enriching the penultimate
that FitNets matches.

**Expected gain.** **+0-1 pp** student PA. Student is saturated
against MuQ-MuLan-512d (V18.1 h=128/256/512 sweep showed h=512
adds +0.07 pp). Adding regularization to the teacher doesn't add
new information to the encoder the student reads.

**Prerequisites.** Validate BPM detection accuracy on a 50-track
EDM hand-rating before training. `librosa.beat.tempo` halves/
doubles routinely on DnB (175 BPM read as 87.5 BPM is common).

**Cost.** 8-10 hr (BPM compute + validation + train). Plus Spotify
ISRC lookup if we want danceability/valence aux targets (~2 hr).

### E2. Ordinal / quantile / LoRA / encoder-swap items below.

### E3. Quantile head — defer (UX, not PA).

### E4. **LoRA r=16 fine-tune of MuQ-MuLan with end-to-end rank loss** (RESTORED TO HIGH PRIORITY)

**Hypothesis.** Fine-tune MuQ-MuLan with LoRA adapters on a
**rank-supervised loss** (pairwise BT logistic, listwise ListMLE,
or RINCE) using our consensus targets. Encoder learns intensity-
discriminative features that aren't in its pretraining.

**What is LoRA (since you asked).** LoRA = Low-Rank Adaptation of
Large Language Models (Hu et al. 2021). The idea:

- A pretrained model has weight matrices like W ∈ ℝ^(768×768) =
  ~590k params *per layer*. Fine-tuning all of them needs huge
  VRAM and risks catastrophic forgetting.
- LoRA decomposition: replace W with W' = W + B·A, where B ∈
  ℝ^(768×r) and A ∈ ℝ^(r×768) with **r small** (e.g., r=8 or 16).
  B·A has rank r → only **2 × 768 × r = 12-25k extra params per
  layer**.
- You **freeze W** and only train B and A. Total trainable params:
  ~0.4M for MuQ-MuLan attention layers vs 700M for full fine-tune.
- After training, you can **merge** the LoRA back into the base:
  W_final = W + B·A → result has the same shape as the original W.
  **Inference cost is identical** to the original model.
- Per Ding et al. NeurIPS-W 2024 ("Parameter-Efficient Transfer
  for Music Foundation Models"): LoRA r=8 on MuQ-MuLan beats
  linear probing by ~2 pp at 2.5-3× faster training than full
  fine-tune, with **zero inference overhead** because of merging.

**Causal chain.** MuQ-MuLan was trained for music-text similarity
(InfoNCE on captioned tracks). Its 512d geometry is shaped by
genre/mood similarity, not intensity. LoRA fine-tuning on a
rank-supervised loss directly re-shapes the embedding so that
**intensity becomes a more dominant axis** without losing the
underlying music-text geometry (LoRA's low-rank constraint
prevents over-writing).

**Expected gain.** **+2-4 pp student PA** per Ding et al. + the
user's Obsidian note. This is the path the existing research
explicitly recommends as next.

**Cost (REVISED twice).**
- *Pre-train inspection* (which `nn.Linear` modules does attention use?
  fused or split QKV?): ~half day.
- *Pre-train parity check* (identity-LoRA → merge → re-export →
  byte-equality vs base ONNX): ~half day.
- LoRA training: ~6-12 hr GPU on the 5090 (Ding et al. baseline).
- Merge + re-export via existing `convert-muq-mulan/export.py`:
  ~1-2 hr including hierarchy debugging if `merge_and_unload()`
  leaves residual containers.
- Post-train parity validation (PyTorch outputs vs ONNX outputs on
  held-out 100-track sample, expect ≤ 1e-4 cosine drift): ~1 hr.
- **Total realistic budget: 2-4 days** (was claimed 1-2 in rev. 1;
  ONNX is *not* a 1-2 week blocker but the gotchas above add up).

**Loss commitment (rev. 2):** **Pick BT logistic, not ListMLE/RINCE.**
The reviewer is right that the draft's "BT, ListMLE, or RINCE — pick
one" is hand-waving. Picking is the work. Commit:
- **BT pairwise logistic** is what Ding et al. (NeurIPS-W 2024) used,
  what BATON (IJCAI 2024) used, and what `Aligning Audio Captions`
  (arXiv 2509.14659) used. Three independent precedents on
  music-LM rank fine-tuning. Stick with it.
- ListMLE moves to I6 as a separate ablation on the *student head*
  only (not on the encoder).
- RINCE remains as I11 — high-novelty, post-V18.2 / round-8 candidate.

**Risks.**
- **Flash-attention conformer + LoRA composability (caught by reviewer):**
  MuQ-MuLan's audio side is a Conformer. If the self-attention block
  uses fused QKV (`nn.Linear(d, 3*d)` + chunk), PEFT's standard
  target list `["q_proj", "k_proj", "v_proj"]` matches nothing.
  **First half-day task: inspect MuQ source for which `nn.Linear`
  modules attention uses.** If QKV is fused, either un-fuse pre-train
  + re-fuse at merge, write a custom LoraLayer for the fused weight,
  or restrict adapters to FFN linears only (known weaker recipe per
  Hu et al. 2021).
- **PyTorch ↔ ONNX parity drift after merge (caught by reviewer):**
  `merge_and_unload()` does B@A in fp32 and may leave residual
  `LoraLayer` containers in the module hierarchy. The export
  monkeypatch (`export.py:88-112`) reaches `mulan.mulan.audio.model.model`
  by exact attribute path; any wrapping silently no-ops the patch.
  **Pre-train parity check:** load base, merge with B=0, A=0
  (identity LoRA), re-export, verify byte-equality of ONNX weights.
  Half-day task BEFORE training. If this fails, fix the export path
  first.
- **Encoder side-effect on similarity:** without **A4** (decoupled
  intensity vs similarity embeddings), LoRA-rotation of the encoder
  for intensity-rank propagates into the similarity vector that
  feeds suggestion-graph / clustering / PCA. A test PA win can
  ship as a similarity regression no one is measuring. **A4 is a
  hard prerequisite for E4.**
- Catastrophic forgetting of music-text similarity if over-trained.
  Mitigation: low-rank constraint (r=8-16) inherently regularizes;
  early-stop on val similarity score against held-out caption-text
  pairs.
- LoRA license: trained adapters are derivative of MuQ-MuLan
  (CC-BY-NC-4.0). Already an accepted license constraint.

**Self-critique.** This was the *original plan* per the 2026-05-07
research note and got delayed by the round-7.6 caption pipeline
work. With V18.1 shipped, this is the natural next step. The
reviewer's "ONNX is a 1-2 week blocker" claim was wrong — they
weren't aware of the existing export pipeline.

### E5. Encoder swap (MAEST / MULE / MusicFM)

Already in TODO + `embedding-models-research.md`. Higher ceiling
than E4 LoRA but multi-week migration. **Sequence: E4 first; E5
after V18.2 ships and the LoRA-fine-tuned MuQ ceiling is measured.**

### E6. Multi-encoder ensemble — defer to post-E5.

### E7. Stem-separated encoding — too expensive at deploy.

### E8. Distillation method upgrade (Decoupled KD) — see I10 below.

---

## 6. Corpus-side improvements

### F1. Full-length tracks vs 30 s previews (RE-SCOPED)

**Critical correction:** re-encoding audio at full-length while keeping
30 s-preview-derived consensus labels means **labels still reflect
the catchy 30 s**. Real gain requires **full-length audio + full-length
captions + re-juror**.

- Audio-only rebuild: +0-1 pp
- Full rebuild (audio + captions + jurors): +3-6 pp

**Cost.** Full rebuild: 100+ hr GPU + new caption sweep + juror pass +
licit data sourcing (Bandcamp is the cleanest legal path).

**Defer until V18.x truly saturates.**

### F2. Audio augmentation — robustness, not in-dist PA. Defer.

### F3. Curated 500-track anchor set (foundational eval)

**Hypothesis.** Build 500 hand-validated tracks spanning intensity
and major sub-genres, ≥ 2 raters, with consensus labels. Use as
held-out test set + training-time oversampling.

**Causal chain.** Provides a sharper held-out signal than
consensus-vs-consensus comparison. Anchors catch "MF captioned the
breakdown instead of the drop" errors.

**Expected gain.** Doesn't move training PA. **Provides eval
foundation for measuring all other gains.**

**Cost.** 500 tracks × 3 min/rating × 2 raters = ~50 hr human
time. Bootstrap option: use existing `aggression_inspect` known-
aggressive/known-liquid lists for the DnB slice (~50 tracks
implicit) + curate ~450 new tracks across other genres.

### F4. Genre balance audit — cheap, run once.

---

## 7. Calibration / evaluation

### H1. LUPI gap decomposition (NEW — explained below)

**What it does.** Train a sequence of teachers with progressively
smaller feature sets to *measure* the privileged-information gap
rather than just diagnose it:

| Teacher | Features | Purpose |
|---|---|---|
| T_audio | audio_emb only | What student should saturate at |
| T_audio+struct | + struct_tags | Marginal of struct tags |
| T_audio+caption | + caption_emb | Marginal of caption channel |
| T_full | + r7.5_tags (current setup) | Today's teacher |

**Causal chain.** The marginal PA gain per added feature **bounds**
each modality's contribution. Two scenarios:

- **If T_audio hits 0.83-0.85**: student at 0.81 is 2-4 pp from
  the audio-encoder ceiling. E4 LoRA can plausibly close most of
  that gap.
- **If T_audio only hits 0.78**: student at 0.81 is *above* T_audio,
  meaning the FitNets penultimate-matching is already extracting
  all available audio signal. E4 LoRA still helps (changes the
  encoder, raises T_audio), but the marginal effect of *more
  privileged features* would be modest.

**Why this matters.** Tells us **which lever to pull with confidence**
rather than guessing. If T_audio is already > 0.85, the encoder
isn't the bottleneck and we should focus elsewhere. If T_audio is
~0.78, encoder is the bottleneck (consistent with current research)
and E4 LoRA is the right move.

**Cost.** ~2 days of work. 4 teacher trains × 30 s + 4 students ×
3 s + analysis. Trivial compute.

**Self-critique.** Should have been in the original spec. Cheap
enough to run in parallel with anything else.

### H2. DEAM external benchmark — in-spec optional.

### H3. Cross-library hand-eval (non-DnB) — required before further optimization.

### H4. MTG-Jamendo / FMA external validation — cheap CC benches.

### H5. MARBLE-2 benchmark — newer than DEAM, larger N.

---

## 8. Novel methodology directions — explained (per request)

This section explains what each of these techniques *actually does*
to the model behaviour.

### I1. Audio-text contrastive pretraining → subsumed by E4 LoRA.

### I2. Test-time per-collection adaptation — drop, breaks library invariance.

### I3. Multi-clip transformer-pool — couples to F1, defer.

### I4. SimCLR-style self-supervised intensity head — explained

**What it does.** Take pairs of clips:
- **Positives**: two clips from the *same track* — the model should
  embed them close together.
- **Negatives**: clips from tracks at very *different* consensus-
  intensity tiers — the model should push them apart.

Train a small head over MuQ-MuLan-512d via contrastive loss
(InfoNCE). The head learns geometry where intensity-similar tracks
cluster.

**Effect on the model.** Reshapes the 512d embedding to be more
intensity-aware *without* re-training the encoder itself. Does what
LoRA does conceptually but only in the projection-head, so it's
much cheaper but with a smaller ceiling.

**Expected gain.** +1-2 pp. Compare to E4 LoRA's +2-4 pp at 5×
more compute.

### I5. Triplet loss with anchor-derived triplets — explained

**What it does.** Sample (anchor, positive, negative) where
positive is intensity-similar to anchor and negative is intensity-
different. Train the head so:
`distance(anchor, positive) + margin < distance(anchor, negative)`.

**Effect on the model.** Same family as SimCLR but more direct —
the loss is "this pair should be closer than that pair", explicit
about which way embeddings should move.

**Expected gain.** +0-2 pp depending on triplet sampling strategy.
Often beats MSE for monotone-ranking targets.

### I6. ListMLE / listwise rank loss — explained

**What it does.** Currently we use **MSE** which optimizes squared
error on individual scores. PA (the eval metric) is fundamentally
a **ranking** metric: "for pair (i, j), does the model agree with
consensus on who's more intense?". MSE ≠ PA loss surface.

ListMLE optimizes **the probability that the model's predicted
ranking matches the consensus ranking**. Specifically, given a
list of scores [c₁, c₂, ..., cₙ] sorted by consensus, it computes
the likelihood that the model assigns scores in the same order:

```
P(model ranks list correctly) = ∏ᵢ exp(s_π(i)) / Σⱼ≥ᵢ exp(s_π(j))
```

where π is the permutation by consensus order.

**Effect on the model.** Shifts loss landscape so the model focuses
on *getting orderings right*, not exact values. Often +1-3 pp on
rank metrics. Round-7.5 already used ListMLE — we have the
infrastructure, just need to bring it back for V18.

**Expected gain.** +1-3 pp test PA.

### I7. BPM/key as model input — same caveats as E1.

### I8. Mixup augmentation in embedding space

Linear interpolation between embedding pairs as synthetic training
samples. Smooths the loss landscape, regularizes. Modest gain (+0.5-1 pp)
in regression. Cheap.

### I9. Born-Again Networks (BAN) self-distillation — explained

**What it does.** Train V18.2 using V18.1's outputs as soft targets
(instead of consensus directly). Same architecture, same data, but
V18.2 sees V18.1's full output distribution rather than just hard
labels. Iterate: V18.2 → V18.3 → V18.4.

**Why it works.** V18.1's soft outputs encode "dark knowledge" —
relative confidence between samples that hard labels don't have.
"This track is 0.81 and that one is 0.79" is more informative than
"this track is the 1230th most intense". V18.2 learns the dark
knowledge structure even though it sees the same surface labels.

**Furlanello 2018 (NeurIPS) BAN result:** generations 1-3 typically
gain 0.5-2 pp; 4+ generations rarely help further.

**Effect on the model.** Closes ~50 % of the LUPI gap *with no new
data and no new compute except retraining*. Free PA.

**Expected gain.** +1-3 pp. Specifically attractive because it's
~hours of compute and no methodology risk.

### I10. Decoupled Knowledge Distillation (Zhao 2022 CVPR) — explained

**What it does.** Standard KD has one combined loss for matching
the teacher's full output distribution. Decoupled KD splits this
into:

1. **TCKD (Target-Class KD)**: how well the student matches the
   teacher's confidence on the *correct* class (or, for regression,
   the dominant target).
2. **NCKD (Non-target KD)**: how well the student matches the
   teacher's distribution over *other* classes (or, for regression,
   the residual/uncertainty).

These are weighted separately. Often the optimal weighting is
**very different** from the implicit equal-weight in standard KD —
particularly when the student head is small (which V18.1's
512→128→1 is).

**Effect on the model.** Lets the student spend more capacity on
the right thing instead of fighting the loss landscape.

**Expected gain.** +0-1 pp. Modest.

### I11. RINCE (Ranking InfoNCE) — from Obsidian E3, explained

**What it does.** Per Bosch's AAAI 2022 paper. Standard InfoNCE
treats pairs as binary positive/negative. RINCE consumes **rank
labels** directly:

- Given K-way ranked tuples (e.g., the consensus ranks 4 tracks),
  enforce that the top-ranked track is closest to itself, then the
  2nd-ranked, etc. Embedding similarity should *gradually decrease*
  with rank position.
- Implementation: weighted InfoNCE where the weighting reflects
  rank distance. Skip the BT MLE step entirely — directly use the
  K-way rankings as supervision.

**Effect on the model.** The Obsidian note flags this as the **one
architectural change that genuinely uses our LLM rankings as
supervision** rather than BT-mediated regression targets. Avoids
BT score-collapse on sparse pair graphs.

**Expected gain.** Unknown; high-novelty per the Obsidian note.
Worth a 2-week pilot **after** E4 LoRA proves the easier path. The
note suggests RINCE is the long-term right answer for scaling to
16+ axes simultaneously.

---

## 9. Methodology I considered and rejected

| Approach | Rejected because |
|---|---|
| AST (Audio Spectrogram Transformer) | AudioSet-pretrained; weaker on music-specific features. |
| MusicFM | Worth a probe; flag for post-MAEST migration consideration. Skip-by-default per Obsidian "Skip — chord/beat strength, not similarity". |
| EnCodec features | Discrete codes; intermediate continuous reps untested for intensity. Speculative. |
| CLAP (LAION-CLAP-fusion) | Music-text contrastive like MuQ-MuLan; alternative if MuQ saturates. |
| Jukebox features | ~5B params; deploy infeasible. |
| HTSAT, PaSST, Wav2Vec2 | Tag-focused or speech-focused; weaker than music-specific MuQ/MAEST. |
| DistilBERT-style two-stage distillation | Modest gain; not worth complication. |
| Constitutional AI for judges | Folded into C1 by adding Claude. |
| Self-Consistency / Tree of Thoughts | Folded into C1 reasoning models. |
| G-Eval / GPTScore | Better LLM eval design; consider for next juror prompt iteration. |
| CRD distillation | Student already saturated against current encoder. |
| Train music encoder from scratch | Per Obsidian Q3: 128 hr corpus is 7-50× short of smallest credible recipe (~1 000 hr). |
| Distill MuQ-MuLan into smaller student | For round 8 productisation per Obsidian. |

---

## 10. Process / measurement

### J1. Canonical eval protocol — one invocation, 5 numbers (test PA, library PA, anchor ρ, DEAM ρ, cross-library PA).

### J2. A/B sweep infrastructure — config-dict driven.

### J3. CI on V18.x in mesh-cue.

---

## 11. Final ranked recommendation (post-feedback re-evaluation)

The major change from the prior ranking: **E4 (LoRA on MuQ-MuLan) is
restored to high priority** (the reviewer's ONNX-cost objection was
wrong; we already have a working custom export pipeline). The user's
existing research note (`Mesh — Pushing Past the 70% PA Ceiling`)
explicitly identified E4 as the next priority after the round-7.6
judge swap, with combined +5-10 pp expected gain.

Also: **B4 (sub-genre specialist local jurors) is promoted** because
it directly attacks σ²-floor saturation in a way that adds new
information rather than just "more general jurors".

Ranked by **(expected impact) × (probability of working) / (true cost)**:

| Rank | Suggestion | Why this rank | Cost | Expected gain |
|---:|---|---|---|---|
| 1 | **J1+J2** Canonical eval + sweep infra | Foundation for every comparison below | 2-3 days | Foundation |
| 2 | **B6** Caption stability check | 5 min; tells us if B2 is worth running | 5 min | Information |
| 3 | **H1** LUPI gap decomposition | Cheapest information-bearing experiment; gates E4 expectation | 2 days | Diagnostic — gates everything |
| 4 | **F3** Curated 500-track anchor set (split prompt-anchor / verification-anchor) | Eval foundation; supplies B4 anchors via F3 not user library | ~50 hr human (2 raters) | 0 PA (eval) |
| 5 | **A4** Decoupled intensity vs similarity embeddings | Prereq for **A1 *and* E4** | 1 day code + reanalysis | 0 PA (enables A1, E4) |
| 6 | **H3** Cross-library hand-eval (non-DnB) | Without this we're optimizing one user's DnB; gating signal for E4 | hours-days | Foundation |
| 7 | **A1** Realign deploy clip strategy with training | Cheap inference win | 30 min code + reanalysis | +0-2 pp |
| 8 | **A3** Anchor-relative percentile display | Biggest UX win | 2 days code | 0 PA (UX) |
| 9 | **E4** LoRA r=16 on MuQ-MuLan + **BT logistic** | Highest single PA-moving move; gated on H1, requires A4 + parity-check | 2-4 days (12 hr GPU + parity + export gotchas) | +2-4 pp (conditional on H1) |
| 10 | **I9** Born-Again Networks self-distill — **on post-E4 student** | Free ~50% of residual LUPI gap after E4 lands | 2-4 hr | +1-3 pp |
| 11 | **B3-SMOKE** Likert 200-track sanity (gates B3 full sweep) | Validates Likert path is not a second collapse; ~5 min | 5 min | Win/fail signal |
| 12 | **B3** MF as 4th juror via 5-bucket Likert path (if smoke passes) | Audio-modality juror; breaks σ²-floor on modality | 17-25 hr GPU | +2-4 pp |
| 13 | **B3-FALLBACK** MF-Think → text-LLM rerate (Path B if Likert collapses) | Backup per Pointwise Findings | +1 day | (replaces B3 if needed) |
| 14 | **B4** Sub-genre specialist local jurors (USER'S IDEA) | Attacks σ²-floor with new-information jurors; needs F3 (G2) | F3-dependent + sweep K=8/16/32 | +1-3 pp library separation |
| 15 | **C1+D2** Frontier-API jurors (Claude Haiku + GPT-4o-mini) + Snorkel | Cheap; D2 only useful after C1/B3/B4 land | $20 + 1 day code | +1-3 pp (with D2: +2-4 pp) |
| 16 | **I6** ListMLE as student-head ablation (separate from E4) | Optimizes the actual eval metric on the head | 1 day | +1-3 pp |
| 17 | **B1** Caption prompt redesign | Sharper teacher inputs cascade modestly to student | 10-15 hr GPU | +0.5-2 pp |
| 18 | **A5** Energy-based clip selection (combined with A1) | Helps long-quiet-intro tracks | 1 day | +1-3 pp |
| 19 | **E1** Multi-task teacher (BPM aux head) | Cheap once BPM accuracy validated; modest expected gain | 8-10 hr | +0-1 pp |
| 20 | **F4** Genre balance audit (then decide) | Cheap audit | hours | conditional |
| 21 | **I4** SimCLR-style contrastive head | Cheaper alternative to E4 (head-only) | 1-2 days | +1-2 pp |
| 22 | **I5** Triplet loss ablation | Worth ablating against MSE | 1 day | +0-2 pp |
| 23 | **I10** Decoupled KD (Zhao 2022) | Helps small student heads | 1 day | +0-1 pp |
| 24 | **B2** Multi-caption ensemble (gated on B6) | Only run if B6 finds caption instability | gated | gated |
| 25 | **I11** RINCE rank-supervised contrastive (Obsidian E3) | **Round-8 priority** (only path scaling to 16+ axes) | 2-week pilot | unknown |
| 26 | **F1** Full-length corpus rebuild (audio + labels) | Highest single-investment win | weeks + legal | +3-6 pp |
| 27 | **E5** MAEST/MULE encoder swap | Already TODO; verify by probe before committing | weeks | +3-6 pp (probe-conditional) |
| 28 | **E6** Multi-encoder ensemble | Post-MAEST | weeks | +3-5 pp |
| 29 | **B5** Re-run round-7.5 BT priors | 50 hr GPU for ~+0-1 pp; poor ROI | 50 hr GPU | +0-1 pp |
| 30 | **F2** Audio-side data augmentation | Robustness; small in-dist gain | days | +1-3 pp OOD |
| 31 | **I8** Mixup augmentation | Small ceiling | 1 hr | +0.5-1 pp |
| 32 | **H2/H4/H5** External benches (DEAM / MTG-Jamendo / MARBLE-2) | External validation | hours each | Diagnostic |

Items dropped: A2, A6, C2, C3, D1, E2, E3, E7, E8, G2, I1, I2, I3.

---

## 12. Recommended Round-7.7 plan

**Phase 0 (this week, ~5 days, no GPU):**
- J1+J2 — eval protocol + sweep infra (#1)
- B6 — caption stability check (#3)
- H1 — LUPI decomposition (#4)
- A4 + A1 — decoupled embeddings + clip-strategy realign (#6, #7)
- A3 — anchor-percentile display (#8)
- F4 — genre balance audit (#18)

**Phase 1 (~2-3 weeks, ~15 hr GPU + ~$20 API) — SEQUENTIAL, gated on H1:**

The reviewer pointed out E4/I9/I6 aren't independent. Sequencing:

1. **H1 (LUPI gap decomposition)** runs first; result gates E4
   expectation:
   - if T_audio < 0.80 → E4 expected +2-4 pp; full commit
   - if 0.80 ≤ T_audio < 0.84 → E4 expected +1-2 pp; reduced
     commit, prefer cheaper alternatives first
   - if T_audio ≥ 0.84 → encoder ceiling already close; defer E4
     in favour of E5 (encoder swap)
2. **F3 (anchor set)** in parallel with H1 (different work; human
   time vs GPU time).
3. **H3 (cross-library hand-eval)** as soon as F3 has any non-DnB
   genre slice ready. H3 becomes the **acceptance criterion for E4**:
   library separation must hold or improve, not just test PA.
4. **E4 (LoRA on MuQ-MuLan with BT logistic)** — the big move.
   Includes pre-train inspection + parity check + train + merge +
   re-export + post-train parity. Commit BT, defer ListMLE/RINCE
   ablations.
5. **I9 (BAN self-distill V18.2)** — runs ON the post-E4 student,
   not before. Closes ~50% of any remaining LUPI gap. ~hours of
   compute.

**Phase 2 (~3-4 weeks, ~30-50 hr GPU):**
- I6 — ListMLE as student-head ablation (separate from E4) (#11)
- **B3 SMOKE first** — 200-track Likert smoke before committing to
  full sweep (#13)
- B3 — MF Likert as 4th juror via 5-bucket path, IF smoke passes
- B4 — sub-genre specialist local jurors (uses F3 split into
  prompt + verification anchors) (#12)
- C1+D2 — frontier-API jurors + Snorkel (#14)
- B1 — caption prompt redesign (#15)
- A5 — energy-based clip selection (#16)

**Phase 3 (high-novelty / verification):**
- I4 — SimCLR head ablation against E4 (#19)
- I5 — triplet loss (#20)
- I10 — Decoupled KD (#21)
- **I11 — RINCE pilot (promoted from #23 to round-8 priority** —
  per reviewer + Obsidian E3, this is the only path that scales to
  16+ axes natively without per-axis BT pipelines, and the axis
  count is increasing as Mesh adds complexity)

**Phase 4+ (Lever 2 — already in TODO):**
- F1 — full-length corpus rebuild (#24)
- E5 — MAEST/MULE migration (#25, run ceiling probe first)

Each phase ends with eval-on-everything via J1.

---

## 13. Alternative: do nothing more

V18.1 hits G3 (0.81 vs 0.75), passes G4, G7, G8, G9, G10. The marginal
UX improvement from going 0.81 → 0.85 PA may not beat spending the
same engineering time on the broader Mesh roadmap (suggestion graph,
set analysis, native effects, USB sync).

**Defensible alternative path:**
1. **Phase 0 only** — captures most cheap wins + foundational
   measurement infrastructure.
2. **Defer Phase 1+** until either (a) MAEST encoder migration
   completes (V19), or (b) user feedback identifies a specific
   intensity-axis pain point that justifies further investment.

Should be a conscious choice. **Recommendation:** do Phase 0
unconditionally; evaluate Phase 1 after H1 (LUPI decomposition)
quantifies the ceiling.

---

## 14. Risks (existential reduced to noted)

### 14.1 Music Flamingo licensing — noted, not blocking

Per user direction (2026-05-09): MF is research-licensed, Mesh is AGPL,
considered compatible for the intended use case. Removed from
"existential risks" framing.

The relevant license interpretation (informational): NVIDIA OneWay
Noncommercial Academic — research use only. Mesh's AGPL distribution
inherits the research-use restriction; commercial use (selling Mesh
or paid licensing) would require either revisiting MF dependency or
pursuing commercial license from NVIDIA. For the foreseeable Mesh
roadmap (research / personal / open-source), MF stays.

### 14.2 Single-genre evaluation

H3 cross-library hand-eval addresses this. Not a deal-breaker, just a
known measurement gap.

### 14.3 σ²-floor consensus saturation

The current 3-juror DS panel is degenerate. Until B3 / B4 / C1 land
new-information jurors, claims about "3-juror consensus robustness"
overstate independence. The fix is in Phase 1-2.

---

## 15. Summary

Three foundational issues constrain V18.1:

1. **σ²-floor consensus saturation** — we have ~1.5 effective
   independent juror votes wearing a 3-juror costume. Fix:
   B3 (MF Likert via existing path) + B4 (sub-genre specialists)
   + C1+D2 (frontier-API + Snorkel).

2. **Encoder bottleneck** — MuQ-MuLan-512d under-represents
   intensity-discriminative features. The V18.1 MLP h=128/256/512
   sweep proved no more *student* capacity helps. Fix: **E4
   LoRA-fine-tune MuQ-MuLan** with rank-supervised loss (the path
   the existing 2026-05-07 research note explicitly recommends).

3. **Single-genre evaluation** — all metrics on one DnB library.
   Fix: H3 cross-library hand-eval + F3 anchor set + external
   benches.

**Highest expected-value Phase-1 combination (sequential, gated on H1):**
- H1 (LUPI decomposition) — gates E4 expectation
- E4 (LoRA on MuQ-MuLan with **BT logistic** committed) — encoder bottleneck
- I9 (BAN self-distill on post-E4 student) — close residual LUPI gap

I6 (ListMLE) moves to Phase 2 as a separate student-head ablation
(not bundled with E4). The reviewer correctly noted that bundling
ListMLE with E4 creates the loss-overlap trap: if E4 already trains
with rank loss, I6's marginal contribution shrinks toward 0.

**Combined Phase-1 expected gain: +3-5 pp test PA** (revised down
from +4-8 pp). Composing E4 + I9 isn't additive because both
affect the same encoder→student pathway; reviewer's analysis of
the overlapping mechanisms is correct.

Library separation target: clearing the +60 pp "excellent" bar
remains achievable but conditional on H3 (cross-library hand-eval)
showing the gain holds outside DnB.

The expensive infrastructural items (E5 MAEST, F1 full-length
corpus) remain the right destinations long-term but should not
block Phase 0/1 work.

---

## 16. Phase-1 failure modes (reviewer-predicted)

The most likely ways Phase 1 derails, ordered by probability per
reviewer's pass-2 analysis:

**Most likely (~40%): merged-LoRA ONNX has numerical drift from
PyTorch.** Cause: PEFT's `merge_and_unload()` does B@A matmul in
fp32 but the export monkeypatch reaches encoder modules by exact
attribute path (`mulan.mulan.audio.model.model`), and any wrapping
silently no-ops the patch. **Mitigation already added to E4 cost:**
half-day pre-train parity check (identity LoRA → re-export →
byte-equality). If parity fails, fix the export path before any
training.

**Second most likely (~30%): test PA improves on Deezer (+2-3 pp),
library separation regresses on DnB (-1-2 pp).** Cause: σ²-floor
saturation means consensus targets share systematic bias; LoRA
amplifies the bias rather than learning intensity. **Mitigation:**
H3 (cross-library hand-eval) is an explicit acceptance criterion
for E4 — not just a "nice to have" downstream check. If library
separation regresses, roll back E4 and prioritize σ²-floor
breakage (B3 / B4 / C1) before re-attempting LoRA.

**Third (~20%): everything ships and PA improves, but suggestion-
graph similarity quality degrades enough that users notice
"recommendations feel weirder".** Cause: encoder side-effect on
similarity geometry. **Mitigation:** A4 (decoupled embeddings) is
now an explicit prereq for E4. The post-LoRA encoder writes only
the intensity-side embedding column; similarity column keeps the
pretrained MuQ-MuLan encoder's output.

**Tail (~10%): everything works as predicted.** Optimistic.

These three failure modes share a common root: **without H1, A4,
and H3 in place before E4, we ship a regression.** That's why
Phase 0 + Phase 1 sequencing matters so much.
