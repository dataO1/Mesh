//! Multiband Effect Host
//!
//! A container effect that provides:
//! - Pre-FX chain (before multiband split)
//! - Multiband frequency splitting (optional crossover effect)
//! - Per-band effect chains (each band can have multiple effects of ANY type)
//! - Post-FX chain (after bands are summed)
//! - 8 macro knobs with many-to-many parameter routing
//!
//! This is effect-agnostic: it works with any effect implementing the `Effect` trait,
//! including PD effects, CLAP plugins, native Rust effects, or future effect types.
//!
//! # Architecture
//!
//! ```text
//! Input → [Pre-FX Chain] → [Crossover] → Band 1 → [Effect Chain] → ┐
//!                                      → Band 2 → [Effect Chain] → ├→ Sum → [Post-FX Chain] → Output
//!                                      → Band N → [Effect Chain] → ┘
//!
//! 8 Macro Knobs → [Routing Matrix] → Effect Parameters (Pre-FX, Bands, Post-FX)
//! ```
//!
//! # Single-Band Mode
//!
//! By default, MultibandHost starts with 1 band (no frequency splitting).
//! This makes it a simple effect container that can be expanded to multiband
//! when the user adds more bands.
//!
//! # Latency
//!
//! Total latency = crossover_latency + max(band_chain_latencies)

use super::native::LinkwitzRileyCrossover;
use super::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

/// Maximum number of frequency bands
pub const MAX_BANDS: usize = 8;

/// Maximum effects per band
pub const MAX_EFFECTS_PER_BAND: usize = 8;

/// Number of macro knobs available for routing
pub const NUM_MACROS: usize = 4;

/// Error type for multiband operations
#[derive(Debug, Clone)]
pub enum MultibandError {
    /// Band index is out of bounds
    BandIndexOutOfBounds { index: usize, max: usize },
    /// Effect index is out of bounds
    EffectIndexOutOfBounds { band: usize, index: usize, max: usize },
    /// Configuration error
    ConfigError(String),
}

impl std::fmt::Display for MultibandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BandIndexOutOfBounds { index, max } => {
                write!(f, "Band index {} out of bounds (max {})", index, max)
            }
            Self::EffectIndexOutOfBounds { band, index, max } => {
                write!(
                    f,
                    "Effect index {} out of bounds in band {} (max {})",
                    index, band, max
                )
            }
            Self::ConfigError(msg) => write!(f, "Configuration error: {}", msg),
        }
    }
}

impl std::error::Error for MultibandError {}

/// Result type for multiband operations
pub type MultibandResult<T> = Result<T, MultibandError>;

/// A macro knob mapping to a specific effect parameter
#[derive(Debug, Clone)]
pub struct MacroMapping {
    /// Which band (0-7)
    pub band_index: usize,
    /// Which effect in the band's chain (0-7)
    pub effect_index: usize,
    /// Which parameter on the effect (0-7)
    pub param_index: usize,
    /// Scaling factor (0.0 to 1.0 maps to min_value..max_value)
    pub min_value: f32,
    pub max_value: f32,
    /// Optional name for UI display
    pub name: Option<String>,
}

impl MacroMapping {
    /// Create a new 1:1 macro mapping (full range)
    pub fn new(band_index: usize, effect_index: usize, param_index: usize) -> Self {
        Self {
            band_index,
            effect_index,
            param_index,
            min_value: 0.0,
            max_value: 1.0,
            name: None,
        }
    }

    /// Set the output range for this mapping
    pub fn with_range(mut self, min: f32, max: f32) -> Self {
        self.min_value = min;
        self.max_value = max;
        self
    }

    /// Set a display name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Apply the macro value (0.0-1.0) to get the output value
    pub fn apply(&self, macro_value: f32) -> f32 {
        self.min_value + macro_value * (self.max_value - self.min_value)
    }
}

/// State of a single band for UI synchronization
#[derive(Debug, Clone)]
pub struct BandState {
    /// Band gain (linear, 0.0-2.0 typical)
    pub gain: f32,
    /// Whether this band is muted
    pub muted: bool,
    /// Whether this band is soloed
    pub soloed: bool,
    /// Number of effects in this band
    pub effect_count: usize,
}

