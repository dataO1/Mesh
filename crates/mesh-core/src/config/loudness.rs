//! Loudness normalization configuration
//!
//! Controls automatic gain compensation to normalize tracks to a target loudness.
//! Uses LUFS values measured during import (EBU R128 integrated loudness).

use serde::{Deserialize, Serialize};

/// Loudness normalization configuration
///
/// Provides gain compensation calculations based on LUFS measurements.
/// Used by both mesh-player (runtime normalization) and mesh-cue (export-time scaling).
///
/// # LUFS Background
///
/// LUFS (Loudness Units Full Scale) is the standard for measuring perceived loudness.
/// A typical club track might be around -6 to -8 LUFS, while more dynamic material
/// might be -12 to -14 LUFS. This config allows normalizing to a consistent level.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoudnessConfig {
    /// Target loudness in LUFS
    /// Tracks are gain-compensated to reach this level.
    /// Default: -9.0 LUFS (balanced loudness suitable for mixing)
    pub target_lufs: f32,

    /// Enable automatic gain compensation based on track LUFS
    /// When disabled, all gain calculations return unity (1.0).
    /// Default: true
    pub auto_gain_enabled: bool,

    /// Maximum boost in dB (safety limit for very quiet tracks)
    /// Prevents excessive amplification of quiet tracks.
    /// Default: 12.0 dB
    pub max_gain_db: f32,

    /// Maximum cut in dB (safety limit for very loud tracks)
    /// Prevents excessive attenuation of loud tracks.
    /// Default: -24.0 dB
    pub min_gain_db: f32,
}

impl Default for LoudnessConfig {
    fn default() -> Self {
        Self {
            target_lufs: -9.0,
            auto_gain_enabled: true,
            max_gain_db: 12.0,
            min_gain_db: -24.0,
        }
    }
}

impl LoudnessConfig {
    /// Compute raw gain in dB before safety clamping.
    ///
    /// Applies a symmetric perceptual density correction on both boost and cut:
    ///
    ///   `gain = delta × (1 + 1 / |target|)`
    ///
    /// where `delta = target_lufs − track_lufs`.
    ///
    /// **Why symmetric:** the core issue is perceptual density, not LUFS measurement
    /// accuracy. A track at −4 LUFS, cut to match a −9 LUFS target on the meter,
    /// still carries the spectral saturation and consistent RMS of a heavily limited
    /// track — it will punch through a mix even at the same measured level. Equally,
    /// a −14 LUFS track boosted to −9 LUFS still feels sparse and weak because it
    /// lacks that density. Both directions require extra correction.
    ///
    /// **Why `1/|target|`:** this normalises the bias against 0 LUFS (the density
    /// ceiling). At a loud mixing standard (−6 LUFS, coefficient ≈ 0.167) density
    /// differences between tracks are more perceptually significant, so the bias is
    /// stronger. At a more dynamic standard (−14 LUFS, coefficient ≈ 0.071) LUFS
    /// is a more honest perceptual measure and the correction weakens accordingly.
    /// No separate config knob is needed — the target already encodes this.
    ///
    /// Example at target=−9: delta=±5 → gain=±5.56 dB (vs ±5.0 dB linear)
    /// Example at target=−6: delta=±5 → gain=±5.83 dB (vs ±5.0 dB linear)
    fn raw_gain_db(&self, track_lufs: f32) -> f32 {
        let delta = self.target_lufs - track_lufs;
        // Symmetric bias: same multiplier for boosts and cuts.
        // At target=-9: multiplier = 1 + 1/9 ≈ 1.111
        // At target=-6: multiplier = 1 + 1/6 ≈ 1.167
        delta * (1.0 + 1.0 / self.target_lufs.abs())
    }

    /// Calculate gain compensation in dB for a track
    ///
    /// Returns `None` if LUFS is not available or auto-gain is disabled.
    ///
    /// # Arguments
    /// * `track_lufs` - The measured LUFS of the track (None if not measured)
    ///
    /// # Returns
    /// The gain adjustment in dB, clamped to safety limits
    ///
    /// # Example
    /// ```
    /// use mesh_core::config::LoudnessConfig;
    ///
    /// let config = LoudnessConfig::default();
    /// // Track at -12 LUFS, target -9 LUFS → delta=3, bias multiplier=1.111 → +3.33 dB
    /// let gain = config.calculate_gain_db(Some(-12.0));
    /// assert!((gain.unwrap() - 3.333).abs() < 0.01);
    /// ```
    pub fn calculate_gain_db(&self, track_lufs: Option<f32>) -> Option<f32> {
        if !self.auto_gain_enabled {
            return None;
        }
        track_lufs.map(|lufs| self.raw_gain_db(lufs).clamp(self.min_gain_db, self.max_gain_db))
    }

    /// Calculate gain compensation in dB for a track (non-optional variant)
    ///
    /// Useful when LUFS is known to be available.
    ///
    /// # Arguments
    /// * `track_lufs` - The measured LUFS of the track
    ///
    /// # Returns
    /// The gain adjustment in dB, clamped to safety limits
    pub fn calculate_gain_db_direct(&self, track_lufs: f32) -> f32 {
        self.raw_gain_db(track_lufs).clamp(self.min_gain_db, self.max_gain_db)
    }

