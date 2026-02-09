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

use rayon::prelude::*;

use super::native::LinkwitzRileyCrossover;
use super::{Effect, EffectBase, EffectInfo, ParamInfo, ParamValue};
use crate::types::{StereoBuffer, StereoSample, MAX_LATENCY_SAMPLES};

/// Maximum delay for per-effect dry/wet compensation (individual plugins rarely exceed this)
const MAX_EFFECT_LATENCY: usize = 4096;

/// Ring buffer delay line for internal latency compensation
///
/// Used to delay dry signals so they align with wet (processed) signals
/// at every dry/wet blend point: per-effect, per-chain, inter-band, and global.
struct DelayLine {
    buffer: Vec<StereoSample>,
    write_pos: usize,
    delay_samples: usize,
}

impl DelayLine {
    /// Create a new delay line with the given maximum size
    fn new(max_samples: usize) -> Self {
        Self {
            buffer: vec![StereoSample::silence(); max_samples],
            write_pos: 0,
            delay_samples: 0,
        }
    }

    /// Set the delay amount in samples
    fn set_delay(&mut self, samples: usize) {
        self.delay_samples = samples.min(self.buffer.len().saturating_sub(1));
    }

    /// Get current delay in samples
    fn delay(&self) -> usize {
        self.delay_samples
    }

    /// Process a single sample through the delay line
    #[inline]
    fn process(&mut self, input: StereoSample) -> StereoSample {
        if self.delay_samples == 0 {
            return input;
        }

        // Write input to buffer
        self.buffer[self.write_pos] = input;

        // Calculate read position (behind write position by delay_samples)
        let read_pos = if self.write_pos >= self.delay_samples {
            self.write_pos - self.delay_samples
        } else {
            self.buffer.len() - (self.delay_samples - self.write_pos)
        };

        let output = self.buffer[read_pos];

        // Advance write position
        self.write_pos = (self.write_pos + 1) % self.buffer.len();

        output
    }

    /// Clear the delay line (fill with silence)
    fn clear(&mut self) {
        self.buffer.fill(StereoSample::silence());
        self.write_pos = 0;
    }
}

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

/// Location of an effect in the multiband chain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectLocation {
    /// Pre-FX chain (before multiband split)
    PreFx,
    /// Band effect chain (within a specific band)
    Band(usize),
    /// Post-FX chain (after band summation)
    PostFx,
}

/// A macro knob mapping to a specific effect parameter
#[derive(Debug, Clone)]
pub struct MacroMapping {
    /// Which chain the effect is in
    pub location: EffectLocation,
    /// Which effect in the chain
    pub effect_index: usize,
    /// Which parameter on the effect
    pub param_index: usize,
    /// Scaling factor (0.0 to 1.0 maps to min_value..max_value)
    pub min_value: f32,
    pub max_value: f32,
    /// Optional name for UI display
    pub name: Option<String>,
}

impl MacroMapping {
    /// Create a new 1:1 macro mapping for a band effect (full range)
    pub fn new(band_index: usize, effect_index: usize, param_index: usize) -> Self {
        Self {
            location: EffectLocation::Band(band_index),
            effect_index,
            param_index,
            min_value: 0.0,
            max_value: 1.0,
            name: None,
        }
    }

    /// Create a new mapping for pre-fx effect
    pub fn pre_fx(effect_index: usize, param_index: usize) -> Self {
        Self {
            location: EffectLocation::PreFx,
            effect_index,
            param_index,
            min_value: 0.0,
            max_value: 1.0,
            name: None,
        }
    }

