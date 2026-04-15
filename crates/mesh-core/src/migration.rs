//! One-time DB migration: absolute-path track IDs → relative-path track IDs
//!
//! ## Why
//!
//! Old track IDs were `hash(absolute_path)` — e.g. hash("/media/usb-abc/MESH/tracks/a.flac").
//! If a USB stick is mounted at a different path the ID changes, breaking `played_after`
//! history and HNSW cross-session continuity.
//!
//! New IDs are `hash(relative_path)` — e.g. hash("tracks/a.flac"). The same track gets
//! the same ID on any mount point and across any DB that uses the same collection layout.
//!
//! ## What gets migrated
//!
//! Every table that stores a track ID as a key or value:
//!
//! Keys  : tracks, cue_points, saved_loops, stem_links, ml_analysis, track_tags,
//!         audio_features (vec), ml_embeddings (vec), ml_pca_embeddings (vec),
//!         stem_energy, track_dissonance, playlist_tracks, similar_to, harmonic_match
//! Values: stem_links.source_track_id, track_plays.track_id, played_after (both keys),
//!         track_plays.played_with_json (embedded JSON IDs)
//!
//! ## Safety
//!
//! - Run this after backing up mesh.db.
//! - Detects ID conflicts (new_id collides with an unchanged track's old_id) and uses
//!   a two-pass strategy (old → temp → new) to avoid data loss.
//! - Idempotent: running it twice on an already-migrated DB is safe (reports 0 changes).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use cozo::DataValue;

use crate::db::{DatabaseService, DbError};
use crate::db::MeshDb;

/// Run the full ID migration on the database at `collection_root`.
///
/// Returns `(changed, skipped)` — the number of tracks whose IDs were updated
/// and the number that were already correct.
pub fn run_migration(collection_root: &Path) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let service = DatabaseService::new(collection_root)?;
    run_migration_on_service(&service)
}

/// Same as `run_migration` but accepts an already-open `DatabaseService`.
pub fn run_migration_on_service(service: &Arc<DatabaseService>) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    println!("Opening database at {:?}", service.collection_root());

    // ── Step 1: enumerate all tracks ─────────────────────────────────────────
    let all_tracks = service.get_all_track_ids_and_paths()?;
    println!("Found {} tracks in database.", all_tracks.len());

    // ── Step 2: compute new IDs ───────────────────────────────────────────────
    let mut old_to_new: Vec<(i64, i64)> = Vec::new();
    for (old_id, path) in &all_tracks {
        let new_id = service.compute_stable_track_id(path);
        if *old_id != new_id {
            old_to_new.push((*old_id, new_id));
        }
    }

    let skipped = all_tracks.len() - old_to_new.len();
    if old_to_new.is_empty() {
        println!("✓ All {} track IDs are already stable. No migration needed.", all_tracks.len());
        return Ok((0, skipped));
    }

    println!("{} tracks need ID updates, {} already correct.", old_to_new.len(), skipped);

    // ── Step 3: detect conflicts ──────────────────────────────────────────────
    // A conflict is when a new_id equals an old_id that is NOT itself being migrated
    // (i.e. it belongs to a stable track). Migrating in a single pass would silently
    // overwrite that stable track's data.
    let changing_old_ids: HashSet<i64> = old_to_new.iter().map(|(o, _)| *o).collect();
    let stable_old_ids: HashSet<i64> = all_tracks.iter()
        .map(|(id, _)| *id)
        .filter(|id| !changing_old_ids.contains(id))
        .collect();
    let has_conflict = old_to_new.iter().any(|(_, new_id)| stable_old_ids.contains(new_id));

    if has_conflict {
        println!("⚠  ID conflicts detected — using two-pass migration (old → temp → new).");
        // Pass 1: old → temp (offset by a large value to avoid all collisions)
        const TEMP_OFFSET: i64 = 9_000_000_000_000_000_000i64; // near i64::MAX/2
        let temp_pairs: Vec<(i64, i64)> = old_to_new.iter()
            .map(|(old, _)| (*old, old.wrapping_add(TEMP_OFFSET)))
            .collect();
        migrate_pass(service.db(), &service, &temp_pairs)?;

        let final_pairs: Vec<(i64, i64)> = old_to_new.iter()
            .map(|(old, new)| (old.wrapping_add(TEMP_OFFSET), *new))
            .collect();
        migrate_pass(service.db(), &service, &final_pairs)?;
    } else {
        migrate_pass(service.db(), &service, &old_to_new)?;
    }

    // ── Step 4: remap played_with_json ────────────────────────────────────────
    let id_map: HashMap<i64, i64> = old_to_new.iter().copied().collect();
    remap_played_with_json(service.db(), &id_map)?;

    // ── Step 5: rebuild played_after graph ────────────────────────────────────
    // Easier than remapping individual edges — the graph is fully derived from track_plays.
    clear_played_after(service.db())?;
    service.build_played_after_graph()?;

    let changed = old_to_new.len();
    println!("✓ Migration complete: {} tracks updated.", changed);
    Ok((changed, skipped))
}

