//! Cross-platform sync engine with metadata-based change detection
//!
//! This module provides efficient file synchronization by:
//! - Scanning local collection from database for playlist membership
//! - Scanning USB collections from database (mesh.db on USB)
//! - Using file metadata (size + mtime) for fast change detection
//! - Building minimal sync plans (what to copy, delete, update)
//!
//! Both local and USB collections use CozoDB databases for track and playlist metadata.

use crate::db::{DatabaseService, MeshDb, MlAnalysisData, PlaylistQuery, TrackRow, CuePoint, SavedLoop, StemLink, CuePointQuery, SavedLoopQuery, StemLinkQuery, TrackQuery, SimilarityQuery};
use super::cache::get_or_open_usb_database;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// Information about a track file with metadata for comparison
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Filename only (e.g., "track.wav")
    pub filename: String,
    /// File size in bytes
    pub size: u64,
    /// Last modified time
    pub mtime: SystemTime,
    /// Database track record (for metadata comparison)
    /// Uses TrackRow internally since this is within mesh-core
    pub db_track: Option<TrackRow>,
    /// Cue points for this track
    pub cue_points: Vec<CuePoint>,
    /// Saved loops for this track
    pub saved_loops: Vec<SavedLoop>,
    /// Stem links for this track (linked stems for prepared mode)
    pub stem_links: Vec<StemLink>,
    /// ML analysis data (for metadata-only sync detection)
    pub ml_analysis: Option<MlAnalysisData>,
    /// Track tags (label, color)
    pub tags: Vec<(String, Option<String>)>,
    /// Whether audio features are stored for this track
    pub has_audio_features: bool,
}

/// A track membership in a playlist (database record)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaylistTrack {
    /// Playlist qualified name (e.g., "My Set" for root, "Live Sets/Opening" for nested)
    pub playlist: String,
    /// Track filename (e.g., "track.wav")
    pub track_filename: String,
}

/// Playlist metadata for sync comparison and hierarchical creation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaylistExportInfo {
    /// Playlist leaf name (e.g., "Opening")
    pub name: String,
    /// Parent playlist leaf name (None for root playlists)
    pub parent_name: Option<String>,
    /// Sort order within parent
    pub sort_order: i32,
}

/// State of a collection (local or USB)
#[derive(Debug, Clone, Default)]
pub struct CollectionState {
    /// Map of filename -> TrackInfo for tracks in tracks/
    pub tracks: HashMap<String, TrackInfo>,
    /// Set of (playlist_qualified_name, track_filename) pairs
    pub playlist_tracks: HashSet<PlaylistTrack>,
    /// Set of playlists with hierarchy info
    pub playlists: HashSet<PlaylistExportInfo>,
}

/// A track to copy during sync
#[derive(Debug, Clone)]
pub struct TrackCopy {
    /// Source path (local)
    pub source: PathBuf,
    /// Destination path (relative to USB collection root, e.g., "tracks/file.wav")
    pub destination: PathBuf,
    /// File size in bytes
    pub size: u64,
}

/// Result of comparing local vs USB collections
#[derive(Debug, Clone, Default)]
pub struct SyncPlan {
    /// Tracks to copy (new or changed content)
    pub tracks_to_copy: Vec<TrackCopy>,
    /// Track filenames needing metadata-only sync (no WAV re-copy)
    pub tracks_to_update: Vec<String>,
    /// Track filenames to delete from USB
    pub tracks_to_delete: Vec<String>,
    /// Playlist track memberships to add (insert into USB database)
    pub playlist_tracks_to_add: Vec<PlaylistTrack>,
    /// Playlist track memberships to remove (delete from USB database)
    pub playlist_tracks_to_remove: Vec<PlaylistTrack>,
    /// Playlists to create in USB database (ordered: parents before children)
    pub playlists_to_create: Vec<PlaylistExportInfo>,
    /// Playlists to delete from USB database (leaf name for lookup)
    pub playlists_to_delete: Vec<PlaylistExportInfo>,
    /// Total bytes to transfer
    pub total_bytes: u64,
    /// Tracks missing LUFS analysis (need to analyze before export)
    pub tracks_missing_lufs: Vec<PathBuf>,
}

impl SyncPlan {
    /// Check if there's anything to sync
    pub fn is_empty(&self) -> bool {
        self.tracks_to_copy.is_empty()
            && self.tracks_to_update.is_empty()
            && self.tracks_to_delete.is_empty()
            && self.playlist_tracks_to_add.is_empty()
            && self.playlist_tracks_to_remove.is_empty()
            && self.playlists_to_create.is_empty()
            && self.playlists_to_delete.is_empty()
    }

