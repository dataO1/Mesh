//! CozoDB schema definitions for mesh-core
//!
//! This module defines the database schema using Rust structs that map to
//! CozoDB relations. The schema includes:
//!
//! - Core relations: tracks, playlists, cue_points, saved_loops
//! - Graph edges: similar_to, played_after, harmonic_match
//! - Vector index: tracks:audio_features (HNSW)

use cozo::DbInstance;
use serde::{Deserialize, Serialize};
use super::DbError;

// ============================================================================
// Core Data Types
// ============================================================================

/// Internal database row representation of a track
///
/// This is the raw database schema - use `Track` from service.rs for the public API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackRow {
    pub id: i64,
    pub path: String,
    pub folder_path: String,
    pub title: String,
    pub original_name: String,
    pub artist: Option<String>,
    pub bpm: Option<f64>,
    pub original_bpm: Option<f64>,
    pub key: Option<String>,
    pub duration_seconds: f64,
    pub lufs: Option<f32>,
    pub integrated_lufs: Option<f32>,
    pub drop_marker: Option<i64>,
    /// First beat position in samples (for beat grid regeneration)
    pub first_beat_sample: i64,
    pub file_mtime: i64,
    pub file_size: i64,
    pub waveform_path: Option<String>,
}

/// A playlist (can be nested)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sort_order: i32,
}

/// Association between playlist and track
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistTrack {
    pub playlist_id: i64,
    pub track_id: i64,
    pub sort_order: i32,
}

/// A cue point on a track (database format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuePoint {
    pub track_id: i64,
    pub index: u8,
    pub sample_position: i64,
    pub label: Option<String>,
    pub color: Option<String>,
}

impl CuePoint {
    /// Create from runtime CuePoint format
    ///
    /// Converts from audio_file::CuePoint (u64 sample, String label)
    /// to database format (i64 sample, Option<String> label).
    pub fn from_runtime(track_id: i64, cue: &crate::audio_file::CuePoint) -> Self {
        Self {
            track_id,
            index: cue.index,
            sample_position: cue.sample_position as i64,
            label: if cue.label.is_empty() { None } else { Some(cue.label.clone()) },
            color: cue.color.clone(),
        }
    }
}

/// A saved loop on a track (database format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedLoop {
    pub track_id: i64,
    pub index: u8,
    pub start_sample: i64,
    pub end_sample: i64,
    pub label: Option<String>,
    pub color: Option<String>,
}

impl SavedLoop {
    /// Create from runtime SavedLoop format
    ///
    /// Converts from audio_file::SavedLoop (u64 samples)
    /// to database format (i64 samples).
    pub fn from_runtime(track_id: i64, loop_: &crate::audio_file::SavedLoop) -> Self {
        Self {
            track_id,
            index: loop_.index,
            start_sample: loop_.start_sample as i64,
            end_sample: loop_.end_sample as i64,
            label: if loop_.label.is_empty() { None } else { Some(loop_.label.clone()) },
            color: loop_.color.clone(),
        }
    }
}

/// A stem link for prepared mode (linking stems between tracks)
///
/// Allows replacing a stem (e.g., drums) from one track with another track's stem,
/// aligned at their respective drop markers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StemLink {
    /// Track that owns this link
    pub track_id: i64,
    /// Which stem slot is being replaced (0=vocals, 1=drums, 2=bass, 3=other)
    pub stem_index: u8,
    /// Source track providing the replacement stem
    pub source_track_id: i64,
    /// Which stem to use from source (0=vocals, 1=drums, 2=bass, 3=other)
    pub source_stem: u8,
}

// ============================================================================
// Graph Edge Types
// ============================================================================

/// Similarity edge between two tracks (computed from vector search)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarTo {
    pub from_track: i64,
    pub to_track: i64,
    pub similarity_score: f32,
}

// ============================================================================
// Session History Types
// ============================================================================

/// A DJ session (one continuous play session, started each time mesh-player launches)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Unix millisecond timestamp at session start — also serves as unique session ID
    pub id: i64,
    /// Unix millisecond timestamp when session ended (None if still active)
    pub ended_at: Option<i64>,
}

