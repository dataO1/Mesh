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
- **Brute-force PCA** replaces HNSW approximate search (all tracks scored)
- **Radial layout** with PCA-derived angles replaces ForceAtlas2 force layout
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
  Parse from `suggestion_tags_json` or compute from track keys.
- [ ] **Key transition distribution**: Histogram of Camelot distances (0–6).
  Shows DJ's harmonic mixing style.
- [ ] **BPM progression**: Plot BPM per track across session. Stddev = genre
  focus (tight < 5 BPM = single genre, wide > 15 = journey).
- [ ] **Genre entropy**: Shannon entropy over genre distribution. Low < 1.0 =
  focused, high > 2.5 = eclectic.
- [ ] **Energy arc smoothness**: Mean absolute 2nd derivative of arousal curve.
  Lower = smoother energy flow.
- [ ] **Selection depth**: Average rank of selected suggestion (needs rank data).
- [ ] **Suggestion vs browse ratio**: load_source distribution per session.

## Energy arc visualization

- [ ] **Multi-layer timeline**: Canvas widget showing intensity + BPM + key
  progression stacked. Color strip for key (Camelot colors). Transition
  quality pips at step boundaries (green/amber/red).
- [ ] **Chapter detection**: Segment energy curve into monotonic regions.
  Label: low+rising = warm-up, peak = climax, high+falling = release.
- [ ] **"Complete my set"**: Given partial trail, suggest tracks for remaining
  chapters using suggestion engine with appropriate energy_direction.

## Style fingerprinting (novel — no DJ software does this)

Computable from aggregated session history:

- [ ] **Genre loyalty length**: Average consecutive tracks in same genre cluster.
- [ ] **Key walk smoothness**: Entropy of Camelot distance distribution.
- [ ] **Energy direction bias**: Average slope of energy curve across sessions.
- [ ] **Exploration breadth**: Unique graph clusters visited per session.
- [ ] **Radar chart**: Render style vector as a spider plot (harmonic smoothness,
  energy arc, genre loyalty, exploration, BPM range, decision confidence).

## Cluster heatmap

- [ ] **Visitation frequency**: Join track_plays with t-SNE positions. Color-code
  graph nodes by play count (hot = frequently played, cold = never).
- [ ] **Coverage metric**: "You've explored 34% of your library's stylistic space."
- [ ] **Underexplored suggestions**: Boost tracks from clusters adjacent to
  frequently visited clusters but never visited themselves.

## Transition pattern learning (needs data accumulation)

- [ ] **Personalized re-ranker**: Logistic regression over transition features
  [key_dist, bpm_diff, energy_diff, genre_match] trained on positive/negative
  examples from suggestion context. Per-DJ model.
- [ ] **Preference surfacing**: "You prefer adjacent key walks (72%) over
  same-key (18%). You rarely jump > 4 BPM."

## Collaborative filtering (multi-user, future)

- [ ] **Transition co-occurrence matrix**: Count how many DJs select B after A.
  Weight by recency. Works with 10-20 active DJs.
- [ ] **Sequential pattern mining**: Find frequent A→B→C subsequences across DJs.
- [ ] **Opt-in sharing**: Local-first, share only aggregated transition counts.

---

# Smart Suggestions & Library Intelligence (v3)

## Transition Graph — History-Informed Suggestions

Real-world co-play data captures musical compatibility that audio features alone
miss (genre nuance, crowd energy, DJ taste). `played_with_json` already records
which tracks were playing simultaneously, but uses names — ambiguous for graph
traversal. These TODOs turn raw co-play data into a queryable graph.

- [x] **Store track IDs in co-play records**: `played_with_json` currently stores
  track names only. Change to `Vec<(i64, String)>` (id + name) in
  `TrackPlayRecord`. No backwards compat needed — old name-only records are
  ignored when building the graph.

- [x] **Materialize `played_after` graph relation**: CozoDB relation
  `{ from_id: Int, to_id: Int => count: Int, last_played_epoch: Int }`
  built from `track_plays.played_with` data. Add
  `build_played_after_graph()` on `DatabaseService` using CozoDB `+=` bulk
  insert. Rebuild on "Analyse Library" button (mesh-cue) or incrementally on
  session end. Time-decay at query time: `count * exp(-age_days / 30.0)`.

