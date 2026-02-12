//! Query builders and helpers for CozoDB
//!
//! This module provides typed query APIs that generate CozoScript internally.

use super::schema::{TrackRow, Playlist, AudioFeatures, CuePoint, SavedLoop, StemLink};
use super::{MeshDb, DbError};
use cozo::{DataValue, NamedRows};
use std::collections::BTreeMap;

/// Column list for track queries (must match schema order for first_beat_sample)
const TRACK_COLUMNS: &str = "id, path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path";

// ============================================================================
// Track Queries
// ============================================================================

/// Query builder for tracks
pub struct TrackQuery;

impl TrackQuery {
    /// Get all tracks in a folder
    pub fn get_by_folder(db: &MeshDb, folder_path: &str) -> Result<Vec<TrackRow>, DbError> {
        log::debug!("TrackQuery::get_by_folder: querying folder_path='{}'", folder_path);

        let mut params = BTreeMap::new();
        params.insert("folder".to_string(), DataValue::Str(folder_path.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                folder_path = $folder
            :order name
        "#, params)?;

        let tracks = rows_to_tracks(&result);
        log::debug!("TrackQuery::get_by_folder: found {} tracks for folder_path='{}'", tracks.len(), folder_path);
        Ok(tracks)
    }

    /// Get a track by ID
    pub fn get_by_id(db: &MeshDb, track_id: i64) -> Result<Option<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                id = $id
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Get multiple tracks by IDs in a single query (batch lookup)
    ///
    /// More efficient than calling get_by_id in a loop when fetching multiple tracks.
    pub fn get_by_ids(db: &MeshDb, track_ids: &[i64]) -> Result<Vec<TrackRow>, DbError> {
        if track_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "ids".to_string(),
            DataValue::List(track_ids.iter().map(|&id| DataValue::from(id)).collect()),
        );

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                id in $ids
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Get a track by path
    pub fn get_by_path(db: &MeshDb, path: &str) -> Result<Option<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("path".to_string(), DataValue::Str(path.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                path = $path
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Get all tracks in the database
    pub fn get_all(db: &MeshDb) -> Result<Vec<TrackRow>, DbError> {
        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order name
        "#, BTreeMap::new())?;

        Ok(rows_to_tracks(&result))
    }

    /// Search tracks by name or artist
    pub fn search(db: &MeshDb, query: &str, limit: usize) -> Result<Vec<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        let query_lower = query.to_lowercase();
        params.insert("query".to_string(), DataValue::Str(query_lower.into()));
        params.insert("limit".to_string(), DataValue::from(limit as i64));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                (lowercase(name) ~ $query or
                 (is_not_null(artist) and lowercase(artist) ~ $query))
            :limit $limit
            :order name
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Insert or update a track
    pub fn upsert(db: &MeshDb, track: &TrackRow) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track.id));
        params.insert("path".to_string(), DataValue::Str(track.path.clone().into()));
        params.insert("folder_path".to_string(), DataValue::Str(track.folder_path.clone().into()));
        params.insert("name".to_string(), DataValue::Str(track.name.clone().into()));
        params.insert("artist".to_string(), track.artist.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("bpm".to_string(), track.bpm.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("original_bpm".to_string(), track.original_bpm.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("key".to_string(), track.key.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("duration_seconds".to_string(), DataValue::from(track.duration_seconds));
        params.insert("lufs".to_string(), track.lufs.map(|v| DataValue::from(v as f64)).unwrap_or(DataValue::Null));
        params.insert("drop_marker".to_string(), track.drop_marker.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("first_beat_sample".to_string(), DataValue::from(track.first_beat_sample));
        params.insert("file_mtime".to_string(), DataValue::from(track.file_mtime));
        params.insert("file_size".to_string(), DataValue::from(track.file_size));
        params.insert("waveform_path".to_string(), track.waveform_path.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
                $id, $path, $folder_path, $name, $artist, $bpm, $original_bpm, $key,
                $duration_seconds, $lufs, $drop_marker, $first_beat_sample, $file_mtime, $file_size, $waveform_path
            ]]
            :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                         duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
        "#, params)?;

        Ok(())
    }

    /// Delete a track by ID
    pub fn delete(db: &MeshDb, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track_id));

        db.run_script(r#"
            ?[id] <- [[$id]]
            :rm tracks {id}
        "#, params)?;

        Ok(())
    }

    /// Update a single field of a track by ID
    ///
    /// Supported fields: artist, bpm, original_bpm, key, lufs, drop_marker, first_beat_sample
    /// For string fields, pass the string value directly.
    /// For numeric fields, parse to the appropriate type first.
    pub fn update_field(db: &MeshDb, track_id: i64, field: &str, value: &str) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track_id));

        // Parse value based on field type and build the appropriate update query
        let query = match field {
            "artist" => {
                let val = if value.is_empty() { DataValue::Null } else { DataValue::Str(value.into()) };
                params.insert("value".to_string(), val);
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, bpm, original_bpm, key,
                                duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        artist = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid BPM value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, original_bpm, key,
                                duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        bpm = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "original_bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid original_bpm value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, key,
                                duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        original_bpm = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "key" => {
                let val = if value.is_empty() { DataValue::Null } else { DataValue::Str(value.into()) };
                params.insert("value".to_string(), val);
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm,
                                duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        key = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "lufs" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid LUFS value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                                duration_seconds, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        lufs = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "drop_marker" => {
                let val: i64 = value.parse().map_err(|_| DbError::Query(format!("Invalid drop_marker value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        drop_marker = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "first_beat_sample" => {
                let val: i64 = value.parse().map_err(|_| DbError::Query(format!("Invalid first_beat_sample value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        first_beat_sample = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            _ => {
                return Err(DbError::Query(format!("Unknown or immutable field: {}", field)));
            }
        };

        db.run_script(query, params)?;
        Ok(())
    }

    /// Update a track by path (convenience wrapper for cases where we have path but not ID)
    pub fn update_field_by_path(db: &MeshDb, path: &str, field: &str, value: &str) -> Result<(), DbError> {
        // First find the track ID by path
        let track = Self::get_by_path(db, path)?
            .ok_or_else(|| DbError::Query(format!("Track not found: {}", path)))?;

        Self::update_field(db, track.id, field, value)
    }

    /// Get all unique folder paths
    pub fn get_folders(db: &MeshDb) -> Result<Vec<String>, DbError> {
        log::debug!("TrackQuery::get_folders: querying all folders");

        let result = db.run_query(r#"
            ?[folder_path] := *tracks{folder_path}
            :order folder_path
        "#, BTreeMap::new())?;

        let folders: Vec<String> = result.rows.into_iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str().map(|s| s.to_string())))
            .collect();

        log::debug!("TrackQuery::get_folders: found {} folders: {:?}", folders.len(), folders);
        Ok(folders)
    }

    /// Count tracks in the database
    pub fn count(db: &MeshDb) -> Result<usize, DbError> {
        log::debug!("TrackQuery::count: counting all tracks");

        let result = db.run_query(r#"
            ?[count(id)] := *tracks{id}
        "#, BTreeMap::new())?;

        let count = result.rows.first()
            .and_then(|row| row.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) as usize;

        log::info!("TrackQuery::count: total tracks = {}", count);
        Ok(count)
    }
}

// ============================================================================
// Playlist Queries
// ============================================================================

/// Query builder for playlists
pub struct PlaylistQuery;

impl PlaylistQuery {
    /// Get all playlists
    pub fn get_all(db: &MeshDb) -> Result<Vec<Playlist>, DbError> {
        let result = db.run_query(r#"
            ?[id, parent_id, name, sort_order] := *playlists{id, parent_id, name, sort_order}
            :order sort_order, name
        "#, BTreeMap::new())?;

        Ok(rows_to_playlists(&result))
    }

    /// Get root playlists (no parent)
    pub fn get_roots(db: &MeshDb) -> Result<Vec<Playlist>, DbError> {
        let result = db.run_query(r#"
            ?[id, parent_id, name, sort_order] :=
                *playlists{id, parent_id, name, sort_order},
                is_null(parent_id)
            :order sort_order, name
        "#, BTreeMap::new())?;

        Ok(rows_to_playlists(&result))
    }

    /// Get children of a playlist
    pub fn get_children(db: &MeshDb, parent_id: i64) -> Result<Vec<Playlist>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("parent_id".to_string(), DataValue::from(parent_id));

        let result = db.run_query(r#"
            ?[id, parent_id, name, sort_order] :=
                *playlists{id, parent_id, name, sort_order},
                parent_id = $parent_id
            :order sort_order, name
        "#, params)?;

        Ok(rows_to_playlists(&result))
    }

    /// Get tracks in a playlist
    pub fn get_tracks(db: &MeshDb, playlist_id: i64) -> Result<Vec<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("playlist_id".to_string(), DataValue::from(playlist_id));

        // Note: sort_order must be in output columns to use :order in CozoDB
        let result = db.run_query(r#"
            ?[track_id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, sort_order] :=
                *playlist_tracks{playlist_id: pid, track_id, sort_order},
                *tracks{id: track_id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                pid = $playlist_id
            :order sort_order
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Create a new playlist
    pub fn create(db: &MeshDb, name: &str, parent_id: Option<i64>) -> Result<i64, DbError> {
        // Generate a new ID based on current time
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        params.insert("name".to_string(), DataValue::from(name));
        params.insert(
            "parent_id".to_string(),
            parent_id.map(DataValue::from).unwrap_or(DataValue::Null),
        );
        params.insert("sort_order".to_string(), DataValue::from(0_i64));

        db.run_script(
            r#"
            ?[id, parent_id, name, sort_order] <- [[$id, $parent_id, $name, $sort_order]]
            :put playlists {id => parent_id, name, sort_order}
        "#,
            params,
        )?;

        Ok(id)
    }

    /// Get a playlist by its ID
    pub fn get_by_id(db: &MeshDb, id: i64) -> Result<Option<Playlist>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));

        let result = db.run_query(
            r#"
            ?[id, parent_id, name, sort_order] :=
                *playlists{id, parent_id, name, sort_order},
                id = $id
        "#,
            params,
        )?;

        Ok(rows_to_playlists(&result).into_iter().next())
    }

    /// Get a playlist by name under a specific parent
    pub fn get_by_name(db: &MeshDb, name: &str, parent_id: Option<i64>) -> Result<Option<Playlist>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("name".to_string(), DataValue::from(name));

        let query = if parent_id.is_some() {
            params.insert("parent_id".to_string(), DataValue::from(parent_id.unwrap()));
            r#"
                ?[id, parent_id, name, sort_order] :=
                    *playlists{id, parent_id, name, sort_order},
                    name = $name,
                    parent_id = $parent_id
            "#
        } else {
            r#"
                ?[id, parent_id, name, sort_order] :=
                    *playlists{id, parent_id, name, sort_order},
                    name = $name,
                    is_null(parent_id)
            "#
        };

        let result = db.run_query(query, params)?;
        Ok(rows_to_playlists(&result).into_iter().next())
    }

    /// Rename a playlist
    pub fn rename(db: &MeshDb, id: i64, new_name: &str) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        params.insert("new_name".to_string(), DataValue::from(new_name));

        db.run_script(
            r#"
            ?[id, parent_id, name, sort_order] :=
                *playlists{id, parent_id, name: _, sort_order},
                id = $id,
                name = $new_name
            :put playlists {id => parent_id, name, sort_order}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Delete a playlist and all its track associations
    pub fn delete(db: &MeshDb, id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));

        // First remove all track associations
        db.run_script(
            r#"
            ?[playlist_id, track_id] :=
                *playlist_tracks{playlist_id, track_id},
                playlist_id = $id
            :rm playlist_tracks {playlist_id, track_id}
        "#,
            params.clone(),
        )?;

        // Then delete the playlist itself
        db.run_script(
            r#"
            ?[id] := id = $id
            :rm playlists {id}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Add a track to a playlist
    pub fn add_track(db: &MeshDb, playlist_id: i64, track_id: i64, sort_order: i32) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("playlist_id".to_string(), DataValue::from(playlist_id));
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("sort_order".to_string(), DataValue::from(sort_order as i64));

        db.run_script(
            r#"
            ?[playlist_id, track_id, sort_order] <- [[$playlist_id, $track_id, $sort_order]]
            :put playlist_tracks {playlist_id, track_id => sort_order}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Remove a track from a playlist
    pub fn remove_track(db: &MeshDb, playlist_id: i64, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("playlist_id".to_string(), DataValue::from(playlist_id));
        params.insert("track_id".to_string(), DataValue::from(track_id));

        db.run_script(
            r#"
            ?[playlist_id, track_id] := playlist_id = $playlist_id, track_id = $track_id
            :rm playlist_tracks {playlist_id, track_id}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Get the next sort order for a playlist
    pub fn next_sort_order(db: &MeshDb, playlist_id: i64) -> Result<i32, DbError> {
        let mut params = BTreeMap::new();
        params.insert("playlist_id".to_string(), DataValue::from(playlist_id));

        let result = db.run_query(
            r#"
            ?[max(sort_order)] :=
                *playlist_tracks{playlist_id, sort_order},
                playlist_id = $playlist_id
        "#,
            params,
        )?;

        let max = result
            .rows
            .first()
            .and_then(|row| row.first())
            .and_then(|v| v.get_int())
            .unwrap_or(-1);

        Ok((max + 1) as i32)
    }
}

// ============================================================================
// Similarity Queries
// ============================================================================

/// Query builder for similarity search
pub struct SimilarityQuery;

impl SimilarityQuery {
    /// Find similar tracks using HNSW vector search
    ///
    /// Uses the audio_features relation with HNSW index for fast approximate
    /// nearest neighbor search based on the 16-dimensional audio feature vector.
    pub fn find_similar(db: &MeshDb, track_id: i64, limit: usize) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("k".to_string(), DataValue::from((limit + 1) as i64)); // +1 to exclude self

        // First get the embedding for the query track, then search using HNSW
        let result = db.run_query(r#"
            ?[track_id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                *audio_features{track_id: $track_id, vec: query_vec},
                ~audio_features:similarity_index{track_id | query: query_vec, k: $k, ef: 50, bind_distance: dist},
                track_id != $track_id,
                *tracks{id: track_id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Find harmonically compatible tracks
    pub fn find_harmonic_compatible(db: &MeshDb, track_id: i64, limit: usize) -> Result<Vec<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("limit".to_string(), DataValue::from(limit as i64));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *harmonic_match{from_track: $track_id, to_track: id},
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :limit $limit
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Insert or update audio features for a track
    ///
    /// The feature vector is stored in the audio_features relation and automatically
    /// indexed by the HNSW similarity_index for fast nearest neighbor queries.
    pub fn upsert_features(db: &MeshDb, track_id: i64, features: &AudioFeatures) -> Result<(), DbError> {
        let vec = features.to_vector();
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("vec".to_string(), DataValue::List(vec.into_iter().map(DataValue::from).collect()));

        db.run_script(r#"
            ?[track_id, vec] <- [[$track_id, $vec]]
            :put audio_features {track_id => vec}
        "#, params)?;

        Ok(())
    }

    /// Check if a track has audio features stored
    pub fn has_features(db: &MeshDb, track_id: i64) -> Result<bool, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[count(track_id)] := *audio_features{track_id}, track_id = $track_id
        "#, params)?;

        Ok(result.rows.first()
            .and_then(|row| row.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) > 0)
    }

    /// Get all track IDs that have audio features
    pub fn get_tracks_with_features(db: &MeshDb) -> Result<Vec<i64>, DbError> {
        let result = db.run_query(r#"
            ?[track_id] := *audio_features{track_id}
        "#, BTreeMap::new())?;

        Ok(result.rows.iter()
            .filter_map(|row| row.first().and_then(|v| v.get_int()))
            .collect())
    }

    /// Count tracks with audio features
    pub fn count_with_features(db: &MeshDb) -> Result<usize, DbError> {
        let result = db.run_query(r#"
            ?[count(track_id)] := *audio_features{track_id}
        "#, BTreeMap::new())?;

        Ok(result.rows.first()
            .and_then(|row| row.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) as usize)
    }
}

// ============================================================================
// Cue Point Queries
// ============================================================================

/// Query builder for cue points
pub struct CuePointQuery;

impl CuePointQuery {
    /// Get all cue points for a track
    pub fn get_for_track(db: &MeshDb, track_id: i64) -> Result<Vec<CuePoint>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[track_id, index, sample_position, label, color] :=
                *cue_points{track_id, index, sample_position, label, color},
                track_id = $track_id
            :order index
        "#, params)?;

        Ok(rows_to_cue_points(&result))
    }

    /// Insert or update a single cue point
    pub fn upsert(db: &MeshDb, cue: &CuePoint) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(cue.track_id));
        params.insert("index".to_string(), DataValue::from(cue.index as i64));
        params.insert("sample_position".to_string(), DataValue::from(cue.sample_position));
        params.insert("label".to_string(), cue.label.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("color".to_string(), cue.color.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[track_id, index, sample_position, label, color] <- [[$track_id, $index, $sample_position, $label, $color]]
            :put cue_points {track_id, index => sample_position, label, color}
        "#, params)?;

        Ok(())
    }

    /// Delete a single cue point
    pub fn delete(db: &MeshDb, track_id: i64, index: u8) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("index".to_string(), DataValue::from(index as i64));

        db.run_script(r#"
            ?[track_id, index] := track_id = $track_id, index = $index
            :rm cue_points {track_id, index}
        "#, params)?;

        Ok(())
    }

    /// Delete all cue points for a track
    pub fn delete_all_for_track(db: &MeshDb, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        db.run_script(r#"
            ?[track_id, index] := *cue_points{track_id, index}, track_id = $track_id
            :rm cue_points {track_id, index}
        "#, params)?;

        Ok(())
    }

    /// Replace all cue points for a track (delete existing, insert new)
    pub fn replace_all(db: &MeshDb, track_id: i64, cues: &[CuePoint]) -> Result<(), DbError> {
        // Delete all existing
        Self::delete_all_for_track(db, track_id)?;

        // Insert new ones
        for cue in cues {
            Self::upsert(db, cue)?;
        }

        Ok(())
    }
}

// ============================================================================
// Saved Loop Queries
// ============================================================================

/// Query builder for saved loops
pub struct SavedLoopQuery;

impl SavedLoopQuery {
    /// Get all saved loops for a track
    pub fn get_for_track(db: &MeshDb, track_id: i64) -> Result<Vec<SavedLoop>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[track_id, index, start_sample, end_sample, label, color] :=
                *saved_loops{track_id, index, start_sample, end_sample, label, color},
                track_id = $track_id
            :order index
        "#, params)?;

        Ok(rows_to_saved_loops(&result))
    }

    /// Insert or update a single saved loop
    pub fn upsert(db: &MeshDb, loop_: &SavedLoop) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(loop_.track_id));
        params.insert("index".to_string(), DataValue::from(loop_.index as i64));
        params.insert("start_sample".to_string(), DataValue::from(loop_.start_sample));
        params.insert("end_sample".to_string(), DataValue::from(loop_.end_sample));
        params.insert("label".to_string(), loop_.label.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("color".to_string(), loop_.color.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[track_id, index, start_sample, end_sample, label, color] <- [[$track_id, $index, $start_sample, $end_sample, $label, $color]]
            :put saved_loops {track_id, index => start_sample, end_sample, label, color}
        "#, params)?;

        Ok(())
    }

    /// Delete a single saved loop
    pub fn delete(db: &MeshDb, track_id: i64, index: u8) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("index".to_string(), DataValue::from(index as i64));

        db.run_script(r#"
            ?[track_id, index] := track_id = $track_id, index = $index
            :rm saved_loops {track_id, index}
        "#, params)?;

        Ok(())
    }

    /// Delete all saved loops for a track
    pub fn delete_all_for_track(db: &MeshDb, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        db.run_script(r#"
            ?[track_id, index] := *saved_loops{track_id, index}, track_id = $track_id
            :rm saved_loops {track_id, index}
        "#, params)?;

        Ok(())
    }

    /// Replace all saved loops for a track (delete existing, insert new)
    pub fn replace_all(db: &MeshDb, track_id: i64, loops: &[SavedLoop]) -> Result<(), DbError> {
        // Delete all existing
        Self::delete_all_for_track(db, track_id)?;

        // Insert new ones
        for loop_ in loops {
            Self::upsert(db, loop_)?;
        }

        Ok(())
    }
}

// ============================================================================
// Stem Link Queries
// ============================================================================

/// Query builder for stem links (prepared mode stem replacements)
pub struct StemLinkQuery;

impl StemLinkQuery {
    /// Get all stem links for a track
    pub fn get_for_track(db: &MeshDb, track_id: i64) -> Result<Vec<StemLink>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[track_id, stem_index, source_track_id, source_stem] :=
                *stem_links{track_id, stem_index, source_track_id, source_stem},
                track_id = $track_id
            :order stem_index
        "#, params)?;

        Ok(rows_to_stem_links(&result))
    }

    /// Insert or update a single stem link
    pub fn upsert(db: &MeshDb, link: &StemLink) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(link.track_id));
        params.insert("stem_index".to_string(), DataValue::from(link.stem_index as i64));
        params.insert("source_track_id".to_string(), DataValue::from(link.source_track_id));
        params.insert("source_stem".to_string(), DataValue::from(link.source_stem as i64));

        db.run_script(r#"
            ?[track_id, stem_index, source_track_id, source_stem] <- [[$track_id, $stem_index, $source_track_id, $source_stem]]
            :put stem_links {track_id, stem_index => source_track_id, source_stem}
        "#, params)?;

        Ok(())
    }

    /// Delete a single stem link
    pub fn delete(db: &MeshDb, track_id: i64, stem_index: u8) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("stem_index".to_string(), DataValue::from(stem_index as i64));

        db.run_script(r#"
            ?[track_id, stem_index] := track_id = $track_id, stem_index = $stem_index
            :rm stem_links {track_id, stem_index}
        "#, params)?;

        Ok(())
    }

    /// Delete all stem links for a track
    pub fn delete_all_for_track(db: &MeshDb, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        db.run_script(r#"
            ?[track_id, stem_index] := *stem_links{track_id, stem_index}, track_id = $track_id
            :rm stem_links {track_id, stem_index}
        "#, params)?;

        Ok(())
    }

    /// Replace all stem links for a track (delete existing, insert new)
    pub fn replace_all(db: &MeshDb, track_id: i64, links: &[StemLink]) -> Result<(), DbError> {
        // Delete all existing
        Self::delete_all_for_track(db, track_id)?;

        // Insert new ones
        for link in links {
            Self::upsert(db, link)?;
        }

        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

fn rows_to_cue_points(result: &NamedRows) -> Vec<CuePoint> {
    result.rows.iter().filter_map(|row| {
        Some(CuePoint {
            track_id: row.get(0)?.get_int()?,
            index: row.get(1)?.get_int()? as u8,
            sample_position: row.get(2)?.get_int()?,
            label: row.get(3)?.get_str().map(|s| s.to_string()),
            color: row.get(4)?.get_str().map(|s| s.to_string()),
        })
    }).collect()
}

fn rows_to_saved_loops(result: &NamedRows) -> Vec<SavedLoop> {
    result.rows.iter().filter_map(|row| {
        Some(SavedLoop {
            track_id: row.get(0)?.get_int()?,
            index: row.get(1)?.get_int()? as u8,
            start_sample: row.get(2)?.get_int()?,
            end_sample: row.get(3)?.get_int()?,
            label: row.get(4)?.get_str().map(|s| s.to_string()),
            color: row.get(5)?.get_str().map(|s| s.to_string()),
        })
    }).collect()
}

fn rows_to_tracks(result: &NamedRows) -> Vec<TrackRow> {
    result.rows.iter().filter_map(|row| {
        Some(TrackRow {
            id: row.get(0)?.get_int()?,
            path: row.get(1)?.get_str()?.to_string(),
            folder_path: row.get(2)?.get_str()?.to_string(),
            name: row.get(3)?.get_str()?.to_string(),
            artist: row.get(4)?.get_str().map(|s| s.to_string()),
            bpm: row.get(5)?.get_float(),
            original_bpm: row.get(6)?.get_float(),
            key: row.get(7)?.get_str().map(|s| s.to_string()),
            duration_seconds: row.get(8)?.get_float().unwrap_or(0.0),
            lufs: row.get(9)?.get_float().map(|f| f as f32),
            drop_marker: row.get(10)?.get_int(),
            first_beat_sample: row.get(11)?.get_int().unwrap_or(0),
            file_mtime: row.get(12)?.get_int().unwrap_or(0),
            file_size: row.get(13)?.get_int().unwrap_or(0),
            waveform_path: row.get(14)?.get_str().map(|s| s.to_string()),
        })
    }).collect()
}

fn rows_to_tracks_with_distance(result: &NamedRows) -> Vec<(TrackRow, f32)> {
    result.rows.iter().filter_map(|row| {
        let track = TrackRow {
            id: row.get(0)?.get_int()?,
            path: row.get(1)?.get_str()?.to_string(),
            folder_path: row.get(2)?.get_str()?.to_string(),
            name: row.get(3)?.get_str()?.to_string(),
            artist: row.get(4)?.get_str().map(|s| s.to_string()),
            bpm: row.get(5)?.get_float(),
            original_bpm: row.get(6)?.get_float(),
            key: row.get(7)?.get_str().map(|s| s.to_string()),
            duration_seconds: row.get(8)?.get_float().unwrap_or(0.0),
            lufs: row.get(9)?.get_float().map(|f| f as f32),
            drop_marker: row.get(10)?.get_int(),
            first_beat_sample: row.get(11)?.get_int().unwrap_or(0),
            file_mtime: row.get(12)?.get_int().unwrap_or(0),
            file_size: row.get(13)?.get_int().unwrap_or(0),
            waveform_path: row.get(14)?.get_str().map(|s| s.to_string()),
        };
        let distance = row.get(15)?.get_float()? as f32;
        Some((track, distance))
    }).collect()
}

fn rows_to_stem_links(result: &NamedRows) -> Vec<StemLink> {
    result.rows.iter().filter_map(|row| {
        Some(StemLink {
            track_id: row.get(0)?.get_int()?,
            stem_index: row.get(1)?.get_int()? as u8,
            source_track_id: row.get(2)?.get_int()?,
            source_stem: row.get(3)?.get_int()? as u8,
        })
    }).collect()
}

fn rows_to_playlists(result: &NamedRows) -> Vec<Playlist> {
    result.rows.iter().filter_map(|row| {
        Some(Playlist {
            id: row.get(0)?.get_int()?,
            parent_id: row.get(1)?.get_int(),
            name: row.get(2)?.get_str()?.to_string(),
            sort_order: row.get(3)?.get_int().unwrap_or(0) as i32,
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_crud() {
        let db = MeshDb::in_memory().unwrap();

        let track = TrackRow {
            id: 1,
            path: "/music/track1.wav".to_string(),
            folder_path: "/music".to_string(),
            name: "Test Track".to_string(),
            artist: Some("Test Artist".to_string()),
            bpm: Some(128.0),
            original_bpm: Some(128.0),
            key: Some("8A".to_string()),
            duration_seconds: 180.0,
            lufs: Some(-8.0),
            drop_marker: None,
            first_beat_sample: 14335,
            file_mtime: 1234567890,
            file_size: 1000000,
            waveform_path: None,
        };

        // Insert
        TrackQuery::upsert(&db, &track).unwrap();

        // Read back
        let retrieved = TrackQuery::get_by_id(&db, 1).unwrap().unwrap();
        assert_eq!(retrieved.name, "Test Track");
        assert_eq!(retrieved.artist, Some("Test Artist".to_string()));

        // Count
        assert_eq!(TrackQuery::count(&db).unwrap(), 1);

        // Delete
        TrackQuery::delete(&db, 1).unwrap();
        assert_eq!(TrackQuery::count(&db).unwrap(), 0);
    }
}
