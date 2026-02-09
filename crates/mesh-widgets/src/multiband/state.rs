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
///
/// The macro applies a bipolar offset to the parameter's base value:
/// - Macro at 0%: actual = base - offset_range
/// - Macro at 50%: actual = base (no change)
/// - Macro at 100%: actual = base + offset_range
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamMacroMapping {
    /// Which macro (0-7) controls this param, None if unmapped
    pub macro_index: Option<usize>,
    /// Offset range: how much the macro can offset from base value
    /// E.g., 0.1 means macro can offset by ±10% (normalized)
    pub offset_range: f32,
}

impl Default for ParamMacroMapping {
    fn default() -> Self {
        Self {
            macro_index: None,
            offset_range: 0.25, // Default ±25% range
        }
    }
}

impl ParamMacroMapping {
    /// Create a new mapping with specified macro and offset range
    pub fn new(macro_index: usize, offset_range: f32) -> Self {
        Self {
            macro_index: Some(macro_index),
            offset_range,
        }
    }

    /// Calculate the modulated value given base value and macro position
    ///
    /// Formula: actual = base + (macro * 2 - 1) * offset_range
    /// - macro=0: offset = -offset_range
    /// - macro=0.5: offset = 0
    /// - macro=1: offset = +offset_range
    pub fn modulate(&self, base_value: f32, macro_value: f32) -> f32 {
        let offset = (macro_value * 2.0 - 1.0) * self.offset_range;
        (base_value + offset).clamp(0.0, 1.0)
    }