    /// Calculate linear gain multiplier for a track
    ///
    /// Returns 1.0 (unity gain) if LUFS is not available or auto-gain is disabled.
    ///
    /// # Arguments
    /// * `track_lufs` - The measured LUFS of the track (None if not measured)
    ///
    /// # Returns
    /// Linear gain multiplier (1.0 = no change)
    pub fn calculate_gain_linear(&self, track_lufs: Option<f32>) -> f32 {
        self.calculate_gain_db(track_lufs)
            .map(|db| 10.0_f32.powf(db / 20.0))
            .unwrap_or(1.0)
    }

    /// Calculate linear gain multiplier for a track (non-optional variant)
    ///
    /// # Arguments
    /// * `track_lufs` - The measured LUFS of the track
    ///
    /// # Returns
    /// Linear gain multiplier
    pub fn calculate_gain_linear_direct(&self, track_lufs: f32) -> f32 {
        let db = self.calculate_gain_db_direct(track_lufs);
        10.0_f32.powf(db / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let config = LoudnessConfig::default();
        assert_eq!(config.target_lufs, -9.0);
        assert!(config.auto_gain_enabled);
        assert_eq!(config.max_gain_db, 12.0);
        assert_eq!(config.min_gain_db, -24.0);
    }

    #[test]
    fn test_gain_calculation_boost() {
        let config = LoudnessConfig::default(); // target = -9.0
        // Track at -12 LUFS, target -9 LUFS: delta=3, multiplier = 1 + 1/9 ≈ 1.111
        // Expected: 3 × 1.111 = 3.333 dB (biased boost, not plain 3.0)
        let gain_db = config.calculate_gain_db(Some(-12.0)).unwrap();
        assert!((gain_db - 3.333).abs() < 0.01);
    }

    #[test]
    fn test_gain_calculation_cut() {
        let config = LoudnessConfig::default(); // target = -9.0
        // Track at -4 LUFS, target -9 LUFS: delta=-5, multiplier = 1 + 1/9 ≈ 1.111
        // Expected: -5 × 1.111 = -5.556 dB (more cut than plain -5, density bias)
        let gain_db = config.calculate_gain_db(Some(-4.0)).unwrap();
        assert!((gain_db - (-5.556)).abs() < 0.01);
    }

    #[test]
    fn test_boost_bias_scales_with_delta() {
        // Larger deficit → larger proportional extra boost
        let config = LoudnessConfig { target_lufs: -6.0, ..Default::default() };
        let gain_small = config.calculate_gain_db_direct(-8.0); // delta=2
        let gain_large = config.calculate_gain_db_direct(-14.0); // delta=8
        // Both should exceed their plain linear values, and the ratio of extras should scale
        assert!(gain_small > 2.0);
        assert!(gain_large > 8.0);
        let extra_small = gain_small - 2.0;
        let extra_large = gain_large - 8.0;
        // Extra boost grows proportionally: large/small ≈ 8/2 = 4
        assert!((extra_large / extra_small - 4.0).abs() < 0.01);
    }

    #[test]
    fn test_cut_has_symmetric_bias() {
        // Loud tracks get the same density bias as quiet tracks — symmetric formula
        let config = LoudnessConfig { target_lufs: -6.0, ..Default::default() };
        let gain = config.calculate_gain_db_direct(-4.0); // delta=-2, multiplier=1+1/6=1.167
        assert!((gain - (-2.333)).abs() < 0.01);
    }

    #[test]
    fn test_gain_clamping() {
        let config = LoudnessConfig::default();

        // Very quiet track: limited to max boost
        let gain_db = config.calculate_gain_db(Some(-30.0)).unwrap();
        assert_eq!(gain_db, 12.0);

        // Very loud track: limited to max cut
        let gain_db = config.calculate_gain_db(Some(10.0)).unwrap();
        assert_eq!(gain_db, -19.0); // target (-9) - track (10) = -19, within range
    }

    #[test]
    fn test_disabled_auto_gain() {
        let config = LoudnessConfig {
            auto_gain_enabled: false,
            ..Default::default()
        };
        assert!(config.calculate_gain_db(Some(-12.0)).is_none());
        assert_eq!(config.calculate_gain_linear(Some(-12.0)), 1.0);
    }

    #[test]
    fn test_no_lufs_returns_unity() {
        let config = LoudnessConfig::default();
        assert!(config.calculate_gain_db(None).is_none());
        assert_eq!(config.calculate_gain_linear(None), 1.0);
    }

    #[test]
    fn test_linear_gain_conversion() {
        // Track at -12 LUFS, target -6 LUFS: delta=6, multiplier=1+1/6=1.167 → 7.0 dB → ~2.239x
        let config = LoudnessConfig {
            target_lufs: -6.0,
            ..Default::default()
        };
        let linear = config.calculate_gain_linear(Some(-12.0));
        let expected = 10.0_f32.powf(7.0 / 20.0); // ~2.239
        assert!((linear - expected).abs() < 0.01);
    }
}
