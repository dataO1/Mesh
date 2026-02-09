//! Deck Preset Widget
//!
//! A deck-level preset selector for mesh-player.
//! Provides:
//! - Deck preset dropdown selector (wraps 4 stem presets)
//! - 4 shared macro knobs that control parameters across all stems
//!
//! This replaces the per-stem preset selectors with a single deck-level
//! preset that references individual stem presets by name.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  [Deck Preset Name ▾]  or  [No Deck Preset]                            │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  [Macro 1] [Macro 2] [Macro 3] [Macro 4]                                │
//! │    (shared macro knobs controlling params across all stems)             │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

mod message;
mod view;

pub use message::DeckPresetMessage;
pub use view::deck_preset_view;

/// Number of shared macro knobs per deck
pub const NUM_MACROS: usize = 4;

/// Default macro names when no preset is loaded
pub const DEFAULT_MACRO_NAMES: [&str; NUM_MACROS] = [
    "Macro 1", "Macro 2", "Macro 3", "Macro 4",
];

/// A macro-to-parameter mapping for direct modulation
///
/// Stores all the information needed to compute and send modulated
/// parameter values directly, without relying on the engine's macro system.
/// Unlike the old per-stem version, this includes a `stem_index` so a single
/// macro can map to parameters across multiple stems.
#[derive(Debug, Clone)]
pub struct MacroParamMapping {
    /// Which stem this mapping applies to (0-3)
    pub stem_index: usize,
    /// Which chain the effect is in
    pub location: crate::multiband::EffectChainLocation,
    /// Which effect in the chain
    pub effect_index: usize,
    /// Which parameter on the effect (the actual CLAP param index)
    pub param_index: usize,
    /// Base value (normalized 0-1)
    pub base_value: f32,
    /// Bipolar offset range (-1 to +1)
    pub offset_range: f32,
}

impl MacroParamMapping {
    /// Compute the modulated parameter value for a given macro position
    ///
    /// Formula: result = base + (macro * 2 - 1) * offset_range
    /// - macro=0: result = base - offset_range
    /// - macro=0.5: result = base
    /// - macro=1: result = base + offset_range
    pub fn modulate(&self, macro_value: f32) -> f32 {
        let offset = (macro_value * 2.0 - 1.0) * self.offset_range;
        (self.base_value + offset).clamp(0.0, 1.0)
    }
}

/// State for a deck preset selector
///
/// Manages a single deck-level preset that wraps 4 stem presets and
/// provides shared macros. Unlike the old `StemPresetState` (one per stem),
/// there is one `DeckPresetState` per deck.
#[derive(Debug, Clone)]
pub struct DeckPresetState {
    /// Currently loaded deck preset name (None = no preset loaded)
    pub loaded_deck_preset: Option<String>,

    /// Shared macro knob values [0.0-1.0]
    pub macro_values: [f32; NUM_MACROS],

    /// Shared macro names from the loaded preset (for display)
    pub macro_names: [String; NUM_MACROS],

    /// Per-stem loaded preset name (from deck preset references)
    pub stem_preset_names: [Option<String>; 4],

    /// Per-stem macro-to-parameter mappings for direct modulation
    /// stem_macro_mappings[macro_idx] = Vec<MacroParamMapping> (can span multiple stems)
    pub macro_mappings: [Vec<MacroParamMapping>; NUM_MACROS],

    /// Available deck presets (cached from filesystem)
    pub available_deck_presets: Vec<String>,

    /// Available stem presets (cached from filesystem)
    pub available_stem_presets: Vec<String>,

    /// Whether the preset picker dropdown is open
    pub picker_open: bool,
}

impl Default for DeckPresetState {
    fn default() -> Self {
        Self::new()
    }
}

impl DeckPresetState {
    /// Create a new deck preset state with defaults
    pub fn new() -> Self {
        Self {
            loaded_deck_preset: None,
            macro_values: [0.5; NUM_MACROS], // Center position by default
            macro_names: DEFAULT_MACRO_NAMES.map(String::from),
            stem_preset_names: [None, None, None, None],
            macro_mappings: Default::default(),
            available_deck_presets: Vec::new(),
            available_stem_presets: Vec::new(),
            picker_open: false,
        }
    }

