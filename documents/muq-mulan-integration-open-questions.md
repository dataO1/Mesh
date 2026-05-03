# MuQ-MuLan integration — open questions for review

Branch: `muq-mulan-integration` (off `muq-mulan-eval` at `94546c6`).

This is the **agreement-discussion checklist** for the parts of the
integration that have a defensible default but where you may want to
tune things once you've run reanalysis on the full corpus and looked at
suggestions / graph / aggression behavior.

Each item is independent — pick any to discuss, the others can ride.

---

## 1. Per-track clip count: `MUQ_MULAN_MAX_CLIPS = 6`

`crates/mesh-cue/src/ml_analysis/inference.rs:53`

PyTorch's `extract_audio_latents` averages every non-overlapping 10 s
clip — for a 4-min track that's 24 clips. We cap at **6 evenly-spaced**
clips (~1.7 s of CPU per track at 290 ms/clip).

Trade-off:
- Higher (12 / 24): closer to PyTorch reference; reanalysis 2–4× slower.
- Lower (3): faster; risk of missing rare-but-defining sections (e.g. a
  short hard-techno breakdown that defines the track).

**To decide:** after reanalysis is done, sample 10 tracks from your
collection where the suggestion neighbors look "off" and check whether
upping the clip count changes them. Easy A/B.

## 2. Mel pipeline numerical drift vs torchaudio

`crates/mesh-cue/src/ml_analysis/preprocessing.rs:7`

We re-implemented MuQ's `MelSTFT` in pure Rust (24 kHz / n_fft=2048 /
hop=240 / 128 HTK mel / power=2 / center=True reflect / AmplitudeToDB
top_db=80 / trim last frame). The integration test confirms sine vs
noise produce distinct embeddings (cosine = 0.596). What we **haven't**
proven is that the per-frame mel values match torchaudio's bit-for-bit.

Likely-acceptable sources of drift:
- realfft vs torch FFT — single-precision rounding noise.
- Reflect-padding boundary handling.
- Anti-alias resample (windowed-sinc Hann) vs torchaudio's resample.

If neighbor quality looks degraded vs the spike's cosine=1.000 result,
the most likely culprit is the resample step (when source is 48 kHz).
Easy validation: write a tiny script that runs the same WAV through
both pipelines and compare per-frame mels — would surface drift fast.

**To decide:** worth doing this validation now, or trust the model's
robustness to mel perturbations and only investigate if neighbors
look bad?

## 3. Genre tagging — fully dropped, callers degrade silently

`crates/mesh-core/src/db/schema.rs:262` — `MlAnalysisData.top_genre` /
`genre_scores` kept for back-compat with old rows but always written as
`None` / empty under MuQ-MuLan.

Affected paths:
- **Auto-tag from ML** (`batch_import.rs:947`): the genre tag-creation
  loop iterates over an empty `genre_scores` → no genre tags applied.
  Existing genre tags from prior MAEST runs are NOT cleared by reanalysis;
  they linger until the user explicitly clears them.
- **Aggression scoring** (`suggestions/aggression.rs:330,734,1045`):
  takes a `genre_labels: HashMap<i64, String>` → MuQ-MuLan path passes
  empty → genre-aware tier construction degrades to neutral baselines
  (covered by `build_calibration_plan_without_genre_labels` regression
  test).
- **Graph community macro labels** (`graph_compute.rs:1112`): same —
  empty labels → "Other" macro for everything → community color/grouping
  in the graph view loses genre semantics.

**To decide:**
  a) Leave as-is (genre semantics dormant, neighbors driven by audio
     embedding cosine alone).
  b) Add a one-time "clear stale ML genre tags" UI button that wipes
     the residual MAEST-era tags so the visual state matches the new
     ML pipeline.
  c) Repurpose `top_genre` slot for some MuQ-MuLan-derived signal
     (e.g. nearest neighbor's user-applied tag majority — semi-supervised
     auto-tagging from your existing tag work). Bigger lift.

## 4. Aggression axis — needs re-fit on 512-d embeddings

`crates/mesh-core/src/suggestions/aggression.rs`

