If unsure ask questions, dont assume stuff. Try to make sure that logic lies
as much as possible in mesh-core and mesh-widget and only if necessary in the ui.

# Features

## Saved Loops
- [ ] Loop buttons are there and styled correctly, but dont do anything (they
  are greyed out). Wire them with deck looping capabilities: press to create a
  new loop at the current playhead (snap to grid), loop size from beatjump
  width. Toggle on/off. Represents one of 8 saved loops stored to file.

## Collection Browser
- [ ] Tag editing UI: support adding, removing, and editing tags on tracks
  directly from the browser. Needs inline tag editor or context menu with
  autocomplete from existing tags (get_all_tags). Color picker optional.
- [ ] USB tag export: ensure the track_tags relation is exported alongside
  other metadata when exporting collections to USB sticks.

## MIDI
- [ ] Jog wheel beat nudging for backwards compatibility with older devices
  (like SB2). Must work with current snapping system: when a user nudges by N
  samples, that offset is preserved across beat jumps, hot cue presses and
  other seek operations so the DJ doesn't need to nudge again.

## Stem Linking
- [ ] On-the-fly (unprepared) stem linking: shift + stem button redirects to
  browser with highlighting. Encoder press confirms. Populates the deck's
  linked stem buffer with the selected track's stem (matching the grid). New
  stem is prepared but not active until shift + stem button toggles it.
  Happens internally in the deck, UI sends high-level commands only. Waveform
  should visually indicate linked stems.

## Slicer
- [ ] Single morph knob per deck that scrolls through preset banks (up to 8
  presets per bank).

## Smart Suggestions (v3 — Future Improvements)

V2 (current) uses a unified scoring formula with energy direction fader and
selectable key scoring model (Camelot / Krumhansl). The following are ideas
for v3 and beyond.

### History-Informed Suggestions (requires deep research)
- [ ] **Play history graph**: once mesh-player records session history to the
  USB stick (see DJ History below), feed co-play data into the suggestion
  algorithm via the graph DB. Tracks that have been played together frequently
  across sessions should score higher as candidates — they have proven
  real-world compatibility that audio features alone cannot capture.
- [ ] **Pattern mining from play history**: research existing algorithms and
  systems for finding patterns in DJ play histories. Areas to investigate:
  - Collaborative filtering (item-item similarity from co-occurrence matrices)
  - Sequential pattern mining (frequent subsequences in set tracklists)
  - Graph-based recommendations (PageRank / random walks on the track
    co-play graph, weighted by recency and frequency)
  - Transition probability models (Markov chains over track-to-track
    transitions, conditioned on energy/key context)
  - Session-aware recommendation systems from the RecSys literature
    (e.g. GRU4Rec, STAMP, SR-GNN — adapted for DJ set context)
  - Existing DJ-specific research: DJ mix graph datasets, automatic
    playlist continuation, set.fm / 1001tracklists data analysis
  This needs a dedicated research document before implementation.
- [ ] **Negative signals**: tracks that were loaded but immediately skipped
  (played < 30 seconds) could receive a soft penalty in future sessions,
  especially when paired with the same seed tracks.
- [ ] **DJ profile divergence**: when multiple DJs use the same collection
  (B2B, shared USB), per-DJ history should be kept separate so suggestions
  reflect each DJ's mixing style, not a blended average.

### Stem-Specific Search
- [ ] **"Find a fitting vocal"** mode: search for tracks whose *vocal* stem
  characteristics complement the currently playing mix. Uses per-stem audio
  features (if indexed) or falls back to full-track features with a stem-type
  weight. The DJ selects which stem type they're looking for (Vocals, Drums,
  Bass, Other).
- [ ] **Stem contrast mode**: find tracks that are *harmonically compatible*
  but *timbrally different* — useful when the DJ wants to introduce fresh
  texture without clashing. Invert the HNSW distance component so that
  higher timbral distance scores better, while still enforcing key/BPM fit.

### Genre / Tag Awareness
- [ ] **Genre affinity scoring**: if genre tags or cluster labels are stored
  in the DB, use them as an optional filter or soft weight. The DJ can say
  "stay in techno" or "drift toward house" to control genre blending.
- [ ] **Tag-based exclusion**: allow the DJ to exclude tags (e.g. "no vocals",
  "no breakbeat") to narrow results.

### UI
- in mesh-cue there is a huge gap between the waveforms hot cue buttons and file browser. the browser
  should take all the space up until hot cue buttons. also remove the save changes button, since we have auto save.

###

### Infrastructure
- [ ] Per-stem audio feature indexing (HNSW per stem type) for stem-specific
  similarity queries.

## DJ History & Playlists
- [ ] Keep DJ history per session, per DJ, persisted to DB while playing.
  Initially used to update graph-based relations for track exploration, later
  for full set reconstruction.