    /// Get the currently loaded deck preset name
    pub fn preset_name(&self) -> Option<&str> {
        self.loaded_deck_preset.as_deref()
    }

    /// Check if a deck preset is loaded
    pub fn has_preset(&self) -> bool {
        self.loaded_deck_preset.is_some()
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

    /// Set macro names (typically called after loading a deck preset)
    pub fn set_macro_names(&mut self, names: [String; NUM_MACROS]) {
        self.macro_names = names;
    }

    /// Set the available deck presets list
    pub fn set_available_deck_presets(&mut self, presets: Vec<String>) {
        self.available_deck_presets = presets;
    }

    /// Set the available stem presets list
    pub fn set_available_stem_presets(&mut self, presets: Vec<String>) {
        self.available_stem_presets = presets;
    }

    /// Load a deck preset (sets name, resets macros to center)
    pub fn load_deck_preset(&mut self, name: Option<String>) {
        self.loaded_deck_preset = name;
        self.macro_values = [0.5; NUM_MACROS];
        self.picker_open = false;
        self.macro_mappings = Default::default();
        self.stem_preset_names = [None, None, None, None];
        // Macro names, stem references, and mappings will be set separately by the handler
    }

    /// Clear the loaded deck preset (passthrough mode)
    pub fn clear_preset(&mut self) {
        self.loaded_deck_preset = None;
        self.macro_values = [0.5; NUM_MACROS];
        self.macro_names = DEFAULT_MACRO_NAMES.map(String::from);
        self.stem_preset_names = [None, None, None, None];
        self.picker_open = false;
        self.macro_mappings = Default::default();
    }

    /// Set macro mappings (called after loading preset config)
    pub fn set_macro_mappings(&mut self, mappings: [Vec<MacroParamMapping>; NUM_MACROS]) {
        self.macro_mappings = mappings;
    }

    /// Add a single macro mapping
    pub fn add_macro_mapping(&mut self, macro_index: usize, mapping: MacroParamMapping) {
        if macro_index < NUM_MACROS {
            self.macro_mappings[macro_index].push(mapping);
        }
    }

    /// Get the mappings for a specific macro
    pub fn mappings_for_macro(&self, macro_index: usize) -> &[MacroParamMapping] {
        self.macro_mappings.get(macro_index).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the mappings for a specific macro filtered to a single stem
    pub fn mappings_for_macro_on_stem(&self, macro_index: usize, stem_index: usize) -> Vec<&MacroParamMapping> {
        self.mappings_for_macro(macro_index)
            .iter()
            .filter(|m| m.stem_index == stem_index)
            .collect()
    }

    /// Handle a message and update state
    ///
    /// Returns true if the message requires backend action (preset load or macro change)
    pub fn handle_message(&mut self, message: DeckPresetMessage) -> bool {
        match message {
            DeckPresetMessage::SelectDeckPreset(name) => {
                if name.is_some() {
                    self.load_deck_preset(name);
                } else {
                    self.clear_preset();
                }
                true // Requires backend action
            }
            DeckPresetMessage::SetMacro { index, value } => {
                self.set_macro_value(index, value);
                true // Requires backend action (apply to all stems)
            }
            DeckPresetMessage::TogglePicker => {
                self.picker_open = !self.picker_open;
                false
            }
            DeckPresetMessage::ClosePicker => {
                self.picker_open = false;
                false
            }
            DeckPresetMessage::RefreshPresets => {
                // Handler will update available presets
                true
            }
            DeckPresetMessage::SetAvailableDeckPresets(presets) => {
                self.set_available_deck_presets(presets);
                false
            }
            DeckPresetMessage::SetAvailableStemPresets(presets) => {
                self.set_available_stem_presets(presets);
                false
            }
            DeckPresetMessage::SetMacroNames(names) => {
                self.set_macro_names(names);
                false
            }
        }
    }
}