The aggression-axis Pearson fit was last run against MAEST's 2304-d
embeddings (then PCA'd to ~131-d). After your reanalysis completes,
the PCA rebuild fires automatically (different dim → ml_pca_embeddings
gets dropped) but the **aggression axis weights aren't auto-rebuilt** —
they're fit through the calibration pair flow.

You'll want to:
1. Confirm the calibration UI still works (it should — it operates on
   PCA dims regardless of source ml dim).
2. Re-run a calibration session of ~10–20 pairs to fit a fresh
   aggression axis on the new 512-d → PCA-reduced space.

**To decide:** is there value in seeding the new fit from old
calibration pairs (re-evaluating cosine similarity against the new
embeddings) or is a fresh user calibration cleaner?

## 5. PCA target dim under 512-d input

`crates/mesh-core/src/db/service.rs:967`

PCA auto-selects target dim via 95% explained-variance. Under MAEST
(2304-d) this landed around 131-d. Under MuQ-MuLan (512-d input,
already l2-normalized + averaged across clips) it'll likely land
much lower — possibly 30–60-d. Need to confirm post-reanalysis.

**To decide:** is the 95% threshold still right? With a smaller input
dim and l2-normalized output, the variance landscape is different.
Could lower or raise the threshold for better suggestion behavior;
no defensible default until we see the actual eigenvalue distribution.

## 6. Suggestion bell-σ tuning

`crates/mesh-core/src/suggestions/query.rs` — the recently-tuned
"widen bells" parameters (sim 0.18, aggr 0.22, see commits
`6bd1972` / `045aba0` on main).

Those were tuned for MAEST embeddings. MuQ-MuLan's cosine distribution
across your library will likely be **different** — possibly more
spread out (since the model learned on text-conditioned contrastive
loss, not classification). Suggestions might feel too tight or too
loose initially.

**To decide:** schedule a re-tuning pass once you've done a few
suggestion sessions on the new embeddings — same diagnostic logging
infrastructure as before (`e2ce7db chore(suggestions): expanded
scoring diagnostics for tuning`) is still in place.

## 7. CI release packaging — currently manual

`.github/workflows/release.yml` (chore commit `0381fd0`).

Released `.deb`/`.zip` artifacts no longer bundle EffNet/MAEST/Jamendo
ONNX. They also do **not** bundle MuQ-MuLan (1.2 GB ONNX needs
PyTorch 2.5+cu124 + 2.65 GB HF weights at conversion time — too heavy
for GitHub-hosted runners).

End users currently have to run `nix run .#convert-muq-mulan-model`
once on first install. mesh-cue auto-installs the result into
`~/.cache/mesh-cue/ml-models/`.

**To decide:**
  a) Stay manual; document in release notes.
  b) Provision a self-hosted GPU runner for the conversion.
  c) Build the ONNX once locally, host it on the existing GH "models"
     release, have mesh-cue download from there (matches the prior
     MAEST UX). This is the least-effort change — adds ~1.2 GB to the
     models release and an HTTP download path to `models.rs`.

## 8. USB sync of the new dim

USB sync code is dimension-agnostic (copies via `get_ml_embedding_raw` →
`store_ml_embedding`, schema is recreated on USB at first connection).
**No code changes needed**, but **existing USB sticks** still hold
2304-d MAEST embeddings — first sync after this branch will:
1. Drop+recreate `ml_embeddings` on the USB DB at 512-d.
2. Re-copy 512-d vectors from the local DB.
3. mesh-player on the USB will see the new HNSW + index.

**To decide:** sanity-check this on a real USB stick after local
reanalysis is complete — confirm the player loads the new vectors and
similarity-by-track-id queries work.

## 9. Stale `database is locked` log on rare reanalysis writes

Spotted in your run:
```
WARN reanalyze_metadata_track: Failed to store stem energy: Query("database is locked (code 5)")
```

Pre-existing; not introduced by this branch. Just flagging it as a
follow-up — concurrent stem-energy writes occasionally race during
high-throughput reanalysis. If it shows up frequently after this
reanalysis pass, worth a small retry-with-backoff fix.

## 10. Phase 4 (Multimodal Audio LLM) — design only, blocked on this

`documents/embedding-models-research.md::Phase 4`

Phase 4 (caption-augmented graph fusion) was scoped to land on top of
either MAEST or MuQ-MuLan. With MuQ-MuLan now active, Phase 4 design
should be revisited — particularly the "caption embedding fused with
audio embedding" decision, since MuQ-MuLan's joint audio+text space
already gives us a free text-query path that overlaps with what an
LLM-caption would provide.

**To decide:** is Phase 4 still the right next step, or does
MuQ-MuLan's text-tower (currently deferred — `extract_text_latents`
isn't exported yet) cover most of the value at much lower cost?

