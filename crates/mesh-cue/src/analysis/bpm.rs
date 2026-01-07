//! BPM detection using multiple algorithms
//!
//! This module provides tempo detection via a trait-based abstraction,
//! allowing different algorithms to be swapped based on configuration.
//!
//! ## Available Algorithms
//!
//! **Essentia-based (built-in):**
//! - `EssentiaMultifeature` - RhythmExtractor2013 with multifeature method (default)
//! - `EssentiaDegara` - RhythmExtractor2013 with degara method
//! - `EssentiaBeatTrackerMulti` - Standalone BeatTrackerMultiFeature
//! - `EssentiaBeatTrackerDegara` - Standalone BeatTrackerDegara
//!
//! **Python-based (external):**
//! - `MadmomDbn` - Madmom DBN Beat Tracker (requires Python + madmom)
//! - `BeatFM` - BeatFM 2025 transformer-based tracker

use anyhow::{anyhow, Context, Result};
use essentia::algorithm::rhythm::beat_tracker_degara::BeatTrackerDegara;
use essentia::algorithm::rhythm::beat_tracker_multi_feature::BeatTrackerMultiFeature;
use essentia::algorithm::rhythm::rhythm_extractor_2013::RhythmExtractor2013;
use essentia::data::GetFromDataContainer;
use essentia::essentia::Essentia;

use super::algorithm::BpmAlgorithm;
use crate::config::BpmConfig;

/// Result of BPM detection
#[derive(Debug, Clone)]
pub struct BpmResult {
    /// Detected BPM (beats per minute), rounded to nearest integer
    pub bpm: f64,
    /// Raw BPM before rounding
    pub raw_bpm: f64,
    /// Detection confidence (0.0 - 1.0, where available)
    pub confidence: f64,
    /// Beat positions in seconds
    pub beats: Vec<f64>,
}

impl BpmResult {
    /// Create a new BPM result
    pub fn new(bpm: f64, confidence: f64, beats: Vec<f64>) -> Self {
        Self {
            bpm: bpm.round(),
            raw_bpm: bpm,
            confidence,
            beats,
        }
    }
}

/// Trait for BPM detection implementations
///
/// Each detector takes audio samples and returns a BPM result.
/// Implementations can be swapped based on user configuration.
pub trait BpmDetector: Send + Sync {
    /// Detect BPM from audio samples
    ///
    /// # Arguments
    /// * `samples` - Mono audio samples at 44.1kHz (Essentia's expected rate)
    /// * `config` - BPM detection configuration (min/max tempo range)
    ///
    /// # Returns
    /// BPM detection result with tempo, confidence, and beat positions
    fn detect(&self, samples: &[f32], config: &BpmConfig) -> Result<BpmResult>;

    /// Get the algorithm identifier
    fn algorithm(&self) -> BpmAlgorithm;
}

/// Essentia-based BPM detector
///
/// Supports multiple Essentia algorithms:
/// - RhythmExtractor2013 (multifeature and degara methods)
/// - BeatTrackerMultiFeature
/// - BeatTrackerDegara
pub struct EssentiaDetector {
    algorithm: BpmAlgorithm,
}

impl EssentiaDetector {
    /// Create a new Essentia detector for the specified algorithm
    pub fn new(algorithm: BpmAlgorithm) -> Result<Self> {
        if !algorithm.is_essentia() {
            return Err(anyhow!(
                "Algorithm {:?} is not an Essentia algorithm",
                algorithm
            ));
        }
        Ok(Self { algorithm })
    }

