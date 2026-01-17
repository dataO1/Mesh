# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added - Performance Optimization Phase 1: CozoDB Database Layer

This phase introduces a high-performance database layer using CozoDB, replacing the
per-file metadata reading approach that caused O(n) performance degradation.

#### New Database Module (`mesh-core/src/db/`)

- **`mod.rs`**: Main database module with `MeshDb` wrapper
  - `MeshDb::open(path)` - Open or create a persistent SQLite-backed database
  - `MeshDb::in_memory()` - Create an in-memory database for testing
  - `run_script()` / `run_query()` - Execute CozoScript queries
  - `params!` macro for convenient parameter map creation

- **`schema.rs`**: CozoDB relation definitions
  - Core types: `Track`, `Playlist`, `PlaylistTrack`, `CuePoint`, `SavedLoop`
  - Graph edge types: `SimilarTo`, `PlayedAfter`, `HarmonicMatch`
  - `AudioFeatures` - 16-dimensional vector for similarity search:
    - Rhythm (4 dims): BPM, confidence, beat strength, regularity
    - Harmony (4 dims): Key (circular encoding), mode, complexity
    - Energy (4 dims): LUFS, dynamic range, mean energy, variance
    - Timbre (4 dims): Spectral centroid, bandwidth, rolloff, MFCC flatness
  - Automatic schema creation on database open

- **`queries.rs`**: Typed query builders
  - `TrackQuery`: CRUD operations, search, folder listing
  - `PlaylistQuery`: Hierarchical playlist navigation
  - `SimilarityQuery`: Vector similarity search, harmonic compatibility

- **`migration.rs`**: WAV collection migration utility
  - Parallel metadata reading using rayon
  - Progress callbacks for UI feedback
  - Incremental update support via file modification time checking
  - Batch insert optimization for large collections

#### Dependencies Added

- `cozo = "0.7.6"` with SQLite storage backend
- `crossbeam = "0.8"` for message-driven architecture (Phase 2)
- `notify = "6"` for file system watching (Phase 2)
- `rayon = ">=1.9,<1.10"` (pinned for graph_builder 0.4.1 compatibility with graph-algo)
- `thiserror` for typed errors

#### Performance Targets

| Operation | Before | Target |
|-----------|--------|--------|
| Folder selection (100 tracks) | 2-5s | <5ms |
| Folder selection (1000 tracks) | 20-50s | <20ms |
| USB preload (100 tracks) | 5-10s | <100ms |
| Text search (10K tracks) | N/A | <10ms |

### Fixed

- Pre-existing test issue: `SyncPlan` missing `tracks_missing_lufs` field in test

### Added - Performance Optimization Phase 2: Message-Driven Services

This phase introduces a message-driven architecture with background services,
replacing direct synchronous operations to keep the UI responsive.

#### New Services Module (`mesh-core/src/services/`)

- **`messages.rs`**: Command and event type definitions
  - `QueryCommand` - Request-reply commands for database operations
  - `WatchCommand` - Commands for file system monitoring
  - `AppEvent` - Broadcast events for state changes
  - `EventBus` - Multi-subscriber event broadcasting
  - `ServiceHandle` - Generic handle for service communication
  - Helper types: `EnergyDirection`, `MixSuggestion`, `MixReason`, `AnalysisPhase`

- **`query.rs`**: QueryService - background database service
  - Runs in dedicated thread, handles all CozoDB operations
  - Request-reply pattern using tokio oneshot channels
  - `QueryClient` - convenient API for common operations
  - Mix suggestion algorithm with BPM and energy-based filtering

- **`watch.rs`**: FileWatchService - file system monitoring
  - Uses `notify` crate for cross-platform file watching
  - Debounced events to avoid duplicate notifications
  - Filters by file extension (WAV files)
  - `WatchClient` - API for managing watched directories

#### Architecture Pattern

```text
┌─────────────┐     Commands      ┌──────────────┐
│   UI Thread │ ───────────────►  │ QueryService │
│             │ ◄─────────────── │   (CozoDB)   │
└─────────────┘     Replies       └──────────────┘
      │                                  │
      │ Subscribe                        │ Publish
      ▼                                  ▼
┌─────────────────────────────────────────────┐
│               Event Bus                      │
└─────────────────────────────────────────────┘
                     ▲
                     │ Publish
              ┌──────────────┐
              │ WatchService │
              └──────────────┘
```

#### Dependencies Added

- `tokio` (workspace) - for oneshot channels in request-reply patterns

### Added - Performance Optimization Phase 3: Audio Feature Extraction & Vector Search

This phase implements the audio feature extraction system and HNSW vector index
for similarity-based track discovery and recommendations.

#### New Features Module (`mesh-core/src/features/`)

