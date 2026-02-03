//! Multiband CLAP Host Effect
//!
//! A container effect similar to Kilohearts Multipass that provides:
//! - Multiband frequency splitting via LSP Crossover
//! - Per-band effect chains (each band can have multiple CLAP effects)
//! - 8 macro knobs with many-to-many parameter routing
//!
//! # Architecture
//!
//! ```text
//! Input → [LSP Crossover] → Band 1 → [Effect Chain] → ┐
//!                        → Band 2 → [Effect Chain] → ├→ Mix → Output
//!                        → Band 3 → [Effect Chain] → │
//!                        → Band 4 → [Effect Chain] → ┘
//!
//! 8 Macro Knobs → [Routing Matrix] → Effect Parameters
//! ```
//!
//! # Latency
//!
//! Total latency = crossover_latency + max(band_chain_latencies)

use crate::effect::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::StereoBuffer;

use super::effect::ClapEffect;
use super::error::{ClapError, ClapResult};

/// Maximum number of frequency bands
pub const MAX_BANDS: usize = 8;

/// Maximum effects per band
pub const MAX_EFFECTS_PER_BAND: usize = 4;

/// Number of macro knobs available for routing
pub const NUM_MACROS: usize = 8;

/// Known LSP Crossover plugin IDs
pub const LSP_CROSSOVER_STEREO_ID: &str = "https://lsp-plug.in/plugins/clap/crossover_stereo";
pub const LSP_CROSSOVER_STEREO_X8_ID: &str = "https://lsp-plug.in/plugins/clap/crossover_stereo_x8";

/// A macro knob mapping to a specific effect parameter
#[derive(Debug, Clone)]
pub struct MacroMapping {
    /// Which band (0-7)
    pub band_index: usize,
    /// Which effect in the band's chain (0-3)
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

/// A single frequency band with its effect chain
struct Band {
    /// Effects in this band's chain
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
            // Apply gain to interleaved samples
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
}

/// Configuration for a multiband preset
#[derive(Debug, Clone)]
pub struct MultibandConfig {
    /// Number of active bands (2-8)
    pub num_bands: usize,
    /// Crossover frequencies (Hz) - one less than num_bands
    pub crossover_frequencies: Vec<f32>,
    /// Filter slopes per band (dB/octave: 12, 24, 48)
    pub slopes: Vec<u32>,
}

impl Default for MultibandConfig {
    fn default() -> Self {
        // Default: 4-band configuration
        Self {
            num_bands: 4,
            crossover_frequencies: vec![200.0, 800.0, 3000.0],
            slopes: vec![24, 24, 24, 24],
        }
    }
}

/// Multiband CLAP Host Effect
///
/// A container effect that splits audio into frequency bands and applies
/// separate effect chains to each band, with 8 macro knobs for parameter control.
pub struct MultibandClapHost {
    /// Effect base (info, bypass state, macro values)
    base: EffectBase,

    /// LSP Crossover plugin for band splitting
    crossover: Option<Box<ClapEffect>>,

    /// Frequency bands with their effect chains
    bands: Vec<Band>,

    /// Current configuration
    config: MultibandConfig,

    /// Macro knob mappings (each macro can map to multiple parameters)
    macro_mappings: [Vec<MacroMapping>; NUM_MACROS],

    /// Current macro values (0.0-1.0)
    macro_values: [f32; NUM_MACROS],

    /// Whether any band is soloed (for solo logic)
    any_soloed: bool,

    /// Cached total latency
    cached_latency: u32,