/// Data captured when a track is first loaded to a deck
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackPlayRecord {
    pub session_id: i64,
    /// Unix ms timestamp of load — composite key with session_id
    pub loaded_at: i64,
    pub track_path: String,
    /// "{artist} - {title}" display name, self-contained for set reconstruction
    pub track_name: String,
    pub track_id: Option<i64>,
    pub deck_index: u8,
    /// "browser" or "suggestions"
    pub load_source: String,
    pub suggestion_score: Option<f32>,
    /// JSON: [["Key ▲", "#2d8a4e"], ...]
    pub suggestion_tags_json: Option<String>,
    pub suggestion_energy_dir: Option<f32>,
}

/// Playback data filled in after play starts and when track is finalized
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackPlayUpdate {
    pub play_started_at: Option<i64>,
    pub play_start_sample: Option<i64>,
    pub play_ended_at: Option<i64>,
    pub seconds_played: Option<f32>,
    pub hot_cues_used_json: Option<String>,
    pub loop_was_active: bool,
    /// JSON: ["Artist A - Title X", "Artist B - Title Y"]
    pub played_with_json: Option<String>,
}

/// Harmonic compatibility type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarmonicMatchType {
    /// Same key
    Same,
    /// Adjacent on Camelot wheel (e.g., 8A to 8B or 7A)
    Adjacent,
    /// Energy boost (+1 on wheel)
    EnergyBoost,
    /// Energy drop (-1 on wheel)
    EnergyDrop,
}

impl HarmonicMatchType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Adjacent => "adjacent",
            Self::EnergyBoost => "energy_boost",
            Self::EnergyDrop => "energy_drop",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "same" => Some(Self::Same),
            "adjacent" => Some(Self::Adjacent),
            "energy_boost" => Some(Self::EnergyBoost),
            "energy_drop" => Some(Self::EnergyDrop),
            _ => None,
        }
    }
}

/// Harmonic compatibility between two tracks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarmonicMatch {
    pub from_track: i64,
    pub to_track: i64,
    pub match_type: HarmonicMatchType,
}

// ============================================================================
// Audio Feature Vector
// ============================================================================

/// Audio features extracted from a track (16 dimensions)
///
/// Used for vector similarity search to find musically similar tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFeatures {
    // Rhythm (4 dims)
    /// BPM normalized to [0,1] range: (bpm - 60) / 140
    pub bpm_normalized: f32,
    /// Confidence of BPM detection
    pub bpm_confidence: f32,
    /// Beat strength / percussiveness
    pub beat_strength: f32,
    /// Rhythm regularity (how consistent the beat is)
    pub rhythm_regularity: f32,

    // Harmony (4 dims)
    /// Key encoded as cos(key * 2π / 12) for circular similarity
    pub key_x: f32,
    /// Key encoded as sin(key * 2π / 12) for circular similarity
    pub key_y: f32,
    /// Mode: 0.0 = minor, 1.0 = major
    pub mode: f32,
    /// Harmonic complexity / chord variety
    pub harmonic_complexity: f32,

    // Energy (4 dims)
    /// LUFS normalized to [0,1] range
    pub lufs_normalized: f32,
    /// Dynamic range (compression level)
    pub dynamic_range: f32,
    /// Mean RMS energy
    pub energy_mean: f32,
    /// Energy variance (how much energy changes)
    pub energy_variance: f32,

    // Timbre (4 dims)
    /// Spectral centroid (brightness)
    pub spectral_centroid: f32,
    /// Spectral bandwidth (frequency spread)
    pub spectral_bandwidth: f32,
    /// Spectral rolloff (high frequency content)
    pub spectral_rolloff: f32,
    /// MFCC flatness (noisiness vs tonality)
    pub mfcc_flatness: f32,
}

impl AudioFeatures {
    /// Convert to a 16-dimensional vector for HNSW indexing
    pub fn to_vector(&self) -> Vec<f64> {
        vec![
            self.bpm_normalized as f64,
            self.bpm_confidence as f64,
            self.beat_strength as f64,
            self.rhythm_regularity as f64,
            self.key_x as f64,
            self.key_y as f64,
            self.mode as f64,
            self.harmonic_complexity as f64,
            self.lufs_normalized as f64,
            self.dynamic_range as f64,
            self.energy_mean as f64,
            self.energy_variance as f64,
            self.spectral_centroid as f64,
            self.spectral_bandwidth as f64,
            self.spectral_rolloff as f64,
            self.mfcc_flatness as f64,
        ]
    }