- **`mod.rs`**: Module entry point with re-exports
  - `AudioFeatures` - 16-dimensional feature vector struct
  - `extract_audio_features()` - Main extraction function
  - `extract_audio_features_in_subprocess()` - Process-isolated extraction

- **`extraction.rs`**: Audio feature extraction using Essentia algorithms
  - **Rhythm features** (4 dimensions):
    - `bpm_normalized` - BPM normalized to [0,1] range via RhythmDescriptors
    - `bpm_confidence` - Confidence from RhythmDescriptors
    - `beat_strength` - Danceability score normalized from Danceability algorithm
    - `rhythm_regularity` - First peak weight from BPM histogram
  - **Harmony features** (4 dimensions):
    - `key_x`, `key_y` - Circular key encoding via KeyExtractor (EDMA profile)
    - `mode` - Major (1.0) / Minor (0.0) from KeyExtractor
    - `harmonic_complexity` - SpectralComplexity normalized
  - **Energy features** (4 dimensions):
    - `lufs_normalized` - LUFS via LoudnessEbur128
    - `dynamic_range` - DynamicComplexity algorithm
    - `energy_mean`, `energy_variance` - Segment-based Energy statistics
  - **Timbre features** (4 dimensions):
    - `spectral_centroid` - SpectralCentroidTime
    - `spectral_bandwidth` - Computed from spectrum variance
    - `spectral_rolloff` - RollOff at 85% energy
    - `mfcc_flatness` - Flatness ratio (geometric/arithmetic mean)

#### Database Schema Updates

- **`audio_features` relation**: New relation for storing feature vectors
  - `track_id: Int` - Foreign key to tracks
  - `vec: <F32; 16>` - 16-dimensional F32 vector type

- **HNSW Vector Index**: `audio_features:similarity_index`
  - Cosine distance metric for musical similarity
  - m=16 connections, ef_construction=200 for good recall
  - Automatic indexing on insert via `:put audio_features`

#### Query API Extensions

- `SimilarityQuery::find_similar()` - HNSW-based k-nearest neighbor search
- `SimilarityQuery::upsert_features()` - Insert/update track features
- `SimilarityQuery::has_features()` - Check if track has features
- `SimilarityQuery::get_tracks_with_features()` - List all indexed tracks
- `SimilarityQuery::count_with_features()` - Count indexed tracks

#### Dependencies Added

- `essentia = "0.1.5"` - Rust bindings for Essentia audio analysis library
- `essentia-sys = "0.1.5"` - FFI bindings for StereoSample type
- `procspawn = "1.0"` - Process isolation for thread-unsafe Essentia

#### Performance Targets

| Operation | Target |
|-----------|--------|
| Feature extraction (3 min track) | <5s |
| Find similar (top 10) | <5ms |
| Batch indexing (100 tracks) | <1s |

### Added - Performance Optimization Phase 4: Polymorphic Playlist Storage

This phase introduces polymorphic playlist storage, enabling the UI to transparently
use either filesystem scanning or database-backed instant access.

#### New DatabaseStorage Implementation (`mesh-core/src/playlist/database.rs`)

- **`DatabaseStorage`**: Implements `PlaylistStorage` trait using CozoDB backend
  - `get_tracks()` reads from pre-indexed database (O(1) vs O(n) file I/O)
  - In-memory `TreeCache` for fast folder hierarchy navigation
  - Cache invalidation via `dirty` flag on mutations
  - Thread-safe via `RwLock<TreeCache>`

#### Polymorphic Storage Architecture

- **`CollectionState::playlist_storage`**: Changed from `Box<FilesystemStorage>` to `Box<dyn PlaylistStorage>`
- **Helper functions**: Updated to accept `&dyn PlaylistStorage`:
  - `build_tree_nodes(storage: &dyn PlaylistStorage)`
  - `get_tracks_for_folder(storage: &dyn PlaylistStorage, folder_id: &NodeId)`

#### Storage Selection Logic (`mesh-cue/src/ui/app.rs`)

- Automatic backend selection at startup:
  - If `collection_root/mesh.db` exists → use `DatabaseStorage` for instant access
  - Otherwise → fall back to `FilesystemStorage` (legacy mode)

#### Key Performance Benefit

| Operation | FilesystemStorage | DatabaseStorage |
|-----------|-------------------|-----------------|
| `get_tracks(100)` | ~2-5s (file I/O) | **<5ms** (DB query) |
| `get_tracks(1000)` | ~20-50s | **<20ms** |

### Technical Notes

- CozoDB uses Datalog for queries, enabling powerful recursive graph traversal
- The `graph-algo` feature is enabled by pinning rayon to `>=1.9,<1.10` (graph_builder 0.4.1
  has a bug with rayon 1.10+ that will be fixed in a future release)
- Essentia is NOT thread-safe; all extraction runs via procspawn subprocesses
- Feature vectors are normalized to [0,1] range for optimal cosine similarity
