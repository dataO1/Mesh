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
    /// // Track at -12 LUFS, target -9 LUFS = +3 dB boost
    /// let gain = config.calculate_gain_db(Some(-12.0));
    /// assert!((gain.unwrap() - 3.0).abs() < 0.001);
    /// ```
    pub fn calculate_gain_db(&self, track_lufs: Option<f32>) -> Option<f32> {
        if !self.auto_gain_enabled {
            return None;
        }
        track_lufs.map(|lufs| (self.target_lufs - lufs).clamp(self.min_gain_db, self.max_gain_db))
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
        (self.target_lufs - track_lufs).clamp(self.min_gain_db, self.max_gain_db)
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
        let config = LoudnessConfig::default();
        // Track at -12 LUFS, target -9 LUFS = +3 dB boost
        let gain_db = config.calculate_gain_db(Some(-12.0)).unwrap();
        assert!((gain_db - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_gain_calculation_cut() {
        let config = LoudnessConfig::default();
        // Track at -4 LUFS, target -9 LUFS = -5 dB cut
        let gain_db = config.calculate_gain_db(Some(-4.0)).unwrap();
        assert!((gain_db - (-5.0)).abs() < 0.001);
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
        let config = LoudnessConfig::default();
        // +6 dB should be ~2x linear gain
        let config_6db = LoudnessConfig {
            target_lufs: -6.0,
            ..Default::default()
        };
        let linear = config_6db.calculate_gain_linear(Some(-12.0));
        assert!((linear - 2.0).abs() < 0.01);
    }
}