    /// Create a new mapping for post-fx effect
    pub fn post_fx(effect_index: usize, param_index: usize) -> Self {
        Self {
            location: EffectLocation::PostFx,
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
    /// Per-effect dry/wet mix (0.0=dry, 1.0=wet, one per effect)
    effect_dry_wet: Vec<f32>,
    /// Chain dry/wet for entire band (0.0=dry, 1.0=wet)
    chain_dry_wet: f32,
    /// Buffer for dry signal (before processing)
    dry_buffer: StereoBuffer,

    // ── Latency compensation delay lines ──

    /// Per-effect dry/wet delay lines — delays dry signal by each effect's latency
    effect_dry_delay_lines: Vec<DelayLine>,
    /// Chain dry/wet delay line — delays dry buffer by total chain latency
    chain_dry_delay_line: DelayLine,
    /// Band alignment delay line — aligns shorter bands to max band latency
    alignment_delay_line: DelayLine,
}

impl Band {
    fn new(buffer_size: usize) -> Self {
        Self {
            effects: Vec::new(),
            gain: 1.0,
            muted: false,
            soloed: false,
            buffer: StereoBuffer::silence(buffer_size),
            effect_dry_wet: Vec::new(),
            chain_dry_wet: 1.0, // Default: 100% wet (normal processing)
            dry_buffer: StereoBuffer::silence(buffer_size),
            effect_dry_delay_lines: Vec::new(),
            chain_dry_delay_line: DelayLine::new(MAX_LATENCY_SAMPLES),
            alignment_delay_line: DelayLine::new(MAX_LATENCY_SAMPLES),
        }
    }

    /// Process audio through this band's effect chain with dry/wet mixing
    fn process(&mut self) {
        if self.muted {
            self.buffer.clear();
            return;
        }

        // Store dry signal before chain processing (for chain dry/wet)
        if self.chain_dry_wet < 1.0 {
            self.dry_buffer.copy_from(&self.buffer);
        }

        // Process through each effect in the chain with per-effect dry/wet
        // NOTE: We can't use iter_mut() on both effects and delay_lines simultaneously
        // due to borrow checker, so we use index-based iteration.
        for i in 0..self.effects.len() {
            if !self.effects[i].is_bypassed() {
                let mix = self.effect_dry_wet.get(i).copied().unwrap_or(1.0);

                if mix >= 1.0 {
                    // 100% wet - normal processing
                    self.effects[i].process(&mut self.buffer);
                } else if mix <= 0.0 {
                    // 0% wet - skip effect entirely
                    // Still must delay dry signal to maintain time alignment
                    // with downstream effects
                    if let Some(dl) = self.effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in self.buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }
                } else {
                    // Partial mix - store dry, process, then blend
                    let mut wet_buffer = self.buffer.clone();
                    self.effects[i].process(&mut wet_buffer);

                    // Delay dry to match wet's plugin latency
                    if let Some(dl) = self.effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in self.buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }

                    // Blend: output = dry * (1-mix) + wet * mix
                    let dry_gain = 1.0 - mix;
                    for (sample, wet_sample) in self.buffer.iter_mut().zip(wet_buffer.iter()) {
                        sample.left = sample.left * dry_gain + wet_sample.left * mix;
                        sample.right = sample.right * dry_gain + wet_sample.right * mix;
                    }
                }
            }
        }

        // Apply chain dry/wet with latency compensation
        if self.chain_dry_wet < 1.0 {
            let wet_gain = self.chain_dry_wet;
            let dry_gain = 1.0 - wet_gain;

            // Delay dry buffer by total chain latency to align with wet
            if self.chain_dry_delay_line.delay() > 0 {
                for sample in self.dry_buffer.iter_mut() {
                    *sample = self.chain_dry_delay_line.process(*sample);
                }
            }

            for (sample, dry_sample) in self.buffer.iter_mut().zip(self.dry_buffer.iter()) {
                sample.left = dry_sample.left * dry_gain + sample.left * wet_gain;
                sample.right = dry_sample.right * dry_gain + sample.right * wet_gain;
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

    // ─────────────────────────────────────────────────────────────────────
    // Dry/Wet Mix Controls
    // ─────────────────────────────────────────────────────────────────────

    /// Per-effect dry/wet for pre-fx chain (one per effect)
    pre_fx_effect_dry_wet: Vec<f32>,
    /// Pre-fx chain dry/wet (entire chain)
    pre_fx_chain_dry_wet: f32,
    /// Buffer for pre-fx dry signal
    pre_fx_dry_buffer: StereoBuffer,

    /// Per-effect dry/wet for post-fx chain (one per effect)
    post_fx_effect_dry_wet: Vec<f32>,
    /// Post-fx chain dry/wet (entire chain)
    post_fx_chain_dry_wet: f32,
    /// Buffer for post-fx dry signal
    post_fx_dry_buffer: StereoBuffer,

    /// Global dry/wet (entire effect rack)
    global_dry_wet: f32,
    /// Buffer for global dry signal
    global_dry_buffer: StereoBuffer,

    // ── Latency compensation delay lines ──

    /// Pre-FX per-effect dry/wet delay lines
    pre_fx_effect_dry_delay_lines: Vec<DelayLine>,
    /// Pre-FX chain dry/wet delay line
    pre_fx_chain_dry_delay_line: DelayLine,

    /// Post-FX per-effect dry/wet delay lines
    post_fx_effect_dry_delay_lines: Vec<DelayLine>,
    /// Post-FX chain dry/wet delay line
    post_fx_chain_dry_delay_line: DelayLine,

    /// Global dry/wet delay line — delays dry signal by total multiband latency
    global_dry_delay_line: DelayLine,
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
            // Dry/wet controls - default to 1.0 (100% wet = normal processing)
            pre_fx_effect_dry_wet: Vec::new(),
            pre_fx_chain_dry_wet: 1.0,
            pre_fx_dry_buffer: StereoBuffer::silence(buffer_size),
            post_fx_effect_dry_wet: Vec::new(),
            post_fx_chain_dry_wet: 1.0,
            post_fx_dry_buffer: StereoBuffer::silence(buffer_size),
            global_dry_wet: 1.0,
            global_dry_buffer: StereoBuffer::silence(buffer_size),
            // Latency compensation delay lines
            pre_fx_effect_dry_delay_lines: Vec::new(),
            pre_fx_chain_dry_delay_line: DelayLine::new(MAX_LATENCY_SAMPLES),
            post_fx_effect_dry_delay_lines: Vec::new(),
            post_fx_chain_dry_delay_line: DelayLine::new(MAX_LATENCY_SAMPLES),
            global_dry_delay_line: DelayLine::new(MAX_LATENCY_SAMPLES),
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
        self.pre_fx_effect_dry_wet.push(1.0); // Default: 100% wet
        self.pre_fx_effect_dry_delay_lines.push(DelayLine::new(MAX_EFFECT_LATENCY));
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
        if index < self.pre_fx_effect_dry_wet.len() {
            self.pre_fx_effect_dry_wet.remove(index);
        }
        if index < self.pre_fx_effect_dry_delay_lines.len() {
            self.pre_fx_effect_dry_delay_lines.remove(index);
        }
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
        self.post_fx_effect_dry_wet.push(1.0); // Default: 100% wet
        self.post_fx_effect_dry_delay_lines.push(DelayLine::new(MAX_EFFECT_LATENCY));
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
        if index < self.post_fx_effect_dry_wet.len() {
            self.post_fx_effect_dry_wet.remove(index);
        }
        if index < self.post_fx_effect_dry_delay_lines.len() {
            self.post_fx_effect_dry_delay_lines.remove(index);
        }
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
        band.effect_dry_wet.push(1.0); // Default: 100% wet
        band.effect_dry_delay_lines.push(DelayLine::new(MAX_EFFECT_LATENCY));
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
        if effect_index < band.effect_dry_wet.len() {
            band.effect_dry_wet.remove(effect_index);
        }
        if effect_index < band.effect_dry_delay_lines.len() {
            band.effect_dry_delay_lines.remove(effect_index);
        }

        // Remove any macro mappings that referenced this effect
        for mappings in &mut self.macro_mappings {
            mappings.retain(|m| !(m.location == EffectLocation::Band(band_index) && m.effect_index == effect_index));
            // Adjust effect indices for effects after the removed one
            for m in mappings.iter_mut() {
                if m.location == EffectLocation::Band(band_index) && m.effect_index > effect_index {
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

    // ─────────────────────────────────────────────────────────────────────
    // Dry/Wet Mix Control
    // ─────────────────────────────────────────────────────────────────────

    /// Set per-effect dry/wet mix for pre-fx chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_pre_fx_effect_dry_wet(&mut self, effect_index: usize, mix: f32) -> MultibandResult<()> {
        if effect_index >= self.pre_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index: effect_index,
                max: self.pre_fx.len(),
            });
        }

        self.pre_fx_effect_dry_wet[effect_index] = mix.clamp(0.0, 1.0);
        Ok(())
    }

    /// Set per-effect dry/wet mix for a band's effect chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_band_effect_dry_wet(
        &mut self,
        band_index: usize,
        effect_index: usize,
        mix: f32,
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

        band.effect_dry_wet[effect_index] = mix.clamp(0.0, 1.0);
        Ok(())
    }

    /// Set per-effect dry/wet mix for post-fx chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_post_fx_effect_dry_wet(&mut self, effect_index: usize, mix: f32) -> MultibandResult<()> {
        if effect_index >= self.post_fx.len() {
            return Err(MultibandError::EffectIndexOutOfBounds {
                band: 0,
                index: effect_index,
                max: self.post_fx.len(),
            });
        }

        self.post_fx_effect_dry_wet[effect_index] = mix.clamp(0.0, 1.0);
        Ok(())
    }

