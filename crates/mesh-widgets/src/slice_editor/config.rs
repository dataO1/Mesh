//! Slicer configuration types for serialization
//!
//! These types provide a serializable representation of slicer presets
//! that can be stored in YAML config files and shared between mesh-cue and mesh-player.

use super::state::{SliceEditPreset, SliceEditSequence, SliceEditStep, SliceEditorState};
use serde::{Deserialize, Serialize};

/// Slicer configuration
///
/// Stores 8 presets, each containing patterns for 4 stems.
/// This is the serializable version of SliceEditorState.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlicerConfig {
    /// 8 preset patterns
    pub presets: [SlicerPresetConfig; 8],
    /// Default slicer buffer size in bars (1, 4, 8, or 16)
    ///
    /// Controls how many bars the 16 slices span:
    /// - 1 bar = 4 beats → 4 slices per beat (tighter cuts)
    /// - 4 bars = 16 beats → 1 slice per beat (standard)
    pub buffer_bars: u32,
}

impl Default for SlicerConfig {
    fn default() -> Self {
        Self {
            presets: std::array::from_fn(|_| SlicerPresetConfig::default()),
            buffer_bars: 1, // 1 bar = 4 beats
        }
    }
}

impl SlicerConfig {
    /// Get validated buffer bars (clamp to valid options: 1, 4, 8, 16)
    pub fn validated_buffer_bars(&self) -> u32 {
        match self.buffer_bars {
            1 | 4 | 8 | 16 => self.buffer_bars,
            _ => 1, // Default to 1 bar if invalid
        }
    }

    /// Convert from SliceEditorState (UI state → config for saving)
    /// Note: buffer_bars is preserved from the current config, not from editor state
    pub fn from_editor_state_with_buffer(state: &SliceEditorState, buffer_bars: u32) -> Self {
        Self {
            presets: std::array::from_fn(|i| SlicerPresetConfig::from_edit_preset(&state.presets[i])),
            buffer_bars,
        }
    }

    /// Convert from SliceEditorState using default buffer_bars
    pub fn from_editor_state(state: &SliceEditorState) -> Self {
        Self::from_editor_state_with_buffer(state, Self::default().buffer_bars)
    }

    /// Apply to SliceEditorState (config → UI state for loading)
    pub fn apply_to_editor_state(&self, state: &mut SliceEditorState) {
        for (i, preset_config) in self.presets.iter().enumerate() {
            preset_config.apply_to_edit_preset(&mut state.presets[i]);
        }
    }
}

/// A single slicer preset (patterns for 4 stems)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlicerPresetConfig {
    /// Per-stem patterns (None = default diagonal pattern)
    /// Index: [VOC=0, DRM=1, BAS=2, OTH=3]
    pub stems: [Option<SlicerSequenceConfig>; 4],
}

impl Default for SlicerPresetConfig {
    fn default() -> Self {
        Self {
            // All stems use default diagonal pattern
            stems: [None, None, None, None],
        }
    }
}

impl SlicerPresetConfig {
    fn from_edit_preset(preset: &SliceEditPreset) -> Self {
        Self {
            stems: std::array::from_fn(|i| {
                preset.stems[i].as_ref().map(SlicerSequenceConfig::from_edit_sequence)
            }),
        }
    }

    fn apply_to_edit_preset(&self, preset: &mut SliceEditPreset) {
        for (i, seq_config) in self.stems.iter().enumerate() {
            preset.stems[i] = seq_config.as_ref().map(|s| s.to_edit_sequence());
        }
    }
}

/// A single stem's slice sequence (16 steps)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlicerSequenceConfig {
    /// 16 steps, each with muted flag and active slices
    pub steps: [SlicerStepConfig; 16],
}

impl Default for SlicerSequenceConfig {
    fn default() -> Self {
        // Default: diagonal pattern (step N plays slice N)
        Self {
            steps: std::array::from_fn(|i| SlicerStepConfig {
                muted: false,
                slices: vec![i as u8],
            }),
        }
    }
}

impl SlicerSequenceConfig {
    fn from_edit_sequence(seq: &SliceEditSequence) -> Self {
        Self {
            steps: std::array::from_fn(|i| SlicerStepConfig {
                muted: seq.steps[i].muted,
                slices: seq.steps[i].active_slices.clone(),
            }),
        }
    }

    fn to_edit_sequence(&self) -> SliceEditSequence {
        SliceEditSequence {
            steps: std::array::from_fn(|i| SliceEditStep {
                muted: self.steps[i].muted,
                active_slices: self.steps[i].slices.clone(),
            }),
        }
    }
}

/// A single step in the slice sequence
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlicerStepConfig {
    /// Whether this step is muted
    #[serde(default)]
    pub muted: bool,
    /// Active slices at this step (0-15)
    #[serde(default)]
    pub slices: Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared Presets File I/O
// ─────────────────────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};

/// Default presets filename within the collection folder
pub const SLICER_PRESETS_FILENAME: &str = "slicer-presets.yaml";

/// Get the slicer presets file path for a collection
///
/// Returns: {collection_path}/slicer-presets.yaml
pub fn slicer_presets_path(collection_path: &Path) -> PathBuf {
    collection_path.join(SLICER_PRESETS_FILENAME)
}

/// Load slicer presets from the shared presets file
///
/// Both mesh-cue and mesh-player use this file for slicer preset persistence.
/// If the file doesn't exist or is invalid, returns default config.
pub fn load_slicer_presets(collection_path: &Path) -> SlicerConfig {
    let path = slicer_presets_path(collection_path);
    log::info!("load_slicer_presets: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_slicer_presets: File doesn't exist, using defaults");
        return SlicerConfig::default();
    }

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_yaml::from_str::<SlicerConfig>(&contents) {
            Ok(config) => {
                log::info!(
                    "load_slicer_presets: Loaded {} presets, buffer_bars: {}",
                    config.presets.len(),
                    config.buffer_bars
                );
                config
            }
            Err(e) => {
                log::warn!("load_slicer_presets: Failed to parse: {}, using defaults", e);
                SlicerConfig::default()
            }
        },
        Err(e) => {
            log::warn!("load_slicer_presets: Failed to read: {}, using defaults", e);
            SlicerConfig::default()
        }
    }
}

/// Save slicer presets to the shared presets file
///
/// Both mesh-cue and mesh-player use this file for slicer preset persistence.
/// Creates the collection directory if it doesn't exist.
pub fn save_slicer_presets(config: &SlicerConfig, collection_path: &Path) -> Result<(), String> {
    let path = slicer_presets_path(collection_path);
    log::info!("save_slicer_presets: Saving to {:?}", path);

    // Ensure collection directory exists
    if let Err(e) = std::fs::create_dir_all(collection_path) {
        let msg = format!("Failed to create collection directory: {}", e);
        log::error!("save_slicer_presets: {}", msg);
        return Err(msg);
    }

    // Serialize to YAML
    let yaml = match serde_yaml::to_string(config) {
        Ok(y) => y,
        Err(e) => {
            let msg = format!("Failed to serialize presets: {}", e);
            log::error!("save_slicer_presets: {}", msg);
            return Err(msg);
        }
    };

    // Write to file
    if let Err(e) = std::fs::write(&path, yaml) {
        let msg = format!("Failed to write presets file: {}", e);
        log::error!("save_slicer_presets: {}", msg);
        return Err(msg);
    }

    log::info!("save_slicer_presets: Saved successfully");
    Ok(())
}