/// Info about an effect in a band for UI display
#[derive(Debug, Clone)]
pub struct BandEffectInfo {
    /// Effect name
    pub name: String,
    /// Effect category
    pub category: String,
    /// Whether the effect is bypassed
    pub bypassed: bool,
    /// Parameter names
    pub param_names: Vec<String>,
    /// Current parameter values (normalized)
    pub param_values: Vec<f32>,
}

/// A single frequency band with its effect chain
pub struct Band {
    /// Effects in this band's chain (any Effect type)
    effects: Vec<Box<dyn Effect>>,
    /// Band gain (linear)
    gain: f32,
    /// Whether this band is muted
    muted: bool,
    /// Whether this band is soloed
    soloed: bool,
    /// Processing buffer for this band
    buffer: StereoBuffer,
}

impl Band {
    fn new(buffer_size: usize) -> Self {
        Self {
            effects: Vec::new(),
            gain: 1.0,
            muted: false,
            soloed: false,
            buffer: StereoBuffer::silence(buffer_size),
        }
    }

    /// Process audio through this band's effect chain
    fn process(&mut self) {
        if self.muted {
            self.buffer.clear();
            return;
        }

        // Process through each effect in the chain
        for effect in &mut self.effects {
            if !effect.is_bypassed() {
                effect.process(&mut self.buffer);
            }
        }

        // Apply band gain
        if (self.gain - 1.0).abs() > 0.001 {
            let interleaved = self.buffer.as_interleaved_mut();
            for sample in interleaved.iter_mut() {
                *sample *= self.gain;
            }
        }
    }

    /// Get the total latency of this band's effect chain
    fn latency_samples(&self) -> u32 {
        self.effects.iter().map(|e| e.latency_samples()).sum()
    }

    /// Get state for UI
    fn state(&self) -> BandState {
        BandState {
            gain: self.gain,
            muted: self.muted,
            soloed: self.soloed,
            effect_count: self.effects.len(),
        }
    }

    /// Get info about an effect for UI
    fn effect_info(&self, index: usize) -> Option<BandEffectInfo> {
        self.effects.get(index).map(|effect| {
            let info = effect.info();
            let params = effect.get_params();
            BandEffectInfo {
                name: info.name.clone(),
                category: info.category.clone(),
                bypassed: effect.is_bypassed(),
                param_names: info.params.iter().map(|p| p.name.clone()).collect(),
                param_values: params.iter().map(|p| p.normalized).collect(),
            }
        })
    }
}

/// Configuration for a multiband preset
#[derive(Debug, Clone)]
pub struct MultibandConfig {
    /// Number of active bands (1-8)
    pub num_bands: usize,
    /// Crossover frequencies (Hz) - one less than num_bands
    pub crossover_frequencies: Vec<f32>,
}

impl Default for MultibandConfig {
    fn default() -> Self {
        // Default: single band (no crossover)
        Self {
            num_bands: 1,
            crossover_frequencies: Vec::new(),
        }
    }
}

/// Effect chain location identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectLocation {
    /// Pre-FX chain (before multiband split)
    PreFx,
    /// Band effect chain (with band index)
    Band(usize),
    /// Post-FX chain (after band summation)
    PostFx,
}

/// Multiband Effect Host
///
/// A container effect that can hold effects of any type (PD, CLAP, native, etc.)
/// organized into frequency bands with macro knob routing.
///
/// Signal flow: Input → Pre-FX → Bands (parallel) → Sum → Post-FX → Output
///
/// Starts with 1 band by default (simple effect container mode).
/// Add more bands for multiband processing.
pub struct MultibandHost {
    /// Effect base (info, bypass state, macro values)
    base: EffectBase,

    /// Pre-FX chain: effects processed BEFORE multiband split
    pre_fx: Vec<Box<dyn Effect>>,

    /// Native LR24 crossover for frequency band splitting
    crossover: LinkwitzRileyCrossover,

    /// Frequency bands with their effect chains
    bands: Vec<Band>,

    /// Post-FX chain: effects processed AFTER bands are summed
    post_fx: Vec<Box<dyn Effect>>,

    /// Current configuration
    config: MultibandConfig,

    /// Macro knob mappings (each macro can map to multiple parameters)
    macro_mappings: [Vec<MacroMapping>; NUM_MACROS],