- [x] **Co-play score bonus in suggestion scoring**: after normal HNSW scoring,
  fetch `played_after` neighbors of the seed and apply a +0.10 bonus (capped,
  decays beyond count=10). Minimum 5 co-plays before any bonus to avoid
  noise from single-session accidents.

- [x] **Played-after row highlight in file browser**: tracks with proven
  played_after edges to the current seed get a subtle tint in the browser —
  distinct from the already-played dimming. Shows the DJ their own proven
  follow-ups at a glance without changing the suggestion panel.

---

## Library Community Detection — Smart Playlists

HNSW similarity finds neighbors but doesn't reveal macro structure. Louvain
clustering on the HNSW graph (already available via CozoDB `graph-algo`) can
partition the library into 10–40 sonic clusters automatically, enabling
playlist auto-generation, new-music discovery, and a visual library map.

- [ ] **"Analyse Library" action** (mesh-cue): button in library settings that
  runs Louvain community detection on the HNSW graph. Stores `community_id`
  and `community_size` per track in a new `track_community` relation.

- [ ] **Auto-generated "Sound Clusters" in browser sidebar**: mesh-cue browser
  sidebar section "Clusters" lists each community with track count. Selecting
  one filters the browser to that cluster — automatic genre/vibe grouping
  without any manual tagging.

- [ ] **New Music Discovery mode** in suggestions panel: an "Explore" toggle
  that suggests one representative track from each *neighboring* community
  instead of the seed's own cluster. Breaks the echo-chamber problem where
  HNSW always surfaces the same genre zone.

- [ ] **2D cluster scatter plot** in mesh-cue library view: X = spectral
  centroid (tonal brightness), Y = `composite_intensity` normalized over the
  library. Each dot is one track, colored by community_id. Lets DJs visually
  scan collection density and plan a set arc by identifying clusters to move
  through. Requires the Analyse Library step first.

---

## PCA Dimension Reduction — Library-Tuned Similarity

At 1280 dims, cosine distances cluster around 1/√1280 ≈ 0.028 — a
concentration-of-measure effect that makes all HNSW distances look the same and
degrades suggestion quality. PCA reduces to ~200 dims capturing *this library's*
variance, sharpening the distance field for the specific collection.

- [x] **"Build Similarity Index" action** (mesh-cue): runs incremental PCA on
  all stored EffNet 1280-dim embeddings, projects to ~200 dims, stores in a new
  `ml_pca_embeddings { track_id => vec: <F32; 200> }` relation + separate HNSW
  index. Suggestion system uses PCA HNSW when present, 1280-dim as fallback.
  Show a stale warning if library has grown >10% since last build. Re-run does
  not require re-import; all ingredients are already in `ml_embeddings`.

---

## Session Energy Arc — Intensity History Strip

DJs currently have no at-a-glance read of the energy arc they've built during a
set. This strip solves the "where am I in the set?" awareness problem.

- [ ] **Intensity strip at bottom of file browser**: horizontal bar strip showing
  `composite_intensity` of each track played in the current session, left→right
  chronologically. Bar height = intensity, color = same gradient as suggestion
  rows (green→red). Intensity is normalized per-library
  `(track_intensity - lib_mean) / lib_stddev` so the strip is meaningful
  regardless of genre mix.
  - Needs a one-time per-track `composite_intensity` computation for tracks
    loaded from the browser (not only suggestion candidates). Compute at load
    time and cache in a `track_intensity_cache` relation or in `audio_features`.
  - Gives the DJ visual confirmation that they've been building for 40 min and
    it's time to cool down — without leaving the browser.

---

## Camelot Wheel in Suggestion Panel

DJs think in Camelot notation. Reading key tags in a list is slow; a radial
view maps directly to the Camelot wheel muscle memory.

- [ ] **Radial key display in suggestion panel**: 24-segment wheel (12 positions
  × A/B mode). Seed track's key = highlighted segment. Compatible zones
  (SameKey, AdjacentUp/Down) = green; diagonal transitions = amber; far-step/
  cross = red. Candidate suggestion tracks rendered as small dots on their wheel
  position, sized by score. Drives from the `classify_transition()` result
  already computed per candidate — no new scoring needed, purely a view change.

---

## Dual-Deck Context-Aware Suggestions

Currently always seeds from one deck. DJs layering two tracks need a suggestion
that works *with both*, not just one — especially for long ambient/techno layers.

