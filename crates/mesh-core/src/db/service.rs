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
//! if let Some(track) = db.get_track_by_path("/path/to/track.flac")? {
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

use super::batch::BatchQuery;
use super::queries::{TrackQuery, PlaylistQuery, SimilarityQuery, CuePointQuery, SavedLoopQuery, StemLinkQuery, HistoryQuery};
use super::schema::{TrackRow, Playlist, CuePoint, SavedLoop, StemLink, TrackPlayRecord, TrackPlayUpdate};
use super::{MeshDb, DbError};
use cozo::DataValue;
use std::collections::{BTreeMap, HashMap};

// ============================================================================
// ML Scores - Lightweight Struct for Suggestion Scoring
// ============================================================================

/// Lightweight ML scores for suggestion scoring (subset of MlAnalysisData)
#[derive(Debug, Clone, Default)]
pub struct MlScores {
    /// Danceability probability (0.0 = not danceable, 1.0 = very danceable)
    pub danceability: Option<f32>,
    /// Music approachability regression score (0.0–1.0)
    pub approachability: Option<f32>,
    /// Timbre brightness probability (0.0 = dark, 1.0 = bright)
    pub timbre: Option<f32>,
    /// Tonality probability (0.0 = atonal, 1.0 = tonal)
    pub tonal: Option<f32>,
    /// Acoustic sound probability (0.0 = non-acoustic, 1.0 = acoustic)
    pub mood_acoustic: Option<f32>,
    /// Electronic sound probability (0.0 = non-electronic, 1.0 = electronic)
    pub mood_electronic: Option<f32>,
    /// Primary genre label
    pub top_genre: Option<String>,
}

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
/// let track = db.get_track_by_path("/path/to/track.flac")?;
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
    /// Track title (parsed from filename or embedded tags)
    pub title: String,
    /// Original filename before metadata parsing (for re-analysis)
    pub original_name: String,
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
    /// Drop LUFS loudness (top 10% of short-term windows, used for gain staging)
    pub lufs: Option<f32>,
    /// Integrated LUFS loudness (whole-track EBU R128 average, stored for future use)
    pub integrated_lufs: Option<f32>,
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
    pub fn new(path: impl Into<PathBuf>, title: impl Into<String>) -> Self {
        Self {
            id: None,
            path: path.into(),
            folder_path: String::new(),
            title: title.into(),
            original_name: String::new(),
            artist: None,
            bpm: None,
            original_bpm: None,
            key: None,
            duration_seconds: 0.0,
            lufs: None,
            integrated_lufs: None,
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
            title: row.title,
            original_name: row.original_name,
            artist: row.artist,
            bpm: row.bpm,
            original_bpm: row.original_bpm,
            key: row.key,
            duration_seconds: row.duration_seconds,
            lufs: row.lufs,
            integrated_lufs: row.integrated_lufs,
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
        let id = self.id.unwrap_or_else(|| {
            // Hash the path RELATIVE to the collection root so the same track
            // gets the same ID regardless of where the USB stick is mounted.
            let rel = self.path.strip_prefix(collection_root).unwrap_or(&self.path);
            generate_track_id(rel)
        });
        let folder_path = if self.folder_path.is_empty() {
            extract_folder_path(&self.path, collection_root)
        } else {
            self.folder_path.clone()
        };

        TrackRow {
            id,
            path: self.path.to_string_lossy().to_string(),
            folder_path,
            title: self.title.clone(),
            original_name: self.original_name.clone(),
            artist: self.artist.clone(),
            bpm: self.bpm,
            original_bpm: self.original_bpm,
            key: self.key.clone(),
            duration_seconds: self.duration_seconds,
            lufs: self.lufs,
            integrated_lufs: self.integrated_lufs,
            drop_marker: self.drop_marker,
            first_beat_sample: self.first_beat_sample,
            file_mtime: self.file_mtime,
            file_size: self.file_size,
            waveform_path: self.waveform_path.clone(),
        }
    }

    /// Get formatted display name: "Artist - Title" or just "Title"
    pub fn display_name(&self) -> String {
        match &self.artist {
            Some(a) if !a.is_empty() => format!("{} - {}", a, self.title),
            _ => self.title.clone(),
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

    /// Run a read-only CozoScript query (crate-internal delegation to MeshDb)
    pub(crate) fn run_query(&self, script: &str, params: BTreeMap<String, DataValue>) -> Result<cozo::NamedRows, DbError> {
        self.db.run_query(script, params)
    }

    /// Compute the stable (relative-path) track ID for a given absolute path.
    ///
    /// This is the same algorithm used by `to_row()` when `Track.id` is None.
    /// Use this to predict/verify IDs without inserting a track.
    pub fn compute_stable_track_id(&self, absolute_path: &Path) -> i64 {
        let rel = absolute_path.strip_prefix(&self.collection_root).unwrap_or(absolute_path);
        generate_track_id(rel)
    }

    /// List every track as (id, absolute_path) — for migration and diagnostics.
    pub fn get_all_track_ids_and_paths(&self) -> Result<Vec<(i64, PathBuf)>, DbError> {
        let result = self.db.run_query(
            "?[id, path] := *tracks{id, path} :order id",
            BTreeMap::new(),
        )?;
        Ok(result.rows.iter().filter_map(|row| {
            let id = row.get(0)?.get_int()?;
            let path = row.get(1)?.get_str().map(PathBuf::from)?;
            Some((id, path))
        }).collect())
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

    /// Find a track by matching the filename portion of its path
    ///
    /// Useful when absolute paths differ (e.g., USB mounted at different paths)
    /// but the filename is the same. Returns the first match.
    pub fn find_track_by_filename(&self, filename: &str) -> Result<Option<Track>, DbError> {
        let row = TrackQuery::find_by_filename(&self.db, filename)?;
        match row {
            Some(r) => Ok(Some(Track::from_row_only(r))),
            None => Ok(None),
        }
    }

    /// Save a track with all its metadata
    ///
    /// This will insert or update the track and all associated metadata
    /// (cue_points, saved_loops, stem_links). Returns the track ID.
    ///
    /// # Example
    /// ```ignore
    /// let mut track = Track::new("/path/to/track.flac", "My Track");
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
            "DatabaseService::save_track: title='{}' path='{}' id={}",
            track.title,
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

    /// Atomically sync a track using batch inserts (optimized for USB export)
    ///
    /// This method is optimized for bulk operations like USB export where many tracks
    /// need to be synced. It uses batch inserts instead of individual queries:
    ///
    /// - 1 query: Upsert track row
    /// - 3 queries: Delete old metadata (cue_points, saved_loops, stem_links)
    /// - 3 queries: Batch insert new metadata
    ///
    /// Total: ~7 queries instead of 18+ with individual inserts.
    ///
    /// # Arguments
    /// * `track` - The track to sync (from source database)
    /// * `source_db` - The source database (for resolving stem link paths)
    ///
    /// # Stem Link Remapping
    /// Stem links reference tracks by ID, but IDs differ between databases.
    /// This method remaps stem link source_track_id values by:
    /// 1. Looking up the source track's filename in source_db
    /// 2. Finding the matching track in this (USB) database by filename
    /// 3. Using the USB database's track ID for the stem link
    pub fn sync_track_atomic(
        &self,
        track: &Track,
        source_db: &DatabaseService,
        source_track_id: i64,
    ) -> Result<i64, DbError> {
        // Convert track to row and get the track ID
        let row = track.to_row(&self.collection_root);
        let track_id = row.id;

        log::debug!(
            "sync_track_atomic: title='{}' path='{}' id={} source_id={}",
            track.title,
            track.path.display(),
            track_id,
            source_track_id
        );

        // 1. Upsert track row
        TrackQuery::upsert(&self.db, &row)?;

        // 2. Delete all existing metadata for this track
        BatchQuery::batch_delete_track_metadata(&self.db, track_id)?;

        // 3. Remap stem links to USB database IDs
        let remapped_links = self.remap_stem_links_for_export(&track.stem_links, source_db)?;

        // 4. Batch insert all metadata
        BatchQuery::batch_insert_cue_points(&self.db, track_id, &track.cue_points)?;
        BatchQuery::batch_insert_saved_loops(&self.db, track_id, &track.saved_loops)?;
        BatchQuery::batch_insert_stem_links(&self.db, track_id, &remapped_links)?;

        // 5. Sync ML analysis data + intensity components
        if let Ok(Some(ml_data)) = source_db.get_ml_analysis(source_track_id) {
            let _ = self.store_ml_analysis(track_id, &ml_data);
        }
        if let Ok(components) = source_db.batch_get_intensity_components(&[source_track_id]) {
            if let Some(ic) = components.get(&source_track_id) {
                let _ = self.store_intensity_components(track_id, ic);
            }
        }

        // 6. Sync tags (batch insert — single query instead of N)
        if let Ok(tags) = source_db.get_tags(source_track_id) {
            if let Err(e) = BatchQuery::batch_insert_tags(&self.db, track_id, &tags) {
                log::warn!("sync_track_atomic: Failed to batch insert tags for track {}: {}", track_id, e);
            }
        }

        // 7. Sync EffNet ML embedding
        if let Ok(Some(emb)) = source_db.get_ml_embedding_raw(source_track_id) {
            let _ = self.store_ml_embedding(track_id, &emb);
        }

        // 9. Sync stem energy densities
        if let Ok(Some((v, d, b, o))) = source_db.get_stem_energy(source_track_id) {
            let _ = self.store_stem_energy(track_id, v, d, b, o);
        }

        // 10. Sync PCA embedding (128-dim library-tuned similarity index)
        if let Ok(Some(pca_emb)) = source_db.get_pca_embedding_raw(source_track_id) {
            let _ = self.store_pca_embedding(track_id, &pca_emb);
        }

        log::debug!("sync_track_atomic: SUCCESS id={}", track_id);
        Ok(track_id)
    }

    /// Remap stem links from source database IDs to this database's IDs
    ///
    /// For USB export, stem links need to reference tracks in the USB database,
    /// not the local database. This method finds the matching track by filename.
    fn remap_stem_links_for_export(
        &self,
        links: &[StemLink],
        source_db: &DatabaseService,
    ) -> Result<Vec<StemLink>, DbError> {
        let mut remapped = Vec::with_capacity(links.len());

        for link in links {
            // Get source track from local DB to find its filename
            if let Some(source_track) = source_db.get_track(link.source_track_id)? {
                // Extract filename from source path
                let filename = source_track
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                // Find corresponding track in USB DB by path pattern
                // USB tracks are at: {collection_root}/tracks/{filename}
                let usb_path = self.collection_root.join("tracks").join(filename);
                let usb_path_str = usb_path.to_string_lossy();

                if let Some(usb_track) = self.get_track_by_path(&usb_path_str)? {
                    if let Some(usb_id) = usb_track.id {
                        remapped.push(StemLink {
                            track_id: 0, // Will be set by batch_insert
                            stem_index: link.stem_index,
                            source_track_id: usb_id, // USB DB ID
                            source_stem: link.source_stem,
                        });
                    }
                } else {
                    log::warn!(
                        "remap_stem_links_for_export: target '{}' not found on USB, skipping stem link",
                        filename
                    );
                }
            }
        }

        Ok(remapped)
    }

    /// Delete a track and all associated metadata
    pub fn delete_track(&self, id: i64) -> Result<(), DbError> {
        // Delete all associated metadata (cue points, saved loops, stem links,
        // ml_analysis, track_tags, audio_features)
        BatchQuery::batch_delete_track_metadata(&self.db, id)?;

        // Delete the track row itself
        TrackQuery::delete(&self.db, id)
    }

    /// Delete multiple tracks and all their associated metadata in batch
    pub fn delete_tracks_batch(&self, ids: &[i64]) -> Result<(), DbError> {
        for &id in ids {
            BatchQuery::batch_delete_track_metadata(&self.db, id)?;
        }
        // Batch delete all track rows in a single query
        if !ids.is_empty() {
            let rows: Vec<DataValue> = ids.iter()
                .map(|&id| DataValue::List(vec![DataValue::from(id)]))
                .collect();
            let mut params = BTreeMap::new();
            params.insert("rows".to_string(), DataValue::List(rows));
            self.db.run_script(r#"
                ?[id] <- $rows
                :rm tracks {id}
            "#, params)?;
        }
        Ok(())
    }

    /// Remove multiple tracks from a playlist by their database IDs
    pub fn remove_tracks_from_playlist_batch(&self, playlist_id: i64, track_ids: &[i64]) -> Result<(), DbError> {
        PlaylistQuery::remove_tracks_batch(&self.db, playlist_id, track_ids)
    }

    /// Get all tracks in the database
    ///
    /// Returns tracks with basic metadata only (no cue_points/saved_loops/stem_links).
    pub fn get_all_tracks(&self) -> Result<Vec<Track>, DbError> {
        let rows = TrackQuery::get_all(&self.db)?;
        Ok(rows.into_iter().map(Track::from_row_only).collect())
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

    /// Get all distinct artist names from the collection
    ///
    /// Returns only non-null artist strings. Used by the metadata module
    /// for known-artist disambiguation during filename parsing.
    pub fn get_distinct_artists(&self) -> Result<Vec<String>, DbError> {
        let result = self.db.run_query(r#"
            ?[artist] := *tracks{artist}, is_not_null(artist)
        "#, BTreeMap::new())?;

        Ok(result.rows.into_iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str().map(|s| s.to_string())))
            .collect())
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
    /// Uses batch query to fetch all source tracks in a single database round-trip,
    /// avoiding N+1 query pattern when tracks have multiple stem links.
    ///
    /// Used by both mesh-cue and mesh-player to populate TrackMetadata.stem_links.
    pub fn convert_stem_links_to_runtime(&self, db_links: &[StemLink]) -> Vec<crate::audio_file::StemLinkReference> {
        use std::collections::HashMap;

        if db_links.is_empty() {
            return Vec::new();
        }

        // Collect all source track IDs for batch lookup
        let source_ids: Vec<i64> = db_links.iter()
            .map(|link| link.source_track_id)
            .collect();

        // Single batch query instead of N individual queries
        let source_tracks = match TrackQuery::get_by_ids(&self.db, &source_ids) {
            Ok(tracks) => tracks,
            Err(e) => {
                log::warn!("Failed to batch load stem link source tracks: {:?}", e);
                return Vec::new();
            }
        };

        // Build lookup map by track ID
        let track_map: HashMap<i64, &TrackRow> = source_tracks.iter()
            .map(|t| (t.id, t))
            .collect();

        // Convert links using the map
        db_links.iter().filter_map(|link| {
            match track_map.get(&link.source_track_id) {
                Some(source_track) => {
                    Some(crate::audio_file::StemLinkReference {
                        stem_index: link.stem_index,
                        source_path: std::path::PathBuf::from(&source_track.path),
                        source_stem: link.source_stem,
                        source_drop_marker: source_track.drop_marker.unwrap_or(0) as u64,
                    })
                }
                None => {
                    log::warn!("Stem link source track not found: id={}", link.source_track_id);
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

    /// Resolve a NodeId path like "playlists/Parent/Child" to a playlist DB ID
    ///
    /// Walks the path segments, resolving each against its parent, supporting
    /// arbitrary nesting depth.
    pub fn resolve_playlist_path(&self, node_path: &str) -> Result<Option<i64>, DbError> {
        let playlist_path = match node_path.strip_prefix("playlists/") {
            Some(p) => p,
            None => return Ok(None),
        };
        let segments: Vec<&str> = playlist_path.split('/').collect();
        let mut parent_id: Option<i64> = None;
        for segment in &segments {
            match PlaylistQuery::get_by_name(&self.db, segment, parent_id)? {
                Some(playlist) => parent_id = Some(playlist.id),
                None => return Ok(None),
            }
        }
        Ok(parent_id)
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

    /// Get all playlist memberships as a map from track_id → list of playlist names.
    ///
    /// Single query covering all playlists; intended for batch reverse-lookup
    /// when attaching playlist pills to suggestion results.
    pub fn get_all_playlist_memberships(&self) -> Result<HashMap<i64, Vec<String>>, DbError> {
        PlaylistQuery::get_all_memberships(&self.db)
    }

    /// Add a track to a playlist
    pub fn add_track_to_playlist(&self, playlist_id: i64, track_id: i64, sort_order: i32) -> Result<(), DbError> {
        PlaylistQuery::add_track(&self.db, playlist_id, track_id, sort_order)
    }

    /// Add multiple tracks to a playlist in a single batch query
    pub fn add_tracks_to_playlist_batch(&self, playlist_id: i64, track_ids: &[(i64, i32)]) -> Result<(), DbError> {
        PlaylistQuery::add_tracks_batch(&self.db, playlist_id, track_ids)
    }

    /// Resolve multiple file paths to track database IDs in a single query.
    ///
    /// Returns a map from path → track_id. Paths not found in the database are omitted.
    pub fn get_track_ids_by_paths(&self, paths: &[&str]) -> Result<std::collections::HashMap<String, i64>, DbError> {
        use std::collections::HashMap;

        if paths.is_empty() {
            return Ok(HashMap::new());
        }

        let path_values: Vec<DataValue> = paths.iter().map(|p| DataValue::Str((*p).into())).collect();
        let mut params = BTreeMap::new();
        params.insert("paths".to_string(), DataValue::List(path_values));

        let result = self.db.run_query(r#"
            ?[id, path] := *tracks{id, path}, path in $paths
        "#, params)?;

        let mut map = HashMap::new();
        for row in &result.rows {
            let id = row[0].get_int().unwrap_or(0);
            if let Some(path) = row[1].get_str() {
                map.insert(path.to_string(), id);
            }
        }
        Ok(map)
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
    // ML Embeddings & Similarity
    // ========================================================================

    // ── EffNet ML embedding ─────────────────────────────────────────────────

    /// Store a 1280-dim EffNet embedding for similarity search.
    pub fn store_ml_embedding(&self, track_id: i64, embedding: &[f32]) -> Result<(), DbError> {
        SimilarityQuery::upsert_ml_embedding(&self.db, track_id, embedding)
    }

    /// Retrieve the raw EffNet embedding for a track (None if not yet analysed).
    pub fn get_ml_embedding_raw(&self, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
        SimilarityQuery::get_ml_embedding_raw(&self.db, track_id)
    }

    /// Find similar tracks via EffNet HNSW (same DB, by track ID).
    pub fn find_similar_tracks_ml(&self, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let results = SimilarityQuery::find_similar_by_ml_id(&self.db, track_id, limit)?;
        Ok(results.into_iter().map(|(row, score)| (Track::from_row_only(row), score)).collect())
    }

    /// Find similar tracks via EffNet HNSW using a raw vector (cross-database).
    pub fn find_similar_by_ml_vector(&self, query_vec: &[f64], limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let results = SimilarityQuery::find_similar_by_ml_vector(&self.db, query_vec, limit)?;
        Ok(results.into_iter().map(|(row, score)| (Track::from_row_only(row), score)).collect())
    }

    // ── Stem energy densities ───────────────────────────────────────────────

    /// Store per-stem RMS energy densities `(vocal, drums, bass, other)`.
    pub fn store_stem_energy(&self, track_id: i64, vocal: f32, drums: f32, bass: f32, other: f32) -> Result<(), DbError> {
        SimilarityQuery::upsert_stem_energy(&self.db, track_id, vocal, drums, bass, other)
    }

    /// Get per-stem energy densities for a single track.
    pub fn get_stem_energy(&self, track_id: i64) -> Result<Option<(f32, f32, f32, f32)>, DbError> {
        SimilarityQuery::get_stem_energy(&self.db, track_id)
    }

    /// Batch-fetch stem energy for multiple tracks (avoids N+1 in scoring loops).
    pub fn batch_get_stem_energy(&self, track_ids: &[i64]) -> Result<HashMap<i64, (f32, f32, f32, f32)>, DbError> {
        SimilarityQuery::batch_get_stem_energy(&self.db, track_ids)
    }

    // ── PCA 128-dim embeddings ───────────────────────────────────────────────

    /// Store a 128-dim PCA-projected embedding (built by "Build Similarity Index").
    pub fn store_pca_embedding(&self, track_id: i64, embedding: &[f32]) -> Result<(), DbError> {
        SimilarityQuery::upsert_pca_embedding(&self.db, track_id, embedding)
    }

    /// Retrieve the raw 128-dim PCA embedding (None if not yet built).
    pub fn get_pca_embedding_raw(&self, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
        SimilarityQuery::get_pca_embedding_raw(&self.db, track_id)
    }

    /// Find similar tracks via PCA HNSW (same DB, by track ID).
    pub fn find_similar_tracks_pca(&self, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let results = SimilarityQuery::find_similar_by_pca_id(&self.db, track_id, limit)?;
        Ok(results.into_iter().map(|(row, score)| (Track::from_row_only(row), score)).collect())
    }

    /// Find similar tracks via PCA HNSW using a raw 128-dim vector (cross-database).
    pub fn find_similar_by_pca_vector(&self, query_vec: &[f64], limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let results = SimilarityQuery::find_similar_by_pca_vector(&self.db, query_vec, limit)?;
        Ok(results.into_iter().map(|(row, score)| (Track::from_row_only(row), score)).collect())
    }

    /// Scan all PCA embeddings with track metadata (brute-force graph view scoring).
    pub fn get_all_pca_with_tracks(&self) -> Result<Vec<(Track, Vec<f32>)>, DbError> {
        let results = SimilarityQuery::get_all_pca_with_tracks(&self.db)?;
        Ok(results.into_iter().map(|(row, vec)| (Track::from_row_only(row), vec)).collect())
    }

    /// Scan all 1280-dim EffNet embeddings — input for PCA build.
    pub fn get_all_ml_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>, DbError> {
        SimilarityQuery::get_all_ml_embeddings(&self.db)
    }

    /// Get all track IDs in the database (lightweight, no metadata).
    pub fn get_all_track_ids(&self) -> Result<Vec<i64>, DbError> {
        let result = self.db.run_query(
            "?[id] := *tracks{id} :order id",
            BTreeMap::new(),
        )?;
        Ok(result.rows.iter().filter_map(|row| {
            row.get(0)?.get_int()
        }).collect())
    }

    /// Build the full graph edge set for the suggestion graph view.
    ///
    /// For each track, queries the `k` nearest neighbors via HNSW (PCA > ML > 16-dim
    /// fallback), then deduplicates bidirectional edges and annotates with co-play data.
    pub fn build_graph_edges(&self, k: usize) -> Result<Vec<crate::suggestions::GraphEdge>, DbError> {
        use rayon::prelude::*;
        use std::collections::HashSet;

        let all_ids = self.get_all_track_ids()?;
        if all_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Parallel HNSW queries — collect raw directed edges
        let raw_edges: Vec<(i64, i64, f32)> = all_ids
            .par_iter()
            .flat_map(|&track_id| {
                let neighbors = self.find_similar_tracks_pca(track_id, k)
                    .or_else(|_| self.find_similar_tracks_ml(track_id, k))
                    .unwrap_or_default();

                neighbors.into_iter().filter_map(move |(track, dist)| {
                    track.id.map(|to_id| (track_id, to_id, dist))
                }).collect::<Vec<_>>()
            })
            .collect();

        // Deduplicate bidirectional edges: keep (min_id, max_id), best distance
        let mut edge_map: HashMap<(i64, i64), f32> = HashMap::new();
        for (from, to, dist) in raw_edges {
            if from == to { continue; }
            let key = if from < to { (from, to) } else { (to, from) };
            edge_map.entry(key)
                .and_modify(|d| { if dist < *d { *d = dist; } })
                .or_insert(dist);
        }

        // Build played_after pair data for edge annotation
        let mut pa_pairs: HashSet<(i64, i64)> = HashSet::new();
        let mut pa_weights: HashMap<(i64, i64), f32> = HashMap::new();
        for &track_id in &all_ids {
            if let Ok(neighbors) = self.get_played_after_neighbors(track_id, 100) {
                for (to_id, weight) in neighbors {
                    let key = if track_id < to_id { (track_id, to_id) } else { (to_id, track_id) };
                    pa_pairs.insert(key);
                    pa_weights.entry(key)
                        .and_modify(|w| *w = w.max(weight))
                        .or_insert(weight);
                }
            }
        }

        // Build final edge list
        let edges: Vec<crate::suggestions::GraphEdge> = edge_map
            .into_iter()
            .map(|((from_id, to_id), hnsw_distance)| {
                let key = (from_id, to_id);
                let is_played_after = pa_pairs.contains(&key);
                let played_after_weight = pa_weights.get(&key).copied().unwrap_or(0.0);
                crate::suggestions::GraphEdge {
                    from_id,
                    to_id,
                    hnsw_distance,
                    is_played_after,
                    played_after_weight,
                }
            })
            .collect();

        log::info!(
            "[GRAPH] Built {} edges from {} tracks (k={})",
            edges.len(), all_ids.len(), k
        );
        Ok(edges)
    }

    // ── Transition graph (played_after) ─────────────────────────────────────

    /// Rebuild the played_after transition graph from all co-play records.
    /// Called from "Analyse Library" in mesh-cue. Returns edge count written.
    pub fn build_played_after_graph(&self) -> Result<usize, DbError> {
        HistoryQuery::build_played_after_graph(&self.db)
    }

    /// Time-decayed co-play neighbors for a seed track (count ≥ 5 threshold).
    pub fn get_played_after_neighbors(&self, track_id: i64, limit: usize) -> Result<Vec<(i64, f32)>, DbError> {
        HistoryQuery::get_played_after_neighbors(&self.db, track_id, limit)
    }

    /// Batch time-decayed co-play neighbors for multiple seeds.
    /// Returns map of to_track_id → max weight across all seeds.
    pub fn batch_get_played_after_neighbors(&self, ids: &[i64], limit_per_seed: usize) -> Result<HashMap<i64, f32>, DbError> {
        HistoryQuery::batch_get_played_after_neighbors(&self.db, ids, limit_per_seed)
    }

    // ── Opener candidates ────────────────────────────────────────────────────

    /// All tracks that have a drop marker set — used for on-the-fly opener scoring.
    pub fn get_tracks_with_drop_marker(&self) -> Result<Vec<Track>, DbError> {
        let rows = TrackQuery::get_with_drop_marker(&self.db)?;
        Ok(rows.into_iter().map(Track::from_row_only).collect())
    }

    /// Store intensity components for a track.
    pub fn store_intensity_components(&self, track_id: i64, ic: &crate::db::schema::IntensityComponents) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("spectral_flux".to_string(), DataValue::from(ic.spectral_flux as f64));
        params.insert("flatness".to_string(), DataValue::from(ic.flatness as f64));
        params.insert("spectral_centroid".to_string(), DataValue::from(ic.spectral_centroid as f64));
        params.insert("dissonance".to_string(), DataValue::from(ic.dissonance as f64));
        params.insert("crest_factor".to_string(), DataValue::from(ic.crest_factor as f64));
        params.insert("energy_variance".to_string(), DataValue::from(ic.energy_variance as f64));
        params.insert("harmonic_complexity".to_string(), DataValue::from(ic.harmonic_complexity as f64));
        params.insert("spectral_rolloff".to_string(), DataValue::from(ic.spectral_rolloff as f64));
        params.insert("centroid_variance".to_string(), DataValue::from(ic.centroid_variance as f64));
        params.insert("flux_variance".to_string(), DataValue::from(ic.flux_variance as f64));
        self.db.run_script(r#"
            ?[track_id, spectral_flux, flatness, spectral_centroid, dissonance, crest_factor,
              energy_variance, harmonic_complexity, spectral_rolloff, centroid_variance, flux_variance] <-
                [[$track_id, $spectral_flux, $flatness, $spectral_centroid, $dissonance, $crest_factor,
                  $energy_variance, $harmonic_complexity, $spectral_rolloff, $centroid_variance, $flux_variance]]
            :put track_intensity {
                track_id =>
                spectral_flux, flatness, spectral_centroid, dissonance,
                crest_factor, energy_variance, harmonic_complexity, spectral_rolloff,
                centroid_variance, flux_variance
            }
        "#, params)?;
        Ok(())
    }

    /// Batch-fetch intensity components for multiple tracks.
    pub fn batch_get_intensity_components(&self, track_ids: &[i64]) -> Result<HashMap<i64, crate::db::schema::IntensityComponents>, DbError> {
        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let id_values: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(id_values));
        let result = self.db.run_query(r#"
            ?[track_id, spectral_flux, flatness, spectral_centroid, dissonance, crest_factor,
              energy_variance, harmonic_complexity, spectral_rolloff, centroid_variance, flux_variance] :=
                *track_intensity{track_id, spectral_flux, flatness, spectral_centroid, dissonance, crest_factor,
                                 energy_variance, harmonic_complexity, spectral_rolloff, centroid_variance, flux_variance},
                track_id in $ids
        "#, params)?;
        let mut map = HashMap::new();
        for row in &result.rows {
            if let Some(tid) = row[0].get_int() {
                map.insert(tid, crate::db::schema::IntensityComponents {
                    spectral_flux: row[1].get_float().unwrap_or(0.0) as f32,
                    flatness: row[2].get_float().unwrap_or(0.0) as f32,
                    spectral_centroid: row[3].get_float().unwrap_or(0.0) as f32,
                    dissonance: row[4].get_float().unwrap_or(0.0) as f32,
                    crest_factor: row[5].get_float().unwrap_or(0.0) as f32,
                    energy_variance: row[6].get_float().unwrap_or(0.0) as f32,
                    harmonic_complexity: row[7].get_float().unwrap_or(0.0) as f32,
                    spectral_rolloff: row[8].get_float().unwrap_or(0.0) as f32,
                    centroid_variance: row[9].get_float().unwrap_or(0.0) as f32,
                    flux_variance: row[10].get_float().unwrap_or(0.0) as f32,
                });
            }
        }
        Ok(map)
    }

    // ========================================================================
    // PCA Aggression Axis
    // ========================================================================

    /// Store the PCA aggression weight vector (one weight per PCA dimension).
    pub fn store_aggression_weights(&self, weights: &[f32], correlation: f32) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(0i64));
        params.insert("weights".to_string(), DataValue::List(
            weights.iter().map(|&w| DataValue::from(w as f64)).collect(),
        ));
        params.insert("correlation".to_string(), DataValue::from(correlation as f64));
        self.db.run_script(r#"
            ?[id, weights, correlation] <- [[$id, $weights, $correlation]]
            :put pca_aggression_axis {id => weights, correlation}
        "#, params)?;
        Ok(())
    }

    /// Get the PCA aggression weight vector and combined correlation.
    /// Returns None if no weights have been computed yet.
    pub fn get_aggression_weights(&self) -> Result<Option<(Vec<f32>, f32)>, DbError> {
        let result = self.db.run_query(r#"
            ?[weights, correlation] := *pca_aggression_axis{id: 0, weights, correlation}
        "#, BTreeMap::new())?;
        Ok(result.rows.first().and_then(|row| {
            let weights = match &row[0] {
                DataValue::List(items) => {
                    items.iter().filter_map(|v| v.get_float().map(|f| f as f32)).collect::<Vec<_>>()
                }
                _ => return None,
            };
            if weights.is_empty() { return None; }
            let corr = row[1].get_float()? as f32;
            Some((weights, corr))
        }))
    }

    // ========================================================================
    // Aggression Calibration Pairs
    // ========================================================================

    /// Store a calibration pair (user pairwise comparison).
    /// Returns the auto-incremented id of the new pair.
    pub fn store_calibration_pair(&self, track_a: i64, track_b: i64, choice: i32) -> Result<i64, DbError> {
        // Find next ID: collect all IDs, take max + 1 (or 1 if empty)
        let max_result = self.db.run_query(r#"
            ?[id] := *aggression_calibration_pairs{id}
        "#, BTreeMap::new())?;
        let next_id = max_result.rows.iter()
            .filter_map(|row| row[0].get_int())
            .max()
            .unwrap_or(0) + 1;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(next_id));
        params.insert("track_a".to_string(), DataValue::from(track_a));
        params.insert("track_b".to_string(), DataValue::from(track_b));
        params.insert("choice".to_string(), DataValue::from(choice as i64));
        params.insert("timestamp".to_string(), DataValue::from(timestamp));
        self.db.run_script(r#"
            ?[id, track_a, track_b, choice, timestamp] <- [[$id, $track_a, $track_b, $choice, $timestamp]]
            :put aggression_calibration_pairs {id => track_a, track_b, choice, timestamp}
        "#, params)?;
        Ok(next_id)
    }

    /// Get all calibration pairs. Returns (id, track_a, track_b, choice, timestamp).
    pub fn get_all_calibration_pairs(&self) -> Result<Vec<(i64, i64, i64, i32, i64)>, DbError> {
        let result = self.db.run_query(r#"
            ?[id, track_a, track_b, choice, timestamp] :=
                *aggression_calibration_pairs{id, track_a, track_b, choice, timestamp}
            :order id
        "#, BTreeMap::new())?;
        Ok(result.rows.iter().filter_map(|row| {
            Some((
                row[0].get_int()?,
                row[1].get_int()?,
                row[2].get_int()?,
                row[3].get_int()? as i32,
                row[4].get_int()?,
            ))
        }).collect())
    }

    /// Get the number of stored calibration pairs.
    pub fn get_calibration_pair_count(&self) -> Result<usize, DbError> {
        let result = self.db.run_query(r#"
            ids[id] := *aggression_calibration_pairs{id}
            ?[count(id)] := ids[id]
        "#, BTreeMap::new())?;
        Ok(result.rows.first()
            .and_then(|row| row[0].get_int())
            .unwrap_or(0) as usize)
    }

    /// Delete the most recently added calibration pair (for undo).
    pub fn delete_last_calibration_pair(&self) -> Result<(), DbError> {
        self.db.run_script(r#"
            max_id[id] := id = max(i), *aggression_calibration_pairs{id: i}
            ?[id] := max_id[id]
            :rm aggression_calibration_pairs {id}
        "#, BTreeMap::new())?;
        Ok(())
    }

    /// Delete all calibration pairs (reset calibration data).
    pub fn clear_calibration_pairs(&self) -> Result<(), DbError> {
        self.db.run_script(r#"
            ?[id] := *aggression_calibration_pairs{id}
            :rm aggression_calibration_pairs {id}
        "#, BTreeMap::new())?;
        Ok(())
    }

    // ========================================================================
    // Graph Position Cache
    // ========================================================================

    /// Get cached graph positions for the given cache key.
    /// Returns None if no cache exists for this key.
    pub fn get_graph_positions(&self, cache_key: &str) -> Result<Option<std::collections::HashMap<i64, (f32, f32)>>, DbError> {
        use std::collections::HashMap;
        let mut params = BTreeMap::new();
        params.insert("key".to_string(), DataValue::Str(cache_key.into()));

        let result = self.db.run_query(r#"
            ?[track_id, x, y] := *graph_positions{cache_key, track_id, x, y}, cache_key = $key
        "#, params)?;

        if result.rows.is_empty() {
            return Ok(None);
        }

        let mut positions = HashMap::new();
        for row in &result.rows {
            if let (Some(id), Some(x), Some(y)) = (row[0].get_int(), row[1].get_float(), row[2].get_float()) {
                positions.insert(id, (x as f32, y as f32));
            }
        }
        Ok(Some(positions))
    }

    /// Store graph positions for the given cache key.
    /// Clears any existing positions (all cache keys) before storing.
    pub fn store_graph_positions(&self, cache_key: &str, positions: &std::collections::HashMap<i64, (f32, f32)>) -> Result<(), DbError> {
        // Clear all existing cached positions
        self.db.run_script(r#"
            ?[cache_key, track_id] := *graph_positions{cache_key, track_id}
            :rm graph_positions {cache_key, track_id}
        "#, BTreeMap::new())?;

        // Store new positions in batches
        let batch_size = 500;
        let entries: Vec<_> = positions.iter().collect();
        for chunk in entries.chunks(batch_size) {
            let rows: Vec<String> = chunk.iter()
                .map(|(&id, &(x, y))| format!("[\"{}\", {}, {}, {}]", cache_key, id, x as f64, y as f64))
                .collect();
            let rows_str = rows.join(", ");
            let script = format!(
                "?[cache_key, track_id, x, y] <- [{}] :put graph_positions {{cache_key, track_id => x, y}}",
                rows_str
            );
            self.db.run_script(&script, BTreeMap::new())?;
        }
        Ok(())
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
    // Tag Operations
    // ========================================================================

    /// Get all tags for a single track
    pub fn get_tags(&self, track_id: i64) -> Result<Vec<(String, Option<String>)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("tid".to_string(), DataValue::from(track_id));

        let result = self.db.run_query(r#"
            ?[label, color, sort_order] := *track_tags{track_id: $tid, label, color, sort_order}
            :order sort_order, label
        "#, params)?;

        Ok(result.rows.iter().map(|row| {
            let label = row[0].get_str().unwrap_or("").to_string();
            let color = row[1].get_str().map(|s| s.to_string());
            (label, color)
        }).collect())
    }

    /// Get tags for multiple tracks in one query (avoids N+1)
    pub fn get_tags_batch(&self, track_ids: &[i64]) -> Result<std::collections::HashMap<i64, Vec<(String, Option<String>)>>, DbError> {
        use std::collections::HashMap;

        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let id_values: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(id_values));

        let result = self.db.run_query(r#"
            ?[track_id, label, color, sort_order] := *track_tags{track_id, label, color, sort_order},
                                                     track_id in $ids
            :order track_id, sort_order, label
        "#, params)?;

        let mut map: HashMap<i64, Vec<(String, Option<String>)>> = HashMap::new();
        for row in &result.rows {
            let tid = row[0].get_int().unwrap_or(0);
            let label = row[1].get_str().unwrap_or("").to_string();
            let color = row[2].get_str().map(|s| s.to_string());
            map.entry(tid).or_default().push((label, color));
        }
        Ok(map)
    }

    /// Get hot cue counts for multiple tracks in one query (avoids N+1)
    pub fn get_cue_counts_batch(&self, track_ids: &[i64]) -> Result<std::collections::HashMap<i64, u8>, DbError> {
        use std::collections::HashMap;

        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let id_values: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(id_values));

        let result = self.db.run_query(r#"
            ?[track_id, count(index)] := *cue_points{track_id, index}, track_id in $ids
        "#, params)?;

        let mut map = HashMap::new();
        for row in &result.rows {
            let tid = row[0].get_int().unwrap_or(0);
            let count = row[1].get_int().unwrap_or(0) as u8;
            if count > 0 {
                map.insert(tid, count);
            }
        }
        Ok(map)
    }

    /// Add a tag to a track (upsert — if label exists, updates color)
    pub fn add_tag(&self, track_id: i64, label: &str, color: Option<&str>) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("tid".to_string(), DataValue::from(track_id));
        params.insert("label".to_string(), DataValue::Str(label.into()));
        params.insert("color".to_string(), match color {
            Some(c) => DataValue::Str(c.into()),
            None => DataValue::Null,
        });

        self.db.run_script(r#"
            ?[track_id, label, color] <- [[$tid, $label, $color]]
            :put track_tags {track_id, label => color}
        "#, params)?;

        Ok(())
    }

    /// Remove a tag from a track
    pub fn remove_tag(&self, track_id: i64, label: &str) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("tid".to_string(), DataValue::from(track_id));
        params.insert("label".to_string(), DataValue::Str(label.into()));

        self.db.run_script(r#"
            ?[track_id, label] <- [[$tid, $label]]
            :rm track_tags {track_id, label}
        "#, params)?;

        Ok(())
    }

    /// Get all unique tag labels in the database (for autocomplete/filter UI)
    pub fn get_all_tags(&self) -> Result<Vec<(String, Option<String>)>, DbError> {
        let result = self.db.run_query(r#"
            ?[label, color] := *track_tags{label, color}
        "#, BTreeMap::new())?;

        Ok(result.rows.iter().map(|row| {
            let label = row[0].get_str().unwrap_or("").to_string();
            let color = row[1].get_str().map(|s| s.to_string());
            (label, color)
        }).collect())
    }

    /// Find all tracks that have ALL of the given tags (AND query)
    pub fn find_tracks_by_tags_all(&self, tags: &[&str]) -> Result<Vec<i64>, DbError> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }

        // Build Datalog rules: each tag constrains the same tid variable
        let tag_rules: Vec<String> = tags.iter().enumerate().map(|(i, _)| {
            format!("*track_tags{{track_id: tid, label: $tag{}}}", i)
        }).collect();

        let script = format!(
            "?[tid] := {}\n:order tid",
            tag_rules.join(",\n           ")
        );

        let mut params = BTreeMap::new();
        for (i, tag) in tags.iter().enumerate() {
            params.insert(format!("tag{}", i), DataValue::Str((*tag).into()));
        }

        let result = self.db.run_query(&script, params)?;
        Ok(result.rows.iter().filter_map(|row| row[0].get_int()).collect())
    }

    /// Find all tracks that have ANY of the given tags (OR query)
    ///
    /// Returns (track_id, matching_tag_count) sorted by match count descending.
    pub fn find_tracks_by_tags_any(&self, tags: &[&str]) -> Result<Vec<(i64, usize)>, DbError> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }

        let tag_values: Vec<DataValue> = tags.iter().map(|&t| DataValue::Str(t.into())).collect();
        let mut params = BTreeMap::new();
        params.insert("tags".to_string(), DataValue::List(tag_values));

        let result = self.db.run_query(r#"
            ?[tid, count(label)] := *track_tags{track_id: tid, label},
                                     label in $tags
            :order -count(label), tid
        "#, params)?;

        Ok(result.rows.iter().filter_map(|row| {
            let tid = row[0].get_int()?;
            let count = row[1].get_int()? as usize;
            Some((tid, count))
        }).collect())
    }

    // ========================================================================
    // ML Analysis Operations
    // ========================================================================

    /// Store ML analysis result for a track (upsert)
    pub fn store_ml_analysis(&self, track_id: i64, data: &super::schema::MlAnalysisData) -> Result<(), DbError> {
        let genre_scores_json = serde_json::to_string(&data.genre_scores)
            .unwrap_or_else(|_| "[]".to_string());
        let mood_scores_json = data.mood_themes.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "[]".to_string()));
        let binary_moods_json = data.binary_moods.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "[]".to_string()));

        let opt_f32 = |v: Option<f32>| -> DataValue {
            v.map(|f| DataValue::from(f as f64)).unwrap_or(DataValue::Null)
        };

        let mut params = BTreeMap::new();
        params.insert("tid".to_string(), DataValue::from(track_id));
        params.insert("vocal_presence".to_string(), DataValue::from(data.vocal_presence as f64));
        params.insert("arousal".to_string(), opt_f32(data.arousal));
        params.insert("valence".to_string(), opt_f32(data.valence));
        params.insert("top_genre".to_string(), data.top_genre.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("genre_scores_json".to_string(), DataValue::Str(genre_scores_json.into()));
        params.insert("mood_scores_json".to_string(), mood_scores_json.map(|s| DataValue::Str(s.into())).unwrap_or(DataValue::Null));
        params.insert("binary_moods_json".to_string(), binary_moods_json.map(|s| DataValue::Str(s.into())).unwrap_or(DataValue::Null));
        params.insert("danceability".to_string(), opt_f32(data.danceability));
        params.insert("approachability".to_string(), opt_f32(data.approachability));
        params.insert("reverb".to_string(), opt_f32(data.reverb));
        params.insert("timbre".to_string(), opt_f32(data.timbre));
        params.insert("tonal".to_string(), opt_f32(data.tonal));
        params.insert("mood_acoustic".to_string(), opt_f32(data.mood_acoustic));
        params.insert("mood_electronic".to_string(), opt_f32(data.mood_electronic));

        self.db.run_script(r#"
            ?[track_id, vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
              danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic] <- [[
                $tid, $vocal_presence, $arousal, $valence, $top_genre, $genre_scores_json, $mood_scores_json, $binary_moods_json,
                $danceability, $approachability, $reverb, $timbre, $tonal, $mood_acoustic, $mood_electronic
            ]]
            :put ml_analysis {track_id => vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
                              danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic}
        "#, params)?;

        Ok(())
    }

    /// Get ML analysis data for a track
    pub fn get_ml_analysis(&self, track_id: i64) -> Result<Option<super::schema::MlAnalysisData>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("tid".to_string(), DataValue::from(track_id));

        let result = self.db.run_query(r#"
            ?[vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
              danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic] :=
                *ml_analysis{track_id: $tid, vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
                             danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic}
        "#, params)?;

        if let Some(row) = result.rows.first() {
            let vocal_presence = row[0].get_float().unwrap_or(0.0) as f32;
            let arousal = row[1].get_float().map(|f| f as f32);
            let valence = row[2].get_float().map(|f| f as f32);
            let top_genre = row[3].get_str().map(|s| s.to_string());
            let genre_scores: Vec<(String, f32)> = row[4].get_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let mood_themes: Option<Vec<(String, f32)>> = row[5].get_str()
                .and_then(|s| serde_json::from_str(s).ok());
            let binary_moods: Option<Vec<(String, f32)>> = row[6].get_str()
                .and_then(|s| serde_json::from_str(s).ok());
            let danceability = row[7].get_float().map(|f| f as f32);
            let approachability = row[8].get_float().map(|f| f as f32);
            let reverb = row[9].get_float().map(|f| f as f32);
            let timbre = row[10].get_float().map(|f| f as f32);
            let tonal = row[11].get_float().map(|f| f as f32);
            let mood_acoustic = row[12].get_float().map(|f| f as f32);
            let mood_electronic = row[13].get_float().map(|f| f as f32);

            Ok(Some(super::schema::MlAnalysisData {
                vocal_presence,
                arousal,
                valence,
                top_genre,
                genre_scores,
                mood_themes,
                binary_moods,
                danceability,
                approachability,
                reverb,
                timbre,
                tonal,
                mood_acoustic,
                mood_electronic,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get all ML analysis data for all tracks (bulk query for sync)
    pub fn get_all_ml_analysis(&self) -> Result<std::collections::HashMap<i64, super::schema::MlAnalysisData>, DbError> {
        use std::collections::HashMap;

        let result = self.db.run_query(r#"
            ?[track_id, vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
              danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic] :=
                *ml_analysis{track_id, vocal_presence, arousal, valence, top_genre, genre_scores_json, mood_scores_json, binary_moods_json,
                             danceability, approachability, reverb, timbre, tonal, mood_acoustic, mood_electronic}
        "#, BTreeMap::new())?;

        let mut map = HashMap::new();
        for row in &result.rows {
            let tid = match row[0].get_int() {
                Some(id) => id,
                None => continue,
            };
            let vocal_presence = row[1].get_float().unwrap_or(0.0) as f32;
            let arousal = row[2].get_float().map(|f| f as f32);
            let valence = row[3].get_float().map(|f| f as f32);
            let top_genre = row[4].get_str().map(|s| s.to_string());
            let genre_scores: Vec<(String, f32)> = row[5].get_str()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let mood_themes: Option<Vec<(String, f32)>> = row[6].get_str()
                .and_then(|s| serde_json::from_str(s).ok());
            let binary_moods: Option<Vec<(String, f32)>> = row[7].get_str()
                .and_then(|s| serde_json::from_str(s).ok());
            let danceability = row[8].get_float().map(|f| f as f32);
            let approachability = row[9].get_float().map(|f| f as f32);
            let reverb = row[10].get_float().map(|f| f as f32);
            let timbre = row[11].get_float().map(|f| f as f32);
            let tonal = row[12].get_float().map(|f| f as f32);
            let mood_acoustic = row[13].get_float().map(|f| f as f32);
            let mood_electronic = row[14].get_float().map(|f| f as f32);

            map.insert(tid, super::schema::MlAnalysisData {
                vocal_presence,
                arousal,
                valence,
                top_genre,
                genre_scores,
                mood_themes,
                binary_moods,
                danceability,
                approachability,
                reverb,
                timbre,
                tonal,
                mood_acoustic,
                mood_electronic,
            });
        }
        Ok(map)
    }

    /// Get all tags for all tracks (bulk query for sync)
    pub fn get_all_track_tags(&self) -> Result<std::collections::HashMap<i64, Vec<(String, Option<String>)>>, DbError> {
        use std::collections::HashMap;

        let result = self.db.run_query(r#"
            ?[track_id, label, color, sort_order] := *track_tags{track_id, label, color, sort_order}
            :order track_id, sort_order, label
        "#, BTreeMap::new())?;

        let mut map: HashMap<i64, Vec<(String, Option<String>)>> = HashMap::new();
        for row in &result.rows {
            let tid = match row[0].get_int() {
                Some(id) => id,
                None => continue,
            };
            let label = row[1].get_str().unwrap_or("").to_string();
            let color = row[2].get_str().map(|s| s.to_string());
            map.entry(tid).or_default().push((label, color));
        }
        Ok(map)
    }

    /// Batch-fetch arousal values for multiple tracks (for suggestion scoring)
    pub fn get_arousal_batch(&self, track_ids: &[i64]) -> Result<std::collections::HashMap<i64, f32>, DbError> {
        use std::collections::HashMap;

        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let id_values: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(id_values));

        let result = self.db.run_query(r#"
            ?[track_id, arousal] := *ml_analysis{track_id, arousal},
                                    track_id in $ids,
                                    is_not_null(arousal)
        "#, params)?;

        let mut map = HashMap::new();
        for row in &result.rows {
            if let (Some(tid), Some(arousal)) = (row[0].get_int(), row[1].get_float()) {
                map.insert(tid, arousal as f32);
            }
        }
        Ok(map)
    }

    /// Batch-fetch ML scores for suggestion scoring
    pub fn get_ml_scores_batch(&self, track_ids: &[i64]) -> Result<std::collections::HashMap<i64, MlScores>, DbError> {
        use std::collections::HashMap;

        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let id_values: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(id_values));

        let result = self.db.run_query(r#"
            ?[track_id, danceability, approachability, timbre, tonal,
              mood_acoustic, mood_electronic, top_genre] :=
                *ml_analysis{track_id, danceability, approachability, timbre, tonal,
                             mood_acoustic, mood_electronic, top_genre},
                track_id in $ids
        "#, params)?;

        let mut map = HashMap::new();
        for row in &result.rows {
            if let Some(tid) = row[0].get_int() {
                map.insert(tid, MlScores {
                    danceability: row[1].get_float().map(|f| f as f32),
                    approachability: row[2].get_float().map(|f| f as f32),
                    timbre: row[3].get_float().map(|f| f as f32),
                    tonal: row[4].get_float().map(|f| f as f32),
                    mood_acoustic: row[5].get_float().map(|f| f as f32),
                    mood_electronic: row[6].get_float().map(|f| f as f32),
                    top_genre: row[7].get_str().map(|s| s.to_string()),
                });
            }
        }
        Ok(map)
    }

    // ========================================================================
    // Session History
    // ========================================================================

    /// Create a new DJ session record
    pub fn create_session(&self, id: i64) -> Result<(), DbError> {
        HistoryQuery::insert_session(&self.db, id)
    }

    /// Mark a session as ended
    pub fn end_session(&self, id: i64, ended_at: i64) -> Result<(), DbError> {
        HistoryQuery::end_session(&self.db, id, ended_at)
    }

    /// Insert a track play record (load-time fields; play fields start null)
    pub fn insert_track_play(&self, record: &TrackPlayRecord) -> Result<(), DbError> {
        HistoryQuery::insert_track_play(&self.db, record)
    }

    /// Update play_started fields when the DJ first presses play
    pub fn update_play_started(
        &self,
        session_id: i64,
        loaded_at: i64,
        play_started_at: i64,
        play_start_sample: i64,
        played_with_json: Option<String>,
    ) -> Result<(), DbError> {
        HistoryQuery::update_play_started(&self.db, session_id, loaded_at, play_started_at, play_start_sample, played_with_json)
    }

    /// Update played_with_json on an existing track play (bidirectional co-play)
    pub fn update_played_with(&self, session_id: i64, loaded_at: i64, played_with_json: Option<String>) -> Result<(), DbError> {
        HistoryQuery::update_played_with(&self.db, session_id, loaded_at, played_with_json)
    }

    /// Finalize a track play when the track is replaced or session ends
    pub fn finalize_track_play(&self, session_id: i64, loaded_at: i64, update: &TrackPlayUpdate) -> Result<(), DbError> {
        HistoryQuery::finalize_track_play(&self.db, session_id, loaded_at, update)
    }

    /// Get all track paths played in a session (for suggestion filtering)
    pub fn get_session_played_paths(&self, session_id: i64) -> Result<std::collections::HashSet<String>, DbError> {
        HistoryQuery::get_session_played_paths(&self.db, session_id)
    }

    // ========================================================================
    // Low-level Access (for advanced usage within mesh-core)
    // ========================================================================

    /// Get the underlying MeshDb for advanced queries
    ///
    /// Prefer the typed methods above for normal usage.
    /// This is for diagnostics and advanced queries.
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
    fn test_service_save_and_get_track() {
        let temp = TempDir::new().unwrap();
        let tracks_dir = temp.path().join("tracks");
        std::fs::create_dir_all(&tracks_dir).unwrap();

        // Create a dummy file
        let track_path = tracks_dir.join("test.flac");
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
        assert_eq!(loaded.title, "Test Track");
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
                    let track_path = tracks_dir.join(format!("track_{}.flac", i));
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

        let track_path = tracks_dir.join("test.flac");
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
