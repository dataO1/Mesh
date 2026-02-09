//! Multiband preset configuration types for serialization
//!
//! These types provide a serializable representation of multiband effect presets
//! that can be stored in YAML config files.
//!
//! # Preset Hierarchy
//!
//! ```text
//! presets/
//!   stems/           # Individual stem presets (reusable across deck presets)
//!     vocal_reverb.yaml
//!     drum_crunch.yaml
//!   decks/           # Deck presets (wrappers referencing stem presets)
//!     my_party_preset.yaml
//! ```
//!
//! A **deck preset** wraps 4 stem presets and owns the shared macros.
//! A **stem preset** stores the effect chain (pre-fx, bands, post-fx, dry/wet)
//! without macros — macro mappings on parameters reference deck-level macro indices.

use super::state::{BandUiState, EffectSourceType, EffectUiState, MacroUiState, MultibandEditorState, ParamMacroMapping};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Stem presets subfolder within the presets directory
pub const STEM_PRESETS_FOLDER: &str = "presets/stems";

/// Deck presets subfolder within the presets directory
pub const DECK_PRESETS_FOLDER: &str = "presets/decks";

/// Legacy presets folder (for backwards-compatible listing)
pub const MULTIBAND_PRESETS_FOLDER: &str = "presets";

// ─────────────────────────────────────────────────────────────────────────────
// Stem Preset Config (per-stem effect chain, no macros)
// ─────────────────────────────────────────────────────────────────────────────

/// Stem preset configuration
///
/// Stores a single stem's effect chain including:
/// - Pre-FX chain (before multiband split)
/// - Crossover frequencies
/// - Band configurations with effects and their macro mappings
/// - Post-FX chain (after band summation)
/// - Dry/wet mix controls at chain and global levels
///
/// **No macros** — macro mappings on parameters (macro_index 0-3) reference
/// deck-level macros defined in the parent `DeckPresetConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StemPresetConfig {
    /// Preset name
    pub name: String,
    /// Pre-FX chain effects
    pub pre_fx: Vec<EffectPresetConfig>,
    /// Crossover frequencies (N-1 for N bands)
    pub crossover_freqs: Vec<f32>,
    /// Band configurations
    pub bands: Vec<BandPresetConfig>,
    /// Post-FX chain effects
    pub post_fx: Vec<EffectPresetConfig>,

    // Dry/Wet Mix Controls
    /// Pre-FX chain dry/wet (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub pre_fx_chain_dry_wet: f32,
    /// Macro mapping for pre-fx chain dry/wet
    #[serde(default)]
    pub pre_fx_chain_dry_wet_macro_mapping: Option<ParamMappingConfig>,
    /// Post-FX chain dry/wet (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub post_fx_chain_dry_wet: f32,
    /// Macro mapping for post-fx chain dry/wet
    #[serde(default)]
    pub post_fx_chain_dry_wet_macro_mapping: Option<ParamMappingConfig>,
    /// Global dry/wet for entire effect rack (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub global_dry_wet: f32,
    /// Macro mapping for global dry/wet
    #[serde(default)]
    pub global_dry_wet_macro_mapping: Option<ParamMappingConfig>,
}

impl Default for StemPresetConfig {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            pre_fx: Vec::new(),
            crossover_freqs: Vec::new(),
            bands: vec![BandPresetConfig::default()],
            post_fx: Vec::new(),
            pre_fx_chain_dry_wet: 1.0,
            pre_fx_chain_dry_wet_macro_mapping: None,
            post_fx_chain_dry_wet: 1.0,
            post_fx_chain_dry_wet_macro_mapping: None,
            global_dry_wet: 1.0,
            global_dry_wet_macro_mapping: None,
        }
    }
}

impl StemPresetConfig {
    /// Create from MultibandEditorState (captures effect data only, not macros)
    pub fn from_editor_state(state: &MultibandEditorState, name: &str) -> Self {
        Self {
            name: name.to_string(),
            pre_fx: state.pre_fx.iter().map(EffectPresetConfig::from_effect_state).collect(),
            crossover_freqs: state.crossover_freqs.clone(),
            bands: state.bands.iter().map(BandPresetConfig::from_band_state).collect(),
            post_fx: state.post_fx.iter().map(EffectPresetConfig::from_effect_state).collect(),
            // Dry/wet mix controls
            pre_fx_chain_dry_wet: state.pre_fx_chain_dry_wet,
            pre_fx_chain_dry_wet_macro_mapping: state.pre_fx_chain_dry_wet_macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
            post_fx_chain_dry_wet: state.post_fx_chain_dry_wet,
            post_fx_chain_dry_wet_macro_mapping: state.post_fx_chain_dry_wet_macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
            global_dry_wet: state.global_dry_wet,
            global_dry_wet_macro_mapping: state.global_dry_wet_macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
        }
    }

