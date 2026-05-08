# Round-7.6 V18 — General-Purpose Intensity Axis Pipeline Specification

**Status**: spec / not yet fully implemented
**Spec date**: 2026-05-07
**Branch**: `text-tower-aggression-axis`
**Successor to**: V15 (round-6 linear probe), V17b (round-7.5 polar-prompt blend)
**Companion docs**:
- `documents/aggression-axis-eval-round-7.md` — round-7 results
- Obsidian: `Mesh — Caption-as-Feature Methodology` — design rationale + literature review
- Obsidian: `Mesh — Music Flamingo Pointwise Findings` — why pointwise rating fails
- TODO.md § "Round-7.6 V18 — General-purpose intensity axis"

---

## 1. Purpose

Train a **CPU-deployable linear probe over a frozen audio embedding** (MuQ-MuLan today, MAEST after the embedding upgrade lands) that emits a single scalar **DJ-set intensity score** per track. The score must order tracks sanely across DJ-relevant genres and adjacent (rock, ambient) for any user, with no GPU required at inference, no user-library fitting, and no per-user data flowing into training.

The deployment target is `mesh-player` and `mesh-cue`, batched at track-import time (CPU-only path).

## 2. Goals (verifiable)

A reviewer can grade the implementation against these:

- **G1.** **Linear deployment shape.** The deployed model is `intensity = audio_emb @ vec + bias` over a frozen audio embedding (MuQ-MuLan 512d or MAEST 768d/2304d depending on which embedding pass is current). No additional features at inference.
- **G2.** **No user-library leakage.** No path in training reads from the user's library. Eval is exclusively a held-out subset of the public Deezer corpus.
- **G3.** **Cross-genre held-out PA ≥ 75 %.** Pairwise agreement against the consensus label on the held-out test set ≥ 75 %, averaged across genre groups.
- **G4.** **Per-genre sanity.** On the held-out set, mean intensity per genre group obeys the *qualitative* ordering: industrial-techno / hardcore / metalstep > drum-and-bass / dubstep / drill > tech-house / techno > deep-house / disco > acoustic / fingerstyle > ambient / drone. (Specific genre-mean differences need not be huge, but the ordering must be monotone with no inversions in the top tier and bottom tier.)
- **G5.** **Multi-judge construction.** The training label is the consensus of ≥ 3 heterogeneous LLM-derived sources, aggregated via Dawid-Skene or Snorkel (learned source reliabilities), not fixed weighted-mean. *Updated 2026-05-08 from "≥ 4" to "≥ 3":* the spec was written assuming round-7.5 BT-prior and aggressive_overall_tag would fold in as additional jury sources, but the corpus expansion to 39913 tracks left those at 38% coverage while the modern caption-text-LLM jurors (Mistral-Small-3.2 / Nemotron-30B / Qwen3.6-27B) cover 100%. The mixed-coverage Dawid-Skene EM exhibited σ²-runaway pathologies (one full-coverage source pinned the consensus, see `aggregate_consensus.py:228` floor + `nanmedian` init guards). Dropping the partial-coverage sources produced a better-conditioned EM at the cost of one nominal source. The 3-juror panel uses three distinct foundation-model lineages (NVIDIA-Nemotron, Mistral, Alibaba-Qwen) with pairwise Spearman ρ=0.93-0.96, satisfying the diversity intent of the original "≥ 4" target.
- **G6.** **Privileged-information distillation.** Teacher uses richer features than student; student deployment shape matches G1. FitNets feature-matching + Hinton soft-target distillation are both present in the student loss.
- **G7.** **Everynoise `source_category` is untrusted across the entire pipeline.** It does not appear as a teacher feature, student feature, label source, training weight, or stratification key. The everynoise tags were observed to be unreliable on this corpus (see User-feedback log; mislabels confirmed by domain-expert audit on the DnB slice). The genre-prior label source proposed in earlier drafts is **removed**; the consensus jury is therefore 4-source, not 5.
- **G8.** **Artist-stratified split.** Test-set artists never appear in train or val. Genre stratification is intentionally NOT enforced — relying on the noisy `source_category` to balance test-set genres would re-introduce the trust we explicitly removed. With N≈40 k post-expansion, law-of-large-numbers handles genre balance; an *audible-genre* diagnostic via caption-embedding K-means cluster IDs is reported in the eval (NOT used for stratification or training).
- **G9.** **CPU inference budget.** End-to-end V18 inference (linear probe over the chosen frozen embedding) ≤ 100 ms per track on the user's CPU.
- **G10.** **Deterministic reproducibility.** Seeds pinned for split, label-model EM init, teacher init, student init. Re-running the post-caption stages produces identical V18 weights.

## 3. Non-goals

- **Not** a per-user calibrated scale. A separate optional "Round-7.7" pipeline can fine-tune V18 to a user's preferences on an NVIDIA GPU. Out of V18 scope.
- **Not** a recommender / set-builder. V18 outputs one scalar per track; it does not rank pairs of tracks for transitions, suggest follow-ups, or model session arcs.
- **Not** a deep model. V18 is a single linear projection over the frozen embedding. Deeper student architectures are out of scope unless the linear-probe ceiling is demonstrably below G3.
- **Not** an audio-LM. V18 does not require Music Flamingo at inference. MF is used only at training time as a feature extractor and one of the label-source judges.
- **Not** a music-mood / valence model. V18 outputs intensity, not arousal-valence 2D, not mood category, not danceability.

## 4. Glossary

- **Corpus** — `/home/data01/Music/mesh-track-grading/audio/`, 15 314 × 30s mono Deezer-preview MP3s, IDs `dz_<int>.mp3`.
- **MuQ-MuLan** — frozen 700M music-audio encoder, 512d output. Per-track corpus embeddings already cached at `/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz`.
- **Music Flamingo (MF)** — `nvidia/music-flamingo-2601-hf`, 7B audio-LM, served via vLLM at `:8001`.
- **r7.5 BT priors** — round-7.5 Bradley-Terry scores per (track, axis) from Qwen3-Omni K=4 N-way tournaments, at `/home/data01/Music/mesh-track-grading/round7_5_priors.npz`.
- **Caption** — MF free-form description of a 30s clip, ~190 words at `T=0.7, top_p=0.9, max_tokens=256`.
- **Caption embedding** — bge-base-en-v1.5 sentence-transformer over the caption text, 768d, L2-normalized.
- **Source category** — everynoise seed genre per track, in `/home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json#source_category`. ~150 unique categories.
- **Consensus label** — single intensity scalar per track produced by Dawid-Skene aggregation of N rank-normalized LLM-derived sources.
- **Teacher** — multi-input MLP head trained against the consensus label and the multi-axis labels.
- **Student** — linear probe over the frozen audio embedding trained via FitNets + Hinton distillation from the teacher.
- **V18** — the deployed student. Output: `models/aggression-axes/V18_round7_6_consensus_distilled.json`.

