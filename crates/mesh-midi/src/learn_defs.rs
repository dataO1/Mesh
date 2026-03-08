//! Data model types for the MIDI learn tree wizard.
//!
//! These are pure data types used to define what can be mapped.
//! The static catalog in `learn_catalog.rs` uses these to declare
//! all mappable actions. The runtime tree builder in mesh-player
//! expands these into concrete tree nodes based on the user's
//! topology configuration.
//!
//! All types are `Send + Sync + 'static` — no UI dependency.

use crate::config::{ControlBehavior, PadModeSource};

/// Expected physical control type for a mapping slot.
///
/// Used to validate and display what kind of control the user should assign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlType {
    /// Physical button (Note On/Off)
    Button,
    /// Rotary encoder (relative CC, infinite rotation)
    Encoder,
    /// Rotary potentiometer (absolute CC 0-127)
    Knob,
    /// Linear fader (absolute CC 0-127)
    Fader,
}

/// How a section repeats across the controller.
///
/// The tree builder creates N concrete instances of a section
/// based on the topology configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    /// Single instance (e.g., Global Controls, Navigation)
    Once,
    /// One per physical deck (deck_count instances: 2 or 4)
    PerPhysicalDeck,
    /// One per virtual deck / mixer channel (num_mixer_channels: 2 or 4)
    PerVirtualDeck,
}

/// Conditions that control visibility of sections or individual mappings.
///
/// Checked against `TopologyConfig::is_visible()` when building
/// the runtime tree. Invisible nodes are omitted entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Always visible regardless of configuration
    Always,
    /// Only visible when has_layer_toggle is true
    LayerToggleOnly,
    /// Only visible when pad_mode_source == Controller
    ControllerPadModeOnly,
    /// Only visible when there are 4 virtual decks (any topology yielding 4 channels)
    FourDeckOnly,
    /// Only visible when there are 4 physical decks (not counting layer toggle virtual decks)
    FourPhysicalDeckOnly,
}

/// Definition of a single mappable action (leaf node in the tree).
///
/// This is the single source of truth for each mapping. The tree builder,
/// config generator, and verification window all derive from these definitions.
#[derive(Debug, Clone, Copy)]
pub struct MappingDef {
    /// Unique identifier, e.g., "transport.play"
    pub id: &'static str,
    /// Display label shown in the tree, e.g., "Play"
    pub label: &'static str,
    /// Short explanation shown when cursor is on this mapping
    pub description: &'static str,
    /// Config action string written to YAML, e.g., "deck.play"
    pub action: &'static str,
    /// Expected hardware control type
    pub control_type: ControlType,
    /// Default control behavior for this action
    pub behavior: ControlBehavior,
    /// Feedback state name for LED output (e.g., "deck.is_playing"), None if no LED feedback
    pub feedback_state: Option<&'static str>,
    /// Parameter key for parameterized mappings (e.g., "slot", "stem", "macro", "pad")
    pub param_key: Option<&'static str>,
    /// Parameter value (e.g., pad index 0-7, stem index 0-3)
    pub param_value: Option<usize>,
    /// Whether this mapping uses physical_deck (layer-resolved) vs deck_index (direct)
    pub uses_physical_deck: bool,
    /// Visibility condition — when false for the current topology, this node is hidden
    pub visibility: Visibility,
    /// Mode condition for momentary overlays (e.g., "hot_cue", "slicer", "browse")
    pub mode_condition: Option<&'static str>,
}

/// Definition of a tree section (collapsible group of mappings).
///
/// Sections can repeat per-deck using `repeat_mode`. The tree builder
/// creates N concrete instances (e.g., "Transport — Deck 1", "Transport — Deck 2").
#[derive(Debug, Clone, Copy)]
pub struct SectionDef {
    /// Unique identifier, e.g., "transport"
    pub id: &'static str,
    /// Display label template, e.g., "Transport"
    /// For repeated sections, the tree builder appends " — Deck N"
    pub label: &'static str,
    /// Short explanation shown below the section header
    pub description: &'static str,
    /// How this section repeats across decks
    pub repeat_mode: RepeatMode,
    /// Visibility condition for the entire section
    pub visibility: Visibility,
    /// Leaf mapping nodes in this section
    pub mappings: &'static [MappingDef],
}

/// Topology configuration derived from the pre-tree setup.
///
/// Determines the shape of the runtime tree: how many deck instances,
/// which sections/mappings are visible, etc.
#[derive(Debug, Clone)]
pub struct TopologyConfig {
    /// Number of physical decks (2 or 4)
    pub deck_count: usize,
    /// Whether controller has layer toggle buttons (2 physical → 4 virtual)
    pub has_layer_toggle: bool,
    /// How pad button actions are determined
    pub pad_mode_source: PadModeSource,
}

impl TopologyConfig {
    /// Number of virtual decks / mixer channels.
    /// With layer toggle: 2 physical × 2 layers = 4 channels.
    pub fn num_mixer_channels(&self) -> usize {
        if self.has_layer_toggle {
            self.deck_count * 2
        } else {
            self.deck_count
        }
    }

    /// Number of physical deck repetitions (same as deck_count).
    pub fn physical_deck_count(&self) -> usize {
        self.deck_count
    }

    /// How many instances a section creates for a given repeat mode.
    pub fn repeat_count(&self, mode: RepeatMode) -> usize {
        match mode {
            RepeatMode::Once => 1,
            RepeatMode::PerPhysicalDeck => self.deck_count,
            RepeatMode::PerVirtualDeck => self.num_mixer_channels(),
        }
    }

    /// Check whether a visibility condition is satisfied.
    pub fn is_visible(&self, vis: Visibility) -> bool {
        match vis {
            Visibility::Always => true,
            Visibility::LayerToggleOnly => self.has_layer_toggle,
            Visibility::ControllerPadModeOnly => {
                self.pad_mode_source == PadModeSource::Controller
            }
            Visibility::FourDeckOnly => self.num_mixer_channels() >= 4,
            Visibility::FourPhysicalDeckOnly => self.deck_count >= 4,
        }
    }
}

impl Default for TopologyConfig {
    fn default() -> Self {
        Self {
            deck_count: 2,
            has_layer_toggle: false,
            pad_mode_source: PadModeSource::default(),
        }
    }
}
