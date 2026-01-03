//! BPM detection using Essentia's RhythmExtractor2013
//!
//! This module wraps Essentia's BPM detection algorithm to provide
//! accurate tempo analysis for dance music.

use anyhow::{Context, Result};
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

/// Detect BPM and beat positions from audio samples
///
/// Uses Essentia's RhythmExtractor2013 algorithm which is optimized
/// for electronic/dance music and provides both BPM and beat tick positions.
///
/// # Arguments
/// * `samples` - Mono audio samples at 44.1kHz
///
/// # Returns
/// Tuple of (BPM value, beat positions in seconds)
pub fn detect_bpm(samples: &[f32]) -> Result<(f64, Vec<f64>)> {
    log::info!("Starting BPM detection on {} samples", samples.len());

    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure RhythmExtractor2013
    // - min_tempo: 40 BPM (default)
    // - max_tempo: 208 BPM (default)
    // - method: "multifeature" for best accuracy with confidence scores
    let mut rhythm = essentia
        .create::<RhythmExtractor2013>()
        .min_tempo(40)
        .context("Failed to set min_tempo")?
        .max_tempo(208)
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

    log::info!(
        "BPM detection complete: {:.2} BPM, {} beats detected",
        bpm,
        ticks.len()
    );

    // Convert to f64 for downstream compatibility
    let bpm = bpm as f64;
    let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();

    Ok((bpm, beats))
}
