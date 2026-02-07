//! State structures for the multiband editor widget

use mesh_core::effect::{BandEffectInfo, BandState, ParamInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::knob::Knob;

/// Maximum number of UI knobs per effect (hardware constraint)
pub const MAX_UI_KNOBS: usize = 8;

/// Effect source type for display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectSourceType {
    /// Pure Data effect
    Pd,
    /// CLAP plugin
    Clap,
    /// Native Rust effect
    Native,
}

impl std::fmt::Display for EffectSourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pd => write!(f, "PD"),
            Self::Clap => write!(f, "CLAP"),
            Self::Native => write!(f, "Native"),
        }
    }
}

/// A mapping from a macro knob to an effect parameter
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamMacroMapping {
    /// Which macro (0-7) controls this param, None if unmapped
    pub macro_index: Option<usize>,
    /// Min value when macro is at 0
    pub min_value: f32,
    /// Max value when macro is at 1
    pub max_value: f32,
}

impl Default for ParamMacroMapping {
    fn default() -> Self {
        Self {
            macro_index: None,
            min_value: 0.0,
            max_value: 1.0,
        }
    }
}

/// Information about an available parameter (mirrors ParamInfo for serialization)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableParam {
    /// Parameter name
    pub name: String,
    /// Minimum value
    pub min: f32,
    /// Maximum value
    pub max: f32,
    /// Default value (normalized 0-1)
    pub default: f32,
    /// Unit label (e.g., "ms", "dB", "%")
    pub unit: String,
}

impl AvailableParam {
    /// Create from mesh_core ParamInfo
    pub fn from_param_info(info: &ParamInfo) -> Self {
        Self {
            name: info.name.clone(),
            min: info.min,
            max: info.max,
            default: info.default,
            unit: info.unit.clone(),
        }
    }
}

/// Assignment of a UI knob to an effect parameter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnobAssignment {
    /// Parameter index in effect's available_params list (None = unassigned)
    pub param_index: Option<usize>,
    /// Current value (normalized 0.0-1.0)
    pub value: f32,
    /// Macro mapping for this knob (if any)
    pub macro_mapping: Option<ParamMacroMapping>,
}

impl Default for KnobAssignment {
    fn default() -> Self {
        Self {
            param_index: None,
            value: 0.5,
            macro_mapping: None,
        }
    }
}

impl KnobAssignment {
    /// Create a new assignment to a specific parameter
    pub fn assigned(param_index: usize, value: f32) -> Self {
        Self {
            param_index: Some(param_index),
            value,
            macro_mapping: None,
        }
    }
}

/// UI state for a single effect in a band (serializable for presets)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectUiState {
    /// Effect identifier (for recreation from preset)
    pub id: String,
    /// Effect display name
    pub name: String,
    /// Effect category
    pub category: String,
    /// Effect source type
    pub source: EffectSourceType,
    /// Whether the effect is bypassed
    pub bypassed: bool,
    /// Whether the plugin GUI window is open (CLAP only, not serialized)
    #[serde(skip)]
    pub gui_open: bool,

    /// All available parameters from the effect (can be 100+ for CLAP plugins)
    pub available_params: Vec<AvailableParam>,

    /// UI knob assignments (exactly 8 knobs)
    /// Each knob can be assigned to any parameter index, or be unassigned
    pub knob_assignments: [KnobAssignment; MAX_UI_KNOBS],

    // Legacy fields kept for backwards compatibility during migration
    // TODO: Remove these after migration is complete
    /// Parameter names (up to 8) - LEGACY, use available_params instead
    #[serde(default)]
    pub param_names: Vec<String>,
    /// Current parameter values (normalized 0.0-1.0) - LEGACY, use knob_assignments instead
    #[serde(default)]
    pub param_values: Vec<f32>,
    /// Macro mappings for each parameter - LEGACY, use knob_assignments instead
    #[serde(default)]
    pub param_mappings: Vec<ParamMacroMapping>,
}

