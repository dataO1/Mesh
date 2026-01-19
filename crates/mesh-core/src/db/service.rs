//! Thread-safe database service for mesh applications
//!
//! This module provides the primary public API for all database operations.
//! Domain code should ONLY use `DatabaseService` methods - never access
//! query modules directly.
//!
//! # Usage
//!
//! ```ignore
//! use mesh_core::db::{DatabaseService, Track};
//!
//! // Create the service (returns Arc for sharing)
//! let db = DatabaseService::new("~/Music/mesh-collection")?;
//!
//! // Load a track with all metadata
//! if let Some(track) = db.get_track_by_path("/path/to/track.wav")? {
//!     println!("BPM: {:?}, Hot cues: {}", track.bpm, track.cue_points.len());
//! }
//!
//! // Save a track (inserts or updates)
//! db.save_track(&track)?;
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::schema::{TrackRow, Playlist, AudioFeatures, CuePoint, SavedLoop, StemLink};
use super::queries::{TrackQuery, PlaylistQuery, SimilarityQuery, CuePointQuery, SavedLoopQuery, StemLinkQuery};
use super::{MeshDb, DbError};

// ============================================================================
// Track - The Public API Type
// ============================================================================

/// Complete track with all metadata - the primary public type
///
/// This is the single source of truth for track data across the system.
/// Used for loading, saving, and syncing tracks between databases.
///
/// # Example
/// ```ignore
/// // Load a track with all metadata
/// let track = db.get_track_by_path("/path/to/track.wav")?;
///
/// // Access all data in one place
/// println!("BPM: {:?}, Hot cues: {}", track.bpm, track.cue_points.len());
/// ```
#[derive(Debug, Clone)]
pub struct Track {
    /// Database ID (None for new tracks not yet saved)
    pub id: Option<i64>,
    /// Full path to the track file
    pub path: PathBuf,
    /// Folder path relative to collection root
    pub folder_path: String,
    /// Track display name
    pub name: String,
    /// Artist name
    pub artist: Option<String>,
    /// Detected BPM (after rounding)
    pub bpm: Option<f64>,
    /// Original BPM before rounding
    pub original_bpm: Option<f64>,
    /// Musical key (e.g., "8A", "11B")
    pub key: Option<String>,
    /// Duration in seconds
    pub duration_seconds: f64,
    /// Integrated LUFS loudness
    pub lufs: Option<f32>,
    /// Drop marker sample position (for stem alignment)
    pub drop_marker: Option<i64>,
    /// First beat sample position (for beat grid regeneration)
    pub first_beat_sample: i64,

    // ─── File Metadata ─────────────────────────────────────────────────
    /// File modification time (Unix timestamp)
    pub file_mtime: i64,
    /// File size in bytes
    pub file_size: i64,
    /// Path to cached waveform image
    pub waveform_path: Option<String>,

    // ─── Associated Metadata ───────────────────────────────────────────
    /// Hot cue points (up to 8)
    pub cue_points: Vec<CuePoint>,
    /// Saved loops (up to 8)
    pub saved_loops: Vec<SavedLoop>,
    /// Stem links for prepared mode
    pub stem_links: Vec<StemLink>,
}

