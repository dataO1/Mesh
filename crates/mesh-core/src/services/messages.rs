//! Message types for service communication
//!
//! This module defines the command and event types used for inter-service
//! communication in the message-driven architecture. Commands are request-reply
//! patterns using oneshot channels, while events are broadcast to all subscribers.

use crate::db::{Track, Playlist, AudioFeatures};
use std::path::PathBuf;
use std::time::Duration;

// ============================================================================
// Query Commands (Request-Reply)
// ============================================================================

/// Commands sent to the QueryService
///
/// Each command includes a oneshot sender for the reply, enabling
/// async request-reply patterns without blocking the UI thread.
pub enum QueryCommand {
    /// Get all tracks in a specific folder
    GetTracksInFolder {
        folder_path: String,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Track>, String>>,
    },

    /// Get a single track by ID
    GetTrack {
        track_id: i64,
        reply: tokio::sync::oneshot::Sender<Result<Option<Track>, String>>,
    },

    /// Get a track by file path
    GetTrackByPath {
        path: String,
        reply: tokio::sync::oneshot::Sender<Result<Option<Track>, String>>,
    },

    /// Search tracks by name or artist
    Search {
        query: String,
        limit: usize,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Track>, String>>,
    },

    /// Get all unique folder paths in the collection
    GetFolders {
        reply: tokio::sync::oneshot::Sender<Result<Vec<String>, String>>,
    },

    /// Get total track count
    GetTrackCount {
        reply: tokio::sync::oneshot::Sender<Result<usize, String>>,
    },

    /// Find similar tracks using vector similarity
    FindSimilar {
        track_id: i64,
        limit: usize,
        reply: tokio::sync::oneshot::Sender<Result<Vec<(Track, f32)>, String>>,
    },

    /// Find harmonically compatible tracks
    FindHarmonicMatches {
        track_id: i64,
        limit: usize,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Track>, String>>,
    },

    /// Get mix suggestions based on current track and energy direction
    GetMixSuggestions {
        current_track_id: i64,
        energy_direction: EnergyDirection,
        limit: usize,
        reply: tokio::sync::oneshot::Sender<Result<Vec<MixSuggestion>, String>>,
    },

    /// Get all playlists
    GetPlaylists {
        reply: tokio::sync::oneshot::Sender<Result<Vec<Playlist>, String>>,
    },

    /// Get tracks in a playlist
    GetPlaylistTracks {
        playlist_id: i64,
        reply: tokio::sync::oneshot::Sender<Result<Vec<Track>, String>>,
    },