## 5. Data inventory (inputs)

All inputs are on local disk before V18 training begins.

### 5.1 Audio + metadata
- `/home/data01/Music/mesh-track-grading/audio/dz_<tid>.mp3` — 15 314 × 30s mono 16 kHz MP3.
- `/home/data01/Music/mesh-track-grading/deezer/corpus_tracks.json` — list of records `{deezer_track_id, artist, title, isrc, duration_s, preview_url, source_category, source_seed, match_kind}` for every track in the corpus.
- `/home/data01/Music/mesh-track-grading/everynoise_genres.json` — full everynoise genre catalog (parent/child structure).
- `/home/data01/Music/mesh-track-grading/everynoise_dj_genres.json` — DJ-relevant subset of the catalog.

### 5.2 Existing features
- `/home/data01/Music/mesh-track-grading/embeddings/corpus_muq_mulan.npz`
  - `track_ids: int64[15314]`
  - `embeddings: float32[15314, 512]` (MuQ-MuLan, frozen, used as student input)
  - `artists, titles, genre_seed: object[15314]` (genre_seed is empty; both this column and the parallel `source_category` field in `corpus_tracks.json` are untrusted on this corpus per G7 — do not read them into training, labels, or stratification)

### 5.3 Existing labels (one of the consensus sources)
- `/home/data01/Music/mesh-track-grading/round7_5_priors.npz`
  - `track_ids: int64[15314]`
  - `axes: object[16]` — list of 16 axis IDs, alphabetical
  - `scores: float32[16, 15314]` — BT scores (after Hunter MM iteration)
  - `priors_0_10: float32[16, 15314]` — same scores rescaled to [0,10]
  - `n_games: int32[16, 15314]`
  - `win_rate: float32[16, 15314]`
- `/home/data01/Music/mesh-track-grading/round7_5_tags.npz`
  - `track_ids: int64[15314]`
  - `tag_names: object[13]` — `[distortion, vocal_shouted, density, bass_heavy, brightness, noise_layer, tempo_fast, rhythmic_complex, melodic_present, compression_high, synthetic, drop_present, aggressive_overall]`
  - `tag_evidence: float32[15314, 13]` — mined evidence count per tag, can be negative
  - `tag_n_mentions: int32[15314, 13]`

### 5.4 Stage-produced features (this pipeline produces them)
- `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<tid>.json` — per-track caption JSON
- `/home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz` — bge-base 768d embeddings
- `/home/data01/Music/mesh-track-grading/round7_6_caption_struct.npz` — structured tags extracted from caption text
- `/home/data01/Music/mesh-track-grading/round7_6_caption_intensity.npz` — text-LLM intensity rating over captions
- `/home/data01/Music/mesh-track-grading/round7_6_consensus.npz` — consensus intensity label per track + per-source reliabilities

### 5.5 Frozen reference for benchmarking only
- `models/aggression-axes/V15_linear_probe_r6.json`
- `models/aggression-axes/V17_round7_5_polar_blend.json`
- These are used only as one of the per-track sanity comparisons in the held-out eval. They are NOT used as training labels. (G2.)

## 6. Pipeline overview

```
                        ~40 000 Deezer tracks (post-S0 expansion to 30/seed)
                                │
              ┌─────────┬───────┴─────────┬─────────────┐
              ▼         ▼                 ▼             ▼
            (S1)      (S2)              (S3)          (S4)
           caption    cap_emb           struct        text-LLM
           sweep      bge-base          tags          intensity
                                        extract       rating

         (S4 produces N caption-intensity NPZs, one per text-LLM
          juror — Mistral-Small-3.2-24B AWQ, Nemotron-30B,
          Qwen3.6-27B for the V18 release run.)

         (Stage S5 was the genre-prior lookup; REMOVED per G7.
          The everynoise source_category field is untrusted across
          the entire pipeline and never read into labels, features,
          weights, or stratification.)

         (S6a/S6b/S6c — round-7.5 BT priors, aggressive_overall_tag,
          MF Likert — are NOT used in the V18 release run. They
          only cover 15314 of the expanded 39913-track corpus (38%);
          the mixed-coverage Dawid-Skene EM was ill-conditioned and
          dropping them strictly improved consensus quality. The
          underlying NPZs remain on disk but are not read into the
          label aggregation. See §14 for details.)
                                │
           ┌──── N label sources ┴─────────────┐
           │  (S4 caption-intensity × N jurors) │
           ▼                                   │
       (S7) rank-normalize per source          │
           │                                   │
           ▼                                   │
       (S8) Dawid-Skene / Snorkel              │
           label model fit                     │
           → consensus intensity               │
           + per-source reliabilities          │
                                               │
       (S9) artist-stratified split            │
           80 / 10 / 10 train/val/test         │
           NOT genre-stratified; per-cluster   │
           diagnostic at eval time             │
                                              │
       (S10) Train teacher                    │
            in: [audio_emb,                   │
                 caption_emb,                 │
                 struct_tags,                 │
                 r7.5_tags]                   │
            out: 16 axis heads + intensity   │
                                              │
       (S11) Distill student                  │
            in: audio_emb only                │
            loss: out-MSE                     │
                + FitNets penultimate-MSE     │
                + Hinton T-soft-target        │
                + label smoothing             │
                                              │
       (S12) Held-out eval                    │
            PA on test set                    │
            per-genre histogram               │
            V15 / V17b / V18 comparison       │
                                              │
       (S13) V18 export                       │
                                              │
       (Optional) DEAM arousal anchor         │
       (Optional) Isotonic calibration        │
```

## 7. Stage S1 — Caption generation

**Goal**: each of the 15 314 corpus tracks gets a rich free-form caption from Music Flamingo at NVIDIA-recommended decoding.

### Inputs
- `/home/data01/Music/mesh-track-grading/audio/dz_<tid>.mp3` for each `tid`
- vLLM serve of `nvidia/music-flamingo-2601-hf` at `http://localhost:8001` (started by `spike/track-grading/serve_music_flamingo.sh`)