impl EffectUiState {
    /// Create from backend BandEffectInfo
    pub fn from_backend(id: String, source: EffectSourceType, info: &BandEffectInfo) -> Self {
        let param_count = info.param_values.len().min(MAX_UI_KNOBS);

        // Convert backend param info to AvailableParam
        let available_params: Vec<AvailableParam> = info
            .param_names
            .iter()
            .enumerate()
            .map(|(i, name)| AvailableParam {
                name: name.clone(),
                min: 0.0,
                max: 1.0,
                default: info.param_values.get(i).copied().unwrap_or(0.5),
                unit: String::new(),
            })
            .collect();

        // Create default knob assignments mapping first N params to first N knobs
        let mut knob_assignments: [KnobAssignment; MAX_UI_KNOBS] = Default::default();
        for (i, assignment) in knob_assignments.iter_mut().enumerate().take(param_count) {
            assignment.param_index = Some(i);
            assignment.value = info.param_values.get(i).copied().unwrap_or(0.5);
        }

        Self {
            id,
            name: info.name.clone(),
            category: info.category.clone(),
            source,
            bypassed: info.bypassed,
            gui_open: false,
            available_params,
            knob_assignments,
            // Legacy fields for backwards compat
            param_names: info.param_names.clone(),
            param_values: info.param_values.clone(),
            param_mappings: vec![ParamMacroMapping::default(); param_count],
        }
    }

    /// Create a new effect UI state with all available parameters
    pub fn new_with_params(
        id: String,
        name: String,
        category: String,
        source: EffectSourceType,
        available_params: Vec<AvailableParam>,
    ) -> Self {
        let param_count = available_params.len().min(MAX_UI_KNOBS);

        // Auto-assign first N params to first N knobs
        let mut knob_assignments: [KnobAssignment; MAX_UI_KNOBS] = Default::default();
        for (i, assignment) in knob_assignments.iter_mut().enumerate().take(param_count) {
            assignment.param_index = Some(i);
            assignment.value = available_params.get(i).map(|p| p.default).unwrap_or(0.5);
        }

        // Legacy fields
        let param_names: Vec<String> = available_params.iter().take(MAX_UI_KNOBS).map(|p| p.name.clone()).collect();
        let param_values: Vec<f32> = available_params.iter().take(MAX_UI_KNOBS).map(|p| p.default).collect();

        Self {
            id,
            name,
            category,
            source,
            bypassed: false,
            gui_open: false,
            available_params,
            knob_assignments,
            param_names,
            param_values,
            param_mappings: vec![ParamMacroMapping::default(); param_count],
        }
    }

    /// Get the parameter name for a knob (or "[assign]" if unassigned)
    pub fn knob_param_name(&self, knob_idx: usize) -> &str {
        if let Some(assignment) = self.knob_assignments.get(knob_idx) {
            if let Some(param_idx) = assignment.param_index {
                if let Some(param) = self.available_params.get(param_idx) {
                    return &param.name;
                }
            }
        }
        "[assign]"
    }

    /// Get a short name for compact display (max 10 chars)
    pub fn short_name(&self) -> &str {
        if self.name.len() <= 10 {
            &self.name
        } else {
            &self.name[..10]
        }
    }
}

/// UI state for a single frequency band
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandUiState {
    /// Band index (0-7)
    pub index: usize,
    /// Low frequency bound (Hz)
    pub freq_low: f32,
    /// High frequency bound (Hz)
    pub freq_high: f32,
    /// Band gain (linear, 0.0-2.0)
    pub gain: f32,
    /// Whether this band is muted
    pub muted: bool,
    /// Whether this band is soloed
    pub soloed: bool,
    /// Effects in this band's chain
    pub effects: Vec<EffectUiState>,
}

