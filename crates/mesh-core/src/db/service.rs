//! Thread-safe database service for mesh applications
//!
//! This module provides a high-level API for database operations,
//! abstracting away CozoDB details and ensuring thread-safe access
//! through a shared `Arc<DatabaseService>`.
//!
//! # Usage
//!
//! ```ignore
//! use mesh_core::db::DatabaseService;
//!
//! // Create the service (returns Arc for sharing)
//! let service = DatabaseService::new("~/Music/mesh-collection")?;
//!
//! // Pass to threads, use domain methods
//! let track_id = service.add_track(&track_data)?;
//! let tracks = service.get_tracks_in_folder("tracks")?;
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cozo::DataValue;

use super::schema::{Track, Playlist, AudioFeatures, CuePoint, SavedLoop, StemLink};
use super::queries::{TrackQuery, PlaylistQuery, SimilarityQuery, CuePointQuery, SavedLoopQuery, StemLinkQuery};
use super::{MeshDb, DbError};

// ============================================================================
// Track Insertion Types
// ============================================================================

/// Parameters for inserting a newly analyzed track into the database.
///
/// This is used during import to store analysis results directly in the database.
#[derive(Debug, Clone)]
pub struct NewTrackData {
    /// Full path to the track file
    pub path: PathBuf,
    /// Track display name
    pub name: String,
    /// Artist name (if extracted from filename)
    pub artist: Option<String>,
    /// Detected BPM
    pub bpm: Option<f64>,
    /// Original BPM before rounding
    pub original_bpm: Option<f64>,
    /// Musical key
    pub key: Option<String>,
    /// Duration in seconds
    pub duration_seconds: f64,
    /// Integrated LUFS loudness
    pub lufs: Option<f32>,
    /// First beat sample position (for beatgrid regeneration)
    /// Required - beatgrid is essential for beat matching
    pub first_beat_sample: i64,
}

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

