//! Audio analysis module using Essentia
//!
//! Provides BPM detection, key detection, and beat grid generation
//! for imported stem files.

pub mod beatgrid;
pub mod bpm;
pub mod key;
pub mod loudness;

pub use beatgrid::generate_beat_grid;
pub use bpm::detect_bpm;
pub use key::detect_key;
pub use loudness::{
    calculate_gain_compensation, calculate_gain_compensation_clamped, db_to_linear, linear_to_db,
    measure_lufs,
};

// Note: Re-analysis types (AnalysisType, ReanalysisScope, etc.) and functions
// (analyze_partial, analyze_partial_in_subprocess) are defined below and
// are already public.

use crate::config::BpmConfig;
use mesh_core::playlist::NodeId;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ============================================================================
// Re-analysis types
// ============================================================================

/// Types of analysis that can be performed
///
/// Used for partial re-analysis to only run specific analysis algorithms
/// without re-processing the entire track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisType {
    /// LUFS loudness measurement (requires waveform regeneration)
    Loudness,
    /// BPM detection and beat grid generation
    Bpm,
    /// Musical key detection
    Key,
    /// All analysis types
    All,
}

impl AnalysisType {
    /// Returns human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Loudness => "Loudness",
            Self::Bpm => "BPM",
            Self::Key => "Key",
            Self::All => "All",
        }
    }

    /// Returns whether this analysis type requires waveform regeneration
    ///
    /// LUFS changes affect the waveform preview gain, so we need to
    /// re-export the entire file rather than just updating the bext chunk.
    pub fn requires_waveform_regeneration(&self) -> bool {
        matches!(self, Self::Loudness | Self::All)
    }
}

/// Scope of re-analysis operation
///
/// Determines which tracks should be re-analyzed.
#[derive(Debug, Clone)]
pub enum ReanalysisScope {
    /// Single track by its node ID
    SingleTrack(NodeId),
    /// Multiple selected tracks
    SelectedTracks(Vec<NodeId>),
    /// All tracks in a playlist folder
    PlaylistFolder(NodeId),
    /// Entire collection
    EntireCollection,
}

impl ReanalysisScope {
    /// Returns human-readable description of the scope
    pub fn description(&self) -> String {
        match self {
            Self::SingleTrack(_) => "1 track".to_string(),
            Self::SelectedTracks(ids) => format!("{} tracks", ids.len()),
            Self::PlaylistFolder(_) => "playlist".to_string(),
            Self::EntireCollection => "entire collection".to_string(),
        }
    }
}

/// Result of partial re-analysis
///
/// Only the fields corresponding to the requested analysis type will be populated.
/// This allows merging with existing metadata without overwriting unrelated fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartialAnalysisResult {
    /// Detected BPM (if BPM or All analysis was requested)
    pub bpm: Option<f64>,
    /// Beat grid as sample positions (if BPM or All analysis was requested)
    pub beat_grid: Option<Vec<u64>>,
    /// Musical key (if Key or All analysis was requested)
    pub key: Option<String>,
    /// Integrated LUFS loudness (if Loudness or All analysis was requested)
    pub lufs: Option<f32>,
}

/// Progress updates for batch re-analysis operations
///
/// Sent from the worker thread to the UI to update the progress modal.
#[derive(Debug, Clone)]
pub enum ReanalysisProgress {
    /// Batch operation started
    Started {
        total_tracks: usize,
        analysis_type: AnalysisType,
    },
    /// Starting analysis of a specific track
    TrackStarted {
        track_name: String,
        index: usize,
        total: usize,
    },
    /// Track analysis completed (success or failure)
    TrackCompleted {
        track_name: String,
        success: bool,
        error: Option<String>,
    },
    /// All tracks processed
    AllComplete {
        succeeded: usize,
        failed: usize,
        duration: Duration,
    },
}

/// Result of audio analysis
///
/// Serializable for subprocess communication (procspawn)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Detected BPM (beats per minute)
    pub bpm: f64,
    /// Original detected BPM before any rounding/adjustment
    pub original_bpm: f64,
    /// Musical key (e.g., "Am", "C", "F#m")
    pub key: String,
    /// Beat grid as sample positions at the system sample rate
    pub beat_grid: Vec<u64>,
    /// Analysis confidence (0.0 - 1.0)
    pub confidence: f32,
    /// Integrated LUFS loudness (EBU R128)
    /// Used for automatic gain compensation to target loudness
    pub lufs: Option<f32>,
}

impl Default for AnalysisResult {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            original_bpm: 120.0,
            key: String::from("C"),
            beat_grid: Vec::new(),
            confidence: 0.0,
            lufs: None,
        }
    }
}