    /// Create from a 16-dimensional vector
    pub fn from_vector(v: &[f64]) -> Option<Self> {
        if v.len() != 16 {
            return None;
        }
        Some(Self {
            bpm_normalized: v[0] as f32,
            bpm_confidence: v[1] as f32,
            beat_strength: v[2] as f32,
            rhythm_regularity: v[3] as f32,
            key_x: v[4] as f32,
            key_y: v[5] as f32,
            mode: v[6] as f32,
            harmonic_complexity: v[7] as f32,
            lufs_normalized: v[8] as f32,
            dynamic_range: v[9] as f32,
            energy_mean: v[10] as f32,
            energy_variance: v[11] as f32,
            spectral_centroid: v[12] as f32,
            spectral_bandwidth: v[13] as f32,
            spectral_rolloff: v[14] as f32,
            mfcc_flatness: v[15] as f32,
        })
    }
}

// ============================================================================
// ML Analysis Data
// ============================================================================

/// ML analysis results for a track (voice detection, genre, mood, audio characteristics)
///
/// This struct is used across crates (mesh-core, mesh-cue, mesh-player) to pass
/// ML analysis results. It has no iced dependencies.
///
/// All probability/score fields are `Option<f32>` in 0.0–1.0 range.
/// Tags are derived from these at display time with appropriate thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MlAnalysisData {
    /// Vocal presence probability (0.0 = instrumental, 1.0 = definitely vocal)
    pub vocal_presence: f32,
    /// Legacy arousal field (no longer written, kept for DB compat)
    pub arousal: Option<f32>,
    /// Legacy valence field (no longer written, kept for DB compat)
    pub valence: Option<f32>,
    /// Primary genre label (highest confidence)
    pub top_genre: Option<String>,
    /// Top genre scores above threshold: Vec<(label, confidence)> serialized as JSON
    pub genre_scores: Vec<(String, f32)>,
    /// Jamendo mood/theme tags: Vec<(label, confidence)>
    pub mood_themes: Option<Vec<(String, f32)>>,
    /// Binary mood classifier probabilities: Vec<(label, probability)>
    pub binary_moods: Option<Vec<(String, f32)>>,
    /// Danceability probability (0.0 = not danceable, 1.0 = very danceable)
    pub danceability: Option<f32>,
    /// Music approachability regression score (0.0–1.0)
    pub approachability: Option<f32>,
    /// Reverb "wetness" probability (0.0 = dry, 1.0 = wet/reverberant)
    pub reverb: Option<f32>,
    /// Timbre brightness probability (0.0 = dark, 1.0 = bright)
    pub timbre: Option<f32>,
    /// Tonality probability (0.0 = atonal, 1.0 = tonal)
    pub tonal: Option<f32>,
    /// Acoustic sound probability (0.0 = non-acoustic, 1.0 = acoustic)
    pub mood_acoustic: Option<f32>,
    /// Electronic sound probability (0.0 = non-electronic, 1.0 = electronic)
    pub mood_electronic: Option<f32>,
}

// ============================================================================
// Schema Creation
// ============================================================================

/// Get the set of existing relation names in the database
fn get_existing_relations(db: &DbInstance) -> Result<std::collections::HashSet<String>, DbError> {
    let result = db
        .run_script("::relations", Default::default(), cozo::ScriptMutability::Immutable)
        .map_err(|e| DbError::Schema(e.to_string()))?;

    // The result has columns [name, arity, access_level, description]
    // We want the first column (name)
    let mut relations = std::collections::HashSet::new();
    for row in result.rows {
        if let Some(name) = row.first().and_then(|v| v.get_str()) {
            relations.insert(name.to_string());
        }
    }
    Ok(relations)
}