    /// Get summary for display
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("{} tracks to copy ({})", self.tracks_to_copy.len(), super::format_bytes(self.total_bytes)),
        ];
        if !self.tracks_to_update.is_empty() {
            parts.push(format!("{} to update", self.tracks_to_update.len()));
        }
        parts.push(format!("{} to delete", self.tracks_to_delete.len()));
        parts.push(format!("{} playlist entries to add", self.playlist_tracks_to_add.len()));
        parts.push(format!("{} to remove", self.playlist_tracks_to_remove.len()));
        parts.join(", ")
    }

    /// Validate that USB has enough space
    pub fn validate_space(&self, available_bytes: u64) -> Result<(), super::UsbError> {
        if self.total_bytes > available_bytes {
            return Err(super::UsbError::InsufficientSpace {
                required: self.total_bytes,
                available: available_bytes,
            });
        }
        Ok(())
    }
}

/// Progress callback for scanning
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

/// Build a qualified name for a playlist by walking up the parent chain
///
/// Returns path-like names: "Live Sets/Opening" for nested, "My Set" for root.
/// This provides a unique identifier even when multiple playlists share a leaf name.
fn build_qualified_name(playlist: &crate::db::Playlist, id_to_playlist: &HashMap<i64, &crate::db::Playlist>) -> String {
    let mut parts = vec![playlist.name.clone()];
    let mut current = playlist;
    while let Some(pid) = current.parent_id {
        if let Some(parent) = id_to_playlist.get(&pid) {
            parts.push(parent.name.clone());
            current = parent;
        } else {
            break;
        }
    }
    parts.reverse();
    parts.join("/")
}

/// Recursively collect all descendant playlist IDs
fn collect_descendants(parent_id: i64, all_playlists: &[crate::db::Playlist], result: &mut HashSet<i64>) {
    for p in all_playlists {
        if p.parent_id == Some(parent_id) && !result.contains(&p.id) {
            result.insert(p.id);
            collect_descendants(p.id, all_playlists, result);
        }
    }
}