- [x] **Blend-aware seed selection**: when two decks are playing simultaneously,
  infer the suggestion target from the intent slider position (0.0–1.0):
  - **Slider at 0.5 (middle, `|bias| < 0.3`)**: blend mode — compute a "blend
    embedding" = weighted average of both decks' EffNet vectors → query HNSW
    for the blend point → suggests a third track that bridges both sonic spaces.
  - **Slider at edges (0 or 1, `|bias| > 0.6`)**: transition mode — identify
    the outgoing deck (lower volume or longer playing time) → use only its
    embedding as seed, same as current single-deck behaviour.
  - Middle range (0.5→0 or 0.5→1): linear interpolation between blend and
    outgoing-deck modes.
  Key scoring still uses the outgoing deck's key regardless of blend mode.

---

## Intro / Set-Opener Suggestions (No Seed)

When all decks are empty the current suggestion panel is useless. Opener quality
is a distinct signal — long vocal-free intros, high intensity delta (build) —
derivable from data already in the DB without new analysis.

- [x] **Opener quality scoring**: when no deck is playing, rank the library by
  opener suitability instead of default sort. Score components:
  - **Intro length in bars** (`intro_bars`): from drop marker beat position ÷
    beats-per-bar (beat grid). Long intros (≥32 bars) strongly preferred.
  - **Vocal-free intro** (`intro_vocal_free: bool`): `vocal_density` of the
    intro segment < 0.15. Clean opener without an immediate vocal hook.
  - **Intensity delta** (`intensity_delta`): `composite_intensity` at drop
    segment minus at intro segment. High delta = dramatic build.
  - **BPM range**: configurable sweet-spot range (default 120–135), soft penalty
    outside it. Avoids suggesting 180 BPM tracks as openers in a progressive set.
  - Store `intro_bars`, `intro_vocal_free`, `intensity_delta` in a new
    `track_intro_profile` relation computed at import time (all ingredients
    already available: drop markers, beat grid, `vocal_density`, per-segment
    intensity). No re-import needed once stem_energy and ml_embeddings are
    populated (via "Re-analyse features").

---

# Features

## Collection Browser
- [ ] Tag editing UI: support adding, removing, and editing tags on tracks
  directly from the browser. Needs inline tag editor or context menu with
  autocomplete from existing tags (get_all_tags). Color picker optional.

## MIDI
- [ ] optional: Jog wheel beat nudging for backwards compatibility with older devices
  (like SB2). Must work with current snapping system: when a user nudges by N
  samples, that offset is stored for this deck only, resets on load of a new
  track and is preserved across beat jumps, hot cue presses and
  other seek operations so the DJ doesn't need to nudge again. We could
  potentially map several possible midi interfaces to this: classical jog wheel,
  +/- buttons for fine grained nudging.

## Slicer
- [ ] optional: Single morph knob per deck that scrolls through preset banks (up to 8
  presets per bank).

## B2B Mode
- big future update, after v1.0.0: i would like to support a b2b mode, where 2 people with mesh systems, can
  connect their 2 systems together via ethernet lan cable, the system recognizes
  this and automatically goes into b2b mode, where each of their system shows
  the info of the other deck (waveforms). this means that each dj has their own
  hardware and settings, but can also play music from their partners library and
  everything is synced to the same master clock and bpm. all playlist features
  like smart suggestions should also work.

## Smart Suggestions (v3 — Future Improvements)

V2 (current) uses a unified scoring formula with energy direction fader and
selectable key scoring model (Camelot / Krumhansl). The following are ideas
for v3 and beyond.

### History-Informed Suggestions (requires deep research)
- [x] **Session history foundation**: HistoryManager records all play data
  (loads, plays, hot cues, loops, co-play, suggestion metadata) to all active
  databases. Played tracks excluded from suggestions and dimmed in browser.
- [x] **Play history graph**: feed co-play data into the suggestion algorithm
  via the graph DB. Tracks that have been played together frequently across
  sessions should score higher as candidates — they have proven real-world
  compatibility that audio features alone cannot capture. Time-decayed
  co-play scoring (half-life ~30 days) to prioritize recent patterns.
- [ ] **Pattern mining from play history**: research existing algorithms for
  finding patterns in DJ play histories (see DJ History section for details).
  Areas: collaborative filtering, PrefixSpan, graph-based recommendations,
  transition probability models, session-aware RecSys (GRU4Rec, SR-GNN).
