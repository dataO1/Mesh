//! Feature Extraction Service
//!
//! Background service for extracting audio features from tracks.
//! Uses subprocess isolation for Essentia algorithms (which are not thread-safe).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     Commands      ┌─────────────────────┐
//! │   UI/Import │ ───────────────►  │FeatureExtractionSvc │
//! │             │ ◄─────────────── │   (background)      │
//! └─────────────┘     Events        └─────────────────────┘
//!                                            │
//!                                   ┌────────┴────────┐
//!                                   ▼                 ▼
//!                          ┌──────────────┐  ┌──────────────┐
//!                          │  Subprocess  │  │    CozoDB    │
//!                          │  (Essentia)  │  │   (store)    │
//!                          └──────────────┘  └──────────────┘
//! ```

use crate::db::{AudioFeatures, MeshDb, SimilarityQuery};
use crate::features::extract_audio_features_in_subprocess;
use crossbeam::channel::{self, Receiver, Sender};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

use super::messages::{AnalysisPhase, AppEvent, ServiceHandle};

// ============================================================================
// Commands
// ============================================================================

/// Commands for the FeatureExtractionService
pub enum FeatureCommand {
    /// Extract features for a single track
    ExtractSingle {
        track_id: i64,
        samples: Vec<f32>,
        reply: Option<tokio::sync::oneshot::Sender<Result<AudioFeatures, String>>>,
    },

    /// Extract features for multiple tracks (batch)
    ExtractBatch {
        /// List of (track_id, samples) pairs
        tracks: Vec<(i64, Vec<f32>)>,
        reply: Option<tokio::sync::oneshot::Sender<Result<usize, String>>>,
    },

    /// Extract features from a file path (reads samples internally)
    ExtractFromFile {
        track_id: i64,
        path: PathBuf,
        reply: Option<tokio::sync::oneshot::Sender<Result<AudioFeatures, String>>>,
    },

    /// Shutdown the service
    Shutdown,
}

// ============================================================================
// Service Configuration
// ============================================================================

/// Configuration for the FeatureExtractionService
#[derive(Clone)]
pub struct FeatureExtractionConfig {
    /// Database instance for storing features
    pub db: Arc<MeshDb>,
    /// Maximum concurrent subprocess extractions
    pub max_concurrent: usize,
}

impl FeatureExtractionConfig {
    pub fn new(db: Arc<MeshDb>) -> Self {
        Self {
            db,
            max_concurrent: 1, // Essentia subprocess isolation means we can't parallelize anyway
        }
    }
}

// ============================================================================
// Service
// ============================================================================

/// Background service for audio feature extraction
pub struct FeatureExtractionService;

impl FeatureExtractionService {
    /// Spawn the service in a background thread
    ///
    /// # Arguments
    /// * `config` - Service configuration
    /// * `event_tx` - Channel for publishing events
    ///
    /// # Returns
    /// ServiceHandle for sending commands to the service
    pub fn spawn(
        config: FeatureExtractionConfig,
        event_tx: Sender<AppEvent>,
    ) -> ServiceHandle<FeatureCommand> {
        let (command_tx, command_rx) = channel::unbounded();

        let thread_handle = thread::Builder::new()
            .name("feature-extraction".to_string())
            .spawn(move || {
                // Publish service started event
                let _ = event_tx.send(AppEvent::ServiceStarted {
                    service_name: "FeatureExtractionService".to_string(),
                });

                Self::run(config, command_rx, event_tx.clone());

                // Publish service stopped event
                let _ = event_tx.send(AppEvent::ServiceStopped {
                    service_name: "FeatureExtractionService".to_string(),
                });
            })
            .expect("Failed to spawn feature extraction service");

        ServiceHandle {
            command_tx,
            thread_handle: Some(thread_handle),
        }
    }

