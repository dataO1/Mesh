//! Effect system - traits, chains, and parameter mapping
//!
//! This module provides a unified effect interface for all effect types:
//! - Native Rust effects
//! - Pure Data effects (via libpd)
//! - CLAP plugins (via clack-host)
//! - Multiband container (holds any effect type)

pub mod multiband;
pub mod native;

pub use multiband::{
    BandEffectInfo, BandState, EffectLocation, MacroMapping, MultibandConfig, MultibandError,
    MultibandHost, MultibandResult, MAX_BANDS, MAX_EFFECTS_PER_BAND, NUM_MACROS,
};

use crate::types::StereoBuffer;

/// Maximum number of UI knobs for effect control (hardware knob limit)
/// Note: Effects can have unlimited parameters internally; this is the UI display limit
pub const MAX_EFFECT_KNOBS: usize = 8;

/// Information about an effect parameter
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Parameter name for display
    pub name: String,
    /// Default value (0.0-1.0)
    pub default: f32,
    /// Minimum value (typically 0.0)
    pub min: f32,
    /// Maximum value (typically 1.0)
    pub max: f32,
    /// Unit label (e.g., "ms", "dB", "%")
    pub unit: String,
}

impl Default for ParamInfo {
    fn default() -> Self {
        Self {
            name: String::new(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
            unit: String::new(),
        }
    }
}

impl ParamInfo {
    /// Create a new parameter info with name and default value
    pub fn new(name: impl Into<String>, default: f32) -> Self {
        Self {
            name: name.into(),
            default,
            ..Default::default()
        }
    }

    /// Set the value range
    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Set the unit label
    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
}

/// Current parameter value with display formatting
#[derive(Debug, Clone, Copy)]
pub struct ParamValue {
    /// Normalized value (0.0-1.0)
    pub normalized: f32,
    /// Actual value after range mapping
    pub actual: f32,
}

impl Default for ParamValue {
    fn default() -> Self {
        Self {
            normalized: 0.5,
            actual: 0.5,
        }
    }
}

impl ParamValue {
    /// Create a new parameter value
    pub fn new(normalized: f32, actual: f32) -> Self {
        Self { normalized, actual }
    }

    /// Create from normalized value with the given param info
    pub fn from_normalized(normalized: f32, info: &ParamInfo) -> Self {
        let normalized = normalized.clamp(0.0, 1.0);
        let actual = info.min + normalized * (info.max - info.min);
        Self { normalized, actual }
    }
}

/// Information about an effect
#[derive(Debug, Clone)]
pub struct EffectInfo {
    /// Effect name for display
    pub name: String,
    /// Effect category (e.g., "Filter", "Delay", "Reverb", "Neural")
    pub category: String,
    /// Parameter descriptions (no limit - can be 100+ for CLAP plugins)
    /// The UI's 8 knobs are "slots" that can be assigned to any parameter index
    pub params: Vec<ParamInfo>,
    /// Processing latency in samples (reported by plugin)
    pub latency_samples: u32,
}

impl EffectInfo {
    /// Create a new effect info
    pub fn new(name: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            category: category.into(),
            params: Vec::new(),
            latency_samples: 0,
        }
    }

    /// Add a parameter to this effect
    ///
    /// Effects can have unlimited parameters. The UI's 8 knobs are "slots"
    /// that can be assigned to control any parameter index.
    pub fn with_param(mut self, param: ParamInfo) -> Self {
        self.params.push(param);
        self
    }

    /// Get the number of parameters
    pub fn param_count(&self) -> usize {
        self.params.len()
    }
}

/// The core effect trait - implemented by all audio effects
///
/// Effects process stereo audio buffers and can report their latency for
/// global compensation. All parameters are normalized (0.0-1.0) for easy
/// mapping to hardware knobs.
pub trait Effect: Send {
    /// Process a stereo buffer in-place
    ///
    /// The buffer contains interleaved stereo samples that should be
    /// processed at the given sample rate.
    fn process(&mut self, buffer: &mut StereoBuffer);

    /// Get the latency of this effect in samples
    ///
    /// This is used for global latency compensation across all stems.
    fn latency_samples(&self) -> u32;

    /// Get information about this effect (name, category, parameters)
    fn info(&self) -> &EffectInfo;

    /// Get the current parameter values
    fn get_params(&self) -> &[ParamValue];

    /// Set a parameter by index (normalized value 0.0-1.0)
    fn set_param(&mut self, index: usize, value: f32);

    /// Set the bypass state
    fn set_bypass(&mut self, bypass: bool);

    /// Check if the effect is bypassed
    fn is_bypassed(&self) -> bool;

    /// Reset the effect state (called on track load, etc.)
    fn reset(&mut self);

