//! Query builders and helpers for CozoDB
//!
//! This module provides typed query APIs that generate CozoScript internally.

use super::{MeshDb, DbError, Track, Playlist, AudioFeatures};
use cozo::{DataValue, NamedRows};
use std::collections::BTreeMap;

// ============================================================================
// Track Queries
// ============================================================================

/// Query builder for tracks
pub struct TrackQuery;

impl TrackQuery {
    /// Get all tracks in a folder
    pub fn get_by_folder(db: &MeshDb, folder_path: &str) -> Result<Vec<Track>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("folder".to_string(), DataValue::Str(folder_path.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                folder_path = $folder
            :order name
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Get a track by ID
    pub fn get_by_id(db: &MeshDb, track_id: i64) -> Result<Option<Track>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("id".to_string(), DataValue::from(track_id));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                id = $id
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Get a track by path
    pub fn get_by_path(db: &MeshDb, path: &str) -> Result<Option<Track>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("path".to_string(), DataValue::Str(path.into()));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                path = $path
        "#, params)?;

        Ok(rows_to_tracks(&result).into_iter().next())
    }

    /// Search tracks by name or artist
    pub fn search(db: &MeshDb, query: &str, limit: usize) -> Result<Vec<Track>, DbError> {
        let mut params = BTreeMap::new();
        let query_lower = query.to_lowercase();
        params.insert("query".to_string(), DataValue::Str(query_lower.into()));
        params.insert("limit".to_string(), DataValue::from(limit as i64));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                (lowercase(name) ~ $query or
                 (is_not_null(artist) and lowercase(artist) ~ $query))
            :limit $limit
            :order name
        "#, params)?;

        Ok(rows_to_tracks(&result))
    }

    /// Insert or update a track
    pub fn upsert(db: &MeshDb, track: &Track) -> Result<(), DbError> {
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
        params.insert("file_mtime".to_string(), DataValue::from(track.file_mtime));
        params.insert("file_size".to_string(), DataValue::from(track.file_size));
        params.insert("waveform_path".to_string(), track.waveform_path.as_ref().map(|s| DataValue::Str(s.clone().into())).unwrap_or(DataValue::Null));

        db.run_script(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] <- [[
                $id, $path, $folder_path, $name, $artist, $bpm, $original_bpm, $key,
                $duration_seconds, $lufs, $drop_marker, $file_mtime, $file_size, $waveform_path
            ]]
            :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                         duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
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
    /// Supported fields: artist, bpm, original_bpm, key, lufs, drop_marker
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
                      duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, bpm, original_bpm, key,
                                duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        artist = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
                "#
            }
            "bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid BPM value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, original_bpm, key,
                                duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        bpm = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
                "#
            }
            "original_bpm" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid original_bpm value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, key,
                                duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        original_bpm = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
                "#
            }
            "key" => {
                let val = if value.is_empty() { DataValue::Null } else { DataValue::Str(value.into()) };
                params.insert("value".to_string(), val);
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm,
                                duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        key = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
                "#
            }
            "lufs" => {
                let val: f64 = value.parse().map_err(|_| DbError::Query(format!("Invalid LUFS value: {}", value)))?;
                params.insert("value".to_string(), DataValue::from(val));
                r#"
                    ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
                      duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                        *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                                duration_seconds, drop_marker, file_mtime, file_size, waveform_path},
                        id = $id,
                        lufs = $value
                    :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key,
                                 duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
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
        let result = db.run_query(r#"
            ?[folder_path] := *tracks{folder_path}
            :order folder_path
        "#, BTreeMap::new())?;

        Ok(result.rows.into_iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str().map(|s| s.to_string())))
            .collect())
    }

    /// Count tracks in the database
    pub fn count(db: &MeshDb) -> Result<usize, DbError> {
        let result = db.run_query(r#"
            ?[count(id)] := *tracks{id}
        "#, BTreeMap::new())?;

        Ok(result.rows.first()
            .and_then(|row| row.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0) as usize)
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
    pub fn get_tracks(db: &MeshDb, playlist_id: i64) -> Result<Vec<Track>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("playlist_id".to_string(), DataValue::from(playlist_id));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *playlist_tracks{playlist_id, track_id, sort_order},
                *tracks{id: track_id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path},
                playlist_id = $playlist_id
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
    pub fn find_similar(db: &MeshDb, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("k".to_string(), DataValue::from((limit + 1) as i64)); // +1 to exclude self

        // First get the embedding for the query track, then search using HNSW
        let result = db.run_query(r#"
            ?[track_id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path, dist] :=
                *audio_features{track_id: $track_id, vec: query_vec},
                ~audio_features:similarity_index{track_id | query: query_vec, k: $k, ef: 50 | dist},
                track_id != $track_id,
                *tracks{id: track_id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
            :order dist
        "#, params)?;

        Ok(rows_to_tracks_with_distance(&result))
    }

    /// Find harmonically compatible tracks
    pub fn find_harmonic_compatible(db: &MeshDb, track_id: i64, limit: usize) -> Result<Vec<Track>, DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));
        params.insert("limit".to_string(), DataValue::from(limit as i64));

        let result = db.run_query(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *harmonic_match{from_track: $track_id, to_track: id},
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}
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
// Helper Functions
// ============================================================================

fn rows_to_tracks(result: &NamedRows) -> Vec<Track> {
    result.rows.iter().filter_map(|row| {
        Some(Track {
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
            file_mtime: row.get(11)?.get_int().unwrap_or(0),
            file_size: row.get(12)?.get_int().unwrap_or(0),
            waveform_path: row.get(13)?.get_str().map(|s| s.to_string()),
        })
    }).collect()
}

fn rows_to_tracks_with_distance(result: &NamedRows) -> Vec<(Track, f32)> {
    result.rows.iter().filter_map(|row| {
        let track = Track {
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
            file_mtime: row.get(11)?.get_int().unwrap_or(0),
            file_size: row.get(12)?.get_int().unwrap_or(0),
            waveform_path: row.get(13)?.get_str().map(|s| s.to_string()),
        };
        let distance = row.get(14)?.get_float()? as f32;
        Some((track, distance))
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

        let track = Track {
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