impl Track {
    /// Create a new track with minimal required data
    pub fn new(path: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            id: None,
            path: path.into(),
            folder_path: String::new(),
            name: name.into(),
            artist: None,
            bpm: None,
            original_bpm: None,
            key: None,
            duration_seconds: 0.0,
            lufs: None,
            drop_marker: None,
            first_beat_sample: 0,
            file_mtime: 0,
            file_size: 0,
            waveform_path: None,
            cue_points: Vec::new(),
            saved_loops: Vec::new(),
            stem_links: Vec::new(),
        }
    }

    /// Convert from internal database row representation
    pub(crate) fn from_row(
        row: TrackRow,
        cue_points: Vec<CuePoint>,
        saved_loops: Vec<SavedLoop>,
        stem_links: Vec<StemLink>,
    ) -> Self {
        Self {
            id: Some(row.id),
            path: PathBuf::from(&row.path),
            folder_path: row.folder_path,
            name: row.name,
            artist: row.artist,
            bpm: row.bpm,
            original_bpm: row.original_bpm,
            key: row.key,
            duration_seconds: row.duration_seconds,
            lufs: row.lufs,
            drop_marker: row.drop_marker,
            first_beat_sample: row.first_beat_sample,
            file_mtime: row.file_mtime,
            file_size: row.file_size,
            waveform_path: row.waveform_path,
            cue_points,
            saved_loops,
            stem_links,
        }
    }

    /// Convert from row without loading associated data (for batch operations)
    pub(crate) fn from_row_only(row: TrackRow) -> Self {
        Self::from_row(row, Vec::new(), Vec::new(), Vec::new())
    }

    /// Convert to internal database row representation
    pub(crate) fn to_row(&self, collection_root: &Path) -> TrackRow {
        let id = self.id.unwrap_or_else(|| generate_track_id(&self.path));
        let folder_path = if self.folder_path.is_empty() {
            extract_folder_path(&self.path, collection_root)
        } else {
            self.folder_path.clone()
        };

        TrackRow {
            id,
            path: self.path.to_string_lossy().to_string(),
            folder_path,
            name: self.name.clone(),
            artist: self.artist.clone(),
            bpm: self.bpm,
            original_bpm: self.original_bpm,
            key: self.key.clone(),
            duration_seconds: self.duration_seconds,
            lufs: self.lufs,
            drop_marker: self.drop_marker,
            first_beat_sample: self.first_beat_sample,
            file_mtime: self.file_mtime,
            file_size: self.file_size,
            waveform_path: self.waveform_path.clone(),
        }
    }
}

// ============================================================================
// Internal Helpers
// ============================================================================

/// Generate a unique track ID from the file path
///
/// Uses a hash of the path to ensure consistency across sessions.
fn generate_track_id(path: &Path) -> i64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    // Mask to positive i64 to avoid issues with CozoDB
    (hasher.finish() as i64).abs()
}

/// Extract folder path from full track path relative to collection root
fn extract_folder_path(path: &Path, collection_root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(collection_root) {
        if let Some(parent) = relative.parent() {
            return parent.to_string_lossy().to_string();
        }
    }
    String::new()
}

/// Get file metadata (mtime, size) for a track
fn get_file_metadata(path: &Path) -> Result<(i64, i64), DbError> {
    let file_meta = std::fs::metadata(path)
        .map_err(|e| DbError::Migration(format!("Failed to read file metadata: {}", e)))?;

    let file_size = file_meta.len() as i64;
    let file_mtime = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    Ok((file_mtime, file_size))
}

// ============================================================================
// DatabaseService
// ============================================================================

/// Thread-safe database service for mesh applications
///
/// This service wraps a single `MeshDb` connection and provides
/// domain-specific methods for track and playlist management.
/// It's designed to be shared across threads via `Arc`.
///
/// # Design Principle
///
/// This is the ONLY public API for database operations. Domain code should
/// never access query modules directly - all operations go through this service.
pub struct DatabaseService {
    db: MeshDb,
    collection_root: PathBuf,
}

impl DatabaseService {
    /// Create a new database service at the given collection path
    ///
    /// The database file will be created at `collection_root/mesh.db`.
    /// Schema is initialized idempotently (safe to call multiple times).
    ///
    /// Returns an `Arc<Self>` for thread-safe sharing.
    pub fn new(collection_root: impl AsRef<Path>) -> Result<Arc<Self>, DbError> {
        let collection_root = collection_root.as_ref().to_path_buf();
        let db_path = collection_root.join("mesh.db");

        // Ensure directory exists
        std::fs::create_dir_all(&collection_root)
            .map_err(|e| DbError::Open(format!("Failed to create directory: {}", e)))?;

        log::info!("Opening database at {:?}", db_path);
        let db = MeshDb::open(&db_path)?;

        Ok(Arc::new(Self { db, collection_root }))
    }

    /// Create an in-memory database service (for testing)
    pub fn in_memory(collection_root: impl AsRef<Path>) -> Result<Arc<Self>, DbError> {
        let collection_root = collection_root.as_ref().to_path_buf();
        let db = MeshDb::in_memory()?;

        Ok(Arc::new(Self { db, collection_root }))
    }

    /// Get the collection root path
    pub fn collection_root(&self) -> &Path {
        &self.collection_root
    }

    // ========================================================================
    // Track Operations (Primary API)
    // ========================================================================

    /// Get a track by its database ID with all metadata
    ///
    /// Returns the track with cue_points, saved_loops, and stem_links loaded.
    pub fn get_track(&self, id: i64) -> Result<Option<Track>, DbError> {
        let row = match TrackQuery::get_by_id(&self.db, id)? {
            Some(r) => r,
            None => return Ok(None),
        };

        Ok(Some(self.load_track_metadata(row)?))
    }