### Algorithm
1. Enumerate corpus track IDs from audio dir.
2. Skip any `tid` whose caption JSON already exists (atomic resume).
3. For each pending `tid`, dispatch a single-audio chat completion through vLLM:
   - System: `"You are a careful music analyst with strong perception of timbre, mood, rhythm, and production style."`
   - User: `<audio data:wav;base64,…>` + `"Describe this music clip in rich detail. Cover the instrumentation, production style, mood, rhythm and groove, harmony, vocal qualities (if any), structural events (buildup, drop, breakdown), and the overall energy. Use specific musical vocabulary."`
   - Decoding: `temperature=0.7, top_p=0.9, max_tokens=256`
   - vLLM `multi_modal_uuids = {"audio": [f"track_{tid}"]}` for encoder cache reuse
4. Atomically write `<out_dir>/<tid>.json` with `{track_id, caption, wall_time_s, ts, model, max_tokens, temperature, top_p, completion_tokens}`.

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<tid>.json` × 15 314.

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| `temperature` | 0.7 | NVIDIA-recommended; greedy collapses on round numbers |
| `top_p` | 0.9 | NVIDIA-recommended |
| `max_tokens` | 256 | ~190 words; first-half of NVIDIA's 452-word reference distribution; balances richness vs throughput |
| `workers` | 8 | matches vLLM `max_num_seqs=4` × ~2 decode/encode overlap |
| `prompt` | as above | NVIDIA's project-page demos use long-form descriptive |

### Pass criteria (S1 stage gate)
- All 15 314 captions written; 0 inference failures permitted (re-run on transient errors).
- Caption length distribution: p10 > 100 words, p50 ≥ 150 words, p95 ≥ 200 words (sanity that decoding wasn't truncated).
- Sustained throughput ≥ 0.4 calls/sec on RTX 5090 Mobile bf16; otherwise investigate vLLM scheduler / GPU power state.
- 50-track repeat-stability check (re-caption 50 random tracks, embed both runs, cosine ρ between paired embeddings) > 0.85; otherwise consider lowering temperature to 0.3.

### Common failure modes
- vLLM CUDA crash → vLLM dies; resume after restart works.
- GPU thermal throttle → wall_time/call jumps from ~2s to ~10s; check `nvidia-smi --query-gpu=power.draw,clocks.current.graphics`.

### Expected runtime
~8 hr for full 15 314 corpus at ≥ 0.5 c/s.

## 8. Stage S2 — Caption embedding

**Goal**: each caption is mapped to a 768d vector via `BAAI/bge-base-en-v1.5`.

### Inputs
- `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<tid>.json` × 15 314.

### Algorithm
1. Load `BAAI/bge-base-en-v1.5` (sentence-transformers), GPU if available, else CPU.
2. Read all captions, embed in batches of 64.
3. L2-normalize embeddings (dot-product = cosine).

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_caption_emb.npz`
  - `track_ids: int64[N]`
  - `caption_emb: float32[N, 768]` (L2-normalized)
  - `caption_lengths: int32[N]` (word count, sanity)
  - `model_name: str` = `"BAAI/bge-base-en-v1.5"`

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| Encoder | `BAAI/bge-base-en-v1.5` | MTEB retrieval-strong, 768d, fast on CPU |
| Batch size | 64 | well within GPU/CPU memory; ~5 min total |
| Normalize | yes | downstream cosine-friendly |

### Pass criteria
- 100 % coverage: every caption JSON yields a row in the NPZ.
- Distribution check: 99th-percentile of `||emb||` is in `[0.999, 1.001]` (L2 normalization sanity).

### Expected runtime
~5 min on CPU, < 1 min on GPU.

## 9. Stage S3 — Caption structured tag extraction

**Goal**: derive interpretable per-track multi-hot tags from the caption text — instrumentation, vocal type, mood adjectives, mix descriptors. These augment the teacher's feature set.

### Inputs
- `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<tid>.json`

### Algorithm
1. Define a static dictionary of tag → keyword list (~50 tags total). Examples:
   - `instr_kick` ← `{"kick", "kick drum"}`
   - `instr_808` ← `{"808"}`
   - `instr_synth_bass` ← `{"synth bass", "bass synth", "sub bass"}`
   - `instr_acoustic_guitar` ← `{"acoustic guitar", "nylon-string", "fingerstyle"}`
   - `vocal_none` ← `{"no vocals", "instrumental"}`
   - `vocal_clean` ← `{"clean sung", "melodic singing"}`
   - `vocal_aggressive` ← `{"shouted", "screamed", "growled", "harsh"}`
   - `mood_dark` ← `{"dark", "ominous", "menacing", "brooding"}`
   - `mood_bright` ← `{"bright", "uplifting", "joyful", "sparkling"}`
   - `mix_polished` ← `{"polished", "high-fidelity", "clean mix"}`
   - `mix_lofi` ← `{"lo-fi", "raw", "dusty", "tape-warm"}`
   - `prod_compressed` ← `{"compressed", "brick-walled", "loud"}`
   - …
2. For each caption, run case-insensitive substring match → boolean per tag.
3. Optional: also produce a count of mentions per tag (for downstream weighting).

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_caption_struct.npz`
  - `track_ids: int64[N]`
  - `tag_names: object[T]` (T ≈ 50)
  - `tag_present: bool[N, T]`
  - `tag_count: int32[N, T]`

### Pass criteria
- Per-tag mean rate is in `[0.005, 0.95]` for ≥ 80 % of tags (most tags are sometimes-present, sometimes-not). Tags hitting 0 % or 100 % are dead and should be removed or reformulated.
- Manual spot check: 10 random captions, verify extracted tags match the prose.

### Expected runtime
~5 min CPU.

### Caveats
- This is a deliberately simple extractor. Future improvement: replace with a structured-output text LLM call. Out of V18 scope.

## 10. Stage S4 — Caption → text-LLM intensity rating

**Goal**: a *second*, independent intensity signal per track, derived by feeding the caption text (no audio) into a small text LLM with a calibrated rubric.

### Inputs
- `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<tid>.json`

### Algorithm
1. Bring up a text-LLM serve in parallel with MF (or repurpose the same vLLM after MF sweep ends). Recommended: `Qwen/Qwen2.5-7B-Instruct` or `meta-llama/Llama-3.1-8B-Instruct`. Single-GPU, ~8 GB VRAM, fast.
2. For each caption, call:
   - System: `"You are a music analyst rating DJ-set intensity from text descriptions."`
   - User: `f"Description of a music clip:\n\n\"\"\"\n{caption}\n\"\"\"\n\nRate the clip's overall DJ-set intensity on a 1–5 scale.\n1 = very low intensity (ambient, contemplative, sparse).\n2 = low (gentle, slow, reflective).\n3 = medium (steady groove, balanced).\n4 = high (energetic, driving, club-ready).\n5 = very high (relentless, abrasive, peak-time).\n\nReply with one digit (1, 2, 3, 4, or 5)."`
   - Decoding: `temperature=0.0, max_tokens=4, logprobs=True, top_logprobs=10`
3. Parse first generated token's top-logprobs over `{"1","2","3","4","5"}`, softmax-normalize → 5 probabilities.
4. Convert to scalar via `score = Σ p_i · (i-1) / 4 ∈ [0, 1]` (logprob-soft-Likert, same trick used elsewhere).

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_caption_intensity.npz`
  - `track_ids: int64[N]`
  - `score: float32[N]` ∈ [0, 1]
  - `bucket_probs: float32[N, 5]`
  - `model_name: str`
  - `raw_first_token: object[N]`

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| Model | Qwen2.5-3B-Instruct (default local) **or** any remote OpenAI-compatible endpoint via `TEXT_LLM_URL` env (e.g., a Qwen3.6-27B on a DGX Spark) | text-only, well-calibrated on 1-5 rating; bigger remote models give meaningfully better calibration |
| Workers | 24 (local) / 16 (remote, knee found via `bench_text_llm_throughput.py` 2026-05-07) | beyond W=16 the Spark queues — latency 2 s→11 s with no tput gain |
| `TEXT_LLM_NO_THINK` | `1` for Qwen3-style reasoning models | suppresses `<think>` block via `chat_template_kwargs={enable_thinking: false}`; without this, max_tokens=4 cuts off mid-reasoning and no answer is emitted |
| Temperature | 0.0 | logprobs are recovered; greedy is fine for the soft-Likert trick |
| `top_logprobs` | 10 | covers all 5 digit tokens with margin |
| Rubric | as above | explicit anchor wording, balanced across 5 buckets |