/// Scan a local collection from the database to discover tracks and playlist membership
///
/// This is the database-based version that reads playlist membership from CozoDB
/// instead of scanning the filesystem for symlinks.
///
/// # Arguments
/// * `db` - Database connection
/// * `collection_root` - Path to the local collection (for resolving track paths)
/// * `selected_playlists` - List of playlist names to include
/// * `progress` - Optional callback for progress updates
/// * `db_service` - Optional DatabaseService for ML/tags/features queries
pub fn scan_local_collection_from_db(
    db: &MeshDb,
    _collection_root: &Path,  // Kept for API compatibility, paths come from DB
    selected_playlists: &[String],
    progress: Option<ProgressCallback>,
    db_service: Option<&DatabaseService>,
) -> Result<CollectionState, Box<dyn std::error::Error + Send + Sync>> {
    let mut state = CollectionState::default();
    // Map filename -> (path, Track) to keep the database record for metadata comparison
    let mut track_data: HashMap<String, (PathBuf, TrackRow)> = HashMap::new();

    // Get all playlists from the database
    let all_playlists = PlaylistQuery::get_all(db)
        .map_err(|e| format!("Failed to get playlists: {}", e))?;

    // Build id→playlist map for parent name resolution
    let id_to_playlist: HashMap<i64, &crate::db::Playlist> =
        all_playlists.iter().map(|p| (p.id, p)).collect();

    // Build qualified_name→id map for matching against selected_playlists (which come from NodeIds)
    let mut qualified_to_id: HashMap<String, i64> = HashMap::new();
    let mut id_to_qualified: HashMap<i64, String> = HashMap::new();
    for playlist in &all_playlists {
        let qname = build_qualified_name(playlist, &id_to_playlist);
        qualified_to_id.insert(qname.clone(), playlist.id);
        id_to_qualified.insert(playlist.id, qname);
    }

    // Resolve selected_playlists to IDs, including descendants
    let mut selected_ids: HashSet<i64> = HashSet::new();
    for name in selected_playlists {
        // Try exact match first (for root playlists, name == qualified_name)
        if let Some(&id) = qualified_to_id.get(name.as_str()) {
            selected_ids.insert(id);
            collect_descendants(id, &all_playlists, &mut selected_ids);
        } else {
            // Also try matching just the leaf name for backwards compatibility
            for playlist in &all_playlists {
                if playlist.name == *name {
                    selected_ids.insert(playlist.id);
                    collect_descendants(playlist.id, &all_playlists, &mut selected_ids);
                }
            }
        }
    }

    // Filter to selected playlists and get their tracks
    for playlist in &all_playlists {
        if !selected_ids.contains(&playlist.id) {
            continue;
        }

        let qualified_name = id_to_qualified.get(&playlist.id)
            .cloned()
            .unwrap_or_else(|| playlist.name.clone());

        // Resolve parent name for hierarchy preservation
        let parent_name = playlist.parent_id
            .and_then(|pid| id_to_playlist.get(&pid))
            .map(|p| p.name.clone());

        state.playlists.insert(PlaylistExportInfo {
            name: playlist.name.clone(),
            parent_name,
            sort_order: playlist.sort_order,
        });

        // Get tracks in this playlist
        let tracks = PlaylistQuery::get_tracks(db, playlist.id)
            .map_err(|e| format!("Failed to get tracks for playlist {}: {}", playlist.name, e))?;

        for track in tracks {
            let path = PathBuf::from(&track.path);
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&track.name)
                .to_string();

            // Keep track data for metadata comparison (use first occurrence if duplicate filename)
            // Note: PlaylistQuery returns TrackRow, not Track
            track_data.entry(filename.clone()).or_insert((path, track));

            // Add playlist track membership (using qualified name for unique identification)
            state.playlist_tracks.insert(PlaylistTrack {
                playlist: qualified_name.clone(),
                track_filename: filename,
            });
        }
    }

    // Pre-fetch cue points, loops, and stem links for all tracks (sequential DB queries)
    let mut cue_points_map: HashMap<i64, Vec<CuePoint>> = HashMap::new();
    let mut loops_map: HashMap<i64, Vec<SavedLoop>> = HashMap::new();
    let mut stem_links_map: HashMap<i64, Vec<StemLink>> = HashMap::new();
    let mut ml_analysis_map: HashMap<i64, MlAnalysisData> = HashMap::new();
    let mut tags_map: HashMap<i64, Vec<(String, Option<String>)>> = HashMap::new();
    let mut audio_features_map: HashMap<i64, bool> = HashMap::new();

    for (_, (_, track)) in &track_data {
        if let Ok(cues) = CuePointQuery::get_for_track(db, track.id) {
            cue_points_map.insert(track.id, cues);
        }
        if let Ok(loops) = SavedLoopQuery::get_for_track(db, track.id) {
            loops_map.insert(track.id, loops);
        }
        if let Ok(links) = StemLinkQuery::get_for_track(db, track.id) {
            stem_links_map.insert(track.id, links);
        }
        // Fetch ML analysis, tags, and audio features via DatabaseService
        if let Some(svc) = db_service {
            if let Ok(Some(ml)) = svc.get_ml_analysis(track.id) {
                ml_analysis_map.insert(track.id, ml);
            }
            if let Ok(t) = svc.get_tags(track.id) {
                tags_map.insert(track.id, t);
            }
            if let Ok(has) = SimilarityQuery::has_features(db, track.id) {
                audio_features_map.insert(track.id, has);
            }
        }
    }

    // Get file metadata for all unique tracks (parallel for speed)
    let track_list: Vec<(String, PathBuf, TrackRow)> = track_data
        .into_iter()
        .map(|(filename, (path, track))| (filename, path, track))
        .collect();
    let total_files = track_list.len();
    let progress_counter = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    let track_infos: Vec<Result<TrackInfo, std::io::Error>> = track_list
        .into_par_iter()
        .map(|(filename, path, db_track)| {
            let metadata = std::fs::metadata(&path)?;
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            // Get pre-fetched data
            let cue_points = cue_points_map.get(&db_track.id).cloned().unwrap_or_default();
            let saved_loops = loops_map.get(&db_track.id).cloned().unwrap_or_default();
            let stem_links = stem_links_map.get(&db_track.id).cloned().unwrap_or_default();
            let ml_analysis = ml_analysis_map.get(&db_track.id).cloned();
            let tags = tags_map.get(&db_track.id).cloned().unwrap_or_default();
            let has_audio_features = audio_features_map.get(&db_track.id).copied().unwrap_or(false);

            Ok(TrackInfo {
                path,
                filename,
                size: metadata.len(),
                mtime,
                db_track: Some(db_track),
                cue_points,
                saved_loops,
                stem_links,
                ml_analysis,
                tags,
                has_audio_features,
            })
        })
        .collect();

    // Collect results
    for result in track_infos {
        let info = result?;
        state.tracks.insert(info.filename.clone(), info);
    }

    Ok(state)
}

