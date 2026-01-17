//! Migration utilities for importing WAV collection into CozoDB
//!
//! This module provides functions to migrate existing WAV file collections
//! into the CozoDB database, reading metadata from WAV file chunks and
//! populating the tracks, cue_points, and saved_loops relations.

use super::{MeshDb, DbError};
use crate::audio_file::read_metadata;
use cozo::DataValue;
use rayon::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use walkdir::WalkDir;

/// Progress callback for migration operations
pub type ProgressCallback = Box<dyn Fn(MigrationProgress) + Send + Sync>;

/// Migration progress information
#[derive(Debug, Clone)]
pub struct MigrationProgress {
    /// Current track being processed
    pub current: usize,
    /// Total tracks to process
    pub total: usize,
    /// Path of current track (if available)
    pub current_path: Option<PathBuf>,
    /// Phase of migration
    pub phase: MigrationPhase,
}

/// Phases of the migration process
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationPhase {
    /// Scanning filesystem for WAV files
    Scanning,
    /// Reading metadata from WAV files
    ReadingMetadata,
    /// Inserting into database
    Inserting,
    /// Migration complete
    Complete,
}

/// Result of a migration operation
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Number of tracks successfully migrated
    pub tracks_migrated: usize,
    /// Number of tracks that failed to migrate
    pub tracks_failed: usize,
    /// Paths of failed tracks with error messages
    pub failures: Vec<(PathBuf, String)>,
}

/// Generate a unique track ID from the file path
///
/// Uses a hash of the path to ensure consistency across migrations.
/// This allows us to detect existing tracks and update rather than duplicate.
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

/// Extract track name from path (filename without extension)
fn extract_track_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

/// Migrate a WAV file collection into the CozoDB database
///
/// This function:
/// 1. Scans the tracks/ directory for all WAV files
/// 2. Reads metadata from each file in parallel
/// 3. Batch inserts into the database
///
/// # Arguments
///
/// * `db` - The MeshDb instance to populate
/// * `collection_root` - Root path of the collection (contains tracks/ subdirectory)
/// * `progress` - Optional progress callback
///
/// # Returns
///
/// Result containing migration statistics
pub fn migrate_from_wav_collection(
    db: &MeshDb,
    collection_root: &Path,
    progress: Option<ProgressCallback>,
) -> Result<MigrationResult, DbError> {
    let tracks_dir = collection_root.join("tracks");

    if !tracks_dir.exists() {
        return Err(DbError::Migration(format!(
            "Tracks directory does not exist: {}",
            tracks_dir.display()
        )));
    }

    // Phase 1: Scan for WAV files
    if let Some(ref cb) = progress {
        cb(MigrationProgress {
            current: 0,
            total: 0,
            current_path: None,
            phase: MigrationPhase::Scanning,
        });
    }

    let wav_files: Vec<PathBuf> = WalkDir::new(&tracks_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("wav"))
                .unwrap_or(false)
        })
        .map(|e| e.path().to_owned())
        .collect();

    let total = wav_files.len();
    if total == 0 {
        return Ok(MigrationResult {
            tracks_migrated: 0,
            tracks_failed: 0,
            failures: vec![],
        });
    }

    // Phase 2: Read metadata in parallel
    let processed = Arc::new(AtomicUsize::new(0));
    let progress_arc = progress.map(Arc::new);

    let track_data: Vec<Result<TrackData, (PathBuf, String)>> = wav_files
        .par_iter()
        .map(|path| {
            let current = processed.fetch_add(1, Ordering::Relaxed);

            if let Some(ref cb) = progress_arc {
                cb(MigrationProgress {
                    current,
                    total,
                    current_path: Some(path.clone()),
                    phase: MigrationPhase::ReadingMetadata,
                });
            }

            match read_wav_track_data(path, &tracks_dir) {
                Ok(data) => Ok(data),
                Err(e) => Err((path.clone(), e)),
            }
        })
        .collect();

    // Separate successes and failures
    let mut successes = Vec::with_capacity(total);
    let mut failures = Vec::new();

    for result in track_data {
        match result {
            Ok(data) => successes.push(data),
            Err((path, err)) => failures.push((path, err)),
        }
    }

    // Phase 3: Batch insert into database
    if let Some(ref cb) = progress_arc {
        cb(MigrationProgress {
            current: 0,
            total: successes.len(),
            current_path: None,
            phase: MigrationPhase::Inserting,
        });
    }

    batch_insert_tracks(db, &successes)?;

    // Complete
    if let Some(ref cb) = progress_arc {
        cb(MigrationProgress {
            current: successes.len(),
            total: successes.len(),
            current_path: None,
            phase: MigrationPhase::Complete,
        });
    }

    Ok(MigrationResult {
        tracks_migrated: successes.len(),
        tracks_failed: failures.len(),
        failures,
    })
}