    /// Detect using RhythmExtractor2013
    fn rhythm_extractor(
        &self,
        samples: &[f32],
        config: &BpmConfig,
        method: &str,
    ) -> Result<BpmResult> {
        // DIAGNOSTIC: Log what samples Essentia actually receives
        let sample_sum: f64 = samples.iter().take(10000).map(|&s| s as f64).sum();
        let sample_max: f32 = samples.iter().take(10000).fold(0.0f32, |a, &b| a.max(b.abs()));
        eprintln!(
            "ESSENTIA_DIAG: rhythm_extractor receiving {} samples, sum(10k)={:.6}, max(10k)={:.6}, min_tempo={}, max_tempo={}, method={}",
            samples.len(), sample_sum, sample_max, config.min_tempo, config.max_tempo, method
        );

        let essentia = Essentia::new();

        let mut rhythm = essentia
            .create::<RhythmExtractor2013>()
            .min_tempo(config.min_tempo)
            .context("Failed to set min_tempo")?
            .max_tempo(config.max_tempo)
            .context("Failed to set max_tempo")?
            .method(method)
            .context("Failed to set method")?
            .configure()
            .context("Failed to configure RhythmExtractor2013")?;

        let result = rhythm
            .compute(samples)
            .context("RhythmExtractor2013 computation failed")?;

        let bpm: f32 = result
            .bpm()
            .context("Failed to get BPM output")?
            .get();

        let ticks: Vec<f32> = result
            .ticks()
            .context("Failed to get ticks output")?
            .get();

        let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();

        // DIAGNOSTIC: Calculate BPM from beats ourselves to compare with Essentia's BPM output
        let calculated_bpm = calculate_bpm_from_beats(&beats);
        let last_beat = beats.last().copied().unwrap_or(0.0);
        let first_beat = beats.first().copied().unwrap_or(0.0);
        let beat_span = last_beat - first_beat;

        // DIAGNOSTIC: Log what Essentia returned vs our calculation (eprintln works in subprocess)
        eprintln!(
            "ESSENTIA_DIAG: RhythmExtractor2013 returned BPM={:.4}, beats={}, first={:.4}, last={:.4}, span={:.2}s",
            bpm,
            beats.len(),
            first_beat,
            last_beat,
            beat_span
        );
        eprintln!(
            "ESSENTIA_DIAG: Calculated BPM from beats={:.4} (vs Essentia's {:.4}, diff={:.4})",
            calculated_bpm,
            bpm,
            (calculated_bpm - bpm as f64).abs()
        );

        // FIX: Use calculated BPM from beat intervals instead of Essentia's buggy BPM output
        // Essentia's RhythmExtractor2013 BPM output appears to return constant values regardless of input.
        // The beat positions (ticks) are correct, so we calculate BPM from those.
        let final_bpm = if calculated_bpm > 0.0 {
            calculated_bpm
        } else {
            bpm as f64 // Fallback to Essentia's value if calculation fails
        };

        log::info!(
            "RhythmExtractor2013 ({}): {:.2} BPM (calculated from {} beats, Essentia reported {:.2})",
            method,
            final_bpm,
            beats.len(),
            bpm
        );

        // RhythmExtractor2013 doesn't provide confidence, use 0.8 as default
        Ok(BpmResult::new(final_bpm, 0.8, beats))
    }

    /// Detect using BeatTrackerMultiFeature
    fn beat_tracker_multi(&self, samples: &[f32], config: &BpmConfig) -> Result<BpmResult> {
        let essentia = Essentia::new();

        let mut tracker = essentia
            .create::<BeatTrackerMultiFeature>()
            .min_tempo(config.min_tempo)
            .context("Failed to set min_tempo")?
            .max_tempo(config.max_tempo)
            .context("Failed to set max_tempo")?
            .configure()
            .context("Failed to configure BeatTrackerMultiFeature")?;

        let result = tracker
            .compute(samples)
            .context("BeatTrackerMultiFeature computation failed")?;

        let ticks: Vec<f32> = result
            .ticks()
            .context("Failed to get ticks output")?
            .get();

        let confidence: f32 = result
            .confidence()
            .context("Failed to get confidence output")?
            .get();

        let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();
        let bpm = calculate_bpm_from_beats(&beats);

        // Normalize confidence from [0, 5.32] to [0, 1]
        let normalized_confidence = (confidence / 5.32).min(1.0);

        log::info!(
            "BeatTrackerMultiFeature: {:.2} BPM, {} beats, confidence: {:.2}",
            bpm,
            beats.len(),
            normalized_confidence
        );

        Ok(BpmResult::new(bpm, normalized_confidence as f64, beats))
    }

    /// Detect using BeatTrackerDegara
    fn beat_tracker_degara(&self, samples: &[f32], config: &BpmConfig) -> Result<BpmResult> {
        let essentia = Essentia::new();

        let mut tracker = essentia
            .create::<BeatTrackerDegara>()
            .min_tempo(config.min_tempo)
            .context("Failed to set min_tempo")?
            .max_tempo(config.max_tempo)
            .context("Failed to set max_tempo")?
            .configure()
            .context("Failed to configure BeatTrackerDegara")?;

        let result = tracker
            .compute(samples)
            .context("BeatTrackerDegara computation failed")?;

        let ticks: Vec<f32> = result
            .ticks()
            .context("Failed to get ticks output")?
            .get();

        let beats: Vec<f64> = ticks.iter().map(|&t| t as f64).collect();
        let bpm = calculate_bpm_from_beats(&beats);

        log::info!(
            "BeatTrackerDegara: {:.2} BPM, {} beats",
            bpm,
            beats.len()
        );

        // BeatTrackerDegara doesn't provide confidence
        Ok(BpmResult::new(bpm, 0.7, beats))
    }
}

impl BpmDetector for EssentiaDetector {
    fn detect(&self, samples: &[f32], config: &BpmConfig) -> Result<BpmResult> {
        log::info!(
            "Starting BPM detection with {:?} on {} samples (range: {}-{} BPM)",
            self.algorithm,
            samples.len(),
            config.min_tempo,
            config.max_tempo
        );

        match self.algorithm {
            BpmAlgorithm::EssentiaMultifeature => {
                self.rhythm_extractor(samples, config, "multifeature")
            }
            BpmAlgorithm::EssentiaDegara => self.rhythm_extractor(samples, config, "degara"),
            BpmAlgorithm::EssentiaBeatTrackerMulti => self.beat_tracker_multi(samples, config),
            BpmAlgorithm::EssentiaBeatTrackerDegara => self.beat_tracker_degara(samples, config),
            _ => Err(anyhow!(
                "Algorithm {:?} is not supported by EssentiaDetector",
                self.algorithm
            )),
        }
    }

