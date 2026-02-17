//! Audio analysis module using Essentia
//!
//! Provides BPM detection, key detection, and beat grid generation
//! for imported stem files.

pub mod beatgrid;
pub mod bpm;
pub mod key;
pub mod loudness;

pub use beatgrid::generate_beat_grid;
pub use bpm::{detect_bpm, detect_onset_function, fit_bpm_to_range, BpmResult, OnsetFunctionResult};
pub use key::detect_key;
pub use loudness::{
    calculate_gain_compensation, calculate_gain_compensation_clamped, db_to_linear, linear_to_db,
    measure_lufs,
};

// Note: Re-analysis types (AnalysisType, ReanalysisScope, etc.) and functions
// (analyze_partial, analyze_partial_in_subprocess) are defined below and
// are already public.

use crate::config::{BeatDetectionBackend, BpmConfig};
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
    /// ML similarity features: genre, mood, vocal presence, arousal/valence
    Similarity,
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
            Self::Similarity => "Similarity",
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

    /// Returns whether this analysis type uses the ML pipeline (ort/EffNet)
    /// rather than the Essentia subprocess pipeline.
    pub fn is_ml_analysis(&self) -> bool {
        matches!(self, Self::Similarity)
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
    /// 16-dimensional audio features for similarity search
    /// Includes rhythm, harmony, energy, and timbre features
    pub audio_features: Option<mesh_core::db::AudioFeatures>,
    /// Mel spectrogram for ML analysis (96 bands, computed in worker thread)
    /// Only populated when ML analysis is enabled
    #[serde(skip)]
    pub mel_spectrogram: Option<crate::ml_analysis::preprocessing::MelSpectrogramResult>,
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
            audio_features: None,
            mel_spectrogram: None,
        }
    }
}