/// Create all required relations in the database (idempotent)
///
/// This function checks which relations already exist and only creates
/// missing ones. Safe to call multiple times.
pub fn create_all_relations(db: &DbInstance) -> Result<(), DbError> {
    // Get existing relations to avoid "already exists" errors
    let existing = get_existing_relations(db)?;
    log::debug!("Existing relations: {:?}", existing);

    // Clean up stale temp relations from interrupted migrations.
    // Avoid underscore-prefixed names — CozoDB treats them as session-scoped.
    for stale in &["tracks_staging", "tracks_old"] {
        if existing.contains(*stale) {
            log::warn!("Found stale '{}' from interrupted migration, removing", stale);
            let _ = db.run_script(
                &format!("::remove {}", stale),
                Default::default(),
                cozo::ScriptMutability::Mutable,
            );
        }
    }

    // Core relations
    if !existing.contains("tracks") {
        log::debug!("Creating 'tracks' relation");
        create_tracks_relation(db)?;
    } else {
        migrate_tracks_if_needed(db)?;
    }
    if !existing.contains("playlists") {
        log::debug!("Creating 'playlists' relation");
        create_playlists_relation(db)?;
    }
    if !existing.contains("playlist_tracks") {
        log::debug!("Creating 'playlist_tracks' relation");
        create_playlist_tracks_relation(db)?;
    }
    if !existing.contains("cue_points") {
        log::debug!("Creating 'cue_points' relation");
        create_cue_points_relation(db)?;
    }
    if !existing.contains("saved_loops") {
        log::debug!("Creating 'saved_loops' relation");
        create_saved_loops_relation(db)?;
    }
    if !existing.contains("stem_links") {
        log::debug!("Creating 'stem_links' relation");
        create_stem_links_relation(db)?;
    }

    // Graph relations
    if !existing.contains("similar_to") {
        log::debug!("Creating 'similar_to' relation");
        create_similar_to_relation(db)?;
    }
    if !existing.contains("harmonic_match") {
        log::debug!("Creating 'harmonic_match' relation");
        create_harmonic_match_relation(db)?;
    }

    // History relations
    if !existing.contains("sessions") {
        log::debug!("Creating 'sessions' relation");
        create_sessions_relation(db)?;
    }
    if !existing.contains("track_plays") {
        log::debug!("Creating 'track_plays' relation");
        create_track_plays_relation(db)?;
    }

    if !existing.contains("track_tags") {
        log::debug!("Creating 'track_tags' relation");
        create_track_tags_relation(db)?;
    }

    // ML analysis relation
    if !existing.contains("ml_analysis") {
        log::debug!("Creating 'ml_analysis' relation");
        create_ml_analysis_relation(db)?;
    } else {
        migrate_ml_analysis_if_needed(db)?;
    }

    // Vector index (HNSW) - check for audio_features relation
    if !existing.contains("audio_features") {
        log::debug!("Creating 'audio_features' HNSW index");
        create_audio_features_index(db)?;
    }

    Ok(())
}

fn run_schema(db: &DbInstance, script: &str) -> Result<(), DbError> {
    db.run_script(script, Default::default(), cozo::ScriptMutability::Mutable)
        .map_err(|e| DbError::Schema(e.to_string()))?;
    Ok(())
}

fn create_tracks_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create tracks {
            id: Int =>
            path: String,
            folder_path: String,
            title: String,
            original_name: String default '',
            artist: String?,
            bpm: Float?,
            original_bpm: Float?,
            key: String?,
            duration_seconds: Float,
            lufs: Float?,
            integrated_lufs: Float?,
            drop_marker: Int?,
            first_beat_sample: Int default 0,
            file_mtime: Int,
            file_size: Int,
            waveform_path: String?
        }}
    "#)
}

fn create_playlists_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create playlists {
            id: Int =>
            parent_id: Int?,
            name: String,
            sort_order: Int
        }}
    "#)
}

fn create_playlist_tracks_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create playlist_tracks {
            playlist_id: Int,
            track_id: Int =>
            sort_order: Int
        }}
    "#)
}

fn create_cue_points_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create cue_points {
            track_id: Int,
            index: Int =>
            sample_position: Int,
            label: String?,
            color: String?
        }}
    "#)
}

fn create_saved_loops_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create saved_loops {
            track_id: Int,
            index: Int =>
            start_sample: Int,
            end_sample: Int,
            label: String?,
            color: String?
        }}
    "#)
}

fn create_stem_links_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create stem_links {
            track_id: Int,
            stem_index: Int =>
            source_track_id: Int,
            source_stem: Int
        }}
    "#)
}

fn create_similar_to_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create similar_to {
            from_track: Int,
            to_track: Int =>
            similarity_score: Float
        }}
    "#)
}

fn create_sessions_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create sessions {
            id: Int =>
            ended_at: Int?
        }}
    "#)
}