    fn algorithm(&self) -> BpmAlgorithm {
        self.algorithm
    }
}

/// Calculate BPM from beat positions using median inter-beat interval
///
/// Uses median instead of mean for robustness against outliers
/// (missed beats or extra detected beats).
fn calculate_bpm_from_beats(beats: &[f64]) -> f64 {
    if beats.len() < 2 {
        log::warn!("Not enough beats to calculate BPM (found {})", beats.len());
        return 0.0;
    }

    // Calculate inter-beat intervals
    let mut intervals: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();

    // Filter out unreasonable intervals (< 0.2s = 300 BPM, > 2s = 30 BPM)
    intervals.retain(|&i| i > 0.2 && i < 2.0);

    if intervals.is_empty() {
        log::warn!("No valid inter-beat intervals found");
        return 0.0;
    }

    // Sort for median calculation
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Use median interval (robust to outliers)
    let median_interval = intervals[intervals.len() / 2];

    60.0 / median_interval
}

/// Create a BPM detector for the specified algorithm
///
/// Returns an appropriate detector implementation based on the algorithm type.
/// For Python algorithms, this will create a Python subprocess detector.
pub fn create_detector(algorithm: BpmAlgorithm) -> Result<Box<dyn BpmDetector>> {
    match algorithm {
        BpmAlgorithm::EssentiaMultifeature
        | BpmAlgorithm::EssentiaDegara
        | BpmAlgorithm::EssentiaBeatTrackerMulti
        | BpmAlgorithm::EssentiaBeatTrackerDegara => {
            Ok(Box::new(EssentiaDetector::new(algorithm)?))
        }
        BpmAlgorithm::MadmomDbn => {
            use super::python::MadmomDetector;
            Ok(Box::new(MadmomDetector::new()?))
        }
        BpmAlgorithm::BeatFM => {
            // BeatFM not yet implemented
            Err(anyhow!(
                "BeatFM algorithm not yet implemented. Use Madmom or an Essentia algorithm."
            ))
        }
    }
}

// ============================================================================
// Legacy API (backward compatibility)
// ============================================================================

/// Detect BPM and beat positions from audio samples (legacy API)
///
/// Uses Essentia's RhythmExtractor2013 algorithm which is optimized
/// for electronic/dance music and provides both BPM and beat tick positions.
///
/// # Arguments
/// * `samples` - Mono audio samples at 44.1kHz
/// * `config` - BPM detection configuration (min/max tempo range, rounding)
///
/// # Returns
/// Tuple of (BPM value, beat positions in seconds)
/// BPM is rounded to nearest integer if `config.round_bpm` is true,
/// otherwise returns raw decimal value.
pub fn detect_bpm(samples: &[f32], config: &BpmConfig) -> Result<(f64, Vec<f64>)> {
    // Use algorithm from config
    let algorithm = config.algorithm;
    let detector = create_detector(algorithm)?;
    let result = detector.detect(samples, config)?;

    // Apply rounding based on config
    let final_bpm = if config.round_bpm {
        result.bpm // Already rounded in BpmResult::new()
    } else {
        result.raw_bpm // Use raw decimal value
    };

    log::info!(
        "BPM detection complete: {:.2} BPM (raw: {:.2}, rounded: {}), {} beats detected",
        final_bpm,
        result.raw_bpm,
        config.round_bpm,
        result.beats.len()
    );

    Ok((final_bpm, result.beats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_bpm_from_beats() {
        // 120 BPM = 0.5s per beat
        let beats = vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5];
        let bpm = calculate_bpm_from_beats(&beats);
        assert!((bpm - 120.0).abs() < 1.0);
    }

    #[test]
    fn test_calculate_bpm_with_outlier() {
        // 120 BPM with one missed beat (outlier interval of 1.0s)
        let beats = vec![0.0, 0.5, 1.5, 2.0, 2.5, 3.0];
        let bpm = calculate_bpm_from_beats(&beats);
        // Should still be close to 120 due to median
        assert!((bpm - 120.0).abs() < 5.0);
    }

    #[test]
    fn test_calculate_bpm_insufficient_beats() {
        let beats = vec![0.0];
        let bpm = calculate_bpm_from_beats(&beats);
        assert_eq!(bpm, 0.0);
    }

    #[test]
    fn test_essentia_detector_creation() {
        let detector = EssentiaDetector::new(BpmAlgorithm::EssentiaMultifeature);
        assert!(detector.is_ok());

        let detector = EssentiaDetector::new(BpmAlgorithm::MadmomDbn);
        assert!(detector.is_err());
    }

    #[test]
    fn test_bpm_result_rounding() {
        let result = BpmResult::new(174.3, 0.9, vec![]);
        assert_eq!(result.bpm, 174.0);
        assert_eq!(result.raw_bpm, 174.3);

        let result = BpmResult::new(174.7, 0.9, vec![]);
        assert_eq!(result.bpm, 175.0);
    }
}