    /// Apply effect data to MultibandEditorState (does NOT touch macros)
    pub fn apply_to_editor_state(&self, state: &mut MultibandEditorState) {
        // Clear any active drag/hover state that references old effects
        state.selected_effect = None;
        state.dragging_effect_knob = None;
        state.dragging_macro_knob = None;
        state.dragging_mod_range = None;
        state.hovered_mapping = None;
        state.dragging_crossover = None;
        state.dragging_macro = None;
        state.learning_knob = None;
        state.param_picker_open = None;

        // Clear effect knobs - they reference old effect locations
        state.effect_knobs.clear();
        state.effect_dry_wet_knobs.clear();

        // Clear the macro mappings index - will be rebuilt after
        for mappings in &mut state.macro_mappings_index {
            mappings.clear();
        }

        // Apply pre-fx chain
        state.pre_fx = self.pre_fx.iter().map(|e| e.to_effect_state()).collect();

        // Set crossover frequencies
        state.crossover_freqs = self.crossover_freqs.clone();

        // Rebuild bands from preset
        state.bands.clear();
        for (i, band_config) in self.bands.iter().enumerate() {
            state.bands.push(band_config.to_band_state(i));
        }

        // Update band frequencies from crossovers
        state.update_band_frequencies();

        // Apply post-fx chain
        state.post_fx = self.post_fx.iter().map(|e| e.to_effect_state()).collect();

        // Apply dry/wet mix controls
        state.pre_fx_chain_dry_wet = self.pre_fx_chain_dry_wet;
        state.pre_fx_chain_dry_wet_macro_mapping = self.pre_fx_chain_dry_wet_macro_mapping.as_ref().map(|m| m.to_mapping());
        state.post_fx_chain_dry_wet = self.post_fx_chain_dry_wet;
        state.post_fx_chain_dry_wet_macro_mapping = self.post_fx_chain_dry_wet_macro_mapping.as_ref().map(|m| m.to_mapping());
        state.global_dry_wet = self.global_dry_wet;
        state.global_dry_wet_macro_mapping = self.global_dry_wet_macro_mapping.as_ref().map(|m| m.to_mapping());

        // Rebuild band chain dry/wet knobs to match new band data
        state.band_chain_dry_wet_knobs.clear();
        for band in &state.bands {
            let mut k = crate::knob::Knob::new(36.0);
            k.set_value(band.chain_dry_wet);
            state.band_chain_dry_wet_knobs.push(k);
        }

        // Sync chain-level dry/wet knobs
        state.pre_fx_chain_dry_wet_knob.set_value(self.pre_fx_chain_dry_wet);
        state.post_fx_chain_dry_wet_knob.set_value(self.post_fx_chain_dry_wet);
        state.global_dry_wet_knob.set_value(self.global_dry_wet);

        // Update solo state
        state.any_soloed = state.bands.iter().any(|b| b.soloed);
    }
}

/// Type alias for backwards compatibility — the old name still works everywhere
pub type MultibandPresetConfig = StemPresetConfig;

// ─────────────────────────────────────────────────────────────────────────────
// Deck Preset Config (wrapper: macros + stem preset references)
// ─────────────────────────────────────────────────────────────────────────────

/// Deck preset configuration — wraps 4 stem presets and owns shared macros
///
/// ```yaml
/// name: "My Party Preset"
/// macros:
///   - name: "Reverb"
///     value: 0.5
///   - name: "Filter"
///     value: 0.5
/// stems:
///   vocals: "vocal_reverb"    # references presets/stems/vocal_reverb.yaml
///   drums: "drum_crunch"
///   bass: null                # no effects (passthrough)
///   other: "ambient_wash"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckPresetConfig {
    /// Preset name
    pub name: String,
    /// Shared macro knob configurations (4 macros)
    pub macros: Vec<MacroPresetConfig>,
    /// References to stem presets by name
    pub stems: DeckStemReferences,
}

