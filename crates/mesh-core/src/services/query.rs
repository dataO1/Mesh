//! QueryService - Background service for database queries
//!
//! This service runs in a dedicated thread and handles all database operations,
//! keeping the UI thread responsive. Commands are received via channels and
//! results are sent back through oneshot reply channels.

use super::messages::{QueryCommand, AppEvent, ServiceHandle};
use crate::db::{DatabaseService, Track, PlaylistQuery, SimilarityQuery};
use crossbeam::channel::{Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// QueryService handles all database operations in a background thread
pub struct QueryService {
    service: Arc<DatabaseService>,
    command_rx: Receiver<QueryCommand>,
    event_tx: Sender<AppEvent>,
}

impl QueryService {
    /// Spawn a new QueryService in a background thread
    ///
    /// Returns a handle for sending commands to the service.
    pub fn spawn(
        db_service: Arc<DatabaseService>,
        event_tx: Sender<AppEvent>,
    ) -> Result<ServiceHandle<QueryCommand>, String> {
        let (command_tx, command_rx) = crossbeam::channel::unbounded();

        let service = QueryService {
            service: db_service,
            command_rx,
            event_tx: event_tx.clone(),
        };

        // Spawn service thread
        let handle = thread::Builder::new()
            .name("query-service".into())
            .spawn(move || {
                service.run();
            })
            .map_err(|e| format!("Failed to spawn query service thread: {}", e))?;

        // Notify that service started
        let _ = event_tx.send(AppEvent::ServiceStarted {
            service_name: "QueryService".to_string(),
        });

        Ok(ServiceHandle {
            command_tx,
            thread_handle: Some(handle),
        })
    }

    /// Main service loop
    fn run(self) {
        log::info!("QueryService started");

        while let Ok(cmd) = self.command_rx.recv() {
            match cmd {
                QueryCommand::Shutdown => {
                    log::info!("QueryService shutting down");
                    break;
                }
                _ => self.handle_command(cmd),
            }
        }

        let _ = self.event_tx.send(AppEvent::ServiceStopped {
            service_name: "QueryService".to_string(),
        });

        log::info!("QueryService stopped");
    }

    /// Handle a single command
    fn handle_command(&self, cmd: QueryCommand) {
        match cmd {
            QueryCommand::GetTracksInFolder { folder_path, reply } => {
                // Use DatabaseService method which returns Track (not TrackRow)
                let result = self.service.get_tracks_in_folder(&folder_path)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrack { track_id, reply } => {
                // Use DatabaseService method which returns Track with full metadata
                let result = self.service.get_track(track_id)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrackByPath { path, reply } => {
                // Use DatabaseService method which returns Track with full metadata
                let result = self.service.get_track_by_path(&path)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::Search { query, limit, reply } => {
                // Use DatabaseService method which returns Track (not TrackRow)
                let result = self.service.search_tracks(&query, limit)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetFolders { reply } => {
                let result = self.service.get_folders()
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrackCount { reply } => {
                let result = self.service.track_count()
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::FindSimilar { track_id, limit, reply } => {
                let result = self.service.find_similar_tracks_ml(track_id, limit)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::FindHarmonicMatches { track_id, limit, reply } => {
                // Convert TrackRow to Track using from_row_only (available within mesh-core)
                let result = SimilarityQuery::find_harmonic_compatible(self.service.db(), track_id, limit)
                    .map(|rows| rows.into_iter().map(Track::from_row_only).collect())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetPlaylists { reply } => {
                let result = PlaylistQuery::get_all(self.service.db())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetPlaylistTracks { playlist_id, reply } => {
                // Use DatabaseService method which returns Vec<Track>
                let result = self.service.get_playlist_tracks(playlist_id)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::UpsertTrack { track, reply } => {
                // Use DatabaseService method which handles Track -> TrackRow conversion
                let result = self.service.save_track(&track)
                    .map(|_| ()) // save_track returns the ID, but channel expects ()
                    .map_err(|e| e.to_string());

                if result.is_ok() {
                    let track_id = track.id.unwrap_or(0);
                    let _ = self.event_tx.send(AppEvent::TrackUpdated {
                        track_id,
                        track: track.clone(),
                    });
                }

                let _ = reply.send(result);
            }

            QueryCommand::DeleteTrack { track_id, reply } => {
                let result = self.service.delete_track(track_id)
                    .map_err(|e| e.to_string());

                if result.is_ok() {
                    let _ = self.event_tx.send(AppEvent::TrackRemoved(track_id));
                }

                let _ = reply.send(result);
            }

            QueryCommand::Shutdown => {
                // Handled in run() loop
            }
        }
    }

}

/// Client for interacting with the QueryService
///
/// Provides a convenient async-like API using oneshot channels.
pub struct QueryClient {
    command_tx: crossbeam::channel::Sender<QueryCommand>,
}

impl QueryClient {
    /// Create a new client from a service handle
    pub fn new(handle: &ServiceHandle<QueryCommand>) -> Self {
        Self {
            command_tx: handle.command_tx.clone(),
        }
    }

    /// Get tracks in a folder (blocking)
    pub fn get_tracks_in_folder(&self, folder_path: &str) -> Result<Vec<Track>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::GetTracksInFolder {
                folder_path: folder_path.to_string(),
                reply: tx,
            })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Get a track by ID (blocking)
    pub fn get_track(&self, track_id: i64) -> Result<Option<Track>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::GetTrack { track_id, reply: tx })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Search tracks (blocking)
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Track>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::Search {
                query: query.to_string(),
                limit,
                reply: tx,
            })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Get track count (blocking)
    pub fn get_track_count(&self) -> Result<usize, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::GetTrackCount { reply: tx })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Find similar tracks (blocking)
    pub fn find_similar(&self, track_id: i64, limit: usize) -> Result<Vec<(Track, f32)>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::FindSimilar {
                track_id,
                limit,
                reply: tx,
            })
            .map_err(|e| e.to_string())?;

        rx.blocking_recv().map_err(|e| e.to_string())?
    }

    /// Shutdown the service
    pub fn shutdown(&self) -> Result<(), String> {
        self.command_tx
            .send(QueryCommand::Shutdown)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::messages::EventBus;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_query_service_lifecycle() {
        let event_bus = EventBus::new(16);
        let temp_dir = TempDir::new().unwrap();
        let db_service = DatabaseService::in_memory(temp_dir.path()).unwrap();

        let handle = QueryService::spawn(db_service, event_bus.sender()).unwrap();
        let client = QueryClient::new(&handle);

        // Test basic operations
        let count = client.get_track_count().unwrap();
        assert_eq!(count, 0);

        // Shutdown
        client.shutdown().unwrap();

        // Wait for thread to finish
        if let Some(h) = handle.thread_handle {
            h.join().unwrap();
        }
    }

    #[test]
    fn test_query_service_crud() {
        let event_bus = EventBus::new(16);
        let temp_dir = TempDir::new().unwrap();
        let db_service = DatabaseService::in_memory(temp_dir.path()).unwrap();

        let handle = QueryService::spawn(db_service, event_bus.sender()).unwrap();

        // Insert a track
        let track = Track {
            id: Some(42),
            path: PathBuf::from("/music/test.flac"),
            folder_path: "/music".to_string(),
            title: "Test Track".to_string(),
            original_name: String::new(),
            artist: Some("Test Artist".to_string()),
            bpm: Some(128.0),
            original_bpm: Some(128.0),
            key: Some("8A".to_string()),
            duration_seconds: 180.0,
            lufs: Some(-8.0),
            integrated_lufs: Some(-10.0),
            drop_marker: None,
            first_beat_sample: 0,
            file_mtime: 1234567890,
            file_size: 1000000,
            waveform_path: None,
            cue_points: Vec::new(),
            saved_loops: Vec::new(),
            stem_links: Vec::new(),
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        handle.command_tx.send(QueryCommand::UpsertTrack {
            track: track.clone(),
            reply: tx,
        }).unwrap();
        rx.blocking_recv().unwrap().unwrap();

        // Verify count
        let client = QueryClient::new(&handle);
        let count = client.get_track_count().unwrap();
        assert_eq!(count, 1);

        // Get track back
        let retrieved = client.get_track(42).unwrap().unwrap();
        assert_eq!(retrieved.title, "Test Track");

        // Cleanup
        client.shutdown().unwrap();
    }
}