    /// Processing buffers for band mixing
    mix_buffer: StereoBuffer,
}

impl MultibandClapHost {
    /// Create a new MultibandClapHost
    ///
    /// Note: The crossover plugin must be set separately via `set_crossover()`
    /// after creation, as it requires access to the ClapManager.
    pub fn new(buffer_size: usize) -> Self {
        let mut info = EffectInfo::new("Multiband FX", "Multiband");

        // Add 8 macro parameters
        for i in 0..NUM_MACROS {
            info = info.with_param(
                ParamInfo::new(format!("Macro {}", i + 1), 0.5).with_range(0.0, 1.0),
            );
        }

        let base = EffectBase::new(info);

        // Create default 4 bands
        let bands = (0..4).map(|_| Band::new(buffer_size)).collect();

        Self {
            base,
            crossover: None,
            bands,
            config: MultibandConfig::default(),
            macro_mappings: Default::default(),
            macro_values: [0.5; NUM_MACROS],
            any_soloed: false,
            cached_latency: 0,
            mix_buffer: StereoBuffer::silence(buffer_size),
        }
    }

    /// Set the crossover plugin
    pub fn set_crossover(&mut self, crossover: ClapEffect) {
        self.crossover = Some(Box::new(crossover));
        self.update_latency();
    }

    /// Check if the crossover is configured
    pub fn has_crossover(&self) -> bool {
        self.crossover.is_some()
    }

    /// Get the current configuration
    pub fn config(&self) -> &MultibandConfig {
        &self.config
    }

    /// Set the number of active bands (2-8)
    pub fn set_num_bands(&mut self, num_bands: usize) -> ClapResult<()> {
        if num_bands < 2 || num_bands > MAX_BANDS {
            return Err(ClapError::MultibandConfigError(format!(
                "Number of bands must be 2-{}, got {}",
                MAX_BANDS, num_bands
            )));
        }

        // Resize bands array
        let buffer_size = self.bands.first().map(|b| b.buffer.len()).unwrap_or(256);
        while self.bands.len() < num_bands {
            self.bands.push(Band::new(buffer_size));
        }
        self.bands.truncate(num_bands);

        self.config.num_bands = num_bands;

        // Ensure we have the right number of crossover frequencies
        while self.config.crossover_frequencies.len() < num_bands - 1 {
            // Add default frequency based on position
            let last_freq = self.config.crossover_frequencies.last().copied().unwrap_or(200.0);
            self.config.crossover_frequencies.push(last_freq * 2.5);
        }
        self.config.crossover_frequencies.truncate(num_bands - 1);

        // TODO: Update crossover plugin parameters

        self.update_latency();
        Ok(())
    }