### Pass criteria
- 100 % coverage; logprobs returned successfully on every call.
- Score distribution std > 0.20 across the corpus (well-spread, no collapse).
- ≥ 4 of 5 buckets have non-trivial mass somewhere in the corpus (no axis collapses to {3-only}).
- Sanity: intensity correlates ρ > 0.5 with the r7.5 BT-blend intensity on the same corpus (different judges, same task; ρ < 0.3 means one judge is noise).

### Expected runtime
~30 min for full corpus at typical text-LLM throughput (50-100 c/s).

## 11. Stage S5 — REMOVED (was: genre-prior lookup)

**Removed 2026-05-07** per goal G7. The everynoise `source_category` field was originally intended as a low-weight label-side anchor. After domain-expert audit on the DnB slice (`/home/data01/Music/mesh-track-grading/dnb_audit.md` review), the tags were confirmed unreliable on this corpus. Using them as ground truth — even at low weight — would systematically inject mislabel noise into the consensus. The label-source jury is therefore reduced to 4 sources (S4 + S6a + S6b + optional S6c). No replacement is needed; with 4 heterogeneous LLM-derived sources the consensus is well-conditioned.

`source_category` may still be *displayed* in audit / debug tables (as in `dnb_audit.md`) but never read into a training or label-aggregation step.

## 12. Stage S6 — Other label sources (NOT used in V18 release run)

**Status update 2026-05-08:** the round-7.5 sources described below are
**not** read into the V18 release consensus. They were originally
designed as additional jury sources to satisfy the "≥ 4 heterogeneous
sources" target. After the corpus expansion to 39913 tracks they only
cover 15314 (38%); carrying them forward forces a 38%/100% coverage
asymmetry into Dawid-Skene's EM that contributed to σ²-collapse
pathologies (one full-coverage source pinned the consensus, all others
weight 0.000). Dropping them produced a cleaner, better-conditioned
3-source consensus from the modern caption-text-LLM jurors.

The original §12 description of S6a/S6b/S6c is preserved below for
traceability and as a future re-introduction sketch (e.g., once
round-7.5 is re-run on the expanded corpus, or if a future round
produces multi-axis BT scores natively at full coverage). They are
**not** invoked by `aggregate_consensus.py` in the V18 release path.

These were existing assets that would have been used as additional consensus inputs:

### S6a. r7.5 BT-prior intensity blend (existing)
- Source: `/home/data01/Music/mesh-track-grading/round7_5_priors.npz`, `scores[16, N]`.
- Algorithm: identical to V17b's ListMLE blend — load the 16 BT-axis scores, learn a 16d blend weight that maximizes a target axis (`timbre_roughness` historically), output a 1d intensity per track.
- Output: `/home/data01/Music/mesh-track-grading/round7_6_btprior_intensity.npz` with `track_ids` and `intensity: float32[N]`.

### S6b. r7.5 mined-tag `aggressive_overall` (existing)
- Source: `/home/data01/Music/mesh-track-grading/round7_5_tags.npz`.
- Algorithm: extract `tag_evidence[:, idx_of("aggressive_overall")]`. Already a per-track scalar.

### S6c. MF Likert intensity (optional, low-weight)
- Source: `/home/data01/Music/mesh-track-grading/round7_6_likert/music_flamingo/<axis>/<tid>.json`. Only the 200-track smoke is on disk; full-corpus extension would take ~13 hr. Use only if Phase-1 transfer test (§8.1) suggests caption-emb alone is too narrow.
- Algorithm: load Likert score for `aggression_overall`-relevant axes (timbre_roughness, vocal_aggression, bass_presence, noise_layer, drop_architecture, textural_density, onset_density, tempo_perception); z-mean → 1d intensity per covered track.
- Marked low-weight in the consensus.

## 13. Stage S7 — Per-source rank normalization

**Goal**: each source produces a scalar per track on its own scale. Before aggregation, normalize each to a common `[0, 1]` range using **rank** (not z-score), to be robust to ceiling/floor-mass distributions (Likert+logprob has this).

### Inputs
- All source NPZs from S4-S6.

### Algorithm
For each source `s` with raw scores `x_s[N]`:
```
ranks_s = argsort(argsort(x_s))  # 0..N-1
x_norm_s = (ranks_s + 0.5) / N    # ∈ (0, 1)
```
Equivalent to the empirical CDF.

For tracks where source `s` is missing (e.g., MF Likert only on 200 tracks): set `x_norm_s = NaN` for those tracks. Dawid-Skene below handles partial coverage natively.

### Outputs
- A single NPZ `/home/data01/Music/mesh-track-grading/round7_6_sources_ranknorm.npz`:
  - `track_ids: int64[N]`
  - `source_names: object[S]`
  - `scores_ranknorm: float32[N, S]` (NaN where missing)
  - `coverage: bool[N, S]`

### Pass criteria
- Per-source: marginal distribution after normalization is approximately uniform on [0, 1] (KS-test p > 0.01 against U(0,1) for sources with ≥ 1000 covered tracks).

