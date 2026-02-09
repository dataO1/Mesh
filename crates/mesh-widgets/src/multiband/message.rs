//! Messages for the multiband editor widget

use super::state::EffectChainLocation;
use crate::knob::KnobEvent;

/// Target for chain-level dry/wet control
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChainTarget {
    /// Pre-FX chain (before multiband split)
    PreFx,
    /// A specific band's effect chain
    Band(usize),
    /// Post-FX chain (after band summation)
    PostFx,
}

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
        location: EffectChainLocation,
        effect: usize,
        param: usize,
    },

    /// Remove a macro mapping from a parameter
    RemoveParamMapping {
        location: EffectChainLocation,
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
    // Macro Modulation Range Controls
    // ─────────────────────────────────────────────────────────────────────
    /// Start dragging a modulation range indicator
    StartDragModRange {
        macro_index: usize,
        mapping_idx: usize,
    },

    /// Update modulation range while dragging (-1.0 to 1.0)
    DragModRange {
        macro_index: usize,
        mapping_idx: usize,
        new_offset_range: f32,
    },

    /// End modulation range drag
    EndDragModRange,

    /// Hover over modulation indicator (highlights target param)
    HoverModRange {
        macro_index: usize,
        mapping_idx: usize,
    },

    /// Stop hovering over modulation indicator
    UnhoverModRange,

    /// Hover over parameter knob (highlights mapped macro button)
    HoverParam {
        location: EffectChainLocation,
        effect: usize,
        param: usize,
    },

    /// Stop hovering over parameter knob
    UnhoverParam,

    // ─────────────────────────────────────────────────────────────────────
    // Dry/Wet Mix Controls
    // ─────────────────────────────────────────────────────────────────────
    /// Set per-effect dry/wet mix
    SetEffectDryWet {
        location: EffectChainLocation,
        effect: usize,
        mix: f32,
    },

    /// Effect dry/wet knob event
    EffectDryWetKnob {
        location: EffectChainLocation,
        effect: usize,
        event: KnobEvent,
    },

    /// Set pre-fx chain dry/wet mix
    SetPreFxChainDryWet(f32),

    /// Pre-fx chain dry/wet knob event
    PreFxChainDryWetKnob(KnobEvent),

    /// Set band chain dry/wet mix
    SetBandChainDryWet { band: usize, mix: f32 },

    /// Band chain dry/wet knob event
    BandChainDryWetKnob { band: usize, event: KnobEvent },

    /// Set post-fx chain dry/wet mix
    SetPostFxChainDryWet(f32),

    /// Post-fx chain dry/wet knob event
    PostFxChainDryWetKnob(KnobEvent),

    /// Set global dry/wet mix
    SetGlobalDryWet(f32),

    /// Global dry/wet knob event
    GlobalDryWetKnob(KnobEvent),

    /// Drop macro on effect dry/wet
    DropMacroOnEffectDryWet {
        macro_index: usize,
        location: EffectChainLocation,
        effect: usize,
    },

    /// Drop macro on chain dry/wet
    DropMacroOnChainDryWet {
        macro_index: usize,
        chain: ChainTarget,
    },

    /// Drop macro on global dry/wet
    DropMacroOnGlobalDryWet { macro_index: usize },

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

    // ─────────────────────────────────────────────────────────────────────
    // Parameter picker (knob-to-param assignment)
    // ─────────────────────────────────────────────────────────────────────
    /// Open parameter picker for a specific knob
    OpenParamPicker {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
    },

    /// Close parameter picker without making a selection
    CloseParamPicker,

    /// Assign a parameter to a knob (None clears the assignment)
    AssignParam {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
        param_index: Option<usize>,
    },

    /// Update param picker search filter text
    SetParamPickerFilter(String),

    // ─────────────────────────────────────────────────────────────────────
    // CLAP Plugin GUI Learning Mode
    // ─────────────────────────────────────────────────────────────────────
    /// Open the plugin GUI for a CLAP effect
    OpenPluginGui {
        location: EffectChainLocation,
        effect: usize,
    },

    /// Close the plugin GUI for an effect
    ClosePluginGui {
        location: EffectChainLocation,
        effect: usize,
    },

    /// Start learning mode for a knob - next plugin GUI interaction assigns the param
    StartLearning {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
    },

    /// Cancel learning mode
    CancelLearning,

    /// A parameter was learned from plugin GUI interaction
    /// (emitted by the handler when a param change is detected from plugin GUI)
    ParamLearned {
        location: EffectChainLocation,
        effect: usize,
        knob: usize,
        /// The CLAP parameter ID that was learned
        param_id: u32,
        /// The parameter name
        param_name: String,
    },

    // ─────────────────────────────────────────────────────────────────────
    // Global mouse events (for knob drag capture)
    // ─────────────────────────────────────────────────────────────────────
    /// Global mouse moved (for active knob drag)
    GlobalMouseMoved(iced::Point),

    /// Global mouse released (ends any active knob drag)
    GlobalMouseReleased,
}