impl Default for DeckPresetConfig {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            macros: (0..super::NUM_MACROS).map(|i| MacroPresetConfig {
                name: format!("Macro {}", i + 1),
                value: 0.5,
            }).collect(),
            stems: DeckStemReferences::default(),
        }
    }
}

impl DeckPresetConfig {
    /// Build from current editor state (all 4 stems + macros)
    pub fn from_editor_states(
        name: &str,
        stem_preset_names: &[Option<String>; 4],
        macros: &[MacroPresetConfig],
    ) -> Self {
        Self {
            name: name.to_string(),
            macros: macros.to_vec(),
            stems: DeckStemReferences {
                vocals: stem_preset_names[0].clone(),
                drums: stem_preset_names[1].clone(),
                bass: stem_preset_names[2].clone(),
                other: stem_preset_names[3].clone(),
            },
        }
    }

    /// Load a fully resolved deck preset (wrapper + all referenced stem presets)
    pub fn load_resolved(collection_path: &Path, name: &str) -> Result<ResolvedDeckPreset, String> {
        let deck_config = load_deck_preset(collection_path, name)?;

        let stem_refs = [
            deck_config.stems.vocals.as_deref(),
            deck_config.stems.drums.as_deref(),
            deck_config.stems.bass.as_deref(),
            deck_config.stems.other.as_deref(),
        ];

        let mut stems: [Option<StemPresetConfig>; 4] = Default::default();
        let mut stem_names: [Option<String>; 4] = Default::default();

        for (i, stem_ref) in stem_refs.iter().enumerate() {
            if let Some(stem_name) = stem_ref {
                match load_stem_preset(collection_path, stem_name) {
                    Ok(config) => {
                        stems[i] = Some(config);
                        stem_names[i] = Some(stem_name.to_string());
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to load stem preset '{}' for deck preset '{}': {}",
                            stem_name, name, e
                        );
                        // Continue loading other stems rather than failing entirely
                    }
                }
            }
        }

        Ok(ResolvedDeckPreset {
            name: deck_config.name,
            macros: deck_config.macros,
            stems,
            stem_names,
        })
    }
}

/// References to stem presets within a deck preset
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeckStemReferences {
    /// Vocals stem preset name (None = passthrough)
    pub vocals: Option<String>,
    /// Drums stem preset name (None = passthrough)
    pub drums: Option<String>,
    /// Bass stem preset name (None = passthrough)
    pub bass: Option<String>,
    /// Other stem preset name (None = passthrough)
    pub other: Option<String>,
}

impl DeckStemReferences {
    /// Get stem reference by index (0=vocals, 1=drums, 2=bass, 3=other)
    pub fn by_index(&self, index: usize) -> Option<&str> {
        match index {
            0 => self.vocals.as_deref(),
            1 => self.drums.as_deref(),
            2 => self.bass.as_deref(),
            3 => self.other.as_deref(),
            _ => None,
        }
    }

    /// Set stem reference by index
    pub fn set_by_index(&mut self, index: usize, name: Option<String>) {
        match index {
            0 => self.vocals = name,
            1 => self.drums = name,
            2 => self.bass = name,
            3 => self.other = name,
            _ => {}
        }
    }
}

/// A fully resolved deck preset with stem configs loaded from disk
#[derive(Debug, Clone)]
pub struct ResolvedDeckPreset {
    /// Deck preset name
    pub name: String,
    /// Shared macro configurations
    pub macros: Vec<MacroPresetConfig>,
    /// Loaded stem configs (None = passthrough)
    pub stems: [Option<StemPresetConfig>; 4],
    /// Original reference names from the deck preset file
    pub stem_names: [Option<String>; 4],
}

// ─────────────────────────────────────────────────────────────────────────────
// Band / Effect / Knob / Mapping Config (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

/// Band configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BandPresetConfig {
    /// Band gain (linear, 0.0-2.0)
    pub gain: f32,
    /// Whether this band is muted
    pub muted: bool,
    /// Whether this band is soloed
    pub soloed: bool,
    /// Effects in this band's chain
    pub effects: Vec<EffectPresetConfig>,
    /// Chain dry/wet for entire band (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub chain_dry_wet: f32,
    /// Macro mapping for chain dry/wet
    #[serde(default)]
    pub chain_dry_wet_macro_mapping: Option<ParamMappingConfig>,
}

