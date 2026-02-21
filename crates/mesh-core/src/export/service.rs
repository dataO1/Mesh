//! Export service with optimized sequential pipeline for USB flash
//!
//! Key optimization: separates file I/O (sequential to USB) from DB I/O (local SSD).
//! The old approach interleaved WAV copies with DB writes via par_iter — worst of both
//! worlds for flash storage. The new pipeline:
//!
//! 1. Presets: Copy small YAML files to USB
//! 2. Staging: Copy USB mesh.db to local temp dir, open as DatabaseService
//! 3. WAV copy: Sequential 1 MB buffered writes to USB, fsync each file
//! 4. DB update: All metadata/playlist ops against local staging DB
//! 5. DB writeback: Copy staging DB back to USB (single large sequential write)
//! 6. Delete: Remove obsolete track files from USB
//!
//! This eliminates random writes to USB flash entirely.

use super::ExportProgress;
use crate::db::DatabaseService;
use crate::usb::cache::clear_usb_database;
use crate::usb::sync::{copy_large_file, SyncPlan, PlaylistTrack};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

/// Thread pool service for USB export operations
///
/// Owns a single-thread pool and coordinates the sequential export pipeline.
/// DB operations are batched against a local staging copy — not the USB drive.
pub struct ExportService {
    /// Thread pool for export (single thread — pipeline is sequential by design)
    thread_pool: rayon::ThreadPool,
    /// Cancellation flag shared with workers
    cancel_flag: Arc<AtomicBool>,
}

