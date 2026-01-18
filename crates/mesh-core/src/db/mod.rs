//! Database module using CozoDB for high-performance track metadata storage
//!
//! This module provides:
//! - Relational storage for tracks, playlists, cue points, loops
//! - Graph edges for track relationships (similarity, harmonic compatibility)
//! - Vector search via HNSW index for audio feature similarity
//!
//! # Architecture
//!
//! CozoDB is used as a unified database supporting:
//! - **Relational queries** via Datalog
//! - **Graph traversal** with built-in algorithms
//! - **Vector similarity** with HNSW indexes
//!
//! All queries are performed through typed Rust APIs that generate
//! CozoScript (Datalog) queries internally.

mod schema;
mod queries;
mod migration;
mod service;

pub use schema::{
    Track, Playlist, PlaylistTrack, CuePoint, SavedLoop,
    SimilarTo, PlayedAfter, HarmonicMatch, HarmonicMatchType,
    AudioFeatures,
};
pub use queries::{TrackQuery, PlaylistQuery, SimilarityQuery};
pub use migration::{
    migrate_from_wav_collection, migrate_single_track, track_needs_update,
    insert_analyzed_track, NewTrackData,
    MigrationProgress, MigrationPhase, MigrationResult,
};
pub use service::DatabaseService;

use cozo::{DbInstance, DataValue, NamedRows};
use std::collections::BTreeMap;
use std::path::Path;

/// Database connection wrapper
pub struct MeshDb {
    db: DbInstance,
}

impl MeshDb {
    /// Open or create a database at the given path
    ///
    /// Uses SQLite backend for persistence with good performance.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let db = DbInstance::new("sqlite", path, "")
            .map_err(|e| DbError::Open(e.to_string()))?;

        let mesh_db = Self { db };
        mesh_db.ensure_schema()?;

        Ok(mesh_db)
    }

    /// Create an in-memory database (for testing)
    pub fn in_memory() -> Result<Self, DbError> {
        let db = DbInstance::new("mem", "", "")
            .map_err(|e| DbError::Open(e.to_string()))?;

        let mesh_db = Self { db };
        mesh_db.ensure_schema()?;

        Ok(mesh_db)
    }

    /// Ensure all required relations exist
    fn ensure_schema(&self) -> Result<(), DbError> {
        schema::create_all_relations(&self.db)?;
        Ok(())
    }

    /// Run a raw CozoScript query
    pub fn run_script(&self, script: &str, params: BTreeMap<String, DataValue>) -> Result<NamedRows, DbError> {
        self.db.run_script(script, params, cozo::ScriptMutability::Mutable)
            .map_err(|e| DbError::Query(e.to_string()))
    }

    /// Run a read-only query
    pub fn run_query(&self, script: &str, params: BTreeMap<String, DataValue>) -> Result<NamedRows, DbError> {
        self.db.run_script(script, params, cozo::ScriptMutability::Immutable)
            .map_err(|e| DbError::Query(e.to_string()))
    }

    /// Get the underlying DbInstance for advanced usage
    pub fn inner(&self) -> &DbInstance {
        &self.db
    }
}

/// Database errors
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("Failed to open database: {0}")]
    Open(String),

    #[error("Query error: {0}")]
    Query(String),

    #[error("Schema error: {0}")]
    Schema(String),

    #[error("Migration error: {0}")]
    Migration(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Helper macro for creating parameter maps
#[macro_export]
macro_rules! params {
    () => {
        std::collections::BTreeMap::new()
    };
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut map = std::collections::BTreeMap::new();
        $(
            map.insert($key.to_string(), cozo::DataValue::from($value));
        )+
        map
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let db = MeshDb::in_memory().unwrap();
        // Should be able to run a simple query
        let result = db.run_query("?[x] := x = 1", params!()).unwrap();
        assert_eq!(result.rows.len(), 1);
    }
}