impl Default for BandPresetConfig {
    fn default() -> Self {
        Self {
            gain: 1.0,
            muted: false,
            soloed: false,
            effects: Vec::new(),
            chain_dry_wet: 1.0,
            chain_dry_wet_macro_mapping: None,
        }
    }
}

impl BandPresetConfig {
    fn from_band_state(band: &BandUiState) -> Self {
        Self {
            gain: band.gain,
            muted: band.muted,
            soloed: band.soloed,
            effects: band.effects.iter().map(EffectPresetConfig::from_effect_state).collect(),
            chain_dry_wet: band.chain_dry_wet,
            chain_dry_wet_macro_mapping: band.chain_dry_wet_macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
        }
    }

    fn to_band_state(&self, index: usize) -> BandUiState {
        let mut band = BandUiState::new(index, super::FREQ_MIN, super::FREQ_MAX);
        band.gain = self.gain;
        band.muted = self.muted;
        band.soloed = self.soloed;
        band.effects = self.effects.iter().map(|e| e.to_effect_state()).collect();
        band.chain_dry_wet = self.chain_dry_wet;
        band.chain_dry_wet_macro_mapping = self.chain_dry_wet_macro_mapping.as_ref().map(|m| m.to_mapping());
        band
    }
}

/// Effect configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectPresetConfig {
    /// Effect identifier (path or plugin ID)
    pub id: String,
    /// Effect display name
    pub name: String,
    /// Effect category
    pub category: String,
    /// Effect source type ("pd", "clap", "native")
    pub source: String,
    /// Whether the effect is bypassed
    #[serde(default)]
    pub bypassed: bool,
    /// Knob assignments (8 knobs per effect)
    /// Each stores: param_index (for learned params), value, macro_mapping
    pub knob_assignments: Vec<KnobAssignmentConfig>,
    /// All parameter values (normalized 0.0-1.0), indexed by param_index
    /// This stores the complete plugin state, not just the 8 knob-mapped params.
    /// Essential for preserving settings made via the plugin GUI (e.g., reverb mode).
    #[serde(default)]
    pub all_param_values: Vec<f32>,
    /// Per-effect dry/wet mix (0.0 = dry, 1.0 = wet)
    #[serde(default = "default_dry_wet")]
    pub dry_wet: f32,
    /// Macro mapping for dry/wet
    #[serde(default)]
    pub dry_wet_macro_mapping: Option<ParamMappingConfig>,
}

/// Default dry/wet value (100% wet = normal processing)
fn default_dry_wet() -> f32 {
    1.0
}

/// Knob assignment configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnobAssignmentConfig {
    /// Parameter index in effect's available_params list (None = unassigned)
    pub param_index: Option<usize>,
    /// Current value (normalized 0.0-1.0)
    pub value: f32,
    /// Macro mapping for this knob (if any)
    pub macro_mapping: Option<ParamMappingConfig>,
}

impl Default for KnobAssignmentConfig {
    fn default() -> Self {
        Self {
            param_index: None,
            value: 0.5,
            macro_mapping: None,
        }
    }
}

impl EffectPresetConfig {
    fn from_effect_state(effect: &EffectUiState) -> Self {
        // Use saved_param_values if available (captured from plugin before save)
        let captured = if !effect.saved_param_values.is_empty() {
            Some(effect.saved_param_values.clone())
        } else {
            None
        };
        Self::from_effect_state_with_params(effect, captured)
    }