    /// Current macro values (0.0-1.0)
    macro_values: [f32; NUM_MACROS],

    /// Macro names for UI
    macro_names: [String; NUM_MACROS],

    /// Whether any band is soloed (for solo logic)
    any_soloed: bool,

    /// Cached total latency
    cached_latency: u32,

    /// Buffer size for new bands
    buffer_size: usize,
}

impl MultibandHost {
    /// Create a new MultibandHost with single-band mode
    pub fn new(buffer_size: usize) -> Self {
        let mut info = EffectInfo::new("Multiband FX", "Container");

        // Add 8 macro parameters
        for i in 0..NUM_MACROS {
            info = info.with_param(
                ParamInfo::new(format!("Macro {}", i + 1), 0.5).with_range(0.0, 1.0),
            );
        }

        let base = EffectBase::new(info);

        // Create single band (default mode)
        let bands = vec![Band::new(buffer_size)];

        Self {
            base,
            pre_fx: Vec::new(),
            crossover: LinkwitzRileyCrossover::new(),
            bands,
            post_fx: Vec::new(),
            config: MultibandConfig::default(),
            macro_mappings: Default::default(),
            macro_values: [0.5; NUM_MACROS],
            macro_names: std::array::from_fn(|i| format!("Macro {}", i + 1)),
            any_soloed: false,
            cached_latency: 0,
            buffer_size,
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Accessor methods for UI synchronization
    // ─────────────────────────────────────────────────────────────────────

    /// Get the number of active bands
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// Get the crossover frequencies
    pub fn crossover_frequencies(&self) -> &[f32] {
        &self.config.crossover_frequencies
    }

    /// Get state for a band
    pub fn band_state(&self, index: usize) -> Option<BandState> {
        self.bands.get(index).map(|b| b.state())
    }

    /// Get info about an effect in a band
    pub fn band_effect_info(&self, band_index: usize, effect_index: usize) -> Option<BandEffectInfo> {
        self.bands.get(band_index).and_then(|b| b.effect_info(effect_index))
    }

    /// Get effect count for a band
    pub fn band_effect_count(&self, band_index: usize) -> usize {
        self.bands.get(band_index).map(|b| b.effects.len()).unwrap_or(0)
    }

    /// Get macro name
    pub fn macro_name(&self, index: usize) -> Option<&str> {
        self.macro_names.get(index).map(|s| s.as_str())
    }

    /// Get macro value
    pub fn macro_value(&self, index: usize) -> f32 {
        self.macro_values.get(index).copied().unwrap_or(0.5)
    }

    /// Get macro mappings
    pub fn macro_mappings(&self, index: usize) -> &[MacroMapping] {
        self.macro_mappings.get(index).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if crossover is enabled (more than 1 band)
    pub fn has_crossover(&self) -> bool {
        self.crossover.is_enabled()
    }

    /// Get the current configuration
    pub fn config(&self) -> &MultibandConfig {
        &self.config
    }

    // ─────────────────────────────────────────────────────────────────────
    // Crossover configuration
    // ─────────────────────────────────────────────────────────────────────

    /// Get crossover frequency for a specific point
    pub fn crossover_frequency(&self, crossover_index: usize) -> f32 {
        self.crossover.frequency(crossover_index)
    }

    /// Reset crossover filter state (call when starting playback)
    pub fn reset_crossover(&mut self) {
        self.crossover.reset();
    }

    /// Add a new band
    ///
    /// Returns the index of the new band, or error if at max bands.
    pub fn add_band(&mut self) -> MultibandResult<usize> {
        if self.bands.len() >= MAX_BANDS {
            return Err(MultibandError::ConfigError(format!(
                "Maximum {} bands allowed",
                MAX_BANDS
            )));
        }

        let new_index = self.bands.len();
        self.bands.push(Band::new(self.buffer_size));
        self.config.num_bands = self.bands.len();

        // Add a default crossover frequency if we now have >1 band
        if self.bands.len() > 1 && self.config.crossover_frequencies.len() < self.bands.len() - 1 {
            // Add crossover at logarithmic midpoint
            let last_freq = self.config.crossover_frequencies.last().copied().unwrap_or(200.0);
            let new_freq = (last_freq * 20000.0_f32).sqrt().min(18000.0);
            self.config.crossover_frequencies.push(new_freq);
        }

        self.update_latency();
        Ok(new_index)
    }

    /// Remove a band
    ///
    /// Cannot remove the last band (minimum 1 band required).
    pub fn remove_band(&mut self, index: usize) -> MultibandResult<()> {
        if self.bands.len() <= 1 {
            return Err(MultibandError::ConfigError(
                "Cannot remove last band".to_string(),
            ));
        }

        if index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index,
                max: self.bands.len(),
            });
        }

        self.bands.remove(index);
        self.config.num_bands = self.bands.len();

        // Remove corresponding crossover frequency
        if !self.config.crossover_frequencies.is_empty() {
            let freq_index = index.min(self.config.crossover_frequencies.len() - 1);
            self.config.crossover_frequencies.remove(freq_index);
        }

        // Update solo state
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
        self.update_latency();
        Ok(())
    }

