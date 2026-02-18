# Changelog

All notable changes to Mesh are documented in this file.

---

## [0.8.9]

### Fixed

- **Browser not updating during import/reanalysis** — Track metadata (BPM, key,
  tags, new tracks) now refreshes in real-time as each track completes instead
  of requiring manual navigation. The tick handler was silently discarding all
  `Task` returns from progress handlers; these are now collected and returned
  via `Task::batch()`. Reanalysis also fires per-track `RefreshCollection` on
  success, matching the pattern import already used.
- **Audio muted during USB export** — Removed unnecessary audio stream
  pause/resume around USB export. Only import and reanalysis (which are
  CPU-intensive) pause the stream; export is I/O-bound and doesn't need it.
- **Tags column too wide in mesh-cue** — Reduced track table Tags column from
  300px to 150px so the Name column has more room.
- **mesh-cue Windows build failing** — mesh-cue hardcoded `pd-effects` as a
  direct dependency, pulling in `libffi-sys` which fails to cross-compile for
  MinGW. Now feature-gated like mesh-player: `pd-effects` is a default feature
  (enabled on Linux) but disabled by `--no-default-features` on Windows. The
  PD stub module was also updated with missing methods/fields. Windows build
  script now fails on either crate instead of silently skipping mesh-cue.

---

## [0.8.8]

### Fixed

- **Audio crackling during batch operations** — CPAL audio stream is now paused
  during import, export, and reanalysis to eliminate buffer underruns caused by
  CPU contention between the real-time audio callback and heavy processing
  threads (ML inference, stem separation, file I/O). The stream starts paused
  at launch (no track loaded = no audio needed) and resumes only when a track
  is loaded for preview. Cancel and error paths also correctly resume audio.

---

## [0.8.7]

### Added

- **Cross-source suggestions** — Suggestions now query all connected databases
  (local + USB). HNSW vector search runs across both sources, combining results
  into a unified ranked list with source tags ("Local" / "USB") on each
  suggestion.
- **Cross-source deduplication** — When the same track exists in both local and
  USB databases, only the entry with the best HNSW distance is kept, preventing
  duplicate suggestions.
- **USB export: tags, ML analysis, audio features & presets** — USB export now
  syncs ML analysis data, track tags, and audio feature vectors alongside track
  files. Effect presets (stems, decks, slicer) are also copied to USB.
- **Metadata sync progress** — USB export reports per-track progress during the
  metadata-only sync phase, keeping the overlay progress bar responsive.
- **ML audio analysis** — 6 new EffNet classification heads: timbre
  (bright/dark), tonal/atonal, acoustic, electronic, danceability, and
  approachability. ML-based vocal detection replaces RMS-based approach.
- **Energy-direction suggestion scoring** — Suggestions incorporate ML arousal
  scores, genre-normalized aggression, and production match scoring. Key scoring
  blends toward energy direction at fader extremes.
- **Event-driven seed refresh** — Suggestion seeds now auto-refresh on deck
  load, play/pause, and volume changes with debounced timer.
- **Multi-factor reason tags** — Suggestion entries show sorted reason tags
  (key compatibility, energy direction) with color-coded confidence.
- **Hierarchical USB playlists** — USB export supports nested playlist folders
  with portable relative paths.

### Fixed

- **Audio features not exported to USB** — `get_audio_features()` failed
  silently on CozoDB's `DataValue::Vec(Vector::F32(...))` type, only matching
  `DataValue::List`. Audio feature vectors were never synced to USB.
- **Cross-DB HNSW search** — `find_similar_by_vector()` passed the query vector
  as `DataValue::List`, but HNSW requires a proper Vector type. Fixed with
  CozoScript's `vec()` function.
- **USB track metadata lookup** — `load_track_metadata()` now converts absolute
  paths to relative paths for USB storage.
- **USB playlist browsing** — Fixed playlist browsing broken after relative-path
  migration.
- **Track deletion cleanup** — `delete_track` now cleans all child relations
  (cue points, saved loops, stem links, tags, ML analysis, audio features).
- **Export phase message ordering** — Corrected progress message ordering to
  prevent UI stall during export.
- **DnB sub-genre consolidation** — Consolidated DnB sub-genre tags, suppressed
  redundant Instrumental genre tag.

### Improved

- **Tick handler performance** — Optimized hot-path tick handler with
  documentation for lock-free architecture.
- **Export metadata sync performance** — Replaced O(n^2) per-track
  `get_all_tracks()` scan with pre-built `HashMap` lookup.
