# Changelog

## Cross-Source Suggestions & USB Export Fixes

### Features
- **Cross-source suggestion search**: Suggestions now query all connected
  databases (local + USB). When a deck seed is loaded from USB, HNSW vector
  search runs across both the local and USB databases, combining results into a
  single ranked list with source tags ("Local" / "USB") on each suggestion.
- **Cross-source deduplication**: When the same track exists in both local and
  USB databases, only the entry with the best HNSW distance is kept, preventing
  duplicate suggestions.
- **Metadata sync progress**: USB export now reports per-track progress during
  the metadata-only sync phase, so the overlay progress bar updates smoothly
  instead of stalling.

### Bug Fixes
- **Audio features not exported to USB**: `get_audio_features()` failed
  silently on CozoDB's native `DataValue::Vec(Vector::F32(...))` type, only
  matching `DataValue::List`. This caused audio feature vectors to never be
  synced to USB, breaking all similarity-based suggestions from USB seeds.
- **Cross-DB HNSW search "Expected vector" error**: `find_similar_by_vector()`
  passed the query vector as `DataValue::List`, but CozoDB's HNSW `~` operator
  requires a proper Vector type. Fixed by wrapping with CozoScript's `vec()`
  function at query time.
- **USB track metadata lookup**: `load_track_metadata()` now converts absolute
  file paths to relative paths when the active storage is USB, matching the
  portable path format stored in the USB database.

### Performance
- **Export metadata sync**: Replaced O(n^2) per-track `get_all_tracks()` scan
  with a pre-built `HashMap<filename, track_id>` lookup. Switched from
  `par_iter` to sequential iteration since DB writes are serialized anyway.