---

## Quick-win order of operations once reanalysis finishes

1. **Verify**: open a track in mesh-cue → "Suggestions" → confirm the
   neighbors look reasonable (item 6).
2. **Rebuild PCA + Calibrate**: trigger PCA index build, then run a
   short calibration session for the aggression axis (items 4, 5).
3. **Inspect graph**: open the graph view → confirm clusters form and
   tracks position sensibly. Communities will lose genre-color labels
   (item 3) — that's expected, not broken.
4. **USB sync test**: plug in a USB and sync (item 8).
5. **Loop me back here** with anything that looks off — most items
   above have a defensible default but several land in the "right
   answer depends on what you see" category.

---

## Update 2026-05-03 — text-tower intensity-axis pipeline

Branch `text-tower-aggression-axis`. See
`documents/aggression-axis-text-tower-plan.md` for the full design.

**Item 4 (aggression axis re-fit) is now resolved by a different mechanism**
than that section anticipated: instead of re-fitting Pearson weights via
calibration pairs, the axis is now a **precomputed intensity vector**
derived from MuQ-MuLan's text tower over polar prompts at design time.
Runtime cost identical (one dot product on the 512-d audio embedding).

What landed:
- **7 axis variants** generated by `nix run .#derive-aggression-axes`,
  living in `models/aggression-axes/`. Each variant is a different
  combination of 5 sub-axes (aggression / distortion / density /
  darkness / noisiness). Cross-variant cosine similarity matrix
  printed at derive time confirms variants disagree meaningfully —
  V3 (pure density) vs V7 (dark+noisy emphasis) at +0.05, near
  orthogonal. V5 (aggression-led blend) vs V1 (pure aggression) at
  +0.96, near-redundant.
- **V5_aggression_led** is the current default
  (`models/muq-mulan-aggression-axis.json` is a copy). Swap with
  `scripts/select-active-axis.sh <variant_id>`.
- **Eval CSVs** in `documents/axis-eval-results/` (one per variant +
  `combined.csv`). 883 tracks ranked descending by intensity, with all
  sub-axis projection scores included. Spot-check from the V7 run:
  top tracks are Hyper / Joe Ford / Neonlight (drum-and-bass, dark techno);
  bottom tracks are ZHU "Faded" / NTO / Oliver Koletzki (deep house /
  ambient). Looks musically right out of the box.
- **Calibration UI kept as-is for now** (TODO below). The
  `pca_aggression_axis.correlation` field is repurposed as
  calibration-pair agreement rate (eval-only signal).

**Validation/discussion TODOs** (do these before deciding the final variant):
1. Eyeball the per-variant CSVs in `documents/axis-eval-results/`. The
   most useful comparison: pick 5–10 known-extreme tracks from your
   collection and check whether each variant ranks them where you'd
   expect. Variants that rank ZHU "Faded" near zero are probably
   correct; variants that rank it negative are probably even better.
2. Optional: feed the CSVs to a second LLM agent ("here are 7 different
   intensity rankings of my 883-track collection — which ranking best
   matches a DJ's intuition for set-building intensity?") for an
   independent opinion.
3. Decide the final variant. Activate via
   `scripts/select-active-axis.sh <id>`. Trigger "Build Similarity
   Index" in mesh-cue afterward to refresh `pca_aggression_axis`.
4. **Calibration UI rescope** (deferred from this branch — explicitly
   requested kept-as-is for now): rewrite the modal to be eval-only.
   Pairs continue to be stored, but they no longer drive weight
   fitting; instead they feed the agreement-rate metric. See
   `aggression-axis-text-tower-plan.md::Calibration UI` section options.
5. Evaluate whether the polar-prompt axis is enough or if Option 3
   (LLM-labelled tag heads) is still worth the GPU-hours. The polar
   axis is a stronger baseline than anyone has currently — re-decide
   after a few suggestion sessions.

**Operational notes:**
- The text tower itself never ships to end users. The dev box runs
  `derive-aggression-axes` once, commits the variant JSONs.
  `models/muq-mulan-aggression-axis.json` (~3 KB) is the only artifact
  end users need; it lives next to the ONNX in the GH `models` release
  and is auto-installed by `MlModelManager::install_to_cache`.
- Schema unchanged — `pca_aggression_axis` is already `[Float]`,
  variable-length list, fits 512-d as easily as the prior 27-d Pearson
  weights.
