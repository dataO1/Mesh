//! Export service with thread pool for atomic per-track exports
//!
//! This service owns a rayon thread pool and coordinates USB exports.
//! Each track export is atomic: WAV copy + DB sync + progress callback.

use super::ExportProgress;
use crate::db::DatabaseService;
use crate::usb::get_or_open_usb_database;
use crate::usb::sync::{copy_with_verification, SyncPlan, PlaylistTrack};

use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Thread pool service for USB export operations
///
/// Owns the thread pool and coordinates per-track atomic exports.
/// Each worker thread handles: WAV copy -> DB sync -> progress callback
pub struct ExportService {
    /// Thread pool for parallel track exports
    thread_pool: rayon::ThreadPool,
    /// Cancellation flag shared with workers
    cancel_flag: Arc<AtomicBool>,
}

impl ExportService {
    /// Create a new export service with 4 worker threads
    ///
    /// The thread pool is reusable - create once at startup, not per export.
    pub fn new() -> Self {
        let thread_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
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
    /// Runs tracks through the thread pool, each handling:
    /// 1. WAV copy with verification (3 retries)
    /// 2. Atomic DB sync (batch inserts)
    /// 3. Progress notification
    ///
    /// Returns a receiver for progress messages. The export runs in the
    /// background - poll the receiver for updates.
    ///
    /// # Arguments
    /// * `plan` - The sync plan (what to copy/delete)
    /// * `local_db` - The local database (source of track metadata)
    /// * `usb_collection_root` - Path to mesh-collection/ on USB
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

        // Move into thread pool scope
        let total_tracks = plan.tracks_to_copy.len();
        let total_bytes = plan.total_bytes;
        let tracks_to_copy = plan.tracks_to_copy.clone();
        let playlists_to_create = plan.playlists_to_create.clone();
        let playlist_tracks_to_add = plan.playlist_tracks_to_add.clone();
        let playlist_tracks_to_remove = plan.playlist_tracks_to_remove.clone();
        let playlists_to_delete = plan.playlists_to_delete.clone();
        let tracks_to_delete = plan.tracks_to_delete.clone();

        self.thread_pool.spawn(move || {
            let start_time = Instant::now();

            // Send started message
            let _ = progress_tx.send(ExportProgress::Started {
                total_tracks,
                total_bytes,
            });

            // Thread-safe counters
            let tracks_complete = AtomicUsize::new(0);
            let bytes_complete = AtomicU64::new(0);
            let tracks_failed = AtomicUsize::new(0);
            let failed_files: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

            // Get USB database from cache (or open if not cached)
            let usb_db = match get_or_open_usb_database(&usb_root) {
                Some(db) => db,
                None => {
                    log::error!("Failed to open USB database");
                    let _ = progress_tx.send(ExportProgress::Complete {
                        duration: start_time.elapsed(),
                        tracks_exported: 0,
                        failed_files: vec![("database".to_string(), "Failed to open".to_string())],
                    });
                    return;
                }
            };

            // Phase 1: Create playlists (sequential, must happen before tracks)
            for playlist_name in &playlists_to_create {
                if let Err(e) = usb_db.create_playlist(playlist_name, None) {
                    log::warn!("Failed to create playlist {}: {}", playlist_name, e);
                }
            }

            // Phase 2: Export tracks in parallel (WAV copy + DB sync atomic)
            tracks_to_copy.par_iter().enumerate().for_each(|(index, track)| {
                if cancel_flag.load(Ordering::Relaxed) {
                    return;
                }

                let filename = track
                    .source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();

                // Notify track started
                let _ = progress_tx.send(ExportProgress::TrackStarted {
                    filename: filename.clone(),
                    track_index: index,
                });

                // Step 1: Copy WAV with verification
                let dest_path = usb_root.join(&track.destination);
                if let Err(e) = copy_with_verification(&track.source, &dest_path, track.size, 3) {
                    log::error!("Failed to copy {}: {}", filename, e);
                    tracks_failed.fetch_add(1, Ordering::Relaxed);
                    failed_files.lock().unwrap().push((filename.clone(), e.to_string()));
                    let _ = progress_tx.send(ExportProgress::TrackFailed {
                        filename,
                        track_index: index,
                        error: e.to_string(),
                    });
                    return;
                }

                // Step 2: Sync track to USB database (atomic batch inserts)
                let source_path_str = track.source.to_string_lossy().to_string();
                let local_track = match local_db.get_track_by_path(&source_path_str) {
                    Ok(Some(t)) => t,
                    Ok(None) => {
                        log::warn!("Track {} not found in local DB, skipping metadata sync", filename);
                        // WAV copied successfully, just no metadata
                        let _ = tracks_complete.fetch_add(1, Ordering::Relaxed);
                        let bytes = bytes_complete.fetch_add(track.size, Ordering::Relaxed) + track.size;
                        let _ = progress_tx.send(ExportProgress::TrackComplete {
                            filename,
                            track_index: index,
                            total_tracks,
                            bytes_complete: bytes,
                            total_bytes,
                        });
                        return;
                    }
                    Err(e) => {
                        log::warn!("Failed to get track {} from local DB: {}", filename, e);
                        tracks_failed.fetch_add(1, Ordering::Relaxed);
                        failed_files.lock().unwrap().push((filename.clone(), e.to_string()));
                        let _ = progress_tx.send(ExportProgress::TrackFailed {
                            filename,
                            track_index: index,
                            error: e.to_string(),
                        });
                        return;
                    }
                };

                // Create USB track with updated path
                let mut usb_track = local_track.clone();
                usb_track.id = None; // Generate new ID for USB database
                usb_track.path = dest_path.clone();
                usb_track.folder_path = "tracks".to_string();
                usb_track.name = filename.trim_end_matches(".wav").to_string();

                // Sync with atomic batch inserts
                if let Err(e) = usb_db.sync_track_atomic(&usb_track, &local_db) {
                    log::error!("Failed to sync track {} to USB DB: {}", filename, e);
                    tracks_failed.fetch_add(1, Ordering::Relaxed);
                    failed_files.lock().unwrap().push((filename.clone(), e.to_string()));
                    let _ = progress_tx.send(ExportProgress::TrackFailed {
                        filename,
                        track_index: index,
                        error: e.to_string(),
                    });
                    return;
                }

                // Step 3: Success - update counters and send progress
                let _ = tracks_complete.fetch_add(1, Ordering::Relaxed);
                let bytes = bytes_complete.fetch_add(track.size, Ordering::Relaxed) + track.size;

                let _ = progress_tx.send(ExportProgress::TrackComplete {
                    filename,
                    track_index: index,
                    total_tracks,
                    bytes_complete: bytes,
                    total_bytes,
                });
            });

            // Check for cancellation
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = progress_tx.send(ExportProgress::Cancelled);
                return;
            }

            // Phase 3-4: Update playlist memberships (parallel with progress)
            let tracks_dir = usb_root.join("tracks");
            let total_playlist_ops = playlist_tracks_to_add.len() + playlist_tracks_to_remove.len();

            if total_playlist_ops > 0 {
                let _ = progress_tx.send(ExportProgress::PlaylistOpsStarted {
                    total_operations: total_playlist_ops,
                });

                let playlist_ops_complete = AtomicUsize::new(0);

                // Parallelize playlist track additions
                playlist_tracks_to_add.par_iter().for_each(|playlist_track| {
                    add_track_to_playlist(&usb_db, &tracks_dir, playlist_track);
                    let completed = playlist_ops_complete.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = progress_tx.send(ExportProgress::PlaylistOpComplete {
                        completed,
                        total: total_playlist_ops,
                    });
                });

                // Parallelize playlist track removals
                playlist_tracks_to_remove.par_iter().for_each(|playlist_track| {
                    remove_track_from_playlist(&usb_db, &tracks_dir, playlist_track);
                    let completed = playlist_ops_complete.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = progress_tx.send(ExportProgress::PlaylistOpComplete {
                        completed,
                        total: total_playlist_ops,
                    });
                });
            }

            // Phase 5: Delete playlists
            for playlist_name in &playlists_to_delete {
                if let Ok(Some(playlist)) = usb_db.get_playlist_by_name(playlist_name, None) {
                    if let Err(e) = usb_db.delete_playlist(playlist.id) {
                        log::warn!("Failed to delete playlist {}: {}", playlist_name, e);
                    }
                }
            }

            // Phase 6: Delete tracks from USB
            for filename in &tracks_to_delete {
                let track_path = tracks_dir.join(filename);
                if track_path.exists() {
                    if let Err(e) = std::fs::remove_file(&track_path) {
                        log::warn!("Failed to delete track {}: {}", track_path.display(), e);
                    }
                }
                // Also delete from database
                let track_path_str = track_path.to_string_lossy().to_string();
                if let Ok(Some(track)) = usb_db.get_track_by_path(&track_path_str) {
                    if let Some(track_id) = track.id {
                        if let Err(e) = usb_db.delete_track(track_id) {
                            log::warn!("Failed to delete track {} from USB DB: {}", filename, e);
                        }
                    }
                }
            }

            // Send completion
            let failed = failed_files.into_inner().unwrap();
            let _ = progress_tx.send(ExportProgress::Complete {
                duration: start_time.elapsed(),
                tracks_exported: tracks_complete.load(Ordering::Relaxed),
                failed_files: failed,
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

/// Add a track to a playlist in the USB database
fn add_track_to_playlist(
    usb_db: &DatabaseService,
    tracks_dir: &Path,
    playlist_track: &PlaylistTrack,
) {
    // Find the playlist ID
    if let Ok(Some(playlist)) = usb_db.get_playlist_by_name(&playlist_track.playlist, None) {
        // Find the track ID by filename
        let track_path = tracks_dir.join(&playlist_track.track_filename);
        let track_path_str = track_path.to_string_lossy().to_string();

        if let Ok(Some(track)) = usb_db.get_track_by_path(&track_path_str) {
            if let Some(track_id) = track.id {
                // Get next sort order and add track to playlist
                if let Ok(sort_order) = usb_db.next_playlist_sort_order(playlist.id) {
                    if let Err(e) = usb_db.add_track_to_playlist(playlist.id, track_id, sort_order) {
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

/// Remove a track from a playlist in the USB database
fn remove_track_from_playlist(
    usb_db: &DatabaseService,
    tracks_dir: &Path,
    playlist_track: &PlaylistTrack,
) {
    // Find the playlist ID
    if let Ok(Some(playlist)) = usb_db.get_playlist_by_name(&playlist_track.playlist, None) {
        // Find the track ID by filename
        let track_path = tracks_dir.join(&playlist_track.track_filename);
        let track_path_str = track_path.to_string_lossy().to_string();

        if let Ok(Some(track)) = usb_db.get_track_by_path(&track_path_str) {
            if let Some(track_id) = track.id {
                if let Err(e) = usb_db.remove_track_from_playlist(playlist.id, track_id) {
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