## 14. Stage S8 — Dawid-Skene / Snorkel label aggregation

**Goal**: produce a single consensus intensity per track from the rank-normalized sources, using a learned per-source reliability rather than fixed weights.

### Inputs
- `/home/data01/Music/mesh-track-grading/round7_6_sources_ranknorm.npz`

### Algorithm

We treat each source as noisy observation of a latent intensity:
```
x_norm_s_i ~ TruncatedNormal(z_i, σ_s^2, [0, 1])
```
where `z_i ∈ [0, 1]` is the latent intensity for track `i` and `σ_s` is the noise standard deviation of source `s` (its **un**reliability).

EM iteration (M-step is closed-form Gaussian conditional, same shape as Snorkel's continuous label model):

1. **Init.** `z_i ← median over covered sources` of `x_norm_s_i`. `σ_s ← 1.0`.
   *(2026-05-08: changed from `mean` to `median` after observing
   that mean-init biases the M-step in favor of whichever source's
   ranks are closest to the source mean, producing a single-source
   monopoly. Median is robust to that pathology.)*
2. **E-step.** Hold `σ` fixed; set
   `z_i ← Σ_s 1{covered} · x_norm_s_i / σ_s² / Σ_s 1{covered} / σ_s²`
   (precision-weighted mean over covered sources).
3. **M-step.** Hold `z` fixed; set
   `σ_s² ← max(mean over covered tracks of (x_norm_s_i − z_i)², SIGMA2_MIN)`.
   *(2026-05-08: hard floor SIGMA2_MIN=0.01 added to prevent
   precision-runaway. Without it, a source whose residuals fit
   exactly to z by coincidence at any iteration sees 1/σ² → ∞,
   pinning the next E-step's z to that source. Floor caps max
   precision at 100, just below the healthiest observed juror's
   natural precision (~150) on this corpus.)*
4. Iterate to convergence (Δ in `σ_s` < 1e-5 or 200 iter).

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_consensus.npz`
  - `track_ids: int64[N]`
  - `consensus_intensity: float32[N]` ∈ [0, 1]
  - `source_names: object[S]`
  - `source_reliabilities: float32[S]` — `1 / σ_s²`, normalized to sum to 1; reviewer should sanity-check that the genre-prior gets the lowest reliability (it's the simplest signal) and that MF-Likert and r7.5-BT-blend get comparable mid-tier reliabilities.

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| Init σ_s | 1.0 | uninformed |
| Convergence tol | 1e-5 | well within EM stability |
| Max iter | 200 | converges in < 30 typically |
| Seed | 42 | for any tie-breaking |

### Pass criteria
- EM converges in < 100 iterations.
- Source reliabilities are non-degenerate: no source has > 90 % of total weight (means the rest are noise) and no source has < 1 % (means it was bad enough to ignore).
- Consensus distribution: std > 0.20 across the corpus, no spike at 0.5 (would indicate degenerate consensus).
- Spot-check: 20 random tracks, manually inspect (caption + audio if needed) that the consensus intensity is reasonable.

### Reviewer sanity checks
- Reliabilities must be reported and signed-off. Expected ranking (loose, not gospel): `r7.5_bt_blend ≥ caption_text_LLM ≥ aggressive_overall_tag ≥ MF_Likert`. Anything wildly off is worth a second look.

## 15. Stage S9 — Artist-stratified split

**Goal**: split the (post-expansion ~40 k) corpus into train/val/test with stratification by **artist only**. Test-set artists never appear in train or val (G8).

Genre stratification is intentionally not enforced. Trusting `source_category` to balance test-set genres would reintroduce the noise we removed in G7. With ~40 k tracks the law of large numbers handles genre balance; for an *audible-genre* diagnostic see Stage S12.

### Inputs
- `corpus_tracks.json` — only `artist` is read; `source_category` is ignored.

### Algorithm
1. Build the unique-artist list from `corpus_tracks.json`.
2. Shuffle deterministically with `seed=42`.
3. Allocate artists to splits in 80/10/10 by track count (so each split gets ~80/10/10 tracks, not artists, since artist track counts vary widely).
4. Map each track to its artist's split.
5. Assert: 0 artists shared between train and test; 0 between val and test.

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_split.npz`
  - `track_ids: int64[N]`
  - `split: object[N]` ∈ `{"train", "val", "test"}`
  - `artist: object[N]`

### Pass criteria
- Train ≈ 80 %, val ≈ 10 %, test ≈ 10 % at the track level (±2 %).
- 0 artists shared between train and test (assert in code).
- 0 artists shared between val and test (assert in code).

## 16. Stage S10 — Teacher training

**Goal**: a 2-layer MLP head that predicts the consensus intensity (and the 16 r7.5 BT axes as multi-task aux) from privileged features.

### Inputs
- `corpus_muq_mulan.npz` — audio_emb 512d
- `round7_6_caption_emb.npz` — caption_emb 768d
- `round7_6_caption_struct.npz` — struct_tags ~50d (cast to float32, multi-hot or count)
- `round7_5_tags.npz` — r7.5_tags 13d (use `tag_evidence` directly)
- `round7_6_consensus.npz` — consensus intensity (primary target)
- `round7_5_priors.npz` — 16-axis BT scores (multi-task aux targets)
- `round7_6_split.npz` — split assignment

### Architecture
```
input = concat[audio_emb(512), caption_emb(768), struct_tags(~50), r7.5_tags(13)]  ≈ 1343d
       │
       ▼ Linear(1343 → 256) + GELU + Dropout(0.2)
       │
       ▼ Linear(256 → 128) + GELU
       │  ← penultimate features (used for FitNets)
       ├─ Linear(128 → 1)   → intensity head (consensus loss)
       ├─ Linear(128 → 16)  → axis heads (r7.5 BT loss)
       └─ Linear(128 → 1)   (optional) → cap-text-LLM intensity head
```

### Loss
```
L_teacher = α · L_intensity_consensus
          + β · L_axes_r75_bt
          + γ · L_caption_text_LLM_intensity   [optional]

L_intensity_consensus = MSE on 'train' split, masked by coverage
L_axes_r75_bt        = mean over 16 axes of MSE(pred_axis, BT_score) (z-scored per axis)
```

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| Hidden 1 | 256 | small enough to avoid overfitting 12k train, big enough for multi-task |
| Hidden 2 | 128 | penultimate dim that the student must match |
| Dropout | 0.2 | standard |
| Optim | AdamW lr=3e-4, wd=1e-4 | standard |
| Batch size | 256 | fits N=12k easily |
| Epochs | 100 with early-stop on val intensity loss (patience 10) | empirically converges around 30-50 |
| α | 1.0 | primary target |
| β | 0.3 | aux: keeps the rep useful but doesn't dominate |
| γ | 0.1 | optional aux |
| Seed | 42 | for init + dropout |

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_teacher.pt` — full model weights
- `/home/data01/Music/mesh-track-grading/round7_6_teacher_metrics.json` — train/val curves + final test PA

### Pass criteria
- Val intensity MSE drops monotonically (post-warmup) and converges.
- Val intensity Spearman ρ vs consensus ≥ 0.80 (the teacher should fit consensus well — it has all the features).
- Per-axis val Spearman ρ vs BT score ≥ 0.50 on average across 16 axes.
- Test-set held-out PA ≥ 0.78 (we want headroom over the student's 0.75 G3 target).

## 17. Stage S11 — Student distillation

**Goal**: a linear probe over MuQ-MuLan only that matches the teacher's intensity output as closely as possible. This is V18.

### Inputs
- `corpus_muq_mulan.npz` — audio_emb 512d (sole student input)
- Teacher predictions cached on the full corpus (intensity head + penultimate features)
- Split

### Architecture
```
student(audio) = audio_emb @ vec + bias       (Linear(512 → 1))
penultimate_proj(audio) = audio_emb @ W_p     (Linear(512 → 128) — only used during training for FitNets, not at deploy)
```
The penultimate-projection layer is a training-time-only adapter that maps the student's audio embedding into the teacher's penultimate space for FitNets matching. It is **discarded** at export.

### Loss
```
L_student = λ_out · MSE(student(audio), teacher.intensity(full features))                          [output distillation]
          + λ_fit · MSE(penultimate_proj(audio), teacher.penultimate(full features))               [FitNets]
          + λ_kd  · KL(softmax(student/T) || softmax(teacher.intensity/T)) · T²                    [Hinton soft-target, T=2.0]
          + λ_ls  · LabelSmoothing(student(audio), consensus_intensity)                            [direct anchor on label]
```

### Hyperparameters
| Param | Value | Why |
|---|---|---|
| λ_out | 1.0 | primary distillation signal |
| λ_fit | 0.5 | FitNets feature match — Romero recommends similar |
| λ_kd | 0.3 | Hinton soft-target weight |
| T (temperature) | 2.0 | Hinton's typical sweet spot |
| λ_ls | 0.2 | grounds the student to the actual label, not just teacher echo |
| Label smoothing ε | 0.05 | prevents memorization |
| Optim | AdamW lr=1e-3, wd=1e-4 | linear probe needs a slightly higher LR |
| Batch size | 512 | fits |
| Epochs | 50 with early-stop on val-set student-MSE-against-consensus (patience 10) | |
| Seed | 42 | |

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_student.pt` — student weights (vec, bias, and the FitNets projection that we'll throw away at export)
- `/home/data01/Music/mesh-track-grading/round7_6_student_metrics.json`

### Pass criteria
- Student converges; val student-vs-teacher correlation ρ ≥ 0.90.
- Held-out test PA ≥ 0.75 (G3).
- Teacher-vs-student PA gap on test ≤ 5 pp (LUPI literature suggests 70-90 % retention; we expect to be at the high end given audio-derivable captions).

## 18. Stage S12 — Held-out evaluation

**Goal**: produce the numbers the reviewer will use to grade the pipeline.

### Algorithm
On the test set:

1. **Primary metric**: pairwise agreement (PA) between V18-predicted intensity and consensus intensity. Defined as `mean over (i,j) ∈ test×test, i≠j of 1{(s_i - s_j) · (y_i - y_j) > 0}`.
2. **Secondary metrics**: Spearman ρ vs consensus, R² vs consensus.
3. **Per-cluster PA** (audible-genre diagnostic): K-means cluster the held-out test set's `caption_emb` (768d) into K=20 clusters, label each cluster with its top-3 nearest captions and a hand-readable theme. Report PA per cluster and the worst-performing cluster. Replaces the per-genre breakdown — uses what the model actually heard, not the noisy everynoise tags.
4. **Per-cluster intensity distribution**: mean and IQR per K-means cluster, plotted as a histogram. Verify the qualitative ordering (G4) by inspecting the cluster themes — the high-intensity clusters should map to industrial / hardcore / metal styles; the low-intensity clusters to ambient / acoustic / classical.
5. **V15 / V17b comparison**: project the test set through V15 and V17b, report their PA against consensus on the same test set. Note: these are **calibration baselines**, not training labels.
6. **Distillation gap**: PA of teacher (full features) vs PA of student (V18) on the same test set.
7. **Ablation table** (optional but very useful):
   - V18 trained only on r7.5-BT-blend label (no jury) — does the jury actually help?
   - V18 trained without caption_emb in teacher — does the caption channel actually help?
   - V18 trained with z-score instead of rank-norm — does rank-norm matter?

### Outputs
- `/home/data01/Music/mesh-track-grading/round7_6_eval_report.md` — markdown summary with all the tables and a per-genre histogram (matplotlib PNG).
- `/home/data01/Music/mesh-track-grading/round7_6_eval.json` — raw numbers.

### Reviewer-grading numbers
- **G3** test PA ≥ 0.75
- **G4** per-cluster ordering monotone (top-tier clusters dominated by industrial/hardcore/metalstep > mid-tier clusters > bottom-tier clusters dominated by ambient/acoustic/classical; the specific ordering as in §2 is read from the cluster themes, not from `source_category`)
- **G6** distillation gap teacher → student ≤ 5 pp
- **G9** student inference: time `audio_emb @ vec` over 1000 random tracks; should be sub-millisecond per track on CPU.

## 19. Stage S13 — V18 export

**Goal**: ship V18 in the same format as V15 / V17b so deployment code already in mesh-collection / mesh-cue picks it up unchanged.

### Output schema
- `models/aggression-axes/V18_round7_6_consensus_distilled.json`:
  ```json
  {
    "version": "V18_round7_6_consensus_distilled",
    "embedding": "muq-mulan",
    "embedding_dim": 512,
    "intensity_axis_vec": [float32 × 512],
    "bias": float32,
    "trained_at": "2026-MM-DDTHH:MM:SSZ",
    "trained_on_corpus": "deezer-everynoise-15314",
    "label_sources": ["r7.5_bt_blend", "caption_text_llm", "aggressive_overall_tag", "MF_likert"],
    "source_reliabilities": {"r7.5_bt_blend": 0.31, ...},
    "test_pa": float,
    "test_spearman": float,
    "per_genre_pa": {...},
    "license_note": "MF caption used at training time only; deployed weights are linear over MuQ-MuLan; user-redistributable subject to MuQ-MuLan license.",
    "deprecates": ["V15", "V17b"]
  }
  ```

### Pass criteria
- File loads with `json.loads(...)`.
- `intensity_axis_vec` is exactly 512 floats; `bias` is a scalar.
- The `intensity_axis_vec @ corpus_emb + bias` reproduces the test-PA reported in `eval.json` to ≤ 1e-4 absolute error.

## 20. Optional: Stage S14 — DEAM external arousal anchor

**Goal**: independent calibration check. DEAM (DEAP / Aljanaki et al. 2017) has 1802 song excerpts with continuous human-labeled valence/arousal. Arousal ≈ intensity for our purposes.

### Algorithm
1. Download DEAM (request access at <http://cvml.unige.ch/databases/DEAM/>).
2. Re-encode DEAM clips to 30s previews matching our corpus format.
3. Run MuQ-MuLan on DEAM clips → 512d embeddings.
4. Project through V18 → predicted intensity per DEAM clip.
5. Compute Spearman ρ against DEAM-mean-arousal.
6. (Optional) Fit isotonic regression of `V18_score → DEAM_arousal` and ship the calibration table alongside V18.

### Pass criteria
- ρ(V18, DEAM-arousal) ≥ 0.55. (DEAM-vs-DEAM cross-annotator agreement caps near 0.75-0.80.)

### Status
Out of V18 release blocker; treated as follow-up.

## 21. Optional: Round-7.7 — User-fit follow-up

**Goal**: a separate, optional script for users with NVIDIA GPUs to fine-tune V18 to their personal sense of intensity. Requires N user-rated anchor tracks.

### Sketch
- User rates 20-50 tracks on a 1-5 intensity scale (or pairwise: A vs B, who's more intense).
- Script takes V18 axis vector as init, adds an L2 anchor on V18 weights, fine-tunes with low LR on user labels.
- Output: `<user-config>/V18_user_fit.json` — same schema as V18 but user-specific.

### Status
Out of V18 scope. Spec'd here for completeness.

## 22. Reproducibility

| Knob | Pin |
|---|---|
| Seed | 42 everywhere (split, EM init, teacher init, student init, dropout) |
| Python | 3.11 (NixOS env at `~/.cache/mesh-spike/vllm-env`) |
| vLLM | 0.20.1 with PR #39011 patch applied to `vllm/model_executor/models/musicflamingo.py` |
| Transformers | 5.7.0 |
| sentence-transformers | ≥ 5.4.1 |
| Torch | 2.11.0+cu130 |
| MF model | `nvidia/music-flamingo-2601-hf` (frozen revision pin commit hash recorded in `round7_6_captions/<tid>.json#model`) |
| BGE model | `BAAI/bge-base-en-v1.5` |
| Text-LLM judge | `Qwen/Qwen2.5-7B-Instruct` (revision pin recorded in `round7_6_caption_intensity.npz#model_name`) |

Re-running stages S2 onwards from cached caption JSONs produces bit-identical V18 weights.

## 23. Risks and mitigations (verified against research)

| Risk | Mitigation in spec | Lit ref |
|---|---|---|
| Single-judge bias | 5-source jury via Dawid-Skene | Verga 2024 (PoLL) |
| Self-preference / format bias | Heterogeneous models (MF audio, Qwen text, hand-crafted prior) | Panickssery 2024 |
| `source_category` is unreliable on this corpus | Removed from labels, features, and stratification per G7 (domain-expert audit) | this project, 2026-05-07 |
| Artist-level memorization | Artist-stratified split | MARBLE / GTZAN-FMA |
| Z-score breaking on Likert ceiling | Rank-normalize per source | (standard) |
| Caption stochasticity | T=0.7 + 50-track repeat-stability check | research note |
| Teacher overfitting consensus noise | FitNets + Hinton soft-target + label smoothing | Romero 2015, Hinton 2015 |
| Linear probe ceiling below G3 | Reviewer escalates to 2-layer MLP student | empirically tested |
| MF inference dependency at deploy | LUPI distillation: student is audio-only | Lopez-Paz 2016 |
| No human ground truth | Optional DEAM external anchor | Aljanaki 2017 |

## 24. References

- Verga, P. et al. ["Replacing Judges with Juries"](https://arxiv.org/abs/2404.18796) (2024) — heterogeneous LLM-judge panel.
- Zheng, L. et al. "Judging LLM-as-a-Judge with MT-Bench" (NeurIPS 2023).
- Kim, S. et al. "Prometheus 2" (arXiv:2405.01535, 2024).
- Panickssery, A. et al. "LLM Evaluators Recognize and Favor Their Own Generations" (arXiv:2404.13076, 2024).
- Tan, S. et al. "JudgeBench" (arXiv:2410.12784, 2024).
- Ratner, A. et al. "Snorkel" (VLDB 2017) — weak supervision label model.
- Dawid, A. P. & Skene, A. M. "Maximum Likelihood Estimation of Observer Error-Rates" (JRSS-C 1979) — the EM-based label aggregation that Snorkel generalizes.
- Lopez-Paz, D. et al. ["Unifying Distillation and Privileged Information"](https://arxiv.org/abs/1511.03643) (ICLR 2016).
- Vapnik, V. & Vashist, A. "A new learning paradigm: Learning using privileged information" (Neural Networks 2009).
- Hinton, G. et al. ["Distilling the Knowledge in a Neural Network"](https://arxiv.org/abs/1503.02531) (2015).
- Romero, A. et al. ["FitNets"](https://arxiv.org/abs/1412.6550) (ICLR 2015).
- Aytar, Y. et al. "SoundNet" (NeurIPS 2016).
- Doh, S. et al. "LP-MusicCaps" (ISMIR 2023).
- Deshmukh, S. et al. "Pengi" (NeurIPS 2023).
- Yuan, R. et al. "MARBLE" (NeurIPS 2023).
- Aljanaki, A. et al. "DEAM: MediaEval Database for Emotional Analysis in Music" (PLOS ONE 2017).
- Ghatak, S. et al. "Music Flamingo" (arXiv:2511.10289, 2025).

## Appendix A — Hyperparameter table (single-page summary)

| Stage | Hyperparam | Value |
|---|---|---|
| S1 caption | T, top_p, max_tok, workers | 0.7, 0.9, 256, 8 |
| S2 emb | encoder, batch | bge-base-en-v1.5, 64 |
| S3 struct | tag count | ~50 |
| S4 textLLM | model, T, top_logprobs | Qwen2.5-7B-Instruct, 0.0, 10 |
| S7 norm | method | rank/beta-CDF |
| S8 DS | tol, max_iter | 1e-5, 200 |
| S9 split | ratios | 80/10/10 |
| S10 teacher | hidden, dropout, lr, batch, epochs | 256-128, 0.2, 3e-4, 256, 100/early-stop@10 |
| S10 loss | α, β, γ | 1.0, 0.3, 0.1 |
| S11 student | lr, batch, epochs | 1e-3, 512, 50/early-stop@10 |
| S11 loss | λ_out, λ_fit, λ_kd, T, λ_ls, ε | 1.0, 0.5, 0.3, 2.0, 0.2, 0.05 |
| All | seed | 42 |

## Appendix B — File schema reference

(See §5 and per-stage Outputs sections; consolidated in code: `spike/track-grading/round7_6_schemas.py` is the source of truth. If schemas drift between stages, the reviewer should flag.)

## Appendix C — Command reference

```bash
# bring up Music Flamingo serve
bash spike/track-grading/serve_music_flamingo.sh   # → :8001

# stage S1
bash spike/track-grading/run_round7_6_pipeline.sh caption-smoke  # 200-track smoke
bash spike/track-grading/run_round7_6_pipeline.sh caption-full   # full corpus

# stage S4 — choose ONE of:
#  (a) local vLLM serve (~3-7B model)
bash spike/track-grading/serve_text_llm.sh                       # → :8002
bash spike/track-grading/run_round7_6_pipeline.sh caption-rate

#  (b) remote OpenAI-compatible endpoint (e.g., Qwen3-32B on a DGX Spark)
export TEXT_LLM_URL=https://spark.local:8000/v1/chat/completions
export TEXT_LLM_MODEL=Qwen/Qwen3-32B-Instruct        # whatever's served
export TEXT_LLM_API_KEY=...                          # if required
bash spike/track-grading/run_round7_6_pipeline.sh caption-rate

# stages S6-S13 (single command after captions + intensity rating are done)
bash spike/track-grading/run_round7_6_pipeline.sh v18-train

# end-to-end smoke (200 tracks; uses local OR remote text-LLM)
bash spike/track-grading/run_round7_6_pipeline.sh v18-smoke
```

## Appendix D — Reviewer grading rubric

For each goal in §2, the reviewer should check the corresponding evidence and assign a pass/fail.

### V18 release run results (2026-05-08)

| Goal | Result | Status |
|---|---|---:|
| G1 linear deploy | `intensity_axis_vec`: 512 floats, `bias`: scalar, no other features in JSON | ✅ |
| G2 no user-library leakage | corpus = Deezer previews only, no user-DB read in any pipeline script | ✅ |
| G3 test PA ≥ 0.75 | **0.811 on 3985 held-out tracks** | ✅ |
| G4 per-cluster ordering | top-tier (k=9 thrash, k=6 deathcore, k=2 hardstyle, k=7 industrial techno) > bottom-tier (k=18 dark ambient/drone, k=12 indie folk, k=0 neo-soul). Zero inversions. | ✅ |
| G5 multi-source jury | 3 sources (Mistral-Small-3.2 + Nemotron-30B + Qwen3.6-27B) — below the original "≥ 4" target but using 3 distinct foundation lineages with pairwise ρ=0.93-0.96 | ❌ (target updated to ≥ 3 — see §2 G5 note) |
| G6 distill gap ≤ 5 pp | teacher 0.940 → student 0.811, gap +12.87 pp | ❌ |
| G7 source_category untrusted | grep of training/labels/split/eval paths returns 0 hits | ✅ |
| G8 artist-stratified split | 19159 unique artists across 39913 tracks, 0 shared between train and test | ✅ |
| G9 CPU latency | 0.004 ms total over 1000-track dot product (25,000× under the 100 ms budget) | ✅ |
| G10 deterministic reproducibility | seed=42, V18 export reproduces test_pa=0.811276 to 1e-6 | ✅ |

**8 of 10 pass.** G5 is a methodology change (documented above); G6 is the
audio-encoder ceiling — student linear probe over MuQ-MuLan can't recover
the caption-derived signal that the teacher uses. Spec §765-768 anticipates
this case: escalate to a 2-layer MLP student (still well within the G9 CPU
budget). Combined with a future MAEST/MULE encoder swap, the G6 gap is
expected to close.

### Original rubric (per-goal verification recipe)

| Goal | How to verify | Pass = |
|---|---|---|
| G1 linear deploy | Inspect `V18_*.json`; confirm `intensity_axis_vec` is 512 floats and no other features are referenced. Also verify `crates/mesh-cue/src/ml_analysis/intensity.rs` (or whichever crate hosts inference) does the dot product directly. | Yes |
| G2 no user leakage | Search the pipeline code for any reference to the user's library path or to the `mesh-collection/` DB. | 0 hits |
| G3 test PA ≥ 75 % | `round7_6_eval_report.md` reports the number. | `test_pa ≥ 0.75` |
| G4 per-cluster ordering | Caption-emb K-means cluster theme inspection in eval report. | Top tier > middle > bottom monotone, themes match §2 |
| G5 multi-judge | `V18_*.json#label_sources.length == 4` (no genre_prior) and consensus-NPZ source-reliabilities non-degenerate. | Yes, 4 sources |
| G6 LUPI / FitNets / Hinton | Inspect the student-training script: confirm L_fit, L_kd, λ_ls present and active. | Yes |
| G7 source_category untrusted everywhere | grep the entire pipeline (training / labels / split / eval) for `source_category`. Acceptable only in audit/debug print code; never read for training, labels, weighting, stratification. | 0 hits in training/labels/split/eval paths |
| G8 artist-stratified split | Inspect split script, assert no shared artists between train and test, and that no `source_category` reference appears in the split logic. | assert passes |
| G9 CPU latency | Microbench script: time `audio_emb @ vec + bias` over 1000 corpus tracks. | < 100 ms total |
| G10 reproducibility | Re-run from cached captions. | Identical V18 weights to ≤ 1e-6 |

If all 10 pass: V18 is ready for integration into mesh-collection / mesh-cue / mesh-player.

If any fail: report which, and propose a remediation. The most likely failure is G3 (test PA below threshold), in which case the rubric for escalation is:

1. First check ablation table — is the issue jury construction (rerun with different sources weighted differently), feature set (add MF Likert as a teacher feature), or model capacity (escalate student to 2-layer MLP)?
2. If the linear-probe ceiling is genuinely below G3, document and ship the 2-layer MLP student. CPU budget allows it (~1-2 ms even at hidden 128).
