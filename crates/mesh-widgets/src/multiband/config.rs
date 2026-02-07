//! Multiband preset configuration types for serialization
//!
//! These types provide a serializable representation of multiband effect presets
//! that can be stored in YAML config files.

use super::state::{BandUiState, EffectSourceType, EffectUiState, MacroUiState, MultibandEditorState, ParamMacroMapping};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default presets folder name within the collection folder
pub const MULTIBAND_PRESETS_FOLDER: &str = "multiband-presets";

/// Multiband preset configuration
///
/// Stores the complete multiband configuration including:
/// - Pre-FX chain (before multiband split)
/// - Crossover frequencies
/// - Band configurations with effects and their macro mappings
/// - Post-FX chain (after band summation)
/// - Macro knob names
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MultibandPresetConfig {
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
    /// Macro knob configurations
    pub macros: Vec<MacroPresetConfig>,
}

impl Default for MultibandPresetConfig {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            pre_fx: Vec::new(),
            crossover_freqs: Vec::new(),
            bands: vec![BandPresetConfig::default()],
            post_fx: Vec::new(),
            macros: (0..8).map(|i| MacroPresetConfig {
                name: format!("Macro {}", i + 1),
            }).collect(),
        }
    }
}

impl MultibandPresetConfig {
    /// Create from MultibandEditorState
    pub fn from_editor_state(state: &MultibandEditorState, name: &str) -> Self {
        Self {
            name: name.to_string(),
            pre_fx: state.pre_fx.iter().map(EffectPresetConfig::from_effect_state).collect(),
            crossover_freqs: state.crossover_freqs.clone(),
            bands: state.bands.iter().map(BandPresetConfig::from_band_state).collect(),
            post_fx: state.post_fx.iter().map(EffectPresetConfig::from_effect_state).collect(),
            macros: state.macros.iter().map(MacroPresetConfig::from_macro_state).collect(),
        }
    }

    /// Apply to MultibandEditorState
    pub fn apply_to_editor_state(&self, state: &mut MultibandEditorState) {
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

        // Apply macro names
        for (i, macro_config) in self.macros.iter().enumerate() {
            if let Some(macro_state) = state.macros.get_mut(i) {
                macro_state.name = macro_config.name.clone();
            }
        }

        // Update solo state
        state.any_soloed = state.bands.iter().any(|b| b.soloed);
    }
}

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
}

impl Default for BandPresetConfig {
    fn default() -> Self {
        Self {
            gain: 1.0,
            muted: false,
            soloed: false,
            effects: Vec::new(),
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
        }
    }

    fn to_band_state(&self, index: usize) -> BandUiState {
        let mut band = BandUiState::new(index, super::FREQ_MIN, super::FREQ_MAX);
        band.gain = self.gain;
        band.muted = self.muted;
        band.soloed = self.soloed;
        band.effects = self.effects.iter().map(|e| e.to_effect_state()).collect();
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
    /// Parameter names
    pub param_names: Vec<String>,
    /// Parameter values (normalized 0.0-1.0)
    pub param_values: Vec<f32>,
    /// Macro mappings for each parameter
    pub param_mappings: Vec<ParamMappingConfig>,
}

impl EffectPresetConfig {
    fn from_effect_state(effect: &EffectUiState) -> Self {
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
            param_names: effect.param_names.clone(),
            param_values: effect.param_values.clone(),
            param_mappings: effect.param_mappings.iter().map(ParamMappingConfig::from_mapping).collect(),
        }
    }

    fn to_effect_state(&self) -> EffectUiState {
        use super::state::{AvailableParam, KnobAssignment, MAX_UI_KNOBS};

        let source = match self.source.as_str() {
            "pd" => EffectSourceType::Pd,
            "clap" => EffectSourceType::Clap,
            _ => EffectSourceType::Native,
        };

        // Convert legacy param_names to available_params
        let available_params: Vec<AvailableParam> = self
            .param_names
            .iter()
            .enumerate()
            .map(|(i, name)| AvailableParam {
                name: name.clone(),
                min: 0.0,
                max: 1.0,
                default: self.param_values.get(i).copied().unwrap_or(0.5),
                unit: String::new(),
            })
            .collect();

        // Create knob assignments from legacy values
        let mut knob_assignments: [KnobAssignment; MAX_UI_KNOBS] = Default::default();
        for (i, assignment) in knob_assignments
            .iter_mut()
            .enumerate()
            .take(self.param_values.len().min(MAX_UI_KNOBS))
        {
            assignment.param_index = Some(i);
            assignment.value = self.param_values.get(i).copied().unwrap_or(0.5);
            if let Some(mapping) = self.param_mappings.get(i) {
                assignment.macro_mapping = Some(mapping.to_mapping());
            }
        }

        EffectUiState {
            id: self.id.clone(),
            name: self.name.clone(),
            category: self.category.clone(),
            source,
            bypassed: self.bypassed,
            gui_open: false,
            available_params,
            knob_assignments,
            param_names: self.param_names.clone(),
            param_values: self.param_values.clone(),
            param_mappings: self.param_mappings.iter().map(|m| m.to_mapping()).collect(),
        }
    }
}