- [ ] **Negative signals**: tracks loaded but played < 30s could receive a
  soft penalty in future sessions, especially when paired with the same seeds.
- [ ] **DJ profile divergence**: see DJ History section.


### Infrastructure

## DJ History & Playlists
- [x] Keep DJ history per session, persisted to all active DBs while playing.
  HistoryManager tracks loads, plays, hot cues, loops, co-play (played_with),
  suggestion metadata. Played tracks dimmed in browser and excluded from
  suggestions.
- [ ] **Set reconstruction UI**: session browser that lists past sessions with
  timeline view showing which tracks played on which decks, when transitions
  happened (via played_with), and how long each track was played. Export as
  text tracklist or shareable format.
- [ ] **Track metadata mining from play history**: once enough sessions are
  recorded (50+), mine patterns to improve suggestions:
  - Co-play frequency scoring: tracks played together often across sessions
    get a bonus in suggestion scoring (time-decayed, half-life ~30 days)
  - Transition probability models: Markov chains over track-to-track
    transitions, conditioned on energy/key context
  - Skip penalty: tracks loaded but played < 30s get a soft penalty when
    paired with the same seed tracks in future sessions
  - PrefixSpan sequential pattern mining for finding common subsequences
    in set tracklists (requires 200+ sessions for meaningful patterns)
- [ ] **Per-DJ history divergence**: when multiple DJs use the same collection
  (B2B, shared USB), per-DJ history (keyed by USB stick identity) should be
  kept separate so suggestions reflect each DJ's mixing style.
- [ ] Improved playlist features using graph DB and vector features.
- [ ] Database backup (without wav files, just DB, so hot cues, markers,
  analysis data etc.).
- [ ] Session import from USB sticks to local collection via mesh-cue. so when
  playing on the player (for example on embedded, or another mesh setup) the
  history is stored on the sticks db. we need to automatically import this
  (rename "export" to "sync") as its own import step before the export step.

## Audio Processing
- [x] Live peak meter per channel and master channel.
- [ ] Built-in native effects (beat-synced echo, phaser, reverb, filter),
  which we can use via the multiband plugin preset system.

## Documentation
- we need much better strucutred and complete documentation:
  * a readme that is actually useful for people that come to github, see the
    project for the first time and dont want to get overwhelmed with
    information. we need a strong overview, which is still faithful with the
    goal of mesh. the key selling point of mesh is its very easy for beginners
    to use (due to beat sync, auto lufs gain control etc), but has a ton of features that do not exist on denon, pioneer, other
    software based mixing software like traktor etc (actually research what all
    these have, dont make false claims), also explain that due to inherent
    workflow of autosync and grid analysis electronic music and 4/4 beats are
    the target music, then a short list of the most important
    high-level features, which are important for the overall workflow of
    preparation (cue) and performance(player). check the previous list, but most importantly actually scan the codebase to see if anything is missing or not true. then a roadmap, which lists features that we still need to implement and dont have(again check the todo file, this also should just describe the high level important features) then a how to install, that references the build artifacts. Remove the contributing section, i dont want any contributions. But people are welcome to start issues if they have feature requests or bugs (we need templates for this, research good templates). Keep License and Acknoledgements and extend these if necessary.
  * linked md docs for various important documents:
    + the mesh-collection folder, where it is for each os, how to import, how to
      export to usb stick etc.
    + midi/hid mapping, already supported devices
    + effects (especially how to bundle puredata effets, including nn~ external,
      or generally externals, but nn is special since we need to build this),
      clap effects etc, where to place them
    + Embedded (we already have this), including a complete bom for recommended
      setup, how to wire, how to install sd image(we have the nix cache now,
      reference this).

# Bugs

# Stubbed / Deferred

# Performance

# Open Questions

# Auto Headphones Cue system
- [x] Instead of the user needing to automatically cue tracks to headphones out
  (theres a button per channel currently), we can automate this. usually the use
  case for this button is to pre listen to a newly loaded track, to beat match
  and check if this fits and we are at the correct beat grid offset etc, but
  since we have auto sync, a lot of this is useless, and for mesh its only
  important to prelisten to the newly cued track to verify its on the same
  beatgrid snap and that it musically fits. this means that all tracks, that are
  loaded but currently at volume 0 (or a threshold very low, like less than 30%)
  should be send to cue headphones out. this should not just binary, but
  gradually mixed in so at 50% they might be audible still a bit, at 30% fully (at 0 still fully )(exponential curve or just two stages linear, both is fine). users can still
  use the cue buttons (make this configurable in the player ui, default is
  auto-cue). Autocue should ONLY be active, when the master and cue outputs are
  different outputs, otherwise there is problems with the output! plan this thorougly, break it into subtasks, at the end document
  this as a new feature in the changelog and commit all.

