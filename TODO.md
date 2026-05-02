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