impl ExportService {
    /// Create a new export service with a single worker thread
    ///
    /// Sequential pipeline: parallel threads caused random I/O on USB flash.
    pub fn new() -> Self {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .thread_name(|i| format!("usb-export-{}", i))
            .build()
            .expect("Failed to create export thread pool");

        Self {
            thread_pool,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Execute the export plan
    ///
    /// Pipeline:
    /// 1. Copy presets to USB (small YAML files)
    /// 2. Stage USB database locally (fast SSD copy)
    /// 3. Sequential WAV copy to USB (1 MB buffered, fsync per file)
    /// 4. Update staging DB (metadata + playlists + deletions)
    /// 5. Write staging DB back to USB (single sequential copy)
    /// 6. Delete obsolete track files from USB
    ///
    /// Returns a receiver for progress messages.
    pub fn start_export(
        &self,
        plan: SyncPlan,
        local_db: Arc<DatabaseService>,
        usb_collection_root: &Path,
    ) -> Receiver<ExportProgress> {
        // Reset cancellation flag
        self.cancel_flag.store(false, Ordering::SeqCst);

        let (progress_tx, progress_rx) = channel();
        let cancel_flag = self.cancel_flag.clone();
        let usb_root = usb_collection_root.to_path_buf();

        let total_tracks = plan.tracks_to_copy.len() + plan.tracks_to_update.len();
        let total_bytes = plan.total_bytes;
        let tracks_to_copy = plan.tracks_to_copy.clone();
        let tracks_to_update = plan.tracks_to_update.clone();
        let playlists_to_create = plan.playlists_to_create.clone();
        let playlist_tracks_to_add = plan.playlist_tracks_to_add.clone();
        let playlist_tracks_to_remove = plan.playlist_tracks_to_remove.clone();
        let playlists_to_delete = plan.playlists_to_delete.clone();
        let tracks_to_delete = plan.tracks_to_delete.clone();

        self.thread_pool.spawn(move || {
            let start_time = Instant::now();
            let mut tracks_exported: usize = 0;
            let mut bytes_exported: u64 = 0;
            let mut failed_files: Vec<(String, String)> = Vec::new();

            // Send Started immediately so the UI transitions before any I/O
            let _ = progress_tx.send(ExportProgress::Started {
                total_tracks,
                total_bytes,
            });

            // ================================================================
            // Phase 0: Copy preset files (small YAML files)
            // ================================================================
            {
                let t = Instant::now();
                let local_root = local_db.collection_root();
                let presets_stems_src = local_root.join("presets/stems");
                let presets_decks_src = local_root.join("presets/decks");
                let slicer_src = local_root.join("slicer-presets.yaml");

                if presets_stems_src.exists() {
                    if let Err(e) = copy_dir_all(&presets_stems_src, &usb_root.join("presets/stems")) {
                        log::warn!("Failed to copy stem presets: {}", e);
                    }
                }
                if presets_decks_src.exists() {
                    if let Err(e) = copy_dir_all(&presets_decks_src, &usb_root.join("presets/decks")) {
                        log::warn!("Failed to copy deck presets: {}", e);
                    }
                }
                if slicer_src.exists() {
                    if let Err(e) = std::fs::copy(&slicer_src, &usb_root.join("slicer-presets.yaml")) {
                        log::warn!("Failed to copy slicer presets: {}", e);
                    }
                }
                log::info!("[export] Phase 0 (presets to USB): {:.1}s", t.elapsed().as_secs_f64());
                let _ = progress_tx.send(ExportProgress::PresetsCopied);
            }

            // ================================================================
            // Phase 1: Stage USB database locally
            // ================================================================
            // Copy mesh.db from USB to a local temp directory, then open it.
            // All DB operations happen against this fast local copy.
            let t_phase1 = Instant::now();
            let temp_dir = match tempfile::TempDir::new() {
                Ok(d) => d,
                Err(e) => {
                    log::error!("Failed to create temp directory for staging DB: {}", e);
                    let _ = progress_tx.send(ExportProgress::Complete {
                        duration: start_time.elapsed(),
                        tracks_exported: 0,
                        failed_files: vec![("staging".to_string(), format!("Temp dir creation failed: {}", e))],
                    });
                    return;
                }
            };

            let usb_db_path = usb_root.join("mesh.db");
            let staging_db_path = temp_dir.path().join("mesh.db");

            // Copy USB database to staging (sequential read from USB — fast)
            if usb_db_path.exists() {
                let db_size = std::fs::metadata(&usb_db_path).map(|m| m.len()).unwrap_or(0);
                log::info!("[export] USB mesh.db size: {:.1} MB", db_size as f64 / 1_048_576.0);
                if let Err(e) = std::fs::copy(&usb_db_path, &staging_db_path) {
                    log::error!("Failed to copy USB database to staging: {}", e);
                    let _ = progress_tx.send(ExportProgress::Complete {
                        duration: start_time.elapsed(),
                        tracks_exported: 0,
                        failed_files: vec![("staging".to_string(), format!("DB copy failed: {}", e))],
                    });
                    return;
                }
            }

            // Open staging database (temp path won't collide with USB_DB_CACHE)
            let t_open = Instant::now();
            let staging_db = match DatabaseService::new(temp_dir.path()) {
                Ok(db) => db,
                Err(e) => {
                    log::error!("Failed to open staging database: {}", e);
                    let _ = progress_tx.send(ExportProgress::Complete {
                        duration: start_time.elapsed(),
                        tracks_exported: 0,
                        failed_files: vec![("staging".to_string(), format!("DB open failed: {}", e))],
                    });
                    return;
                }
            };

            log::info!("[export] Phase 1 CozoDB open: {:.1}s", t_open.elapsed().as_secs_f64());
            log::info!("[export] Phase 1 (stage DB locally): {:.1}s", t_phase1.elapsed().as_secs_f64());

            // ================================================================
            // Phase 2: Sequential WAV copy to USB
            // ================================================================

            let t_phase2 = Instant::now();

            // Create playlists on staging DB before track copy (parents before children)
            for info in &playlists_to_create {
                let parent_id = info.parent_name.as_ref().and_then(|pname| {
                    staging_db.get_playlist_by_name(pname, None)
                        .ok()
                        .flatten()
                        .map(|p| p.id)
                });

                if let Err(e) = staging_db.create_playlist(&info.name, parent_id) {
                    log::warn!("Failed to create playlist {}: {}", info.name, e);
                }
            }

            for (index, track) in tracks_to_copy.iter().enumerate() {
                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = progress_tx.send(ExportProgress::Cancelled);
                    return;
                }

                let filename = track
                    .source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let _ = progress_tx.send(ExportProgress::TrackStarted {
                    filename: filename.clone(),
                    track_index: index,
                });

                // Copy WAV with buffered I/O + fsync
                let dest_path = usb_root.join(&track.destination);
                match copy_large_file(&track.source, &dest_path, |_| {}) {
                    Ok(bytes_written) => {
                        tracks_exported += 1;
                        bytes_exported += bytes_written;

                        let _ = progress_tx.send(ExportProgress::TrackComplete {
                            filename,
                            track_index: index,
                            total_tracks,
                            bytes_complete: bytes_exported,
                            total_bytes,
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to copy {}: {}", filename, e);
                        failed_files.push((filename.clone(), e.to_string()));
                        let _ = progress_tx.send(ExportProgress::TrackFailed {
                            filename,
                            track_index: index,
                            error: e.to_string(),
                        });
                    }
                }
            }

            log::info!(
                "[export] Phase 2 (WAV copy): {:.1}s — {} tracks copied, {} bytes",
                t_phase2.elapsed().as_secs_f64(),
                tracks_exported,
                bytes_exported,
            );

            // Check for cancellation after WAV copy phase
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = progress_tx.send(ExportProgress::Cancelled);
                return;
            }

            // ================================================================
            // Phase 3: Update staging database (all DB ops on local SSD)
            // ================================================================
            // Count total discrete operations for unified progress
            let total_db_ops = tracks_to_copy.len()
                + tracks_to_update.len()
                + playlist_tracks_to_add.len()
                + playlist_tracks_to_remove.len()
                + tracks_to_delete.len()
                + playlists_to_delete.len();

            let mut db_ops_completed: usize = 0;

            let t_phase3 = Instant::now();

            // Build filename→track_id lookup for metadata-only updates
            let t_lookup = Instant::now();
            let track_id_by_filename: HashMap<String, i64> = local_db
                .get_all_tracks()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| {
                    let fname = t.path.file_name()?.to_str()?.to_string();
                    Some((fname, t.id?))
                })
                .collect();
            log::info!(
                "[export] Phase 3 get_all_tracks lookup: {:.1}s ({} tracks)",
                t_lookup.elapsed().as_secs_f64(),
                track_id_by_filename.len(),
            );

            // 3a: Sync track metadata for newly copied tracks
            let t_3a = Instant::now();
            for track in &tracks_to_copy {
                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = progress_tx.send(ExportProgress::Cancelled);
                    return;
                }

                let filename = track
                    .source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let source_path_str = track.source.to_string_lossy().to_string();
                if let Ok(Some(local_track)) = local_db.get_track_by_path(&source_path_str) {
                    let source_track_id = local_track.id.unwrap_or(0);
                    let mut usb_track = local_track;
                    usb_track.id = None;
                    usb_track.path = track.destination.clone();
                    usb_track.folder_path = "tracks".to_string();
                    usb_track.name = filename.trim_end_matches(".wav").to_string();

                    if let Err(e) = staging_db.sync_track_atomic(&usb_track, &local_db, source_track_id) {
                        log::warn!("DB sync failed for {}: {}", filename, e);
                    }
                }

                db_ops_completed += 1;
                let _ = progress_tx.send(ExportProgress::UpdatingDatabase {
                    completed: db_ops_completed,
                    total: total_db_ops,
                });
            }

            log::info!(
                "[export] Phase 3a (sync copied tracks DB): {:.1}s — {} tracks",
                t_3a.elapsed().as_secs_f64(),
                tracks_to_copy.len(),
            );

            // 3b: Metadata-only sync for tracks that don't need WAV re-copy
            let t_3b = Instant::now();
            for filename in &tracks_to_update {
                if cancel_flag.load(Ordering::Relaxed) {
                    let _ = progress_tx.send(ExportProgress::Cancelled);
                    return;
                }

                let local_track = track_id_by_filename
                    .get(filename.as_str())
                    .and_then(|&id| local_db.get_track(id).ok().flatten());

                if let Some(local_track) = local_track {
                    let source_id = local_track.id.unwrap_or(0);
                    let mut usb_track = local_track;
                    usb_track.id = None;
                    usb_track.path = PathBuf::from(format!("tracks/{}", filename));
                    usb_track.folder_path = "tracks".to_string();
                    usb_track.name = filename.trim_end_matches(".wav").to_string();

                    if let Err(e) = staging_db.sync_track_atomic(&usb_track, &local_db, source_id) {
                        log::warn!("Metadata sync failed for {}: {}", filename, e);
                    }
                }

                db_ops_completed += 1;
                let _ = progress_tx.send(ExportProgress::UpdatingDatabase {
                    completed: db_ops_completed,
                    total: total_db_ops,
                });
            }

            log::info!(
                "[export] Phase 3b (metadata-only sync): {:.1}s — {} tracks",
                t_3b.elapsed().as_secs_f64(),
                tracks_to_update.len(),
            );

            // 3c: Playlist membership operations
            let tracks_dir_rel = Path::new("_unused");
            for playlist_track in &playlist_tracks_to_add {
                add_track_to_playlist(&staging_db, tracks_dir_rel, playlist_track);
                db_ops_completed += 1;
                let _ = progress_tx.send(ExportProgress::UpdatingDatabase {
                    completed: db_ops_completed,
                    total: total_db_ops,
                });
            }

            for playlist_track in &playlist_tracks_to_remove {
                remove_track_from_playlist(&staging_db, tracks_dir_rel, playlist_track);
                db_ops_completed += 1;
                let _ = progress_tx.send(ExportProgress::UpdatingDatabase {
                    completed: db_ops_completed,
                    total: total_db_ops,
                });
            }

            // 3d: Delete playlists (children before parents due to sort order)
            for info in &playlists_to_delete {
                let parent_id = info.parent_name.as_ref().and_then(|pname| {
                    staging_db.get_playlist_by_name(pname, None)
                        .ok()
                        .flatten()
                        .map(|p| p.id)
                });

                if let Ok(Some(playlist)) = staging_db.get_playlist_by_name(&info.name, parent_id) {
                    if let Err(e) = staging_db.delete_playlist(playlist.id) {
                        log::warn!("Failed to delete playlist {}: {}", info.name, e);
                    }
                }
            }

            // 3e: Delete tracks from staging DB
            for filename in &tracks_to_delete {
                let rel_path = format!("tracks/{}", filename);
                if let Ok(Some(track)) = staging_db.get_track_by_path(&rel_path) {
                    if let Some(track_id) = track.id {
                        if let Err(e) = staging_db.delete_track(track_id) {
                            log::warn!("Failed to delete track {} from staging DB: {}", filename, e);
                        }
                    }
                }

                db_ops_completed += 1;
                let _ = progress_tx.send(ExportProgress::UpdatingDatabase {
                    completed: db_ops_completed,
                    total: total_db_ops,
                });
            }

            log::info!(
                "[export] Phase 3 total (DB operations): {:.1}s",
                t_phase3.elapsed().as_secs_f64(),
            );

            // 3f: Drop staging DB to flush CozoDB WAL and release file lock
            let t = Instant::now();
            drop(staging_db);
            let staging_size = std::fs::metadata(&staging_db_path).map(|m| m.len()).unwrap_or(0);
            log::info!(
                "[export] CozoDB WAL flush: {:.1}s (staging DB: {:.1} MB)",
                t.elapsed().as_secs_f64(),
                staging_size as f64 / 1_048_576.0,
            );

            // 3g: Write staging DB back to USB (single large sequential write)
            if staging_db_path.exists() {
                let t = Instant::now();
                log::info!("Writing staging database back to USB...");
                if let Err(e) = copy_large_file(&staging_db_path, &usb_db_path, |_| {}) {
                    log::error!("Failed to write staging DB to USB: {}", e);
                    failed_files.push(("mesh.db".to_string(), format!("DB writeback failed: {}", e)));
                }
                log::info!("[export] DB writeback to USB: {:.1}s", t.elapsed().as_secs_f64());
            }

            // 3h: Invalidate cached USB database (will be lazily re-opened on next access)
            clear_usb_database(&usb_root);

            // ================================================================
            // Phase 4: Delete removed track files from USB
            // ================================================================
            let t_phase4 = Instant::now();
            let tracks_dir = usb_root.join("tracks");
            for filename in &tracks_to_delete {
                let track_path = tracks_dir.join(filename);
                if track_path.exists() {
                    if let Err(e) = std::fs::remove_file(&track_path) {
                        log::warn!("Failed to delete track {}: {}", track_path.display(), e);
                    }
                }
            }
            if !tracks_to_delete.is_empty() {
                log::info!(
                    "[export] Phase 4 (delete files): {:.1}s — {} files",
                    t_phase4.elapsed().as_secs_f64(),
                    tracks_to_delete.len(),
                );
            }

            // ================================================================
            // Phase 5: Complete
            // ================================================================
            log::info!(
                "[export] TOTAL: {:.1}s — {} tracks exported, {} failed",
                start_time.elapsed().as_secs_f64(),
                tracks_exported,
                failed_files.len(),
            );
            let _ = progress_tx.send(ExportProgress::Complete {
                duration: start_time.elapsed(),
                tracks_exported,
                failed_files,
            });
        });

        progress_rx
    }