impl BandUiState {
    /// Create a new band UI state
    pub fn new(index: usize, freq_low: f32, freq_high: f32) -> Self {
        Self {
            index,
            freq_low,
            freq_high,
            gain: 1.0,
            muted: false,
            soloed: false,
            effects: Vec::new(),
        }
    }

    /// Update from backend BandState
    pub fn update_from_backend(&mut self, state: &BandState) {
        self.gain = state.gain;
        self.muted = state.muted;
        self.soloed = state.soloed;
    }

    /// Get the band name based on frequency range
    pub fn name(&self) -> &'static str {
        super::default_band_name(self.freq_low, self.freq_high)
    }

    /// Get frequency range as formatted string
    pub fn freq_range_str(&self) -> String {
        format!(
            "{} - {}",
            super::format_freq(self.freq_low),
            super::format_freq(self.freq_high)
        )
    }
}

/// State for a macro (serializable metadata, knob widget is separate)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroUiState {
    /// Macro index (0-7)
    pub index: usize,
    /// Display name
    pub name: String,
    /// Number of mappings to effect parameters
    pub mapping_count: usize,
}

impl MacroUiState {
    /// Create a new macro UI state with default name
    pub fn new(index: usize) -> Self {
        Self {
            index,
            name: format!("Macro {}", index + 1),
            mapping_count: 0,
        }
    }
}

/// Effect chain location for UI interaction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectChainLocation {
    /// Pre-FX chain (before multiband split)
    PreFx,
    /// Band effect chain
    Band(usize),
    /// Post-FX chain (after band summation)
    PostFx,
}

/// Key for effect parameter knobs
pub type EffectKnobKey = (EffectChainLocation, usize, usize); // (location, effect_idx, param_idx)

/// Complete state for the multiband editor widget
#[derive(Debug, Clone)]
pub struct MultibandEditorState {
    /// Whether the editor modal is open
    pub is_open: bool,

    /// Target deck index (0-3)
    pub deck: usize,

    /// Target stem index (0-3)
    pub stem: usize,

    /// Stem name for display
    pub stem_name: String,

    /// Pre-FX chain (effects before multiband split)
    pub pre_fx: Vec<EffectUiState>,

    /// Crossover frequencies (N-1 for N bands)
    pub crossover_freqs: Vec<f32>,

    /// Which crossover divider is being dragged (index)
    pub dragging_crossover: Option<usize>,

    /// Which macro is being dragged for mapping (index)
    pub dragging_macro: Option<usize>,

    /// Band states
    pub bands: Vec<BandUiState>,

    /// Post-FX chain (effects after bands are summed)
    pub post_fx: Vec<EffectUiState>,

    /// Currently selected effect for parameter focus
    /// (location, effect_index)
    pub selected_effect: Option<(EffectChainLocation, usize)>,

    /// Macro metadata (names, mapping counts)
    pub macros: Vec<MacroUiState>,

    /// Macro knob widgets (stateful, with stable IDs)
    pub macro_knobs: Vec<Knob>,

    /// Effect parameter knob widgets, keyed by (location, effect_idx, knob_idx)
    pub effect_knobs: HashMap<EffectKnobKey, Knob>,

    /// Whether the preset browser is open (for loading)
    pub preset_browser_open: bool,

    /// Whether the save dialog is open
    pub save_dialog_open: bool,

    /// Preset name input for save dialog
    pub preset_name_input: String,

    /// Available preset names
    pub available_presets: Vec<String>,

    /// Whether any band is soloed (for solo logic display)
    pub any_soloed: bool,

    // ─────────────────────────────────────────────────────────────────────
    // Parameter picker state
    // ─────────────────────────────────────────────────────────────────────

    /// Open param picker: (location, effect_idx, knob_idx)
    pub param_picker_open: Option<(EffectChainLocation, usize, usize)>,

    /// Search filter for param picker
    pub param_picker_search: String,

