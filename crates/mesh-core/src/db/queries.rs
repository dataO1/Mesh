//! Query builders and helpers for CozoDB
//!
//! This module provides typed query APIs that generate CozoScript internally.

use super::schema::{TrackRow, Playlist, AudioFeatures, CuePoint, SavedLoop, StemLink, TrackPlayRecord, TrackPlayUpdate};
use super::{MeshDb, DbError};
use cozo::{DataValue, NamedRows, Vector};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Column list for track queries (must match schema order for first_beat_sample)
const TRACK_COLUMNS: &str = "id, path, folder_path, title, original_name, artist, bpm, original_bpm, key, duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path";

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
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                folder_path = $folder
            :order title
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
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
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
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                id in $ids
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Get a track by path
    pub fn get_by_path(db: &MeshDb, path: &str) -> Result<Option<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("path".to_string(), DataValue::Str(path.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                path = $path
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Find a track by filename (last path segment), useful for cross-mount lookups
    pub fn find_by_filename(db: &MeshDb, filename: &str) -> Result<Option<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("filename".to_string(), DataValue::Str(filename.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                ends_with(path, $filename)
            :limit 1
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Get all tracks in the database
    pub fn get_all(db: &MeshDb) -> Result<Vec<TrackRow>, DbError> {
        let result = db.run_query(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order title
        "#, BTreeMap::new())?;

        Ok(rows_to_tracks(&result))
    }

    /// Search tracks by title or artist
    pub fn search(db: &MeshDb, query: &str, limit: usize) -> Result<Vec<TrackRow>, DbError> {
        let mut params = BTreeMap::new();
        let query_lower = query.to_lowercase();
        params.insert("query".to_string(), DataValue::Str(query_lower.into()));
        params.insert("limit".to_string(), DataValue::from(limit as i64));

        let result = db.run_query(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                (lowercase(title) ~ $query or
                 (is_not_null(artist) and lowercase(artist) ~ $query))
            :limit $limit
            :order title
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Insert or update a track
    pub fn upsert(db: &MeshDb, track: &TrackRow) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track.id));
        params.insert("path".to_string(), DataValue::Str(track.path.clone().into()));
        params.insert("folder_path".to_string(), DataValue::Str(track.folder_path.clone().into()));
        params.insert("title".to_string(), DataValue::Str(track.title.clone().into()));
        params.insert("original_name".to_string(), DataValue::Str(track.original_name.clone().into()));
        params.insert("artist".to_string(), track.artist.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("bpm".to_string(), track.bpm.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("original_bpm".to_string(), track.original_bpm.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("key".to_string(), track.key.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("duration_seconds".to_string(), DataValue::from(track.duration_seconds));
        params.insert("lufs".to_string(), track.lufs.map(|v| DataValue::from(v as f64)).unwrap_or(DataValue::Null));
        params.insert("integrated_lufs".to_string(), track.integrated_lufs.map(|v| DataValue::from(v as f64)).unwrap_or(DataValue::Null));
        params.insert("drop_marker".to_string(), track.drop_marker.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("first_beat_sample".to_string(), DataValue::from(track.first_beat_sample));
        params.insert("file_mtime".to_string(), DataValue::from(track.file_mtime));
        params.insert("file_size".to_string(), DataValue::from(track.file_size));
        params.insert("waveform_path".to_string(), track.waveform_path.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
                $id, $path, $folder_path, $title, $original_name, $artist, $bpm, $original_bpm, $key,
                $duration_seconds, $lufs, $integrated_lufs, $drop_marker, $first_beat_sample, $file_mtime, $file_size, $waveform_path
            ]]
            :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                         duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
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
    /// Supported fields: title, artist, bpm, original_bpm, key, lufs, integrated_lufs, drop_marker, first_beat_sample
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
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, bpm, original_bpm, key,
                                duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        artist = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid BPM value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, original_bpm, key,
                                duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        bpm = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "original_bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid original_bpm value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, key,
                                duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        original_bpm = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "key" => {
                let val = if value.is_empty() { DataValue::Null } else { DataValue::Str(value.into()) };
                params.insert("value".to_string(), val);
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm,
                                duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        key = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "lufs" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid LUFS value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                duration_seconds, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        lufs = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "integrated_lufs" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid integrated_lufs value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        integrated_lufs = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "drop_marker" => {
                let val: i64 = value.parse().map_err(|_| DbError::Query(format!("Invalid drop_marker value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, integrated_lufs, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        drop_marker = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "first_beat_sample" => {
                let val: i64 = value.parse().map_err(|_| DbError::Query(format!("Invalid first_beat_sample value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, integrated_lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        first_beat_sample = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
                "#
            }
            "title" => {
                let val = if value.is_empty() { DataValue::Null } else { DataValue::Str(value.into()) };
                params.insert("value".to_string(), val);
                r#"
                    ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, original_name, artist, bpm, original_bpm, key,
                                duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                        id = $id,
                        title = $value
                    :put tracks {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
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

    /// Get all tracks that have a drop marker set.
    /// Used for opener suggestions (no deck playing) — scored on-the-fly.
    pub fn get_with_drop_marker(db: &MeshDb) -> Result<Vec<TrackRow>, DbError> {
        let result = db.run_query(r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                is_not_null(drop_marker)
            :order id
        "#, BTreeMap::new())?;

        Ok(rows_to_tracks(&result))
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
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, sort_order] :=
                *playlist_tracks{playlist_id: pid, track_id, sort_order},
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
                pid = $playlist_id
            :order sort_order
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Get all playlist memberships as a map from track_id to playlist names.
    ///
    /// Intended for batch reverse-lookup: given a set of suggestion track IDs,
    /// find which playlists they belong to in a single query.
    pub fn get_all_memberships(db: &MeshDb) -> Result<HashMap<i64, Vec<String>>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, name] :=
                *playlist_tracks{playlist_id, track_id},
                *playlists{id: playlist_id, name}
            :order track_id
        "#, BTreeMap::new())?;

        let mut memberships: HashMap<i64, Vec<String>> = HashMap::new();
        for row in &result.rows {
            let track_id = match row.get(0).and_then(|v| v.get_int()) {
                Some(id) => id,
                None => continue,
            };
            let name = match row.get(1).and_then(|v| v.get_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            memberships.entry(track_id).or_default().push(name);
        }
        Ok(memberships)
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

    /// Add multiple tracks to a playlist in a single query
    pub fn add_tracks_batch(db: &MeshDb, playlist_id: i64, track_ids: &[(i64, i32)]) -> Result<(), DbError> {
        if track_ids.is_empty() {
            return Ok(());
        }
        // Build inline data rows: [[playlist_id, track_id, sort_order], ...]
        let rows: Vec<DataValue> = track_ids.iter()
            .map(|&(tid, sort)| DataValue::List(vec![
                DataValue::from(playlist_id),
                DataValue::from(tid),
                DataValue::from(sort as i64),
            ]))
            .collect();
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(
            r#"
            ?[playlist_id, track_id, sort_order] <- $rows
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

    /// Remove multiple tracks from a playlist in a single query
    pub fn remove_tracks_batch(db: &MeshDb, playlist_id: i64, track_ids: &[i64]) -> Result<(), DbError> {
        if track_ids.is_empty() {
            return Ok(());
        }
        let rows: Vec<DataValue> = track_ids.iter()
            .map(|&tid| DataValue::List(vec![
                DataValue::from(playlist_id),
                DataValue::from(tid),
            ]))
            .collect();
        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(
            r#"
            ?[playlist_id, track_id] <- $rows
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

/// Extract an f32 vector from a CozoDB DataValue.
///
/// CozoDB typed vectors (`<F32; N>`) are returned as `DataValue::Vec(Vector::F32(...))`,
/// while ad-hoc lists come back as `DataValue::List(...)`. This handles both.
fn extract_f32_vec(val: Option<&DataValue>) -> Result<Option<Vec<f32>>, DbError> {
    match val {
        Some(DataValue::Vec(Vector::F32(arr))) => Ok(Some(arr.to_vec())),
        Some(DataValue::Vec(Vector::F64(arr))) => Ok(Some(arr.iter().map(|&v| v as f32).collect())),
        Some(DataValue::List(items)) => {
            let v: Vec<f32> = items
                .iter()
                .filter_map(|v| v.get_float().map(|f| f as f32))
                .collect();
            if v.is_empty() { Ok(None) } else { Ok(Some(v)) }
        }
        _ => Ok(None),
    }
}

impl SimilarityQuery {
    /// Find similar tracks using HNSW vector search
    ///
    /// Uses the audio_features relation with HNSW index for fast approximate
    /// nearest neighbor search based on the 16-dimensional audio feature vector.
    pub fn find_similar(db: &MeshDb, track_id: i64, limit: usize) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        let k = (limit + 1) as i64; // +1 to account for self-exclusion
        params.insert("k".to_string(), DataValue::from(k));
        // ef (search beam width) must be >= k for good recall
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        // First get the embedding for the query track, then search using HNSW
        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                *audio_features{track_id: $track_id, vec: query_vec},
                ~audio_features:similarity_index{track_id | query: query_vec, k: $k, ef: $ef, bind_distance: dist},
                track_id != $track_id,
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Find similar tracks by raw feature vector (for cross-database search).
    ///
    /// Unlike `find_similar()` which looks up the vector from the same DB,
    /// this accepts an external vector — enabling seeds from one database
    /// to search another database's HNSW index.
    pub fn find_similar_by_vector(db: &MeshDb, query_vec: &[f64], limit: usize) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("query_vec".to_string(), DataValue::List(query_vec.iter().map(|&v| DataValue::from(v)).collect()));
        let k = limit as i64;
        params.insert("k".to_string(), DataValue::from(k));
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                ~audio_features:similarity_index{track_id | query: vec($query_vec), k: $k, ef: $ef, bind_distance: dist},
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
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
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] :=
                *harmonic_match{from_track: $track_id, to_track: id},
                *tracks{id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
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

    pub fn get_tracks_with_ml_embeddings(db: &MeshDb) -> Result<Vec<i64>, DbError> {
        let result = db.run_query(r#"
            ?[track_id] := *ml_embeddings{track_id}
        "#, BTreeMap::new())?;
        Ok(result.rows.iter()
            .filter_map(|row| row.first().and_then(|v| v.get_int()))
            .collect())
    }

    pub fn get_tracks_with_stem_energy(db: &MeshDb) -> Result<Vec<i64>, DbError> {
        let result = db.run_query(r#"
            ?[track_id] := *stem_energy{track_id}
        "#, BTreeMap::new())?;
        Ok(result.rows.iter()
            .filter_map(|row| row.first().and_then(|v| v.get_int()))
            .collect())
    }

    pub fn get_tracks_with_dissonance(db: &MeshDb) -> Result<Vec<i64>, DbError> {
        let result = db.run_query(r#"
            ?[track_id] := *track_dissonance{track_id}
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

    // ── EffNet 1280-dim embedding ──────────────────────────────────────────

    /// Insert or update a 1280-dim EffNet embedding for a track.
    pub fn upsert_ml_embedding(db: &MeshDb, track_id: i64, embedding: &[f32]) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("vec".to_string(), DataValue::List(
            embedding.iter().map(|&v| DataValue::from(v as f64)).collect(),
        ));

        db.run_script(r#"
            ?[track_id, vec] <- [[$track_id, $vec]]
            :put ml_embeddings {track_id => vec}
        "#, params)?;

        Ok(())
    }

    /// Find similar tracks via EffNet HNSW using the seed track's stored embedding.
    pub fn find_similar_by_ml_id(
        db: &MeshDb,
        track_id: i64,
        limit: usize,
    ) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        let k = (limit + 1) as i64;
        params.insert("k".to_string(), DataValue::from(k));
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                *ml_embeddings{track_id: $track_id, vec: query_vec},
                ~ml_embeddings:similarity_index{track_id | query: query_vec, k: $k, ef: $ef, bind_distance: dist},
                track_id != $track_id,
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Find similar tracks via EffNet HNSW using a raw vector (cross-database).
    pub fn find_similar_by_ml_vector(
        db: &MeshDb,
        query_vec: &[f64],
        limit: usize,
    ) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("query_vec".to_string(), DataValue::List(
            query_vec.iter().map(|&v| DataValue::from(v)).collect(),
        ));
        let k = limit as i64;
        params.insert("k".to_string(), DataValue::from(k));
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                ~ml_embeddings:similarity_index{track_id | query: vec($query_vec), k: $k, ef: $ef, bind_distance: dist},
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Retrieve the raw 1280-dim EffNet embedding for a track (returns None if absent).
    pub fn get_ml_embedding_raw(db: &MeshDb, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[vec] := *ml_embeddings{track_id: $track_id, vec}
        "#, params)?;

        let row = match result.rows.first() {
            Some(r) => r,
            None => return Ok(None),
        };
        extract_f32_vec(row.first())
    }

    // ── Stem energy densities ─────────────────────────────────────────────

    /// Insert or update per-stem RMS energy densities for a track.
    pub fn upsert_stem_energy(
        db: &MeshDb,
        track_id: i64,
        vocal: f32,
        drums: f32,
        bass: f32,
        other: f32,
    ) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("vocal".to_string(), DataValue::from(vocal as f64));
        params.insert("drums".to_string(), DataValue::from(drums as f64));
        params.insert("bass".to_string(), DataValue::from(bass as f64));
        params.insert("other".to_string(), DataValue::from(other as f64));

        db.run_script(r#"
            ?[track_id, vocal_density, drums_density, bass_density, other_density] <-
                [[$track_id, $vocal, $drums, $bass, $other]]
            :put stem_energy {track_id => vocal_density, drums_density, bass_density, other_density}
        "#, params)?;

        Ok(())
    }

    /// Get per-stem energy densities for a single track.
    /// Returns `(vocal, drums, bass, other)` or None if absent.
    pub fn get_stem_energy(
        db: &MeshDb,
        track_id: i64,
    ) -> Result<Option<(f32, f32, f32, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[vocal, drums, bass, other] :=
                *stem_energy{track_id: $track_id,
                             vocal_density: vocal,
                             drums_density: drums,
                             bass_density: bass,
                             other_density: other}
        "#, params)?;

        Ok(result.rows.first().and_then(|row| {
            if row.len() < 4 { return None; }
            let v = row[0].get_float()? as f32;
            let d = row[1].get_float()? as f32;
            let b = row[2].get_float()? as f32;
            let o = row[3].get_float()? as f32;
            Some((v, d, b, o))
        }))
    }

    /// Batch-fetch stem energy for multiple tracks (avoids N+1 in the scoring loop).
    /// Returns a map of `track_id → (vocal, drums, bass, other)`.
    pub fn batch_get_stem_energy(
        db: &MeshDb,
        track_ids: &[i64],
    ) -> Result<HashMap<i64, (f32, f32, f32, f32)>, DbError> {
        if track_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids_list: Vec<DataValue> = track_ids.iter().map(|&id| DataValue::from(id)).collect();
        let mut params = BTreeMap::new();
        params.insert("ids".to_string(), DataValue::List(ids_list));

        let result = db.run_query(r#"
            ?[track_id, vocal, drums, bass, other] :=
                *stem_energy{track_id,
                             vocal_density: vocal,
                             drums_density: drums,
                             bass_density: bass,
                             other_density: other},
                is_in(track_id, $ids)
        "#, params)?;

        let mut map = HashMap::new();
        for row in &result.rows {
            if row.len() < 5 { continue; }
            let id = match row[0].get_int() { Some(v) => v, None => continue };
            let v  = match row[1].get_float() { Some(f) => f as f32, None => continue };
            let d  = match row[2].get_float() { Some(f) => f as f32, None => continue };
            let b  = match row[3].get_float() { Some(f) => f as f32, None => continue };
            let o  = match row[4].get_float() { Some(f) => f as f32, None => continue };
            map.insert(id, (v, d, b, o));
        }
        Ok(map)
    }

    // ── PCA 128-dim embeddings ─────────────────────────────────────────────

    /// Insert or update a 128-dim PCA-projected embedding for a track.
    pub fn upsert_pca_embedding(db: &MeshDb, track_id: i64, embedding: &[f32]) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("vec".to_string(), DataValue::List(
            embedding.iter().map(|&v| DataValue::from(v as f64)).collect(),
        ));

        db.run_script(r#"
            ?[track_id, vec] <- [[$track_id, $vec]]
            :put ml_pca_embeddings {track_id => vec}
        "#, params)?;

        Ok(())
    }

    /// Find similar tracks via PCA HNSW using the seed track's stored embedding.
    pub fn find_similar_by_pca_id(
        db: &MeshDb,
        track_id: i64,
        limit: usize,
    ) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        let k = (limit + 1) as i64;
        params.insert("k".to_string(), DataValue::from(k));
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                *ml_pca_embeddings{track_id: $track_id, vec: query_vec},
                ~ml_pca_embeddings:similarity_index{track_id | query: query_vec, k: $k, ef: $ef, bind_distance: dist},
                track_id != $track_id,
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Find similar tracks via PCA HNSW using a raw 128-dim vector (cross-database).
    pub fn find_similar_by_pca_vector(
        db: &MeshDb,
        query_vec: &[f64],
        limit: usize,
    ) -> Result<Vec<(TrackRow, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("query_vec".to_string(), DataValue::List(
            query_vec.iter().map(|&v| DataValue::from(v)).collect(),
        ));
        let k = limit as i64;
        params.insert("k".to_string(), DataValue::from(k));
        params.insert("ef".to_string(), DataValue::from(k.max(50)));

        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, dist] :=
                ~ml_pca_embeddings:similarity_index{track_id | query: vec($query_vec), k: $k, ef: $ef, bind_distance: dist},
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Retrieve the raw 128-dim PCA embedding for a track (returns None if not yet built).
    pub fn get_pca_embedding_raw(db: &MeshDb, track_id: i64) -> Result<Option<Vec<f32>>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[vec] := *ml_pca_embeddings{track_id: $track_id, vec}
        "#, params)?;

        let row = match result.rows.first() {
            Some(r) => r,
            None => return Ok(None),
        };
        let vec = extract_f32_vec(row.first())?;
        Ok(vec)
    }

    /// Scan all 128-dim PCA embeddings with track metadata.
    /// Returns (Track, 128-dim vector) for every track with a PCA embedding.
    /// Used by the graph view for brute-force all-tracks scoring.
    pub fn get_all_pca_with_tracks(db: &MeshDb) -> Result<Vec<(TrackRow, Vec<f32>)>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path, vec] :=
                *ml_pca_embeddings{track_id, vec},
                *tracks{id: track_id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
        "#, BTreeMap::new())?;

        Ok(result.rows.iter().filter_map(|row| {
            let vec = extract_f32_vec(row.get(17)).ok().flatten()?;
            if vec.len() != 128 { return None; }
            let track_row = TrackRow {
                id: row.get(0)?.get_int()?,
                path: row.get(1)?.get_str()?.to_string(),
                folder_path: row.get(2)?.get_str()?.to_string(),
                title: row.get(3)?.get_str()?.to_string(),
                original_name: row.get(4)?.get_str().unwrap_or("").to_string(),
                artist: row.get(5)?.get_str().map(|s| s.to_string()),
                bpm: row.get(6)?.get_float(),
                original_bpm: row.get(7)?.get_float(),
                key: row.get(8)?.get_str().map(|s| s.to_string()),
                duration_seconds: row.get(9)?.get_float().unwrap_or(0.0),
                lufs: row.get(10)?.get_float().map(|f| f as f32),
                integrated_lufs: row.get(11)?.get_float().map(|f| f as f32),
                drop_marker: row.get(12)?.get_int(),
                first_beat_sample: row.get(13)?.get_int().unwrap_or(0),
                file_mtime: row.get(14)?.get_int().unwrap_or(0),
                file_size: row.get(15)?.get_int().unwrap_or(0),
                waveform_path: row.get(16)?.get_str().map(|s| s.to_string()),
            };
            Some((track_row, vec))
        }).collect())
    }

    /// Scan all 1280-dim EffNet embeddings — used as input for PCA build.
    /// Returns (track_id, 1280-dim vector) for every track that has been ML-analysed.
    pub fn get_all_ml_embeddings(db: &MeshDb) -> Result<Vec<(i64, Vec<f32>)>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, vec] := *ml_embeddings{track_id, vec}
        "#, BTreeMap::new())?;

        Ok(result.rows.iter().filter_map(|row| {
            let id = row.get(0)?.get_int()?;
            let vec = extract_f32_vec(row.get(1)).ok().flatten()?;
            if vec.len() != 1280 { return None; }
            Some((id, vec))
        }).collect())
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

    /// Get all cue points for all tracks (bulk query for sync)
    pub fn get_all(db: &MeshDb) -> Result<HashMap<i64, Vec<CuePoint>>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, index, sample_position, label, color] :=
                *cue_points{track_id, index, sample_position, label, color}
            :order track_id, index
        "#, BTreeMap::new())?;

        let mut map: HashMap<i64, Vec<CuePoint>> = HashMap::new();
        for cue in rows_to_cue_points(&result) {
            map.entry(cue.track_id).or_default().push(cue);
        }
        Ok(map)
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

    /// Get all saved loops for all tracks (bulk query for sync)
    pub fn get_all(db: &MeshDb) -> Result<HashMap<i64, Vec<SavedLoop>>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, index, start_sample, end_sample, label, color] :=
                *saved_loops{track_id, index, start_sample, end_sample, label, color}
            :order track_id, index
        "#, BTreeMap::new())?;

        let mut map: HashMap<i64, Vec<SavedLoop>> = HashMap::new();
        for loop_ in rows_to_saved_loops(&result) {
            map.entry(loop_.track_id).or_default().push(loop_);
        }
        Ok(map)
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

    /// Get all stem links for all tracks (bulk query for sync)
    pub fn get_all(db: &MeshDb) -> Result<HashMap<i64, Vec<StemLink>>, DbError> {
        let result = db.run_query(r#"
            ?[track_id, stem_index, source_track_id, source_stem] :=
                *stem_links{track_id, stem_index, source_track_id, source_stem}
            :order track_id, stem_index
        "#, BTreeMap::new())?;

        let mut map: HashMap<i64, Vec<StemLink>> = HashMap::new();
        for link in rows_to_stem_links(&result) {
            map.entry(link.track_id).or_default().push(link);
        }
        Ok(map)
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
// History Queries
// ============================================================================

/// Query builder for DJ session history (sessions + track_plays)
pub struct HistoryQuery;

impl HistoryQuery {
    /// Create a new session record
    pub fn insert_session(db: &MeshDb, id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        db.run_script(r#"
            ?[id, ended_at] <- [[$id, null]]
            :put sessions {id => ended_at}
        "#, params)?;
        Ok(())
    }

    /// Mark session as ended
    pub fn end_session(db: &MeshDb, id: i64, ended_at: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(id));
        params.insert("ended_at".to_string(), DataValue::from(ended_at));
        db.run_script(r#"
            ?[id, ended_at] <- [[$id, $ended_at]]
            :put sessions {id => ended_at}
        "#, params)?;
        Ok(())
    }

    /// Insert a new track play record (load-time fields only; play fields start null)
    pub fn insert_track_play(db: &MeshDb, r: &TrackPlayRecord) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("session_id".to_string(), DataValue::from(r.session_id));
        params.insert("loaded_at".to_string(), DataValue::from(r.loaded_at));
        params.insert("track_path".to_string(), DataValue::Str(r.track_path.clone().into()));
        params.insert("track_name".to_string(), DataValue::Str(r.track_name.clone().into()));
        params.insert("track_id".to_string(), r.track_id.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("deck_index".to_string(), DataValue::from(r.deck_index as i64));
        params.insert("load_source".to_string(), DataValue::Str(r.load_source.clone().into()));
        params.insert("suggestion_score".to_string(), r.suggestion_score.map(|f| DataValue::from(f as f64)).unwrap_or(DataValue::Null));
        params.insert("suggestion_tags_json".to_string(), r.suggestion_tags_json.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("suggestion_energy_dir".to_string(), r.suggestion_energy_dir.map(|f| DataValue::from(f as f64)).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[session_id, loaded_at, track_path, track_name, track_id, deck_index,
              load_source, suggestion_score, suggestion_tags_json, suggestion_energy_dir,
              play_started_at, play_start_sample, play_ended_at, seconds_played,
              hot_cues_used_json, loop_was_active, played_with_json]
            <- [[$session_id, $loaded_at, $track_path, $track_name, $track_id, $deck_index,
                 $load_source, $suggestion_score, $suggestion_tags_json, $suggestion_energy_dir,
                 null, null, null, null, null, false, null]]
            :put track_plays {
                session_id, loaded_at =>
                track_path, track_name, track_id, deck_index,
                load_source, suggestion_score, suggestion_tags_json, suggestion_energy_dir,
                play_started_at, play_start_sample, play_ended_at, seconds_played,
                hot_cues_used_json, loop_was_active, played_with_json
            }
        "#, params)?;
        Ok(())
    }

    /// Update play_started fields when the DJ first presses play
    pub fn update_play_started(
        db: &MeshDb,
        session_id: i64,
        loaded_at: i64,
        play_started_at: i64,
        play_start_sample: i64,
        played_with_json: Option<String>,
    ) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("session_id".to_string(), DataValue::from(session_id));
        params.insert("loaded_at".to_string(), DataValue::from(loaded_at));
        params.insert("play_started_at".to_string(), DataValue::from(play_started_at));
        params.insert("play_start_sample".to_string(), DataValue::from(play_start_sample));
        params.insert("played_with_json".to_string(), played_with_json.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[session_id, loaded_at, play_started_at, play_start_sample, played_with_json]
            <- [[$session_id, $loaded_at, $play_started_at, $play_start_sample, $played_with_json]]
            :update track_plays {
                session_id, loaded_at =>
                play_started_at, play_start_sample, played_with_json
            }
        "#, params)?;
        Ok(())
    }

    /// Update only played_with_json on an existing track play (bidirectional co-play update)
    pub fn update_played_with(
        db: &MeshDb,
        session_id: i64,
        loaded_at: i64,
        played_with_json: Option<String>,
    ) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("session_id".to_string(), DataValue::from(session_id));
        params.insert("loaded_at".to_string(), DataValue::from(loaded_at));
        params.insert("played_with_json".to_string(), played_with_json.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[session_id, loaded_at, played_with_json]
            <- [[$session_id, $loaded_at, $played_with_json]]
            :update track_plays { session_id, loaded_at => played_with_json }
        "#, params)?;
        Ok(())
    }

    /// Finalize a track play when the track is replaced or session ends
    pub fn finalize_track_play(
        db: &MeshDb,
        session_id: i64,
        loaded_at: i64,
        u: &TrackPlayUpdate,
    ) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("session_id".to_string(), DataValue::from(session_id));
        params.insert("loaded_at".to_string(), DataValue::from(loaded_at));
        params.insert("play_ended_at".to_string(), u.play_ended_at.map(DataValue::from).unwrap_or(DataValue::Null));
        params.insert("seconds_played".to_string(), u.seconds_played.map(|f| DataValue::from(f as f64)).unwrap_or(DataValue::Null));
        params.insert("hot_cues_used_json".to_string(), u.hot_cues_used_json.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));
        params.insert("loop_was_active".to_string(), DataValue::from(u.loop_was_active));

        db.run_script(r#"
            ?[session_id, loaded_at, play_ended_at, seconds_played, hot_cues_used_json, loop_was_active]
            <- [[$session_id, $loaded_at, $play_ended_at, $seconds_played, $hot_cues_used_json, $loop_was_active]]
            :update track_plays {
                session_id, loaded_at =>
                play_ended_at, seconds_played, hot_cues_used_json, loop_was_active
            }
        "#, params)?;
        Ok(())
    }

    // ── Transition graph ──────────────────────────────────────────────────

    /// Rebuild the played_after graph from all track_plays records.
    ///
    /// Reads every track_play with a known track_id and a played_with_json in the
    /// new `[[id, "name"], ...]` format. Aggregates counts and max epoch per edge,
    /// clears the existing relation, and batch-upserts the result.
    ///
    /// Safe to call repeatedly — always produces a consistent snapshot of the history.
    pub fn build_played_after_graph(db: &MeshDb) -> Result<usize, DbError> {
        // Fetch all co-play records with IDs
        let result = db.run_query(r#"
            ?[from_id, played_with_json, play_started_at] :=
                *track_plays{track_id: from_id, played_with_json, play_started_at},
                from_id != null,
                played_with_json != null
        "#, BTreeMap::new())?;

        // Parse JSON and aggregate: (from_id, to_id) → (count, max_epoch)
        let mut edges: HashMap<(i64, i64), (u32, i64)> = HashMap::new();
        let mut parsed_count = 0usize;
        let mut skipped_old_format = 0usize;

        for row in &result.rows {
            let from_id = match row.get(0).and_then(|v| v.get_int()) {
                Some(id) => id,
                None => continue,
            };
            let json_str = match row.get(1).and_then(|v| v.get_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let epoch = row.get(2).and_then(|v| v.get_int()).unwrap_or(0);

            // New format: [[i64, "name"], ...]. Old format (["name", ...]) will fail to
            // deserialize as Vec<(i64, String)> and is silently skipped.
            let pairs: Vec<(i64, String)> = match serde_json::from_str(&json_str) {
                Ok(p) => p,
                Err(_) => { skipped_old_format += 1; continue; }
            };

            for (to_id, _) in &pairs {
                let entry = edges.entry((from_id, *to_id)).or_insert((0, epoch));
                entry.0 += 1;
                if epoch > entry.1 { entry.1 = epoch; }
            }
            parsed_count += 1;
        }

        log::info!(
            "[HISTORY GRAPH] Parsed {} play records ({} old-format skipped), found {} unique co-play edges",
            parsed_count, skipped_old_format, edges.len()
        );

        if edges.is_empty() {
            return Ok(0);
        }

        // Clear existing played_after data (full rebuild via drop+recreate)
        let _ = db.run_script("::remove played_after", BTreeMap::new());
        db.run_script(r#"
            {:create played_after {
                from_id: Int,
                to_id: Int =>
                count: Int,
                last_played_epoch: Int
            }}
        "#, BTreeMap::new())?;

        // Batch upsert all edges
        let rows: Vec<DataValue> = edges.iter().map(|((from, to), (cnt, epoch))| {
            DataValue::List(vec![
                DataValue::from(*from),
                DataValue::from(*to),
                DataValue::from(*cnt as i64),
                DataValue::from(*epoch),
            ])
        }).collect();

        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(r#"
            ?[from_id, to_id, count, last_played_epoch] <- $rows
            :put played_after { from_id, to_id => count, last_played_epoch }
        "#, params)?;

        log::info!("[HISTORY GRAPH] Built {} co-play edges from {} play records", edges.len(), parsed_count);
        Ok(edges.len())
    }

    /// Time-decayed co-play neighbors for a single seed track.
    ///
    /// Returns `(to_track_id, weight)` pairs where:
    /// `weight = min(count/10, 1.0) * exp(-age_days / 30.0)`
    ///
    /// Only returns pairs with count ≥ 5 (noise threshold).
    pub fn get_played_after_neighbors(
        db: &MeshDb,
        track_id: i64,
        limit: usize,
    ) -> Result<Vec<(i64, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("from_id".to_string(), DataValue::from(track_id));
        let k = limit as i64;
        params.insert("k".to_string(), DataValue::from(k));

        let result = db.run_query(r#"
            ?[to_id, count, last_played_epoch] :=
                *played_after{from_id: $from_id, to_id, count, last_played_epoch}
            :order -count
            :limit $k
        "#, params)?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let neighbors = result.rows.iter().filter_map(|row| {
            let to_id     = row.get(0)?.get_int()?;
            let count     = row.get(1)?.get_int()? as f32;
            let last_epoch = row.get(2)?.get_int()?;

            if count < 5.0 { return None; } // noise threshold

            let age_days = (now_ms - last_epoch).max(0) as f32 / 86_400_000.0;
            let weight = (count / 10.0).min(1.0) * (-age_days / 30.0f32).exp();
            Some((to_id, weight))
        }).collect();

        Ok(neighbors)
    }

    /// Batch time-decayed co-play neighbors for multiple seed tracks.
    ///
    /// Returns a map of `to_track_id → max_weight_across_seeds`.
    /// Only entries with count ≥ 5 are included.
    pub fn batch_get_played_after_neighbors(
        db: &MeshDb,
        seed_ids: &[i64],
        limit_per_seed: usize,
    ) -> Result<HashMap<i64, f32>, DbError> {
        if seed_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut params = BTreeMap::new();
        params.insert("from_ids".to_string(), DataValue::List(
            seed_ids.iter().map(|&id| DataValue::from(id)).collect(),
        ));
        // Note: CozoDB has no per-key limit; fetch all and limit in Rust
        let _ = limit_per_seed; // documented: used as hint only

        let result = db.run_query(r#"
            ?[from_id, to_id, count, last_played_epoch] :=
                *played_after{from_id, to_id, count, last_played_epoch},
                is_in(from_id, $from_ids)
        "#, params)?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let mut map: HashMap<i64, f32> = HashMap::new();
        for row in &result.rows {
            let to_id      = match row.get(1).and_then(|v| v.get_int()) { Some(v) => v, None => continue };
            let count      = match row.get(2).and_then(|v| v.get_int()) { Some(v) => v as f32, None => continue };
            let last_epoch = match row.get(3).and_then(|v| v.get_int()) { Some(v) => v, None => continue };

            if count < 5.0 { continue; }

            let age_days = (now_ms - last_epoch).max(0) as f32 / 86_400_000.0;
            let weight = (count / 10.0).min(1.0) * (-age_days / 30.0f32).exp();
            map.entry(to_id).and_modify(|v| *v = v.max(weight)).or_insert(weight);
        }

        Ok(map)
    }

    /// Get all track paths played in a session (for suggestion filtering and browser dimming)
    pub fn get_session_played_paths(db: &MeshDb, session_id: i64) -> Result<HashSet<String>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("session_id".to_string(), DataValue::from(session_id));

        let result = db.run_query(r#"
            ?[track_path] :=
                *track_plays{session_id: $session_id, track_path, play_started_at},
                is_not_null(play_started_at)
        "#, params)?;

        Ok(result.rows.iter()
            .filter_map(|row| row.first()?.get_str().map(|s| s.to_string()))
            .collect())
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
            title: row.get(3)?.get_str()?.to_string(),
            original_name: row.get(4)?.get_str().unwrap_or("").to_string(),
            artist: row.get(5)?.get_str().map(|s| s.to_string()),
            bpm: row.get(6)?.get_float(),
            original_bpm: row.get(7)?.get_float(),
            key: row.get(8)?.get_str().map(|s| s.to_string()),
            duration_seconds: row.get(9)?.get_float().unwrap_or(0.0),
            lufs: row.get(10)?.get_float().map(|f| f as f32),
            integrated_lufs: row.get(11)?.get_float().map(|f| f as f32),
            drop_marker: row.get(12)?.get_int(),
            first_beat_sample: row.get(13)?.get_int().unwrap_or(0),
            file_mtime: row.get(14)?.get_int().unwrap_or(0),
            file_size: row.get(15)?.get_int().unwrap_or(0),
            waveform_path: row.get(16)?.get_str().map(|s| s.to_string()),
        })
    }).collect()
}

fn rows_to_tracks_with_distance(result: &NamedRows) -> Vec<(TrackRow, f32)> {
    result.rows.iter().filter_map(|row| {
        let track = TrackRow {
            id: row.get(0)?.get_int()?,
            path: row.get(1)?.get_str()?.to_string(),
            folder_path: row.get(2)?.get_str()?.to_string(),
            title: row.get(3)?.get_str()?.to_string(),
            original_name: row.get(4)?.get_str().unwrap_or("").to_string(),
            artist: row.get(5)?.get_str().map(|s| s.to_string()),
            bpm: row.get(6)?.get_float(),
            original_bpm: row.get(7)?.get_float(),
            key: row.get(8)?.get_str().map(|s| s.to_string()),
            duration_seconds: row.get(9)?.get_float().unwrap_or(0.0),
            lufs: row.get(10)?.get_float().map(|f| f as f32),
            integrated_lufs: row.get(11)?.get_float().map(|f| f as f32),
            drop_marker: row.get(12)?.get_int(),
            first_beat_sample: row.get(13)?.get_int().unwrap_or(0),
            file_mtime: row.get(14)?.get_int().unwrap_or(0),
            file_size: row.get(15)?.get_int().unwrap_or(0),
            waveform_path: row.get(16)?.get_str().map(|s| s.to_string()),
        };
        let distance = row.get(17)?.get_float()? as f32;
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
            path: "/music/track1.flac".to_string(),
            folder_path: "/music".to_string(),
            title: "Test Track".to_string(),
            original_name: String::new(),
            artist: Some("Test Artist".to_string()),
            bpm: Some(128.0),
            original_bpm: Some(128.0),
            key: Some("8A".to_string()),
            duration_seconds: 180.0,
            lufs: Some(-8.0),
            integrated_lufs: Some(-10.0),
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
        assert_eq!(retrieved.title, "Test Track");
        assert_eq!(retrieved.artist, Some("Test Artist".to_string()));

        // Count
        assert_eq!(TrackQuery::count(&db).unwrap(), 1);

        // Delete
        TrackQuery::delete(&db, 1).unwrap();
        assert_eq!(TrackQuery::count(&db).unwrap(), 0);
    }
}