/// Insert a newly analyzed track into the database.
///
/// This is called during the import process after audio analysis completes,
/// allowing the database to be populated immediately.
fn insert_analyzed_track(
    db: &MeshDb,
    collection_root: &Path,
    track_data: &NewTrackData,
) -> Result<i64, DbError> {
    let track_id = generate_track_id(&track_data.path);
    let folder_path = extract_folder_path(&track_data.path, collection_root);

    // Get file metadata
    let file_meta = std::fs::metadata(&track_data.path)
        .map_err(|e| DbError::Migration(format!("Failed to read file metadata: {}", e)))?;

    let file_size = file_meta.len() as i64;
    let file_mtime = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Insert the track
    log::debug!(
        "insert_analyzed_track: inserting track_id={} name='{}' folder_path='{}' path='{}'",
        track_id, track_data.name, folder_path, track_data.path.display()
    );

    let mut params = std::collections::BTreeMap::new();
    params.insert("id".to_string(), DataValue::from(track_id));
    params.insert("path".to_string(), DataValue::from(track_data.path.to_string_lossy().to_string()));
    params.insert("folder_path".to_string(), DataValue::from(folder_path.clone()));
    params.insert("name".to_string(), DataValue::from(track_data.name.clone()));
    params.insert("artist".to_string(), track_data.artist.clone().map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("bpm".to_string(), track_data.bpm.map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("original_bpm".to_string(), track_data.original_bpm.map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("key".to_string(), track_data.key.clone().map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("duration_seconds".to_string(), DataValue::from(track_data.duration_seconds));
    params.insert("lufs".to_string(), track_data.lufs.map(|l| DataValue::from(l as f64)).unwrap_or(DataValue::Null));
    params.insert("drop_marker".to_string(), DataValue::Null);
    params.insert("first_beat_sample".to_string(), DataValue::from(track_data.first_beat_sample));
    params.insert("file_mtime".to_string(), DataValue::from(file_mtime));
    params.insert("file_size".to_string(), DataValue::from(file_size));
    params.insert("waveform_path".to_string(), DataValue::Null);

    let result = db.run_script(
        r#"
        ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
          duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
            $id, $path, $folder_path, $name, $artist, $bpm, $original_bpm, $key,
            $duration_seconds, $lufs, $drop_marker, $first_beat_sample, $file_mtime, $file_size, $waveform_path
        ]]
        :put tracks {
            id =>
            path, folder_path, name, artist, bpm, original_bpm, key,
            duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path
        }
        "#,
        params,
    );

    match &result {
        Ok(_) => log::info!("insert_analyzed_track: SUCCESS track_id={} folder_path='{}'", track_id, folder_path),
        Err(e) => log::error!("insert_analyzed_track: FAILED track_id={} error={}", track_id, e),
    }

    result?;
    Ok(track_id)
}

/// Full track metadata loaded from database
///
/// This struct contains all metadata needed to display and play a track,
/// loaded from the database tables (tracks, cue_points, saved_loops, stem_links).
#[derive(Debug, Clone)]
pub struct LoadedTrackMetadata {
    /// The track record from the tracks table
    pub track: Track,
    /// Cue points for this track
    pub cue_points: Vec<CuePoint>,
    /// Saved loops for this track
    pub saved_loops: Vec<SavedLoop>,
    /// Stem links for this track (prepared mode)
    pub stem_links: Vec<StemLink>,
}

/// Thread-safe database service for mesh applications
///
/// This service wraps a single `MeshDb` connection and provides
/// domain-specific methods for track and playlist management.
/// It's designed to be shared across threads via `Arc`.
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
    // Track Operations
    // ========================================================================

    /// Add an analyzed track to the database
    ///
    /// Returns the generated track ID.
    pub fn add_track(&self, track_data: &NewTrackData) -> Result<i64, DbError> {
        log::info!(
            "DatabaseService::add_track: name='{}' path='{}' collection_root='{}'",
            track_data.name,
            track_data.path.display(),
            self.collection_root.display()
        );
        let track_id = insert_analyzed_track(&self.db, &self.collection_root, track_data)?;
        log::info!("DatabaseService::add_track: SUCCESS id={}, name={}", track_id, track_data.name);
        Ok(track_id)
    }

    /// Get a track by its database ID
    pub fn get_track(&self, id: i64) -> Result<Option<Track>, DbError> {
        TrackQuery::get_by_id(&self.db, id)
    }

    /// Get a track by its file path
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<Track>, DbError> {
        TrackQuery::get_by_path(&self.db, path)
    }

    /// Get all tracks in a folder
    pub fn get_tracks_in_folder(&self, folder_path: &str) -> Result<Vec<Track>, DbError> {
        TrackQuery::get_by_folder(&self.db, folder_path)
    }

    /// Get all unique folder paths in the collection
    pub fn get_folders(&self) -> Result<Vec<String>, DbError> {
        TrackQuery::get_folders(&self.db)
    }

    /// Count total tracks in the database
    pub fn track_count(&self) -> Result<usize, DbError> {
        TrackQuery::count(&self.db)
    }

    /// Search tracks by name or artist
    pub fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<Track>, DbError> {
        TrackQuery::search(&self.db, query, limit)
    }

    /// Update a track field
    pub fn update_track_field(&self, track_id: i64, field: &str, value: &str) -> Result<(), DbError> {
        TrackQuery::update_field(&self.db, track_id, field, value)
    }

    /// Delete a track from the database
    pub fn delete_track(&self, track_id: i64) -> Result<(), DbError> {
        TrackQuery::delete(&self.db, track_id)
    }

    /// Load full track metadata by file path
    ///
    /// This loads the track record along with all associated cue points,
    /// saved loops, and stem links. Returns None if the track is not found.
    pub fn load_track_metadata_by_path(&self, path: &str) -> Result<Option<LoadedTrackMetadata>, DbError> {
        // First, find the track by path
        let track = match TrackQuery::get_by_path(&self.db, path)? {
            Some(t) => t,
            None => return Ok(None),
        };

        self.load_track_metadata_by_id(track.id).map(|opt| opt.or(Some(LoadedTrackMetadata {
            track,
            cue_points: Vec::new(),
            saved_loops: Vec::new(),
            stem_links: Vec::new(),
        })))
    }

    /// Load full track metadata by track ID
    ///
    /// This loads the track record along with all associated cue points,
    /// saved loops, and stem links. Returns None if the track is not found.
    pub fn load_track_metadata_by_id(&self, track_id: i64) -> Result<Option<LoadedTrackMetadata>, DbError> {
        // Get the track
        let track = match TrackQuery::get_by_id(&self.db, track_id)? {
            Some(t) => t,
            None => return Ok(None),
        };

        // Load associated data
        let cue_points = CuePointQuery::get_for_track(&self.db, track_id)?;
        let saved_loops = SavedLoopQuery::get_for_track(&self.db, track_id)?;
        let stem_links = StemLinkQuery::get_for_track(&self.db, track_id)?;

        Ok(Some(LoadedTrackMetadata {
            track,
            cue_points,
            saved_loops,
            stem_links,
        }))
    }

    /// Get cue points for a track
    pub fn get_cue_points(&self, track_id: i64) -> Result<Vec<CuePoint>, DbError> {
        CuePointQuery::get_for_track(&self.db, track_id)
    }

    /// Get saved loops for a track
    pub fn get_saved_loops(&self, track_id: i64) -> Result<Vec<SavedLoop>, DbError> {
        SavedLoopQuery::get_for_track(&self.db, track_id)
    }

    /// Get stem links for a track
    pub fn get_stem_links(&self, track_id: i64) -> Result<Vec<StemLink>, DbError> {
        StemLinkQuery::get_for_track(&self.db, track_id)
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
    pub fn get_playlist_tracks(&self, playlist_id: i64) -> Result<Vec<Track>, DbError> {
        PlaylistQuery::get_tracks(&self.db, playlist_id)
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
    pub fn find_similar_tracks(&self, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        SimilarityQuery::find_similar(&self.db, track_id, limit)
    }

    // ========================================================================
    // Low-level Access (for advanced usage)
    // ========================================================================

    /// Get the underlying MeshDb for advanced queries
    ///
    /// Use this sparingly - prefer domain methods above.
    pub fn db(&self) -> &MeshDb {
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
    fn test_service_add_and_get_track() {
        let temp = TempDir::new().unwrap();
        let tracks_dir = temp.path().join("tracks");
        std::fs::create_dir_all(&tracks_dir).unwrap();

        // Create a dummy file
        let track_path = tracks_dir.join("test.wav");
        std::fs::write(&track_path, b"dummy").unwrap();

        let service = DatabaseService::new(temp.path()).unwrap();

        let track_data = NewTrackData {
            path: track_path.clone(),
            name: "Test Track".to_string(),
            artist: Some("Artist".to_string()),
            bpm: Some(120.0),
            original_bpm: Some(120.0),
            key: Some("Am".to_string()),
            duration_seconds: 180.0,
            lufs: Some(-14.0),
            first_beat_sample: 0,
        };

        let track_id = service.add_track(&track_data).unwrap();
        assert!(track_id != 0);

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

                    let track_data = NewTrackData {
                        path: track_path,
                        name: format!("Track {}", i),
                        artist: None,
                        bpm: Some(120.0),
                        original_bpm: Some(120.0),
                        key: None,
                        duration_seconds: 180.0,
                        lufs: None,
                        first_beat_sample: 0,
                    };

                    service.add_track(&track_data).unwrap()
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
}
