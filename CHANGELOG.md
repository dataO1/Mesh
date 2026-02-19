# Changelog

All notable changes to Mesh are documented in this file.

---

## [0.9.0]

### Added

- **Suggestion energy slider in MIDI learn** — The Browser phase now includes
  two Suggestion Energy steps (left and right side), placed after the browse
  encoder. This allows mapping physical knobs/faders to the smart suggestion
  energy direction slider during MIDI learn. The `deck.suggestion_energy` action
  controls the global energy bias (DROP ↔ PEAK) used by the suggestion engine.
- **Streaming track loading with priority regions** — Track loading is now a
  three-phase progressive pipeline: (1) skeleton with metadata loads instantly
  (<10 ms), giving immediate access to beat markers, cue markers, and navigation;
  (2) priority regions around hot cues and the drop marker load next (~200 ms);
  (3) remaining audio fills in incrementally. The DJ can beat-jump, seek, and
  navigate cue points while audio loads in the background.
- **Incremental waveform visualization** — The overview waveform now grows
  visually as audio loads. Priority regions (hot cue areas) appear first, then
  gap regions fill in progressively in ~15-second visual batches. Unloaded areas
  render as flat/silent, giving clear visual feedback of which parts of the track
  are ready for playback. High-resolution zoomed peaks also update incrementally.
- **Instant partial playback during loading** — Stem buffer snapshots are
  delivered to the audio engine at ~100-second intervals via `UpgradeStems`, so
  the DJ can press play or cue and hear audio from any loaded region. Visual
  peak updates (cheap, ~2 MB) are decoupled from stem clones (expensive,
  ~460 MB) — the waveform grows smoothly while playback catches up at clone
  boundaries. Unloaded areas produce silence on playback.
- **Region-based audio file reading** — New `read_region_into()` method on
  `AudioFileReader` enables seeking to arbitrary sample positions and reading
  directly into pre-allocated stem buffers. Supports 16-bit, 24-bit, and
  32-bit (float and integer) formats. Existing full-read methods now delegate
  to the region reader internally, eliminating code duplication.
- **Engine `UpgradeStems` command** — New real-time-safe command that upgrades
  a deck's stem buffers without resetting playback position. Uses `basedrop::Shared`
  for lock-free deallocation on the audio thread.
- **Skeleton track loading** — `create_skeleton_and_load()` on the domain layer
  creates an instant-load track with zero-length stems but correct duration,
  beat grid, cue points, and metadata. The engine uses `duration_samples` for
  navigation and `stem_data.len()` for audio reads, so navigation works
  immediately while stems are still empty.

### Improved

- **Suggestion energy MIDI debounce** — The energy direction fader now uses
  trailing-edge debounce (300ms) so the suggestion query only fires once the
  fader stops moving, instead of on every value change. Moving the fader also
  auto-enables suggestion mode if not already active.
- **Track load memory usage** — Stem clones (~460 MB each) are sent only at
  ~100-second intervals (~5 clones per 5-minute track, ~500 ms total overhead).
  Visual peak updates are sent every ~15 seconds at negligible cost (~2 MB).
  Peak memory during loading is ~920 MB; the `basedrop` GC thread collects
  stale clones within 100 ms, preventing unbounded growth.
- **Priority region planning** — New `regions` module computes optimal load
  regions around hot cues, drop markers, and the first beat. Regions within
  64 beats of each other are merged to minimize seek operations. Gap regions
  (everything not covered by priority areas) are computed for sequential
  background filling.

---

## [0.8.10]

### Improved

- **USB export throughput** — Rewrote the export pipeline to separate file I/O
  from database I/O. Track files are now copied sequentially with 1 MB buffered
  writes and `fsync` per file (replacing parallel random writes via `par_iter`).
  The USB database is staged locally: copied to a temp directory, updated there
  with all metadata/playlist/deletion operations, then written back as a single
  sequential copy. This eliminates random I/O on flash storage and should reduce
  export times by 50–70%.
- **Batched tag inserts** — `sync_track_atomic` now uses a single CozoDB batch
  query for tag insertion instead of N individual `:put` operations per track.
- **Buffered file copy with fsync** — New `copy_large_file()` utility uses
  `BufReader`/`BufWriter` with 1 MB buffers, `posix_fadvise(SEQUENTIAL)` on
  Linux, and `sync_all()` for data safety on removable media.
- **Simplified export progress** — Merged five separate metadata/playlist
  progress phases into a single unified "Updating database" phase, reducing UI
  complexity and message overhead.

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
- **mesh-cue Windows linker error** — `build.rs` emitted ELF-specific linker
  flags (`--disable-new-dtags`, `--no-as-needed`, `-rpath`) unconditionally.
  MinGW's `ld` doesn't recognize these. Now gated behind a `TARGET` check so
  they only apply on Linux.

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