    /// Main service loop
    fn run(
        config: FeatureExtractionConfig,
        command_rx: Receiver<FeatureCommand>,
        event_tx: Sender<AppEvent>,
    ) {
        log::info!("FeatureExtractionService started");

        while let Ok(cmd) = command_rx.recv() {
            match cmd {
                FeatureCommand::ExtractSingle { track_id, samples, reply } => {
                    let result = Self::extract_single(
                        &config.db,
                        track_id,
                        samples,
                        &event_tx,
                    );
                    if let Some(tx) = reply {
                        let _ = tx.send(result);
                    }
                }

                FeatureCommand::ExtractBatch { tracks, reply } => {
                    let result = Self::extract_batch(
                        &config.db,
                        tracks,
                        &event_tx,
                    );
                    if let Some(tx) = reply {
                        let _ = tx.send(result);
                    }
                }

                FeatureCommand::ExtractFromFile { track_id, path, reply } => {
                    let result = Self::extract_from_file(
                        &config.db,
                        track_id,
                        &path,
                        &event_tx,
                    );
                    if let Some(tx) = reply {
                        let _ = tx.send(result);
                    }
                }

                FeatureCommand::Shutdown => {
                    log::info!("FeatureExtractionService shutting down");
                    break;
                }
            }
        }
    }

    /// Extract features for a single track
    fn extract_single(
        db: &MeshDb,
        track_id: i64,
        samples: Vec<f32>,
        event_tx: &Sender<AppEvent>,
    ) -> Result<AudioFeatures, String> {
        let start = Instant::now();

        // Publish analysis started
        let _ = event_tx.send(AppEvent::AnalysisProgress {
            track_id,
            phase: AnalysisPhase::FeatureExtraction,
            progress: 0.0,
        });

        // Extract features in subprocess
        let features = extract_audio_features_in_subprocess(samples)
            .map_err(|e| {
                let _ = event_tx.send(AppEvent::AnalysisFailed {
                    track_id,
                    error: e.to_string(),
                });
                e.to_string()
            })?;

        // Progress update
        let _ = event_tx.send(AppEvent::AnalysisProgress {
            track_id,
            phase: AnalysisPhase::Saving,
            progress: 0.9,
        });

        // Store in database
        SimilarityQuery::upsert_features(db, track_id, &features)
            .map_err(|e| {
                let _ = event_tx.send(AppEvent::AnalysisFailed {
                    track_id,
                    error: e.to_string(),
                });
                e.to_string()
            })?;

        let duration = start.elapsed();
        log::info!(
            "Feature extraction for track {} completed in {:?}",
            track_id,
            duration
        );

        // Publish completion
        let _ = event_tx.send(AppEvent::AnalysisComplete {
            track_id,
            features: features.clone(),
        });

        Ok(features)
    }

    /// Extract features for multiple tracks
    fn extract_batch(
        db: &MeshDb,
        tracks: Vec<(i64, Vec<f32>)>,
        event_tx: &Sender<AppEvent>,
    ) -> Result<usize, String> {
        let total = tracks.len();
        let mut succeeded = 0;

        for (i, (track_id, samples)) in tracks.into_iter().enumerate() {
            log::info!("Batch extraction: track {} ({}/{})", track_id, i + 1, total);

            match Self::extract_single(db, track_id, samples, event_tx) {
                Ok(_) => succeeded += 1,
                Err(e) => log::warn!("Failed to extract features for track {}: {}", track_id, e),
            }
        }

        log::info!("Batch extraction complete: {}/{} succeeded", succeeded, total);
        Ok(succeeded)
    }

    /// Extract features from a file
    fn extract_from_file(
        db: &MeshDb,
        track_id: i64,
        path: &PathBuf,
        event_tx: &Sender<AppEvent>,
    ) -> Result<AudioFeatures, String> {
        // Publish loading phase
        let _ = event_tx.send(AppEvent::AnalysisStarted {
            track_id,
            path: path.clone(),
        });

        let _ = event_tx.send(AppEvent::AnalysisProgress {
            track_id,
            phase: AnalysisPhase::Loading,
            progress: 0.0,
        });

        // Read audio file and get mono samples
        let samples = Self::load_mono_samples(path)?;

        // Extract features
        Self::extract_single(db, track_id, samples, event_tx)
    }

