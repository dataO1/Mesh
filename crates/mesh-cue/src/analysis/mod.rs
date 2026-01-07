//! Audio analysis module using Essentia
//!
//! Provides BPM detection, key detection, and beat grid generation
//! for imported stem files.
//!
//! ## BPM Detection
//!
//! Multiple algorithms are available via [`BpmAlgorithm`]:
//! - Essentia-based: multifeature, degara, beat trackers
//! - Python-based: Madmom DBN, BeatFM (require external setup)
//!
//! Use [`create_detector`] to get a detector for a specific algorithm.

pub mod algorithm;
pub mod beatgrid;
pub mod bpm;
pub mod key;
pub mod python;

// Re-exports for convenient access
pub use algorithm::BpmAlgorithm;
pub use beatgrid::generate_beat_grid;
pub use bpm::{create_detector, detect_bpm, BpmDetector, BpmResult};
pub use key::detect_key;
pub use python::python_algorithms_available;

use crate::config::BpmConfig;
use serde::{Deserialize, Serialize};

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
}

impl Default for AnalysisResult {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            original_bpm: 120.0,
            key: String::from("C"),
            beat_grid: Vec::new(),
            confidence: 0.0,
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
/// Complete analysis result with BPM, key, and beat grid
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
    })
}