/// Internal struct to hold all data for a track during migration
struct TrackData {
    id: i64,
    path: String,
    folder_path: String,
    name: String,
    artist: Option<String>,
    bpm: Option<f64>,
    original_bpm: Option<f64>,
    key: Option<String>,
    duration_seconds: f64,
    lufs: Option<f32>,
    drop_marker: Option<i64>,
    file_mtime: i64,
    file_size: i64,
    cue_points: Vec<CuePointData>,
    saved_loops: Vec<SavedLoopData>,
}

struct CuePointData {
    track_id: i64,
    index: u8,
    sample_position: i64,
    label: Option<String>,
    color: Option<String>,
}

struct SavedLoopData {
    track_id: i64,
    index: u8,
    start_sample: i64,
    end_sample: i64,
    label: Option<String>,
    color: Option<String>,
}

/// Read track data from a WAV file
fn read_wav_track_data(path: &Path, collection_root: &Path) -> Result<TrackData, String> {
    // Get file metadata
    let file_meta = std::fs::metadata(path)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;

    let file_size = file_meta.len() as i64;
    let file_mtime = file_meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Read WAV metadata
    let metadata = read_metadata(path)
        .map_err(|e| format!("Failed to read WAV metadata: {}", e))?;

    let track_id = generate_track_id(path);
    let folder_path = extract_folder_path(path, collection_root);
    let name = extract_track_name(path);

    // Convert cue points
    let cue_points: Vec<CuePointData> = metadata
        .cue_points
        .iter()
        .map(|cp| CuePointData {
            track_id,
            index: cp.index,
            sample_position: cp.sample_position as i64,
            label: if cp.label.is_empty() { None } else { Some(cp.label.clone()) },
            color: cp.color.clone(),
        })
        .collect();

    // Convert saved loops
    let saved_loops: Vec<SavedLoopData> = metadata
        .saved_loops
        .iter()
        .map(|sl| SavedLoopData {
            track_id,
            index: sl.index,
            start_sample: sl.start_sample as i64,
            end_sample: sl.end_sample as i64,
            label: if sl.label.is_empty() { None } else { Some(sl.label.clone()) },
            color: sl.color.clone(),
        })
        .collect();

    Ok(TrackData {
        id: track_id,
        path: path.to_string_lossy().to_string(),
        folder_path,
        name,
        artist: metadata.artist,
        bpm: metadata.bpm,
        original_bpm: metadata.original_bpm,
        key: metadata.key,
        duration_seconds: metadata.duration_seconds.unwrap_or(0.0),
        lufs: metadata.lufs,
        drop_marker: metadata.drop_marker.map(|d| d as i64),
        file_mtime,
        file_size,
        cue_points,
        saved_loops,
    })
}

