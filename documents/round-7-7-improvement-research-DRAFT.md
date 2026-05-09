# Round-7.7 — Intensity Axis Improvement Research (DRAFT)

**Date:** 2026-05-09
**Branch:** `text-tower-aggression-axis`
**Status:** DRAFT — pending independent reviewer feedback
**Successor docs:** `round-7-6-pipeline-spec.md`, `round-7-6-training-log.md`, `round-7-6-v18-1-mlp-experiment.md`

This document enumerates and reasons about every plausible avenue for
improving the deployed intensity-axis pipeline beyond the current
V18.1 MLP + peak-clip-pool baseline. Each suggestion comes with:

- **Hypothesis** — what specifically would change.
- **Causal chain** — why this should help, traced through the pipeline.
- **Expected gain** — order-of-magnitude estimate (and what we expect not to gain).
- **Cost** — compute/wall/code complexity.
- **Risks** — what could go wrong, what the failure mode looks like.
- **Self-critique** — second-pass scepticism on whether the causal chain holds.

Final ranked recommendation in §11.

---

## 0. Current baseline (the thing we're trying to beat)

| Component | Current state |
|---|---|
| Corpus | 39 913 Deezer 30 s previews, scraped via everynoise (~2 100 DJ-relevant seed genres × 30 tracks/seed) |
| Audio encoder | **MuQ-MuLan-large 512d** (frozen, music-text contrastive pretraining) |
| Caption gen | **Music Flamingo 7B** (NVIDIA), `T=0.7, top_p=0.9, max_tokens=1024`, ~393 words avg |
| Captions per track | **1** |
| Caption embedder | bge-base-en-v1.5 (768d) |
| Caption struct tags | ~50 regex-mined multi-hot tags |
| Jurors | **3** text-LLM (Mistral-Small-3.2-24B AWQ, Nemotron-30B, Qwen3.6-27B), 20-bucket two-token logprob recovery |
| Pairwise juror agreement | Spearman ρ=0.93-0.96 |
| Consensus | Continuous Dawid-Skene EM, σ²-floor=0.01, nanmedian-init → all jurors equal-weight (1/3) |
| Teacher | MLP `1332 → 256 → 128 → 1`, MSE on consensus |
| Student (V18.1) | MLP `512 → 128 → 1` over MuQ-MuLan, FitNets + Hinton + LS distillation |
| Held-out test PA | **0.8113** (linear V18) → **0.8174** (MLP V18.1, +0.6 pp) |
| Teacher-student gap | **+12.87 pp** (irreducible: caption channel's privileged info) |
| Library hand-eval | **+55.8 pp** aggressive_mean − liquid_mean separation (post peak-clip-pool) |
| Inference embedding | 6 × 10 s clips, **project each through V18.1, take peak** |
| Inference compute | ~1.4 µs/track on CPU (V18.1 only); MuQ-MuLan ONNX is the ~1.7 s dominant cost |

**Key deltas to remember when evaluating each suggestion:**
- Teacher 0.94 PA proves the consensus + caption-channel ceiling is high
- Student 0.81 PA proves the MuQ-MuLan-only encoder is the bottleneck
- Library hand-eval +55.8 pp proves real-world ranking is good but not great
- Train-vs-deploy clip strategy is now **3×10s mean (training, internally to MuQ-MuLan)** vs **1-of-6 peak (deployment)** — see §1.3 for the nuance I previously got wrong

## 0.1 Important pipeline correction

I previously documented the train/deploy distribution mismatch as
"30 s contiguous window vs 6 × 10 s mean". Re-reading
`embed_corpus_mulan.py`, that's wrong. `MuQMuLan.from_pretrained()`
with `clip_secs=10` and a 30 s waveform internally **splits into 3 × 10 s
non-overlapping clips and averages**. So:

- **Training-time embedding**: 3-clip mean over the catchy 30 s window
  (Deezer's preview heuristic picks that window).
- **Deployment-time embedding** (post 2026-05-08): 1-of-6 peak across
  the whole track.

Deployment is now *more* peaky than training was. This actually matters:
the V18.1 model was trained on 3-clip-mean inputs, but at inference
sees 1-clip-pick inputs. Those are slightly different distributions in
embedding space. The library-eval improvement (+5.5 pp separation)
suggests it works in practice, but there's potentially more on the
table by **realigning train and deploy** — see suggestion A1.

---

## 1. Inference-time changes (no retraining)

These are the cheapest possible improvements: deploy-side code only,
zero new compute.

### A1. Realign deploy clip strategy with training

**Hypothesis.** Apply the same "find the catchy 30 s window, then 3 × 10 s
mean" pipeline at deploy time. Replace the current 6 × 10 s evenly-spaced
peak-pick with: (1) score 6 × 10 s clips, (2) take the highest-scoring
contiguous 30 s window (the 3 clips with max sum-intensity), (3) mean
their embeddings, (4) project through V18.1.

**Causal chain.** V18.1's weights were learned on 3-clip-mean inputs,
not on single-clip inputs. Single-clip pick is brittle: if the chosen
clip happens to land on a tom fill, a vocal hook, or a brief breakdown
inside the drop, we get a non-representative embedding. The 3-clip mean
of the densest 30 s should reproduce exactly what training saw and is
robust to within-30 s variation.

**Expected gain.** +1-3 pp library separation (closer to the +60 pp
"excellent" target). The fix is more about *consistency* than peak
reach — it should specifically help the mid-pack neurofunk tracks that
are now bouncing between p55 and p75 depending on which clip happens
to win.

**Cost.** ~30 lines in `inference.rs::analyze` and a re-analysis pass
on the user's library (~5 min wall on 909 tracks).

**Risks.**
- Could *under*-perform peak-clip on very long tracks where the catchy
  30 s isn't the actual peak (e.g., tracks with a 30 s ambient passage
  followed by a 10 s drop — the contiguous-window heuristic would
  prefer the longer ambient, not the brief peak).
- More compute per track at inference (still microseconds).

**Self-critique.** I'm framing this as "matches training" but actually
training was 3-clip-mean over Deezer's *catchy 30 s* — Deezer's
heuristic almost always picks a drop section. So the real training
distribution was "3-clip mean of drop". Deploy-time emulation needs
to find the drop, not just the densest 30 s. Densest-by-V18.1 is a
reasonable proxy for "drop" (high intensity → drop in DnB / techno),
but for genres where intensity is more uniform across the track
(e.g., minimal techno, ambient), the heuristic degenerates.
Probably still net-positive but the gain might be 0-2 pp not 1-3.

**Verdict (self):** Worth trying. Cheap to test, easy to revert.

### A2. Top-k peak pooling instead of single peak

**Hypothesis.** Score 6 clips, take **top-3 by V18.1 score, mean their
embeddings**, project. K=3 is a tunable knob.

**Causal chain.** Single-clip peak is high variance — one outlier clip
swings the whole track. Top-3 mean preserves the "use the loudest
section" semantic but smooths out the within-window noise. Bias-
variance trade-off classic.

**Expected gain.** +1-2 pp library separation. Same direction as A1
but achieved differently. Actually subsumes A1 if K is calibrated:
K=3 over a 6-clip-spanning track ≈ 30 s of the most intense 50 s.

**Cost.** 5 lines changed.

**Risks.** Picks a parameter (K) that has no closed-form right answer.
Need to sweep K = 1, 2, 3, 4 on the user's library and pick by
calibration-pair agreement. Risk of over-fitting K to the user's
specific library.

**Self-critique.** A1 is principled (matches training); A2 is a
heuristic that *might* approximate A1. If A1 lands cleanly, A2 is
unnecessary. Pick A1 over A2 if I had to choose one.

### A3. Library-anchored percentile calibration

**Hypothesis.** Add a thin wrapper that maps raw V18.1 scores to
percentiles **of a hand-picked anchor set** rather than percentiles of
the live library. The anchor set is a hard-coded list of ~30 tracks
spanning the spectrum (e.g., 5 ambient, 5 deep house, 5 techno, 5
DnB, 5 neurofunk, 5 hardcore). User's library tracks are then ranked
*against the anchors*, which means "p80" actually means "as intense as
the 80th percentile of the canonical reference set" rather than "as
intense as 80% of *this user's* library".

**Causal chain.** Currently a DnB-only library reads as "80% are
high-intensity" because the whole library lives in the high tail.
Anchor-relative percentiles would read more like "this DnB library's
median is at p70 of the global scale", which matches a DJ's intuitive
mental model.

**Expected gain.** Doesn't actually improve the model — it improves
the *display layer*. But for the user-facing experience this could be
the biggest UX win: percentile labels mean something stable across
libraries.

**Cost.** Embed 30 anchor tracks once, ship the 30 × 512 vector +
their scores in the binary. ~50 lines of Rust.

**Risks.** Anchor set selection is opinionated. If the anchors don't
cover an unusual genre well (e.g., hyperpop, ambient drone), library
tracks in that genre get weird percentiles.

**Self-critique.** This is the most high-leverage idea here — but it's
solving a *different* problem (UI calibration) not a *model* problem.
Worth doing in parallel with the model work, but doesn't move the
0.811 PA number at all.

### A4. Decouple intensity embedding from similarity embedding

**Hypothesis.** Store **two** embeddings per track in the DB:
(a) the peak-clip embedding for intensity scoring, and
(b) the mean-of-all-clips embedding for similarity / clustering /
PCA. The intensity head reads (a); the similarity index reads (b).

**Causal chain.** Currently both downstream uses (intensity, similarity)
share the same 512d. The peak-clip change improved intensity but
shifted similarity semantics: two tracks with similar drops but
different intros now look more similar than they used to. For DJ
mixing, "similar drop" might be what you want, or "similar overall
character" might be what you want — they're different questions, and
the right answer is "store both".

**Expected gain.** Doesn't move intensity PA. Improves
similarity-based suggestions in subjectively measurable ways
(needs hand-eval to confirm).

**Cost.** New DB column, second forward pass, ~50 lines of Rust.
Roughly doubles the per-track ML embedding storage (~4 KB/track ×
909 tracks = 4 MB extra, irrelevant).

**Risks.** Increases code complexity at the DB schema layer.
Migration: existing tracks need to be re-analysed to fill the
new column.

**Self-critique.** Solid. The TODO already flags this. Not an
intensity-PA win but the right architectural call. **Combine
with A1 or A2** rather than a separate item.

### A5. Energy-prefiltered clip selection

**Hypothesis.** Before running MuQ-MuLan on 6 clips, compute cheap
energy stats (RMS, spectral flux, onset density) on a 1 s sliding
window across the whole track. **Pick the 6 clip locations to centre
on the top-6 energy peaks**, not evenly-spaced.

**Causal chain.** Right now we sample 6 evenly-spaced clips and *then*
pick the most intense. For tracks where the drop is short relative to
the track length (e.g., 4 min track with a 20 s drop near the 60% mark),
even spacing might *miss* the drop entirely — none of the 6 clips
land on it. Energy-based pre-selection would always include the drop.

**Expected gain.** +1-3 pp library separation, mostly on tracks with
long quiet intros (jungle, drum-and-bass with breakbeat intros, post-
rock-style buildups).

**Cost.** ~50 lines of Rust + librosa-equivalent in Rust (or precompute
during the existing mel-spec pass — RMS and onset density are cheap from
mel data we already have). ~10-20 ms/track for energy computation
(rounding error vs the ~1.7 s MuQ-MuLan cost).

**Risks.**
- Energy ≠ intensity. A track with a loud lo-fi distorted intro might
  read "high energy" via RMS but isn't peak-time intense. The pre-filter
  needs to be loose enough that it doesn't actively *exclude* the drop.
- Onset density picks up busy hi-hats, which appear in non-drop sections
  too.

**Self-critique.** If the goal is "find the drop", spectral flux + RMS
in the kick frequency range (50-100 Hz) is a much better proxy than
broadband energy. This idea is sound but needs care in implementation.
Combined with A1 or A2 (use the energy peak as the centre, not an even
slot), it could be quite effective.

### A6. Multi-scale clip stack

**Hypothesis.** Run MuQ-MuLan on **multiple clip lengths** (5 s, 10 s,
20 s) at the same peak location, and combine. The shorter clip
captures fine timbre; the longer one captures macro structure.

**Causal chain.** MuQ-MuLan is trained on 10 s clips, so 5 s and 20 s
are off-distribution for the encoder. But the resulting embeddings
might still carry complementary information; concat → 1536d → V18.1
trained on 1536d.

**Expected gain.** Speculative. Maybe +1-2 pp; could be 0.

**Cost.** Requires retraining V18.1 (different input dim), and 3× the
inference cost.

**Risks.** Off-distribution input to MuQ-MuLan likely degrades the
embedding quality. The V18.1 retrain isn't free either.

**Self-critique.** Probably not worth it. The encoder is *trained* for
10 s; deviating from that without good reason is just adding noise.
Drop this from the ranked recommendation.

---

## 2. Caption-side improvements (Music Flamingo / prompts)

These require re-running the MF caption sweep but not re-training the
audio encoder.

### B1. Caption prompt redesign for intensity-relevant aspects

**Hypothesis.** Rewrite the MF prompt to elicit information specifically
useful for intensity discrimination: drop architecture, bass weight,
distortion character, vocal aggression, BPM perception, peak-time
suitability. Current prompt is generic ("Describe this music clip in
rich detail. Cover the instrumentation, production style…").

**Causal chain.** The text-LLM jurors then score the caption. If the
caption explicitly mentions "the drop hits at 90 seconds with a
distorted reece bass at 175 BPM, full peak-time intensity", the juror's
20-bucket judgement will be much sharper than if the caption is
"upbeat track with electronic instrumentation". Right now we let MF
choose what to describe; we could ask for what we need.

**Specific prompt sketch:**
```
You are a careful music analyst. Describe the audio specifically for
DJ-set intensity assessment. In your description, cover ALL of:
1. Genre and sub-genre
2. Bass character (sub, reece, distorted, etc.) and weight
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

**Expected gain.** +2-5 pp test PA. The juror agreement (ρ=0.93-0.96)
suggests current jurors are interpreting captions consistently;
making the captions more discriminative should propagate downward.

**Cost.** 6 hr re-run of the MF caption sweep (39 913 tracks × 0.5 c/s).
0 GPU-time on jurors (re-running them is fast: ~3 hr × 3 jurors).
Then re-run consensus + teacher + student: ~30 s.

**Risks.**
- More-structured prompts can cause MF to **template-fill** rather than
  truthfully describe. If the audio doesn't have e.g. distortion, MF
  might still mention "no distortion present" rather than skipping —
  a kind of hallucinated negative. Need to validate on a 100-track
  sample first.
- Longer required output → more tokens → slower MF inference. Could
  push the 6 hr to 10-15 hr.

**Self-critique.** This is one of the strongest items. Captions are the
*only* path information takes from audio → semantic → LLM-judge. Better
captions = better consensus = better teacher = (probably) better student.
The bottleneck of the student is the audio encoder, not the captions —
but the *teacher* is bottlenecked by caption quality, and a better
teacher means a better distillation target. The +12.87 pp T-S gap might
shrink modestly if the teacher's semantic understanding sharpens.

### B2. Multiple captions per track (T-ensemble)

**Hypothesis.** Generate **3 captions per track at T=0.7** (or via 3
different prompts), embed each, average the bge embeddings →
caption_emb is more stable. Score each caption with each juror, average
the 3 scores per juror → juror score is more stable.

**Causal chain.** MF at T=0.7 is stochastic — re-running gives a
different caption with different specific words. The downstream juror
gets to see an artefact of MF's sampling, not a stable description of
the audio. Averaging across 3 captions de-noises the MF stochasticity.

**Expected gain.** +1-2 pp test PA, more if MF caption stability is
genuinely poor. The spec mentions a 50-track repeat-stability check
should give cosine ρ > 0.85; we never actually ran that, so we don't
know how unstable the captions are.

**Cost.** 3× MF caption sweep (18 hr) + 3× juror cost (~9 hr × 3 jurors,
much of which is parallel). ~$0 in API cost since we use local + Spark.

**Risks.** Most of the gain might already be captured by the 3-juror
ensemble (each juror reads the same caption with their own bias). If
caption stability is high, 3-caption ensemble adds nothing. Also: 3×
the storage and 3× the read/parse time at consensus stage.

**Self-critique.** B1 is much higher leverage than B2 for the same
compute. **Drop B2 unless we measure caption stability first** and
find it's genuinely bad. Or: just run the stability check (B6) and
decide based on that.

### B3. Audio-LLM juror (MF Likert direct)

**Hypothesis.** Add Music Flamingo *itself* as a 4th juror, but instead
of generating prose then having a text-LLM rate the prose, have MF
**directly emit a 0-19 intensity rating** via the same two-token logprob
trick. MF sees the audio; text-LLM jurors only see captions.

**Causal chain.** Captions lose information (MF picks what to describe).
Direct audio → score skips the lossy intermediate. The "MF Likert"
intensity was actually in the original spec (S6c) but was excluded from
V18 because it only covered 200 tracks; full-corpus would take 13 hr
per the spec. With MF perf already tuned to 2 c/s, that's 5.5 hr —
manageable.

**Expected gain.** +2-4 pp test PA. The MF Likert source becomes the
4th jury source, satisfying the original G5 "≥ 4 sources" target.
Pairwise ρ with the text-jurors should be lower (different modality
entirely), which **increases** consensus quality (less correlated
sources = more independent information).

**Cost.** 5.5 hr MF run + 30 s consensus rerun. Need to write the
20-bucket prompt for MF specifically (it's an audio-LM, not a chat-LLM,
so the prompt format differs slightly).

**Risks.**
- MF was trained as a captioner; whether it's well-calibrated on
  20-bucket Likert via two-token logprobs is unverified. The spec's
  "MF Likert" reference is from earlier, lower-resolution rating;
  20-bucket might be too fine-grained.
- The 20-bucket logprob trick relies on the model emitting digits as
  single tokens. MF's tokenizer might split "13" into "1" "3" or
  "13" or some weird split — need to verify.

**Self-critique.** This is the strongest single addition. It satisfies
G5 (4 sources, properly heterogeneous in *modality*) AND closes a
real information gap (audio-grounded judgement vs caption-grounded).
The MF Likert smoke test on 200 tracks already demonstrated technical
viability — scaling is just compute.

### B4. Sub-genre-specialist local jurors

**Hypothesis.** Train (or fine-tune via in-context examples) **per-genre
juror prompts** that know the canonical intensity benchmarks for that
sub-genre. E.g., a "DnB-specialist" juror knows that "neurofunk" is
peak-time and "liquid" is mid-tier; a "techno-specialist" knows that
"industrial techno > minimal techno > deep house". At consensus time,
weight each juror by the genre-cluster membership of the track (from
caption-emb K-means or directly the sub-genre tag in the caption).

**Causal chain.** General-purpose LLMs have weak intra-genre
discrimination — they correctly cluster genres but can't distinguish
"hardstep DnB at 175" from "neurofunk DnB at 174". A specialist with
in-context examples ("Black Sun Empire 'Lights Out' = 17/19, Random
Movement 'Slinkystink' = 5/19") would have much sharper sub-genre
calibration.

**Expected gain.** +2-4 pp library separation, mostly on the per-genre
ranking inside DnB (which is what the user's library actually cares
about). Test PA on the global Deezer corpus might not move much
because the corpus is dominated by genre-boundary discrimination, not
intra-genre.

**Cost.** Build a few-shot example bank per genre cluster (~50 anchor
tracks/genre × 5 genres = 250 hand-rated tracks). Then a 3-4 hr juror
re-run with longer prompts (more in-context examples).

**Risks.**
- Genre detection is itself error-prone; sending a track to the wrong
  specialist gives worse ratings than sending it to the general juror.
- Hand-rating 250 anchors is human-time expensive. Could bootstrap
  from existing aggressive_inspect output (the cherry-picked
  aggressive/liquid lists are essentially anchors), but those are only
  for DnB.
- Specialists might over-anchor on their examples and ignore genuine
  variation. Random Movement's "Slinkystink" might score 5; a similar
  Random Movement track with a heavier drop might still score 5
  because "Random Movement = liquid" is too anchored.

**Self-critique.** The user explicitly mentioned this; it's a direction
worth taking seriously. But the design needs care:
- Don't *gate* between specialists — *blend* them via soft-genre weights
  (e.g., a track classified 70% DnB / 20% techno / 10% drum-and-bass
  gets a weighted average of the 3 specialist scores).
- Use the specialist as one *additional* juror on top of the general
  3, not as a replacement.
- Anchors should come from a curated peer-reviewed list, not the
  user's library (G2 concern).

This is a real direction but has a lot of moving parts. Lower priority
than B1, B3 in my view.

### B5. Pairwise / tournament-style juror

**Hypothesis.** Instead of pointwise rating, present jurors with
**pairs** of caption + caption ("which describes a more intense
track? A or B?") and aggregate via Bradley-Terry. Round-7 already did
this with audio (Qwen3-Omni N-way tournaments) and got reasonable
results.

**Causal chain.** Pointwise rating asks the LLM "what number is this?"
which requires the LLM to have an *internal calibration scale*. Few
LLMs have stable calibration. Pairwise comparison is much easier:
"is A more intense than B?" leverages relative rather than absolute
judgement. Aggregating many pairs via BT recovers an absolute ranking
without needing LLM calibration.

**Expected gain.** +2-4 pp test PA. Round-7's pairwise BT on Qwen3-Omni
worked well; we have evidence the methodology scales.

**Cost.** Need to plan pair dispatches. Naive all-pairs is O(N²) =
1.6B comparisons — infeasible. Smart pair selection (each track in
~30 pairs against ~30 tracks of similar BT-prior intensity) gives
~1.2M pairs. At 2 c/s that's ~7 days per juror. Heavy.

**Risks.**
- Pair selection has to be done carefully or the BT scores converge
  poorly (low-information pairs dominate).
- BT assumes a single underlying scalar; if intensity isn't truly
  unidimensional, BT compresses sub-axes.

**Self-critique.** We *already have* round-7.5 BT priors on 15 314
tracks. The reason they're not in V18's consensus is the coverage
asymmetry (38% vs 100%). Re-running BT on the new 24 599 tracks
brings them to 100% coverage, no consensus instability — but it's
~50 hr on Qwen3-Omni GPU. The value depends on whether we believe
audio-grounded BT > caption-grounded pointwise. Probably yes for
intra-genre discrimination; probably even or worse for cross-genre.

Plausible plan: re-run round-7.5 BT on new 24 599 tracks, add as a
4th juror. Less novel but uses existing infrastructure. Defer to
after MF Likert (B3).

### B6. Caption stability sanity check

**Hypothesis.** Re-caption 50 random tracks at T=0.7 and measure
caption-embedding cosine ρ between paired runs. Spec calls for ρ > 0.85.
We never actually ran this.

**Causal chain.** Pure validation — doesn't directly improve anything.
But if ρ is e.g. 0.6, that's a smoking gun for B2 (multi-caption
ensemble). If ρ is 0.95, B2 would help nothing.

**Expected gain.** Information, not PA. Tells us whether B2 is worth
running.

**Cost.** 50 captions × 1.4 s per = 1.5 min. Embed both runs, compute
pairwise cosine. Five lines of Python.

**Self-critique.** Should have been done on day 1. Trivial; do it.

---

## 3. Juror panel improvements

### C1. Add 2-3 more jurors

**Hypothesis.** Expand the 3-juror panel to 5-6 jurors. Specifically:
- Llama-3.1-70B (instruct, AWQ on Spark) — different lineage from
  Qwen/Mistral/Nemotron
- GPT-4o-mini via API — frontier-lab calibration ($30 for 40k
  captions)
- Claude Haiku 4.5 via API — different pretraining
- A reasoning model (DeepSeek-R1-Distill or QwQ-32B) — explicit
  chain-of-thought might help intensity calibration

**Causal chain.** Per Verga 2024 (PoLL), juror diversity matters more
than juror size. Adding a frontier-lab API model to the panel injects
genuinely different inductive biases (RLHF training, different
distillation strategies). The σ²-floor saturation we observed (all 3
jurors at floor) means **the EM can't distinguish reliability levels**;
adding heterogeneous jurors could break that saturation in either
direction (some jurors clearly more reliable, some less), giving the
consensus more information.

**Expected gain.** +1-3 pp test PA. Diminishing returns past ~5 jurors
in the literature.

**Cost.** Llama-3.1-70B: ~3 hr remote. GPT-4o-mini: ~$30, ~30 min API.
Claude Haiku: ~$50, ~30 min. Total: ~$80 + 4 hr.

**Risks.** Hosted-API jurors mean external dependency and ongoing cost
for re-runs. Also: if the new jurors agree perfectly with the existing
3, gain is 0.

**Self-critique.** The cheapest way to get marginal gains. The
*specifically valuable* addition is the reasoning model — DeepSeek-R1
distilling its scratchpad into the rating could surface genre-specific
intensity factors that the others miss. Mid-priority.

### C2. Add MF Likert as juror (B3 again, here for ranking)

Already covered in B3. Listed here so the "more jurors" bucket reads
consistently — B3 is the highest-impact "add a juror" item, not C1.

### C3. Reasoning-juror with rubric

**Hypothesis.** For one of the jurors, replace the current 20-bucket
prompt with a structured reasoning prompt: "Step 1, identify the
genre. Step 2, recall the intensity range for that genre. Step 3,
rate where this specific track sits in that range. Output: <number>."
A reasoning-style chain-of-thought rated by digit logprobs.

**Causal chain.** Forces the juror to externalise its reasoning,
which (per CoT literature) often improves accuracy. Specifically
helps with intra-genre discrimination, which is where current jurors
are weakest.

**Expected gain.** +0.5-2 pp. Modest but cheap.

**Cost.** Re-run one juror with new prompt, ~3 hr.

**Risks.** Reasoning models can wander; need to verify the final
digit-output is well-calibrated.

**Self-critique.** Subset of C1 ("add a reasoning model"). If we add
a reasoning model anyway via C1, this is automatic. Don't list
separately.

---

## 4. Consensus / aggregation

### D1. Multi-axis consensus (intensity + complementary axes)

**Hypothesis.** Rate jurors on **multiple axes** (intensity, density,
darkness, BPM, polish/lo-fi), then learn **which axes correlate with
intensity** via a regression on a small held-out set. Dawid-Skene runs
per-axis. Final intensity is a weighted combination of axes that the
data tells us are most predictive.

**Causal chain.** Right now we collapse "DJ intensity" into a single
1d rating. But "intensity" is multi-faceted: a fast bouncy hardgroove
techno track and a slow oppressive doom track are both "intense"
in different ways. Multi-axis consensus respects the structure.

**Expected gain.** Speculative. +1-3 pp test PA *if* the held-out
regression learns useful weights. Could be 0 if intensity really is
unidimensional in the data.

**Cost.** 5 axes × N tracks × 3 jurors = 15× the juror compute.
~45 hr across the panel. Plus a held-out anchor set for axis weight
calibration.

**Risks.** Axis selection is opinionated. If we miss an important
axis (e.g., "menace"), we lose information. Also: 5x cost for
an uncertain gain.

**Self-critique.** Round-7.5 already did multi-axis (16 BT axes). The
results were okay but not transformative. Probably not worth re-doing
unless we find a specific axis the current single-target is missing.

### D2. Snorkel-style learned label model

**Hypothesis.** Replace the continuous Dawid-Skene EM with Snorkel's
LabelModel (handles multi-source weak supervision properly, including
correlations between sources). The current EM assumes sources are
conditionally independent given z, which is a strong assumption
violated by all 3 jurors reading the same caption.

**Causal chain.** When sources share a latent (the caption), their
errors correlate. EM assigns them inflated weight (treats correlated
agreement as confirmation). A correlated-source-aware label model
(Snorkel, MeTaL, or a Bayesian network with explicit correlation
structure) would deflate the trio's joint weight, leaving room for
a heterogeneous source (e.g., MF Likert from B3) to contribute more.

**Expected gain.** +0-2 pp depending on how much the 3 jurors actually
correlate. Pairwise ρ=0.93-0.96 means **highly** correlated, so the
deflation could be substantial.

**Cost.** Replace `aggregate_consensus.py` with a Snorkel call. One
afternoon of work + verifying the label-model output is sane.

**Risks.** Snorkel is heavier machinery; needs more data to fit
correlation structure. With only 3 sources, the correlation model
might be under-specified.

**Self-critique.** The σ²-floor saturation in the current EM
*already* normalizes the panel to 1/3 each — so in practice the
correlation issue is moot for the current panel. Snorkel becomes
relevant when we add B3 (MF Likert) or C1 (more jurors) — at that
point a proper correlation-aware model matters. **Defer until after
panel expansion.**

---

## 5. Teacher and student model

### E1. Multi-task teacher

**Hypothesis.** Teacher predicts intensity AND auxiliary targets
(BPM bin, danceability proxy, valence proxy, genre cluster). The
auxiliary heads regularize the shared backbone, forcing the
penultimate to encode features useful for the auxiliary tasks too.
At distillation, only the intensity head is distilled — but the
shared penultimate that the FitNets loss matches is now richer.

**Causal chain.** A representation that has to predict BPM is
forced to encode BPM, which is correlated with intensity in some
genres (DnB: high BPM = high intensity; deep house: medium BPM =
medium intensity; ambient: BPM is irrelevant). The student then
has access to BPM-aware representations through FitNets matching.

**Expected gain.** +1-3 pp test PA. Multi-task learning consistently
helps small-data regimes. 39 913 isn't tiny but isn't huge either.

**Cost.** Need labels for auxiliary tasks. BPM is free (estimate
from audio). Danceability/valence: use Spotify Audio Features API
(free, available for any track with ISRC). Genre cluster: already
have it from caption-emb K-means.

**Risks.** Bad auxiliary labels (Spotify's danceability is opaque)
might add noise.

**Self-critique.** Strong direction. BPM is the obvious starter
auxiliary — it's directly available, well-defined, and intuitively
related to intensity. Adding 2-3 auxiliary heads over BPM, danceability,
and genre cluster is cheap (<1 hr work, no GPU rerun) and almost
certainly helps. Move to high-priority.

### E2. Ordinal regression head

**Hypothesis.** Replace MSE on intensity with an ordinal regression
loss (cumulative-link, e.g. POLR). Each track's prediction is
a sequence of K-1 binary decisions: "is intensity > 0.05?",
"is intensity > 0.10?", etc.

**Causal chain.** MSE treats intensity as continuous Euclidean. Ordinal
regression respects the discrete nature of the consensus targets
(20 buckets) and handles the rank semantics natively. Better
calibration around the tails (very low / very high tracks).

**Expected gain.** +0-2 pp. Modest. The current MSE works because
the target *is* numeric; ordinal mostly helps with monotonic-but-non-
linear noise.

**Cost.** ~50 lines of training code change.

**Risks.** Can hurt if the target is genuinely continuous (not
ordinal). Likely net-positive but small.

**Self-critique.** Probably not worth the complication. Real intensity
ranges over a continuum; the 20 buckets are an artefact of how we got
labels, not of the underlying truth. Skip.

### E3. Quantile head (uncertainty-aware)

**Hypothesis.** Predict P10, P50, P90 of intensity instead of a single
point. Use a pinball loss (or check loss). At inference, P50 is the
display value; (P90-P10) gives a confidence interval.

**Causal chain.** Different tracks have different intrinsic intensity
ambiguity (a clear neurofunk drop vs a genre-bending experimental
piece). The current model emits a single number for both, hiding the
fact that one is well-determined and the other isn't. Quantile heads
expose this.

**Expected gain.** Doesn't move PA. Adds the option for uncertainty-
aware downstream decisions ("don't blend a high-uncertainty track into
a peak-time slot").

**Cost.** Modify student to 3 outputs (or K outputs at K quantiles),
modify loss. ~1 hr.

**Risks.** Triples the head parameters but trivially more compute.
Need a UI surface for the uncertainty.

**Self-critique.** Excellent for *future* work but doesn't move the
current bottleneck. Defer until after the bigger PA-moving changes
land.

### E4. Encoder fine-tuning (LoRA)

**Hypothesis.** Don't just train a probe over the frozen MuQ-MuLan;
**fine-tune MuQ-MuLan with LoRA adapters** on the consensus targets.
The encoder learns intensity-discriminative features that aren't
in its pre-training.

**Causal chain.** MuQ-MuLan was trained for music-text similarity, so
its embedding geometry is optimized for "similar genre/mood near each
other in cosine". Intensity is not the dominant axis. LoRA adapters
let us re-shape the embedding so intensity *becomes* a more dominant
axis, without losing the music-text knowledge entirely.

**Expected gain.** +3-6 pp test PA. The G6 distillation gap is
fundamentally because MuQ-MuLan's embedding under-represents intensity;
LoRA fine-tuning is the cheapest path to making MuQ-MuLan know about
intensity.

**Cost.** GPU training. MuQ-MuLan is 700M params, but LoRA adds maybe
20M trainable. ~3-6 hr on the 5090. Then re-export ONNX with the LoRA
weights baked in; ONNX export is non-trivial (need to verify the LoRA
adapters merge cleanly into the original layers).

**Risks.**
- Catastrophic forgetting: if we over-train, MuQ-MuLan forgets its
  music-text similarity, and our suggestion / similarity features
  degrade.
- ONNX export complexity. The LoRA adapters need to be merged back
  into the base weights for ONNX; need to verify this gives identical
  outputs in PyTorch and ONNX.

**Self-critique.** Probably the highest-impact single intervention
on the audio-encoder side, short of swapping to MAEST. It also keeps
MuQ-MuLan's similarity geometry mostly intact (good for A4 / similarity
use). Worth deep evaluation.

### E5. Encoder swap (MAEST-768d / MULE-1.7k+d)

Already in TODO and `documents/embedding-models-research.md`. Highest
expected gain but most expensive. Listed here for completeness; not
the focus of this round.

### E6. Multi-encoder ensemble

**Hypothesis.** Concat MuQ-MuLan(512d) + MAEST(768d) + handcrafted
features (BPM, RMS, spectral centroid, onset density, key, tempo
stability, dynamic range, loudness LUFS) → ~1300d student input.

**Causal chain.** Each encoder captures different aspects. MuQ-MuLan
is music-text; MAEST is genre/tagging-focused; handcrafted features
are interpretable and complementary. The student learns which
features matter via the linear combination.

**Expected gain.** +3-5 pp test PA. Ensembles consistently help.

**Cost.** Both encoders need to run at inference (2× CPU cost). Need
to compute handcrafted features at inference (cheap, ~50 ms/track).
ONNX deployment of MAEST is needed (already in the embedding-models
research plan).

**Risks.** Doubles inference compute. The G9 budget (100 ms / 1000
tracks) is for the linear probe only — the encoder costs aren't in
G9. Per-track inference would go from ~1.7 s (MuQ-MuLan) to ~3.5 s
(both encoders + handcrafted), still acceptable for batched analysis
but less so for interactive use.

**Self-critique.** Strong direction once MAEST migration lands.
Combine with E4 (LoRA fine-tune both encoders). Highest-ceiling
plausible improvement on the audio side.

### E7. Stem-separated encoding

**Hypothesis.** Run a stem separator (Demucs) → 4 stems (drums, bass,
vocals, other). Encode each stem separately with MuQ-MuLan → 4 × 512d.
Concat or sum → 2048d / 512d student input.

**Causal chain.** Intensity in different genres comes from different
stems. Hardcore intensity = drums + bass; metal intensity = vocals
+ guitar (in "other" stem); ambient intensity = "other" only. Per-stem
encoding lets the model attend to the right stem per genre.

**Expected gain.** Speculative. +2-4 pp possibly.

**Cost.** Demucs is slow: ~10 s/track on CPU, ~1 s on GPU. For 39 913
tracks that's ~5 hr GPU + ~10× current MuQ-MuLan inference cost.
At deployment: ~10 s extra per track. Painful.

**Risks.** Demucs introduces artefacts; embedding artefacts is bad.
Also: 4× the storage per track if stems are kept.

**Self-critique.** Cool direction but expensive. Defer.

### E8. Distillation method upgrade (CRD)

**Hypothesis.** Replace FitNets + Hinton with Contrastive
Representation Distillation (CRD, Tian 2019). CRD pulls student's
representation toward teacher's *for the same track* and pushes apart
*across different tracks*, preserving the teacher's geometry rather
than just point-matching.

**Causal chain.** FitNets matches penultimate features pointwise, which
under-uses the inter-track structure. CRD encodes "track A is more
similar to track B than to C" (per the teacher) into the student's
geometry. Better global structure → better generalization.

**Expected gain.** +0-2 pp test PA. CRD usually helps in
classification; for regression it's less established.

**Cost.** Complete rewrite of distill_v18_student.py loss.
~1 day of work.

**Risks.** CRD has more hyperparameters; tuning takes effort.

**Self-critique.** The student is already saturated against the audio
encoder (the MLP h=128/256/512 sweep showed +0.07 pp from h=128 to
h=512). CRD is unlikely to extract more from the same encoder than
the current setup did. Drop unless we have a specific hypothesis
about FitNets failing.

---

## 6. Corpus-side improvements

### F1. Full-length tracks vs 30 s previews (TODO already)

**Hypothesis.** Replace the 30 s preview corpus with full-length
tracks (or longer 90-120 s previews). Train MF caption + audio embed
on the full track, not just the catchy 30 s.

**Causal chain.** Already extensively documented in TODO and
training-log. The 30 s preview is Deezer's catchiest 30 s — typically
the drop. Training on drops only means the model knows what drops
look like but doesn't know how to handle quiet sections. At deployment
on a full 4-min track, the model sees 6 clips, most of which are NOT
drops (5 out of 6 in many cases).

**Expected gain.** +3-6 pp test PA. The biggest single corpus-side
win.

**Cost.** Heavy. Need to source full audio (Spotify offers no API
for audio download; Deezer non-preview is paid; rip own collection;
rip libre.fm; YouTube-dl). Re-run MF caption sweep on longer audio
(or stratified samples). Re-run audio embedder on longer audio.
Estimated 50-100 hr GPU + significant data engineering.

**Risks.** Audio sourcing is legally complex. Need to identify a
licit corpus before pulling down 40 k tracks.

**Self-critique.** Already in TODO, with a good cost analysis there.
This is the right "if money were no object" direction. Realistically
defer until V18.1 saturates.

### F2. Data augmentation in training

**Hypothesis.** At training time, augment audio inputs:
- random clip selection from longer (>30s) tracks
- pitch-shift ±2 semitones
- time-stretch ±10%
- additive noise
- spec-augment on mel
Then re-embed with MuQ-MuLan, target: same consensus intensity.

**Causal chain.** Current model is brittle to small audio variations
(mastering version, EQ, etc.). Augmentation makes the embedding more
robust to these surface differences and forces the model to focus
on intensity-relevant invariants.

**Expected gain.** +1-3 pp test PA on out-of-distribution tracks.
Less on the held-out test set (which is also from Deezer previews,
so same distribution).

**Cost.** Need to re-run MuQ-MuLan on the augmented audio (heavy).
Or: do augmentation in mel-spec space (cheap) and live with that
not being audio-faithful.

**Risks.** Augmentation can shift the encoder's distribution in
unintended directions. Pitch shift is dicey because pitch is
genre-correlated.

**Self-critique.** Useful for robustness; doesn't move the test PA
much because the test set is in-distribution. Lower priority.

### F3. Curated anchor / hard-example mining

**Hypothesis.** Build a curated set of ~500 hand-validated tracks that
span the intensity spectrum and the major sub-genres, with consensus
labels confirmed by at least 2 humans (or consistent across all jurors).
Use these as a *strong-anchor* held-out test set + as training-time
oversampling.

**Causal chain.** Currently we trust the 3-juror consensus implicitly.
Some of those consensus labels are wrong — e.g., a track where MF
captioned the breakdown instead of the drop, all 3 jurors then under-
rate it. Hand-curated anchors catch these errors and provide a
sharper held-out signal.

**Expected gain.** Doesn't move training PA but gives much sharper
diagnostic. The current 3985-track held-out set has the same noise
floor as training; an anchor set has a lower noise floor, so PA on
anchors is a tighter measurement of true model quality.

**Cost.** Hand-rate 500 tracks (~10 hr human time). Or bootstrap from
the user's existing aggression_inspect output.

**Risks.** Hand ratings introduce a single-rater bias. Need 2-3
raters or a clear rubric.

**Self-critique.** Should do this regardless of other improvements.
Provides the foundation for measuring all subsequent gains. Move to
high-priority.

### F4. Genre balance via smarter scraping

**Hypothesis.** Current corpus is genre-imbalanced — 2109 seed genres
× 30 tracks/seed but seeds have wildly different popularity. Some
DJ-relevant sub-genres might be under-represented. Compute per-genre
counts, identify under-served sub-genres, scrape more.

**Causal chain.** A model trained on imbalanced data will be best at
the dominant genres. If the user's library is in an under-served
genre, model performance there is worse.

**Expected gain.** +0-2 pp depending on how skewed the corpus is
and what genres the user cares about.

**Cost.** Audit existing corpus genre distribution, scrape more.
Probably ~10 hr.

**Risks.** Adding more low-quality or off-genre tracks could hurt.

**Self-critique.** We don't actually know the corpus genre
distribution. Worth auditing first; might not need any action.

---

## 7. Similarity / inference-side use

### G1. Decoupled similarity embedding (= A4)

Already covered.

### G2. Similarity-aware fine-tuning

**Hypothesis.** Add a contrastive auxiliary loss to the student:
"tracks with similar consensus intensity should have similar
embeddings". This co-shapes the embedding for both intensity and
similarity-by-intensity.

**Causal chain.** Currently the embedding is shaped only by predicting
intensity (a scalar). A contrastive loss shapes it by the *relative*
ordering of intensities, which is what similarity downstream uses
(via PCA + cosine).

**Expected gain.** Improves similarity-by-intensity. Doesn't move
intensity PA much.

**Cost.** ~1 day to set up the contrastive loss properly.

**Risks.** Could push the embedding away from MuQ-MuLan's original
geometry, hurting genre-similarity.

**Self-critique.** Niche. Skip unless similarity-by-intensity is a
specific user pain point.

### G3. Per-collection slight calibration

**Hypothesis.** Round-7.7 from the spec — not in V18 scope but worth
noting. Each user could rate ~20 pairwise comparisons; map their
preferences to a per-user bias and (optional) scale on top of V18.1.
The general-purpose axis stays canonical; a thin user override
applies on display.

**Causal chain.** Some users perceive intensity differently
(producer's perception vs DJ's perception vs listener's). A user-
specific affine fix doesn't change the model but matches the
display to their expectation.

**Expected gain.** Per-user UX improvement, not a model improvement.

**Cost.** UI work for the pair-rating loop. ~1 week of UX/eng.

**Risks.** Conflicts with the "general purpose, no per-user fitting"
constraint. Spec says it's out of V18 scope but optional Round-7.7.

**Self-critique.** Out of this round's scope per spec G2. Noted only
to avoid the appearance of having missed it.

---

## 8. Calibration / evaluation

### H1. More user calibration pairs

Currently 18 pairs → 72.2% agreement. With 100+ pairs we could
distinguish whether the model has stable per-user disagreement
patterns or is random-noise-fluctuating. Cheap, just needs UI for
rating pairs.

### H2. DEAM external benchmark

In-spec optional. Provides an external check that V18.1's intensity
correlates with academic-ground-truth arousal. Also enables isotonic
calibration of V18.1 outputs to match DEAM's range (could combine
with A3 anchor calibration).

### H3. Cross-library evaluation

Once we have the calibration UI, multiple users rating different
libraries gives a much sharper picture of generalization. Currently
we're benching against one user's DnB library.

### H4. MTG-Jamendo / FMA external validation

Free open datasets with multi-tag annotations (mood, genre, instrument).
Compute V18.1 score on tracks tagged "aggressive" / "dark" / "energetic"
vs "calm" / "soft" / "peaceful" — separation should be large.

---

## 9. Novel methodology directions

### I1. Audio-text contrastive pretraining on intensity-annotated data

**Hypothesis.** Pretrain (or further-pretrain) the audio encoder via
a CLIP-style loss on (audio, intensity-keyword) pairs. Get audio
intensity-keyword pairs from large-scale music tagging datasets
(MTG-Jamendo, FMA, MusicCaps, AudioCaps).

**Causal chain.** MuQ-MuLan was trained on (audio, caption) pairs;
the captions are general-purpose. Further-training on (audio,
intensity-keyword) pairs sharpens the embedding axis aligned with
intensity.

**Expected gain.** Speculative. +2-4 pp possibly.

**Cost.** Heavy. Needs ~1M annotated pairs, custom training pipeline.

**Self-critique.** Better path: just LoRA-fine-tune (E4) on our own
consensus data. Same idea, much cheaper, same benefits.

### I2. Test-time adaptation

**Hypothesis.** At inference time, use a small in-collection statistics
adaptation: compute per-collection mean/std of audio embeddings, mean-
centre embeddings before V18.1 projection. Reduces collection-level
domain shift.

**Causal chain.** A user's collection has its own distribution
(DnB-only collections live in a tight region of MuQ-MuLan space;
varied collections span more). V18.1 was trained on a specific
distribution; adapting at inference might tighten the projection.

**Expected gain.** Marginal. Could go negative if it shifts the scale
in a way that harms calibration.

**Cost.** ~50 lines of Rust.

**Risks.** Per-collection adaptation breaks library-invariance (G2-like
concern: scores would shift as the user adds tracks).

**Self-critique.** Conflicts with the explicitly-required library-
invariance property. Drop.

### I3. Multi-clip transformer-pool

**Hypothesis.** Replace mean/peak pooling with a small transformer
that attends over the 6 clip embeddings → 1 pooled embedding. Trained
end-to-end on consensus targets.

**Causal chain.** Mean and peak are hand-engineered pooling. A
learned pool can attend to whatever's actually predictive of intensity,
including non-trivial patterns like "intro is short → it's a
DJ-friendly version".

**Expected gain.** +1-3 pp possibly.

**Cost.** Need clip-level data at training time (currently we only
have track-level). Requires re-training the audio embedding step
to keep per-clip outputs. Then training a small transformer (~100k
params) on top.

**Risks.** The 6 clips at training time would need to come from the
same 30 s preview the model was trained on (3 internal clips), which
limits the value of multi-clip pooling. Real value only emerges with
F1 (full-length tracks).

**Self-critique.** Couples to F1. Defer until then.

### I4. Self-supervised intensity discovery

**Hypothesis.** Discover an "intensity axis" in MuQ-MuLan space without
any labels at all, via PCA / ICA / manifold learning over a curated
seed set. The first principal component over a "high-intensity vs
low-intensity" pair set might be a much cleaner intensity axis than
the trained one.

**Causal chain.** If the pretraining objective captured intensity as
a strong axis, an unsupervised method would find it. Compare to the
trained V18.1 vector — they should agree on the dominant direction
if intensity is a natural axis.

**Expected gain.** Diagnostic, not improvement. Probably a worse
axis than V18.1 (which has actual labels). But the *agreement*
between the two would be informative.

**Cost.** ~100 lines of Python. Quick.

**Self-critique.** Worth running once as a sanity check. Don't expect
a deployable result.

### I5. Auxiliary BPM/key conditioning

**Hypothesis.** At inference, also feed in BPM and key (cheap to compute
classically, no ML) → student input is `[audio_emb, bpm_norm, key_id]`.
Multi-task at training time on (intensity, BPM-bin, key) but use BPM
and key as inputs at inference.

**Causal chain.** BPM is highly intensity-correlated within DJ genres
(higher BPM → higher intensity). Adding it as an explicit input takes
a feature that MuQ-MuLan has to *recover* and gives it directly,
freeing up encoder capacity for other features.

**Expected gain.** +1-2 pp test PA. BPM is reliable from `librosa.beat`
or `aubio`.

**Cost.** Compute BPM at inference (cheap, already computed by mesh-cue
in many cases). Re-train V18.1 with the augmented input dim.

**Risks.** BPM detection errors propagate. Cross-genre BPM-intensity
relationship varies (180 BPM in DnB vs 120 in techno).

**Self-critique.** Subset of E1 (multi-task teacher) but with BPM as
an *input* rather than just an aux *target*. Worth testing both:
input vs aux output. Probably the input version helps more.

### I6. Mixup augmentation in embedding space

**Hypothesis.** Linearly interpolate pairs of MuQ-MuLan embeddings
(λ × emb_A + (1-λ) × emb_B) and use the corresponding interpolated
intensity (λ × y_A + (1-λ) × y_B) as a synthetic training example.

**Causal chain.** Smooths the loss landscape, regularizes, reduces
overfitting on the existing 39913 tracks.

**Expected gain.** +0.5-1 pp typically. Mixup is a well-known
regularizer but the gain in regression is smaller than classification.

**Cost.** ~10 lines of training code change.

**Risks.** Linear interpolation in cosine-normalized space (which
MuQ-MuLan output is) isn't quite right; need to renormalize after
interp. Also: the interpolated audio doesn't physically correspond
to a real track.

**Self-critique.** Cheap and almost-free. Worth trying. But the gain
ceiling is small.

---

## 10. Process / measurement improvements

### J1. Bench-driven improvement, not vibe-driven

Establish a single canonical eval protocol:
1. Held-out test set PA (the existing 3985 tracks)
2. User library separation PA (the +55.8 pp metric)
3. DEAM correlation
4. Anchor-set spearman ρ (when F3 lands)

Every change reports all four. Currently we mix metrics inconsistently.

### J2. A/B sweep infrastructure

A small framework that takes a config dict (the hyperparameters in
play for an experiment) and produces all four eval numbers. Lets us
run B1 / B3 / E1 / E4 in a structured way and not re-implement the
plumbing each time.

### J3. Continuous integration on V18

Tests in mesh-cue that load V18.1, embed a fixed reference clip,
and verify the projected score is within tolerance of a frozen golden
value. Catches accidentally shipped broken weights.

---

## 11. Final ranking

Ranked by **(expected PA gain) × (probability of working) / cost**:

| Rank | Suggestion | Why this rank | Cost | Expected PA gain |
|---:|---|---|---|---|
| 1 | **F3** Curated 500-track anchor set | Foundational measurement; everything else depends on it | 10 hr human time | 0 (eval, not model) |
| 2 | **B6** Caption stability check | Trivial cost, tells us if B2 is worth running | 5 min | Information |
| 3 | **A1** Realign deploy clip strategy with training | Highest cheap inference-side win, principled (matches training) | 30 min code + 5 min reanalysis | +1-3 pp |
| 4 | **B3** MF Likert audio-juror (4th juror) | Adds genuinely novel modality; satisfies G5 | 5.5 hr GPU + 30 s consensus | +2-4 pp |
| 5 | **B1** Caption prompt redesign for intensity | Sharper input → better consensus | 6 hr MF + 9 hr jurors | +2-5 pp |
| 6 | **E1** Multi-task teacher (BPM aux head) | Cheap, well-established methodology | 1 hr + 30 s rerun | +1-3 pp |
| 7 | **E4** LoRA fine-tune MuQ-MuLan | Highest-impact single audio-side change short of encoder swap | 6 hr GPU + ONNX export | +3-6 pp |
| 8 | **A4** Decouple intensity vs similarity embeddings | Architectural cleanup, enables better similarity | 1 day code | 0 PA (similarity wins) |
| 9 | **A3** Anchor-relative percentile display | UX win, library-invariant in the right way | 50 lines Rust | 0 PA (UX) |
| 10 | **C1** Add 1-2 frontier-API jurors | Diversity from non-local lineages | $80 + 4 hr | +1-3 pp |
| 11 | **A5** Energy-based clip selection | Combine with A1; helps long-quiet-intro tracks | 50 lines Rust + mel-side | +1-3 pp |
| 12 | **I5** BPM/key as model input | Free signal currently being re-derived by encoder | 1 hr + retrain | +1-2 pp |
| 13 | **B5** Re-run round-7.5 BT on new 24 599 tracks | Existing infrastructure, brings BT to 100% coverage | 50 hr GPU | +2-4 pp |
| 14 | **F2** Audio-side data augmentation | Robustness wins, marginal in-dist gains | 1 day code + heavy GPU | +1-3 pp |
| 15 | **B4** Sub-genre specialist jurors | Promising but design-heavy | days of design + 4 hr juror | +2-4 pp |
| 16 | **D2** Snorkel correlation-aware label model | Only matters after panel expansion | 1 day | +0-2 pp (post-expansion) |
| 17 | **E6** Multi-encoder ensemble (post MAEST migration) | Strong direction, blocked on encoder migration | days | +3-5 pp |
| 18 | **F1** Full-length corpus rebuild | Highest ceiling, highest cost, legal risk | 50-100 hr GPU + data eng | +3-6 pp |
| 19 | **E5** MAEST/MULE encoder swap | Already in TODO | weeks | +3-6 pp |
| 20 | **I6** Mixup augmentation | Small gain ceiling | 1 hr | +0.5-1 pp |

Items dropped from the ranking:
- A2 (top-k peak pool) — A1 subsumes
- A6 (multi-scale clips) — off-distribution for encoder
- B2 (multi-caption ensemble) — gated on B6 outcome
- C3 (reasoning juror with rubric) — folds into C1
- D1 (multi-axis consensus) — round-7.5 already tried
- E2 (ordinal regression) — minor win
- E3 (quantile head) — UX, not PA
- E7 (stem separation) — too expensive for the ceiling
- E8 (CRD distillation) — student already saturated
- F4 (genre balance) — needs audit first
- G2 (similarity-aware FT) — niche
- I1 (intensity-CLIP pretrain) — E4 is the cheaper version
- I2 (test-time adaptation) — breaks library invariance
- I3 (transformer pool) — couples to F1
- I4 (unsupervised intensity discovery) — diagnostic only

---

## 12. Recommended Round-7.7 plan

Phase 1 (1 week, ~$80 in API, ~12 hr GPU):
1. F3 — anchor set (10 hr human)
2. B6 — caption stability check (5 min)
3. A1 — realign deploy clip strategy (30 min)
4. A4 + A3 — decoupled similarity emb + anchor percentile display (2 days)

Phase 2 (1 week, ~30 hr GPU):
5. B3 — MF Likert as 4th juror (6 hr)
6. B1 — caption prompt redesign + re-run captions + jurors (15 hr GPU)
7. E1 — multi-task teacher (BPM aux) (1 hr)
8. C1 — 1-2 API jurors (~$80, 4 hr)

Phase 3 (1-2 weeks, ~10 hr GPU):
9. E4 — LoRA fine-tune MuQ-MuLan (6 hr GPU + ONNX export)
10. I5 — BPM/key as input (1 hr)

Each phase ends with eval-on-everything via the J1 protocol.

After Phase 3, we should have a clear picture of:
- whether teacher PA can break 0.95 (caption-prompt + extra juror)
- whether student PA can hit 0.85+ (LoRA fine-tune)
- whether library separation can hit +60 pp ("excellent")

Phase 4+ (Lever 2): MAEST encoder migration (E5), multi-encoder
ensemble (E6), full-length corpus (F1) — the heavy stuff already in
the TODO and embedding-models-research backlog.