/// Run full analysis on audio samples
///
/// # Arguments
/// * `samples` - Mono audio samples at the system sample rate (48kHz)
/// * `bpm_config` - BPM detection configuration (min/max tempo range)
///
/// # Returns
/// Complete analysis result with BPM, key, beat grid, and LUFS
pub fn analyze_audio(samples: &[f32], bpm_config: &BpmConfig) -> anyhow::Result<AnalysisResult> {
    use mesh_core::types::SAMPLE_RATE;
    log::info!(
        "analyze_audio: received {} samples ({:.1}s at {}Hz)",
        samples.len(),
        samples.len() as f64 / SAMPLE_RATE as f64,
        SAMPLE_RATE
    );

    // Detect BPM and beat positions using configured tempo range
    let (bpm, beat_ticks) = detect_bpm(samples, bpm_config)?;

    log::info!(
        "analyze_audio: Essentia returned {} beat ticks (first: {:.3}s, last: {:.3}s)",
        beat_ticks.len(),
        beat_ticks.first().unwrap_or(&0.0),
        beat_ticks.last().unwrap_or(&0.0)
    );

    // Detect musical key
    let key = detect_key(samples)?;

    // Measure integrated LUFS loudness (for automatic gain staging)
    let lufs = match measure_lufs(samples, SAMPLE_RATE as f32) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("LUFS measurement failed, skipping: {}", e);
            None
        }
    };

    // Generate fixed beat grid from detected beats, using actual track duration
    let beat_grid = generate_beat_grid(bpm, &beat_ticks, samples.len() as u64);

    log::info!(
        "analyze_audio: Generated {} beats in grid (first: {}, last: {})",
        beat_grid.len(),
        beat_grid.first().unwrap_or(&0),
        beat_grid.last().unwrap_or(&0)
    );

    Ok(AnalysisResult {
        bpm,
        original_bpm: bpm,
        key,
        beat_grid,
        confidence: 0.8, // TODO: Get from essentia
        lufs,
    })
}

// ============================================================================
// Partial Analysis (for re-analysis of specific metadata)
// ============================================================================

/// Run selective analysis on audio samples
///
/// Only performs the analysis algorithms corresponding to the requested type.
/// This is more efficient than full analysis when only updating specific metadata.
///
/// # Arguments
/// * `samples` - Mono audio samples at the system sample rate (48kHz)
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration (only used if type is Bpm or All)
///
/// # Returns
/// Partial result with only the requested fields populated
pub fn analyze_partial(
    samples: &[f32],
    analysis_type: AnalysisType,
    bpm_config: &BpmConfig,
) -> anyhow::Result<PartialAnalysisResult> {
    use mesh_core::types::SAMPLE_RATE;

    log::info!(
        "analyze_partial: {} analysis on {} samples ({:.1}s)",
        analysis_type.display_name(),
        samples.len(),
        samples.len() as f64 / SAMPLE_RATE as f64
    );

    let mut result = PartialAnalysisResult::default();

    match analysis_type {
        AnalysisType::Loudness => {
            result.lufs = Some(measure_lufs(samples, SAMPLE_RATE as f32)?);
        }
        AnalysisType::Bpm => {
            let (bpm, beat_ticks) = detect_bpm(samples, bpm_config)?;
            let beat_grid = generate_beat_grid(bpm, &beat_ticks, samples.len() as u64);
            result.bpm = Some(bpm);
            result.beat_grid = Some(beat_grid);
        }
        AnalysisType::Key => {
            result.key = Some(detect_key(samples)?);
        }
        AnalysisType::All => {
            // Run all analysis
            let (bpm, beat_ticks) = detect_bpm(samples, bpm_config)?;
            let beat_grid = generate_beat_grid(bpm, &beat_ticks, samples.len() as u64);
            result.bpm = Some(bpm);
            result.beat_grid = Some(beat_grid);
            result.key = Some(detect_key(samples)?);
            result.lufs = match measure_lufs(samples, SAMPLE_RATE as f32) {
                Ok(v) => Some(v),
                Err(e) => {
                    log::warn!("LUFS measurement failed: {}", e);
                    None
                }
            };
        }
    }

    log::info!("analyze_partial: Complete - {:?}", result);
    Ok(result)
}

/// Run selective analysis in an isolated subprocess
///
/// Essentia's C++ library has global state and is NOT thread-safe.
/// This spawns each analysis in a separate process for isolation.
///
/// # Arguments
/// * `samples` - Audio samples (ownership transferred to avoid copy)
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
///
/// # Returns
/// Partial result from subprocess
pub fn analyze_partial_in_subprocess(
    samples: Vec<f32>,
    analysis_type: AnalysisType,
    bpm_config: BpmConfig,
) -> anyhow::Result<PartialAnalysisResult> {
    use anyhow::Context;
    use std::io::{Read, Write};

    // Generate unique temp file path
    let temp_path = std::env::temp_dir().join(format!(
        "mesh_reanalyze_{}.bin",
        std::process::id()
            ^ (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u32)
    ));

    // Write samples to temp file (raw f32 bytes)
    {
        let mut file = std::fs::File::create(&temp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", temp_path))?;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                samples.as_ptr() as *const u8,
                samples.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes)
            .with_context(|| "Failed to write samples to temp file")?;
    }
    let sample_count = samples.len();
    drop(samples); // Free memory before spawning subprocess

    // Spawn subprocess with temp file path and analysis type
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let handle = procspawn::spawn(
        (temp_path_str.clone(), sample_count, analysis_type, bpm_config),
        |(path, count, atype, config)| {
            // Read samples from temp file in subprocess
            let samples = (|| -> std::result::Result<Vec<f32>, String> {
                let mut file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                let mut bytes = vec![0u8; count * std::mem::size_of::<f32>()];
                file.read_exact(&mut bytes).map_err(|e| e.to_string())?;

                // Convert bytes back to f32
                let samples: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                Ok(samples)
            })()?;

            // Run partial analysis in isolated process
            analyze_partial(&samples, atype, &config).map_err(|e| e.to_string())
        },
    );

    // Wait for result
    let result = handle
        .join()
        .map_err(|e| anyhow::anyhow!("Reanalysis subprocess failed: {:?}", e))?
        .map_err(|e| anyhow::anyhow!("Reanalysis error: {}", e));

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    result
}