/// Batch insert tracks into the database
fn batch_insert_tracks(db: &MeshDb, tracks: &[TrackData]) -> Result<(), DbError> {
    if tracks.is_empty() {
        return Ok(());
    }

    // Build track rows
    let track_rows: Vec<Vec<DataValue>> = tracks
        .iter()
        .map(|t| {
            vec![
                DataValue::from(t.id),
                DataValue::from(t.path.clone()),
                DataValue::from(t.folder_path.clone()),
                DataValue::from(t.name.clone()),
                t.artist.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                t.bpm.map(DataValue::from).unwrap_or(DataValue::Null),
                t.original_bpm.map(DataValue::from).unwrap_or(DataValue::Null),
                t.key.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                DataValue::from(t.duration_seconds),
                t.lufs.map(|l| DataValue::from(l as f64)).unwrap_or(DataValue::Null),
                t.drop_marker.map(DataValue::from).unwrap_or(DataValue::Null),
                DataValue::from(t.file_mtime),
                DataValue::from(t.file_size),
                DataValue::Null, // waveform_path - populated later during analysis
            ]
        })
        .collect();

    // Insert tracks using :put (upsert)
    let mut params = std::collections::BTreeMap::new();
    params.insert("tracks".to_string(), DataValue::List(
        track_rows.into_iter().map(DataValue::List).collect()
    ));

    db.run_script(
        r#"
        ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
          duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] <- $tracks
        :put tracks {
            id =>
            path, folder_path, name, artist, bpm, original_bpm, key,
            duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path
        }
        "#,
        params,
    )?;

    // Build and insert cue points
    let cue_rows: Vec<Vec<DataValue>> = tracks
        .iter()
        .flat_map(|t| {
            t.cue_points.iter().map(|cp| {
                vec![
                    DataValue::from(cp.track_id),
                    DataValue::from(cp.index as i64),
                    DataValue::from(cp.sample_position),
                    cp.label.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                    cp.color.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                ]
            })
        })
        .collect();

    if !cue_rows.is_empty() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("cues".to_string(), DataValue::List(
            cue_rows.into_iter().map(DataValue::List).collect()
        ));

        db.run_script(
            r#"
            ?[track_id, index, sample_position, label, color] <- $cues
            :put cue_points { track_id, index => sample_position, label, color }
            "#,
            params,
        )?;
    }

    // Build and insert saved loops
    let loop_rows: Vec<Vec<DataValue>> = tracks
        .iter()
        .flat_map(|t| {
            t.saved_loops.iter().map(|sl| {
                vec![
                    DataValue::from(sl.track_id),
                    DataValue::from(sl.index as i64),
                    DataValue::from(sl.start_sample),
                    DataValue::from(sl.end_sample),
                    sl.label.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                    sl.color.clone().map(DataValue::from).unwrap_or(DataValue::Null),
                ]
            })
        })
        .collect();

    if !loop_rows.is_empty() {
        let mut params = std::collections::BTreeMap::new();
        params.insert("loops".to_string(), DataValue::List(
            loop_rows.into_iter().map(DataValue::List).collect()
        ));

        db.run_script(
            r#"
            ?[track_id, index, start_sample, end_sample, label, color] <- $loops
            :put saved_loops { track_id, index => start_sample, end_sample, label, color }
            "#,
            params,
        )?;
    }

    Ok(())
}

/// Check if a track needs updating based on file modification time
pub fn track_needs_update(db: &MeshDb, path: &Path) -> Result<bool, DbError> {
    let track_id = generate_track_id(path);

    let file_mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut params = std::collections::BTreeMap::new();
    params.insert("id".to_string(), DataValue::from(track_id));

    let result = db.run_query(
        r#"
        ?[file_mtime] := *tracks{id: $id, file_mtime}
        "#,
        params,
    )?;

    if result.rows.is_empty() {
        // Track doesn't exist, needs update
        return Ok(true);
    }

    // Check if file has been modified
    if let Some(row) = result.rows.first() {
        if let DataValue::Num(cozo::Num::Int(db_mtime)) = &row[0] {
            return Ok(file_mtime > *db_mtime);
        }
    }

    Ok(false)
}

/// Migrate a single track (useful for incremental updates)
pub fn migrate_single_track(
    db: &MeshDb,
    path: &Path,
    collection_root: &Path,
) -> Result<(), DbError> {
    let data = read_wav_track_data(path, collection_root)
        .map_err(|e| DbError::Migration(e))?;

    batch_insert_tracks(db, &[data])
}

