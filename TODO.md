If unsure ask questions, dont assume stuff. Try to make sure that logic lies
as much as possible in mesh-core and mesh-widget and only if necessary in the ui.

# Features

## Saved Loops
- [ ] Loop buttons are there and styled correctly, but dont do anything (they
  are greyed out). Wire them with deck looping capabilities: press to create a
  new loop at the current playhead (snap to grid), loop size from beatjump
  width. Toggle on/off. Represents one of 8 saved loops stored to file.

## Collection Browser
- [ ] Multi selection (for multi drag and drop) and multi deletion. Implement
  deletion of track from playlist/collection in the mesh-widget via del key.
  Same for playlists. Always require confirmation via a small popup. Also need
  a right-click context menu for rename/delete/re-analyse.
- [ ] Sort by multiple columns: click column headers to sort by BPM, key,
  artist, etc. Standard library UX.
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
- [ ] Light feedback: sometimes flickering. Need to distinguish RGB vs fixed
  light color and map different actions/states to different colors for RGB
  controllers.

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
- [ ] Visual feedback: show *why* a track was suggested (e.g. small tag like
  "↑ energy", "same key", "similar timbre") next to each suggestion row.
- [ ] Per-deck vs global suggestion scope toggle.

### Infrastructure
- [ ] Per-stem audio feature indexing (HNSW per stem type) for stem-specific
  similarity queries.
- [ ] BPM range constraint: allow the DJ to set a target BPM window
  (e.g. ±5 from current). Useful when gradually accelerating or decelerating.

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
- [ ] Master limiter/clipper — prevents speaker damage and distortion.
- [ ] Live peak meter per channel and master channel.
- [ ] Set recording master output.
- [ ] Real-time short-term LUFS normalisation per stem (no latency) using
  essentia, ebur128 or lufs crate. Goal: stems after FX processing should be
  comparable loudness to input stem loudness, since processing can be very
  loud or silent.
- [ ] Built-in native effects (beat-synced echo, flanger, phaser, gater, etc.).

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

## HID/MIDI Unification
- [ ] HID learn capture race from Settings UI: when learn mode is started from
  the Settings UI (not --midi-learn), the existing controller has profiles
  matched to HID devices. drain() in tick handler consumes HID events through
  the mapping engine before learn-mode capture code can see them. Fix: skip
  drain() when learn mode is active, or reconnect controller in learn-only
  mode when entering learn from Settings. (mesh-player/src/ui/handlers/tick.rs)
- [ ] Shared HID event channel has no device tag: all HID devices share one
  hid_event_rx channel. In drain(), each event is checked against every HID
  device's mapping engine. If two identical devices (e.g. two Kontrol F1s) are
  connected, an event from one could match the other's profile. Fix: tag
  ControlEvent with a device_id and match against the originating device only.
  (mesh-midi/src/lib.rs, mesh-midi/src/hid/thread.rs)
- [ ] FeedbackChangeTracker is value-only, not color-aware: tracks by
  (ControlAddress, u8 value) but not RGB color. If two states have the same
  value but different colors (e.g. on_value: 127 red vs alt_on_value: 127
  green), the color change is suppressed. Fix: track (value, Option<[u8;3]>)
  as cached state. Currently not triggered because layer feedback uses
  different values (127 vs 50). (mesh-midi/src/feedback.rs)

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

# Future / Embedded
- [ ] Fixed sample rate optimisation for embedded hardware (e.g. 48 or 96 kHz
  everywhere instead of being sample-rate agnostic).

# Package Building
- Debian builds for older target (Ubuntu 22.04, works on older PopOS as well)
- Windows works as well
- Need to bundle UVR5 and extraction into the process (UVR is compilable for
  Linux and Windows)

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

# Changelog

## v0.7.0
- feat: tag system — TrackTag type in mesh-widgets, Tags column with colored
  pill rendering, track_tags CozoDB relation with batch loading, tag CRUD on
  DatabaseService (get/add/remove/batch/filter), and tag-based query methods
  (find_tracks_by_tags_all, find_tracks_by_tags_any)
- feat: suggestion reason tags — auto-generated colored tags on suggestion rows
  showing transition type, key compatibility, BPM match, and energy direction
  with symbols (↑/↓/═) and traffic-light color coding (green/amber/red)
- feat: smart suggestions v2 — unified scoring formula
  (0.40*hnsw + 0.30*key + 0.15*lufs + 0.15*bpm) replaces v1 mode-based system
- feat: energy direction fader — continuous slider steers suggestions toward
  higher or lower energy with adaptive filter threshold
- feat: Krumhansl-Kessler key scoring model — 24×24 perceptual key distance
  matrix selectable in Settings alongside the default Camelot model
- feat: energy-aware Camelot transition types — 15 categories (SameKey,
  AdjacentUp/Down, EnergyBoost/Cool, MoodLift/Darken, DiagonalUp/Down,
  SemitoneUp/Down, FarStep, FarCross, Tritone) with directional modifiers
- feat: multi-seed merge — all loaded decks act as seeds, results merged
  keeping best distance per candidate
- feat: auto-refresh suggestions on track load and settings save
- feat: MIDI encoder scroll and energy fader mapping for suggestions

## v0.6.13
- fix: preserve master deck phase offset on beat jump and hot cue press
- feat: enable slip mode by default for all decks
