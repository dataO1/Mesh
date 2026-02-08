//! Stem Preset Widget
//!
//! A lightweight preset selector for stem effect chains in mesh-player.
//! Provides:
//! - Preset dropdown selector
//! - 8 labeled macro knobs for real-time control
//!
//! This is a simplified interface compared to the full MultibandEditorState,
//! designed for performance use where editing is not needed.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  [Preset Name ▾]  or  [No Preset]                                       │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  [Macro 1] [Macro 2] [Macro 3] [Macro 4] [Macro 5] [Macro 6] [M7] [M8]  │
//! │    (named sliders from preset configuration)                            │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

mod message;
mod view;

pub use message::StemPresetMessage;
pub use view::stem_preset_view;

/// Number of macro knobs per preset
pub const NUM_MACROS: usize = 8;

/// Default macro names when no preset is loaded
pub const DEFAULT_MACRO_NAMES: [&str; 8] = [
    "Macro 1", "Macro 2", "Macro 3", "Macro 4",
    "Macro 5", "Macro 6", "Macro 7", "Macro 8",
];

/// State for a stem preset selector
///
/// Lightweight state for preset-based stem effects in mesh-player.
/// Unlike MultibandEditorState, this only stores the preset reference
/// and macro values - no editing capabilities.
#[derive(Debug, Clone)]
pub struct StemPresetState {
    /// Currently loaded preset name (None = passthrough/no effects)
    pub loaded_preset: Option<String>,

    /// Macro knob values [0.0-1.0]
    pub macro_values: [f32; NUM_MACROS],

    /// Macro names from the loaded preset (for display)
    pub macro_names: [String; NUM_MACROS],

    /// Available presets (cached from filesystem)
    pub available_presets: Vec<String>,

    /// Whether the preset picker dropdown is open
    pub picker_open: bool,
}

impl Default for StemPresetState {
    fn default() -> Self {
        Self::new()
    }
}

impl StemPresetState {
    /// Create a new stem preset state with defaults
    pub fn new() -> Self {
        Self {
            loaded_preset: None,
            macro_values: [0.5; NUM_MACROS], // Center position by default
            macro_names: DEFAULT_MACRO_NAMES.map(String::from),
            available_presets: Vec::new(),
            picker_open: false,
        }
    }

    /// Get the currently loaded preset name
    pub fn preset_name(&self) -> Option<&str> {
        self.loaded_preset.as_deref()
    }

    /// Check if a preset is loaded (not passthrough mode)
    pub fn has_preset(&self) -> bool {
        self.loaded_preset.is_some()
    }

    /// Get a macro value by index
    pub fn macro_value(&self, index: usize) -> f32 {
        self.macro_values.get(index).copied().unwrap_or(0.5)
    }

    /// Set a macro value by index
    pub fn set_macro_value(&mut self, index: usize, value: f32) {
        if index < NUM_MACROS {
            self.macro_values[index] = value.clamp(0.0, 1.0);
        }
    }

    /// Get a macro name by index
    pub fn macro_name(&self, index: usize) -> &str {
        self.macro_names
            .get(index)
            .map(String::as_str)
            .unwrap_or("?")
    }

    /// Set macro names (typically called after loading a preset)
    pub fn set_macro_names(&mut self, names: [String; NUM_MACROS]) {
        self.macro_names = names;
    }

    /// Set the available presets list
    pub fn set_available_presets(&mut self, presets: Vec<String>) {
        self.available_presets = presets;
    }

    /// Load a preset (sets name, resets macros to center)
    pub fn load_preset(&mut self, name: Option<String>) {
        self.loaded_preset = name;
        self.macro_values = [0.5; NUM_MACROS];
        self.picker_open = false;
        // Macro names will be set separately by the handler after loading the config
    }

    /// Clear the loaded preset (passthrough mode)
    pub fn clear_preset(&mut self) {
        self.loaded_preset = None;
        self.macro_values = [0.5; NUM_MACROS];
        self.macro_names = DEFAULT_MACRO_NAMES.map(String::from);
        self.picker_open = false;
    }

    /// Handle a message and update state
    ///
    /// Returns true if the message requires backend action (preset load or macro change)
    pub fn handle_message(&mut self, message: StemPresetMessage) -> bool {
        match message {
            StemPresetMessage::SelectPreset(name) => {
                if name.is_some() {
                    self.load_preset(name);
                } else {
                    self.clear_preset();
                }
                true // Requires backend action
            }
            StemPresetMessage::SetMacro { index, value } => {
                self.set_macro_value(index, value);
                true // Requires backend action
            }
            StemPresetMessage::TogglePicker => {
                self.picker_open = !self.picker_open;
                false
            }
            StemPresetMessage::ClosePicker => {
                self.picker_open = false;
                false
            }
            StemPresetMessage::RefreshPresets => {
                // Handler will update available_presets
                true
            }
            StemPresetMessage::SetAvailablePresets(presets) => {
                self.set_available_presets(presets);
                false
            }
            StemPresetMessage::SetMacroNames(names) => {
                self.set_macro_names(names);
                false
            }
        }
    }
}