    /// Set chain dry/wet mix for the entire pre-fx chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_pre_fx_chain_dry_wet(&mut self, mix: f32) {
        self.pre_fx_chain_dry_wet = mix.clamp(0.0, 1.0);
    }

    /// Set chain dry/wet mix for a band's entire effect chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_band_chain_dry_wet(&mut self, band_index: usize, mix: f32) -> MultibandResult<()> {
        if band_index >= self.bands.len() {
            return Err(MultibandError::BandIndexOutOfBounds {
                index: band_index,
                max: self.bands.len(),
            });
        }

        self.bands[band_index].chain_dry_wet = mix.clamp(0.0, 1.0);
        Ok(())
    }

    /// Set chain dry/wet mix for the entire post-fx chain
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_post_fx_chain_dry_wet(&mut self, mix: f32) {
        self.post_fx_chain_dry_wet = mix.clamp(0.0, 1.0);
    }

    /// Set global dry/wet mix for the entire effect rack
    /// mix: 0.0 = fully dry (bypassed), 1.0 = fully wet (normal processing)
    pub fn set_global_dry_wet(&mut self, mix: f32) {
        self.global_dry_wet = mix.clamp(0.0, 1.0);
    }

    /// Get pre-fx effect dry/wet value
    pub fn pre_fx_effect_dry_wet(&self, effect_index: usize) -> Option<f32> {
        self.pre_fx_effect_dry_wet.get(effect_index).copied()
    }

    /// Get band effect dry/wet value
    pub fn band_effect_dry_wet(&self, band_index: usize, effect_index: usize) -> Option<f32> {
        self.bands.get(band_index)
            .and_then(|band| band.effect_dry_wet.get(effect_index).copied())
    }

    /// Get post-fx effect dry/wet value
    pub fn post_fx_effect_dry_wet(&self, effect_index: usize) -> Option<f32> {
        self.post_fx_effect_dry_wet.get(effect_index).copied()
    }

    /// Get pre-fx chain dry/wet value
    pub fn pre_fx_chain_dry_wet(&self) -> f32 {
        self.pre_fx_chain_dry_wet
    }

    /// Get band chain dry/wet value
    pub fn band_chain_dry_wet(&self, band_index: usize) -> Option<f32> {
        self.bands.get(band_index).map(|band| band.chain_dry_wet)
    }

    /// Get post-fx chain dry/wet value
    pub fn post_fx_chain_dry_wet(&self) -> f32 {
        self.post_fx_chain_dry_wet
    }

    /// Get global dry/wet value
    pub fn global_dry_wet(&self) -> f32 {
        self.global_dry_wet
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

        // Log diagnostic info about target effect
        let target_exists = match &mapping.location {
            EffectLocation::PreFx => mapping.effect_index < self.pre_fx.len(),
            EffectLocation::Band(band_idx) => {
                *band_idx < self.bands.len()
                    && mapping.effect_index < self.bands[*band_idx].effects.len()
            }
            EffectLocation::PostFx => mapping.effect_index < self.post_fx.len(),
        };

        log::info!(
            "[MULTIBAND] add_macro_mapping: macro={} -> {:?} effect={} param={} range=[{:.2}, {:.2}] target_exists={}",
            macro_index, mapping.location, mapping.effect_index, mapping.param_index,
            mapping.min_value, mapping.max_value, target_exists
        );

        if !target_exists {
            log::warn!(
                "[MULTIBAND] Target effect does not exist! bands={} pre_fx={} post_fx={}",
                self.bands.len(), self.pre_fx.len(), self.post_fx.len()
            );
            // Also log band effect counts
            for (i, band) in self.bands.iter().enumerate() {
                log::warn!("[MULTIBAND]   band[{}] has {} effects", i, band.effects.len());
            }
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

    /// Counter for occasional logging (to avoid flooding)
    #[cfg(debug_assertions)]
    fn should_log_apply_macros() -> bool {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        count % 10000 == 0 // Log every 10000 calls
    }

    /// Apply macro values to mapped effect parameters
    fn apply_macros(&mut self) {
        #[cfg(debug_assertions)]
        let should_log = Self::should_log_apply_macros();

        for (macro_idx, mappings) in self.macro_mappings.iter().enumerate() {
            let macro_value = self.macro_values[macro_idx];

            #[cfg(debug_assertions)]
            if should_log && !mappings.is_empty() {
                log::debug!(
                    "[MULTIBAND_APPLY] macro[{}]={:.3} has {} mappings",
                    macro_idx, macro_value, mappings.len()
                );
            }

            for mapping in mappings {
                let param_value = mapping.apply(macro_value);

                match mapping.location {
                    EffectLocation::PreFx => {
                        if mapping.effect_index < self.pre_fx.len() {
                            self.pre_fx[mapping.effect_index].set_param(mapping.param_index, param_value);
                        }
                    }
                    EffectLocation::Band(band_index) => {
                        if band_index < self.bands.len() {
                            let band = &mut self.bands[band_index];
                            if mapping.effect_index < band.effects.len() {
                                band.effects[mapping.effect_index].set_param(mapping.param_index, param_value);
                            } else {
                                // Log once per frame is too noisy - use trace level
                                log::trace!(
                                    "[MULTIBAND_MACRO] Effect {} not found in band {} (have {} effects)",
                                    mapping.effect_index, band_index, band.effects.len()
                                );
                            }
                        } else {
                            log::trace!(
                                "[MULTIBAND_MACRO] Band {} not found (have {} bands)",
                                band_index, self.bands.len()
                            );
                        }
                    }
                    EffectLocation::PostFx => {
                        if mapping.effect_index < self.post_fx.len() {
                            self.post_fx[mapping.effect_index].set_param(mapping.param_index, param_value);
                        }
                    }
                }
            }
        }
    }

    /// Update cached latency value and configure internal delay lines
    ///
    /// Called whenever effects are added, removed, or bypassed.
    /// Sets delay amounts on all compensation delay lines so that:
    /// - Per-effect dry/wet blends are phase-aligned
    /// - Per-chain dry/wet blends are phase-aligned
    /// - Bands with different latencies are time-aligned before summing
    /// - Global dry/wet blend is phase-aligned
    fn update_latency(&mut self) {
        // ── Pre-FX latency (serial chain) ──
        let pre_fx_latency: u32 = self.pre_fx.iter().map(|e| e.latency_samples()).sum();

        // Update pre-FX per-effect delay lines
        for (i, effect) in self.pre_fx.iter().enumerate() {
            if let Some(dl) = self.pre_fx_effect_dry_delay_lines.get_mut(i) {
                dl.set_delay(effect.latency_samples() as usize);
            }
        }
        self.pre_fx_chain_dry_delay_line.set_delay(pre_fx_latency as usize);

        // ── Band latencies + alignment ──
        let band_latencies: Vec<u32> = self.bands.iter().map(|b| b.latency_samples()).collect();
        let max_band_latency = band_latencies.iter().copied().max().unwrap_or(0);

        for (i, band) in self.bands.iter_mut().enumerate() {
            let band_lat = band_latencies[i];

            // Alignment: shorter bands get delayed to match the longest
            band.alignment_delay_line.set_delay((max_band_latency - band_lat) as usize);

            // Per-effect delay lines within each band
            for (j, effect) in band.effects.iter().enumerate() {
                if let Some(dl) = band.effect_dry_delay_lines.get_mut(j) {
                    dl.set_delay(effect.latency_samples() as usize);
                }
            }

            // Chain dry/wet delay = total band chain latency
            band.chain_dry_delay_line.set_delay(band_lat as usize);
        }

        // ── Post-FX latency (serial chain) ──
        let post_fx_latency: u32 = self.post_fx.iter().map(|e| e.latency_samples()).sum();

        // Update post-FX per-effect delay lines
        for (i, effect) in self.post_fx.iter().enumerate() {
            if let Some(dl) = self.post_fx_effect_dry_delay_lines.get_mut(i) {
                dl.set_delay(effect.latency_samples() as usize);
            }
        }
        self.post_fx_chain_dry_delay_line.set_delay(post_fx_latency as usize);

        // ── Total + global ──
        self.cached_latency = pre_fx_latency + max_band_latency + post_fx_latency;
        self.global_dry_delay_line.set_delay(self.cached_latency as usize);
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
        // STEP 0: Store global dry signal (before any processing)
        // ═══════════════════════════════════════════════════════════════════
        if self.global_dry_wet < 1.0 {
            self.global_dry_buffer.copy_from(buffer);
        }

        // ═══════════════════════════════════════════════════════════════════
        // STEP 1: Pre-FX chain (before multiband split) with dry/wet
        // ═══════════════════════════════════════════════════════════════════
        // Store pre-fx dry signal
        if self.pre_fx_chain_dry_wet < 1.0 {
            self.pre_fx_dry_buffer.copy_from(buffer);
        }

        // Process each pre-fx effect with per-effect dry/wet + latency compensation
        for i in 0..self.pre_fx.len() {
            if !self.pre_fx[i].is_bypassed() {
                let mix = self.pre_fx_effect_dry_wet.get(i).copied().unwrap_or(1.0);

                if mix >= 1.0 {
                    self.pre_fx[i].process(buffer);
                } else if mix <= 0.0 {
                    // 0% wet - still delay dry for time alignment
                    if let Some(dl) = self.pre_fx_effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }
                } else {
                    let mut wet_buffer = buffer.clone();
                    self.pre_fx[i].process(&mut wet_buffer);

                    // Delay dry to match wet's plugin latency
                    if let Some(dl) = self.pre_fx_effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }

                    let dry_gain = 1.0 - mix;
                    for (sample, wet_sample) in buffer.iter_mut().zip(wet_buffer.iter()) {
                        sample.left = sample.left * dry_gain + wet_sample.left * mix;
                        sample.right = sample.right * dry_gain + wet_sample.right * mix;
                    }
                }
            }
        }

        // Apply pre-fx chain dry/wet with latency compensation
        if self.pre_fx_chain_dry_wet < 1.0 {
            let wet_gain = self.pre_fx_chain_dry_wet;
            let dry_gain = 1.0 - wet_gain;

            // Delay dry buffer by total pre-fx chain latency
            if self.pre_fx_chain_dry_delay_line.delay() > 0 {
                for sample in self.pre_fx_dry_buffer.iter_mut() {
                    *sample = self.pre_fx_chain_dry_delay_line.process(*sample);
                }
            }

            for (sample, dry_sample) in buffer.iter_mut().zip(self.pre_fx_dry_buffer.iter()) {
                sample.left = dry_sample.left * dry_gain + sample.left * wet_gain;
                sample.right = dry_sample.right * dry_gain + sample.right * wet_gain;
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

            // Step 2b: Process each band through its effect chain (parallel)
            let any_soloed = self.any_soloed;
            self.bands.par_iter_mut().for_each(|band| {
                if !band.muted && (!any_soloed || band.soloed) {
                    band.process();
                } else {
                    // Muted/not-soloed bands are silent
                    band.buffer.fill_silence();
                }

                // Step 2b½: Align this band to max latency
                // Shorter bands are delayed so all band outputs are time-aligned before summing
                if band.alignment_delay_line.delay() > 0 {
                    for sample in band.buffer.iter_mut() {
                        *sample = band.alignment_delay_line.process(*sample);
                    }
                }
            });

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
        // STEP 3: Post-FX chain (after bands are summed) with dry/wet
        // ═══════════════════════════════════════════════════════════════════
        // Store post-fx dry signal
        if self.post_fx_chain_dry_wet < 1.0 {
            self.post_fx_dry_buffer.copy_from(buffer);
        }

        // Process each post-fx effect with per-effect dry/wet + latency compensation
        for i in 0..self.post_fx.len() {
            if !self.post_fx[i].is_bypassed() {
                let mix = self.post_fx_effect_dry_wet.get(i).copied().unwrap_or(1.0);

                if mix >= 1.0 {
                    self.post_fx[i].process(buffer);
                } else if mix <= 0.0 {
                    // 0% wet - still delay dry for time alignment
                    if let Some(dl) = self.post_fx_effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }
                } else {
                    let mut wet_buffer = buffer.clone();
                    self.post_fx[i].process(&mut wet_buffer);

                    // Delay dry to match wet's plugin latency
                    if let Some(dl) = self.post_fx_effect_dry_delay_lines.get_mut(i) {
                        if dl.delay() > 0 {
                            for sample in buffer.iter_mut() {
                                *sample = dl.process(*sample);
                            }
                        }
                    }

                    let dry_gain = 1.0 - mix;
                    for (sample, wet_sample) in buffer.iter_mut().zip(wet_buffer.iter()) {
                        sample.left = sample.left * dry_gain + wet_sample.left * mix;
                        sample.right = sample.right * dry_gain + wet_sample.right * mix;
                    }
                }
            }
        }

        // Apply post-fx chain dry/wet with latency compensation
        if self.post_fx_chain_dry_wet < 1.0 {
            let wet_gain = self.post_fx_chain_dry_wet;
            let dry_gain = 1.0 - wet_gain;

            // Delay dry buffer by total post-fx chain latency
            if self.post_fx_chain_dry_delay_line.delay() > 0 {
                for sample in self.post_fx_dry_buffer.iter_mut() {
                    *sample = self.post_fx_chain_dry_delay_line.process(*sample);
                }
            }

            for (sample, dry_sample) in buffer.iter_mut().zip(self.post_fx_dry_buffer.iter()) {
                sample.left = dry_sample.left * dry_gain + sample.left * wet_gain;
                sample.right = dry_sample.right * dry_gain + sample.right * wet_gain;
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // STEP 4: Apply global dry/wet with latency compensation
        // ═══════════════════════════════════════════════════════════════════
        if self.global_dry_wet < 1.0 {
            let wet_gain = self.global_dry_wet;
            let dry_gain = 1.0 - wet_gain;

            // Delay global dry buffer by total multiband latency
            if self.global_dry_delay_line.delay() > 0 {
                for sample in self.global_dry_buffer.iter_mut() {
                    *sample = self.global_dry_delay_line.process(*sample);
                }
            }

            for (sample, dry_sample) in buffer.iter_mut().zip(self.global_dry_buffer.iter()) {
                sample.left = dry_sample.left * dry_gain + sample.left * wet_gain;
                sample.right = dry_sample.right * dry_gain + sample.right * wet_gain;
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
        for dl in &mut self.pre_fx_effect_dry_delay_lines {
            dl.clear();
        }
        self.pre_fx_chain_dry_delay_line.clear();

        // Reset all band effects and delay lines
        for band in &mut self.bands {
            for effect in &mut band.effects {
                effect.reset();
            }
            for dl in &mut band.effect_dry_delay_lines {
                dl.clear();
            }
            band.chain_dry_delay_line.clear();
            band.alignment_delay_line.clear();
        }

        // Reset crossover
        self.crossover.reset();

        // Reset post-fx chain
        for effect in &mut self.post_fx {
            effect.reset();
        }
        for dl in &mut self.post_fx_effect_dry_delay_lines {
            dl.clear();
        }
        self.post_fx_chain_dry_delay_line.clear();

        self.global_dry_delay_line.clear();
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