    /// Set a crossover frequency
    pub fn set_crossover_frequency(&mut self, index: usize, freq_hz: f32) -> ClapResult<()> {
        if index >= self.config.crossover_frequencies.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index,
                max: self.config.crossover_frequencies.len(),
            });
        }

        self.config.crossover_frequencies[index] = freq_hz.clamp(20.0, 20000.0);
        // TODO: Update crossover plugin parameter

        Ok(())
    }

    /// Add an effect to a band's chain
    pub fn add_effect_to_band(
        &mut self,
        band_index: usize,
        effect: Box<dyn Effect>,
    ) -> ClapResult<()> {
        if band_index >= self.bands.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if band.effects.len() >= MAX_EFFECTS_PER_BAND {
            return Err(ClapError::MultibandConfigError(format!(
                "Band {} already has maximum {} effects",
                band_index, MAX_EFFECTS_PER_BAND
            )));
        }

        band.effects.push(effect);
        self.update_latency();
        Ok(())
    }

    /// Remove an effect from a band's chain
    pub fn remove_effect_from_band(
        &mut self,
        band_index: usize,
        effect_index: usize,
    ) -> ClapResult<()> {
        if band_index >= self.bands.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        let band = &mut self.bands[band_index];
        if effect_index >= band.effects.len() {
            return Err(ClapError::MultibandConfigError(format!(
                "Effect index {} out of bounds for band {}",
                effect_index, band_index
            )));
        }

        band.effects.remove(effect_index);
        self.update_latency();
        Ok(())
    }

    /// Set band gain (linear, 0.0-2.0 typical)
    pub fn set_band_gain(&mut self, band_index: usize, gain: f32) -> ClapResult<()> {
        if band_index >= self.bands.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].gain = gain.max(0.0);
        Ok(())
    }

    /// Set band mute state
    pub fn set_band_mute(&mut self, band_index: usize, muted: bool) -> ClapResult<()> {
        if band_index >= self.bands.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].muted = muted;
        Ok(())
    }

    /// Set band solo state
    pub fn set_band_solo(&mut self, band_index: usize, soloed: bool) -> ClapResult<()> {
        if band_index >= self.bands.len() {
            return Err(ClapError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].soloed = soloed;
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
        Ok(())
    }

    /// Add a macro mapping
    pub fn add_macro_mapping(&mut self, macro_index: usize, mapping: MacroMapping) -> ClapResult<()> {
        if macro_index >= NUM_MACROS {
            return Err(ClapError::MultibandConfigError(format!(
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
        let crossover_latency = self
            .crossover
            .as_ref()
            .map(|c| c.latency_samples())
            .unwrap_or(0);

        let max_band_latency = self
            .bands
            .iter()
            .map(|b| b.latency_samples())
            .max()
            .unwrap_or(0);

        self.cached_latency = crossover_latency + max_band_latency;
    }

    /// Get effect count for a band
    pub fn band_effect_count(&self, band_index: usize) -> usize {
        self.bands.get(band_index).map(|b| b.effects.len()).unwrap_or(0)
    }

    /// Get the number of active bands
    pub fn num_bands(&self) -> usize {
        self.bands.len()
    }
}

impl Effect for MultibandClapHost {
    fn process(&mut self, buffer: &mut StereoBuffer) {
        if self.base.is_bypassed() {
            return;
        }

        // Apply macro values to effect parameters
        self.apply_macros();

        // If no crossover is configured, just pass through
        // (This allows testing band effects without the crossover)
        if self.crossover.is_none() {
            // Process through first band only as passthrough mode
            if let Some(band) = self.bands.first_mut() {
                band.buffer.copy_from(buffer);
                band.process();
                buffer.copy_from(&band.buffer);
            }
            return;
        }

        // TODO: Full multiband processing with crossover
        //
        // The full implementation would:
        // 1. Process input through crossover to split into bands
        // 2. Copy each band's output to the corresponding band buffer
        // 3. Process each band through its effect chain
        // 4. Mix all bands back together
        //
        // This requires understanding how LSP Crossover exposes its
        // multi-output routing via CLAP. For now, we implement a
        // simplified version that processes all effects in series.

        // Simplified: process all band effects in series (not true multiband)
        for band in &mut self.bands {
            if self.any_soloed && !band.soloed {
                continue; // Skip non-soloed bands when something is soloed
            }

            band.buffer.copy_from(buffer);
            band.process();
        }

        // Mix: for now, use only the first non-muted band
        // True multiband would sum all bands
        if let Some(band) = self.bands.iter().find(|b| !b.muted && (!self.any_soloed || b.soloed)) {
            buffer.copy_from(&band.buffer);
        } else {
            buffer.clear();
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
        // Reset all band effects
        for band in &mut self.bands {
            for effect in &mut band.effects {
                effect.reset();
            }
        }

        // Reset crossover
        if let Some(crossover) = &mut self.crossover {
            crossover.reset();
        }
    }
}

// Safety: MultibandClapHost is Send because all fields are Send
unsafe impl Send for MultibandClapHost {}

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
        assert_eq!(config.num_bands, 4);
        assert_eq!(config.crossover_frequencies.len(), 3);
    }

    #[test]
    fn test_multiband_creation() {
        let host = MultibandClapHost::new(256);
        assert_eq!(host.num_bands(), 4);
        assert!(!host.has_crossover());
    }

    #[test]
    fn test_set_num_bands() {
        let mut host = MultibandClapHost::new(256);

        assert!(host.set_num_bands(6).is_ok());
        assert_eq!(host.num_bands(), 6);

        assert!(host.set_num_bands(1).is_err()); // Too few
        assert!(host.set_num_bands(9).is_err()); // Too many
    }
}
