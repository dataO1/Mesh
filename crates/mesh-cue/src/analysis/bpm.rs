//! BPM detection using Essentia's RhythmExtractor2013
//!
//! This module wraps Essentia's BPM detection algorithm to provide
//! accurate tempo analysis for dance music.
//!
//! ## Implementation Note
//!
//! Essentia's RhythmExtractor2013 requires 44.1kHz input and returns accurate
//! BPM values when run without min/max tempo constraints.
//!
//! Our solution:
//! 1. Run Essentia without tempo constraints (use defaults 40-208)
//! 2. Use Essentia's direct BPM output (accurate without constraints)
//! 3. Apply octave/triplet fitting to match user's desired tempo range
//! 4. Extract real confidence from the multifeature tracker [0, 5.32] → [0, 1]

use anyhow::{Context, Result};
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

use crate::config::BpmConfig;

/// Maximum confidence value from Essentia's TempoTapMaxAgreement
const MAX_ESSENTIA_CONFIDENCE: f32 = 5.32;

/// Result of BPM detection
#[derive(Debug, Clone)]
pub struct BpmResult {
    /// BPM fitted to the configured tempo range
    pub bpm: f64,
    /// Beat tick positions in seconds (from Essentia)
    pub beat_ticks: Vec<f64>,
    /// Normalized confidence [0.0, 1.0] from Essentia's multifeature tracker
    pub confidence: f32,
}

/// Fit BPM into target range using musical multipliers
///
/// Tries multiple multipliers in order of likelihood:
/// 1. Octave shifts (×2, ÷2) - most common beat detection error
/// 2. Triplet shifts (×1.5, ÷1.5) - happens with syncopated rhythms
///
/// # Examples
/// - 86 BPM with range 150-180 → 86 × 2 = 172 BPM
/// - 117 BPM with range 150-180 → 117 × 1.5 = 175.5 → 176 BPM
pub fn fit_bpm_to_range(detected_bpm: f64, min_tempo: i32, max_tempo: i32) -> f64 {
    let min = min_tempo as f64;
    let max = max_tempo as f64;

    // If already in range, just round and return
    if detected_bpm >= min && detected_bpm <= max {
        return detected_bpm.round();
    }

    // Try multipliers in order of musical likelihood
    // Octave (×2, ÷2) is most common, then triplet (×1.5, ÷1.5)
    let multipliers: [f64; 6] = [2.0, 0.5, 1.5, 2.0 / 3.0, 3.0, 1.0 / 3.0];

    for &mult in &multipliers {
        let candidate = detected_bpm * mult;
        if candidate >= min && candidate <= max {
            return candidate.round();
        }
    }

    // No multiplier fits - return original rounded (edge case: very narrow range)
    detected_bpm.round()
}

/// Detect BPM, beat positions, and confidence from audio samples
///
/// Uses Essentia's RhythmExtractor2013 algorithm which is optimized
/// for electronic/dance music and provides BPM, beat tick positions,
/// and a confidence score from the multifeature beat tracker.
///
/// # Arguments
/// * `samples` - Mono audio samples at 44.1kHz
/// * `config` - BPM detection configuration (min/max tempo range for fitting)
///
/// # Returns
/// `BpmResult` with fitted BPM, beat ticks in seconds, and normalized confidence
pub fn detect_bpm(samples: &[f32], config: &BpmConfig) -> Result<BpmResult> {
    log::info!(
        "Starting BPM detection on {} samples (will fit to {}-{} BPM)",
        samples.len(),
        config.min_tempo,
        config.max_tempo
    );

    // Create Essentia instance
    let essentia = Essentia::new();

    // Create and configure RhythmExtractor2013 WITHOUT tempo constraints
    // Essentia's min/max tempo causes buggy constant BPM output.
    // We use defaults (40-208) and apply our own post-processing.
    let mut rhythm = essentia
        .create::<RhythmExtractor2013>()
        .method("multifeature")
        .context("Failed to set method")?
        .configure()
        .context("Failed to configure RhythmExtractor2013")?;

    // Run the algorithm with input signal
    let result = rhythm
        .compute(samples)
        .context("RhythmExtractor2013 computation failed")?;

    // Extract beat ticks (in seconds)
    let ticks: Vec<f32> = result
        .ticks()
        .context("Failed to get ticks output")?
        .get();

    // Convert to f64 for downstream compatibility
    let beat_ticks: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();

    // Get Essentia's direct BPM output (accurate when run without min/max constraints)
    let raw_bpm: f32 = result
        .bpm()
        .context("Failed to get bpm output")?
        .get();

    // Extract real confidence from the multifeature tracker
    // Range is [0, 5.32] from TempoTapMaxAgreement; normalize to [0, 1]
    let raw_confidence: f32 = result
        .confidence()
        .context("Failed to get confidence output")?
        .get();
    let confidence = (raw_confidence / MAX_ESSENTIA_CONFIDENCE).clamp(0.0, 1.0);

    // Apply octave/triplet fitting to match user's desired tempo range
    let fitted_bpm = fit_bpm_to_range(raw_bpm as f64, config.min_tempo, config.max_tempo);

    log::info!(
        "BPM detection complete: raw={:.1} -> fitted={} BPM (range {}-{}), {} beats, confidence={:.2} (raw={:.2})",
        raw_bpm,
        fitted_bpm as u32,
        config.min_tempo,
        config.max_tempo,
        beat_ticks.len(),
        confidence,
        raw_confidence
    );

    Ok(BpmResult {
        bpm: fitted_bpm,
        beat_ticks,
        confidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fit_bpm_to_range_already_in_range() {
        assert_eq!(fit_bpm_to_range(175.0, 150, 180), 175.0);
        assert_eq!(fit_bpm_to_range(150.0, 150, 180), 150.0);
        assert_eq!(fit_bpm_to_range(180.0, 150, 180), 180.0);
    }

    #[test]
    fn test_fit_bpm_to_range_octave_double() {
        // 86 BPM should double to 172 for DnB range
        assert_eq!(fit_bpm_to_range(86.0, 150, 180), 172.0);
        assert_eq!(fit_bpm_to_range(87.5, 150, 180), 175.0);
        assert_eq!(fit_bpm_to_range(88.0, 150, 180), 176.0);
    }

    #[test]
    fn test_fit_bpm_to_range_octave_half() {
        // 340 BPM should halve to 170 for DnB range
        assert_eq!(fit_bpm_to_range(340.0, 150, 180), 170.0);
    }

    #[test]
    fn test_fit_bpm_to_range_triplet() {
        // 117 BPM × 1.5 = 175.5 → 176 for DnB range
        assert_eq!(fit_bpm_to_range(117.0, 150, 180), 176.0);
        // 116 BPM × 1.5 = 174 for DnB range
        assert_eq!(fit_bpm_to_range(116.0, 150, 180), 174.0);
    }
}