    /// Set a crossover frequency
    pub fn set_crossover_frequency(&mut self, index: usize, freq_hz: f32) -> MultibandResult<()> {
        if index >= self.config.crossover_frequencies.len() {
            return Err(MultibandError::ConfigError(format!(
                "Crossover index {} out of bounds (have {} frequencies)",
                index,
                self.config.crossover_frequencies.len()
            )));
        }

        let freq = freq_hz.clamp(20.0, 20000.0);
        self.config.crossover_frequencies[index] = freq;
        // Update native crossover filter
        self.crossover.set_frequency(index, freq);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Pre-FX chain management
    // ─────────────────────────────────────────────────────────────────────

    /// Get number of effects in pre-fx chain
    pub fn pre_fx_count(&self) -> usize {
        self.pre_fx.len()
    }

    /// Get info about a pre-fx effect for UI
    pub fn pre_fx_info(&self, index: usize) -> Option<BandEffectInfo> {
        self.pre_fx.get(index).map(|effect| {
            let info = effect.info();
            let params = effect.get_params();
            BandEffectInfo {
                name: info.name.clone(),
                category: info.category.clone(),
                bypassed: effect.is_bypassed(),
                param_names: info.params.iter().map(|p| p.name.clone()).collect(),
                param_values: params.iter().map(|p| p.normalized).collect(),
            }
        })
    }

    /// Add an effect to the pre-fx chain
    pub fn add_pre_fx(&mut self, effect: Box<dyn Effect>) -> MultibandResult<usize> {
        if self.pre_fx.len() >= MAX_EFFECTS_PER_BAND {
            return Err(MultibandError::ConfigError(format!(
                "Pre-FX chain already has maximum {} effects",
                MAX_EFFECTS_PER_BAND
            )));
        }

        let effect_index = self.pre_fx.len();
        self.pre_fx.push(effect);
        self.update_latency();
        Ok(effect_index)
    }

    /// Remove an effect from the pre-fx chain
    pub fn remove_pre_fx(&mut self, index: usize) -> MultibandResult<()> {
        if index >= self.pre_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index,
                max: self.pre_fx.len(),
            });
        }

        self.pre_fx.remove(index);
        self.update_latency();
        Ok(())
    }

    /// Set pre-fx effect bypass
    pub fn set_pre_fx_bypass(&mut self, index: usize, bypass: bool) -> MultibandResult<()> {
        if index >= self.pre_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index,
                max: self.pre_fx.len(),
            });
        }

        self.pre_fx[index].set_bypass(bypass);
        self.update_latency();
        Ok(())
    }

    /// Set pre-fx effect parameter
    pub fn set_pre_fx_param(&mut self, effect_index: usize, param_index: usize, value: f32) -> MultibandResult<()> {
        if effect_index >= self.pre_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index: effect_index,
                max: self.pre_fx.len(),
            });
        }

        self.pre_fx[effect_index].set_param(param_index, value);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Post-FX chain management
    // ─────────────────────────────────────────────────────────────────────

    /// Get number of effects in post-fx chain
    pub fn post_fx_count(&self) -> usize {
        self.post_fx.len()
    }

    /// Get info about a post-fx effect for UI
    pub fn post_fx_info(&self, index: usize) -> Option<BandEffectInfo> {
        self.post_fx.get(index).map(|effect| {
            let info = effect.info();
            let params = effect.get_params();
            BandEffectInfo {
                name: info.name.clone(),
                category: info.category.clone(),
                bypassed: effect.is_bypassed(),
                param_names: info.params.iter().map(|p| p.name.clone()).collect(),
                param_values: params.iter().map(|p| p.normalized).collect(),
            }
        })
    }

    /// Add an effect to the post-fx chain
    pub fn add_post_fx(&mut self, effect: Box<dyn Effect>) -> MultibandResult<usize> {
        if self.post_fx.len() >= MAX_EFFECTS_PER_BAND {
            return Err(MultibandError::ConfigError(format!(
                "Post-FX chain already has maximum {} effects",
                MAX_EFFECTS_PER_BAND
            )));
        }

        let effect_index = self.post_fx.len();
        self.post_fx.push(effect);
        self.update_latency();
        Ok(effect_index)
    }

    /// Remove an effect from the post-fx chain
    pub fn remove_post_fx(&mut self, index: usize) -> MultibandResult<()> {
        if index >= self.post_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index,
                max: self.post_fx.len(),
            });
        }

        self.post_fx.remove(index);
        self.update_latency();
        Ok(())
    }

    /// Set post-fx effect bypass
    pub fn set_post_fx_bypass(&mut self, index: usize, bypass: bool) -> MultibandResult<()> {
        if index >= self.post_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index,
                max: self.post_fx.len(),
            });
        }

        self.post_fx[index].set_bypass(bypass);
        self.update_latency();
        Ok(())
    }

    /// Set post-fx effect parameter
    pub fn set_post_fx_param(&mut self, effect_index: usize, param_index: usize, value: f32) -> MultibandResult<()> {
        if effect_index >= self.post_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index: effect_index,
                max: self.post_fx.len(),
            });
        }

        self.post_fx[effect_index].set_param(param_index, value);
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Band effect chain management
    // ─────────────────────────────────────────────────────────────────────

    /// Add an effect to a band's chain
    pub fn add_effect_to_band(
        &mut self,
        band_index: usize,
        effect: Box<dyn Effect>,
    ) -> MultibandResult<usize> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if band.effects.len() >= MAX_EFFECTS_PER_BAND {
            return Err(MultibandError::ConfigError(format!(
                "Band {} already has maximum {} effects",
                band_index, MAX_EFFECTS_PER_BAND
            )));
        }

        let effect_index = band.effects.len();
        band.effects.push(effect);
        self.update_latency();
        Ok(effect_index)
    }

    /// Remove an effect from a band's chain
    pub fn remove_effect_from_band(
        &mut self,
        band_index: usize,
        effect_index: usize,
    ) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if effect_index >= band.effects.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: band_index,
                index: effect_index,
                max: band.effects.len(),
            });
        }

        band.effects.remove(effect_index);

        // Remove any macro mappings that referenced this effect
        for mappings in &mut self.macro_mappings {
            mappings.retain(|m| !(m.band_index == band_index && m.effect_index == effect_index));
            // Adjust effect indices for effects after the removed one
            for m in mappings.iter_mut() {
                if m.band_index == band_index && m.effect_index > effect_index {
                    m.effect_index -= 1;
                }
            }
        }

        self.update_latency();
        Ok(())
    }

    /// Set effect bypass state
    pub fn set_effect_bypass(
        &mut self,
        band_index: usize,
        effect_index: usize,
        bypass: bool,
    ) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if effect_index >= band.effects.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: band_index,
                index: effect_index,
                max: band.effects.len(),
            });
        }

        band.effects[effect_index].set_bypass(bypass);
        self.update_latency();
        Ok(())
    }

    /// Set effect parameter
    pub fn set_effect_param(
        &mut self,
        band_index: usize,
        effect_index: usize,
        param_index: usize,
        value: f32,
    ) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if effect_index >= band.effects.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: band_index,
                index: effect_index,
                max: band.effects.len(),
            });
        }

        band.effects[effect_index].set_param(param_index, value);
        Ok(())
    }

    /// Set band gain (linear, 0.0-2.0 typical)
    pub fn set_band_gain(&mut self, band_index: usize, gain: f32) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].gain = gain.max(0.0);
        Ok(())
    }

    /// Set band mute state
    pub fn set_band_mute(&mut self, band_index: usize, muted: bool) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].muted = muted;
        Ok(())
    }

    /// Set band solo state
    pub fn set_band_solo(&mut self, band_index: usize, soloed: bool) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].soloed = soloed;
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
        Ok(())
    }

    /// Set macro name
    pub fn set_macro_name(&mut self, index: usize, name: String) -> MultibandResult<()> {
        if index >= NUM_MACROS {
            return Err(MultibandError::ConfigError(format!(
                "Macro index {} out of bounds",
                index
            )));
        }
        self.macro_names[index] = name;
        Ok(())
    }

    /// Add a macro mapping
    pub fn add_macro_mapping(&mut self, macro_index: usize, mapping: MacroMapping) -> MultibandResult<()> {
        if macro_index >= NUM_MACROS {
            return Err(MultibandError::ConfigError(format!(
                "Macro index {} out of bounds (max {})",
                macro_index, NUM_MACROS
            )));
        }

        self.macro_mappings[macro_index].push(mapping);
        Ok(())
    }

    /// Clear all mappings for a macro
    pub fn clear_macro_mappings(&mut self, macro_index: usize) {
        if macro_index < NUM_MACROS {
            self.macro_mappings[macro_index].clear();
        }
    }

    /// Apply macro values to mapped effect parameters
    fn apply_macros(&mut self) {
        for (macro_idx, mappings) in self.macro_mappings.iter().enumerate() {
            let macro_value = self.macro_values[macro_idx];

            for mapping in mappings {
                if mapping.band_index < self.bands.len() {
                    let band = &mut self.bands[mapping.band_index];
                    if mapping.effect_index < band.effects.len() {
                        let effect = &mut band.effects[mapping.effect_index];
                        let param_value = mapping.apply(macro_value);
                        effect.set_param(mapping.param_index, param_value);
                    }
                }
            }
        }
    }

    /// Update cached latency value
    fn update_latency(&mut self) {
        // Pre-FX latency (serial chain)
        let pre_fx_latency: u32 = self.pre_fx.iter().map(|e| e.latency_samples()).sum();

        // Native LR24 crossover has negligible latency (IIR filter)
        let crossover_latency = 0_u32;

        // Band latency (parallel - take max)
        let max_band_latency = self
            .bands
            .iter()
            .map(|b| b.latency_samples())
            .max()
            .unwrap_or(0);

        // Post-FX latency (serial chain)
        let post_fx_latency: u32 = self.post_fx.iter().map(|e| e.latency_samples()).sum();

        self.cached_latency = pre_fx_latency + crossover_latency + max_band_latency + post_fx_latency;
    }
}

