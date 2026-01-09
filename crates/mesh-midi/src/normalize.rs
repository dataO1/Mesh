//! Value normalization for MIDI controls
//!
//! MIDI CC values are 0-127, but application controls have different ranges:
//! - Volume: 0.0 to 1.0
//! - Filter: -1.0 to 1.0 (bipolar)
//! - EQ: 0.0 to 1.0 with 0.5 as center
//!
//! This module handles the conversion automatically based on control type.

use crate::config::EncoderMode;

/// Predefined control value ranges
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlRange {
    /// Unit range: 0.0 to 1.0 (volume, gain, etc.)
    Unit,
    /// Bipolar range: -1.0 to 1.0 (filter, pan)
    Bipolar,
    /// EQ range: 0.0 to 1.0 with 0.5 as neutral center
    Eq,
    /// Custom range
    Custom { min: f32, max: f32 },
}

impl ControlRange {
    /// Get min value for this range
    pub fn min(&self) -> f32 {
        match self {
            Self::Unit => 0.0,
            Self::Bipolar => -1.0,
            Self::Eq => 0.0,
            Self::Custom { min, .. } => *min,
        }
    }

    /// Get max value for this range
    pub fn max(&self) -> f32 {
        match self {
            Self::Unit => 1.0,
            Self::Bipolar => 1.0,
            Self::Eq => 1.0,
            Self::Custom { max, .. } => *max,
        }
    }

    /// Get center value for this range (for EQ-style controls)
    pub fn center(&self) -> f32 {
        match self {
            Self::Unit => 0.5,
            Self::Bipolar => 0.0,
            Self::Eq => 0.5,
            Self::Custom { min, max } => (min + max) / 2.0,
        }
    }
}

/// Normalize a MIDI CC value (0-127) to the target range
///
/// # Arguments
/// * `midi_value` - Raw MIDI value (0-127)
/// * `range` - Target value range
/// * `center_deadzone` - Optional deadzone around center (in MIDI units, e.g., 5 = values 59-68 map to center)
///
/// # Returns
/// Normalized value in target range
pub fn normalize_cc_value(midi_value: u8, range: ControlRange, center_deadzone: Option<u8>) -> f32 {
    let midi = midi_value as f32;
    let midi_max = 127.0;
    let midi_center = 64.0;

    // Handle deadzone around center
    let effective_midi = if let Some(deadzone) = center_deadzone {
        let dz = deadzone as f32;
        let low = midi_center - dz;
        let high = midi_center + dz;

        if midi >= low && midi <= high {
            // In deadzone, snap to center
            midi_center
        } else if midi < low {
            // Below deadzone, remap 0..low to 0..center
            (midi / low) * midi_center
        } else {
            // Above deadzone, remap high..127 to center..127
            midi_center + ((midi - high) / (midi_max - high)) * (midi_max - midi_center)
        }
    } else {
        midi
    };

    // Convert to 0.0-1.0 first
    let normalized = effective_midi / midi_max;

    // Then map to target range
    let min = range.min();
    let max = range.max();
    min + normalized * (max - min)
}

/// Convert encoder relative value to scroll delta
///
/// Different encoders send different relative value formats:
/// - Relative: 1-63 = CW amount, 65-127 = CCW amount (64 is unused)
/// - RelativeSigned: <64 = CCW, >64 = CW, 64 = no change
pub fn encoder_to_delta(midi_value: u8, mode: EncoderMode) -> i32 {
    match mode {
        EncoderMode::Absolute => {
            // Not really meaningful for scroll, but handle it
            midi_value as i32 - 64
        }
        EncoderMode::Relative => {
            // 1-63 = CW (positive), 65-127 = CCW (negative)
            if midi_value >= 1 && midi_value <= 63 {
                midi_value as i32
            } else if midi_value >= 65 {
                -((midi_value as i32) - 64)
            } else {
                0
            }
        }
        EncoderMode::RelativeSigned => {
            // Value is signed around 64
            (midi_value as i32) - 64
        }
    }
}

/// Denormalize a value from target range back to MIDI (0-127)
///
/// Used for LED feedback where we need to convert app state to MIDI values.
pub fn denormalize_to_midi(value: f32, range: ControlRange) -> u8 {
    let min = range.min();
    let max = range.max();

    // Clamp and normalize to 0.0-1.0
    let clamped = value.clamp(min, max);
    let normalized = (clamped - min) / (max - min);

    // Convert to MIDI range
    (normalized * 127.0).round() as u8
}

/// Get the appropriate control range for an action
///
/// The system knows the expected range for each control type.
pub fn range_for_action(action: &str) -> ControlRange {
    match action {
        // Mixer controls
        "mixer.volume" | "mixer.eq_hi" | "mixer.eq_mid" | "mixer.eq_lo" => ControlRange::Eq,
        "mixer.filter" => ControlRange::Bipolar,
        "mixer.crossfader" => ControlRange::Unit,

        // Deck controls
        "deck.effect_param" => ControlRange::Unit,

        // Global controls
        "global.master_volume" | "global.cue_volume" => ControlRange::Unit,

        // Default to unit range
        _ => ControlRange::Unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_range() {
        assert_eq!(normalize_cc_value(0, ControlRange::Unit, None), 0.0);
        assert_eq!(normalize_cc_value(127, ControlRange::Unit, None), 1.0);
        assert!((normalize_cc_value(64, ControlRange::Unit, None) - 0.504).abs() < 0.01);
    }

    #[test]
    fn test_bipolar_range() {
        assert_eq!(normalize_cc_value(0, ControlRange::Bipolar, None), -1.0);
        assert_eq!(normalize_cc_value(127, ControlRange::Bipolar, None), 1.0);
        assert!((normalize_cc_value(64, ControlRange::Bipolar, None) - 0.008).abs() < 0.01);
    }

    #[test]
    fn test_deadzone() {
        // With deadzone of 5, values 59-69 should map to center
        let range = ControlRange::Bipolar;
        let center = normalize_cc_value(64, range, Some(5));
        let in_deadzone_low = normalize_cc_value(60, range, Some(5));
        let in_deadzone_high = normalize_cc_value(68, range, Some(5));

        // All should be close to 0 (center for bipolar)
        assert!((center - 0.008).abs() < 0.01);
        assert!((in_deadzone_low - 0.008).abs() < 0.01);
        assert!((in_deadzone_high - 0.008).abs() < 0.01);
    }

    #[test]
    fn test_encoder_relative() {
        // CW rotation (positive)
        assert_eq!(encoder_to_delta(1, EncoderMode::Relative), 1);
        assert_eq!(encoder_to_delta(10, EncoderMode::Relative), 10);

        // CCW rotation (negative)
        assert_eq!(encoder_to_delta(65, EncoderMode::Relative), -1);
        assert_eq!(encoder_to_delta(75, EncoderMode::Relative), -11);

        // No movement
        assert_eq!(encoder_to_delta(64, EncoderMode::Relative), 0);
        assert_eq!(encoder_to_delta(0, EncoderMode::Relative), 0);
    }

    #[test]
    fn test_denormalize() {
        assert_eq!(denormalize_to_midi(0.0, ControlRange::Unit), 0);
        assert_eq!(denormalize_to_midi(1.0, ControlRange::Unit), 127);
        assert_eq!(denormalize_to_midi(0.5, ControlRange::Unit), 64);

        assert_eq!(denormalize_to_midi(-1.0, ControlRange::Bipolar), 0);
        assert_eq!(denormalize_to_midi(0.0, ControlRange::Bipolar), 64);
        assert_eq!(denormalize_to_midi(1.0, ControlRange::Bipolar), 127);
    }
}