    /// Check for a pending plugin restart and handle it
    ///
    /// CLAP plugins may request a restart when their latency changes (e.g.,
    /// lookahead parameter adjusted). This method performs the
    /// deactivate → reactivate cycle and returns `Some(new_latency)` if the
    /// latency changed, or `None` if no restart was pending.
    ///
    /// Default implementation returns `None` (no restart support).
    fn poll_restart(&mut self) -> Option<u32> {
        None
    }
}

/// Base implementation helper for effects
///
/// Provides common functionality like bypass state and parameter storage.
#[derive(Debug, Clone)]
pub struct EffectBase {
    info: EffectInfo,
    params: Vec<ParamValue>,
    bypassed: bool,
}

impl EffectBase {
    /// Create a new effect base from effect info
    pub fn new(info: EffectInfo) -> Self {
        let params: Vec<ParamValue> = info
            .params
            .iter()
            .map(|p| ParamValue::from_normalized(p.default, p))
            .collect();
        Self {
            info,
            params,
            bypassed: false,
        }
    }

    /// Get the effect info
    pub fn info(&self) -> &EffectInfo {
        &self.info
    }

    /// Get mutable access to the effect info
    ///
    /// Used by ClapEffect to update latency_samples after a plugin restart.
    pub fn info_mut(&mut self) -> &mut EffectInfo {
        &mut self.info
    }

    /// Get the current parameter values
    pub fn get_params(&self) -> &[ParamValue] {
        &self.params
    }

    /// Set a parameter value
    pub fn set_param(&mut self, index: usize, value: f32) {
        if index < self.params.len() {
            self.params[index] = ParamValue::from_normalized(value, &self.info.params[index]);
        }
    }

    /// Get a parameter's actual (denormalized) value
    pub fn param_actual(&self, index: usize) -> f32 {
        self.params.get(index).map(|p| p.actual).unwrap_or(0.0)
    }

    /// Get a parameter's normalized value
    pub fn param_normalized(&self, index: usize) -> f32 {
        self.params.get(index).map(|p| p.normalized).unwrap_or(0.0)
    }

    /// Set bypass state
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }

    /// Check if bypassed
    pub fn is_bypassed(&self) -> bool {
        self.bypassed
    }
}

/// Knob mapping for effect chain controls
///
/// Maps a single hardware knob to one or more effect parameters
/// for live performance control.
#[derive(Debug, Clone)]
pub struct KnobMapping {
    /// Knob name/label
    pub name: String,
    /// Targets for this knob (effect index + param index pairs)
    pub targets: Vec<(usize, usize)>,
    /// Current value (0.0-1.0)
    pub value: f32,
}

impl KnobMapping {
    /// Create a new knob mapping
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            targets: Vec::new(),
            value: 0.0,
        }
    }

    /// Add a target (effect index, param index)
    pub fn add_target(&mut self, effect_idx: usize, param_idx: usize) {
        self.targets.push((effect_idx, param_idx));
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_param_info() {
        let param = ParamInfo::new("Gain", 1.0)
            .with_range(-24.0, 24.0)
            .with_unit("dB");

        assert_eq!(param.name, "Gain");
        assert_eq!(param.default, 1.0);
        assert_eq!(param.min, -24.0);
        assert_eq!(param.max, 24.0);
        assert_eq!(param.unit, "dB");
    }

    #[test]
    fn test_param_value_mapping() {
        let info = ParamInfo::new("Test", 0.5).with_range(0.0, 100.0);

        let value = ParamValue::from_normalized(0.5, &info);
        assert_eq!(value.normalized, 0.5);
        assert_eq!(value.actual, 50.0);

        let value = ParamValue::from_normalized(1.0, &info);
        assert_eq!(value.actual, 100.0);

        let value = ParamValue::from_normalized(0.0, &info);
        assert_eq!(value.actual, 0.0);
    }

    #[test]
    fn test_effect_info() {
        let info = EffectInfo::new("Test Effect", "Filter")
            .with_param(ParamInfo::new("Cutoff", 0.5))
            .with_param(ParamInfo::new("Resonance", 0.0));

        assert_eq!(info.name, "Test Effect");
        assert_eq!(info.category, "Filter");
        assert_eq!(info.param_count(), 2);
    }

    #[test]
    fn test_effect_base() {
        let info = EffectInfo::new("Test", "Test")
            .with_param(ParamInfo::new("P1", 0.5).with_range(0.0, 100.0))
            .with_param(ParamInfo::new("P2", 0.0).with_range(-1.0, 1.0));

        let mut base = EffectBase::new(info);

        // Check initial values
        assert_eq!(base.param_actual(0), 50.0); // 0.5 * 100 = 50
        assert_eq!(base.param_actual(1), -1.0); // 0.0 * 2 - 1 = -1

        // Set new values
        base.set_param(0, 1.0);
        assert_eq!(base.param_actual(0), 100.0);

        base.set_param(1, 0.5);
        assert_eq!(base.param_actual(1), 0.0); // midpoint of -1 to 1

        // Bypass
        assert!(!base.is_bypassed());
        base.set_bypass(true);
        assert!(base.is_bypassed());
    }
}