    /// Get the modulation range bounds for visualization
    /// Returns (min_possible, max_possible) given the base value
    /// Note: Uses absolute offset_range since visualization shows the range extent,
    /// not the direction (direction is shown by the indicator bar fill)
    pub fn modulation_bounds(&self, base_value: f32) -> (f32, f32) {
        let range = self.offset_range.abs();
        let min = (base_value - range).clamp(0.0, 1.0);
        let max = (base_value + range).clamp(0.0, 1.0);
        (min, max)
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

    /// Saved parameter values from preset load (normalized 0.0-1.0)
    /// Contains ALL param values, not just knob-mapped ones.
    /// This preserves settings made via plugin GUI (e.g., reverb mode).
    /// Populated when loading a preset, empty for newly added effects.
    #[serde(skip)]
    pub saved_param_values: Vec<f32>,

    /// Per-effect dry/wet mix (0.0 = fully dry, 1.0 = fully wet)
    /// Default is 1.0 (100% wet = normal processing)
    #[serde(default = "default_dry_wet")]
    pub dry_wet: f32,

    /// Macro mapping for dry/wet parameter
    #[serde(default)]
    pub dry_wet_macro_mapping: Option<ParamMacroMapping>,

    /// Effect latency in samples (reported by plugin, not serialized)
    #[serde(skip)]
    pub latency_samples: u32,
}

/// Default dry/wet value (100% wet = normal processing)
fn default_dry_wet() -> f32 {
    1.0
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
            saved_param_values: Vec::new(), // Fresh effect, no saved values
            dry_wet: 1.0,
            dry_wet_macro_mapping: None,
            latency_samples: 0,
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

        Self {
            id,
            name,
            category,
            source,
            bypassed: false,
            gui_open: false,
            available_params,
            knob_assignments,
            saved_param_values: Vec::new(), // Fresh effect, no saved values
            dry_wet: 1.0,
            dry_wet_macro_mapping: None,
            latency_samples: 0,
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

    /// Chain dry/wet for entire band (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub chain_dry_wet: f32,

    /// Macro mapping for chain dry/wet
    #[serde(default)]
    pub chain_dry_wet_macro_mapping: Option<ParamMacroMapping>,
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
            chain_dry_wet: 1.0,
            chain_dry_wet_macro_mapping: None,
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

/// Identifies which dry/wet knob is being dragged (for global mouse capture)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DryWetKnobId {
    /// Per-effect dry/wet (location, effect_index)
    Effect(EffectChainLocation, usize),
    /// Pre-FX chain dry/wet
    PreFxChain,
    /// Band chain dry/wet (band_index)
    BandChain(usize),
    /// Post-FX chain dry/wet
    PostFxChain,
    /// Global dry/wet
    Global,
}

/// Reference to a macro mapping for the reverse index
///
/// This allows efficient lookup of which parameters are mapped to each macro,
/// enabling the mini modulation indicator UI above macro knobs.
#[derive(Debug, Clone, Copy)]
pub struct MacroMappingRef {
    /// Location of the effect in the chain
    pub location: EffectChainLocation,
    /// Effect index within the chain
    pub effect_idx: usize,
    /// Knob index within the effect (0-7)
    pub knob_idx: usize,
    /// Current offset range (-1 to +1)
    pub offset_range: f32,
}

/// State for dragging a modulation range indicator
#[derive(Debug, Clone, Copy)]
pub struct ModRangeDrag {
    /// Which macro's mapping is being dragged
    pub macro_index: usize,
    /// Index within that macro's mappings
    pub mapping_idx: usize,
    /// Starting offset_range value when drag began
    pub start_offset: f32,
    /// Starting Y position when drag began (None until first mouse move)
    pub start_y: Option<f32>,
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

    /// Starting frequency when crossover drag began (for relative calculation)
    pub crossover_drag_start_freq: Option<f32>,

    /// Last mouse X position during crossover drag (for relative movement)
    pub crossover_drag_last_x: Option<f32>,

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

    /// Which macro name is currently being edited (double-click to edit)
    pub editing_macro_name: Option<usize>,

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

    /// Currently dragging dry/wet knob
    /// When set, global mouse events should be routed to this knob
    pub dragging_dry_wet_knob: Option<DryWetKnobId>,

    // ─────────────────────────────────────────────────────────────────────
    // CLAP Plugin GUI Learning Mode
    // ─────────────────────────────────────────────────────────────────────

    /// Knob currently in learning mode: (location, effect_idx, knob_idx)
    /// When set, the next parameter change from a CLAP plugin GUI will be
    /// assigned to this knob.
    pub learning_knob: Option<(EffectChainLocation, usize, usize)>,

    // ─────────────────────────────────────────────────────────────────────
    // Macro Modulation Range Controls
    // ─────────────────────────────────────────────────────────────────────

    /// Reverse index: for each macro, list of mappings to that macro
    /// This enables efficient lookup for rendering mini modulation indicators.
    pub macro_mappings_index: [Vec<MacroMappingRef>; super::NUM_MACROS],

    /// Currently dragging a modulation range indicator
    pub dragging_mod_range: Option<ModRangeDrag>,

    /// Currently hovered modulation indicator: (macro_idx, mapping_idx)
    /// Used to highlight the target parameter knob when hovering an indicator.
    pub hovered_mapping: Option<(usize, usize)>,

    /// Currently hovered parameter knob: (location, effect_idx, knob_idx)
    /// Used to highlight the mapped macro button when hovering a param.
    pub hovered_param: Option<(EffectChainLocation, usize, usize)>,

    // ─────────────────────────────────────────────────────────────────────
    // Drag and Drop
    // ─────────────────────────────────────────────────────────────────────

    /// Currently dragging a band (band index)
    /// When dropped, the band's effects swap with the target band
    pub dragging_band: Option<usize>,

    /// Drop target for band drag (band index where it would be dropped)
    pub band_drop_target: Option<usize>,

    /// Currently dragging an effect (location, effect_idx)
    pub dragging_effect: Option<(EffectChainLocation, usize)>,

    /// Name of the effect being dragged (for overlay display)
    pub dragging_effect_name: Option<String>,

    /// Current mouse position during effect drag (for overlay positioning)
    pub effect_drag_mouse_pos: Option<(f32, f32)>,

    /// Drop target for effect drag (location, effect_idx - inserts before this position)
    pub effect_drop_target: Option<(EffectChainLocation, usize)>,

    // ─────────────────────────────────────────────────────────────────────
    // Dry/Wet Mix Controls
    // ─────────────────────────────────────────────────────────────────────

    /// Pre-FX chain dry/wet mix (0.0 = dry, 1.0 = wet)
    pub pre_fx_chain_dry_wet: f32,

    /// Macro mapping for pre-fx chain dry/wet
    pub pre_fx_chain_dry_wet_macro_mapping: Option<ParamMacroMapping>,

    /// Post-FX chain dry/wet mix (0.0 = dry, 1.0 = wet)
    pub post_fx_chain_dry_wet: f32,

    /// Macro mapping for post-fx chain dry/wet
    pub post_fx_chain_dry_wet_macro_mapping: Option<ParamMacroMapping>,

    /// Global dry/wet mix for entire effect rack (0.0 = dry, 1.0 = wet)
    pub global_dry_wet: f32,

    /// Macro mapping for global dry/wet
    pub global_dry_wet_macro_mapping: Option<ParamMacroMapping>,

    // ─────────────────────────────────────────────────────────────────────
    // Dry/Wet Knob Widgets (for drag state persistence)
    // ─────────────────────────────────────────────────────────────────────

    /// Knob widgets for per-effect dry/wet (keyed by location + effect index)
    pub effect_dry_wet_knobs: HashMap<(EffectChainLocation, usize), Knob>,

    /// Knob widget for pre-fx chain dry/wet
    pub pre_fx_chain_dry_wet_knob: Knob,

    /// Knob widget for post-fx chain dry/wet
    pub post_fx_chain_dry_wet_knob: Knob,

    /// Knob widgets for band chain dry/wet (indexed by band)
    pub band_chain_dry_wet_knobs: Vec<Knob>,

    /// Knob widget for global dry/wet
    pub global_dry_wet_knob: Knob,
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
            crossover_drag_start_freq: None,
            crossover_drag_last_x: None,
            dragging_macro: None,
            bands: vec![BandUiState::new(0, super::FREQ_MIN, super::FREQ_MAX)],
            post_fx: Vec::new(),
            selected_effect: None,
            macros: (0..super::NUM_MACROS).map(MacroUiState::new).collect(),
            editing_macro_name: None,
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
            dragging_dry_wet_knob: None,
            learning_knob: None,
            macro_mappings_index: Default::default(),
            dragging_mod_range: None,
            hovered_mapping: None,
            hovered_param: None,
            // Drag and drop
            dragging_band: None,
            band_drop_target: None,
            dragging_effect: None,
            dragging_effect_name: None,
            effect_drag_mouse_pos: None,
            effect_drop_target: None,
            // Dry/wet mix controls (default: 100% wet = normal processing)
            pre_fx_chain_dry_wet: 1.0,
            pre_fx_chain_dry_wet_macro_mapping: None,
            post_fx_chain_dry_wet: 1.0,
            post_fx_chain_dry_wet_macro_mapping: None,
            global_dry_wet: 1.0,
            global_dry_wet_macro_mapping: None,
            // Dry/wet knob widgets - all initialized to 100% (1.0)
            effect_dry_wet_knobs: HashMap::new(),
            pre_fx_chain_dry_wet_knob: {
                let mut k = Knob::new(36.0);
                k.set_value(1.0);
                k
            },
            post_fx_chain_dry_wet_knob: {
                let mut k = Knob::new(36.0);
                k.set_value(1.0);
                k
            },
            band_chain_dry_wet_knobs: vec![{
                let mut k = Knob::new(36.0);
                k.set_value(1.0);
                k
            }],
            global_dry_wet_knob: {
                let mut k = Knob::new(48.0);
                k.set_value(1.0);
                k
            },
        }
    }

