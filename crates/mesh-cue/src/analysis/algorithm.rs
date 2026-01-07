//! BPM detection algorithm configuration
//!
//! Defines available algorithms for tempo detection, including
//! Essentia-based and Python-based options.

use serde::{Deserialize, Serialize};

/// Available BPM detection algorithms
///
/// Each algorithm has different strengths:
/// - Essentia algorithms: Fast, no external dependencies
/// - Python algorithms: Higher accuracy, require Python + libraries installed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BpmAlgorithm {
    /// Essentia RhythmExtractor2013 with multifeature method (default)
    ///
    /// Uses multiple feature extraction methods and returns the most confident result.
    /// Best general-purpose algorithm, good for most electronic music.
    #[default]
    EssentiaMultifeature,

    /// Essentia RhythmExtractor2013 with degara method
    ///
    /// Uses the Degara beat tracking algorithm. Can be more accurate for
    /// complex rhythms but may be slower.
    EssentiaDegara,

    /// Essentia BeatTrackerMultiFeature (standalone)
    ///
    /// Standalone beat tracker that combines multiple onset detection functions.
    /// Good for tracks with clear transients.
    EssentiaBeatTrackerMulti,

    /// Essentia BeatTrackerDegara (standalone)
    ///
    /// Standalone implementation of the Degara beat tracking algorithm.
    /// Based on complex spectral difference onset detection.
    EssentiaBeatTrackerDegara,

    /// Madmom DBN Beat Tracker (Python)
    ///
    /// Uses Madmom's deep neural network-based beat tracker with
    /// Dynamic Bayesian Network inference. Highly accurate for EDM.
    /// Requires: `pip install madmom`
    MadmomDbn,

    /// BeatFM 2025 (Python)
    ///
    /// State-of-the-art transformer-based beat tracker from 2025 research.
    /// Requires separate installation.
    BeatFM,
}

impl BpmAlgorithm {
    /// Get all available algorithms for UI picker
    pub const fn all() -> &'static [BpmAlgorithm] {
        &[
            BpmAlgorithm::EssentiaMultifeature,
            BpmAlgorithm::EssentiaDegara,
            BpmAlgorithm::EssentiaBeatTrackerMulti,
            BpmAlgorithm::EssentiaBeatTrackerDegara,
            BpmAlgorithm::MadmomDbn,
            BpmAlgorithm::BeatFM,
        ]
    }

    /// Check if this algorithm requires Python
    pub const fn requires_python(&self) -> bool {
        matches!(self, BpmAlgorithm::MadmomDbn | BpmAlgorithm::BeatFM)
    }

    /// Check if this is an Essentia-based algorithm
    pub const fn is_essentia(&self) -> bool {
        matches!(
            self,
            BpmAlgorithm::EssentiaMultifeature
                | BpmAlgorithm::EssentiaDegara
                | BpmAlgorithm::EssentiaBeatTrackerMulti
                | BpmAlgorithm::EssentiaBeatTrackerDegara
        )
    }

    /// Get human-readable name for UI display
    pub const fn display_name(&self) -> &'static str {
        match self {
            BpmAlgorithm::EssentiaMultifeature => "Essentia Multifeature",
            BpmAlgorithm::EssentiaDegara => "Essentia Degara",
            BpmAlgorithm::EssentiaBeatTrackerMulti => "Essentia Beat Tracker Multi",
            BpmAlgorithm::EssentiaBeatTrackerDegara => "Essentia Beat Tracker Degara",
            BpmAlgorithm::MadmomDbn => "Madmom DBN (Python)",
            BpmAlgorithm::BeatFM => "BeatFM 2025 (Python)",
        }
    }

    /// Get description for tooltips
    pub const fn description(&self) -> &'static str {
        match self {
            BpmAlgorithm::EssentiaMultifeature => {
                "Default algorithm using multiple feature extraction methods"
            }
            BpmAlgorithm::EssentiaDegara => {
                "Uses Degara beat tracking, good for complex rhythms"
            }
            BpmAlgorithm::EssentiaBeatTrackerMulti => {
                "Standalone tracker combining multiple onset functions"
            }
            BpmAlgorithm::EssentiaBeatTrackerDegara => {
                "Standalone Degara tracker using spectral difference"
            }
            BpmAlgorithm::MadmomDbn => {
                "Deep neural network beat tracker, highly accurate for EDM"
            }
            BpmAlgorithm::BeatFM => {
                "State-of-the-art transformer-based tracker (2025)"
            }
        }
    }
}

impl std::fmt::Display for BpmAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_algorithm() {
        let algo = BpmAlgorithm::default();
        assert_eq!(algo, BpmAlgorithm::EssentiaMultifeature);
    }

    #[test]
    fn test_python_detection() {
        assert!(!BpmAlgorithm::EssentiaMultifeature.requires_python());
        assert!(!BpmAlgorithm::EssentiaDegara.requires_python());
        assert!(BpmAlgorithm::MadmomDbn.requires_python());
        assert!(BpmAlgorithm::BeatFM.requires_python());
    }

    #[test]
    fn test_serde_roundtrip() {
        let algo = BpmAlgorithm::EssentiaDegara;
        let json = serde_json::to_string(&algo).unwrap();
        assert_eq!(json, "\"essentia_degara\"");

        let parsed: BpmAlgorithm = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, algo);
    }

    #[test]
    fn test_all_algorithms() {
        let all = BpmAlgorithm::all();
        assert_eq!(all.len(), 6);
    }
}