    /// Create from effect state with optional captured parameter values
    ///
    /// If `captured_param_values` is provided, those values are stored.
    /// Otherwise, falls back to constructing values from knob assignments
    /// and available_params defaults.
    pub fn from_effect_state_with_params(
        effect: &EffectUiState,
        captured_param_values: Option<Vec<f32>>,
    ) -> Self {
        let knob_assignments: Vec<KnobAssignmentConfig> = effect
            .knob_assignments
            .iter()
            .map(|a| KnobAssignmentConfig {
                param_index: a.param_index,
                value: a.value,
                macro_mapping: a.macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
            })
            .collect();

        // Use captured param values if provided, otherwise reconstruct from available data
        let all_param_values = captured_param_values.unwrap_or_else(|| {
            // Build from knob assignments and defaults
            let mut values: Vec<f32> = effect
                .available_params
                .iter()
                .map(|p| p.default)
                .collect();

            // Override with knob assignment values where assigned
            for assignment in &effect.knob_assignments {
                if let Some(param_idx) = assignment.param_index {
                    if param_idx < values.len() {
                        values[param_idx] = assignment.value;
                    }
                }
            }

            values
        });

        Self {
            id: effect.id.clone(),
            name: effect.name.clone(),
            category: effect.category.clone(),
            source: match effect.source {
                EffectSourceType::Pd => "pd".to_string(),
                EffectSourceType::Clap => "clap".to_string(),
                EffectSourceType::Native => "native".to_string(),
            },
            bypassed: effect.bypassed,
            knob_assignments,
            all_param_values,
            dry_wet: effect.dry_wet,
            dry_wet_macro_mapping: effect.dry_wet_macro_mapping.as_ref().map(ParamMappingConfig::from_mapping),
        }
    }

    fn to_effect_state(&self) -> EffectUiState {
        use super::state::{KnobAssignment, MAX_UI_KNOBS};

        let source = match self.source.as_str() {
            "pd" => EffectSourceType::Pd,
            "clap" => EffectSourceType::Clap,
            _ => EffectSourceType::Native,
        };

        // Load knob assignments
        let mut knob_assignments: [KnobAssignment; MAX_UI_KNOBS] = Default::default();
        for (i, config) in self.knob_assignments.iter().enumerate().take(MAX_UI_KNOBS) {
            knob_assignments[i] = KnobAssignment {
                param_index: config.param_index,
                value: config.value,
                macro_mapping: config.macro_mapping.as_ref().map(|m| m.to_mapping()),
            };
        }

        EffectUiState {
            id: self.id.clone(),
            name: self.name.clone(),
            category: self.category.clone(),
            source,
            bypassed: self.bypassed,
            gui_open: false,
            available_params: Vec::new(), // Will be populated when plugin loads
            knob_assignments,
            // Restore saved param values so they can be applied when plugin loads
            saved_param_values: self.all_param_values.clone(),
            dry_wet: self.dry_wet,
            dry_wet_macro_mapping: self.dry_wet_macro_mapping.as_ref().map(|m| m.to_mapping()),
            latency_samples: 0, // Will be populated when plugin loads
        }
    }
}

/// Macro mapping configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamMappingConfig {
    /// Which macro (0-3) controls this param, None if unmapped
    pub macro_index: Option<usize>,
    /// Offset range: how much the macro can offset from base value (±range)
    pub offset_range: f32,
}

impl Default for ParamMappingConfig {
    fn default() -> Self {
        Self {
            macro_index: None,
            offset_range: 0.25, // Default ±25% range
        }
    }
}

impl ParamMappingConfig {
    fn from_mapping(mapping: &ParamMacroMapping) -> Self {
        Self {
            macro_index: mapping.macro_index,
            offset_range: mapping.offset_range,
        }
    }

    fn to_mapping(&self) -> ParamMacroMapping {
        ParamMacroMapping {
            macro_index: self.macro_index,
            offset_range: self.offset_range,
        }
    }
}

/// Macro knob configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroPresetConfig {
    /// Display name
    pub name: String,
    /// Current value (normalized 0.0-1.0)
    #[serde(default = "default_macro_value")]
    pub value: f32,
}

fn default_macro_value() -> f32 {
    0.5
}

