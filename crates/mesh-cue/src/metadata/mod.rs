//! Metadata extraction from embedded tags and filename patterns
//!
//! Provides robust artist/title extraction for the import pipeline by combining:
//! 1. Embedded audio tags (ID3v2, Vorbis, MP4, etc.)
//! 2. Filename pattern parsing (with known-artist disambiguation)
//!
//! Priority: embedded tags > filename parsing > raw base_name

pub mod filename;
pub mod tags;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use mesh_core::db::DatabaseService;

use filename::parse_filename;
use tags::read_embedded_tags;

/// Resolved metadata ready for track creation
#[derive(Debug, Clone)]
pub struct ResolvedMetadata {
    /// Display name: "Artist - Title" or just "Title"
    pub name: String,
    /// Extracted artist (normalized), if found
    pub artist: Option<String>,
}

/// Resolve metadata for a track from tags and/or filename.
///
/// `source_path` is the original audio file for tag reading (None for pre-separated stems).
/// `base_name` is the filename-derived display name (without extension).
/// `known_artists` is a lowercase-normalized set for disambiguation.
///
/// Priority: embedded tags > filename parsing > raw base_name.
/// Normalization (artist connectors → commas, brackets → parens, Original Mix stripped)
/// is applied to ALL sources — tags from Beatport etc. have the same quirks.
pub fn resolve_metadata(
    source_path: Option<&Path>,
    base_name: &str,
    known_artists: &HashSet<String>,
) -> ResolvedMetadata {
    // Try embedded tags first
    let embedded = source_path.and_then(read_embedded_tags);

    // Always parse filename as fallback
    let parsed = parse_filename(base_name, known_artists);

    // Resolve artist: tags > filename
    let raw_artist = embedded
        .as_ref()
        .and_then(|t| t.artist.clone())
        .or(parsed.artist);

    // Resolve title: tags > filename parsed title > base_name
    let raw_title = embedded
        .as_ref()
        .and_then(|t| t.title.clone())
        .unwrap_or(parsed.title);

    // Normalize both (same pipeline applies to all sources)
    let artist = raw_artist.map(|a| filename::normalize_artist_public(&a));
    let title = filename::normalize_title_public(&raw_title);

    // Construct display name
    let name = match &artist {
        Some(a) => format!("{} - {}", a, title),
        None => title.clone(),
    };

    ResolvedMetadata { name, artist }
}

/// Load the set of known artist names from the database.
///
/// Returns a lowercase-normalized HashSet for case-insensitive matching
/// during filename disambiguation.
pub fn get_known_artists(db: &Arc<DatabaseService>) -> HashSet<String> {
    match db.get_distinct_artists() {
        Ok(artists) => artists
            .into_iter()
            .map(|a| a.to_lowercase())
            .collect(),
        Err(e) => {
            log::warn!("metadata: Failed to load known artists: {}", e);
            HashSet::new()
        }
    }
}