# DB
- [ ] we need to introduce database versions, so when the schema changes the db
  knows it needs to migrate. this is also relevant for usb sticks, at some point
  we need to be able to be backwards compatible (not yet, but we should pave the
  way for this possibility, by adding a versioning system for the schema/db version)


# UPDATE LIFECYCLE
- [ ] connecting to wifi should first check if we already have credentials
  stored (via networkmanager) and just reconnect then, since we wont need a
  password entry then.

## Embedded: Silent Boot (investigated, partially working)
- [x] Removed Plymouth splash entirely — the script theme (`ModuleName=script`)
  failed on ARM/RK3588S. Plymouth rendered the NixOS fallback theme instead of
  the custom mesh theme. Root cause never fully identified, but likely:
  font rendering (`Image.Text()` with "Sans Bold 48" needs fonts in initrd),
  and/or `rd.systemd.show_status=auto` switching to verbose mode on slow SD I/O.
  Armbian/Orange Pi Debian doesn't use Plymouth at all — they use a U-Boot-level
  boot logo via the proprietary `resource.img` mechanism.
- [x] Silent boot params applied: `quiet`, `loglevel=0`,
  `rd.systemd.show_status=false`, `systemd.show_status=false`,
  `udev.log_level=3`, `rd.udev.log_level=3`, `vt.global_cursor_default=0`,
  `logo.nologo`, `kernel.printk=0 0 0 0`. Significantly reduces boot output
  but does not achieve fully silent boot on this hardware.
- Attempted `console=tty2` — redirects ALL console output to tty2, but this
  breaks cage-tty1 because the kernel's active VT switches to tty2, preventing
  cage from acquiring DRM master on tty1. Not usable with kiosk setups.
- Future option: raw framebuffer `dd` service (write pre-rendered .fb to
  /dev/fb0 early in boot) or recompile vendor U-Boot with `CONFIG_SPLASH_SCREEN`
  for a true pre-kernel splash.

# OTHER
- [ ] Touch support for certain actions? Is it the screen or iced?
- we alternatively need to support arrow key based workflow isntead of encoders, some hardware dont have encoders. the very first mapping (browse encoder) needs to detect then if the mapped control is an encoder or a button, then adapt the mapping scheme accordingly.
- [ ] when on the fly stem linking in the browser for selecting a linked track,
  we can utilise smart suggestions better by additionally adding specific search parameters for the stem that is about to be linked or weighting certain markers more. for example when linking drums, key is relatively irrelevant, but the energy or lufs, aggression and other metrics matters more. for vocals, key is absolutely the most important, bpm also a bit, not so much energy, for bass i think the weighting can stay as is, for other too. Its also possible that for linked stems(other than drums) very compatible key is actually a hard requirement,  and a filter for results.

Also i thnk we need to incorporate some user feedback to effectively
fine-tune/tune the suggestions system. We need to collect feedback for good and
bad suggestions (only the selected ones), then after a few sessions of collected
input evaluate, which suggestions worked well and which ones did not work well.
We need to know the starting track, the slider positions, the selected following
track and the feedback. what else might be usefull?
not for the specific tracks, but their characteristics, like hnsw index, key
relations compared to each other etc. so we can exfiltrate the relations that
make good transitions. for example maybe our complementarity bias system is
bullshit, we might find out by that. or maybe mostly key + hnsw similarity is
important. maybe the hnsw similarity scaling is off etc. evaluate.