    /// Upsert a track (insert or update)
    UpsertTrack {
        track: Track,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Delete a track
    DeleteTrack {
        track_id: i64,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Update audio features for a track
    UpdateAudioFeatures {
        track_id: i64,
        features: AudioFeatures,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Shutdown the service
    Shutdown,
}

/// Energy direction for mix suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyDirection {
    /// Maintain current energy level
    Maintain,
    /// Build up energy (higher LUFS, faster BPM)
    BuildUp,
    /// Cool down energy (lower LUFS, slower BPM)
    CoolDown,
}

/// A mix suggestion with reasoning
#[derive(Debug, Clone)]
pub struct MixSuggestion {
    /// The suggested track
    pub track: Track,
    /// Why this track was suggested
    pub reason: MixReason,
    /// Overall compatibility score (0.0 - 1.0)
    pub score: f32,
}

/// Reason why a track was suggested as a mix candidate
#[derive(Debug, Clone)]
pub enum MixReason {
    /// Similar audio characteristics
    SimilarEnergy { similarity_score: f32 },
    /// Harmonically compatible (Camelot wheel)
    HarmonicMatch { match_type: String },
    /// Frequently played after current track
    FrequentTransition { play_count: u32 },
    /// BPM is within mixing range
    BpmCompatible { bpm_diff: f32 },
    /// Multiple reasons combined
    Combined { reasons: Vec<MixReason> },
}

// ============================================================================
// File Watch Commands
// ============================================================================

/// Commands sent to the FileWatchService
pub enum WatchCommand {
    /// Start watching a directory for changes
    Watch {
        path: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Stop watching a directory
    Unwatch {
        path: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Get list of currently watched paths
    GetWatchedPaths {
        reply: tokio::sync::oneshot::Sender<Vec<PathBuf>>,
    },

    /// Shutdown the service
    Shutdown,
}

// ============================================================================
// Migration Commands
// ============================================================================

/// Commands sent to trigger collection migrations
pub enum MigrationCommand {
    /// Migrate a collection from WAV files to database
    MigrateCollection {
        collection_root: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<MigrationResult, String>>,
    },

    /// Migrate a single track
    MigrateSingleTrack {
        path: PathBuf,
        collection_root: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<(), String>>,
    },

    /// Check if a track needs updating
    CheckTrackNeedsUpdate {
        path: PathBuf,
        reply: tokio::sync::oneshot::Sender<Result<bool, String>>,
    },
}

/// Result of a migration operation
#[derive(Debug, Clone)]
pub struct MigrationResult {
    pub tracks_migrated: usize,
    pub tracks_failed: usize,
    pub duration: Duration,
}

// ============================================================================
// Application Events (Broadcast)
// ============================================================================

/// Events broadcast to all subscribers
///
/// These events are sent from services to notify the UI and other
/// components about state changes. Uses crossbeam broadcast channels
/// for efficient fan-out.
#[derive(Debug, Clone)]
pub enum AppEvent {
    // --- Track Events ---
    /// A new track was added to the database
    TrackAdded(Track),

    /// A track was updated
    TrackUpdated {
        track_id: i64,
        track: Track,
    },

    /// A track was removed
    TrackRemoved(i64),

    /// Multiple tracks were added (batch operation)
    TracksAdded {
        count: usize,
        folder_path: Option<String>,
    },

    // --- Folder Events ---
    /// A folder scan completed
    FolderScanned {
        path: String,
        track_count: usize,
        duration: Duration,
    },

    // --- File System Events ---
    /// A file was created in a watched directory
    FileCreated(PathBuf),

    /// A file was modified in a watched directory
    FileModified(PathBuf),

    /// A file was deleted from a watched directory
    FileDeleted(PathBuf),

    /// A directory was created
    DirectoryCreated(PathBuf),

    /// A directory was deleted
    DirectoryDeleted(PathBuf),

    // --- Migration Events ---
    /// Migration progress update
    MigrationProgress {
        current: usize,
        total: usize,
        current_path: Option<PathBuf>,
    },

    /// Migration completed
    MigrationComplete {
        tracks_migrated: usize,
        tracks_failed: usize,
        duration: Duration,
    },

    // --- Analysis Events ---
    /// Audio analysis started for a track
    AnalysisStarted {
        track_id: i64,
        path: PathBuf,
    },

    /// Audio analysis progress
    AnalysisProgress {
        track_id: i64,
        phase: AnalysisPhase,
        progress: f32, // 0.0 - 1.0
    },

    /// Audio analysis completed
    AnalysisComplete {
        track_id: i64,
        features: AudioFeatures,
    },

    /// Audio analysis failed
    AnalysisFailed {
        track_id: i64,
        error: String,
    },

    // --- Service Events ---
    /// A service started
    ServiceStarted {
        service_name: String,
    },

    /// A service stopped
    ServiceStopped {
        service_name: String,
    },

    /// A service encountered an error
    ServiceError {
        service_name: String,
        error: String,
    },
}

/// Phases of audio analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisPhase {
    /// Loading audio file
    Loading,
    /// Detecting BPM
    BpmDetection,
    /// Detecting musical key
    KeyDetection,
    /// Measuring loudness (LUFS)
    LoudnessMeasurement,
    /// Extracting audio features for similarity
    FeatureExtraction,
    /// Generating waveform preview
    WaveformGeneration,
    /// Saving results
    Saving,
}

impl std::fmt::Display for AnalysisPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => write!(f, "Loading"),
            Self::BpmDetection => write!(f, "BPM Detection"),
            Self::KeyDetection => write!(f, "Key Detection"),
            Self::LoudnessMeasurement => write!(f, "Loudness Measurement"),
            Self::FeatureExtraction => write!(f, "Feature Extraction"),
            Self::WaveformGeneration => write!(f, "Waveform Generation"),
            Self::Saving => write!(f, "Saving"),
        }
    }
}

// ============================================================================
// Service Handle
// ============================================================================

/// Handle for communicating with a background service
///
/// This provides a typed interface for sending commands to a service
/// and subscribing to events.
pub struct ServiceHandle<Cmd> {
    /// Channel for sending commands to the service
    pub command_tx: crossbeam::channel::Sender<Cmd>,
    /// Thread handle for the service
    pub thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl<Cmd> ServiceHandle<Cmd> {
    /// Send a command to the service
    pub fn send(&self, cmd: Cmd) -> Result<(), crossbeam::channel::SendError<Cmd>> {
        self.command_tx.send(cmd)
    }

    /// Check if the service is still running
    pub fn is_running(&self) -> bool {
        self.thread_handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }
}

/// Event bus for broadcasting events to multiple subscribers
pub struct EventBus {
    sender: crossbeam::channel::Sender<AppEvent>,
    receiver: crossbeam::channel::Receiver<AppEvent>,
}

impl EventBus {
    /// Create a new event bus with bounded capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = crossbeam::channel::bounded(capacity);
        Self { sender, receiver }
    }

    /// Get a sender for publishing events
    pub fn sender(&self) -> crossbeam::channel::Sender<AppEvent> {
        self.sender.clone()
    }

    /// Get a receiver for subscribing to events
    pub fn subscribe(&self) -> crossbeam::channel::Receiver<AppEvent> {
        self.receiver.clone()
    }

    /// Publish an event to all subscribers
    pub fn publish(&self, event: AppEvent) -> Result<(), crossbeam::channel::SendError<AppEvent>> {
        self.sender.send(event)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_bus() {
        let bus = EventBus::new(16);
        let rx = bus.subscribe();

        bus.publish(AppEvent::ServiceStarted {
            service_name: "test".to_string(),
        }).unwrap();

        let event = rx.recv().unwrap();
        match event {
            AppEvent::ServiceStarted { service_name } => {
                assert_eq!(service_name, "test");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_analysis_phase_display() {
        assert_eq!(AnalysisPhase::BpmDetection.to_string(), "BPM Detection");
        assert_eq!(AnalysisPhase::FeatureExtraction.to_string(), "Feature Extraction");
    }
}
