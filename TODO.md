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
  samples, that offset is stored for this deck only, resets on load of a new
  track and is preserved across beat jumps, hot cue presses and
  other seek operations so the DJ doesn't need to nudge again.

## Slicer
- [ ] Single morph knob per deck that scrolls through preset banks (up to 8
  presets per bank).

## B2B Mode
- i would like to support a b2b mode, where 2 people with mesh systems, can
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


###

### Infrastructure

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

## Release Pipeline
- The sd image build has the derivation hash in the name, but this should have
  the tagged version, which triggered the build in name, like the other build
  artifact releases. Also in the build description for the releases for the
  debian and windows builds, cluster linux builds first, then windows build
  (nixos is linux).

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
- [ ] Virtual deck toggle buttons need similar logic to action pad modes. On
  DDJ-SB2, deck toggle makes deck-specific buttons use their own channel
  (action buttons, mode switches).
- [x] Importing tracks don't appear in collection (mesh-cue) immediately after
  analysis. Visible as finished in status bar and written as file, but not in
  the collection list in the file browser.
- [x] On window resize the last canvas state is imprinted and does not go
  away. The actual canvas still works normally.
- [ ] When deleting a file in the file browser, select the next item (or
  previous if no next) instead of scrolling to the top ( i think this happens,
  since we index something that isnt there anymore ).
- [ ] USB manager should invalidate DB connection, cache and notify UI to
  return to root in file browser when a USB stick is removed.

# Performance
- [ ] Optimise stem storage (currently ~200-300 MB per multi-track file).
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

# Other

- loading animation both in the ui and as led feedback for mapped controllers
  (for the loading deck, all action buttons/hotcue buttons or in momentary mode all buttons whose
  secondary mapping is action button/hotcuebutton is should blink fast while a
  track is loading)


- on tracks, where the stem separation is not perfect (the bass stem is actually
  in the drums layer), the lufs analysis is inaccurate. analyse if the lufs
  analysis is actually done on the whole mix regardless of the beat analysis
  config (if we analyse only the drum stem or whole mix). in general i feel like
  tracks with less lufs are still too silent after the automatic gain correction
  chain in mesh-player, make sure this is absolutely accurate! for example
  allied oxidize is  -6.1db scaled down for a target of -14 lufs and Culture
  Shock Breathe is -5db scaled down, which implies ~ db lufs difference, but
  these tracks are in actuallity vastly different loudness. oxidize has the bass
  stem empty and the bass is in the drums stem.

- i realized the bpm and grid analysis mostly only has problems with tracks that have 175
  bpm (only 2-3 problems with 174 compared to 8-10 with 175). maybe this has to
  do something wiht our processing or rounding?. check for potential
  causes, just evaluate first and report back.

- maybe we can utilise online metadata scraping as a fallback,comparison piont
  for bpm and key analysis, also we can get accurate metadata tags, like name
  artist, release date etc from good sources online. check which metadata these
  pages have in common and are consistent, and first do a deep dive on the ease
  of scraping these pages and consistently getting reliable metadata. also
  search on your own for other potential candidates, that might fill other
  genres.:
For finding, organizing, and analyzing music with accurate metadata (BPM, Key, and Genre) across EDM and Rock, the best platforms combine high-quality audio, comprehensive search filters, and, in some cases, specialized DJ-focused analysis.

Here are the best music pages for different genres based on accuracy and variety:
1. Best for Electronic Dance Music (EDM)

    Beatport: The industry standard for electronic music. It offers highly accurate BPM and key data, specialized sub-genre classification, and a wide variety of tracks.
    Beatsource: Sister site to Beatport, focused on open-format and commercial dance music, providing reliable metadata for faster-paced genres.
    ZipDJ: An excellent digital record pool, particularly strong in underground electronic styles and house music, with accurate, high-quality meta-tagged files.

2. Best for Rock (and Indie/Alternative)

    Bandcamp: An extensive catalog where artists upload their own music. It features a superior tagging system (including specific rock sub-genres) and high-quality metadata, making it excellent for finding new, specialized, or independent rock music.
    Discogs: The most comprehensive database of recorded music, covering all rock eras and sub-genres. It is indispensable for verifying release details and finding specific, detailed metadata.
    Apple Music: Offers a vast library with very high-standard, consistent metadata across Rock and related genres, useful for curating playlists.

    Also check spotify and deezer.