fn create_track_plays_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create track_plays {
            session_id: Int,
            loaded_at: Int =>
            track_path: String,
            track_name: String,
            track_id: Int?,
            deck_index: Int,
            load_source: String,
            suggestion_score: Float?,
            suggestion_tags_json: String?,
            suggestion_energy_dir: Float?,
            play_started_at: Int?,
            play_start_sample: Int?,
            play_ended_at: Int?,
            seconds_played: Float?,
            hot_cues_used_json: String?,
            loop_was_active: Bool default false,
            played_with_json: String?
        }}
    "#)
}

fn create_harmonic_match_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create harmonic_match {
            from_track: Int,
            to_track: Int =>
            match_type: String
        }}
    "#)
}

fn create_track_tags_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create track_tags {
            track_id: Int,
            label: String =>
            color: String?,
            sort_order: Int default 0
        }}
    "#)
}

fn create_ml_analysis_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create ml_analysis {
            track_id: Int =>
            vocal_presence: Float?,
            arousal: Float?,
            valence: Float?,
            top_genre: String?,
            genre_scores_json: String?,
            mood_scores_json: String?,
            binary_moods_json: String?,
            danceability: Float?,
            approachability: Float?,
            reverb: Float?,
            timbre: Float?,
            tonal: Float?,
            mood_acoustic: Float?,
            mood_electronic: Float?
        }}
    "#)
}

/// Check if ml_analysis needs schema migration (e.g., missing columns).
/// If so, drop and recreate. ML data is regenerated via Similarity reanalysis.
fn migrate_ml_analysis_if_needed(db: &DbInstance) -> Result<(), DbError> {
    let result = db
        .run_script("::columns ml_analysis", Default::default(), cozo::ScriptMutability::Immutable)
        .map_err(|e| DbError::Schema(e.to_string()))?;

    let has_column = |name: &str| -> bool {
        result.rows.iter().any(|row| {
            row.first().and_then(|v| v.get_str()) == Some(name)
        })
    };

    // Check for latest schema additions — if any are missing, drop and recreate
    let needs_migration = !has_column("binary_moods_json") || !has_column("danceability");

    if needs_migration {
        log::info!("Migrating 'ml_analysis' schema: adding new audio characteristic columns");
        db.run_script("::remove ml_analysis", Default::default(), cozo::ScriptMutability::Mutable)
            .map_err(|e| DbError::Schema(e.to_string()))?;
        create_ml_analysis_relation(db)?;
    }

    Ok(())
}