● Here's my full evaluation of each feature (skipping touch):

  ---
  1. Browser Sort Settings

  Current state: Sort already exists in track_table/mod.rs — clickable column headers, sort_column +
  sort_ascending state, compare_tracks_by_column() supporting all columns (Title, Artist, BPM, Key,
  LUFS, Duration, Order, Cues). It works but it's hidden — column headers are the only surface.

  What's missing:
  - Suggestion panel: no sort control at all. Currently returns purely by score. "By playlist order"
  (preserve original playlist sequence) and "free" (by score) would be useful.
  - Playlist browser sidebar (folder/playlist list): no sort by name/date.
  - Persistent sort preference across sessions.

  Effort: Small-medium. Logic exists; it's mostly adding a compact sort picker (segmented button or
  dropdown) to the browser header area and wiring the suggestions panel sort separately. No
  algorithmic work.

  ---
  2. Fuzzy Search + Overlay Keyboard

  Two independent sub-tasks:

  a) Fuzzy matching on artist + title

  Current: title-only contains() check (line 780–788 in track_table/mod.rs). Easy wins:
  - Extend to artist: title.contains(q) || artist.contains(q)
  - Fuzzy: split query into tokens, require all tokens appear as subsequences. A simple scorer — sum
  character positions × character match ratio — works well for DJ use (typos on controller input are
  rare; abbreviations like "daft" finding "Daft Punk" is the real need). No external crate required.
  - "Cue search" from your list — I believe you mean searching tracks with cue points, or the search
  input within the cue editor context. Same fuzzy matching applies there.

  Effort: Small. One function in track_table/mod.rs.

  b) Overlay keyboard for embedded

  Currently the overlay keyboard (keyboard.rs) is only wired to WiFi password input (one
  keyboard.open() call in network.rs). The search text_input uses a standard iced widget that
  requires a physical keyboard.

  To connect: trigger keyboard.open() when user focuses/presses the search field → route keyboard
  text to search_query → close on submit. Requires a new KeyboardContext enum (search vs. wifi) so
  the submit handler knows where to route the result.

  Effort: Medium. New keyboard context enum + plumbing in app.rs. The keyboard widget itself is
  already fully capable.

  ---
  3. Stem Content Analysis for Mashup Scoring

  What we already have: vocal_presence: f32 in MlAnalysisData — already computed by the ML pipeline,
  per track. Also mood_acoustic, mood_electronic, timbre (bright/dark). The 16-dim HNSW vector
  captures rhythm + harmony + energy + spectral timbre, but vocal content is not in any of these 16
  dimensions. So this is genuinely additive signal.

  Is it overfitting? Does HNSW already capture this?

  No on both counts. HNSW captures spectral centroid/bandwidth (overall brightness/spread), which
  weakly correlates with vocal content but isn't a direct measure. vocal_presence is orthogonal. The
  mashup context also introduces something new: relative complement scoring based on what's currently
   playing — not static track-to-track, but |seed_vocal - candidate_vocal| weighted by live deck
  state. This isn't captured by any existing signal.

  Should we search for non-similar tracks always? No. You still need harmonic + BPM compatibility —
  you want tracks within the harmonically safe set that are content-complementary. Random non-similar
   tracks would include harmonically clashing ones.

  The AAAI 2021 paper on mashup compatibility (found in literature search) confirms: key and tempo
  are table stakes, but spectral/content complementarity predicts mixing quality beyond that. Their
  approach used self-supervised learning on stem pairs from the same vs. different songs — positive
  examples = same song stems (known to blend), negative = mismatched key/tempo. This is more work
  than we need here.

  My recommendation — two stages:

  Short term (use what exists):
  Add a vocal_complement_bonus term: |seed_vocal_presence - cand_vocal_presence| * 0.05. This rewards
   tracks with different vocal balance from the seed. Apply it as a small always-on bonus (not gated
  by bias). Weight 0.05 taken from w_bpm budget.

  Medium term (higher value):
  The 1280-dim EffNet embeddings are computed during import but thrown away — only the downstream
  scalar predictions are stored. Storing them and using cosine similarity between embeddings would be
   far more powerful than the 16-dim handcrafted HNSW vector for this purpose. EffNet's embedding
  implicitly encodes vocal/drum/bass balance, genre texture, production style — everything needed for
   mashup complement scoring. This would require a schema change and an indexed search structure
  (HNSW on the 1280-dim embeddings).

  ---
  4. "Not Too Similar" at Center Intent Position

  Current behavior at center: hnsw_component = norm_dist with w_hnsw = 0.42. Lower distance = higher
  score = spectral twins rank first. With 42% of the total score going to "find the most spectrally
  identical track," the result pool tends to be near-clones.

  The literature confirms this is the classic "filter bubble" problem in recommenders. The
  serendipity formulation (unexpectedness × relevance) says the ideal recommendation is
  simultaneously relevant (compatible key/BPM) and unexpected (not a spectral copy).

  Three options, in order of aggression:

  1. Reduce HNSW weight at center (low risk): Drop from 0.42 to ~0.22 at center (scaling toward 0.42
  at extremes where diversity/similarity direction is intentional). Redistribute the freed 0.20 to
  key + BPM. Result: harmonic/tempo compatibility carries the "similar enough" signal, HNSW stops
  dominating. Simple, reversible.
  2. Goldilocks curve at center (moderate): Replace norm_dist with a peaked curve centered at a
  target distance (~0.3). Tracks at 0.3 normalized distance score best; too close AND too far are
  penalized. Formula: hnsw_component = 1.0 - (norm_dist - 0.30).abs() / 0.70 clamped to [0,1]. This
  actively rewards "somewhat similar but not identical."
  3. Permanent mild diversity floor (aggressive): Always add 0.10 * (1 - norm_dist) as a floor
  regardless of bias, reducing pure-similarity dominance at all positions. Makes every suggestion
  have a built-in "don't be too close" pressure.

  My recommendation: Start with option 1 (weight reduction at center) — it's a one-line change to the
   weight scaling expression and it's easy to A/B. If center results still feel too same-y, layer in
  option 2 (Goldilocks curve). Option 3 is probably too blunt.

  ---
  Priority Order

  If I were to order these by impact/effort ratio:

  1. Fuzzy search (artist+title) — small effort, high daily usability gain
  2. "Not too similar" at center — tiny change, immediate perceptual improvement in suggestions
  3. vocal_presence mashup bonus — data already there, 5 lines of scoring code
  4. Overlay keyboard for search — medium effort, essential for embedded usability
  5. Browser sort header — polish, the column headers already work
  6. Store EffNet embeddings for deep mashup scoring — highest long-term value but a schema +
  pipeline change

