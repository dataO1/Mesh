//! Batch query operations for CozoDB
//!
//! Provides efficient multi-row inserts using CozoDB's native syntax.
//! Instead of N individual inserts, we use a single query with all rows:
//!
//! ```cozoscript
//! ?[track_id, index, sample_position, label, color] <- $rows
//! :put cue_points {track_id, index => sample_position, label, color}
//! ```
//!
//! This reduces the number of database operations from 18+ per track to ~5.

use super::{CuePoint, DbError, MeshDb, SavedLoop, StemLink};
use cozo::DataValue;
use std::collections::BTreeMap;

/// Batch operations for track metadata
///
/// These methods use CozoDB's multi-row insert syntax for efficiency.
/// Each method accepts a slice of items and inserts them in a single query.
pub struct BatchQuery;

impl BatchQuery {
    /// Batch insert cue points for a track
    ///
    /// Inserts all cue points in a single CozoDB query using the `$rows` parameter.
    /// Much more efficient than individual inserts when a track has multiple cue points.
    pub fn batch_insert_cue_points(
        db: &MeshDb,
        track_id: i64,
        cues: &[CuePoint],
    ) -> Result<(), DbError> {
        if cues.is_empty() {
            return Ok(());
        }

        // Build the row data as a DataValue::List of Lists
        let rows: Vec<DataValue> = cues
            .iter()
            .map(|cue| {
                DataValue::List(vec![
                    DataValue::from(track_id),
                    DataValue::from(cue.index as i64),
                    DataValue::from(cue.sample_position),
                    cue.label
                        .as_ref()
                        .map(|s| DataValue::Str(s.clone().into()))
                        .unwrap_or(DataValue::Null),
                    cue.color
                        .as_ref()
                        .map(|s| DataValue::Str(s.clone().into()))
                        .unwrap_or(DataValue::Null),
                ])
            })
            .collect();

        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(
            r#"
            ?[track_id, index, sample_position, label, color] <- $rows
            :put cue_points {track_id, index => sample_position, label, color}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Batch insert saved loops for a track
    ///
    /// Inserts all saved loops in a single CozoDB query.
    pub fn batch_insert_saved_loops(
        db: &MeshDb,
        track_id: i64,
        loops: &[SavedLoop],
    ) -> Result<(), DbError> {
        if loops.is_empty() {
            return Ok(());
        }

        let rows: Vec<DataValue> = loops
            .iter()
            .map(|l| {
                DataValue::List(vec![
                    DataValue::from(track_id),
                    DataValue::from(l.index as i64),
                    DataValue::from(l.start_sample),
                    DataValue::from(l.end_sample),
                    l.label
                        .as_ref()
                        .map(|s| DataValue::Str(s.clone().into()))
                        .unwrap_or(DataValue::Null),
                    l.color
                        .as_ref()
                        .map(|s| DataValue::Str(s.clone().into()))
                        .unwrap_or(DataValue::Null),
                ])
            })
            .collect();

        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(
            r#"
            ?[track_id, index, start_sample, end_sample, label, color] <- $rows
            :put saved_loops {track_id, index => start_sample, end_sample, label, color}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Batch insert stem links for a track
    ///
    /// Inserts all stem links in a single CozoDB query.
    /// Note: The stem links should already have remapped source_track_id values
    /// for the target database (e.g., USB database IDs, not local database IDs).
    pub fn batch_insert_stem_links(
        db: &MeshDb,
        track_id: i64,
        links: &[StemLink],
    ) -> Result<(), DbError> {
        if links.is_empty() {
            return Ok(());
        }

        let rows: Vec<DataValue> = links
            .iter()
            .map(|l| {
                DataValue::List(vec![
                    DataValue::from(track_id),
                    DataValue::from(l.stem_index as i64),
                    DataValue::from(l.source_track_id),
                    DataValue::from(l.source_stem as i64),
                ])
            })
            .collect();

        let mut params = BTreeMap::new();
        params.insert("rows".to_string(), DataValue::List(rows));

        db.run_script(
            r#"
            ?[track_id, stem_index, source_track_id, source_stem] <- $rows
            :put stem_links {track_id, stem_index => source_track_id, source_stem}
        "#,
            params,
        )?;

        Ok(())
    }

    /// Delete all metadata for a track (cue_points, saved_loops, stem_links)
    ///
    /// Removes all associated metadata in 3 queries (one per relation).
    /// This is called before batch inserting new metadata to avoid duplicates.
    pub fn batch_delete_track_metadata(db: &MeshDb, track_id: i64) -> Result<(), DbError> {
        let mut params = BTreeMap::new();
        params.insert("track_id".to_string(), DataValue::from(track_id));

        // Delete cue points
        db.run_script(
            r#"
            ?[track_id, index] := *cue_points{track_id, index}, track_id = $track_id
            :rm cue_points {track_id, index}
        "#,
            params.clone(),
        )?;

        // Delete saved loops
        db.run_script(
            r#"
            ?[track_id, index] := *saved_loops{track_id, index}, track_id = $track_id
            :rm saved_loops {track_id, index}
        "#,
            params.clone(),
        )?;

        // Delete stem links
        db.run_script(
            r#"
            ?[track_id, stem_index] := *stem_links{track_id, stem_index}, track_id = $track_id
            :rm stem_links {track_id, stem_index}
        "#,
            params,
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_insert_cue_points() {
        let db = MeshDb::in_memory().unwrap();

        // First we need a track to reference
        db.run_script(
            r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
                1, "/test/track.wav", "/test", "Test Track", null, 120.0, 120.0, "Am", 180.0, null, null, 0, 0, 0, null
            ]]
            :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
        "#,
            BTreeMap::new(),
        )
        .unwrap();

        // Insert 8 cue points in one batch
        let cues = vec![
            CuePoint { track_id: 1, index: 0, sample_position: 100000, label: Some("Intro".to_string()), color: Some("#FF0000".to_string()) },
            CuePoint { track_id: 1, index: 1, sample_position: 200000, label: Some("Verse".to_string()), color: Some("#00FF00".to_string()) },
            CuePoint { track_id: 1, index: 2, sample_position: 300000, label: Some("Chorus".to_string()), color: Some("#0000FF".to_string()) },
            CuePoint { track_id: 1, index: 3, sample_position: 400000, label: None, color: None },
            CuePoint { track_id: 1, index: 4, sample_position: 500000, label: Some("Bridge".to_string()), color: None },
            CuePoint { track_id: 1, index: 5, sample_position: 600000, label: None, color: Some("#FFFF00".to_string()) },
            CuePoint { track_id: 1, index: 6, sample_position: 700000, label: Some("Drop".to_string()), color: Some("#FF00FF".to_string()) },
            CuePoint { track_id: 1, index: 7, sample_position: 800000, label: Some("Outro".to_string()), color: Some("#00FFFF".to_string()) },
        ];

        BatchQuery::batch_insert_cue_points(&db, 1, &cues).unwrap();

        // Verify all cue points were inserted
        let result = db.run_query(
            "?[track_id, index, sample_position, label, color] := *cue_points{track_id, index, sample_position, label, color}, track_id = 1",
            BTreeMap::new(),
        ).unwrap();

        assert_eq!(result.rows.len(), 8);
    }

    #[test]
    fn test_batch_insert_saved_loops() {
        let db = MeshDb::in_memory().unwrap();

        // Create track
        db.run_script(
            r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
                1, "/test/track.wav", "/test", "Test Track", null, 120.0, 120.0, "Am", 180.0, null, null, 0, 0, 0, null
            ]]
            :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
        "#,
            BTreeMap::new(),
        )
        .unwrap();

        // Insert 4 saved loops
        let loops = vec![
            SavedLoop { track_id: 1, index: 0, start_sample: 100000, end_sample: 200000, label: Some("Loop A".to_string()), color: Some("#FF0000".to_string()) },
            SavedLoop { track_id: 1, index: 1, start_sample: 200000, end_sample: 300000, label: None, color: None },
            SavedLoop { track_id: 1, index: 2, start_sample: 300000, end_sample: 400000, label: Some("Loop C".to_string()), color: None },
            SavedLoop { track_id: 1, index: 3, start_sample: 400000, end_sample: 500000, label: None, color: Some("#00FF00".to_string()) },
        ];

        BatchQuery::batch_insert_saved_loops(&db, 1, &loops).unwrap();

        // Verify
        let result = db.run_query(
            "?[track_id, index] := *saved_loops{track_id, index}, track_id = 1",
            BTreeMap::new(),
        ).unwrap();

        assert_eq!(result.rows.len(), 4);
    }

    #[test]
    fn test_batch_delete_track_metadata() {
        let db = MeshDb::in_memory().unwrap();

        // Create track
        db.run_script(
            r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path] <- [[
                1, "/test/track.wav", "/test", "Test Track", null, 120.0, 120.0, "Am", 180.0, null, null, 0, 0, 0, null
            ]]
            :put tracks {id => path, folder_path, name, artist, bpm, original_bpm, key, duration_seconds, lufs, drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
        "#,
            BTreeMap::new(),
        )
        .unwrap();

        // Insert some cue points and loops
        let cues = vec![
            CuePoint { track_id: 1, index: 0, sample_position: 100000, label: Some("Test".to_string()), color: None },
            CuePoint { track_id: 1, index: 1, sample_position: 200000, label: None, color: None },
        ];
        let loops = vec![
            SavedLoop { track_id: 1, index: 0, start_sample: 100000, end_sample: 200000, label: None, color: None },
        ];

        BatchQuery::batch_insert_cue_points(&db, 1, &cues).unwrap();
        BatchQuery::batch_insert_saved_loops(&db, 1, &loops).unwrap();

        // Verify they exist
        let cue_count = db.run_query(
            "?[count(index)] := *cue_points{track_id, index}, track_id = 1",
            BTreeMap::new(),
        ).unwrap();
        assert_eq!(cue_count.rows[0][0], DataValue::from(2i64));

        // Delete all metadata
        BatchQuery::batch_delete_track_metadata(&db, 1).unwrap();

        // Verify deletion
        let cue_count_after = db.run_query(
            "?[count(index)] := *cue_points{track_id, index}, track_id = 1",
            BTreeMap::new(),
        ).unwrap();
        assert_eq!(cue_count_after.rows[0][0], DataValue::from(0i64));

        let loop_count_after = db.run_query(
            "?[count(index)] := *saved_loops{track_id, index}, track_id = 1",
            BTreeMap::new(),
        ).unwrap();
        assert_eq!(loop_count_after.rows[0][0], DataValue::from(0i64));
    }

    #[test]
    fn test_empty_batch_is_noop() {
        let db = MeshDb::in_memory().unwrap();

        // These should succeed without errors even with empty slices
        BatchQuery::batch_insert_cue_points(&db, 1, &[]).unwrap();
        BatchQuery::batch_insert_saved_loops(&db, 1, &[]).unwrap();
        BatchQuery::batch_insert_stem_links(&db, 1, &[]).unwrap();
    }
}
