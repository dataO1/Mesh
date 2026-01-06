//! Musical key detection using Essentia
//!
//! This module detects the musical key of a track using Essentia's
//! KeyExtractor algorithm, which internally handles HPCP computation
//! and key correlation in one convenient package.

use anyhow::{Context, Result};
use essentia::algorithm::tonal::key_extractor::KeyExtractor;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

/// Detect the musical key from audio samples
///
/// Uses Essentia's KeyExtractor algorithm which combines HPCP (Harmonic
/// Pitch Class Profile) computation with key correlation in a single step.
/// The "edma" profile type is used, which is optimized for Electronic Dance Music.
///
/// # Arguments
/// * `samples` - Mono audio samples at the system sample rate (48kHz)
///
/// # Returns
/// Key string in format like "Am", "C", "F#m", "Bb"
pub fn detect_key(samples: &[f32]) -> Result<String> {
    use mesh_core::types::SAMPLE_RATE;
    log::info!("Starting key detection on {} samples", samples.len());

    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure KeyExtractor
    // - profile_type: "edma" (Electronic Dance Music Average) for EDM tracks
    // - sample_rate: System sample rate (48kHz default)
    let mut key_algo = essentia
        .create::<KeyExtractor>()
        .profile_type("edma")
        .context("Failed to set profile_type")?
        .sample_rate(SAMPLE_RATE as f32)
        .context("Failed to set sample_rate")?
        .configure()
        .context("Failed to configure KeyExtractor")?;

    // Run the algorithm with input signal
    let result = key_algo
        .compute(samples)
        .context("KeyExtractor computation failed")?;

    // Extract outputs from result struct
    // key() returns the root note (e.g., "A", "C#", "Bb")
    // scale() returns "major" or "minor"
    let key: String = result
        .key()
        .context("Failed to get key output")?
        .get();

    let scale: String = result
        .scale()
        .context("Failed to get scale output")?
        .get();

    let strength: f32 = result
        .strength()
        .context("Failed to get strength output")?
        .get();

    log::info!(
        "Key detection complete: {} {} (strength: {:.2})",
        key,
        scale,
        strength
    );

    // Format as standard key notation: "Am", "C", "F#m", "Bb", etc.
    let suffix = if scale == "minor" { "m" } else { "" };
    Ok(format!("{}{}", key, suffix))
}