    // ─────────────────────────────────────────────────────────────────────
    // Global knob drag tracking (for mouse capture)
    // ─────────────────────────────────────────────────────────────────────

    /// Currently dragging effect knob: (location, effect_idx, param_idx)
    /// When set, global mouse events should be routed to this knob
    pub dragging_effect_knob: Option<(EffectChainLocation, usize, usize)>,

    /// Currently dragging macro knob index (0-7)
    pub dragging_macro_knob: Option<usize>,

    // ─────────────────────────────────────────────────────────────────────
    // CLAP Plugin GUI Learning Mode
    // ─────────────────────────────────────────────────────────────────────

    /// Knob currently in learning mode: (location, effect_idx, knob_idx)
    /// When set, the next parameter change from a CLAP plugin GUI will be
    /// assigned to this knob.
    pub learning_knob: Option<(EffectChainLocation, usize, usize)>,
}

impl Default for MultibandEditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl MultibandEditorState {
    /// Create a new multiband editor state (closed, single band)
    pub fn new() -> Self {
        Self {
            is_open: false,
            deck: 0,
            stem: 0,
            stem_name: "Vocals".to_string(),
            pre_fx: Vec::new(),
            crossover_freqs: Vec::new(),
            dragging_crossover: None,
            dragging_macro: None,
            bands: vec![BandUiState::new(0, super::FREQ_MIN, super::FREQ_MAX)],
            post_fx: Vec::new(),
            selected_effect: None,
            macros: (0..super::NUM_MACROS).map(MacroUiState::new).collect(),
            macro_knobs: (0..super::NUM_MACROS).map(|_| Knob::new(64.0)).collect(),
            effect_knobs: HashMap::new(),
            preset_browser_open: false,
            save_dialog_open: false,
            preset_name_input: String::new(),
            available_presets: Vec::new(),
            any_soloed: false,
            param_picker_open: None,
            param_picker_search: String::new(),
            dragging_effect_knob: None,
            dragging_macro_knob: None,
            learning_knob: None,
        }
    }

    /// Check if any knob is currently being dragged (for mouse capture subscription)
    pub fn is_any_knob_dragging(&self) -> bool {
        self.dragging_effect_knob.is_some() || self.dragging_macro_knob.is_some()
    }

    // ─────────────────────────────────────────────────────────────────────
    // CLAP Plugin GUI Learning Mode
    // ─────────────────────────────────────────────────────────────────────

    /// Start learning mode for a knob - the next CLAP plugin GUI interaction
    /// will assign the changed parameter to this knob
    pub fn start_learning(&mut self, location: EffectChainLocation, effect_idx: usize, knob_idx: usize) {
        self.learning_knob = Some((location, effect_idx, knob_idx));
    }

    /// Cancel learning mode
    pub fn cancel_learning(&mut self) {
        self.learning_knob = None;
    }

    /// Check if a specific knob is in learning mode
    pub fn is_knob_learning(&self, location: EffectChainLocation, effect_idx: usize, knob_idx: usize) -> bool {
        self.learning_knob == Some((location, effect_idx, knob_idx))
    }

    /// Check if any knob is in learning mode
    pub fn is_learning(&self) -> bool {
        self.learning_knob.is_some()
    }

    /// Get the current learning target (if any)
    pub fn learning_target(&self) -> Option<(EffectChainLocation, usize, usize)> {
        self.learning_knob
    }

    /// Open the editor for a specific deck and stem
    pub fn open(&mut self, deck: usize, stem: usize, stem_name: &str) {
        self.is_open = true;
        self.deck = deck;
        self.stem = stem;
        self.stem_name = stem_name.to_string();
        self.selected_effect = None;
        self.preset_browser_open = false;
    }

    /// Close the editor
    pub fn close(&mut self) {
        self.is_open = false;
        self.dragging_crossover = None;
        self.dragging_macro = None;
    }