/// Scan a USB collection from its mesh.db database
///
/// Reads track and playlist information from the USB's database file.
/// Also scans the tracks/ directory to get file metadata for change detection.
///
/// # Arguments
/// * `collection_root` - Path to mesh-collection/ on USB
/// * `progress` - Optional callback for progress updates (reports files scanned)
pub fn scan_usb_collection(
    collection_root: &Path,
    progress: Option<ProgressCallback>,
) -> Result<CollectionState, Box<dyn std::error::Error + Send + Sync>> {
    let mut state = CollectionState::default();
    let tracks_dir = collection_root.join("tracks");

    // Scan track files for metadata (size, mtime for change detection)
    let track_paths: Vec<PathBuf> = if tracks_dir.exists() {
        WalkDir::new(&tracks_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wav"))
            .map(|e| e.path().to_path_buf())
            .collect()
    } else {
        Vec::new()
    };

    // Get USB database from cache for metadata comparison
    let usb_db_service = get_or_open_usb_database(collection_root);

    // Build map of filename -> metadata from USB database
    #[allow(clippy::type_complexity)]
    let mut db_metadata: HashMap<String, (TrackRow, Vec<CuePoint>, Vec<SavedLoop>, Vec<StemLink>,
        Option<MlAnalysisData>, Vec<(String, Option<String>)>, bool)> = HashMap::new();
    if let Some(ref db_service) = usb_db_service {
        // Get all tracks from database
        if let Ok(all_tracks) = TrackQuery::get_all(db_service.db()) {
            for track in all_tracks {
                let filename = PathBuf::from(&track.path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&track.name)
                    .to_string();

                let cue_points = CuePointQuery::get_for_track(db_service.db(), track.id)
                    .unwrap_or_default();
                let saved_loops = SavedLoopQuery::get_for_track(db_service.db(), track.id)
                    .unwrap_or_default();
                let stem_links = StemLinkQuery::get_for_track(db_service.db(), track.id)
                    .unwrap_or_default();
                let ml_analysis = db_service.get_ml_analysis(track.id).unwrap_or(None);
                let tags = db_service.get_tags(track.id).unwrap_or_default();
                let has_audio_features = SimilarityQuery::has_features(db_service.db(), track.id)
                    .unwrap_or(false);

                db_metadata.insert(filename, (track, cue_points, saved_loops, stem_links,
                    ml_analysis, tags, has_audio_features));
            }
        }

        // Get playlists with hierarchy info
        if let Ok(playlists) = PlaylistQuery::get_all(db_service.db()) {
            // Build id→playlist map for parent name resolution
            let usb_id_to_playlist: HashMap<i64, &crate::db::Playlist> =
                playlists.iter().map(|p| (p.id, p)).collect();
            let mut usb_id_to_qualified: HashMap<i64, String> = HashMap::new();
            for playlist in &playlists {
                let qname = build_qualified_name(playlist, &usb_id_to_playlist);
                usb_id_to_qualified.insert(playlist.id, qname);
            }

            for playlist in &playlists {
                let parent_name = playlist.parent_id
                    .and_then(|pid| usb_id_to_playlist.get(&pid))
                    .map(|p| p.name.clone());

                state.playlists.insert(PlaylistExportInfo {
                    name: playlist.name.clone(),
                    parent_name,
                    sort_order: playlist.sort_order,
                });

                let qualified_name = usb_id_to_qualified.get(&playlist.id)
                    .cloned()
                    .unwrap_or_else(|| playlist.name.clone());

                // Get tracks in this playlist
                if let Ok(tracks) = PlaylistQuery::get_tracks(db_service.db(), playlist.id) {
                    for track in tracks {
                        let filename = PathBuf::from(&track.path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&track.name)
                            .to_string();

                        state.playlist_tracks.insert(PlaylistTrack {
                            playlist: qualified_name.clone(),
                            track_filename: filename,
                        });
                    }
                }
            }
        }
    }

    // Get file metadata for tracks (parallel for speed)
    let total_files = track_paths.len();
    let progress_counter = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = progress.as_ref();

    let track_infos: Vec<Result<TrackInfo, std::io::Error>> = track_paths
        .into_par_iter()
        .map(|path| {
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let metadata = std::fs::metadata(&path)?;
            let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            // Update progress
            let current = progress_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(cb) = progress_ref {
                cb(current, total_files);
            }

            // Get database metadata if available
            let (db_track, cue_points, saved_loops, stem_links, ml_analysis, tags, has_audio_features) =
                db_metadata
                    .get(&filename)
                    .map(|(t, c, l, s, ml, tg, af)| (Some(t.clone()), c.clone(), l.clone(), s.clone(), ml.clone(), tg.clone(), *af))
                    .unwrap_or((None, Vec::new(), Vec::new(), Vec::new(), None, Vec::new(), false));

            Ok(TrackInfo {
                path,
                filename,
                size: metadata.len(),
                mtime,
                db_track,
                cue_points,
                saved_loops,
                stem_links,
                ml_analysis,
                tags,
                has_audio_features,
            })
        })
        .collect();

    // Collect track results
    for result in track_infos {
        if let Ok(info) = result {
            state.tracks.insert(info.filename.clone(), info);
        }
    }

    Ok(state)
}

/// Check if metadata differs between local and USB tracks
///
/// Compares track fields (BPM, key, artist, etc.), cue points, saved loops, and stem links.
/// Returns true if any metadata field differs and requires re-export.
fn metadata_differs(local: &TrackInfo, usb: &TrackInfo) -> bool {
    // Compare Track fields if both have db_track
    match (&local.db_track, &usb.db_track) {
        (Some(l), Some(u)) => {
            // Compare analysis fields
            if l.bpm != u.bpm {
                log::debug!("metadata_differs: bpm differs for {} ({:?} vs {:?})", local.filename, l.bpm, u.bpm);
                return true;
            }
            if l.original_bpm != u.original_bpm {
                log::debug!("metadata_differs: original_bpm differs for {} ({:?} vs {:?})", local.filename, l.original_bpm, u.original_bpm);
                return true;
            }
            if l.key != u.key {
                log::debug!("metadata_differs: key differs for {} ({:?} vs {:?})", local.filename, l.key, u.key);
                return true;
            }
            if l.artist != u.artist {
                log::debug!("metadata_differs: artist differs for {} ({:?} vs {:?})", local.filename, l.artist, u.artist);
                return true;
            }
            if l.lufs != u.lufs {
                log::debug!("metadata_differs: lufs differs for {} ({:?} vs {:?})", local.filename, l.lufs, u.lufs);
                return true;
            }
            // Compare drop marker (beat position for performance mode)
            if l.drop_marker != u.drop_marker {
                log::debug!("metadata_differs: drop_marker differs for {} ({:?} vs {:?})", local.filename, l.drop_marker, u.drop_marker);
                return true;
            }
        }
        (Some(_), None) => {
            // Local has metadata, USB doesn't - needs export
            log::debug!("metadata_differs: USB missing metadata for {}", local.filename);
            return true;
        }
        _ => {
            // No metadata to compare
        }
    }

    // Compare cue points (hot cues for performance)
    if !cue_points_equal(&local.cue_points, &usb.cue_points) {
        log::debug!("metadata_differs: cue_points differ for {} ({} local vs {} usb)",
            local.filename, local.cue_points.len(), usb.cue_points.len());
        return true;
    }

    // Compare saved loops
    if !saved_loops_equal(&local.saved_loops, &usb.saved_loops) {
        log::debug!("metadata_differs: saved_loops differ for {} ({} local vs {} usb)",
            local.filename, local.saved_loops.len(), usb.saved_loops.len());
        return true;
    }

    // Compare stem links (linked stems for prepared mode)
    if !stem_links_equal(&local.stem_links, &usb.stem_links) {
        log::debug!("metadata_differs: stem_links differ for {} ({} local vs {} usb)",
            local.filename, local.stem_links.len(), usb.stem_links.len());
        return true;
    }

    // Compare ML analysis (check top_genre as a lightweight proxy)
    match (&local.ml_analysis, &usb.ml_analysis) {
        (Some(_), None) | (None, Some(_)) => {
            log::debug!("metadata_differs: ml_analysis presence differs for {}", local.filename);
            return true;
        }
        (Some(l), Some(u)) => {
            if l.top_genre != u.top_genre {
                log::debug!("metadata_differs: top_genre differs for {}", local.filename);
                return true;
            }
        }
        _ => {}
    }

    // Compare tags (sorted for consistent comparison)
    if local.tags.len() != usb.tags.len() {
        log::debug!("metadata_differs: tag count differs for {} ({} vs {})",
            local.filename, local.tags.len(), usb.tags.len());
        return true;
    }
    let mut local_tags = local.tags.clone();
    let mut usb_tags = usb.tags.clone();
    local_tags.sort_by(|a, b| a.0.cmp(&b.0));
    usb_tags.sort_by(|a, b| a.0.cmp(&b.0));
    if local_tags != usb_tags {
        log::debug!("metadata_differs: tags differ for {}", local.filename);
        return true;
    }

    // Compare audio features presence
    if local.has_audio_features != usb.has_audio_features {
        log::debug!("metadata_differs: audio_features presence differs for {}", local.filename);
        return true;
    }

    false
}

/// Compare two cue point lists for equality (ignoring order)
fn cue_points_equal(a: &[CuePoint], b: &[CuePoint]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Compare by index, sample_position, and label
    for cue_a in a {
        let found = b.iter().any(|cue_b| {
            cue_a.index == cue_b.index
                && cue_a.sample_position == cue_b.sample_position
                && cue_a.label == cue_b.label
        });
        if !found {
            return false;
        }
    }
    true
}

/// Compare two saved loop lists for equality (ignoring order)
fn saved_loops_equal(a: &[SavedLoop], b: &[SavedLoop]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Compare by index, start_sample, end_sample, and label
    for loop_a in a {
        let found = b.iter().any(|loop_b| {
            loop_a.index == loop_b.index
                && loop_a.start_sample == loop_b.start_sample
                && loop_a.end_sample == loop_b.end_sample
                && loop_a.label == loop_b.label
        });
        if !found {
            return false;
        }
    }
    true
}

/// Compare two stem link lists for equality (ignoring order)
fn stem_links_equal(a: &[StemLink], b: &[StemLink]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Compare by stem_index and source_track_id (which stem slot links to which track)
    for link_a in a {
        let found = b.iter().any(|link_b| {
            link_a.stem_index == link_b.stem_index
                && link_a.source_track_id == link_b.source_track_id
                && link_a.source_stem == link_b.source_stem
        });
        if !found {
            return false;
        }
    }
    true
}

/// Build a sync plan by comparing local state with USB state
///
/// Detects changes using:
/// - File size/mtime for audio content changes
/// - Database metadata comparison for BPM, key, cue points, loops, etc.
///
/// A file needs to be copied if:
/// - It doesn't exist on USB
/// - Size differs (definite content change)
/// - Local mtime is newer (file was modified)
/// - Database metadata differs (BPM, cue points, etc. changed)
pub fn build_sync_plan(local: &CollectionState, usb: &CollectionState) -> SyncPlan {
    let mut plan = SyncPlan::default();

    // Tracks to copy or update: compare local vs USB
    for (filename, local_info) in &local.tracks {
        match usb.tracks.get(filename) {
            Some(usb_info) => {
                if local_info.size != usb_info.size || local_info.mtime > usb_info.mtime {
                    // Audio content changed → full copy
                    plan.total_bytes += local_info.size;
                    plan.tracks_to_copy.push(TrackCopy {
                        source: local_info.path.clone(),
                        destination: PathBuf::from("tracks").join(filename),
                        size: local_info.size,
                    });
                } else if metadata_differs(local_info, usb_info) {
                    // Only metadata changed → metadata-only sync (no WAV copy)
                    plan.tracks_to_update.push(filename.clone());
                }
            }
            None => {
                // New file → full copy
                plan.total_bytes += local_info.size;
                plan.tracks_to_copy.push(TrackCopy {
                    source: local_info.path.clone(),
                    destination: PathBuf::from("tracks").join(filename),
                    size: local_info.size,
                });
            }
        }
    }

    // Tracks to delete: in USB but not in local
    for filename in usb.tracks.keys() {
        if !local.tracks.contains_key(filename) {
            plan.tracks_to_delete.push(filename.clone());
        }
    }

    // Playlist tracks to add: in local but not in USB
    for track in &local.playlist_tracks {
        if !usb.playlist_tracks.contains(track) {
            plan.playlist_tracks_to_add.push(track.clone());
        }
    }

    // Playlist tracks to remove: in USB but not in local
    for track in &usb.playlist_tracks {
        if !local.playlist_tracks.contains(track) {
            plan.playlist_tracks_to_remove.push(track.clone());
        }
    }

    // Playlists to create: in local but not in USB (matched by name + parent_name)
    for info in &local.playlists {
        if !usb.playlists.contains(info) {
            plan.playlists_to_create.push(info.clone());
        }
    }
    // Sort playlists_to_create topologically: parents (parent_name=None) before children
    plan.playlists_to_create.sort_by(|a, b| {
        match (&a.parent_name, &b.parent_name) {
            (None, Some(_)) => std::cmp::Ordering::Less,     // root before child
            (Some(_), None) => std::cmp::Ordering::Greater,  // child after root
            _ => a.sort_order.cmp(&b.sort_order),            // same level: by sort_order
        }
    });

    // Playlists to delete: in USB but not in local (reverse order: children before parents)
    for info in &usb.playlists {
        if !local.playlists.contains(info) {
            plan.playlists_to_delete.push(info.clone());
        }
    }
    plan.playlists_to_delete.sort_by(|a, b| {
        match (&a.parent_name, &b.parent_name) {
            (None, Some(_)) => std::cmp::Ordering::Greater,  // root after child (delete children first)
            (Some(_), None) => std::cmp::Ordering::Less,
            _ => a.sort_order.cmp(&b.sort_order),
        }
    });

    // Check which tracks are missing LUFS (for auto-analysis before export)
    // Read LUFS from database, not from WAV file metadata
    plan.tracks_missing_lufs = plan
        .tracks_to_copy
        .iter()
        .filter(|track| {
            // Look up the track in local collection by filename
            let filename = track.source.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            local.tracks.get(filename)
                .and_then(|info| info.db_track.as_ref())
                .map(|db_track| db_track.lufs.is_none())
                .unwrap_or(true) // If no DB track, assume LUFS is missing
        })
        .map(|track| track.source.clone())
        .collect();

    if !plan.tracks_missing_lufs.is_empty() {
        log::info!(
            "[LUFS] {} tracks missing LUFS analysis",
            plan.tracks_missing_lufs.len()
        );
    }

    plan
}

/// Copy a large file with buffered I/O, sequential read hints, and fsync
///
/// Optimized for USB flash drives where sequential writes are 2-5x faster
/// than random writes. Uses:
/// - 1 MB read/write buffers (much larger than kernel default 8 KB)
/// - `posix_fadvise(POSIX_FADV_SEQUENTIAL)` on source (Linux only)
/// - `sync_all()` on destination after write (ensures data hits flash)
/// - Size verification post-copy
/// - Progress callback for sub-file progress
///
/// Returns bytes written on success.
pub fn copy_large_file(
    source: &Path,
    destination: &Path,
    on_bytes_written: impl Fn(u64),
) -> Result<u64, super::UsbError> {
    use std::io::{BufReader, BufWriter, Read, Write};

    const BUFFER_SIZE: usize = 1024 * 1024; // 1 MB

    // Ensure parent directory exists
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let source_file = std::fs::File::open(source)?;
    let source_size = source_file.metadata()?.len();

    // Apply sequential read hint on Linux
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::posix_fadvise(
                source_file.as_raw_fd(),
                0,
                source_size as libc::off_t,
                libc::POSIX_FADV_SEQUENTIAL,
            );
        }
    }

    let dest_file = std::fs::File::create(destination)?;
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, source_file);
    let mut writer = BufWriter::with_capacity(BUFFER_SIZE, &dest_file);

    let mut total_written: u64 = 0;
    let mut buf = vec![0u8; BUFFER_SIZE];

    loop {
        let bytes_read = reader.read(&mut buf).map_err(|e| {
            super::UsbError::IoError(format!("Read error on {}: {}", source.display(), e))
        })?;
        if bytes_read == 0 {
            break;
        }
        writer.write_all(&buf[..bytes_read]).map_err(|e| {
            super::UsbError::IoError(format!("Write error on {}: {}", destination.display(), e))
        })?;
        total_written += bytes_read as u64;
        on_bytes_written(total_written);
    }

    // Flush BufWriter, then sync to physical media
    writer.flush().map_err(|e| {
        super::UsbError::IoError(format!("Flush error on {}: {}", destination.display(), e))
    })?;
    drop(writer); // Drop BufWriter to release borrow on dest_file
    dest_file.sync_all().map_err(|e| {
        super::UsbError::IoError(format!("Sync error on {}: {}", destination.display(), e))
    })?;

    // Verify size
    if total_written != source_size {
        return Err(super::UsbError::SizeMismatch {
            path: destination.to_path_buf(),
            expected: source_size,
            actual: total_written,
        });
    }

    Ok(total_written)
}

