//! Messages for the deck preset widget

use super::NUM_MACROS;

/// Messages emitted by the deck preset selector
#[derive(Debug, Clone)]
pub enum DeckPresetMessage {
    /// Select a deck preset by name (None = clear all stems)
    SelectDeckPreset(Option<String>),

    /// Set a shared macro knob value (index 0-3, value 0.0-1.0)
    SetMacro { index: usize, value: f32 },

    /// Toggle the deck preset picker dropdown
    TogglePicker,

    /// Close the deck preset picker dropdown
    ClosePicker,

    /// Refresh the available presets lists (deck + stem)
    RefreshPresets,

    /// Set the available deck presets list (from handler after loading)
    SetAvailableDeckPresets(Vec<String>),

    /// Set the available stem presets list (from handler after loading)
    SetAvailableStemPresets(Vec<String>),

    /// Set macro names (from handler after preset load)
    SetMacroNames([String; NUM_MACROS]),
}
