//! Messages for the multiband editor widget

use super::state::EffectChainLocation;
use crate::knob::KnobEvent;

/// Messages emitted by the multiband editor
#[derive(Debug, Clone)]
pub enum MultibandEditorMessage {
    // ─────────────────────────────────────────────────────────────────────
    // Modal control
    // ─────────────────────────────────────────────────────────────────────
    /// Open the editor for a specific deck and stem
    Open {
        deck: usize,
        stem: usize,
        stem_name: String,
    },

    /// Close the editor
    Close,

    // ─────────────────────────────────────────────────────────────────────
    // Pre-FX chain management (before multiband split)
    // ─────────────────────────────────────────────────────────────────────
    /// Open effect picker to add effect to pre-fx chain
    OpenPreFxEffectPicker,

    /// Effect selected for pre-fx chain
    PreFxEffectSelected { effect_id: String, source: String },

    /// Remove effect from pre-fx chain
    RemovePreFxEffect(usize),

    /// Toggle pre-fx effect bypass
    TogglePreFxBypass(usize),

    // ─────────────────────────────────────────────────────────────────────
    // Crossover control
    // ─────────────────────────────────────────────────────────────────────
    /// Start dragging a crossover divider
    StartDragCrossover(usize),

    /// Drag crossover to new frequency (in Hz)
    DragCrossover(f32),

    /// End crossover drag
    EndDragCrossover,

    // ─────────────────────────────────────────────────────────────────────
    // Band management
    // ─────────────────────────────────────────────────────────────────────
    /// Add a new band
    AddBand,

    /// Remove a band by index
    RemoveBand(usize),

    /// Set band mute state
    SetBandMute { band: usize, muted: bool },

    /// Set band solo state
    SetBandSolo { band: usize, soloed: bool },

    /// Set band gain (linear, 0.0-2.0)
    SetBandGain { band: usize, gain: f32 },

    // ─────────────────────────────────────────────────────────────────────
    // Band effect management
    // ─────────────────────────────────────────────────────────────────────
    /// Open effect picker to add effect to a band
    OpenEffectPicker(usize),

    /// Effect was selected from picker (band_index, effect_id, effect_source)
    EffectSelected {
        band: usize,
        effect_id: String,
        /// "pd", "clap", or "native"
        source: String,
    },

    /// Remove an effect from a band
    RemoveEffect { band: usize, effect: usize },

    /// Toggle effect bypass
    ToggleEffectBypass { band: usize, effect: usize },

    /// Select effect for parameter focus
    SelectEffect { location: EffectChainLocation, effect: usize },

    // ─────────────────────────────────────────────────────────────────────
    // Knob events (unified for all knobs)
    // ─────────────────────────────────────────────────────────────────────
    /// Macro knob event (index, event)
    MacroKnob { index: usize, event: KnobEvent },

    /// Effect parameter knob event (location, effect_idx, param_idx, event)
    EffectKnob {
        location: EffectChainLocation,
        effect: usize,
        param: usize,
        event: KnobEvent,
    },

    // ─────────────────────────────────────────────────────────────────────
    // Post-FX chain management (after band summation)
    // ─────────────────────────────────────────────────────────────────────
    /// Open effect picker to add effect to post-fx chain
    OpenPostFxEffectPicker,

    /// Effect selected for post-fx chain
    PostFxEffectSelected { effect_id: String, source: String },

    /// Remove effect from post-fx chain
    RemovePostFxEffect(usize),

    /// Toggle post-fx effect bypass
    TogglePostFxBypass(usize),

    // ─────────────────────────────────────────────────────────────────────
    // Macro mapping control (drag-to-map)
    // ─────────────────────────────────────────────────────────────────────
    /// Rename a macro
    RenameMacro { index: usize, name: String },

    /// Start dragging a macro for mapping
    StartDragMacro(usize),

    /// End macro drag (cancel or drop outside target)
    EndDragMacro,

    /// Drop macro onto an effect parameter (creates mapping)
    DropMacroOnParam {
        macro_index: usize,
        band: usize,
        effect: usize,
        param: usize,
    },

    /// Remove a macro mapping from a parameter
    RemoveParamMapping {
        band: usize,
        effect: usize,
        param: usize,
    },

    /// Open macro mapping dialog for a macro
    OpenMacroMapper(usize),

    /// Add a mapping from macro to effect parameter
    AddMacroMapping {
        macro_index: usize,
        band: usize,
        effect: usize,
        param: usize,
    },

    /// Clear all mappings for a macro
    ClearMacroMappings(usize),

    // ─────────────────────────────────────────────────────────────────────
    // Preset management
    // ─────────────────────────────────────────────────────────────────────
    /// Open preset browser (for loading)
    OpenPresetBrowser,

    /// Close preset browser
    ClosePresetBrowser,

    /// Open save dialog
    OpenSaveDialog,

    /// Close save dialog
    CloseSaveDialog,

    /// Update preset name input text
    SetPresetNameInput(String),

    /// Load a preset by name
    LoadPreset(String),

    /// Save current state as preset (uses preset_name_input)
    SavePreset,

    /// Delete a preset by name
    DeletePreset(String),

    /// Refresh available presets list
    RefreshPresets,

    /// Set available presets list (from handler after loading)
    SetAvailablePresets(Vec<String>),
}