    /// Get a track by its file path with all metadata
    ///
    /// Returns the track with cue_points, saved_loops, and stem_links loaded.
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<Track>, DbError> {
        let row = match TrackQuery::get_by_path(&self.db, path)? {
            Some(r) => r,
            None => return Ok(None),
        };

        Ok(Some(self.load_track_metadata(row)?))
    }

    /// Save a track with all its metadata
    ///
    /// This will insert or update the track and all associated metadata
    /// (cue_points, saved_loops, stem_links). Returns the track ID.
    ///
    /// # Example
    /// ```ignore
    /// let mut track = Track::new("/path/to/track.wav", "My Track");
    /// track.bpm = Some(128.0);
    /// track.cue_points.push(CuePoint { ... });
    ///
    /// let id = db.save_track(&track)?;
    /// ```
    pub fn save_track(&self, track: &Track) -> Result<i64, DbError> {
        // Get or generate file metadata if not present
        let (file_mtime, file_size) = if track.file_mtime == 0 && track.path.exists() {
            get_file_metadata(&track.path)?
        } else {
            (track.file_mtime, track.file_size)
        };

        // Create a track with updated file metadata
        let mut track_with_meta = track.clone();
        track_with_meta.file_mtime = file_mtime;
        track_with_meta.file_size = file_size;

        let row = track_with_meta.to_row(&self.collection_root);
        let track_id = row.id;

        log::info!(
            "DatabaseService::save_track: name='{}' path='{}' id={}",
            track.name,
            track.path.display(),
            track_id
        );

        // Upsert the track row
        TrackQuery::upsert(&self.db, &row)?;

        // Save associated data (replace all)
        CuePointQuery::delete_all_for_track(&self.db, track_id)?;
        for cue in &track.cue_points {
            let mut cue_with_id = cue.clone();
            cue_with_id.track_id = track_id;
            CuePointQuery::upsert(&self.db, &cue_with_id)?;
        }

        SavedLoopQuery::delete_all_for_track(&self.db, track_id)?;
        for loop_ in &track.saved_loops {
            let mut loop_with_id = loop_.clone();
            loop_with_id.track_id = track_id;
            SavedLoopQuery::upsert(&self.db, &loop_with_id)?;
        }

        StemLinkQuery::delete_all_for_track(&self.db, track_id)?;
        for link in &track.stem_links {
            let mut link_with_id = link.clone();
            link_with_id.track_id = track_id;
            StemLinkQuery::upsert(&self.db, &link_with_id)?;
        }

        log::info!("DatabaseService::save_track: SUCCESS id={}", track_id);
        Ok(track_id)
    }

    /// Sync a track from another database with ID remapping
    ///
    /// Used for USB export where track IDs differ between databases.
    /// Stem links are remapped to use local track IDs based on path matching.
    ///
    /// # Arguments
    /// * `track` - The track to sync (from source database)
    /// * `source_db` - The source database (for resolving stem link paths)
    pub fn sync_track_from(
        &self,
        track: &Track,
        source_db: &DatabaseService,
    ) -> Result<i64, DbError> {
        // Remap stem links - find local track IDs by path
        let mut remapped_links = Vec::new();
        for link in &track.stem_links {
            // Look up the source track path in the source database
            if let Some(source_track) = source_db.get_track(link.source_track_id)? {
                // Find the corresponding track in this database by path
                let source_path = source_track.path.to_string_lossy();
                if let Some(local_track) = self.get_track_by_path(&source_path)? {
                    remapped_links.push(StemLink {
                        track_id: 0, // Will be set by save_track
                        stem_index: link.stem_index,
                        source_track_id: local_track.id.unwrap(),
                        source_stem: link.source_stem,
                    });
                } else {
                    log::warn!(
                        "sync_track_from: source track '{}' not found in target DB, skipping stem link",
                        source_path
                    );
                }
            }
        }

        // Create track with remapped stem links
        let mut synced_track = track.clone();
        synced_track.stem_links = remapped_links;

        self.save_track(&synced_track)
    }

    /// Delete a track and all associated metadata
    pub fn delete_track(&self, id: i64) -> Result<(), DbError> {
        // Delete associated data first
        CuePointQuery::delete_all_for_track(&self.db, id)?;
        SavedLoopQuery::delete_all_for_track(&self.db, id)?;
        StemLinkQuery::delete_all_for_track(&self.db, id)?;

        // Delete the track
        TrackQuery::delete(&self.db, id)
    }