/// Copy a file with size verification
///
/// Returns Ok(()) on success, or error with retry info.
/// Verifies the copy succeeded by checking destination size matches source.
pub fn copy_with_verification(
    source: &Path,
    destination: &Path,
    expected_size: u64,
    max_retries: usize,
) -> Result<(), super::UsbError> {
    // Ensure parent directory exists
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    for attempt in 1..=max_retries {
        // Copy the file
        std::fs::copy(source, destination)?;

        // Verify size matches
        match std::fs::metadata(destination) {
            Ok(meta) if meta.len() == expected_size => {
                return Ok(());
            }
            Ok(meta) => {
                log::warn!(
                    "Size mismatch on attempt {} for {}: expected {} got {}",
                    attempt,
                    destination.display(),
                    expected_size,
                    meta.len()
                );
                if attempt == max_retries {
                    return Err(super::UsbError::SizeMismatch {
                        path: destination.to_path_buf(),
                        expected: expected_size,
                        actual: meta.len(),
                    });
                }
            }
            Err(e) => {
                log::warn!(
                    "Size verification failed on attempt {} for {}: {}",
                    attempt,
                    destination.display(),
                    e
                );
                if attempt == max_retries {
                    return Err(super::UsbError::IoError(e.to_string()));
                }
            }
        }
    }

    Err(super::UsbError::SizeMismatch {
        path: destination.to_path_buf(),
        expected: expected_size,
        actual: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test TrackInfo with default new fields
    fn test_track_info(path: &str, filename: &str, size: u64, mtime: SystemTime) -> TrackInfo {
        TrackInfo {
            path: PathBuf::from(path),
            filename: filename.to_string(),
            size,
            mtime,
            db_track: None,
            cue_points: Vec::new(),
            saved_loops: Vec::new(),
            stem_links: Vec::new(),
            ml_analysis: None,
            tags: Vec::new(),
            has_audio_features: false,
        }
    }

    #[test]
    fn test_sync_plan_summary() {
        let plan = SyncPlan {
            tracks_to_copy: vec![TrackCopy {
                source: PathBuf::from("/tmp/test.wav"),
                destination: PathBuf::from("tracks/test.wav"),
                size: 10_000_000,
            }],
            tracks_to_update: vec![],
            tracks_to_delete: vec![],
            playlist_tracks_to_add: vec![PlaylistTrack {
                playlist: "My Playlist".to_string(),
                track_filename: "test.wav".to_string(),
            }],
            playlist_tracks_to_remove: vec![],
            playlists_to_create: vec![PlaylistExportInfo {
                name: "My Playlist".to_string(),
                parent_name: None,
                sort_order: 0,
            }],
            playlists_to_delete: vec![],
            total_bytes: 10_000_000,
            tracks_missing_lufs: vec![],
        };

        let summary = plan.summary();
        assert!(summary.contains("1 tracks to copy"));
        assert!(summary.contains("10MB")); // format_bytes rounds to whole MB for < 1GB
    }

    #[test]
    fn test_build_sync_plan_empty() {
        let local = CollectionState::default();
        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);
        assert!(plan.is_empty());
    }

    #[test]
    fn test_build_sync_plan_new_track() {
        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            test_track_info("/tmp/test.wav", "test.wav", 1000, SystemTime::now()),
        );

        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);

        assert_eq!(plan.tracks_to_copy.len(), 1);
        assert!(plan.tracks_to_delete.is_empty());
    }

    #[test]
    fn test_build_sync_plan_unchanged_track() {
        let mtime = SystemTime::now();
        let track = test_track_info("/tmp/test.wav", "test.wav", 1000, mtime);

        let mut local = CollectionState::default();
        local.tracks.insert("test.wav".to_string(), track.clone());

        let mut usb = CollectionState::default();
        usb.tracks.insert("test.wav".to_string(), track);

        let plan = build_sync_plan(&local, &usb);
        assert!(plan.tracks_to_copy.is_empty());
    }

    #[test]
    fn test_build_sync_plan_size_changed() {
        let mtime = SystemTime::now();

        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            test_track_info("/tmp/test.wav", "test.wav", 2000, mtime),
        );

        let mut usb = CollectionState::default();
        usb.tracks.insert(
            "test.wav".to_string(),
            test_track_info("/usb/test.wav", "test.wav", 1000, mtime),
        );

        let plan = build_sync_plan(&local, &usb);
        assert_eq!(plan.tracks_to_copy.len(), 1);
    }

    #[test]
    fn test_build_sync_plan_mtime_newer() {
        use std::time::Duration;

        let old_mtime = SystemTime::now() - Duration::from_secs(60);
        let new_mtime = SystemTime::now();

        let mut local = CollectionState::default();
        local.tracks.insert(
            "test.wav".to_string(),
            test_track_info("/tmp/test.wav", "test.wav", 1000, new_mtime),
        );

        let mut usb = CollectionState::default();
        usb.tracks.insert(
            "test.wav".to_string(),
            test_track_info("/usb/test.wav", "test.wav", 1000, old_mtime),
        );

        let plan = build_sync_plan(&local, &usb);
        assert_eq!(plan.tracks_to_copy.len(), 1);
    }

    #[test]
    fn test_build_sync_plan_metadata_only_update() {
        let mtime = SystemTime::now();

        let mut local_track = test_track_info("/tmp/test.wav", "test.wav", 1000, mtime);
        local_track.tags = vec![("Techno".to_string(), Some("#3b82f6".to_string()))];

        let usb_track = test_track_info("/usb/test.wav", "test.wav", 1000, mtime);

        let mut local = CollectionState::default();
        local.tracks.insert("test.wav".to_string(), local_track);

        let mut usb = CollectionState::default();
        usb.tracks.insert("test.wav".to_string(), usb_track);

        let plan = build_sync_plan(&local, &usb);
        assert!(plan.tracks_to_copy.is_empty(), "Should not trigger WAV copy");
        assert_eq!(plan.tracks_to_update.len(), 1, "Should trigger metadata-only update");
    }

    #[test]
    fn test_build_sync_plan_nested_playlists() {
        let mut local = CollectionState::default();
        // Root playlist "Live Sets"
        local.playlists.insert(PlaylistExportInfo {
            name: "Live Sets".to_string(),
            parent_name: None,
            sort_order: 0,
        });
        // Child playlist "Opening" under "Live Sets"
        local.playlists.insert(PlaylistExportInfo {
            name: "Opening".to_string(),
            parent_name: Some("Live Sets".to_string()),
            sort_order: 0,
        });

        let usb = CollectionState::default();
        let plan = build_sync_plan(&local, &usb);

        // Both playlists should be created
        assert_eq!(plan.playlists_to_create.len(), 2);
        // Parent should come first (topological sort)
        assert_eq!(plan.playlists_to_create[0].name, "Live Sets");
        assert!(plan.playlists_to_create[0].parent_name.is_none());
        assert_eq!(plan.playlists_to_create[1].name, "Opening");
        assert_eq!(plan.playlists_to_create[1].parent_name, Some("Live Sets".to_string()));
    }

    #[test]
    fn test_build_sync_plan_nested_playlist_already_on_usb() {
        let mut local = CollectionState::default();
        local.playlists.insert(PlaylistExportInfo {
            name: "Live Sets".to_string(),
            parent_name: None,
            sort_order: 0,
        });
        local.playlists.insert(PlaylistExportInfo {
            name: "Opening".to_string(),
            parent_name: Some("Live Sets".to_string()),
            sort_order: 0,
        });

        // USB already has both playlists
        let mut usb = CollectionState::default();
        usb.playlists.insert(PlaylistExportInfo {
            name: "Live Sets".to_string(),
            parent_name: None,
            sort_order: 0,
        });
        usb.playlists.insert(PlaylistExportInfo {
            name: "Opening".to_string(),
            parent_name: Some("Live Sets".to_string()),
            sort_order: 0,
        });

        let plan = build_sync_plan(&local, &usb);
        assert!(plan.playlists_to_create.is_empty(), "No playlists should be created");
        assert!(plan.playlists_to_delete.is_empty(), "No playlists should be deleted");
    }

    #[test]
    fn test_build_qualified_name() {
        use crate::db::Playlist;

        let parent = Playlist { id: 1, parent_id: None, name: "Live Sets".to_string(), sort_order: 0 };
        let child = Playlist { id: 2, parent_id: Some(1), name: "Opening".to_string(), sort_order: 0 };
        let grandchild = Playlist { id: 3, parent_id: Some(2), name: "Warm Up".to_string(), sort_order: 0 };

        let id_map: HashMap<i64, &Playlist> = vec![
            (1, &parent), (2, &child), (3, &grandchild)
        ].into_iter().collect();

        assert_eq!(build_qualified_name(&parent, &id_map), "Live Sets");
        assert_eq!(build_qualified_name(&child, &id_map), "Live Sets/Opening");
        assert_eq!(build_qualified_name(&grandchild, &id_map), "Live Sets/Opening/Warm Up");
    }
}
