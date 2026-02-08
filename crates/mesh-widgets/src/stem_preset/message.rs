//! Messages for the stem preset widget

/// Messages emitted by the stem preset selector
#[derive(Debug, Clone)]
pub enum StemPresetMessage {
    /// Select a preset by name (None = passthrough/no effects)
    SelectPreset(Option<String>),

    /// Set a macro knob value (index 0-7, value 0.0-1.0)
    SetMacro { index: usize, value: f32 },

    /// Toggle the preset picker dropdown
    TogglePicker,

    /// Close the preset picker dropdown
    ClosePicker,

    /// Refresh the available presets list
    RefreshPresets,

    /// Set the available presets list (from handler after loading)
    SetAvailablePresets(Vec<String>),

    /// Set macro names (from handler after preset load)
    SetMacroNames([String; 8]),
}