    /// Get all tracks in a folder
    ///
    /// Returns tracks with basic metadata only (no cue_points/saved_loops/stem_links).
    /// Use `get_track()` or `get_track_by_path()` for full metadata.
    pub fn get_tracks_in_folder(&self, folder_path: &str) -> Result<Vec<Track>, DbError> {
        let rows = TrackQuery::get_by_folder(&self.db, folder_path)?;
        Ok(rows.into_iter().map(Track::from_row_only).collect())
    }

    /// Search tracks by name or artist
    ///
    /// Returns tracks with basic metadata only.
    pub fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<Track>, DbError> {
        let rows = TrackQuery::search(&self.db, query, limit)?;
        Ok(rows.into_iter().map(Track::from_row_only).collect())
    }

    /// Get all unique folder paths in the collection
    pub fn get_folders(&self) -> Result<Vec<String>, DbError> {
        TrackQuery::get_folders(&self.db)
    }

    /// Count total tracks in the database
    pub fn track_count(&self) -> Result<usize, DbError> {
        TrackQuery::count(&self.db)
    }

    /// Update a track field
    pub fn update_track_field(&self, track_id: i64, field: &str, value: &str) -> Result<(), DbError> {
        TrackQuery::update_field(&self.db, track_id, field, value)
    }

    // ========================================================================
    // Individual Metadata Access (for targeted updates)
    // ========================================================================

    /// Get cue points for a track
    pub fn get_cue_points(&self, track_id: i64) -> Result<Vec<CuePoint>, DbError> {
        CuePointQuery::get_for_track(&self.db, track_id)
    }

    /// Save a single cue point
    pub fn save_cue_point(&self, cue: &CuePoint) -> Result<(), DbError> {
        CuePointQuery::upsert(&self.db, cue)
    }

    /// Delete a single cue point
    pub fn delete_cue_point(&self, track_id: i64, index: u8) -> Result<(), DbError> {
        CuePointQuery::delete(&self.db, track_id, index)
    }

    /// Get saved loops for a track
    pub fn get_saved_loops(&self, track_id: i64) -> Result<Vec<SavedLoop>, DbError> {
        SavedLoopQuery::get_for_track(&self.db, track_id)
    }

    /// Save a single saved loop
    pub fn save_saved_loop(&self, loop_: &SavedLoop) -> Result<(), DbError> {
        SavedLoopQuery::upsert(&self.db, loop_)
    }

    /// Delete a single saved loop
    pub fn delete_saved_loop(&self, track_id: i64, index: u8) -> Result<(), DbError> {
        SavedLoopQuery::delete(&self.db, track_id, index)
    }

    /// Get stem links for a track
    pub fn get_stem_links(&self, track_id: i64) -> Result<Vec<StemLink>, DbError> {
        StemLinkQuery::get_for_track(&self.db, track_id)
    }

    /// Save a single stem link
    pub fn save_stem_link(&self, link: &StemLink) -> Result<(), DbError> {
        StemLinkQuery::upsert(&self.db, link)
    }

    /// Delete a single stem link
    pub fn delete_stem_link(&self, track_id: i64, stem_index: u8) -> Result<(), DbError> {
        StemLinkQuery::delete(&self.db, track_id, stem_index)
    }

    /// Convert database stem links (ID-based) to runtime format (path-based)
    ///
    /// This is the authoritative conversion from database format to runtime format.
    /// Invalid links (missing source track) are filtered out with warnings.
    ///
    /// Used by both mesh-cue and mesh-player to populate TrackMetadata.stem_links.
    pub fn convert_stem_links_to_runtime(&self, db_links: &[StemLink]) -> Vec<crate::audio_file::StemLinkReference> {
        db_links.iter().filter_map(|link| {
            match self.get_track(link.source_track_id) {
                Ok(Some(source_track)) => {
                    Some(crate::audio_file::StemLinkReference {
                        stem_index: link.stem_index,
                        source_path: std::path::PathBuf::from(&source_track.path),
                        source_stem: link.source_stem,
                        source_drop_marker: source_track.drop_marker.unwrap_or(0) as u64,
                    })
                }
                Ok(None) => {
                    log::warn!("Stem link source track not found: id={}", link.source_track_id);
                    None
                }
                Err(e) => {
                    log::warn!("Failed to load stem link source track: {:?}", e);
                    None
                }
            }
        }).collect()
    }

