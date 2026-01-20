//! Export progress messages
//!
//! These messages are sent from worker threads to the UI via mpsc channel.
//! Each message represents a step in the export lifecycle:
//!
//! Started → TrackStarted → TrackComplete/TrackFailed → ... → Complete/Cancelled

use std::time::Duration;

/// Progress messages for USB export
///
/// These messages represent the complete track export lifecycle, including
/// both WAV copying and database sync. Progress is reported after each
/// track is fully exported (WAV + DB), not just after WAV copy.
#[derive(Debug, Clone)]
pub enum ExportProgress {
    /// Export started
    Started {
        /// Total number of tracks to export
        total_tracks: usize,
        /// Total bytes to copy (sum of all track file sizes)
        total_bytes: u64,
    },

    /// A track export started (WAV copy beginning)
    TrackStarted {
        /// Filename of the track being exported
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
    },

    /// A track was fully exported (WAV copied + DB synced)
    ///
    /// This is the atomic completion signal - only sent after both
    /// the WAV file and all database metadata are written.
    TrackComplete {
        /// Filename that was exported
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
        /// Total tracks in the export
        total_tracks: usize,
        /// Cumulative bytes exported so far
        bytes_complete: u64,
        /// Total bytes to export
        total_bytes: u64,
    },

    /// A track failed to export
    TrackFailed {
        /// Filename that failed
        filename: String,
        /// Index in the export queue (0-based)
        track_index: usize,
        /// Error description
        error: String,
    },

    /// All tracks exported (or failed)
    Complete {
        /// Total export duration
        duration: Duration,
        /// Number of tracks successfully exported
        tracks_exported: usize,
        /// Files that failed with their error messages
        failed_files: Vec<(String, String)>,
    },

    /// Export was cancelled by user
    Cancelled,

    /// Playlist operations phase started (after all tracks are copied)
    ///
    /// This phase adds/removes tracks from playlists in the USB database.
    PlaylistOpsStarted {
        /// Total number of playlist membership operations
        total_operations: usize,
    },

    /// A playlist operation completed
    PlaylistOpComplete {
        /// Number of operations completed so far
        completed: usize,
        /// Total number of operations
        total: usize,
    },
}

impl ExportProgress {
    /// Get a human-readable description of this progress message
    pub fn description(&self) -> String {
        match self {
            Self::Started { total_tracks, .. } => {
                format!("Starting export of {} tracks", total_tracks)
            }
            Self::TrackStarted { filename, .. } => {
                format!("Exporting: {}", filename)
            }
            Self::TrackComplete {
                track_index,
                total_tracks,
                ..
            } => {
                format!("Exported {}/{}", track_index + 1, total_tracks)
            }
            Self::TrackFailed { filename, error, .. } => {
                format!("Failed: {} - {}", filename, error)
            }
            Self::Complete {
                duration,
                tracks_exported,
                failed_files,
            } => {
                if failed_files.is_empty() {
                    format!(
                        "Export complete: {} tracks in {:.1}s",
                        tracks_exported,
                        duration.as_secs_f64()
                    )
                } else {
                    format!(
                        "Export complete: {} tracks, {} failed in {:.1}s",
                        tracks_exported,
                        failed_files.len(),
                        duration.as_secs_f64()
                    )
                }
            }
            Self::Cancelled => "Export cancelled".to_string(),
            Self::PlaylistOpsStarted { total_operations } => {
                format!("Updating {} playlist entries...", total_operations)
            }
            Self::PlaylistOpComplete { completed, total } => {
                format!("Playlist entries: {}/{}", completed, total)
            }
        }
    }

    /// Check if this is a terminal message (Complete or Cancelled)
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete { .. } | Self::Cancelled)
    }

    /// Get the progress percentage (0.0 to 1.0)
    pub fn progress_fraction(&self) -> Option<f32> {
        match self {
            Self::TrackComplete {
                track_index,
                total_tracks,
                ..
            } => Some((*track_index + 1) as f32 / *total_tracks as f32),
            Self::PlaylistOpComplete { completed, total } => {
                Some(*completed as f32 / *total as f32)
            }
            Self::Complete { .. } => Some(1.0),
            Self::Started { .. } | Self::PlaylistOpsStarted { .. } => Some(0.0),
            _ => None,
        }
    }
}
