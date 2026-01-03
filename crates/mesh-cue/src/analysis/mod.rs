//! Audio analysis module using Essentia
//!
//! Provides BPM detection, key detection, and beat grid generation
//! for imported stem files.

pub mod beatgrid;
pub mod bpm;
pub mod key;

pub use beatgrid::generate_beat_grid;
pub use bpm::detect_bpm;
pub use key::detect_key;

use crate::config::BpmConfig;

/// Result of audio analysis
#[derive(Debug, Clone)]
pub struct AnalysisResult {
    /// Detected BPM (beats per minute)
    pub bpm: f64,
    /// Original detected BPM before any rounding/adjustment
    pub original_bpm: f64,
    /// Musical key (e.g., "Am", "C", "F#m")
    pub key: String,
    /// Beat grid as sample positions at 44.1kHz
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
/// * `samples` - Mono audio samples at 44.1kHz
/// * `bpm_config` - BPM detection configuration (min/max tempo range)
///
/// # Returns
/// Complete analysis result with BPM, key, and beat grid
pub fn analyze_audio(samples: &[f32], bpm_config: &BpmConfig) -> anyhow::Result<AnalysisResult> {
    // Detect BPM and beat positions using configured tempo range
    let (bpm, beat_ticks) = detect_bpm(samples, bpm_config)?;

    // Detect musical key
    let key = detect_key(samples)?;

    // Generate fixed beat grid from detected beats
    let beat_grid = generate_beat_grid(bpm, &beat_ticks);

    Ok(AnalysisResult {
        bpm,
        original_bpm: bpm,
        key,
        beat_grid,
        confidence: 0.8, // TODO: Get from essentia
    })
}