    /// Get track metadata with stem links converted to runtime format
    ///
    /// This is the preferred method for loading track metadata when you need
    /// stem_links populated (for playback with linked stems).
    pub fn get_track_metadata(&self, path: &str) -> Result<Option<crate::audio_file::TrackMetadata>, DbError> {
        let track = match self.get_track_by_path(path)? {
            Some(t) => t,
            None => return Ok(None),
        };

        // Convert stem links from database format to runtime format
        let stem_links = self.convert_stem_links_to_runtime(&track.stem_links);
        if !stem_links.is_empty() {
            log::info!("DatabaseService: Loaded {} stem links for {:?}", stem_links.len(), path);
        }

        // Convert track to metadata and inject the converted stem links
        let mut metadata: crate::audio_file::TrackMetadata = track.into();
        metadata.stem_links = stem_links;

        Ok(Some(metadata))
    }

    // ========================================================================
    // Playlist Operations
    // ========================================================================

    /// Get all root-level playlists
    pub fn get_root_playlists(&self) -> Result<Vec<Playlist>, DbError> {
        PlaylistQuery::get_roots(&self.db)
    }

    /// Get child playlists of a parent
    pub fn get_child_playlists(&self, parent_id: i64) -> Result<Vec<Playlist>, DbError> {
        PlaylistQuery::get_children(&self.db, parent_id)
    }

    /// Get a playlist by name
    pub fn get_playlist_by_name(&self, name: &str, parent_id: Option<i64>) -> Result<Option<Playlist>, DbError> {
        PlaylistQuery::get_by_name(&self.db, name, parent_id)
    }

    /// Create a new playlist
    pub fn create_playlist(&self, name: &str, parent_id: Option<i64>) -> Result<i64, DbError> {
        PlaylistQuery::create(&self.db, name, parent_id)
    }

    /// Rename a playlist
    pub fn rename_playlist(&self, playlist_id: i64, new_name: &str) -> Result<(), DbError> {
        PlaylistQuery::rename(&self.db, playlist_id, new_name)
    }

    /// Delete a playlist
    pub fn delete_playlist(&self, playlist_id: i64) -> Result<(), DbError> {
        PlaylistQuery::delete(&self.db, playlist_id)
    }

    /// Get tracks in a playlist
    ///
    /// Returns tracks with basic metadata only.
    pub fn get_playlist_tracks(&self, playlist_id: i64) -> Result<Vec<Track>, DbError> {
        let rows = PlaylistQuery::get_tracks(&self.db, playlist_id)?;
        Ok(rows.into_iter().map(Track::from_row_only).collect())
    }

    /// Add a track to a playlist
    pub fn add_track_to_playlist(&self, playlist_id: i64, track_id: i64, sort_order: i32) -> Result<(), DbError> {
        PlaylistQuery::add_track(&self.db, playlist_id, track_id, sort_order)
    }

    /// Remove a track from a playlist
    pub fn remove_track_from_playlist(&self, playlist_id: i64, track_id: i64) -> Result<(), DbError> {
        PlaylistQuery::remove_track(&self.db, playlist_id, track_id)
    }

    /// Get the next sort order for a playlist
    pub fn next_playlist_sort_order(&self, playlist_id: i64) -> Result<i32, DbError> {
        PlaylistQuery::next_sort_order(&self.db, playlist_id)
    }

    // ========================================================================
    // Audio Features & Similarity
    // ========================================================================

    /// Store audio features for a track
    pub fn store_audio_features(&self, track_id: i64, features: &AudioFeatures) -> Result<(), DbError> {
        SimilarityQuery::upsert_features(&self.db, track_id, features)
    }

    /// Find similar tracks using audio features
    ///
    /// Returns tracks with basic metadata only and similarity scores.
    pub fn find_similar_tracks(&self, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let results = SimilarityQuery::find_similar(&self.db, track_id, limit)?;
        Ok(results.into_iter().map(|(row, score)| (Track::from_row_only(row), score)).collect())
    }

    // ========================================================================
    // Internal Helpers
    // ========================================================================

    /// Load full track metadata (cue_points, saved_loops, stem_links) for a row
    fn load_track_metadata(&self, row: TrackRow) -> Result<Track, DbError> {
        let track_id = row.id;
        let cue_points = CuePointQuery::get_for_track(&self.db, track_id)?;
        let saved_loops = SavedLoopQuery::get_for_track(&self.db, track_id)?;
        let stem_links = StemLinkQuery::get_for_track(&self.db, track_id)?;

        Ok(Track::from_row(row, cue_points, saved_loops, stem_links))
    }