/// Run full analysis on audio samples
///
/// # Arguments
/// * `samples` - Mono audio samples at 44100 Hz (Essentia's expected rate).
///   Callers must resample to 44100 Hz before calling this function.
/// * `bpm_config` - BPM detection configuration (min/max tempo range)
///
/// # Returns
/// Complete analysis result with BPM, key, beat grid, LUFS, and audio features
/// # Arguments
/// * `samples` - Full mix audio samples at 44100 Hz (for key/LUFS/features)
/// * `bpm_samples` - Optional separate audio for BPM analysis (drums-only when BpmSource::Drums).
///   When None, uses `samples` for BPM too.
/// * `bpm_config` - BPM detection configuration
pub fn analyze_audio(samples: &[f32], bpm_samples: Option<&[f32]>, bpm_config: &BpmConfig) -> anyhow::Result<AnalysisResult> {
    use crate::features::extract_audio_features;
    use mesh_core::types::SAMPLE_RATE;

    /// Essentia algorithms expect 44100 Hz input
    const ESSENTIA_RATE: f64 = 44100.0;

    // BPM uses dedicated audio (drums-only or full mix based on config)
    let bpm_audio = bpm_samples.unwrap_or(samples);

    log::info!(
        "analyze_audio: received {} samples ({:.1}s at 44100 Hz), bpm_source={}",
        samples.len(),
        samples.len() as f64 / ESSENTIA_RATE,
        if bpm_samples.is_some() { "separate" } else { "same" }
    );

    // BPM detection: skip if using Advanced backend (Beat This! runs outside subprocess)
    let (bpm_result_bpm, beat_ticks, bpm_confidence) =
        if bpm_config.backend == BeatDetectionBackend::Advanced {
            log::info!("analyze_audio: Skipping Essentia BPM (Advanced backend — Beat This! runs outside subprocess)");
            (120.0, vec![], 0.0_f32)
        } else {
            let bpm_result = detect_bpm(bpm_audio, bpm_config)?;
            log::info!(
                "analyze_audio: Essentia returned {} beat ticks (first: {:.3}s, last: {:.3}s), confidence={:.2}",
                bpm_result.beat_ticks.len(),
                bpm_result.beat_ticks.first().unwrap_or(&0.0),
                bpm_result.beat_ticks.last().unwrap_or(&0.0),
                bpm_result.confidence
            );
            (bpm_result.bpm, bpm_result.beat_ticks, bpm_result.confidence)
        };

    // Compute onset detection function for phase-optimal grid alignment
    // (Skip for Advanced backend — grid is built from Beat This! output instead)
    let onset_function = if bpm_config.backend == BeatDetectionBackend::Advanced {
        None
    } else {
        match detect_onset_function(bpm_audio) {
            Ok(odf) => {
                log::info!(
                    "analyze_audio: ODF computed ({} frames at {:.1} fps)",
                    odf.values.len(),
                    odf.frame_rate
                );
                Some(odf)
            }
            Err(e) => {
                log::warn!("Onset detection failed, grid will use circular median only: {}", e);
                None
            }
        }
    };

    // Key and LUFS always use full mix
    let key = detect_key(samples)?;

    // Measure integrated LUFS loudness (for automatic gain staging)
    // Samples are at 44100 Hz — pass correct rate for K-weighting filter
    let lufs = match measure_lufs(samples, ESSENTIA_RATE as f32) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("LUFS measurement failed, skipping: {}", e);
            None
        }
    };

    // Extract 16-dimensional audio features for similarity search (full mix)
    let audio_features = match extract_audio_features(samples) {
        Ok(features) => {
            log::info!("analyze_audio: Audio features extracted successfully");
            Some(features)
        }
        Err(e) => {
            log::warn!("Audio feature extraction failed, skipping: {}", e);
            None
        }
    };

    // Generate fixed beat grid from detected beats, using actual track duration
    // BPM audio + ODF are used for onset-weighted phase anchor computation
    // Grid positions are at SAMPLE_RATE (48kHz), so duration must also be in 48kHz
    // Input samples are at Essentia's rate (44100 Hz) — scale up for consistent units
    let duration_at_system_rate =
        (samples.len() as f64 * SAMPLE_RATE as f64 / ESSENTIA_RATE) as u64;
    let beat_grid = generate_beat_grid(
        bpm_result_bpm,
        &beat_ticks,
        bpm_audio,
        duration_at_system_rate,
        onset_function.as_ref(),
    );

    log::info!(
        "analyze_audio: Generated {} beats in grid (first: {}, last: {})",
        beat_grid.len(),
        beat_grid.first().unwrap_or(&0),
        beat_grid.last().unwrap_or(&0)
    );

    Ok(AnalysisResult {
        bpm: bpm_result_bpm,
        original_bpm: bpm_result_bpm,
        key,
        beat_grid,
        confidence: bpm_confidence,
        lufs,
        audio_features,
        mel_spectrogram: None, // Computed in worker thread, not subprocess
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
/// * `samples` - Full mix audio samples for key/LUFS analysis (44100 Hz)
/// * `bpm_samples` - Optional separate audio for BPM analysis (drums-only when BpmSource::Drums).
///   When None, uses `samples` for BPM too.
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
pub fn analyze_partial(
    samples: &[f32],
    bpm_samples: Option<&[f32]>,
    analysis_type: AnalysisType,
    bpm_config: &BpmConfig,
) -> anyhow::Result<PartialAnalysisResult> {
    use mesh_core::types::SAMPLE_RATE;

    /// Essentia algorithms expect 44100 Hz input
    const ESSENTIA_RATE: f64 = 44100.0;

    // BPM uses dedicated audio (drums-only or full mix based on config)
    let bpm_audio = bpm_samples.unwrap_or(samples);

    log::info!(
        "analyze_partial: {} analysis on {} samples ({:.1}s at 44100 Hz), bpm_source={}",
        analysis_type.display_name(),
        samples.len(),
        samples.len() as f64 / ESSENTIA_RATE,
        if bpm_samples.is_some() { "separate" } else { "same" }
    );

    let mut result = PartialAnalysisResult::default();

    match analysis_type {
        AnalysisType::Loudness => {
            result.lufs = Some(measure_lufs(samples, ESSENTIA_RATE as f32)?);
        }
        AnalysisType::Bpm => {
            let bpm_result = detect_bpm(bpm_audio, bpm_config)?;
            let onset_function = detect_onset_function(bpm_audio).ok();
            let duration_at_system_rate =
                (samples.len() as f64 * SAMPLE_RATE as f64 / ESSENTIA_RATE) as u64;
            let beat_grid = generate_beat_grid(
                bpm_result.bpm,
                &bpm_result.beat_ticks,
                bpm_audio,
                duration_at_system_rate,
                onset_function.as_ref(),
            );
            result.bpm = Some(bpm_result.bpm);
            result.beat_grid = Some(beat_grid);
        }
        AnalysisType::Key => {
            result.key = Some(detect_key(samples)?);
        }
        AnalysisType::Similarity => {
            // ML analysis uses a separate pipeline (ort/EffNet), not this subprocess.
            // This path should not be reached — handled by run_batch_ml_reanalysis().
            log::warn!("Similarity analysis requested in subprocess — use ML pipeline instead");
        }
        AnalysisType::All => {
            // BPM/beat grid uses bpm_audio (drums-only or full mix based on config)
            let bpm_result = detect_bpm(bpm_audio, bpm_config)?;
            let onset_function = detect_onset_function(bpm_audio).ok();
            let duration_at_system_rate =
                (samples.len() as f64 * SAMPLE_RATE as f64 / ESSENTIA_RATE) as u64;
            let beat_grid = generate_beat_grid(
                bpm_result.bpm,
                &bpm_result.beat_ticks,
                bpm_audio,
                duration_at_system_rate,
                onset_function.as_ref(),
            );
            result.bpm = Some(bpm_result.bpm);
            result.beat_grid = Some(beat_grid);
            // Key and LUFS always use full mix
            result.key = Some(detect_key(samples)?);
            result.lufs = match measure_lufs(samples, ESSENTIA_RATE as f32) {
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
/// # Arguments
/// * `samples` - Full mix audio samples (for key/LUFS)
/// * `bpm_samples` - Optional separate audio for BPM analysis (drums-only when BpmSource::Drums)
/// * `analysis_type` - Which analysis to perform
/// * `bpm_config` - BPM detection configuration
pub fn analyze_partial_in_subprocess(
    samples: Vec<f32>,
    bpm_samples: Option<Vec<f32>>,
    analysis_type: AnalysisType,
    bpm_config: BpmConfig,
) -> anyhow::Result<PartialAnalysisResult> {
    use anyhow::Context;
    use std::io::{Read, Write};

    // Generate unique temp file paths
    let uid = std::process::id()
        ^ (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u32);
    let temp_path = std::env::temp_dir().join(format!("mesh_reanalyze_{}.bin", uid));
    let bpm_temp_path = std::env::temp_dir().join(format!("mesh_reanalyze_{}_bpm.bin", uid));

    // Helper to write f32 samples to a temp file
    let write_samples = |path: &std::path::Path, data: &[f32]| -> anyhow::Result<()> {
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create temp file: {:?}", path))?;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                data.as_ptr() as *const u8,
                data.len() * std::mem::size_of::<f32>(),
            )
        };
        file.write_all(bytes)
            .with_context(|| "Failed to write samples to temp file")?;
        Ok(())
    };

    // Write main samples
    write_samples(&temp_path, &samples)?;
    let sample_count = samples.len();

    // Write BPM samples if separate
    let bpm_sample_count = bpm_samples.as_ref().map(|s| s.len());
    if let Some(ref bpm) = bpm_samples {
        write_samples(&bpm_temp_path, bpm)?;
    }

    drop(samples); // Free memory before spawning subprocess
    drop(bpm_samples);

    // Spawn subprocess with temp file path and analysis type
    let temp_path_str = temp_path.to_string_lossy().to_string();
    let bpm_temp_path_str = bpm_temp_path.to_string_lossy().to_string();
    let handle = procspawn::spawn(
        (temp_path_str.clone(), sample_count, bpm_temp_path_str.clone(), bpm_sample_count, analysis_type, bpm_config),
        |(path, count, bpm_path, bpm_count, atype, config)| {
            // Helper to read f32 samples from a temp file
            let read_samples = |path: &str, count: usize| -> std::result::Result<Vec<f32>, String> {
                let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mut bytes = vec![0u8; count * std::mem::size_of::<f32>()];
                file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                Ok(bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect())
            };

            let samples = read_samples(&path, count)?;
            let bpm_samples = match bpm_count {
                Some(c) => Some(read_samples(&bpm_path, c)?),
                None => None,
            };

            // Run partial analysis in isolated process
            analyze_partial(&samples, bpm_samples.as_deref(), atype, &config).map_err(|e| e.to_string())
        },
    );

    // Wait for result
    let result = handle
        .join()
        .map_err(|e| anyhow::anyhow!("Reanalysis subprocess failed: {:?}", e))?
        .map_err(|e| anyhow::anyhow!("Reanalysis error: {}", e));

    // Clean up temp files
    let _ = std::fs::remove_file(&temp_path);
    let _ = std::fs::remove_file(&bpm_temp_path);

    result
}