    /// Get the number of bands
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// Get or create an effect parameter knob
    pub fn get_effect_knob(&mut self, location: EffectChainLocation, effect_idx: usize, param_idx: usize) -> &mut Knob {
        let key = (location, effect_idx, param_idx);
        // Get initial value before borrowing effect_knobs
        let initial_value = if !self.effect_knobs.contains_key(&key) {
            Some(self.get_effect_param_value(location, effect_idx, param_idx))
        } else {
            None
        };

        self.effect_knobs.entry(key).or_insert_with(|| {
            let mut knob = Knob::new(40.0); // Size for effect params
            if let Some(value) = initial_value {
                knob.set_value(value);
            }
            knob
        })
    }

    /// Get effect parameter value from state
    fn get_effect_param_value(&self, location: EffectChainLocation, effect_idx: usize, param_idx: usize) -> f32 {
        match location {
            EffectChainLocation::PreFx => {
                self.pre_fx.get(effect_idx)
                    .and_then(|e| e.param_values.get(param_idx).copied())
                    .unwrap_or(0.5)
            }
            EffectChainLocation::Band(band_idx) => {
                self.bands.get(band_idx)
                    .and_then(|b| b.effects.get(effect_idx))
                    .and_then(|e| e.param_values.get(param_idx).copied())
                    .unwrap_or(0.5)
            }
            EffectChainLocation::PostFx => {
                self.post_fx.get(effect_idx)
                    .and_then(|e| e.param_values.get(param_idx).copied())
                    .unwrap_or(0.5)
            }
        }
    }

    /// Set effect parameter value in state and sync to knob
    pub fn set_effect_param_value(&mut self, location: EffectChainLocation, effect_idx: usize, param_idx: usize, value: f32) {
        // Update effect state
        match location {
            EffectChainLocation::PreFx => {
                if let Some(effect) = self.pre_fx.get_mut(effect_idx) {
                    if let Some(v) = effect.param_values.get_mut(param_idx) {
                        *v = value;
                    }
                }
            }
            EffectChainLocation::Band(band_idx) => {
                if let Some(band) = self.bands.get_mut(band_idx) {
                    if let Some(effect) = band.effects.get_mut(effect_idx) {
                        if let Some(v) = effect.param_values.get_mut(param_idx) {
                            *v = value;
                        }
                    }
                }
            }
            EffectChainLocation::PostFx => {
                if let Some(effect) = self.post_fx.get_mut(effect_idx) {
                    if let Some(v) = effect.param_values.get_mut(param_idx) {
                        *v = value;
                    }
                }
            }
        }

        // Sync knob if it exists
        let key = (location, effect_idx, param_idx);
        if let Some(knob) = self.effect_knobs.get_mut(&key) {
            knob.set_value(value);
        }
    }