    // ========================================================================
    // Low-level Access (for advanced usage within mesh-core)
    // ========================================================================

    /// Get the underlying MeshDb for advanced queries
    ///
    /// This is `pub(crate)` - only accessible within mesh-core.
    /// Domain code should use the methods above instead.
    pub(crate) fn db(&self) -> &MeshDb {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_service_creation() {
        let temp = TempDir::new().unwrap();
        let service = DatabaseService::new(temp.path()).unwrap();
        assert_eq!(service.track_count().unwrap(), 0);
    }

    #[test]
    fn test_service_save_and_get_track() {
        let temp = TempDir::new().unwrap();
        let tracks_dir = temp.path().join("tracks");
        std::fs::create_dir_all(&tracks_dir).unwrap();

        // Create a dummy file
        let track_path = tracks_dir.join("test.wav");
        std::fs::write(&track_path, b"dummy").unwrap();

        let service = DatabaseService::new(temp.path()).unwrap();

        let mut track = Track::new(track_path.clone(), "Test Track");
        track.artist = Some("Artist".to_string());
        track.bpm = Some(120.0);
        track.original_bpm = Some(120.0);
        track.key = Some("Am".to_string());
        track.duration_seconds = 180.0;
        track.lufs = Some(-14.0);

        // Add a cue point
        track.cue_points.push(CuePoint {
            track_id: 0, // Will be set by save_track
            index: 0,
            sample_position: 44100,
            label: Some("Drop".to_string()),
            color: Some("#ff0000".to_string()),
        });

        let track_id = service.save_track(&track).unwrap();
        assert!(track_id != 0);

        // Retrieve and verify
        let loaded = service.get_track(track_id).unwrap().unwrap();
        assert_eq!(loaded.name, "Test Track");
        assert_eq!(loaded.artist, Some("Artist".to_string()));
        assert_eq!(loaded.bpm, Some(120.0));
        assert_eq!(loaded.cue_points.len(), 1);
        assert_eq!(loaded.cue_points[0].sample_position, 44100);
        assert_eq!(loaded.cue_points[0].label, Some("Drop".to_string()));

        let count = service.track_count().unwrap();
        assert_eq!(count, 1);

        let folders = service.get_folders().unwrap();
        assert!(folders.contains(&"tracks".to_string()));
    }

    #[test]
    fn test_service_thread_safety() {
        use std::thread;

        let temp = TempDir::new().unwrap();
        let tracks_dir = temp.path().join("tracks");
        std::fs::create_dir_all(&tracks_dir).unwrap();

        let service = DatabaseService::new(temp.path()).unwrap();

        // Spawn multiple threads that all use the same service
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let service = service.clone();
                let tracks_dir = tracks_dir.clone();

                thread::spawn(move || {
                    let track_path = tracks_dir.join(format!("track_{}.wav", i));
                    std::fs::write(&track_path, b"dummy").unwrap();

                    let mut track = Track::new(track_path, format!("Track {}", i));
                    track.bpm = Some(120.0);
                    track.original_bpm = Some(120.0);
                    track.duration_seconds = 180.0;

                    service.save_track(&track).unwrap()
                })
            })
            .collect();

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify all tracks were added
        let count = service.track_count().unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn test_service_delete_track() {
        let temp = TempDir::new().unwrap();
        let tracks_dir = temp.path().join("tracks");
        std::fs::create_dir_all(&tracks_dir).unwrap();

        let track_path = tracks_dir.join("test.wav");
        std::fs::write(&track_path, b"dummy").unwrap();

        let service = DatabaseService::new(temp.path()).unwrap();

        let mut track = Track::new(track_path, "Test Track");
        track.duration_seconds = 180.0;
        track.cue_points.push(CuePoint {
            track_id: 0,
            index: 0,
            sample_position: 44100,
            label: None,
            color: None,
        });

        let track_id = service.save_track(&track).unwrap();
        assert_eq!(service.track_count().unwrap(), 1);

        service.delete_track(track_id).unwrap();
        assert_eq!(service.track_count().unwrap(), 0);

        // Cue points should also be deleted
        let cues = service.get_cue_points(track_id).unwrap();
        assert!(cues.is_empty());
    }
}