// ── Per-track migration pass ──────────────────────────────────────────────────

fn migrate_pass(
    db: &MeshDb,
    service: &Arc<DatabaseService>,
    pairs: &[(i64, i64)],
) -> Result<(), DbError> {
    for (i, &(old_id, new_id)) in pairs.iter().enumerate() {
        if (i + 1) % 100 == 0 || i + 1 == pairs.len() {
            println!("  Migrating track IDs: {}/{}", i + 1, pairs.len());
        }

        migrate_one_track(db, service, old_id, new_id)?;
    }

    // Remap value-side references (source_track_id in stem_links) for the whole batch
    // after all key migrations are done, to avoid stale references during the key phase.
    for &(old_id, new_id) in pairs {
        remap_stem_link_source(db, old_id, new_id)?;
        remap_track_play_track_id(db, old_id, new_id)?;
    }

    Ok(())
}

fn migrate_one_track(
    db: &MeshDb,
    service: &Arc<DatabaseService>,
    old_id: i64,
    new_id: i64,
) -> Result<(), DbError> {
    let mut p = BTreeMap::new();
    p.insert("old_id".to_string(), DataValue::from(old_id));
    p.insert("new_id".to_string(), DataValue::from(new_id));

    // ── tracks ────────────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm,
          key, duration_seconds, lufs, integrated_lufs, drop_marker,
          first_beat_sample, file_mtime, file_size, waveform_path] :=
            *tracks{id: o, path, folder_path, title, original_name, artist, bpm,
                    original_bpm, key, duration_seconds, lufs, integrated_lufs,
                    drop_marker, first_beat_sample, file_mtime, file_size, waveform_path},
            o = $old_id,
            id = $new_id
        :put tracks {id => path, folder_path, title, original_name, artist, bpm,
                     original_bpm, key, duration_seconds, lufs, integrated_lufs,
                     drop_marker, first_beat_sample, file_mtime, file_size, waveform_path}
    "#, p.clone())?;
    db.run_script(r#"
        ?[id] := *tracks{id}, id = $old_id
        :rm tracks {id}
    "#, p.clone())?;

    // ── cue_points ────────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, index, sample_position, label, color] :=
            *cue_points{track_id: o, index, sample_position, label, color},
            o = $old_id, track_id = $new_id
        :put cue_points {track_id, index => sample_position, label, color}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id, index] := *cue_points{track_id, index}, track_id = $old_id
        :rm cue_points {track_id, index}
    "#, p.clone())?;

    // ── saved_loops ───────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, index, start_sample, end_sample, label, color] :=
            *saved_loops{track_id: o, index, start_sample, end_sample, label, color},
            o = $old_id, track_id = $new_id
        :put saved_loops {track_id, index => start_sample, end_sample, label, color}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id, index] := *saved_loops{track_id, index}, track_id = $old_id
        :rm saved_loops {track_id, index}
    "#, p.clone())?;

    // ── stem_links (key side) ─────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, stem_index, source_track_id, source_stem] :=
            *stem_links{track_id: o, stem_index, source_track_id, source_stem},
            o = $old_id, track_id = $new_id
        :put stem_links {track_id, stem_index => source_track_id, source_stem}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id, stem_index] := *stem_links{track_id, stem_index}, track_id = $old_id
        :rm stem_links {track_id, stem_index}
    "#, p.clone())?;

    // ── ml_analysis ───────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, vocal_presence, arousal, valence, top_genre, genre_scores_json,
          mood_scores_json, binary_moods_json, danceability, approachability,
          reverb, timbre, tonal, mood_acoustic, mood_electronic] :=
            *ml_analysis{track_id: o, vocal_presence, arousal, valence, top_genre,
                         genre_scores_json, mood_scores_json, binary_moods_json,
                         danceability, approachability, reverb, timbre, tonal,
                         mood_acoustic, mood_electronic},
            o = $old_id, track_id = $new_id
        :put ml_analysis {track_id => vocal_presence, arousal, valence, top_genre,
                          genre_scores_json, mood_scores_json, binary_moods_json,
                          danceability, approachability, reverb, timbre, tonal,
                          mood_acoustic, mood_electronic}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id] := *ml_analysis{track_id}, track_id = $old_id
        :rm ml_analysis {track_id}
    "#, p.clone())?;

    // ── track_tags ────────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, label, color, sort_order] :=
            *track_tags{track_id: o, label, color, sort_order},
            o = $old_id, track_id = $new_id
        :put track_tags {track_id, label => color, sort_order}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id, label] := *track_tags{track_id, label}, track_id = $old_id
        :rm track_tags {track_id, label}
    "#, p.clone())?;

    // ── stem_energy ───────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, vocal, drums, bass, other] :=
            *stem_energy{track_id: o, vocal, drums, bass, other},
            o = $old_id, track_id = $new_id
        :put stem_energy {track_id => vocal, drums, bass, other}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id] := *stem_energy{track_id}, track_id = $old_id
        :rm stem_energy {track_id}
    "#, p.clone())?;

    // ── track_dissonance ──────────────────────────────────────────────────────
    db.run_script(r#"
        ?[track_id, dissonance] :=
            *track_dissonance{track_id: o, dissonance},
            o = $old_id, track_id = $new_id
        :put track_dissonance {track_id => dissonance}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id] := *track_dissonance{track_id}, track_id = $old_id
        :rm track_dissonance {track_id}
    "#, p.clone())?;

    // ── playlist_tracks ───────────────────────────────────────────────────────
    db.run_script(r#"
        ?[playlist_id, track_id, sort_order] :=
            *playlist_tracks{playlist_id, track_id: o, sort_order},
            o = $old_id, track_id = $new_id
        :put playlist_tracks {playlist_id, track_id => sort_order}
    "#, p.clone())?;
    db.run_script(r#"
        ?[playlist_id, track_id] :=
            *playlist_tracks{playlist_id, track_id}, track_id = $old_id
        :rm playlist_tracks {playlist_id, track_id}
    "#, p.clone())?;

    // ── similar_to ────────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[from_track, to_track, match_type] :=
            *similar_to{from_track: o, to_track, match_type},
            o = $old_id, from_track = $new_id
        :put similar_to {from_track, to_track => match_type}
    "#, p.clone())?;
    db.run_script(r#"
        ?[from_track, to_track, match_type] :=
            *similar_to{from_track, to_track: o, match_type},
            o = $old_id, to_track = $new_id
        :put similar_to {from_track, to_track => match_type}
    "#, p.clone())?;
    db.run_script(r#"
        ?[from_track, to_track] :=
            *similar_to{from_track, to_track},
            or(from_track = $old_id, to_track = $old_id)
        :rm similar_to {from_track, to_track}
    "#, p.clone())?;

    // ── harmonic_match ────────────────────────────────────────────────────────
    db.run_script(r#"
        ?[from_track, to_track, match_type] :=
            *harmonic_match{from_track: o, to_track, match_type},
            o = $old_id, from_track = $new_id
        :put harmonic_match {from_track, to_track => match_type}
    "#, p.clone())?;
    db.run_script(r#"
        ?[from_track, to_track, match_type] :=
            *harmonic_match{from_track, to_track: o, match_type},
            o = $old_id, to_track = $new_id
        :put harmonic_match {from_track, to_track => match_type}
    "#, p.clone())?;
    db.run_script(r#"
        ?[from_track, to_track] :=
            *harmonic_match{from_track, to_track},
            or(from_track = $old_id, to_track = $old_id)
        :rm harmonic_match {from_track, to_track}
    "#, p.clone())?;

    // ── vector tables (round-trip via service methods) ────────────────────────
    // CozoDB F32 vectors don't survive transparent re-put via Datalog scripts,
    // so we read them in Rust and write back with the new key.
    if let Ok(Some(vec)) = service.get_audio_features(old_id) {
        let _ = service.store_audio_features(new_id, &vec);
    }
    if let Ok(Some(emb)) = service.get_ml_embedding_raw(old_id) {
        let _ = service.store_ml_embedding(new_id, &emb);
    }
    if let Ok(Some(pca)) = service.get_pca_embedding_raw(old_id) {
        let _ = service.store_pca_embedding(new_id, &pca);
    }
    // Delete old vector rows
    db.run_script(r#"
        ?[track_id] := *audio_features{track_id}, track_id = $old_id
        :rm audio_features {track_id}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id] := *ml_embeddings{track_id}, track_id = $old_id
        :rm ml_embeddings {track_id}
    "#, p.clone())?;
    db.run_script(r#"
        ?[track_id] := *ml_pca_embeddings{track_id}, track_id = $old_id
        :rm ml_pca_embeddings {track_id}
    "#, p.clone())?;

    Ok(())
}

// ── Value-side remaps (done after all key migrations) ─────────────────────────

fn remap_stem_link_source(db: &MeshDb, old_id: i64, new_id: i64) -> Result<(), DbError> {
    let mut p = BTreeMap::new();
    p.insert("old_id".to_string(), DataValue::from(old_id));
    p.insert("new_id".to_string(), DataValue::from(new_id));
    // Update source_track_id value for any stem_link pointing at old_id
    db.run_script(r#"
        ?[track_id, stem_index, source_track_id, source_stem] :=
            *stem_links{track_id, stem_index, source_track_id: o, source_stem},
            o = $old_id, source_track_id = $new_id
        :put stem_links {track_id, stem_index => source_track_id, source_stem}
    "#, p)
    .map(|_| ())
}

fn remap_track_play_track_id(db: &MeshDb, old_id: i64, new_id: i64) -> Result<(), DbError> {
    let mut p = BTreeMap::new();
    p.insert("old_id".to_string(), DataValue::from(old_id));
    p.insert("new_id".to_string(), DataValue::from(new_id));
    // Update track_id value field in track_plays
    db.run_script(r#"
        ?[session_id, loaded_at, track_id] :=
            *track_plays{session_id, loaded_at, track_id: o},
            o = $old_id, track_id = $new_id
        :update track_plays {session_id, loaded_at => track_id}
    "#, p)
    .map(|_| ())
}

// ── played_with_json remapping ────────────────────────────────────────────────

fn remap_played_with_json(db: &MeshDb, id_map: &HashMap<i64, i64>) -> Result<(), DbError> {
    if id_map.is_empty() { return Ok(()); }

    // Load all track_plays rows that have a non-null played_with_json
    let result = db.run_query(r#"
        ?[session_id, loaded_at, played_with_json] :=
            *track_plays{session_id, loaded_at, played_with_json},
            is_not_null(played_with_json)
    "#, BTreeMap::new())?;

    let mut updated = 0usize;
    for row in &result.rows {
        let session_id = match row.get(0).and_then(|v| v.get_int()) { Some(v) => v, None => continue };
        let loaded_at  = match row.get(1).and_then(|v| v.get_int()) { Some(v) => v, None => continue };
        let json_str   = match row.get(2).and_then(|v| v.get_str()) { Some(v) => v.to_string(), None => continue };

        // Parse as Vec<(i64, String)> (new format)
        let pairs: Vec<(i64, String)> = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(_) => continue, // old string-only format — skip
        };

        let remapped: Vec<(i64, String)> = pairs.into_iter()
            .map(|(id, name)| (id_map.get(&id).copied().unwrap_or(id), name))
            .collect();

        let new_json = serde_json::to_string(&remapped)
            .map_err(|e| DbError::Serialization(e.to_string()))?;

        let mut p = BTreeMap::new();
        p.insert("session_id".to_string(),    DataValue::from(session_id));
        p.insert("loaded_at".to_string(),     DataValue::from(loaded_at));
        p.insert("played_with_json".to_string(), DataValue::Str(new_json.into()));
        db.run_script(r#"
            ?[session_id, loaded_at, played_with_json] <-
                [[$session_id, $loaded_at, $played_with_json]]
            :update track_plays {session_id, loaded_at => played_with_json}
        "#, p)?;
        updated += 1;
    }

    if updated > 0 {
        println!("  Remapped played_with_json in {} track_play records.", updated);
    }
    Ok(())
}

// ── played_after helpers ──────────────────────────────────────────────────────

fn clear_played_after(db: &MeshDb) -> Result<(), DbError> {
    db.run_script(r#"
        ?[from_id, to_id] := *played_after{from_id, to_id}
        :rm played_after {from_id, to_id}
    "#, BTreeMap::new())
    .map(|_| ())
}