impl MacroPresetConfig {
    /// Create from macro UI state
    pub fn from_macro_state(macro_state: &MacroUiState, value: f32) -> Self {
        Self {
            name: macro_state.name.clone(),
            value,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset File I/O
// ─────────────────────────────────────────────────────────────────────────────

/// Sanitize a preset name for use as filename
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// ── Stem Presets ─────────────────────────────────────────────────────────────

/// Get the stem presets folder path for a collection
pub fn stem_presets_folder(collection_path: &Path) -> PathBuf {
    collection_path.join(STEM_PRESETS_FOLDER)
}

/// Get the stem preset file path for a given preset name
pub fn stem_preset_file_path(collection_path: &Path, preset_name: &str) -> PathBuf {
    let sanitized = sanitize_filename(preset_name);
    stem_presets_folder(collection_path).join(format!("{}.yaml", sanitized))
}

/// Save a stem preset to file
pub fn save_stem_preset(config: &StemPresetConfig, collection_path: &Path) -> Result<(), String> {
    let folder = stem_presets_folder(collection_path);
    let path = stem_preset_file_path(collection_path, &config.name);
    log::info!("save_stem_preset: Saving to {:?}", path);

    if let Err(e) = std::fs::create_dir_all(&folder) {
        let msg = format!("Failed to create stem presets directory: {}", e);
        log::error!("save_stem_preset: {}", msg);
        return Err(msg);
    }

    let yaml = serde_yaml::to_string(config)
        .map_err(|e| format!("Failed to serialize stem preset: {}", e))?;

    std::fs::write(&path, yaml)
        .map_err(|e| format!("Failed to write stem preset file: {}", e))?;

    log::info!("save_stem_preset: Saved '{}' successfully", config.name);
    Ok(())
}

/// Load a stem preset from file
pub fn load_stem_preset(collection_path: &Path, preset_name: &str) -> Result<StemPresetConfig, String> {
    let path = stem_preset_file_path(collection_path, preset_name);
    log::info!("load_stem_preset: Loading from {:?}", path);

    if !path.exists() {
        return Err(format!("Stem preset '{}' not found", preset_name));
    }

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read stem preset file: {}", e))?;

    serde_yaml::from_str::<StemPresetConfig>(&contents)
        .map_err(|e| format!("Failed to parse stem preset: {}", e))
}

/// List available stem preset names
pub fn list_stem_presets(collection_path: &Path) -> Vec<String> {
    list_yaml_files_in(&stem_presets_folder(collection_path))
}

/// Delete a stem preset by name
pub fn delete_stem_preset(collection_path: &Path, preset_name: &str) -> Result<(), String> {
    let path = stem_preset_file_path(collection_path, preset_name);
    if !path.exists() {
        return Err(format!("Stem preset '{}' not found", preset_name));
    }
    std::fs::remove_file(&path)
        .map_err(|e| format!("Failed to delete stem preset: {}", e))
}

// ── Deck Presets ─────────────────────────────────────────────────────────────

/// Get the deck presets folder path for a collection
pub fn deck_presets_folder(collection_path: &Path) -> PathBuf {
    collection_path.join(DECK_PRESETS_FOLDER)
}

/// Get the deck preset file path for a given preset name
pub fn deck_preset_file_path(collection_path: &Path, preset_name: &str) -> PathBuf {
    let sanitized = sanitize_filename(preset_name);
    deck_presets_folder(collection_path).join(format!("{}.yaml", sanitized))
}

/// Save a deck preset to file
pub fn save_deck_preset(config: &DeckPresetConfig, collection_path: &Path) -> Result<(), String> {
    let folder = deck_presets_folder(collection_path);
    let path = deck_preset_file_path(collection_path, &config.name);
    log::info!("save_deck_preset: Saving to {:?}", path);

    if let Err(e) = std::fs::create_dir_all(&folder) {
        let msg = format!("Failed to create deck presets directory: {}", e);
        log::error!("save_deck_preset: {}", msg);
        return Err(msg);
    }

    let yaml = serde_yaml::to_string(config)
        .map_err(|e| format!("Failed to serialize deck preset: {}", e))?;

    std::fs::write(&path, yaml)
        .map_err(|e| format!("Failed to write deck preset file: {}", e))?;

    log::info!("save_deck_preset: Saved '{}' successfully", config.name);
    Ok(())
}

/// Load a deck preset from file
pub fn load_deck_preset(collection_path: &Path, preset_name: &str) -> Result<DeckPresetConfig, String> {
    let path = deck_preset_file_path(collection_path, preset_name);
    log::info!("load_deck_preset: Loading from {:?}", path);

    if !path.exists() {
        return Err(format!("Deck preset '{}' not found", preset_name));
    }

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read deck preset file: {}", e))?;

    serde_yaml::from_str::<DeckPresetConfig>(&contents)
        .map_err(|e| format!("Failed to parse deck preset: {}", e))
}

/// List available deck preset names
pub fn list_deck_presets(collection_path: &Path) -> Vec<String> {
    list_yaml_files_in(&deck_presets_folder(collection_path))
}

/// Delete a deck preset by name
pub fn delete_deck_preset(collection_path: &Path, preset_name: &str) -> Result<(), String> {
    let path = deck_preset_file_path(collection_path, preset_name);
    if !path.exists() {
        return Err(format!("Deck preset '{}' not found", preset_name));
    }
    std::fs::remove_file(&path)
        .map_err(|e| format!("Failed to delete deck preset: {}", e))
}

// ── Legacy Compatibility ─────────────────────────────────────────────────────

/// Get the legacy multiband presets folder path (presets/)
pub fn multiband_presets_folder(collection_path: &Path) -> PathBuf {
    collection_path.join(MULTIBAND_PRESETS_FOLDER)
}

/// Legacy: Get the preset file path for a given preset name (in legacy presets/ folder)
pub fn preset_file_path(collection_path: &Path, preset_name: &str) -> PathBuf {
    let sanitized = sanitize_filename(preset_name);
    multiband_presets_folder(collection_path).join(format!("{}.yaml", sanitized))
}

/// Legacy: List available preset names in the legacy presets folder
pub fn list_presets(collection_path: &Path) -> Vec<String> {
    list_yaml_files_in(&multiband_presets_folder(collection_path))
}

/// Legacy: Load a preset from the legacy presets/ folder
pub fn load_preset(collection_path: &Path, preset_name: &str) -> Result<StemPresetConfig, String> {
    let path = preset_file_path(collection_path, preset_name);
    log::info!("load_preset: Loading multiband preset from {:?}", path);

    if !path.exists() {
        return Err(format!("Preset '{}' not found", preset_name));
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_yaml::from_str::<StemPresetConfig>(&contents) {
            Ok(config) => {
                log::info!(
                    "load_preset: Loaded preset '{}' with {} bands, {} effects total",
                    config.name,
                    config.bands.len(),
                    config.bands.iter().map(|b| b.effects.len()).sum::<usize>()
                );
                Ok(config)
            }
            Err(e) => {
                let msg = format!("Failed to parse preset: {}", e);
                log::error!("load_preset: {}", msg);
                Err(msg)
            }
        },
        Err(e) => {
            let msg = format!("Failed to read preset file: {}", e);
            log::error!("load_preset: {}", msg);
            Err(msg)
        }
    }
}

/// Legacy: Save a preset to the legacy presets/ folder
pub fn save_preset(config: &StemPresetConfig, collection_path: &Path) -> Result<(), String> {
    let folder = multiband_presets_folder(collection_path);
    let path = preset_file_path(collection_path, &config.name);
    log::info!("save_preset: Saving multiband preset to {:?}", path);

    if let Err(e) = std::fs::create_dir_all(&folder) {
        let msg = format!("Failed to create presets directory: {}", e);
        log::error!("save_preset: {}", msg);
        return Err(msg);
    }

    let yaml = serde_yaml::to_string(config)
        .map_err(|e| {
            let msg = format!("Failed to serialize preset: {}", e);
            log::error!("save_preset: {}", msg);
            msg
        })?;

    std::fs::write(&path, yaml)
        .map_err(|e| {
            let msg = format!("Failed to write preset file: {}", e);
            log::error!("save_preset: {}", msg);
            msg
        })?;

    log::info!("save_preset: Saved preset '{}' successfully", config.name);
    Ok(())
}

/// Legacy: Delete a preset by name from the legacy presets/ folder
pub fn delete_preset(collection_path: &Path, preset_name: &str) -> Result<(), String> {
    let path = preset_file_path(collection_path, preset_name);
    log::info!("delete_preset: Deleting preset at {:?}", path);

    if !path.exists() {
        return Err(format!("Preset '{}' not found", preset_name));
    }

    std::fs::remove_file(&path)
        .map_err(|e| {
            let msg = format!("Failed to delete preset: {}", e);
            log::error!("delete_preset: {}", msg);
            msg
        })?;

    log::info!("delete_preset: Deleted preset '{}' successfully", preset_name);
    Ok(())
}

// ── Shared Helpers ───────────────────────────────────────────────────────────

/// List .yaml files in a directory, returning their names (without extension)
fn list_yaml_files_in(folder: &Path) -> Vec<String> {
    if !folder.exists() {
        return Vec::new();
    }

    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "yaml") {
                if let Some(stem) = path.file_stem() {
                    names.push(stem.to_string_lossy().to_string());
                }
            }
        }
    }
    names.sort();
    names
}
