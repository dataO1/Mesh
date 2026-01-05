//! Effect system - traits, chains, and parameter mapping
//!
//! This module provides a unified effect interface for both native Rust effects
//! and Pure Data effects loaded via libpd.

pub mod native;

use crate::types::StereoBuffer;

/// Maximum number of parameters per effect (maps to 8 hardware knobs)
pub const MAX_EFFECT_PARAMS: usize = 8;

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
    /// Parameter descriptions (up to MAX_EFFECT_PARAMS)
    pub params: Vec<ParamInfo>,
}

impl EffectInfo {
    /// Create a new effect info
    pub fn new(name: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            category: category.into(),
            params: Vec::new(),
        }
    }

    /// Add a parameter to this effect
    pub fn with_param(mut self, param: ParamInfo) -> Self {
        assert!(self.params.len() < MAX_EFFECT_PARAMS, "Too many parameters");
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

/// Maximum number of knobs per effect chain
pub const CHAIN_KNOB_COUNT: usize = 8;

/// An effect chain for a single stem
///
/// Processes audio through a series of effects in order. Each chain
/// has 8 mappable knobs for live control and mute/solo functionality.
pub struct EffectChain {
    /// Effects in the chain (processed in order)
    effects: Vec<Box<dyn Effect>>,
    /// Knob mappings for this chain
    knobs: [KnobMapping; CHAIN_KNOB_COUNT],
    /// Mute state
    muted: bool,
    /// Solo state
    soloed: bool,
    /// Cached total latency (recalculated when chain changes)
    cached_latency: u32,
}

impl EffectChain {
    /// Create a new empty effect chain
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
            knobs: std::array::from_fn(|i| KnobMapping::new(format!("Knob {}", i + 1))),
            muted: false,
            soloed: false,
            cached_latency: 0,
        }
    }

    /// Process audio through all effects in the chain
    ///
    /// `any_soloed` indicates if any chain in the group has solo enabled.
    /// If true and this chain isn't soloed, it will output silence.
    pub fn process(&mut self, buffer: &mut StereoBuffer, any_soloed: bool) {
        // If muted, just zero the buffer
        if self.muted {
            buffer.fill_silence();
            return;
        }

        // If another chain is soloed and this one isn't, silence it
        if any_soloed && !self.soloed {
            buffer.fill_silence();
            return;
        }

        // Process through each effect
        for effect in &mut self.effects {
            if !effect.is_bypassed() {
                effect.process(buffer);
            }
        }
    }

    /// Get the total latency of the chain in samples
    pub fn total_latency(&self) -> u32 {
        self.cached_latency
    }

    /// Recalculate the cached latency
    fn update_latency(&mut self) {
        self.cached_latency = self
            .effects
            .iter()
            .filter(|e| !e.is_bypassed())
            .map(|e| e.latency_samples())
            .sum();
    }

    /// Add an effect to the end of the chain
    pub fn add_effect(&mut self, effect: Box<dyn Effect>) {
        self.effects.push(effect);
        self.update_latency();
    }

    /// Insert an effect at a specific position
    pub fn insert_effect(&mut self, index: usize, effect: Box<dyn Effect>) {
        let index = index.min(self.effects.len());
        self.effects.insert(index, effect);
        self.update_latency();
    }

    /// Remove an effect at a specific position
    pub fn remove_effect(&mut self, index: usize) -> Option<Box<dyn Effect>> {
        if index < self.effects.len() {
            let effect = self.effects.remove(index);
            self.update_latency();
            Some(effect)
        } else {
            None
        }
    }

    /// Get the number of effects in the chain
    pub fn effect_count(&self) -> usize {
        self.effects.len()
    }

    /// Get a reference to an effect by index
    pub fn get_effect(&self, index: usize) -> Option<&Box<dyn Effect>> {
        self.effects.get(index)
    }

    /// Get a mutable reference to an effect by index
    pub fn get_effect_mut(&mut self, index: usize) -> Option<&mut Box<dyn Effect>> {
        self.effects.get_mut(index)
    }

    /// Set bypass state for an effect
    pub fn set_effect_bypass(&mut self, index: usize, bypass: bool) {
        if let Some(effect) = self.effects.get_mut(index) {
            effect.set_bypass(bypass);
            self.update_latency();
        }
    }

    /// Set a knob value and update all mapped parameters
    pub fn set_knob(&mut self, knob_index: usize, value: f32) {
        if knob_index >= CHAIN_KNOB_COUNT {
            return;
        }

        let value = value.clamp(0.0, 1.0);
        self.knobs[knob_index].value = value;

        // Update all mapped parameters
        // Iterate by index to avoid cloning the targets Vec
        let num_targets = self.knobs[knob_index].targets.len();
        for i in 0..num_targets {
            let (effect_idx, param_idx) = self.knobs[knob_index].targets[i];
            if let Some(effect) = self.effects.get_mut(effect_idx) {
                effect.set_param(param_idx, value);
            }
        }
    }

    /// Get the current value of a knob
    pub fn get_knob(&self, knob_index: usize) -> f32 {
        self.knobs.get(knob_index).map(|k| k.value).unwrap_or(0.0)
    }

    /// Get a mutable reference to a knob mapping
    pub fn get_knob_mapping_mut(&mut self, knob_index: usize) -> Option<&mut KnobMapping> {
        self.knobs.get_mut(knob_index)
    }

    /// Set mute state
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Get mute state
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Set solo state
    pub fn set_soloed(&mut self, soloed: bool) {
        self.soloed = soloed;
    }

    /// Get solo state
    pub fn is_soloed(&self) -> bool {
        self.soloed
    }

    /// Reset all effects in the chain
    pub fn reset(&mut self) {
        for effect in &mut self.effects {
            effect.reset();
        }
    }

    /// Clear all effects from the chain
    pub fn clear(&mut self) {
        self.effects.clear();
        self.cached_latency = 0;
    }
}

impl Default for EffectChain {
    fn default() -> Self {
        Self::new()
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