impl Effect for MultibandHost {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        // Apply macro values to effect parameters
        self.apply_macros();

        // ═══════════════════════════════════════════════════════════════════
        // STEP 1: Pre-FX chain (before multiband split)
        // ═══════════════════════════════════════════════════════════════════
        for effect in &mut self.pre_fx {
            if !effect.is_bypassed() {
                effect.process(buffer);
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // STEP 2: Multiband processing
        // ═══════════════════════════════════════════════════════════════════
        let band_count = self.bands.len();

        // Single-band mode: no crossover, just process through the band
        if band_count == 1 {
            let band = &mut self.bands[0];
            if !band.muted {
                band.buffer.copy_from(buffer);
                band.process();
                buffer.copy_from(&band.buffer);
            } else {
                buffer.fill_silence();
            }
        } else {
            // Multi-band mode with native LR24 crossover
            // Ensure crossover has correct band count
            self.crossover.set_band_count(band_count);

            // Resize band buffers to match input length (RT-safe: uses pre-allocated capacity)
            // This ensures effects only process the valid sample count, not the full 8192 capacity
            let input_len = buffer.len();
            for band in &mut self.bands {
                band.buffer.set_len_from_capacity(input_len);
            }

            // Step 2a: Split input through crossover into band buffers
            // Process sample-by-sample to split frequencies
            for (i, sample) in buffer.iter().enumerate() {
                let band_samples = self.crossover.process(*sample);

                // Copy each band's frequency content to its buffer
                for (band_idx, band) in self.bands.iter_mut().enumerate() {
                    band.buffer.as_mut_slice()[i] = band_samples[band_idx];
                }
            }

            // Step 2b: Process each band through its effect chain
            for band in &mut self.bands {
                if !band.muted && (!self.any_soloed || band.soloed) {
                    band.process();
                } else {
                    // Muted/not-soloed bands are silent
                    band.buffer.fill_silence();
                }
            }

            // Step 2c: Sum all bands back together
            buffer.fill_silence();
            for (i, sample) in buffer.iter_mut().enumerate() {
                for band in &self.bands {
                    if i < band.buffer.len() {
                        let band_sample = band.buffer[i];
                        sample.left += band_sample.left * band.gain;
                        sample.right += band_sample.right * band.gain;
                    }
                }
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // STEP 3: Post-FX chain (after bands are summed)
        // ═══════════════════════════════════════════════════════════════════
        for effect in &mut self.post_fx {
            if !effect.is_bypassed() {
                effect.process(buffer);
            }
        }
    }

    fn latency_samples(&self) -> u32 {
        self.cached_latency
    }

    fn info(&self) -> &EffectInfo {
        self.base.info()
    }

    fn get_params(&self) -> &[ParamValue] {
        self.base.get_params()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index < NUM_MACROS {
            self.macro_values[index] = value;
            self.base.set_param(index, value);
        }
    }

    fn set_bypass(&mut self, bypass: bool) {
        self.base.set_bypass(bypass);
    }

    fn is_bypassed(&self) -> bool {
        self.base.is_bypassed()
    }

    fn reset(&mut self) {
        // Reset pre-fx chain
        for effect in &mut self.pre_fx {
            effect.reset();
        }

        // Reset all band effects
        for band in &mut self.bands {
            for effect in &mut band.effects {
                effect.reset();
            }
        }

        // Reset crossover
        self.crossover.reset();

        // Reset post-fx chain
        for effect in &mut self.post_fx {
            effect.reset();
        }
    }
}

// Safety: MultibandHost is Send because all fields are Send
// (Band contains Vec<Box<dyn Effect>> which requires Effect: Send)
unsafe impl Send for MultibandHost {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macro_mapping_apply() {
        let mapping = MacroMapping::new(0, 0, 0).with_range(0.2, 0.8);

        assert!((mapping.apply(0.0) - 0.2).abs() < 0.001);
        assert!((mapping.apply(0.5) - 0.5).abs() < 0.001);
        assert!((mapping.apply(1.0) - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_default_config() {
        let config = MultibandConfig::default();
        assert_eq!(config.num_bands, 1);
        assert!(config.crossover_frequencies.is_empty());
    }

    #[test]
    fn test_multiband_creation() {
        let host = MultibandHost::new(256);
        assert_eq!(host.band_count(), 1);
        assert!(!host.has_crossover());
    }

    #[test]
    fn test_add_remove_bands() {
        let mut host = MultibandHost::new(256);

        // Add bands
        assert!(host.add_band().is_ok());
        assert_eq!(host.band_count(), 2);
        assert_eq!(host.crossover_frequencies().len(), 1);

        assert!(host.add_band().is_ok());
        assert_eq!(host.band_count(), 3);
        assert_eq!(host.crossover_frequencies().len(), 2);

        // Remove band
        assert!(host.remove_band(1).is_ok());
        assert_eq!(host.band_count(), 2);

        // Cannot remove last band
        assert!(host.remove_band(0).is_ok());
        assert_eq!(host.band_count(), 1);
        assert!(host.remove_band(0).is_err());
    }

    #[test]
    fn test_crossover_frequency() {
        let mut host = MultibandHost::new(256);
        host.add_band().unwrap();

        assert!(host.set_crossover_frequency(0, 500.0).is_ok());
        assert_eq!(host.crossover_frequencies()[0], 500.0);

        // Clamp to valid range
        assert!(host.set_crossover_frequency(0, 10.0).is_ok());
        assert_eq!(host.crossover_frequencies()[0], 20.0);

        assert!(host.set_crossover_frequency(0, 25000.0).is_ok());
        assert_eq!(host.crossover_frequencies()[0], 20000.0);
    }
}