    /// Cancel the current export
    ///
    /// Sets the cancellation flag - workers will stop at their next checkpoint.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Check if export is currently cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }
}

impl Default for ExportService {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a qualified playlist name (e.g., "Parent/Child") to a playlist ID
///
/// Walks down the hierarchy path, resolving each segment via get_playlist_by_name.
fn resolve_playlist_by_qualified_name(usb_db: &DatabaseService, qualified_name: &str) -> Option<i64> {
    let parts: Vec<&str> = qualified_name.split('/').collect();
    let mut parent_id: Option<i64> = None;

    for part in &parts {
        match usb_db.get_playlist_by_name(part, parent_id) {
            Ok(Some(playlist)) => {
                parent_id = Some(playlist.id);
            }
            _ => return None,
        }
    }

    parent_id // The last resolved ID is the target playlist
}

/// Add a track to a playlist in the USB database
fn add_track_to_playlist(
    usb_db: &DatabaseService,
    _tracks_dir: &Path,
    playlist_track: &PlaylistTrack,
) {
    // Find the playlist ID (handles hierarchical qualified names like "Parent/Child")
    if let Some(playlist_id) = resolve_playlist_by_qualified_name(usb_db, &playlist_track.playlist) {
        // Find the track ID by relative path (USB DB stores portable relative paths)
        let track_path_str = format!("tracks/{}", playlist_track.track_filename);

        if let Ok(Some(track)) = usb_db.get_track_by_path(&track_path_str) {
            if let Some(track_id) = track.id {
                // Get next sort order and add track to playlist
                if let Ok(sort_order) = usb_db.next_playlist_sort_order(playlist_id) {
                    if let Err(e) = usb_db.add_track_to_playlist(playlist_id, track_id, sort_order) {
                        log::warn!(
                            "Failed to add track {} to playlist {}: {}",
                            playlist_track.track_filename,
                            playlist_track.playlist,
                            e
                        );
                    }
                }
            }
        }
    }
}

/// Recursively copy a directory of files (used for preset directories)
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Remove a track from a playlist in the USB database
fn remove_track_from_playlist(
    usb_db: &DatabaseService,
    _tracks_dir: &Path,
    playlist_track: &PlaylistTrack,
) {
    // Find the playlist ID (handles hierarchical qualified names like "Parent/Child")
    if let Some(playlist_id) = resolve_playlist_by_qualified_name(usb_db, &playlist_track.playlist) {
        // Find the track ID by relative path (USB DB stores portable relative paths)
        let track_path_str = format!("tracks/{}", playlist_track.track_filename);

        if let Ok(Some(track)) = usb_db.get_track_by_path(&track_path_str) {
            if let Some(track_id) = track.id {
                if let Err(e) = usb_db.remove_track_from_playlist(playlist_id, track_id) {
                    log::warn!(
                        "Failed to remove track {} from playlist {}: {}",
                        playlist_track.track_filename,
                        playlist_track.playlist,
                        e
                    );
                }
            }
        }
    }
}
