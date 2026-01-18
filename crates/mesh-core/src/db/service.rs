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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::schema::{Track, Playlist, AudioFeatures};
use super::queries::{TrackQuery, PlaylistQuery, SimilarityQuery};
use super::migration::{NewTrackData, insert_analyzed_track};
use super::{MeshDb, DbError};

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
        let track_id = insert_analyzed_track(&self.db, &self.collection_root, track_data)?;
        log::debug!("Added track to database: id={}, name={}", track_id, track_data.name);
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
