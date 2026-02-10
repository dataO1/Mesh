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
- [ ] Real-time short-term LUFS normalisation per stem (no latency) using
  essentia, ebur128 or lufs crate. Goal: stems after FX processing should be
  comparable loudness to input stem loudness, since processing can be very
  loud or silent.

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

## v0.6.13
- fix: preserve master deck phase offset on beat jump and hot cue press
- feat: enable slip mode by default for all decks