✻ Baked for 5m 28s

❯ also which vector algorithm are we using? is this even fitted for audio at all? research this
online and check what we use (i think cozodbs built in vector system? cn we use something for audio
like wav2vec or vggish or other? search online, evaluate if we can improve this.

● 2 Explore agents finished (ctrl+o to expand)
   ├─ Find vector index and HNSW implementation in codebase · 31 tool uses · 60.9k tokens
   │  ⎿  Done
   └─ Research audio embeddings wav2vec VGGish MERT · 10 tool uses · 27.3k tokens
      ⎿  Done

● ★ Insight ─────────────────────────────────────
  The fundamental issue: our 16-dim vector is manually engineered by a human, while the EffNet
  1280-dim embedding is learned from millions of Discogs-labeled tracks. The learned embedding
  implicitly encodes everything we hand-picked (rhythm, timbre, energy) plus far finer distinctions
  we can't hand-code: groove texture, production style, instrument blend, genre micro-nuance.
  ─────────────────────────────────────────────────

  ---
  What We Actually Use

  CozoDB's built-in HNSW (::hnsw create audio_features:similarity_index) with:
  - 16 dimensions, F32, cosine distance
  - m=16 (connections per node), ef_construction=200
  - Queried with k=10,000 (full collection recall)

  The 16 dims: 4 rhythm (BPM, confidence, beat_strength, regularity) + 4 harmony (key X/Y circular,
  mode, harmonic_complexity) + 4 energy (LUFS, dynamic_range, energy_mean/variance) + 4 timbre
  (spectral_centroid, bandwidth, rolloff, MFCC_flatness).

  ---
  The Problem With the 16-Dim Vector

  It double-counts what the scoring pipeline already handles. BPM and key occupy 8 of 16 dimensions,
  but these are already scored separately and precisely by the BPM penalty and the harmonic
  filter/key transition score. So the HNSW component — which has a 0.42 fixed weight in scoring — is
  largely redundant with two other components already in the formula. The remaining 8 "timbre +
  energy" dims are too coarse: spectral_centroid + spectral_bandwidth + spectral_rolloff cannot
  distinguish a pumping tech-house lead from a dark techno kick if their global spectral statistics
  happen to be similar. The result: HNSW neighbors for a given seed are tracks in a similar BPM/key
  region, not tracks with a similar vibe.

  ---
  The Easy Win We're Ignoring

  The EffNet 1280-dim embedding is already computed during every import. It's the input to all the ML
   classification heads (vocal_presence, genre, danceability, etc.). We compute it, use it, then
  discard it. Only the downstream scalar predictions are stored.

  If we stored the 1280-dim embedding per track and indexed it with a second HNSW
  (ml_embeddings:effnet_index), we'd get:
  - A musically learned similarity space trained on Discogs's 400-genre taxonomy
  - Clean separation: HNSW = timbre/vibe/style, BPM/key = their own components (no double-counting)
  - No new model, no new inference — the data is being computed today, just thrown away

  Storage cost: 1280 × 4 bytes × 10,000 tracks = ~50 MB. Negligible.

  ---
  CLAP vs. EffNet for This Use Case

  The research puts CLAP (512-dim, LAION, text-audio trained) at the top for perceptual timbre
  alignment. But for a DJ context specifically:

  ┌────────────────────┬─────────────────────────────────────┬───────────────────────────────┐
  │                    │          EffNet (1280-dim)          │        CLAP (512-dim)         │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ Training data      │ Discogs 400-genre music taxonomy    │ Diverse text-audio pairs      │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ Genre nuance       │ Excellent (400 subgenres)           │ Good                          │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ ONNX               │ Already working in our pipeline     │ Messier, no pre-built ONNX    │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ Size               │ 17 MB (already cached)              │ ~330 MB (new download)        │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ Text search        │ No                                  │ Yes (bonus we don't need yet) │
  ├────────────────────┼─────────────────────────────────────┼───────────────────────────────┤
  │ Integration effort │ Store the output we already compute │ New model, new pipeline step  │
  └────────────────────┴─────────────────────────────────────┴───────────────────────────────┘

  For our use case — finding tracks that sound like they belong in the same DJ set — EffNet on
  Discogs data is probably better than CLAP because the Discogs taxonomy is extremely
  music-genre-specific. CLAP would be more valuable if we wanted to add text-query search ("find
  something dark and hypnotic") down the line.

  ---
  CozoDB HNSW: Is It Suitable?

  Yes. CozoDB's HNSW scales logarithmically in high dimensions and cosine distance is exactly right
  for normalized embeddings. No reason to switch to an external Rust crate (hnswlib-rs, USearch,
  hora). The one parameter to tune: m should increase with dimensionality — current m=16 is fine for
  16 dims but for 1280 dims, m=32–48 gives better recall at similar query cost. ef_construction=200
  can stay.

  ---
  Recommended Path

  Phase 1 — Store EffNet embeddings (high impact, low effort):
  1. Add a ml_embeddings relation to the schema: {track_id => effnet_vec <F32; 1280>}
  2. Create a second HNSW index with cosine distance, m=32, ef=300
  3. In MlAnalyzer, after running EffNet, persist the 1280-dim output alongside the existing scalar
  predictions
  4. In query_suggestions(), use the EffNet HNSW instead of (or blended with) the 16-dim one
  5. The 16-dim index can then be reduced to a fallback for tracks that haven't been ML-analyzed yet

  Phase 2 — Remove BPM/key from the 16-dim vector (or retire it):
  Once EffNet HNSW is live, the 16-dim vector's BPM/key dimensions add noise rather than signal (they
   duplicate the explicit BPM penalty and harmonic filter). Retire the 16-dim index or strip it down
  to 8 pure-timbre dims as a fallback.

  Phase 3 — CLAP (optional, long-term):
  Only worth adding if text-query search becomes a desired feature ("find me something melancholic
  and driving"). Not needed for the current DJ recommendation flow.

  ---
  Bottom line: We're computing exactly the right embedding already — we're just not keeping it. Phase
   1 is essentially "stop throwing away the EffNet output" and wire it into a second HNSW index. The
  scoring quality improvement would be substantial, since the current 16-dim HNSW is genuinely a weak
   signal.




- in graph view set preparation selecting a track could visually render a tracks
overview waveform underneight the previous loaded track waveform, so we can also
visually compare tracks (if they fit in the time domain as well)

- shfit select with a filter selects everything not visible via the filter in
  between
- we can finer differentiate the intensity tags generated from the suggestion
  system, based on which feature is more/less aggressive or most influencing the
  decision. suggest labels for
  each potential delta of seed vs compared tracks feature of the intensity compound score. i will give feedback.
