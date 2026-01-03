//! BPM detection using Essentia's RhythmExtractor2013
//!
//! This module wraps Essentia's BPM detection algorithm to provide
//! accurate tempo analysis for dance music.

use anyhow::{Context, Result};
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

use crate::config::BpmConfig;

/// Detect BPM and beat positions from audio samples
///
/// Uses Essentia's RhythmExtractor2013 algorithm which is optimized
/// for electronic/dance music and provides both BPM and beat tick positions.
///
/// # Arguments
/// * `samples` - Mono audio samples at 44.1kHz
/// * `config` - BPM detection configuration (min/max tempo range)
///
/// # Returns
/// Tuple of (BPM value rounded to nearest integer, beat positions in seconds)
pub fn detect_bpm(samples: &[f32], config: &BpmConfig) -> Result<(f64, Vec<f64>)> {
    log::info!(
        "Starting BPM detection on {} samples (range: {}-{} BPM)",
        samples.len(),
        config.min_tempo,
        config.max_tempo
    );

    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure RhythmExtractor2013 with user-specified tempo range
    // - min_tempo: Essentia supports 40-180
    // - max_tempo: Essentia supports 60-250
    // - method: "multifeature" for best accuracy with confidence scores
    let mut rhythm = essentia
        .create::<RhythmExtractor2013>()
        .min_tempo(config.min_tempo)
        .context("Failed to set min_tempo")?
        .max_tempo(config.max_tempo)
        .context("Failed to set max_tempo")?
        .method("multifeature")
        .context("Failed to set method")?
        .configure()
        .context("Failed to configure RhythmExtractor2013")?;

    // Run the algorithm with input signal
    let result = rhythm
        .compute(samples)
        .context("RhythmExtractor2013 computation failed")?;

    // Extract outputs from result struct
    // The result struct provides accessor methods that return Result<DataContainer, OutputError>
    // We use the GetFromDataContainer trait's .get() method to extract the typed value
    let bpm: f32 = result
        .bpm()
        .context("Failed to get BPM output")?
        .get();

    let ticks: Vec<f32> = result
        .ticks()
        .context("Failed to get ticks output")?
        .get();

    // Round BPM to nearest integer for cleaner display
    let bpm_rounded = (bpm as f64).round();

    log::info!(
        "BPM detection complete: {:.1} -> {} BPM (rounded), {} beats detected",
        bpm,
        bpm_rounded as u32,
        ticks.len()
    );

    // Convert to f64 for downstream compatibility
    let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();

    Ok((bpm_rounded, beats))
}
