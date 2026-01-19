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
    pub name: String,
    pub artist: Option<String>,
    pub bpm: Option<f64>,
    pub original_bpm: Option<f64>,
    pub key: Option<String>,
    pub duration_seconds: f64,
    pub lufs: Option<f32>,
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

/// A cue point on a track
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuePoint {
    pub track_id: i64,
    pub index: u8,
    pub sample_position: i64,
    pub label: Option<String>,
    pub color: Option<String>,
}

/// A saved loop on a track
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedLoop {
    pub track_id: i64,
    pub index: u8,
    pub start_sample: i64,
    pub end_sample: i64,
    pub label: Option<String>,
    pub color: Option<String>,
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

/// Tracks played in sequence (DJ transition history)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayedAfter {
    pub from_track: i64,
    pub to_track: i64,
    pub count: i32,
    pub avg_transition_quality: Option<f32>,
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

    // Core relations
    if !existing.contains("tracks") {
        log::debug!("Creating 'tracks' relation");
        create_tracks_relation(db)?;
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
    if !existing.contains("played_after") {
        log::debug!("Creating 'played_after' relation");
        create_played_after_relation(db)?;
    }
    if !existing.contains("harmonic_match") {
        log::debug!("Creating 'harmonic_match' relation");
        create_harmonic_match_relation(db)?;
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
            name: String,
            artist: String?,
            bpm: Float?,
            original_bpm: Float?,
            key: String?,
            duration_seconds: Float,
            lufs: Float?,
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

fn create_played_after_relation(db: &DbInstance) -> Result<(), DbError> {
    run_schema(db, r#"
        {:create played_after {
            from_track: Int,
            to_track: Int =>
            count: Int,
            avg_transition_quality: Float?
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