    /// Remove knobs for an effect that was removed
    pub fn remove_effect_knobs(&mut self, location: EffectChainLocation, effect_idx: usize) {
        // Remove all knobs for this effect
        self.effect_knobs.retain(|&(loc, eff, _), _| {
            !(loc == location && eff == effect_idx)
        });
        // Shift indices for effects after the removed one
        let keys_to_update: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, eff, _)| loc == location && eff > effect_idx)
            .copied()
            .collect();
        for key in keys_to_update {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                let new_key = (key.0, key.1 - 1, key.2);
                self.effect_knobs.insert(new_key, knob);
            }
        }
    }

    /// Get macro value from knob
    pub fn macro_value(&self, index: usize) -> f32 {
        self.macro_knobs.get(index).map(|k| k.value()).unwrap_or(0.5)
    }

    /// Set macro value
    pub fn set_macro_value(&mut self, index: usize, value: f32) {
        if let Some(knob) = self.macro_knobs.get_mut(index) {
            knob.set_value(value);
        }
    }

    /// Update band frequency ranges from crossover frequencies
    pub fn update_band_frequencies(&mut self) {
        let num_bands = self.bands.len();

        for (i, band) in self.bands.iter_mut().enumerate() {
            band.freq_low = if i == 0 {
                super::FREQ_MIN
            } else {
                self.crossover_freqs[i - 1]
            };

            band.freq_high = if i == num_bands - 1 {
                super::FREQ_MAX
            } else {
                self.crossover_freqs[i]
            };
        }
    }

    /// Add a new band (splits the last band)
    pub fn add_band(&mut self) {
        if self.bands.len() >= 8 {
            return;
        }

        let new_index = self.bands.len();

        // Calculate new crossover frequency (logarithmic midpoint of last band)
        let last_band = self.bands.last().unwrap();
        let log_mid = (last_band.freq_low.log10() + last_band.freq_high.log10()) / 2.0;
        let new_crossover = 10.0_f32.powf(log_mid);

        self.crossover_freqs.push(new_crossover);
        self.bands.push(BandUiState::new(new_index, new_crossover, last_band.freq_high));

        self.update_band_frequencies();
    }

    /// Remove a band by index
    pub fn remove_band(&mut self, index: usize) {
        if self.bands.len() <= 1 || index >= self.bands.len() {
            return;
        }

        // Remove effect knobs for this band
        for effect_idx in 0..self.bands[index].effects.len() {
            self.remove_effect_knobs(EffectChainLocation::Band(index), effect_idx);
        }

        self.bands.remove(index);

        // Remove the corresponding crossover frequency
        if !self.crossover_freqs.is_empty() {
            let freq_index = index.min(self.crossover_freqs.len() - 1);
            self.crossover_freqs.remove(freq_index);
        }

        // Update band indices
        for (i, band) in self.bands.iter_mut().enumerate() {
            band.index = i;
        }

        // Update effect knob keys for bands after the removed one
        let keys_to_update: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, _, _)| matches!(loc, EffectChainLocation::Band(b) if b > index))
            .copied()
            .collect();
        for key in keys_to_update {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                if let EffectChainLocation::Band(b) = key.0 {
                    let new_key = (EffectChainLocation::Band(b - 1), key.1, key.2);
                    self.effect_knobs.insert(new_key, knob);
                }
            }
        }

        self.update_band_frequencies();
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
    }

    /// Set a crossover frequency
    pub fn set_crossover_freq(&mut self, index: usize, freq: f32) {
        if index >= self.crossover_freqs.len() {
            return;
        }

        // Clamp to valid range (must be between adjacent crossovers)
        let min_freq = if index == 0 {
            super::FREQ_MIN + 10.0
        } else {
            self.crossover_freqs[index - 1] + 10.0
        };

        let max_freq = if index == self.crossover_freqs.len() - 1 {
            super::FREQ_MAX - 10.0
        } else {
            self.crossover_freqs[index + 1] - 10.0
        };

        self.crossover_freqs[index] = freq.clamp(min_freq, max_freq);
        self.update_band_frequencies();
    }

    /// Set band mute state
    pub fn set_band_mute(&mut self, index: usize, muted: bool) {
        if let Some(band) = self.bands.get_mut(index) {
            band.muted = muted;
        }
    }

    /// Set band solo state
    pub fn set_band_solo(&mut self, index: usize, soloed: bool) {
        if let Some(band) = self.bands.get_mut(index) {
            band.soloed = soloed;
        }
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
    }

    /// Set effect bypass state
    pub fn set_effect_bypass(&mut self, band_index: usize, effect_index: usize, bypassed: bool) {
        if let Some(band) = self.bands.get_mut(band_index) {
            if let Some(effect) = band.effects.get_mut(effect_index) {
                effect.bypassed = bypassed;
            }
        }
    }

    /// Set macro name
    pub fn set_macro_name(&mut self, index: usize, name: String) {
        if let Some(macro_state) = self.macros.get_mut(index) {
            macro_state.name = name;
        }
    }
}