/// Migrate the tracks relation if it's missing columns added after initial release.
///
/// CozoDB doesn't support ALTER TABLE. We use `::rename` for an atomic swap:
///
/// 1. Create `tracks_staging` with the target schema
/// 2. Copy all rows from `tracks` → `tracks_staging` (with defaults for new columns)
/// 3. Verify row count matches
/// 4. Atomic swap: `::rename tracks -> tracks_old, tracks_staging -> tracks`
/// 5. Drop `tracks_old`
///
/// The `::rename` in step 4 is atomic — `tracks` always exists with valid data.
/// Crash before step 4: old `tracks` is untouched, `tracks_staging` is cleaned up
/// on next startup. Crash after step 4: `tracks` has new schema, `tracks_old`
/// is cleaned up on next startup.
fn migrate_tracks_if_needed(db: &DbInstance) -> Result<(), DbError> {
    let result = db
        .run_script("::columns tracks", Default::default(), cozo::ScriptMutability::Immutable)
        .map_err(|e| DbError::Schema(e.to_string()))?;

    let has_column = |name: &str| -> bool {
        result.rows.iter().any(|row| {
            row.first().and_then(|v| v.get_str()) == Some(name)
        })
    };

    if has_column("original_name") && has_column("title") {
        return Ok(()); // Fully migrated (both original_name and name→title done)
    }

    if has_column("original_name") && !has_column("title") {
        // Has original_name but still uses "name" — needs name→title rename
        return migrate_tracks_name_to_title(db);
    }

    // Doesn't have original_name at all — oldest schema, add original_name AND rename name→title
    log::info!("Migrating 'tracks' schema: adding 'original_name' + renaming 'name' → 'title'");

    // Count rows in old relation for verification
    let old_count = count_relation(db, "tracks")?;
    log::info!("Migration: {} tracks to migrate", old_count);

    // Step 1: Create staging relation with the final schema (title + original_name)
    db.run_script(
        r#"
        {:create tracks_staging {
            id: Int =>
            path: String,
            folder_path: String,
            title: String,
            original_name: String default '',
            artist: String?,
            bpm: Float?,
            original_bpm: Float?,
            key: String?,
            duration_seconds: Float,
            lufs: Float?,
            integrated_lufs: Float?,
            drop_marker: Int?,
            first_beat_sample: Int default 0,
            file_mtime: Int,
            file_size: Int,
            waveform_path: String?
        }}
        "#,
        Default::default(),
        cozo::ScriptMutability::Mutable,
    ).map_err(|e| DbError::Schema(format!("Failed to create staging relation: {}", e)))?;

    // Step 2: Copy all data from old tracks → staging, mapping name→title and defaulting original_name
    if old_count > 0 {
        db.run_script(
            r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
              file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
                        file_mtime, file_size, waveform_path},
                title = name,
                original_name = ''
            :put tracks_staging {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
                              file_mtime, file_size, waveform_path}
            "#,
            Default::default(),
            cozo::ScriptMutability::Mutable,
        ).map_err(|e| DbError::Schema(format!("Failed to copy tracks to staging: {}", e)))?;
    }

    // Step 3: Verify staging has all rows before touching old data
    let staging_count = count_relation(db, "tracks_staging")?;
    if staging_count != old_count {
        // Abort — drop staging, keep old tracks intact
        let _ = db.run_script("::remove tracks_staging", Default::default(), cozo::ScriptMutability::Mutable);
        return Err(DbError::Schema(format!(
            "Migration verification failed: expected {} tracks in staging, got {}. Old data preserved.",
            old_count, staging_count
        )));
    }

    log::info!("Migration: staging verified ({} tracks), performing atomic swap", staging_count);

    // Step 4: Atomic swap — tracks always exists with valid data
    db.run_script(
        "::rename tracks -> tracks_old, tracks_staging -> tracks",
        Default::default(),
        cozo::ScriptMutability::Mutable,
    ).map_err(|e| DbError::Schema(format!("Atomic rename failed: {}", e)))?;

    // Step 5: Drop old relation (safe — tracks already points to new data)
    let _ = db.run_script("::remove tracks_old", Default::default(), cozo::ScriptMutability::Mutable);

    log::info!("Tracks migration complete — {} tracks migrated successfully", staging_count);
    Ok(())
}

/// Migrate tracks schema: rename 'name' column to 'title'
///
/// For databases that already have 'original_name' but still use the old 'name' column.
/// The data is copied as-is (existing "Artist - Title" values stay unchanged for backward compat).
fn migrate_tracks_name_to_title(db: &DbInstance) -> Result<(), DbError> {
    log::info!("Migrating 'tracks' schema: renaming 'name' → 'title'");

    let old_count = count_relation(db, "tracks")?;
    log::info!("Migration: {} tracks to migrate", old_count);

    // Step 1: Create staging relation with 'title' instead of 'name'
    db.run_script(
        r#"
        {:create tracks_staging {
            id: Int =>
            path: String,
            folder_path: String,
            title: String,
            original_name: String default '',
            artist: String?,
            bpm: Float?,
            original_bpm: Float?,
            key: String?,
            duration_seconds: Float,
            lufs: Float?,
            integrated_lufs: Float?,
            drop_marker: Int?,
            first_beat_sample: Int default 0,
            file_mtime: Int,
            file_size: Int,
            waveform_path: String?
        }}
        "#,
        Default::default(),
        cozo::ScriptMutability::Mutable,
    ).map_err(|e| DbError::Schema(format!("Failed to create staging relation: {}", e)))?;

    // Step 2: Copy data, mapping name → title
    if old_count > 0 {
        db.run_script(
            r#"
            ?[id, path, folder_path, title, original_name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
              file_mtime, file_size, waveform_path] :=
                *tracks{id, path, folder_path, name, original_name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
                        file_mtime, file_size, waveform_path},
                title = name
            :put tracks_staging {id => path, folder_path, title, original_name, artist, bpm, original_bpm, key,
                              duration_seconds, lufs, integrated_lufs, drop_marker, first_beat_sample,
                              file_mtime, file_size, waveform_path}
            "#,
            Default::default(),
            cozo::ScriptMutability::Mutable,
        ).map_err(|e| DbError::Schema(format!("Failed to copy tracks to staging: {}", e)))?;
    }

    // Step 3: Verify row count
    let staging_count = count_relation(db, "tracks_staging")?;
    if staging_count != old_count {
        let _ = db.run_script("::remove tracks_staging", Default::default(), cozo::ScriptMutability::Mutable);
        return Err(DbError::Schema(format!(
            "Migration verification failed: expected {} tracks in staging, got {}. Old data preserved.",
            old_count, staging_count
        )));
    }

    log::info!("Migration: staging verified ({} tracks), performing atomic swap", staging_count);

    // Step 4: Atomic swap
    db.run_script(
        "::rename tracks -> tracks_old, tracks_staging -> tracks",
        Default::default(),
        cozo::ScriptMutability::Mutable,
    ).map_err(|e| DbError::Schema(format!("Atomic rename failed: {}", e)))?;

    // Step 5: Drop old relation
    let _ = db.run_script("::remove tracks_old", Default::default(), cozo::ScriptMutability::Mutable);

    log::info!("Tracks name→title migration complete — {} tracks migrated", staging_count);
    Ok(())
}