/// Macro mapping configuration for preset
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamMappingConfig {
    /// Which macro (0-7) controls this param, None if unmapped
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
}

impl MacroPresetConfig {
    fn from_macro_state(macro_state: &MacroUiState) -> Self {
        Self {
            name: macro_state.name.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset File I/O
// ─────────────────────────────────────────────────────────────────────────────

/// Get the multiband presets folder path for a collection
pub fn multiband_presets_folder(collection_path: &Path) -> PathBuf {
    collection_path.join(MULTIBAND_PRESETS_FOLDER)
}

/// Get the preset file path for a given preset name
pub fn preset_file_path(collection_path: &Path, preset_name: &str) -> PathBuf {
    let sanitized = sanitize_filename(preset_name);
    multiband_presets_folder(collection_path).join(format!("{}.yaml", sanitized))
}

/// Sanitize a preset name for use as filename
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// List available preset names in the collection
pub fn list_presets(collection_path: &Path) -> Vec<String> {
    let folder = multiband_presets_folder(collection_path);
    if !folder.exists() {
        return Vec::new();
    }

    let mut presets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "yaml") {
                if let Some(stem) = path.file_stem() {
                    presets.push(stem.to_string_lossy().to_string());
                }
            }
        }
    }
    presets.sort();
    presets
}

/// Load a multiband preset from file
pub fn load_preset(collection_path: &Path, preset_name: &str) -> Result<MultibandPresetConfig, String> {
    let path = preset_file_path(collection_path, preset_name);
    log::info!("load_preset: Loading multiband preset from {:?}", path);

    if !path.exists() {
        return Err(format!("Preset '{}' not found", preset_name));
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_yaml::from_str::<MultibandPresetConfig>(&contents) {
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

/// Save a multiband preset to file
pub fn save_preset(config: &MultibandPresetConfig, collection_path: &Path) -> Result<(), String> {
    let folder = multiband_presets_folder(collection_path);
    let path = preset_file_path(collection_path, &config.name);
    log::info!("save_preset: Saving multiband preset to {:?}", path);

    // Ensure presets directory exists
    if let Err(e) = std::fs::create_dir_all(&folder) {
        let msg = format!("Failed to create presets directory: {}", e);
        log::error!("save_preset: {}", msg);
        return Err(msg);
    }

    // Serialize to YAML
    let yaml = match serde_yaml::to_string(config) {
        Ok(y) => y,
        Err(e) => {
            let msg = format!("Failed to serialize preset: {}", e);
            log::error!("save_preset: {}", msg);
            return Err(msg);
        }
    };

    // Write to file
    if let Err(e) = std::fs::write(&path, yaml) {
        let msg = format!("Failed to write preset file: {}", e);
        log::error!("save_preset: {}", msg);
        return Err(msg);
    }

    log::info!("save_preset: Saved preset '{}' successfully", config.name);
    Ok(())
}

/// Delete a preset by name
pub fn delete_preset(collection_path: &Path, preset_name: &str) -> Result<(), String> {
    let path = preset_file_path(collection_path, preset_name);
    log::info!("delete_preset: Deleting preset at {:?}", path);

    if !path.exists() {
        return Err(format!("Preset '{}' not found", preset_name));
    }

    if let Err(e) = std::fs::remove_file(&path) {
        let msg = format!("Failed to delete preset: {}", e);
        log::error!("delete_preset: {}", msg);
        return Err(msg);
    }

    log::info!("delete_preset: Deleted preset '{}' successfully", preset_name);
    Ok(())
}