/// Parameters for inserting a newly analyzed track into the database.
///
/// This is used during import to store analysis results directly in the database
/// without needing to read back from a WAV file.
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
}

/// Insert a newly analyzed track into the database.
///
/// This is called during the import process after audio analysis completes,
/// allowing the database to be populated immediately rather than waiting
/// for a migration step.
///
/// # Arguments
///
/// * `db` - The database connection
/// * `collection_root` - Root path of the collection (for calculating folder_path)
/// * `track_data` - The analyzed track data to insert
///
/// # Returns
///
/// The generated track ID, or an error if insertion fails.
pub fn insert_analyzed_track(
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
    let mut params = std::collections::BTreeMap::new();
    params.insert("id".to_string(), DataValue::from(track_id));
    params.insert("path".to_string(), DataValue::from(track_data.path.to_string_lossy().to_string()));
    params.insert("folder_path".to_string(), DataValue::from(folder_path));
    params.insert("name".to_string(), DataValue::from(track_data.name.clone()));
    params.insert("artist".to_string(), track_data.artist.clone().map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("bpm".to_string(), track_data.bpm.map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("original_bpm".to_string(), track_data.original_bpm.map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("key".to_string(), track_data.key.clone().map(DataValue::from).unwrap_or(DataValue::Null));
    params.insert("duration_seconds".to_string(), DataValue::from(track_data.duration_seconds));
    params.insert("lufs".to_string(), track_data.lufs.map(|l| DataValue::from(l as f64)).unwrap_or(DataValue::Null));
    params.insert("drop_marker".to_string(), DataValue::Null);
    params.insert("file_mtime".to_string(), DataValue::from(file_mtime));
    params.insert("file_size".to_string(), DataValue::from(file_size));
    params.insert("waveform_path".to_string(), DataValue::Null);

    db.run_script(
        r#"
        ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
          duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] <- [[
            $id, $path, $folder_path, $name, $artist, $bpm, $original_bpm, $key,
            $duration_seconds, $lufs, $drop_marker, $file_mtime, $file_size, $waveform_path
        ]]
        :put tracks {
            id =>
            path, folder_path, name, artist, bpm, original_bpm, key,
            duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path
        }
        "#,
        params,
    )?;

    Ok(track_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_generate_track_id_consistency() {
        let path = Path::new("/some/path/to/track.wav");
        let id1 = generate_track_id(path);
        let id2 = generate_track_id(path);
        assert_eq!(id1, id2, "Same path should generate same ID");
    }

    #[test]
    fn test_extract_folder_path() {
        let root = Path::new("/collection");

        // Track directly in tracks folder → folder_path = "tracks"
        let path = Path::new("/collection/tracks/track.wav");
        let folder = extract_folder_path(path, root);
        assert_eq!(folder, "tracks");

        // Track in subfolder → folder_path = "tracks/subfolder"
        let path = Path::new("/collection/tracks/subfolder/track.wav");
        let folder = extract_folder_path(path, root);
        assert_eq!(folder, "tracks/subfolder");
    }

    #[test]
    fn test_extract_track_name() {
        let path = Path::new("/some/path/Artist - Song Title.wav");
        let name = extract_track_name(path);
        assert_eq!(name, "Artist - Song Title");
    }

    #[test]
    fn test_migration_empty_directory() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("tracks")).unwrap();

        let db = MeshDb::in_memory().unwrap();
        let result = migrate_from_wav_collection(&db, temp.path(), None).unwrap();

        assert_eq!(result.tracks_migrated, 0);
        assert_eq!(result.tracks_failed, 0);
    }

    #[test]
    fn test_migration_no_tracks_dir() {
        let temp = TempDir::new().unwrap();
        let db = MeshDb::in_memory().unwrap();

        let result = migrate_from_wav_collection(&db, temp.path(), None);
        assert!(result.is_err());
    }
}