/// Count rows in a CozoDB stored relation
fn count_relation(db: &DbInstance, relation: &str) -> Result<usize, DbError> {
    let query = format!("?[count(id)] := *{}{{id}}", relation);
    let result = db
        .run_script(&query, Default::default(), cozo::ScriptMutability::Immutable)
        .map_err(|e| DbError::Schema(e.to_string()))?;
    let count = result.rows.first()
        .and_then(|row| row.first())
        .and_then(|v| v.get_int())
        .unwrap_or(0) as usize;
    Ok(count)
}

fn create_audio_features_relation(db: &DbInstance) -> Result<(), DbError> {
    // Create relation for audio feature vectors
    // CozoDB uses <F32; 16> for a 16-dimensional F32 vector type
    run_schema(db, r#"
        {:create audio_features {
            track_id: Int =>
            vec: <F32; 16>
        }}
    "#)
}

fn create_audio_features_index(db: &DbInstance) -> Result<(), DbError> {
    // First ensure the relation exists
    create_audio_features_relation(db)?;

    // HNSW vector index for audio features
    // dim=16 for our audio feature vector
    // m=16 connections per node (default)
    // ef_construction=200 for good recall during index building
    let result = db.run_script(
        r#"
        ::hnsw create audio_features:similarity_index {
            dim: 16,
            m: 16,
            ef_construction: 200,
            dtype: F32,
            fields: [vec],
            distance: Cosine,
            extend_candidates: true,
            keep_pruned_connections: false
        }
        "#,
        Default::default(),
        cozo::ScriptMutability::Mutable,
    );

    // Ignore "already exists" errors - this is expected behavior
    match result {
        Ok(_) => {
            log::info!("HNSW index created successfully");
            Ok(())
        }
        Err(e) => {
            let err_str = e.to_string();
            // Index already exists is fine
            if err_str.contains("already exists") {
                log::debug!("HNSW index already exists");
                Ok(())
            } else {
                Err(DbError::Schema(err_str))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_features_vector_roundtrip() {
        let features = AudioFeatures {
            bpm_normalized: 0.5,
            bpm_confidence: 0.9,
            beat_strength: 0.7,
            rhythm_regularity: 0.8,
            key_x: 0.5,
            key_y: 0.866,
            mode: 1.0,
            harmonic_complexity: 0.3,
            lufs_normalized: 0.6,
            dynamic_range: 0.4,
            energy_mean: 0.7,
            energy_variance: 0.2,
            spectral_centroid: 0.5,
            spectral_bandwidth: 0.4,
            spectral_rolloff: 0.6,
            mfcc_flatness: 0.3,
        };

        let vec = features.to_vector();
        assert_eq!(vec.len(), 16);

        let reconstructed = AudioFeatures::from_vector(&vec).unwrap();
        assert!((reconstructed.bpm_normalized - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_harmonic_match_type() {
        assert_eq!(HarmonicMatchType::Same.as_str(), "same");
        assert_eq!(HarmonicMatchType::from_str("adjacent"), Some(HarmonicMatchType::Adjacent));
        assert_eq!(HarmonicMatchType::from_str("invalid"), None);
    }
}
