//! Messages for the multiband editor widget

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
    // Effect management
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
    SelectEffect { band: usize, effect: usize },

    /// Set effect parameter value
    SetEffectParam {
        band: usize,
        effect: usize,
        param: usize,
        value: f32,
    },

    // ─────────────────────────────────────────────────────────────────────
    // Macro control
    // ─────────────────────────────────────────────────────────────────────
    /// Set macro knob value
    SetMacro { index: usize, value: f32 },

    /// Rename a macro
    RenameMacro { index: usize, name: String },

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
    /// Open preset browser
    OpenPresetBrowser,

    /// Close preset browser
    ClosePresetBrowser,

    /// Load a preset by name
    LoadPreset(String),

    /// Save current state as preset with given name
    SavePreset(String),

    /// Delete a preset by name
    DeletePreset(String),

    /// Refresh available presets list
    RefreshPresets,
}