    /// Check if any knob is currently being dragged (for mouse capture subscription)
    pub fn is_any_knob_dragging(&self) -> bool {
        self.dragging_effect_knob.is_some()
            || self.dragging_macro_knob.is_some()
            || self.dragging_dry_wet_knob.is_some()
            || self.dragging_mod_range.is_some()
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
        self.crossover_drag_start_freq = None;
        self.crossover_drag_last_x = None;
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
            let mut knob = Knob::new(48.0); // Size for effect params
            if let Some(value) = initial_value {
                knob.set_value(value);
            }
            knob
        })
    }

    /// Get effect parameter value from state (knob_idx is the UI knob index 0-7)
    fn get_effect_param_value(&self, location: EffectChainLocation, effect_idx: usize, knob_idx: usize) -> f32 {
        match location {
            EffectChainLocation::PreFx => {
                self.pre_fx.get(effect_idx)
                    .and_then(|e| e.knob_assignments.get(knob_idx))
                    .map(|a| a.value)
                    .unwrap_or(0.5)
            }
            EffectChainLocation::Band(band_idx) => {
                self.bands.get(band_idx)
                    .and_then(|b| b.effects.get(effect_idx))
                    .and_then(|e| e.knob_assignments.get(knob_idx))
                    .map(|a| a.value)
                    .unwrap_or(0.5)
            }
            EffectChainLocation::PostFx => {
                self.post_fx.get(effect_idx)
                    .and_then(|e| e.knob_assignments.get(knob_idx))
                    .map(|a| a.value)
                    .unwrap_or(0.5)
            }
        }
    }

    /// Set effect parameter value in state and sync to knob (knob_idx is the UI knob index 0-7)
    pub fn set_effect_param_value(&mut self, location: EffectChainLocation, effect_idx: usize, knob_idx: usize, value: f32) {
        // Update knob assignment value
        match location {
            EffectChainLocation::PreFx => {
                if let Some(effect) = self.pre_fx.get_mut(effect_idx) {
                    if let Some(assignment) = effect.knob_assignments.get_mut(knob_idx) {
                        assignment.value = value;
                    }
                }
            }
            EffectChainLocation::Band(band_idx) => {
                if let Some(band) = self.bands.get_mut(band_idx) {
                    if let Some(effect) = band.effects.get_mut(effect_idx) {
                        if let Some(assignment) = effect.knob_assignments.get_mut(knob_idx) {
                            assignment.value = value;
                        }
                    }
                }
            }
            EffectChainLocation::PostFx => {
                if let Some(effect) = self.post_fx.get_mut(effect_idx) {
                    if let Some(assignment) = effect.knob_assignments.get_mut(knob_idx) {
                        assignment.value = value;
                    }
                }
            }
        }

        // Sync knob widget if it exists
        let key = (location, effect_idx, knob_idx);
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
        if self.bands.len() >= 3 {
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

    /// Add a new band at a specific frequency
    ///
    /// Inserts a crossover at the given frequency, splitting the band that
    /// contains that frequency into two. The new band gets the upper portion.
    pub fn add_band_at_frequency(&mut self, freq: f32) {
        if self.bands.len() >= 3 {
            return;
        }

        // Clamp frequency to valid range
        let freq = freq.clamp(super::FREQ_MIN + 10.0, super::FREQ_MAX - 10.0);

        // Find which band contains this frequency
        let band_idx = self.bands.iter().position(|b| freq >= b.freq_low && freq < b.freq_high);
        let Some(band_idx) = band_idx else {
            // Frequency not in any band - shouldn't happen but fall back to regular add
            return self.add_band();
        };

        // The crossover index is the same as the band index (crossover goes after the band)
        // Insert the new crossover at the right position to maintain sorted order
        let crossover_idx = band_idx;

        // Insert crossover frequency
        self.crossover_freqs.insert(crossover_idx, freq);

        // Insert new band after the current one
        let new_band_idx = band_idx + 1;
        let old_band_high = self.bands[band_idx].freq_high;
        self.bands.insert(new_band_idx, BandUiState::new(new_band_idx, freq, old_band_high));

        // Add a new dry/wet knob for the new band
        let mut new_knob = super::super::knob::Knob::new(36.0);
        new_knob.set_value(1.0);
        self.band_chain_dry_wet_knobs.insert(new_band_idx, new_knob);

        // Update band indices
        for (i, band) in self.bands.iter_mut().enumerate() {
            band.index = i;
        }

        // Update effect knob keys for bands at or after the new one
        let keys_to_update: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, _, _)| matches!(loc, EffectChainLocation::Band(b) if b >= new_band_idx))
            .copied()
            .collect();
        for key in keys_to_update {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                if let EffectChainLocation::Band(b) = key.0 {
                    let new_key = (EffectChainLocation::Band(b + 1), key.1, key.2);
                    self.effect_knobs.insert(new_key, knob);
                }
            }
        }

        // Same for effect dry/wet knobs
        let dw_keys_to_update: Vec<_> = self.effect_dry_wet_knobs.keys()
            .filter(|&&(loc, _)| matches!(loc, EffectChainLocation::Band(b) if b >= new_band_idx))
            .copied()
            .collect();
        for key in dw_keys_to_update {
            if let Some(knob) = self.effect_dry_wet_knobs.remove(&key) {
                if let EffectChainLocation::Band(b) = key.0 {
                    let new_key = (EffectChainLocation::Band(b + 1), key.1);
                    self.effect_dry_wet_knobs.insert(new_key, knob);
                }
            }
        }

        self.update_band_frequencies();
        self.rebuild_macro_mappings_index();
    }

    /// Swap the processing contents of two bands
    ///
    /// This exchanges the effects, gain, mute/solo state, and dry/wet settings
    /// between two bands while keeping the frequency ranges intact.
    pub fn swap_band_contents(&mut self, a: usize, b: usize) {
        if a >= self.bands.len() || b >= self.bands.len() || a == b {
            return;
        }

        // Ensure a < b for split_at_mut
        let (a, b) = if a < b { (a, b) } else { (b, a) };

        // Split to get mutable references to both bands
        let (left, right) = self.bands.split_at_mut(b);
        let band_a = &mut left[a];
        let band_b = &mut right[0];

        // Swap effect chains
        std::mem::swap(&mut band_a.effects, &mut band_b.effects);

        // Swap other processing-related fields
        std::mem::swap(&mut band_a.gain, &mut band_b.gain);
        std::mem::swap(&mut band_a.muted, &mut band_b.muted);
        std::mem::swap(&mut band_a.soloed, &mut band_b.soloed);
        std::mem::swap(&mut band_a.chain_dry_wet, &mut band_b.chain_dry_wet);
        std::mem::swap(&mut band_a.chain_dry_wet_macro_mapping, &mut band_b.chain_dry_wet_macro_mapping);

        // Swap dry/wet knobs
        if a < self.band_chain_dry_wet_knobs.len() && b < self.band_chain_dry_wet_knobs.len() {
            self.band_chain_dry_wet_knobs.swap(a, b);
        }

        // Update effect knob keys: swap Band(a) <-> Band(b)
        let keys_a: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, _, _)| loc == EffectChainLocation::Band(a))
            .copied()
            .collect();
        let keys_b: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, _, _)| loc == EffectChainLocation::Band(b))
            .copied()
            .collect();

        // Remove all and re-insert with swapped band indices
        let mut knobs_a = Vec::new();
        let mut knobs_b = Vec::new();
        for key in keys_a {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                knobs_a.push((key, knob));
            }
        }
        for key in keys_b {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                knobs_b.push((key, knob));
            }
        }
        for ((_, effect_idx, knob_idx), knob) in knobs_a {
            self.effect_knobs.insert((EffectChainLocation::Band(b), effect_idx, knob_idx), knob);
        }
        for ((_, effect_idx, knob_idx), knob) in knobs_b {
            self.effect_knobs.insert((EffectChainLocation::Band(a), effect_idx, knob_idx), knob);
        }

        // Same for effect dry/wet knobs
        let dw_keys_a: Vec<_> = self.effect_dry_wet_knobs.keys()
            .filter(|&&(loc, _)| loc == EffectChainLocation::Band(a))
            .copied()
            .collect();
        let dw_keys_b: Vec<_> = self.effect_dry_wet_knobs.keys()
            .filter(|&&(loc, _)| loc == EffectChainLocation::Band(b))
            .copied()
            .collect();

        let mut dw_knobs_a = Vec::new();
        let mut dw_knobs_b = Vec::new();
        for key in dw_keys_a {
            if let Some(knob) = self.effect_dry_wet_knobs.remove(&key) {
                dw_knobs_a.push((key, knob));
            }
        }
        for key in dw_keys_b {
            if let Some(knob) = self.effect_dry_wet_knobs.remove(&key) {
                dw_knobs_b.push((key, knob));
            }
        }
        for ((_, effect_idx), knob) in dw_knobs_a {
            self.effect_dry_wet_knobs.insert((EffectChainLocation::Band(b), effect_idx), knob);
        }
        for ((_, effect_idx), knob) in dw_knobs_b {
            self.effect_dry_wet_knobs.insert((EffectChainLocation::Band(a), effect_idx), knob);
        }

        self.any_soloed = self.bands.iter().any(|b| b.soloed);
        self.rebuild_macro_mappings_index();
    }

    /// Move an effect from one location to another
    ///
    /// The effect is removed from `from` and inserted at `to_position` in `to_location`.
    /// All macro mappings, parameter values, and knob widgets are preserved.
    pub fn move_effect(
        &mut self,
        from_location: EffectChainLocation,
        from_idx: usize,
        to_location: EffectChainLocation,
        to_position: usize,
    ) {
        // Step 1: Extract knobs for the effect being moved (save them, don't delete)
        let mut saved_knobs: Vec<(usize, Knob)> = Vec::new();
        for knob_idx in 0..MAX_UI_KNOBS {
            let key = (from_location, from_idx, knob_idx);
            if let Some(knob) = self.effect_knobs.remove(&key) {
                saved_knobs.push((knob_idx, knob));
            }
        }
        let saved_dw_knob = self.effect_dry_wet_knobs.remove(&(from_location, from_idx));

        // Step 2: Get the effect from the source
        let effect = match from_location {
            EffectChainLocation::PreFx => {
                if from_idx >= self.pre_fx.len() { return; }
                self.pre_fx.remove(from_idx)
            }
            EffectChainLocation::Band(band_idx) => {
                if band_idx >= self.bands.len() { return; }
                if from_idx >= self.bands[band_idx].effects.len() { return; }
                self.bands[band_idx].effects.remove(from_idx)
            }
            EffectChainLocation::PostFx => {
                if from_idx >= self.post_fx.len() { return; }
                self.post_fx.remove(from_idx)
            }
        };

        // Step 3: Update knob keys in source chain (shift down for effects after removed)
        self.shift_effect_knobs_after_remove(from_location, from_idx);

        // Step 4: Calculate effective insert position (account for removal if same chain)
        let insert_pos = if from_location == to_location && from_idx < to_position {
            to_position.saturating_sub(1)
        } else {
            to_position
        };

        // Step 5: Update knob keys in destination chain (shift up to make room)
        self.shift_effect_knobs_after_insert(to_location, insert_pos);

        // Step 6: Insert effect at destination
        let final_pos = match to_location {
            EffectChainLocation::PreFx => {
                let pos = insert_pos.min(self.pre_fx.len());
                self.pre_fx.insert(pos, effect);
                pos
            }
            EffectChainLocation::Band(band_idx) => {
                if band_idx >= self.bands.len() { return; }
                let pos = insert_pos.min(self.bands[band_idx].effects.len());
                self.bands[band_idx].effects.insert(pos, effect);
                pos
            }
            EffectChainLocation::PostFx => {
                let pos = insert_pos.min(self.post_fx.len());
                self.post_fx.insert(pos, effect);
                pos
            }
        };

        // Step 7: Insert saved knobs at new location
        for (knob_idx, knob) in saved_knobs {
            let new_key = (to_location, final_pos, knob_idx);
            self.effect_knobs.insert(new_key, knob);
        }
        if let Some(dw_knob) = saved_dw_knob {
            self.effect_dry_wet_knobs.insert((to_location, final_pos), dw_knob);
        }

        self.rebuild_macro_mappings_index();
    }

    /// Shift effect knob keys down after removing an effect
    fn shift_effect_knobs_after_remove(&mut self, location: EffectChainLocation, removed_idx: usize) {
        let keys_to_shift: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, effect_idx, _)| loc == location && effect_idx > removed_idx)
            .copied()
            .collect();
        for key in keys_to_shift {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                let new_key = (location, key.1 - 1, key.2);
                self.effect_knobs.insert(new_key, knob);
            }
        }

        // Same for dry/wet knobs
        let dw_keys_to_shift: Vec<_> = self.effect_dry_wet_knobs.keys()
            .filter(|&&(loc, effect_idx)| loc == location && effect_idx > removed_idx)
            .copied()
            .collect();
        for key in dw_keys_to_shift {
            if let Some(knob) = self.effect_dry_wet_knobs.remove(&key) {
                let new_key = (location, key.1 - 1);
                self.effect_dry_wet_knobs.insert(new_key, knob);
            }
        }
    }

    /// Shift effect knob keys up after inserting an effect
    fn shift_effect_knobs_after_insert(&mut self, location: EffectChainLocation, insert_idx: usize) {
        // Shift keys >= insert_idx up by 1
        let keys_to_shift: Vec<_> = self.effect_knobs.keys()
            .filter(|&&(loc, effect_idx, _)| loc == location && effect_idx >= insert_idx)
            .copied()
            .collect();
        // Sort in reverse to avoid overwriting
        let mut keys_sorted: Vec<_> = keys_to_shift;
        keys_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for key in keys_sorted {
            if let Some(knob) = self.effect_knobs.remove(&key) {
                let new_key = (location, key.1 + 1, key.2);
                self.effect_knobs.insert(new_key, knob);
            }
        }

        // Same for dry/wet knobs
        let dw_keys_to_shift: Vec<_> = self.effect_dry_wet_knobs.keys()
            .filter(|&&(loc, effect_idx)| loc == location && effect_idx >= insert_idx)
            .copied()
            .collect();
        let mut dw_keys_sorted: Vec<_> = dw_keys_to_shift;
        dw_keys_sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for key in dw_keys_sorted {
            if let Some(knob) = self.effect_dry_wet_knobs.remove(&key) {
                let new_key = (location, key.1 + 1);
                self.effect_dry_wet_knobs.insert(new_key, knob);
            }
        }
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

    // ─────────────────────────────────────────────────────────────────────
    // Macro Modulation Range Index
    // ─────────────────────────────────────────────────────────────────────

    /// Rebuild the reverse mapping index from all effects
    ///
    /// Scans pre_fx, bands[].effects, and post_fx to find all knob assignments
    /// that have macro mappings, then builds a reverse index for each macro.
    /// This enables efficient lookup of which parameters are mapped to each macro.
    pub fn rebuild_macro_mappings_index(&mut self) {
        // Clear existing index
        for mappings in &mut self.macro_mappings_index {
            mappings.clear();
        }

        // Helper closure to extract mappings from an effect
        fn extract_mappings(
            location: EffectChainLocation,
            effect_idx: usize,
            effect: &EffectUiState,
            index: &mut [Vec<MacroMappingRef>; super::NUM_MACROS],
        ) {
            for (knob_idx, assignment) in effect.knob_assignments.iter().enumerate() {
                if let Some(ref mapping) = assignment.macro_mapping {
                    if let Some(macro_index) = mapping.macro_index {
                        if macro_index < super::NUM_MACROS {
                            index[macro_index].push(MacroMappingRef {
                                location,
                                effect_idx,
                                knob_idx,
                                offset_range: mapping.offset_range,
                            });
                        }
                    }
                }
            }
        }

        // Scan pre-fx effects
        for (effect_idx, effect) in self.pre_fx.iter().enumerate() {
            extract_mappings(EffectChainLocation::PreFx, effect_idx, effect, &mut self.macro_mappings_index);
        }

        // Scan band effects
        for (band_idx, band) in self.bands.iter().enumerate() {
            for (effect_idx, effect) in band.effects.iter().enumerate() {
                extract_mappings(EffectChainLocation::Band(band_idx), effect_idx, effect, &mut self.macro_mappings_index);
            }
        }

        // Scan post-fx effects
        for (effect_idx, effect) in self.post_fx.iter().enumerate() {
            extract_mappings(EffectChainLocation::PostFx, effect_idx, effect, &mut self.macro_mappings_index);
        }

        // Update macro mapping counts
        for (macro_idx, mappings) in self.macro_mappings_index.iter().enumerate() {
            if let Some(macro_state) = self.macros.get_mut(macro_idx) {
                macro_state.mapping_count = mappings.len();
            }
        }
    }

    /// Add a single mapping to the index (called when a new mapping is created)
    pub fn add_mapping_to_index(&mut self, macro_index: usize, location: EffectChainLocation, effect_idx: usize, knob_idx: usize, offset_range: f32) {
        if macro_index < super::NUM_MACROS {
            self.macro_mappings_index[macro_index].push(MacroMappingRef {
                location,
                effect_idx,
                knob_idx,
                offset_range,
            });
        }
    }

    /// Remove a mapping from the index (called when a mapping is removed)
    pub fn remove_mapping_from_index(&mut self, macro_index: usize, location: EffectChainLocation, effect_idx: usize, knob_idx: usize) {
        if macro_index < super::NUM_MACROS {
            self.macro_mappings_index[macro_index].retain(|m| {
                !(m.location == location && m.effect_idx == effect_idx && m.knob_idx == knob_idx)
            });
        }
    }

    /// Update offset_range for a mapping in the index
    pub fn update_mapping_offset_range(&mut self, macro_index: usize, mapping_idx: usize, new_offset_range: f32) {
        if macro_index < super::NUM_MACROS {
            if let Some(mapping) = self.macro_mappings_index[macro_index].get_mut(mapping_idx) {
                mapping.offset_range = new_offset_range;
            }
        }
    }
}
