If unsure ask questions, dont assume stuff. Try to make sure that logic lies
as much as possible in mesh-core and mesh-widget and only if necessary in the ui.

7. **Built-in native effects** — Beat-synced echo, flanger, phaser, gater.
   Tighter integration than CLAP/PD. See [Audio Processing](#audio-processing).

Post-v1.0: B2B mode, history-informed suggestions, set reconstruction UI,
slicer morph knob, jog wheel nudging.

---

# Suggestion Graph View (mesh-cue) — DONE

Implemented. Interactive graph tab in mesh-cue browser. Key deviations from
original plan:

- **Scoring moved to mesh-core** (not mesh-player) for sharing with mesh-cue
- **Reward-based scoring** replaces penalty-based (higher = better)
- **Brute-force PCA cosine** replaces HNSW approximate search (all tracks scored)
- **t-SNE clustering** for library visualization (Barnes-Hut via bhtsne)
- **SuggestionBlendMode** replaces dead Goldilocks settings (Target/Focus)
- **No separate key_dir** — merged into key_transition_score

### Remaining improvements

- [x] t-SNE for initial no-seed library view (Barnes-Hut via bhtsne)
- [x] Clicking a suggestion in the left panel track table loads it as seed
- [x] Persist graph positions across tab switches
- [ ] Show track waveform preview on hover
- [ ] Audio preview on click-and-hold a node
- [ ] Lasso selection to filter tracks by graph region
- [ ] Color mode toggle (score / key / genre / intensity)
- [ ] In graph view set preparation, selecting a track could visually render
  its waveform overview underneath the previous track waveform, for visual
  comparison of time-domain fit
- [ ] Shift-select with a filter selects everything not visible via the filter
  in between

---

# Set Analysis & DJ Intelligence

Analytics derived from session history (`track_plays`, `played_after` relations)
and graph exploration breadcrumb trails. Most metrics are computable from existing
stored data — gaps noted below.

## Data already stored (per track play)

session_id, loaded_at, track_id, track_path, track_name, deck_index,
load_source ("browser"/"suggestions"), suggestion_score, suggestion_tags_json,
suggestion_energy_dir, play_started_at, seconds_played, played_with_json,
hot_cues_used_json, loop_was_active.

## Data gaps (need to capture)

- [ ] **Suggestion rank position**: Which position in the top-30 list the DJ
  picked. Add `suggestion_rank: Option<u32>` to `TrackPlayRecord`.
- [ ] **Full suggestion context per load**: Store the top-30 IDs + scores shown
  at each load event (~1KB). Enables negative example learning (shown but
  not picked = implicit rejection).
- [ ] **Persist graph breadcrumb trails**: Currently memory-only in mesh-cue.
  New DB relation: `graph_trails { session_id, step, track_id, energy_dir }`.

## Statistical dashboard (per session)

Computable from existing data, no ML needed:

- [ ] **Key compatibility rate**: % of transitions with Camelot distance ≤ 1.
- [ ] **Key transition distribution**: Histogram of Camelot distances (0–6).
- [ ] **BPM progression**: Plot BPM per track across session. Stddev = genre focus.
- [ ] **Genre entropy**: Shannon entropy over genre distribution.
- [ ] **Energy arc smoothness**: Mean absolute 2nd derivative of intensity curve.
- [ ] **Selection depth**: Average rank of selected suggestion (needs rank data).
- [ ] **Suggestion vs browse ratio**: load_source distribution per session.

## Energy arc visualization

- [ ] **Multi-layer timeline**: Canvas widget showing intensity + BPM + key
  progression stacked. Color strip for key (Camelot colors).
- [ ] **Chapter detection**: Segment energy curve into monotonic regions.
  Label: low+rising = warm-up, peak = climax, high+falling = release.
- [ ] **"Complete my set"**: Given partial trail, suggest tracks for remaining
  chapters using suggestion engine with appropriate energy_direction.

## Style fingerprinting (novel — no DJ software does this)

- [ ] **Genre loyalty length**: Average consecutive tracks in same genre cluster.
- [ ] **Key walk smoothness**: Entropy of Camelot distance distribution.
- [ ] **Energy direction bias**: Average slope of energy curve across sessions.
- [ ] **Exploration breadth**: Unique graph clusters visited per session.
- [ ] **Radar chart**: Render style vector as a spider plot.

## Cluster heatmap

- [ ] **Visitation frequency**: Join track_plays with t-SNE positions. Color-code
  graph nodes by play count (hot = frequently played, cold = never).
- [ ] **Coverage metric**: "You've explored 34% of your library's stylistic space."
- [ ] **Underexplored suggestions**: Boost tracks from clusters adjacent to
  frequently visited clusters but never visited themselves.

## Transition pattern learning (needs data accumulation)

- [ ] **Personalized re-ranker**: Logistic regression over transition features
  trained on positive/negative examples from suggestion context. Per-DJ model.
- [ ] **Preference surfacing**: "You prefer adjacent key walks (72%) over
  same-key (18%). You rarely jump > 4 BPM."

---

# Smart Suggestions & Library Intelligence (v3)

## Transition Graph — History-Informed Suggestions — DONE

- [x] **Store track IDs in co-play records**
- [x] **Materialize `played_after` graph relation** with time-decay at query time
- [x] **Co-play score bonus in suggestion scoring** (weight 0.07, center only)
- [x] **Played-after row highlight in file browser**

---

## Library Community Detection — Smart Playlists

t-SNE clustering already visualizes library structure in the graph view.
These features would make clusters queryable and actionable:

- [ ] **Auto-generated "Sound Clusters" in browser sidebar**: extract t-SNE cluster
  assignments, list each cluster with track count. Selecting one filters the
  browser to that cluster — automatic genre/vibe grouping without manual tagging.

- [ ] **New Music Discovery mode**: "Explore" toggle that suggests one
  representative track from each *neighboring* cluster instead of the seed's own.

- [ ] **2D cluster scatter plot** with X = spectral centroid, Y = intensity,
  colored by cluster assignment. For set planning.

---

## PCA Dimension Reduction — DONE

- [x] **"Build Similarity Index" action** (mesh-cue): PCA on EffNet 1280-dim
  embeddings, auto-detects dimensionality via 95% explained variance (60-128 dims).
  `ml_pca_embeddings` with dynamic `[Float]` list. Brute-force cosine distance
  replaces HNSW — all tracks scored exactly. Old 16-dim vector fully removed.

---

## Session Energy Arc — DONE

- [x] **Energy arc ribbon** in browser analytics panel: vertical = intensity,
  width = spectral jump, color = key transition quality. Uses
  `composite_intensity_v2()` from 10-component IntensityComponents.

---

## Intensity Scoring v2 — DONE

- [x] **10-component IntensityComponents**: spectral_flux, flatness, centroid,
  dissonance, crest_factor, energy_variance, harmonic_complexity, spectral_rolloff,
  centroid_variance, flux_variance. Full-track FFT analysis (~4,650 frames for
  a 4-minute track). Raw values stored without artificial scaling — percentile-rank
  normalization at query time ensures equal component contribution.
- [x] **Spectral gradient** replaces peak counting for harmonic_complexity
  (Essentia SpectralComplexity style — doesn't saturate for dense electronic music).
- [x] **All multi-frame**: centroid + energy_variance computed from full-track FFT
  (was single-frame Essentia placeholders). No Essentia subprocess dependency for
  intensity — pure Rust realfft only.
- [x] **4 intensity tag groups**: Texture (Choppy/Smooth), Grit (Gritty/Clean),
  Density (Dense/Punchy), Brightness (Bright/Dark). Top/bottom 20% outliers
  shown as pills in Other stem color. Max 2 per track.
- [x] **Legacy cleanup**: Removed 16-dim AudioFeatures, binary mood classifiers
  (5 models), Beat This! ML beat detection, composite_intensity v1,
  normalize_intensity_by_genre, batch_get_flatness/dissonance.

---

## Dual-Deck Context-Aware Suggestions — DONE

- [x] **Blend-aware seed selection**: blend mode averages PCA vectors,
  transition mode uses outgoing deck. Linear interpolation between modes.

---

## Intro / Set-Opener Suggestions — DONE

- [x] **Opener quality scoring**: when no deck is playing, rank by intro
  length, vocal-free intro, intensity delta, stem balance.

---

## Suggestion Feedback & Tuning (future)

- [ ] Collect per-selection feedback (seed, slider positions, selected track,
  rating). After several sessions, evaluate which scoring components correlate
  with good transitions.

---

# Features

## Collection Browser
- [ ] Tag editing UI: adding, removing, editing tags with autocomplete + color picker.

## MIDI
- [ ] optional: Jog wheel beat nudging for older devices (SB2 etc.)
- [ ] Arrow key workflow as alternative to encoders for hardware without them.

## Slicer
- [ ] optional: Single morph knob per deck scrolling through preset banks.

## B2B Mode (post-v1.0)
- [ ] Two mesh systems connected via ethernet, each showing partner's waveforms,
  shared master clock, cross-library browsing + suggestions.

## Smart Suggestions (v3 — Future)

- [x] Session history + co-play graph + time-decayed scoring
- [ ] Pattern mining from play history (PrefixSpan, GRU4Rec, etc.)
- [ ] Negative signals (tracks played < 30s get soft penalty)
- [ ] DJ profile divergence for B2B / shared USB

## DJ History & Playlists
- [x] Session history persisted to all active DBs
- [ ] Set reconstruction UI (timeline view, export as tracklist)
- [ ] Database backup (DB only, no wav files)
- [ ] Session import from USB sticks ("sync" instead of just "export")
- [ ] Per-DJ history divergence for shared collections

## Audio Processing
- [x] Live peak meter per channel and master channel.
- [ ] Built-in native effects (beat-synced echo, phaser, reverb, filter).

## Documentation
- [ ] Proper structured README + linked docs (collection, MIDI/HID mapping,
  effects, embedded BOM + setup).

# Bugs

# Stubbed / Deferred

# Performance

# Open Questions

# Auto Headphones Cue system — DONE
- [x] Auto-cue: tracks at volume < 30% sent to headphone out (logarithmic curve).
  Configurable in player UI, only active when master/cue are different outputs.

# DB
- [ ] Database versioning system for schema migrations and USB backwards compat.

# UPDATE LIFECYCLE
- [ ] WiFi: check stored credentials first, reconnect without password entry.

## Embedded: Silent Boot (investigated, partially working)
- [x] Plymouth removed, silent boot params applied.
- Future: raw framebuffer splash or U-Boot CONFIG_SPLASH_SCREEN.

# OTHER
- [ ] Touch support (screen or iced limitation?)
- [ ] Smart suggestions for stem linking: per-stem weighting (drums = energy
  focused, vocals = key mandatory, bass/other = default weights).
- [ ] Graph clustering: quality-driven community-count tuning.
  Current behavior (shipped): Louvain parameters (γ, min_cluster_size)
  scale with library size (see `LOUVAIN_*` constants in
  `crates/mesh-core/src/graph_compute.rs`). Works because "more tracks
  usually means more subgenres", but can misfire on a focused 10k-track
  library (too many communities) or a diverse 300-track one (too few).
  Optional improvement: after Louvain, compute a quality metric
  (intra/inter mean-distance ratio, silhouette score, or modularity Q),
  binary-search γ until the metric crosses a target threshold. Must
  combine with a min/max community-count floor/ceiling so a continuous
  library doesn't over-fragment. ~250ms total cost (4-6 Louvain runs at
  ~50ms each). Evaluated: more principled but harder to debug and
  tune — ship only if size-scaling turns out to miss in practice.
- [ ] Graph clustering: user-facing granularity slider (Coarse /
  Default / Fine) mapping to (γ, min_cluster_size) presets. Gives DJs
  direct control over "how many buckets" without needing a quality heuristic.

# Research
Also evaluate potential use cases of multi-modal LLMs like the Nemotron 3 Nano
Omni. can they be of any additional value for this? maybe for example for the
aggression analysis, refinement of genre classification, finding similarity
outliers etc? in addiion to the embeddings, for uers, that have a strong GPU,
like me.

---

# Embedding Model Upgrade

## Quick win — MAEST-30s-pw-519l-2 (replaces EffNet on `embeddings-upgrade`)

Same vendor (MTG-UPF), official ONNX, larger Discogs taxonomy (519 vs 400
styles), trained on 4M tracks vs 3.3M. **Embedding is 2304-dim** on this
branch (CLS|DIST|mean@layer7 stack — paper-recommended pooling, not
CLS-only). Same CC-BY-NC license. CPU cost ~1.5–4s per track vs EffNet's
50–200ms — still inside seconds-per-track import budget.

Branch is **MAEST-only**: EffNet plus all 9 classification heads
(timbre/tonal/danceability/approachability/mood_acoustic/mood_electronic/
JamendoMood/voice/reverb) were removed wholesale at user direction. Heads
can be retrained on top of MAEST embeddings later if needed.

- [x] Swap `discogs-effnet-bsdynamic-1.onnx` for `discogs-maest-30s-pw-519l-2`
  in `crates/mesh-cue/src/ml_analysis/`. Input `[1, 1876, 96]` mel @ 16 kHz,
  pooled output 2304-dim from `PartitionedCall/Identity_7`.
- [x] Remove all classification heads (`MlModelType`, `MlAnalyzer`,
  `MlAnalysisData`, `auto_tag_from_ml`, suggestions/aggression mood input,
  Track table Timbre/Danceability columns, db_inspect/intensity_report/
  pca_aggression bin references, USB sync).
- [x] DB schema migrated: `ml_embeddings` widened `<F32; 1280>` → `<F32; 2304>`,
  `ml_analysis` collapsed to `{track_id => top_genre, genre_scores_json}`,
  HNSW index recreated at dim 2304.
- [x] `cargo check --workspace --no-default-features --all-targets` passes.
- [ ] **User test:** trigger re-analysis on a small set of tracks, verify
  2304-dim embeddings populate, sanity-check HNSW similarity rankings,
  observe import-time regression. Findings → `documents/embedding-models-research.md`.
- [ ] Re-tune Goldilocks bell-σ zones (recent commits 6bd1972, 045aba0
  calibrated to EffNet's 1280-d distance distribution — full re-calibration
  pass needed against MAEST 2304-d).
- [ ] Rebuild PCA + t-SNE for the graph view (ml_pca_embeddings is dynamic-dim
  so no schema change, but the projection itself needs rebuilding).
- [ ] Re-derive aggression axis from MAEST embedding (currently 0.0 placeholder
  for the mood-tag term in `suggestions/aggression.rs`).
- [ ] Add LAION-CLAP `music_audioset_epoch_15` (or distilled DCLAP, ~30MB
  ONNX) as a *parallel* index for text→audio query ("dark hypnotic techno
  ~132 BPM"). 512-dim joint space, doesn't disturb similarity pipeline.

### Candidate model comparison

| Model | Params | Dim | ONNX | CPU/30s | License | Verdict |
|---|---|---|---|---|---|---|
| **MAEST-30s-pw-519l-2** | 87M | 768 | ✅ official | 1.5–4s | CC-BY-NC | Primary upgrade |
| **MULE** (Pandora) | 62M | 1728 | ❌ TF only | 0.5–2s | GPL+CC-BY-NC | Best MARBLE genre, worse license + export work |
| **LAION-CLAP music_audioset** | 80M | 512 | ✅ via optimum | ~1s | MIT/CC0 | Complement only — text→audio query |
| MERT-v1-95M | 95M | 768 | ❌ no official | 5–8s | CC-BY-NC | Worse than MULE on genre, 5–10× slower |
| MuQ-MuLan | 700M | — | ❌ | 5–10s | MIT/CC-BY-NC | Best perceptual numbers (72.4% ABX), no ONNX |
| MusicFM-MSD | 330M | 1024 | ❌ | 5–10s | MIT/Apache | Strong on chords/beats, irrelevant to similarity |
| OMAR-RQ | 580M | — | ❌ | 10s+ | AGPL+CC-BY-NC | Not viable — license, size, novel quantizer ops |

## Future — pure-quality embedding upgrade (architecture change accepted)

Rerank by **embedding quality alone** (still constrained to CPU-feasible local
inference within tens of seconds per track, all weights distributable). License
and ONNX availability treated as integration tax, not disqualifiers.

| Rank | Model | Why it ranks here |
|---|---|---|
| 1 | **MuQ-MuLan** (700M) | Highest published perceptual-similarity number on Inst-Sim-ABX (72.4% triplet agreement on full mix, 90.4% with stem-separation reweighting — beats LAION-CLAP 71.9% / 83.2%). MagnaTagATune zero-shot ROC-AUC 79.3 (SOTA at publication). Music+text joint space gives text-query for free. PyTorch-only and ~10s CPU/track is the cost. |
| 2 | **OMAR-RQ** (580M) | Newest MTG-UPF SSL model (ACM MM 2025), trained on 330K hours of Discogs-flavored YouTube audio. Best open SSL on MTG tagging mAP, pitch, chord, beat, structure. AGPL + novel RVQ/FSQ ops make integration painful but quality is top-tier. |
| 3 | **MULE** (62M) | Beats every MERT variant on MTG-Jamendo genre (88.0 ROC / 20.4 AP), ties Jukebox-5B at 1/80th the size. Contrastively trained at Pandora specifically for similarity/playlist generation. CPU-cheapest of the high-quality options. TF→ONNX export is the integration cost. |
| 4 | **MAEST-30s-pw-519l-2** (87M) | Same-vendor successor to EffNet, supervised on Discogs taxonomy. Strongest electronic-music coverage by construction. Loses to MuQ/OMAR-RQ on cross-domain music-IR but wins on Discogs-style genre alignment specifically. |
| 5 | **MusicFM-MSD** (330M) | Best published numbers on chords/beats/structure. Similarity isn't its strength — picks up here only if Mesh wants to consolidate beat detection (currently Beat This!) and chord/key analysis into one backbone. |
| 6 | **MERT-v1-330M** (330M) | Strong general MIR backbone but MARBLE numbers don't beat MULE on genre. Useful only if frame-level (75Hz) features are wanted for downstream tasks. |
| 7 | **LAION-CLAP** (80M) | Excellent for the text-query path but weaker than music-pretrained models for pure audio-audio similarity. Stays as the complementary text encoder, not the primary embedder. |

Top-quality realistic path: **MuQ-MuLan as primary similarity + MULE as
HNSW-indexed fallback for the long tail + LAION-CLAP for text query.**
Two-stage retrieval (cheap MULE recall → MuQ-MuLan rerank top-K) avoids
running the 700M model over the full library on every query.

---

# Round-7.6 V18 — General-purpose intensity axis (caption-as-feature + judge-jury distill)

**Single source of truth**: `documents/round-7-6-pipeline-spec.md` (the spec
document the reviewer grades the implementation against). The summary below is
a working checklist; the spec is the contract.

**Goal**: a CPU-deployable linear probe over MuQ-MuLan (or future MAEST) that
produces a single scalar intensity score, generalizing across DJ genres + adjacent
(electronic/dance/DnB/techno/house/rock/ambient). NOT user-library-fitted —
that's an optional follow-up tool for users with NVIDIA GPUs (see "Round-7.7
optional user-fit pipeline" below).

Methodology grounded in established research patterns (Verga et al. 2024 PoLL,
Lopez-Paz et al. 2016 LUPI distillation, Romero et al. 2015 FitNets, Ratner et
al. 2017 Snorkel, Hinton 2015 KD). See Obsidian
"Mesh — Caption-as-Feature Methodology" for the full design + citations, and
`documents/round-7-6-pipeline-spec.md` for the per-stage spec + grading rubric.

## Pipeline (V18-fresh, NOT user-fit)

1. **Caption sweep** (~8 hr at 0.5 c/s on RTX 5090 Mobile bf16)
   - All 15314 Deezer tracks → MF rich captions, T=0.7 top_p=0.9 max_tokens=256
   - Atomic JSON cache, resume-safe
   - Outputs: `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/`

2. **Per-track feature extraction** (~10 min total, CPU)
   - bge-base-en-v1.5 sentence embedding over caption → 768d
   - Regex/keyword extraction of structured tags (instrumentation, mood, vocal type, ~50 multi-hot dims)

3. **Build consensus intensity label** via heterogeneous LLM-judge jury (4 sources)
   - r7.5 BT priors blended into 1d intensity (existing V17b path)
   - MF Likert 16-axis (existing 200-track smoke; extend cheaply if needed; ~3 hr if full)
   - MF caption → text-LLM intensity rating (1-5 with logprobs; small text LLM swap on vLLM, ~30 min for full corpus)
   - r7.5 mined-tag `aggressive_overall` evidence (already on disk, free)
   - **Aggregate** via Snorkel / Dawid-Skene (learned per-source reliability), NOT fixed weighted-mean
   - **Rank-normalize** (beta-CDF) per source before aggregation; z-score breaks on Likert ceiling/floor
   - **Note**: a 5th proposed source — hand-crafted `source_category`-based intensity prior — was REMOVED per G7. Domain-expert audit (DnB slice) confirmed everynoise tags are unreliable on this corpus. `source_category` is never read as a label, feature, weight, or stratification key.

4. **Train teacher** (privileged information)
   - Inputs: `[MuQ-MuLan(512), caption_emb(768), structured_tags(~50), r7.5_tags(13)]` = ~1340d
   - **Drop `genre_OH` as a teacher feature** — would cause label leakage (genre is in the consensus label too)
   - Outputs: 16 axis heads + 1 intensity head (+ multi-task aux)
   - Multi-task loss against per-axis r7.5 BT priors + consensus intensity

5. **Distill student** (deployment shape)
   - Inputs: MuQ-MuLan(512) only — matches V15/V17b shape, CPU-cheap
   - Losses: output-MSE + **penultimate-layer feature MSE (FitNets)** + **soft-target T-scaled (Hinton)** + label smoothing
   - The student must transfer caption/tag knowledge through MuQ-MuLan space
   - This is the LUPI pattern (Vapnik 2009); empirically retains 70-90% of teacher when teacher's privileged features are correlated with student inputs (captions of music are largely derivable from audio, so gap should be small)

6. **Artist-stratified split**
   - By **artist only** — test-set artists never appear in train or val (MARBLE / GTZAN-vs-FMA artist-leakage pitfall)
   - Genre stratification intentionally NOT enforced — `source_category` is untrusted, and forcing balance via untrusted tags would re-inject the noise we just removed
   - 80% train / 10% val / 10% test (track-count balanced)
   - Test set never touched until final eval

7. **Held-out eval**
   - PA on the held-out test set (primary metric)
   - **Per-cluster** intensity histogram sanity (caption-emb K-means clusters, NOT source_category): top-tier clusters dominated by industrial / hardcore / metalstep > mid-tier clusters > bottom-tier dominated by ambient / acoustic
   - V18-fresh vs V15 vs V17b on held-out (all three projected from MuQ-MuLan)

## Compute budget (~10 hr, mostly caption sweep)

| Stage | Time |
|---|---:|
| Caption sweep (15314 × 256 tok @ 0.5 c/s) | ~8 hr |
| Caption embedding (bge-base CPU) | 5 min |
| Caption text-LLM intensity rating | 30 min |
| Caption structured-tag extraction | 5 min |
| Rank-normalize + Dawid-Skene aggregation (4 sources) | 10 min |
| Artist-stratified split | 5 min |
| Teacher training | 30 min |
| Student distillation | 30 min |
| Held-out eval + sanity | 10 min |

## Status

- [x] Caption sweep code wired up (`spike/track-grading/run_judge_caption.py`)
- [x] Caption embedder wired up (`spike/track-grading/embed_captions.py`)
- [x] Phase-1 smoke transfer test wired up (`train_probe_caption_smoke.py`)
- [x] Pipeline orchestrator (`run_round7_6_pipeline.sh caption-smoke|caption-full`)
- [x] Music Flamingo serve fixed (vLLM PR #39011 patched in)
- [ ] Phase-1 caption smoke (200 tracks, ~10 min wall)
- [ ] Caption text-LLM intensity rating module
- [ ] Snorkel / Dawid-Skene label aggregation script
- [ ] Stratified (genre × artist) split logic in `train_axes_r7_5.py`
- [ ] Teacher-student distillation training script (FitNets + Hinton soft-target)
- [ ] Held-out eval + per-genre histogram script
- [ ] V18 export
- [ ] Comparison harness V15 vs V17b vs V18 on held-out

## Optional follow-ups (not blocking V18 release)

- [ ] **DEAM arousal external validation** — DEAM dataset has 1802 song excerpts
  with human-labeled valence/arousal. Compute V18 scores on DEAM, correlate
  against arousal. Independent calibration check. ~1-2 hr.
- [ ] **Human-pair anchor** — collect 200-500 pairwise human judgments on
  held-out test tracks. Calibrate V18 final scalar via isotonic regression
  against the human anchor. ~1 day.
- [ ] **Ablation studies**: V18 with vs without caption_emb in teacher; with vs
  without each label source. Quantifies what's pulling weight.

## Round-7.7 (optional, future): per-user fitting

Separate optional pipeline for users with NVIDIA GPUs who want their library
calibrated to their personal sense of intensity. Takes V18 + a few user-rated
anchor tracks → fine-tunes the linear vector with a small LR + L2-anchor on V18.
Single-script tool, runs as a one-time import operation. Ships after V18 has
proven itself general-purpose.

## Deprecated paths (kept for reproducibility, do not run)

- **K=4 N-way ranking with Music Flamingo**: architecturally infeasible; MF
  caps at audio=1 per prompt. Pipeline cmd `smoke|full|post` exits with
  deprecation warning.
- **Pointwise 0–100 (raw-int) rating with MF**: collapses on subjective axes
  (3 of 16 axes returned exactly 50 for every track in the 200-track smoke).
  MF was never trained on scalar rating; "50" is the modal-token attractor
  under greedy decoding. Cmd `pointwise-smoke --mode raw-int`, deprecated.
- **Pointwise 5-bucket Likert with logprob recovery**: works (every axis
  becomes continuous, see Obsidian "Music Flamingo Pointwise Findings"), but
  caps at narrow stds on subjective axes. Replaced by caption-as-feature.
  Cmd `pointwise-smoke|pointwise-full --mode likert`, deprecated but
  reproducible.
- **User-library calibration head**: was an early V18 design. Dropped per
  goal (general-purpose, not user-fit). Moved to optional Round-7.7.

---

# Music Flamingo as a music-understanding asset (NEW — 2026-05-07)

NVIDIA Music Flamingo (`nvidia/music-flamingo-2601-hf`, 7B audio-LM, deployed
local at `~/.cache/mesh-spike/vllm-env`, port 8001) produces extraordinarily
rich, accurate captions on the Mesh corpus. Discovered while building the
round-7.6 caption-as-feature path for the intensity axis; the captions
themselves are valuable far beyond that one experiment.

**On 30-second clips MF correctly identifies:** specific (sub-)genre,
instrumentation, mix/production style, vocal characteristics, structural
events. See `documents/aggression-axis-caption-examples.md` (or generate fresh
via `bash spike/track-grading/run_round7_6_pipeline.sh caption-smoke`).

### Example captions (from 10-track smoke)

> **Vladimir Dubyshkin — Belissimo** (Industrial Techno)
> "This track is a high-energy Industrial Techno piece that fuses the relentless
> drive of classic techno four-on-the-floor rhythms with the abrasive, machine-like
> textures of industrial music. The production is built around a pounding kick
> drum that thumps on every quarter-note, paired with a heavily distorted synth
> bass that anchors the groove. Sharp, clipped electronic percussion — crisp
> hi-hats, metallic clicks, and glitch-y percussive hits — adds rhythmic intricacy,
> while a repetitive, heavily processed synth motif cuts through the mix with a
> gritty, metallic timbre. The overall mix is raw and gritty, with aggressive
> compression and a tight low-end..."

> **Mall Grab — Can't Get You Outta My Mind** (Deep House)
> "This track is a polished Deep House piece that leans toward a melodic, soulful
> sub-genre, blending warm, atmospheric synth pads with a classic four-on-the-floor
> groove. The production is high-fidelity and clean, featuring a wide stereo field
> that lets the deep synth bass and crisp electronic drums sit firmly in the centre
> while airy pads and subtle percussive textures expand outward. Vocals are
> performed by a female mezzo-soprano with a smooth, slightly breathy timbre. The
> delivery is melodic and soulful, treated with moderate reverb and delay..."

> **Jack Rose — Sunflower River Blues** (Acoustic Fingerstyle Guitar)
> "This track is an instrumental solo-guitar piece that blends Acoustic Fingerstyle
> with Contemporary Classical Guitar aesthetics, creating a contemplative,
> minimalist acoustic work. The sole voice is a nylon-string classical guitar,
> recorded with a raw, intimate mic technique that captures the natural resonance
> of the instrument. The mix is narrow and centred, placing the guitar
> front-and-centre with minimal reverb, allowing subtle finger-noise and dynamic
> nuance to be heard clearly..."

> **Loscil — First Narrows** (Dark Ambient)
> "This track is a Dark Ambient / Drone composition that blends deep, evolving
> synth textures with a subtle, industrial-tinged sound design. The soundscape is
> built from massive, sustained synth drones that dominate the low-mid range,
> layered with shimmering, metallic timbres and occasional high-frequency glints.
> Deep sub-bass frequencies underpin the whole piece, while sparse, processed
> percussive clicks and metallic resonances add texture without establishing a
> groove. The mix is highly polished, employing extensive reverb and delay to
> create a cavernous, three-dimensional space..."

(Caveat: MF *hallucinates* specific BPM, duration, and harmonic progressions —
those numeric claims are training-data priors, not measurements. The
descriptive vocabulary is the signal. Don't use the numeric claims as features.)

### Use cases for Mesh (beyond round-7.6 intensity)

- [ ] **Genre / sub-genre detection from captions** — regex/keyword extraction
  covers the full everynoise hierarchy. Cleaner than the current `genre_seed`
  field; can fill gaps where everynoise had no label. Validate against existing
  tags on overlap, see if MF can fill the gaps elsewhere.
- [ ] **Track-level tagging** — multi-label extraction of instrumentation, mood,
  vocal type, mix descriptors. Surface in mesh-cue browser as filter chips.
- [ ] **Suggestion-graph augmentation** — caption embedding cosine similarity
  enters the suggestion graph as a new edge weight, complementary to MuQ-MuLan
  similarity + key/BPM scoring. A/B vs current scoring on a manual test set.
- [ ] **Semantic search / browse** — "find tracks that sound like dusty boom-bap
  with female vocal", embedded via bge-base against caption_emb, retrieve by
  cosine. Better than the current keyword search over genre/title.
- [ ] **Auxiliary multi-task labels for the deployed axis** — train heads for
  genre, mood, vocal-type. Even if not displayed, they regularize the shared
  representation and help the deployed intensity probe.
- [ ] **Sub-genre clustering** — cluster caption embeddings → discover natural
  sub-genres in the user's library, beyond everynoise tags.
- [ ] **DJ-aware playlist generation** — caption text is rich enough to drive an
  LLM-based "build me a 90-min set: dark opener, peak at industrial techno around
  60min, cool to ambient" workflow. Captions as the LLM's per-track context.
- [ ] **Cache caption + caption_emb in `mesh-collection` DB** — `track.caption_text`
  + `track.caption_emb` blob alongside ml_embeddings. Reusable across all features.

### License caveat

NVIDIA Music Flamingo is **CC-BY-NC-4.0 / NVIDIA OneWay Noncommercial Academic** —
research and labels-only. For commercial Mesh deployment, distill caption-derived
features into a permissive student model, or treat the captions as labels-time
artifacts only (not shipped to users).

### File map (where the captions live)

- Captions: `/home/data01/Music/mesh-track-grading/round7_6_captions/music_flamingo/<track_id>.json`
- 768d caption embeddings: `/home/data01/Music/mesh-track-grading/round7_6_caption_emb*.npz`
- Generation script: `spike/track-grading/run_judge_caption.py`
- Embedding script: `spike/track-grading/embed_captions.py`
- Methodology note: `documents/` (this is a research/spike output; rolling
  details live in the Obsidian "Mesh — Caption-as-Feature Methodology" note).

---

# V18.2 — Lever 2: bigger audio encoder (MAEST/MULE) for the intensity axis

V18.1 (deployed 2026-05-08) shipped a 2-layer MLP student over MuQ-MuLan-512d
audio embeddings. Test PA = 0.8113 → 0.8174 (+0.6 pp over V18 linear baseline).

The capacity lever is now exhausted: hidden=128/256/512 sweep showed the gain
saturates at h=128. The remaining +12.26 pp distillation gap (teacher 0.940 →
student 0.817) is **information-bottlenecked** — MuQ-MuLan was trained for
contrastive music-text similarity, not intensity discrimination, so no amount
of student capacity reconstructs caption-derived intensity signal from the
512d audio embedding.

**Next lever: swap the audio encoder.** Phase 2 of the embedding-models work
already underway (`documents/embedding-models-research.md`).

## Plan

1. **Encoder bake-off** — re-encode the 39913-track corpus with each of:
   - MAEST-768d (Music Audio-Embedding Spectral Transformer) — music-track
     classification objective, ~300M params, GPU.
   - MULE-1.7k+d (Music Universal Linear Embedding) — deeper temporal
     context, ~1B params, GPU.
   - MuQ-MuLan-large-512d (current baseline, for delta).
   Atomic resume + L2-norm sanity check per `embed_corpus_mulan.py` shape.

2. **V18.2 train** — same teacher-student topology, captions/jurors/consensus
   reused from `data/round7_6/`:
   - linear probe for direct comparison vs V18 (G1 spec compliance)
   - 2-layer MLP for direct comparison vs V18.1
   On each new encoder, both arches.

3. **Compare** — held-out test PA on the same 3985-track split. Expected gain
   per the spec / lit: +2-4 pp on the new encoders (cumulative with the MLP
   ~0.84-0.88 PA). Closes most of the G6 distillation gap.

4. **Production deploy** — winning encoder becomes the active embedding.
   Requires:
   - Update `crates/mesh-cue/src/ml_analysis/inference.rs` to load the new
     ONNX/safetensors checkpoint instead of MuQ-MuLan.
   - Update `crates/mesh-core/src/intensity_axis.rs::EMBEDDING_DIM` to match.
   - Update `mesh-cue` batch_import to use the new encoder for newly-imported
     tracks.
   - Re-encode existing user libraries on first launch after the upgrade
     (background batch import job, ~1 hr per 10k tracks).

## Cost / risk

- Encoder bake-off: ~3-6 hr GPU per encoder (39913 × 30s clips at typical
  batch throughput).
- V18.2 train: deterministic, ~10s wall, all data already snapshotted in
  `data/round7_6/`.
- Production rollout: needs background re-embedding of user libraries.
  Schema-compatible if `EMBEDDING_DIM` stays 512. Bigger dim (MAEST-768,
  MULE-1.7k) needs a DB migration on the `ml_embeddings` table.

## File pointers

- Spec: `documents/round-7-6-pipeline-spec.md` §765-768 (escalation rule)
- V18.1 training log: `documents/round-7-6-v18-1-mlp-experiment.md`
- Embedding models research: `documents/embedding-models-research.md`
- Current intensity_axis Rust loader: `crates/mesh-core/src/intensity_axis.rs`
  (already supports both V15-class and V18+ schemas; MLP weights in JSON →
  no Rust change needed for V18.2 if it stays linear-or-MLP).

---

# Corpus strengthening — full-length tracks instead of 30 s previews

## The problem

Round-7.6 V18.1 training used **30-second Deezer preview MP3s** as the
audio source. Mesh-cue's deployed inference path uses **full-length
audio files** (3-7 min typical), embedded as **6 evenly-spaced 10 s
clips, mean-averaged**. Different distributions:

- Training: peak-energy 30 s window per track (Deezer's preview heuristic
  picks roughly "the catchiest 30 s" — usually the drop)
- Deployment: a mean over the entire track structure (intros + drops +
  breakdowns + outros)

Hand-verified on the user's 909-track DnB library
(`documents/round-7-6-training-log.md` § "Post-deploy hand verification"):
canonical neurofunk drops ("Modern Propaganda", "Running Blind") land at
p51-p53 on V18.1 instead of p80+ where they should be, because the mean
dilutes their drop intensity. Aggressive-mean / liquid-mean separation is
+50 pp (strong) vs the spec's +60 pp "excellent" target.

## Two fixes, ordered by cost

### Tier 1 (cheap, ships immediately): peak-clip pooling at inference

Instead of mean-averaging the 6 clip embeddings, project each clip
through V18.1 and take the **maximum-intensity clip's score** as the
track's intensity. Operationally aligns "track intensity" with "peak-clip
intensity", which is what the teacher was trained to recognise.

- Implementation: ~10 lines in `crates/mesh-cue/src/ml_analysis/inference.rs`
  — replace `average_embeddings(...)` with a "score-then-max" loop using
  the loaded `IntensityProvider`.
- Cost: 6× the projection per track. V18.1 MLP at h=128 is ~1.4 µs / track,
  so 6× → ~8.4 µs / track. Still 12,000× under the G9 budget (100 ms /
  1000 tracks).
- Risk: low. The 6-clip embedding extraction stays unchanged; only the
  aggregation step changes. DB schema unaffected (still stores 512d audio
  vec; we just project differently).
- Expected effect: aggressive-mean percentile 58 → 75, separation +50 pp →
  +60 pp.

**Decided 2026-05-08: ship Tier 1 immediately.** Tracked separately in the
"V18.1 inference: peak-clip pooling" TODO above.

### Tier 2 (heavy, real fix): full-length corpus for training

The proper way to eliminate the train/inference distribution mismatch is
to make training match deployment: re-build the round-7 corpus with
full-length tracks instead of 30 s previews, embed them with the same
6-clip mean-pool the deployed app uses, and retrain V18.x on those.

#### Cost evaluation

Storage:
- 39913 tracks × ~5 min × 320 kbps MP3 ≈ ~12 GB at typical bit-rate.
- 39913 × 512d × 4 bytes audio_emb = 80 MB (already what we have).
- 39913 × ~10 caption-embedding-style features ≈ trivial.
- Total: **~12 GB audio + ~80 MB embeddings**, manageable.

Acquisition:
- Deezer previews are the only thing the API gives anonymously. Full
  tracks need either (a) a Deezer Premium subscription's authenticated
  download API (terms of service question) or (b) a different corpus
  source entirely.
- Alternatives:
  - **MTG-Jamendo** — open-license full-length tracks, ~55k clips, BUT
    skewed toward instrumental + library-music genres (bias risk on the
    aggression axis).
  - **FMA-large** — 100k+ tracks under CC license. Wider genre coverage.
  - **MARBLE benchmark** — has full-length splits, includes metal/punk
    audio that's well-represented for the high-intensity tail.
  - User's own library — privacy-preserving, perfect distribution match
    for the deployed model, but not redistributable for training-data
    sharing.

GPU re-spend:
- MF caption sweep on 40k full tracks @ 6 clips = 6× audio compute per
  track. Currently 6 hr at 30 s previews → ~36 hr at full tracks.
  Manageable (single overnight run).
- 3 caption-text-LLM jurors stay the same (caption length doesn't change
  much for full-track captions if MF prompt is unchanged).
- Audio embedding (MuQ-MuLan) over 6× more audio → ~26 min × 6 = ~2.6 hr.
  Manageable.

Total Tier 2 cost: ~50-60 hr GPU spread over a few days, plus the corpus
acquisition decision.

#### Plan

1. **Decide corpus source.** Likely candidates: extend the existing Deezer
   manifest with full-track downloads (legal review needed), or migrate
   to MTG-Jamendo / FMA-large with a re-balanced seed list to keep
   genre coverage. **Decision needed before any GPU work.**

2. **Re-encode + re-caption.** Same scripts (`embed_corpus_mulan.py`,
   `run_judge_caption.py`) work on full-track audio with no code change —
   they already pad/truncate appropriately.

3. **Re-rate jurors.** Caption text → 3 jurors → consensus. ~5-8 hr
   wall (mostly the local Mistral pass).

4. **Retrain V18.x.** Same teacher/student topology, same
   `data/round7_6/` snapshot of captions+jurors+features. ~10 s wall.

5. **Compare V18.1 (preview-trained) vs V18.x (full-track-trained) on
   the user's 909-track library.** Hand-verify whether the
   aggressive-mean percentile gap closes from +50 pp toward the +60 pp
   "excellent" target.

#### When to do this

After Tier 1 ships and we measure how much of the gap it closes alone.
If Tier 1 gets us to +60 pp on the user's library, Tier 2 is unnecessary.
If Tier 1 only closes part of the gap, Tier 2 becomes the next lever
(probably alongside the V18.2 / Lever 2 encoder swap, since both require
re-encoding the audio anyway).

## Out of scope here

- Per-user calibration (Round-7.7 sketched in the spec) — orthogonal,
  uses the user's own pairwise feedback to fine-tune the deployed axis.
- DEAM external arousal anchor — also orthogonal, validates the axis
  against an academic ground-truth dataset.

---

# Tune the audio-clip timeframe shared by intensity + similarity

Both intensity scoring and PCA-based similarity inherit from the **same
upstream 512d** in `ml_embeddings`. That vec is currently the peak-clip
output of `MUQ_MULAN_MAX_CLIPS=6 × MUQ_MULAN_CLIP_FRAMES=1000` (6 ×
10 s). Picked 2026-05-08 to align with V18.1's training distribution
(Deezer 30 s previews → catchiest 30 s).

Open question: is **10 s the right window** for both downstream uses,
or should we tune it differently for intensity vs similarity?

## Why the question matters

- **For intensity**: a 10 s window catches a single drop pattern
  cleanly, but might miss the build-up / drop-relationship that defines
  some genres (riddim breakdown → drop). A 30 s contiguous window
  captures a full musical phrase. Trade-off: longer window → more
  context but more "average" pulled in. The training distribution was
  effectively 30 s contiguous, so matching that should help.

- **For similarity**: 10 s windows might cluster on micro-timbre (drum
  pattern matches drum pattern); 30 s windows might cluster on macro
  structure (build-drop matches build-drop). For DJ "find me a similar
  track to mix into" use, micro-timbre is probably what you want —
  10 s is closer. But for "find me a track with the same vibe across
  the whole song", longer is better.

## Plan

1. **Bench peak-clip-by-intensity at three window lengths** on the
   user's library (no retraining needed):
   - Current: 6 × 10 s, peak by V18.1 score → already in production
   - 30 s: 2 × 30 s contiguous (matches training distribution exactly)
   - 5 s: 12 × 5 s (more candidate windows, finer-grained peak)

   Compare `aggression_inspect` numbers (aggressive_mean −
   liquid_mean separation) across the three. Tier 1 already gives us
   ~+50pp → +60pp expected; longer windows might push further.

2. **Bench similarity** with the same three settings by spot-checking
   "given this neuro track, what are the 10 nearest neighbours?" for
   ~20 hand-picked seed tracks across DnB sub-genres. Eyeball whether
   10 s vs 30 s clusters feel more correct.

3. **Pick the operating point.** Likely candidates:
   - **30 s for both** if the training-distribution-match matters more
     than fine-grained similarity. Simplest.
   - **30 s peak for intensity, 10 s peak for similarity**: requires
     storing two embeddings per track (or computing similarity at
     query time from the stored 6 clips, which means the DB schema
     gains a `clips` column or we accept query-time decode cost).
     Adds complexity for a win we may not need.
   - **10 s for both** if the peak-of-10s window is already enough
     and the training mismatch is dominated by other factors.

## Where the knob lives

- `crates/mesh-cue/src/ml_analysis/inference.rs:70-72`:
  ```rust
  const MUQ_MULAN_CLIP_FRAMES: usize = 1000;  // 10 s @ 24 kHz / hop 240
  const MUQ_MULAN_MAX_CLIPS: usize = 6;
  ```
- The mel hop_length (240) is also relevant — 30s = 3000 frames at the
  same hop. MuQ-MuLan's training window was `clip_secs=10` per the
  reference, so going beyond 10 s per clip is technically off-distribution
  for the encoder itself (worth checking how it handles 30 s — may still
  work since it's a transformer, but no guarantees).

## Cost

- Per-clip run cost is roughly proportional to clip length (transformer
  attention is O(n²) on length but n is small here). 30s clips ≈ 3-9×
  per-clip cost. Total compute for the 6→2 clip switch is similar.
- Re-analysis: ~3 min wall on 909 tracks at the current settings; bigger
  clips would push to ~5-10 min. Tractable.

## Decide after

After we measure how V18.1 + 6×10s peak-clip lands on the user's library
post-reanalysis. If the +60pp target is hit, this whole TODO is academic
(don't fix what works). If it's not hit, the clip-window tuning is the
next cheap lever before retraining the model.

---

# DB-locking errors during batch ML reanalysis

Observed 2026-05-08 during the V18.1+peak-clip reanalysis pass on the
user's 909-track library:

```
ERROR mesh_cue::reanalysis] Failed to store ML analysis: Query("database is locked (code 5)")
ERROR mesh_cue::reanalysis] Failed to store ML embedding for ".../52_Billain - Cosmic Gate.flac": Query("database is locked (code 5)")
```

**Tracks that hit this DON'T get their new embedding written.** Stale
mean-pooled vec stays in the DB. After re-analysis "completes", any
track that hit the lock contention is silently using the old aggregation,
and the inspector / similarity index will misrepresent them.

**Root cause hypothesis** (from `crates/mesh-core/src/db/mod.rs:46-48`):
> CozoDB's SQLite backend doesn't support concurrent writers — concurrent
> `run_script` calls from different threads cause "database is locked"
> errors. The write lock is transparent to callers.

So MeshDb has a process-internal `Mutex` (`write_lock`) to serialise
writes. The fact that lock errors *still* leak through means either:

1. Some write path bypasses the mutex (calls `run_script` on
   `db_instance` directly without going through the wrapper).
2. The mutex is held but a SQLite-level busy-timeout is firing because
   another process (or reader) is touching the same DB file.
3. The reanalysis batch runs concurrent rayon workers, each going
   through the lock, but a long-held lock from one worker times out
   the others' SQLite-level busy retries.

**Effects observed:**
- Track `52_Billain - Cosmic Gate.flac` failed both `store_ml_analysis`
  AND the subsequent `store_ml_embedding` — two distinct write attempts,
  both got busy-locked at ~21:43:00. Suggests the write contention is
  long-lived (not a momentary blip).
- Other tracks in the same batch wrote successfully — so the
  contention is sporadic, not total deadlock.

## Plan

1. **Quick win: retry-with-backoff on busy errors.** Wrap `store_ml_*`
   calls in a 3-attempt retry with 100ms / 500ms / 2000ms backoff.
   Cozo + SQLite busy errors are transient by definition; retries
   convert most into successes. ~10 lines in `crates/mesh-cue/src/reanalysis.rs`.

2. **Root cause: serialise rayon batch writes.** Inspect
   `run_batch_reanalysis` (crates/mesh-cue/src/reanalysis.rs:175). If
   it dispatches work via rayon and each worker calls
   `store_ml_embedding` independently, the writes contend on
   MeshDb's internal mutex. Fix: collect (track_id, embedding) tuples
   in workers, batch-insert from a single thread at the end.
   Or use `db.batch_insert_*` if such an API exists / can be added.

3. **Reanalysis logging hardening.** When a write fails, the failure
   should be VERY visible — currently it's an error log line that's
   easy to miss in the firehose. Add a final summary that lists every
   failed track_id so the user can re-run just those:
   ```
   [reanalyze] DONE: 905 succeeded, 4 failed (DB lock):
     - 52_Billain - Cosmic Gate.flac
     - ...
   [reanalyze] re-run with --only-ids <these> to retry just the failures
   ```

4. **Affected tracks for this run:** unknown without grepping the full
   log. The user may want to re-trigger reanalysis on the failures
   individually or just re-run the whole batch (idempotent with the
   peak-clip pooling — re-analysing a track simply overwrites its
   stored 512d).

## Out of scope

- Replacing SQLite with a concurrent-write backend (would invalidate
  the whole CozoDB choice). Not the right tradeoff for the scale.