- [ ] Improved playlist features using graph DB and vector features:
  - [ ] Similarity search for tracks or stems (smart dynamic playlist matching
    vibe of currently running track or a single stem).
  - [ ] Dynamic smart playlists based on Camelot key with energy options
    (lower / keep / raise energy).
  - [ ] Database backup (without wav files, just DB).

## Audio Processing
- [ ] Live peak meter per channel and master channel.
- [ ] Set recording master output.
- [ ] Real-time short-term LUFS normalisation per stem (no latency) using
  essentia, ebur128 or lufs crate. Goal: stems after FX processing should be
  comparable loudness to input stem loudness, since processing can be very
  loud or silent.
- [ ] Built-in native effects (beat-synced echo, flanger, phaser, gater, etc.).

### Documentation
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
- [ ] Virtual deck toggle buttons need similar logic to action pad modes. On
  DDJ-SB2, deck toggle makes deck-specific buttons use their own channel
  (action buttons, mode switches).
- [ ] Importing tracks don't appear in collection (mesh-cue) immediately after
  analysis. Visible as finished in status bar and written as file, but not in
  the collection list in the file browser.
- [ ] On window resize the last canvas state is imprinted and does not go
  away. The actual canvas still works normally.
- [ ] When deleting a file in the file browser, select the next item (or
  previous if no next) instead of scrolling to the top.
- [ ] USB manager should invalidate DB connection, cache and notify UI to
  return to root in file browser when a USB stick is removed.
- [ ] Beat grid analysis quality is not good enough. Research essentia
  beatgrid/rhythm section for EDM-specific beat grid detection.

# Performance
- [ ] Optimise stem storage (currently ~200-300 MB per multi-track file).
- [ ] Reduce code in tick handlers (both player and cue) to lower per-frame
  overhead. Factor out infrequently-changing information into
  message/subscription instead of tick handlers. Canvas sometimes skips
  frames.
- [ ] Real-time thread priority: set SCHED_FIFO with priority ~70 (below
  JACK's 80) for the audio callback thread when not using JACK. On a typical
  Linux desktop a CPU spike from another process can preempt the audio thread.
- [ ] Watchdog / xrun detection: monitor audio callback timing. If a callback
  takes too long, warn and adapt. Show xrun counts in diagnostics (like
  Traktor).

# Open Questions
- [ ] B2B settings management: when multiple DJs play on the same device, each
  with their own USB stick and settings, how does the system decide which
  settings to use? First connected? Should there be a B2B mode where specific
  decks use specific DJ's settings?

# Performance Profiling Reference

Host Track Load (Deck 0) - 524ms Total

Track: 100_Nocturnal - Surveillance (Original Mix).wav
Size: 16.35M frames (523.3 MB of audio data)
┌───────────────────┬───────┬─────────────────────────────────┐
│       Phase       │ Time  │              Notes              │
├───────────────────┼───────┼─────────────────────────────────┤
│ File open         │ 22µs  │ Negligible                      │
├───────────────────┼───────┼─────────────────────────────────┤
│ Buffer allocation │ 271ms │ 4× stem buffers (65MB each)     │
├───────────────────┼───────┼─────────────────────────────────┤
│ Audio read        │ 159ms │ 1647 MB/s from USB              │
├───────────────────┼───────┼─────────────────────────────────┤
│ Peak computation  │ 92ms  │ Highres waveform (65536 points) │
├───────────────────┼───────┼─────────────────────────────────┤
│ Total             │ 524ms │                                 │
└───────────────────┴───────┴─────────────────────────────────┘

Linked Stem Load (Stem 1 for Deck 0) - 2.86s Total

Track: 101_Noisia - Block Control.wav
Source BPM: 172.0 → Target: 174.0 (ratio: 1.0116)
Size: 20.68M frames (661.6 MB)
┌─────────────────────┬────────┬────────────┬───────────────────────────┐
│        Phase        │  Time  │ % of Total │           Notes           │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ USB database lookup │ ~0.1ms │ 0%         │ Cache HIT - instant       │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ Audio file load     │ 450ms  │ 16%        │ USB I/O (2813 MB/s)       │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ Time stretching     │ 2169ms │ 76%        │ The dominant bottleneck   │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ Buffer alignment    │ 109ms  │ 4%         │ Crop/pad to host duration │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ Peak computation    │ 30ms   │ 1%         │ Highres waveform          │
├─────────────────────┼────────┼────────────┼───────────────────────────┤
│ Total               │ 2859ms │ 100%       │                           │
└─────────────────────┴────────┴────────────┴───────────────────────────┘

Remaining Bottlenecks:
1. Time stretching (76% of linked stem load) - Would need GPU acceleration or quality tradeoffs
2. USB I/O (~450ms per track) - Hardware limited, could potentially use async prefetching
3. Buffer allocation (~270-500ms) - Could pool/reuse buffers across loads
