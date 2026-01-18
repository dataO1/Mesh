//! QueryService - Background service for database queries
//!
//! This service runs in a dedicated thread and handles all database operations,
//! keeping the UI thread responsive. Commands are received via channels and
//! results are sent back through oneshot reply channels.

use super::messages::{QueryCommand, AppEvent, ServiceHandle, EnergyDirection, MixSuggestion, MixReason};
use crate::db::{DatabaseService, Track, TrackQuery, PlaylistQuery, SimilarityQuery};
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
                let result = TrackQuery::get_by_folder(self.service.db(), &folder_path)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrack { track_id, reply } => {
                let result = TrackQuery::get_by_id(self.service.db(), track_id)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrackByPath { path, reply } => {
                let result = TrackQuery::get_by_path(self.service.db(), &path)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::Search { query, limit, reply } => {
                let result = TrackQuery::search(self.service.db(), &query, limit)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetFolders { reply } => {
                let result = TrackQuery::get_folders(self.service.db())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetTrackCount { reply } => {
                let result = TrackQuery::count(self.service.db())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::FindSimilar { track_id, limit, reply } => {
                let result = SimilarityQuery::find_similar(self.service.db(), track_id, limit)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::FindHarmonicMatches { track_id, limit, reply } => {
                let result = SimilarityQuery::find_harmonic_compatible(self.service.db(), track_id, limit)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetMixSuggestions { current_track_id, energy_direction, limit, reply } => {
                let result = self.get_mix_suggestions(current_track_id, energy_direction, limit);
                let _ = reply.send(result);
            }

            QueryCommand::GetPlaylists { reply } => {
                let result = PlaylistQuery::get_all(self.service.db())
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::GetPlaylistTracks { playlist_id, reply } => {
                let result = PlaylistQuery::get_tracks(self.service.db(), playlist_id)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }

            QueryCommand::UpsertTrack { track, reply } => {
                let result = TrackQuery::upsert(self.service.db(), &track)
                    .map_err(|e| e.to_string());

                if result.is_ok() {
                    let _ = self.event_tx.send(AppEvent::TrackUpdated {
                        track_id: track.id,
                        track: track.clone(),
                    });
                }

                let _ = reply.send(result);
            }

            QueryCommand::DeleteTrack { track_id, reply } => {
                let result = TrackQuery::delete(self.service.db(), track_id)
                    .map_err(|e| e.to_string());

                if result.is_ok() {
                    let _ = self.event_tx.send(AppEvent::TrackRemoved(track_id));
                }

                let _ = reply.send(result);
            }

            QueryCommand::UpdateAudioFeatures { track_id, features, reply } => {
                let result = SimilarityQuery::upsert_features(self.service.db(), track_id, &features)
                    .map_err(|e| e.to_string());

                if result.is_ok() {
                    let _ = self.event_tx.send(AppEvent::AnalysisComplete {
                        track_id,
                        features,
                    });
                }

                let _ = reply.send(result);
            }

            QueryCommand::Shutdown => {
                // Handled in run() loop
            }
        }
    }

    /// Get mix suggestions based on energy direction
    fn get_mix_suggestions(
        &self,
        current_track_id: i64,
        energy_direction: EnergyDirection,
        limit: usize,
    ) -> Result<Vec<MixSuggestion>, String> {
        // Get current track info
        let current_track = TrackQuery::get_by_id(self.service.db(), current_track_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Current track not found".to_string())?;

        let current_bpm = current_track.bpm.unwrap_or(120.0);
        let current_lufs = current_track.lufs.unwrap_or(-8.0);

        // Build query based on energy direction
        let (bpm_min, bpm_max, _lufs_condition) = match energy_direction {
            EnergyDirection::Maintain => {
                (current_bpm - 4.0, current_bpm + 4.0, "abs(lufs - $current_lufs) < 3")
            }
            EnergyDirection::BuildUp => {
                (current_bpm - 2.0, current_bpm + 8.0, "lufs > $current_lufs")
            }
            EnergyDirection::CoolDown => {
                (current_bpm - 8.0, current_bpm + 2.0, "lufs < $current_lufs")
            }
        };

        // Query for BPM-compatible tracks
        let query = format!(r#"
            ?[id, path, folder_path, name, artist, bpm, original_bpm, key,
              duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path] :=
                *tracks{{id, path, folder_path, name, artist, bpm, original_bpm, key,
                        duration_seconds, lufs, drop_marker, file_mtime, file_size, waveform_path}},
                id != $current_id,
                is_not_null(bpm),
                bpm >= $bpm_min,
                bpm <= $bpm_max
            :limit $limit
            :order abs(bpm - $current_bpm)
        "#);

        let mut params = std::collections::BTreeMap::new();
        params.insert("current_id".to_string(), cozo::DataValue::from(current_track_id));
        params.insert("current_bpm".to_string(), cozo::DataValue::from(current_bpm));
        params.insert("current_lufs".to_string(), cozo::DataValue::from(current_lufs as f64));
        params.insert("bpm_min".to_string(), cozo::DataValue::from(bpm_min));
        params.insert("bpm_max".to_string(), cozo::DataValue::from(bpm_max));
        params.insert("limit".to_string(), cozo::DataValue::from(limit as i64));

        let result = self.service.db().run_query(&query, params)
            .map_err(|e| e.to_string())?;

        // Convert results to MixSuggestions
        let suggestions: Vec<MixSuggestion> = result.rows.iter()
            .filter_map(|row| {
                let track = Track {
                    id: row.get(0)?.get_int()?,
                    path: row.get(1)?.get_str()?.to_string(),
                    folder_path: row.get(2)?.get_str()?.to_string(),
                    name: row.get(3)?.get_str()?.to_string(),
                    artist: row.get(4)?.get_str().map(|s| s.to_string()),
                    bpm: row.get(5)?.get_float(),
                    original_bpm: row.get(6)?.get_float(),
                    key: row.get(7)?.get_str().map(|s| s.to_string()),
                    duration_seconds: row.get(8)?.get_float().unwrap_or(0.0),
                    lufs: row.get(9)?.get_float().map(|f| f as f32),
                    drop_marker: row.get(10)?.get_int(),
                    file_mtime: row.get(11)?.get_int().unwrap_or(0),
                    file_size: row.get(12)?.get_int().unwrap_or(0),
                    waveform_path: row.get(13)?.get_str().map(|s| s.to_string()),
                };

                let bpm_diff = (track.bpm.unwrap_or(current_bpm) - current_bpm).abs();
                let score = 1.0 - (bpm_diff / 16.0).min(1.0); // Normalize to 0-1

                Some(MixSuggestion {
                    track,
                    reason: MixReason::BpmCompatible { bpm_diff: bpm_diff as f32 },
                    score: score as f32,
                })
            })
            .collect();

        Ok(suggestions)
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

    /// Get mix suggestions (blocking)
    pub fn get_mix_suggestions(
        &self,
        current_track_id: i64,
        energy_direction: EnergyDirection,
        limit: usize,
    ) -> Result<Vec<MixSuggestion>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(QueryCommand::GetMixSuggestions {
                current_track_id,
                energy_direction,
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
            id: 42,
            path: "/music/test.wav".to_string(),
            folder_path: "/music".to_string(),
            name: "Test Track".to_string(),
            artist: Some("Test Artist".to_string()),
            bpm: Some(128.0),
            original_bpm: Some(128.0),
            key: Some("8A".to_string()),
            duration_seconds: 180.0,
            lufs: Some(-8.0),
            drop_marker: None,
            file_mtime: 1234567890,
            file_size: 1000000,
            waveform_path: None,
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
        assert_eq!(retrieved.name, "Test Track");

        // Cleanup
        client.shutdown().unwrap();
    }
}