    /// Load mono samples from an audio file
    ///
    /// Reads all stems and creates a mono mix for feature extraction.
    fn load_mono_samples(path: &PathBuf) -> Result<Vec<f32>, String> {
        use crate::audio_file::AudioFileReader;
        use crate::Stem;

        let mut reader = AudioFileReader::open(path)
            .map_err(|e| format!("Failed to open audio file: {}", e))?;

        // Read all stems
        let stems = reader.read_all_stems()
            .map_err(|e| format!("Failed to read audio: {}", e))?;

        // Create mono mix by summing all stems and averaging channels
        let num_samples = stems.len();
        let mut mono = vec![0f32; num_samples];

        // Sum all 4 stems (vocals, drums, bass, other)
        for stem in [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other] {
            let buffer = stems.get(stem);
            for (i, stereo) in buffer.iter().enumerate() {
                if i < mono.len() {
                    // Average left and right channels
                    mono[i] += (stereo.left + stereo.right) * 0.5;
                }
            }
        }

        // Normalize by number of stems
        for sample in mono.iter_mut() {
            *sample *= 0.25;
        }

        Ok(mono)
    }
}

// ============================================================================
// Client
// ============================================================================

/// Client for interacting with the FeatureExtractionService
pub struct FeatureExtractionClient {
    command_tx: Sender<FeatureCommand>,
}

impl FeatureExtractionClient {
    /// Create a new client from a service handle
    pub fn new(handle: &ServiceHandle<FeatureCommand>) -> Self {
        Self {
            command_tx: handle.command_tx.clone(),
        }
    }

    /// Extract features for a single track (async with reply)
    pub fn extract(&self, track_id: i64, samples: Vec<f32>) -> Result<AudioFeatures, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        self.command_tx
            .send(FeatureCommand::ExtractSingle {
                track_id,
                samples,
                reply: Some(reply_tx),
            })
            .map_err(|e| format!("Failed to send command: {}", e))?;

        reply_rx
            .blocking_recv()
            .map_err(|e| format!("Failed to receive reply: {}", e))?
    }

    /// Extract features for a single track (fire-and-forget)
    pub fn extract_async(&self, track_id: i64, samples: Vec<f32>) -> Result<(), String> {
        self.command_tx
            .send(FeatureCommand::ExtractSingle {
                track_id,
                samples,
                reply: None,
            })
            .map_err(|e| format!("Failed to send command: {}", e))
    }

    /// Extract features from a file path
    pub fn extract_from_file(&self, track_id: i64, path: PathBuf) -> Result<AudioFeatures, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        self.command_tx
            .send(FeatureCommand::ExtractFromFile {
                track_id,
                path,
                reply: Some(reply_tx),
            })
            .map_err(|e| format!("Failed to send command: {}", e))?;

        reply_rx
            .blocking_recv()
            .map_err(|e| format!("Failed to receive reply: {}", e))?
    }

    /// Extract features for multiple tracks
    pub fn extract_batch(&self, tracks: Vec<(i64, Vec<f32>)>) -> Result<usize, String> {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

        self.command_tx
            .send(FeatureCommand::ExtractBatch {
                tracks,
                reply: Some(reply_tx),
            })
            .map_err(|e| format!("Failed to send command: {}", e))?;

        reply_rx
            .blocking_recv()
            .map_err(|e| format!("Failed to receive reply: {}", e))?
    }

    /// Shutdown the service
    pub fn shutdown(&self) -> Result<(), String> {
        self.command_tx
            .send(FeatureCommand::Shutdown)
            .map_err(|e| format!("Failed to send shutdown: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_spawn_and_shutdown() {
        let db = MeshDb::in_memory().unwrap();
        let event_bus = super::super::EventBus::default();

        let config = FeatureExtractionConfig::new(Arc::new(db));
        let handle = FeatureExtractionService::spawn(config, event_bus.sender());

        assert!(handle.is_running());

        let client = FeatureExtractionClient::new(&handle);
        client.shutdown().unwrap();

        // Wait for thread to finish
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
