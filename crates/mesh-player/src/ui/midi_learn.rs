//! MIDI Learn Mode — Tree-based mapping system
//!
//! Replaces the linear phase-based wizard with a collapsible tree:
//! - Browse encoder/press mapped first for controller navigation
//! - Topology setup (deck count, layer toggle, compact mode)
//! - Collapsible tree with sections for each mapping category
//! - Live mapping: once mapped, controls execute their action
//! - Verification window before saving

use std::collections::HashMap;
use std::time::{Duration, Instant};
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_widgets::sz;
use mesh_midi::{
    ControlAddress, ControlBehavior, ControlMapping, DeckTargetConfig,
    DeviceProfile, FeedbackMapping, HardwareType, MidiConfig,
    MidiSampleBuffer, PadModeSource, ShiftButtonConfig,
};
use mesh_midi::learn_defs::{ControlType, MappingDef, TopologyConfig};
use crate::ui::midi_learn_tree::{
    FlatNodeType, LearnTree, LogStatus, MappedControl, MappingStatus, TreeNode,
};

/// Debounce duration for MIDI capture (prevents release/encoder spam from double-mapping)
const CAPTURE_DEBOUNCE: Duration = Duration::from_millis(1000);

// ============================================================================
// Phase / Mode
// ============================================================================

/// Current phase of the MIDI learn workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LearnMode {
    /// Map browse encoder and press first (before any questions)
    #[default]
    NavCapture,
    /// Topology setup questions (deck count, compact mode, pad mode)
    Setup,
    /// Main tree view — browse and map controls
    TreeNavigation,
    /// Verification window — review changes before saving
    Verification,
}

/// Topology configuration choice (combines deck count + layer toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyChoice {
    TwoDecks,
    TwoDecksLayer,
    FourDecks,
}

impl Default for TopologyChoice {
    fn default() -> Self {
        Self::TwoDecks
    }
}

impl TopologyChoice {
    /// Human-readable label for UI display.
    pub fn label(&self) -> &'static str {
        match self {
            Self::TwoDecks => "2 Decks",
            Self::TwoDecksLayer => "2 Decks + Layer Toggle",
            Self::FourDecks => "4 Decks",
        }
    }

    /// Description of what this topology means.
    pub fn description(&self) -> &'static str {
        match self {
            Self::TwoDecks => "Two independent deck sections, one per side.",
            Self::TwoDecksLayer => "Two physical sections that switch between 4 virtual decks via layer toggle buttons.",
            Self::FourDecks => "Four independent deck sections — one physical control set per deck.",
        }
    }

    /// Convert to TopologyConfig for tree building.
    pub fn to_topology(
        &self,
        compact_mode: bool,
        pad_mode_source: PadModeSource,
    ) -> TopologyConfig {
        let (deck_count, has_layer_toggle) = match self {
            Self::TwoDecks => (2, false),
            Self::TwoDecksLayer => (2, true),
            Self::FourDecks => (4, false),
        };
        TopologyConfig {
            deck_count,
            has_layer_toggle,
            compact_mode,
            pad_mode_source,
        }
    }

    /// All choices as a static array (for iteration).
    pub const ALL: [TopologyChoice; 3] = [
        Self::TwoDecks,
        Self::TwoDecksLayer,
        Self::FourDecks,
    ];
}

// ============================================================================
// Highlight Target
// ============================================================================

/// UI element to highlight during learning.
///
/// Used to draw a red border around the control being mapped, so the user
/// knows which physical control to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightTarget {
    // Transport controls
    DeckPlay(usize),
    DeckCue(usize),
    DeckLoop(usize),
    DeckLoopEncoder(usize),
    DeckLoopIn(usize),
    DeckLoopOut(usize),
    DeckBeatJumpBack(usize),
    DeckBeatJumpForward(usize),
    DeckSlip(usize),
    DeckKeyMatch(usize),

    // Mode buttons (per-deck)
    DeckHotCueMode(usize),
    DeckSlicerMode(usize),

    // Per-side mode buttons (4-deck compact: side 0=left, 1=right)
    SideHotCueMode(usize),
    SideSlicerMode(usize),
    SideBrowseMode(usize),

    // Performance pads (deck, slot)
    DeckHotCue(usize, usize),
    DeckSlicerPad(usize, usize),
    DeckSlicerReset(usize),

    // Stem controls (deck, stem_index 0-3)
    DeckStemMute(usize, usize),
    DeckStemSolo(usize, usize),
    DeckStemLink(usize, usize),

    // Mixer controls (channel)
    MixerVolume(usize),
    MixerFilter(usize),
    MixerEqHi(usize),
    MixerEqMid(usize),
    MixerEqLo(usize),
    MixerCue(usize),
    MixerCrossfader,

    // Master section
    MasterVolume,
    CueVolume,
    CueMix,
    BpmSlider,

    // Browser
    BrowserEncoder,
    BrowserSelect,
    BrowserEncoderDeck(usize),
    BrowserSelectDeck(usize),

    // FX preset browsing
    FxEncoder,
    FxSelect,

    // FX macro knobs (deck_index, macro_index 0-3)
    DeckFxMacro(usize, usize),

    // Deck load buttons
    DeckLoad(usize),

    // Suggestion energy direction slider (one per side: 0=left, 1=right)
    SuggestionEnergy(usize),

    // Settings toggle button (global)
    SettingsButton,
}

impl HighlightTarget {
    /// Get human-readable description for the UI prompt.
    pub fn description(&self) -> String {
        match self {
            HighlightTarget::DeckPlay(d) => format!("Press PLAY button on deck {}", d + 1),
            HighlightTarget::DeckCue(d) => format!("Press CUE button on deck {}", d + 1),
            HighlightTarget::DeckLoop(d) => format!("Press LOOP toggle on deck {}", d + 1),
            HighlightTarget::DeckLoopEncoder(d) => {
                format!("Turn LOOP SIZE encoder on deck {} (halve/double)", d + 1)
            }
            HighlightTarget::DeckLoopIn(d) => format!("Press LOOP IN button on deck {}", d + 1),
            HighlightTarget::DeckLoopOut(d) => format!("Press LOOP OUT button on deck {}", d + 1),
            HighlightTarget::DeckBeatJumpBack(d) => {
                format!("Press BEAT JUMP BACK on deck {}", d + 1)
            }
            HighlightTarget::DeckBeatJumpForward(d) => {
                format!("Press BEAT JUMP FORWARD on deck {}", d + 1)
            }
            HighlightTarget::DeckSlip(d) => format!("Press SLIP button on deck {}", d + 1),
            HighlightTarget::DeckKeyMatch(d) => {
                format!("Press KEY MATCH button on deck {}", d + 1)
            }
            HighlightTarget::DeckHotCueMode(d) => {
                format!("Press HOT CUE mode button on deck {}", d + 1)
            }
            HighlightTarget::DeckSlicerMode(d) => {
                format!("Press SLICER mode button on deck {}", d + 1)
            }
            HighlightTarget::SideHotCueMode(side) => {
                let s = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side HOT CUE mode button", s)
            }
            HighlightTarget::SideSlicerMode(side) => {
                let s = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side SLICER mode button", s)
            }
            HighlightTarget::SideBrowseMode(side) => {
                let s = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side BROWSE mode button (toggle)", s)
            }
            HighlightTarget::DeckHotCue(d, s) => {
                format!("Press HOT CUE pad {} on deck {}", s + 1, d + 1)
            }
            HighlightTarget::DeckSlicerPad(d, s) => {
                format!("Press SLICER pad {} on deck {}", s + 1, d + 1)
            }
            HighlightTarget::DeckSlicerReset(d) => {
                format!("Press SLICER RESET on deck {}", d + 1)
            }
            HighlightTarget::DeckStemMute(d, s) => {
                let name = ["VOCALS", "DRUMS", "BASS", "OTHER"][*s];
                format!("Press {} mute on deck {}", name, d + 1)
            }
            HighlightTarget::DeckStemSolo(d, s) => {
                let name = ["VOCALS", "DRUMS", "BASS", "OTHER"][*s];
                format!("Press {} solo on deck {}", name, d + 1)
            }
            HighlightTarget::DeckStemLink(d, s) => {
                let name = ["VOCALS", "DRUMS", "BASS", "OTHER"][*s];
                format!("Press {} link on deck {}", name, d + 1)
            }
            HighlightTarget::MixerVolume(ch) => {
                format!("Move VOLUME fader on channel {}", ch + 1)
            }
            HighlightTarget::MixerFilter(ch) => {
                format!("Turn FILTER knob on channel {}", ch + 1)
            }
            HighlightTarget::MixerEqHi(ch) => {
                format!("Turn EQ HIGH knob on channel {}", ch + 1)
            }
            HighlightTarget::MixerEqMid(ch) => {
                format!("Turn EQ MID knob on channel {}", ch + 1)
            }
            HighlightTarget::MixerEqLo(ch) => {
                format!("Turn EQ LOW knob on channel {}", ch + 1)
            }
            HighlightTarget::MixerCue(ch) => {
                format!("Press CUE (headphone) button on channel {}", ch + 1)
            }
            HighlightTarget::MixerCrossfader => "Move the CROSSFADER".to_string(),
            HighlightTarget::MasterVolume => "Move the MASTER volume fader".to_string(),
            HighlightTarget::CueVolume => "Move the CUE/HEADPHONE volume knob".to_string(),
            HighlightTarget::CueMix => "Move the CUE/MASTER MIX knob".to_string(),
            HighlightTarget::BpmSlider => "Move the BPM slider".to_string(),
            HighlightTarget::BrowserEncoder => "Turn the BROWSE encoder".to_string(),
            HighlightTarget::BrowserSelect => {
                "Press the BROWSE encoder (or select button)".to_string()
            }
            HighlightTarget::BrowserEncoderDeck(d) => {
                let side = if *d == 0 { "LEFT" } else { "RIGHT" };
                format!("Turn the {} BROWSE encoder (or skip)", side)
            }
            HighlightTarget::BrowserSelectDeck(d) => {
                let side = if *d == 0 { "LEFT" } else { "RIGHT" };
                format!("Press the {} deck BROWSE select button", side)
            }
            HighlightTarget::FxEncoder => "Turn the FX SCROLL encoder (or skip)".to_string(),
            HighlightTarget::FxSelect => {
                "Press the FX encoder to SELECT (or skip)".to_string()
            }
            HighlightTarget::DeckFxMacro(d, m) => {
                format!("Turn FX MACRO {} knob on deck {}", m + 1, d + 1)
            }
            HighlightTarget::DeckLoad(d) => {
                format!("Press the LOAD button for deck {}", d + 1)
            }
            HighlightTarget::SuggestionEnergy(side) => {
                let s = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Move the {} SUGGESTION ENERGY slider/knob", s)
            }
            HighlightTarget::SettingsButton => "Press SETTINGS button (or skip)".to_string(),
        }
    }
}

/// Map a MappingDef + deck_index to a HighlightTarget.
///
/// Used to set the highlight when the cursor moves to a mapping node.
pub fn highlight_for_mapping(
    def: &MappingDef,
    deck_index: Option<usize>,
) -> Option<HighlightTarget> {
    let d = deck_index.unwrap_or(0);
    match (def.action, def.param_key, def.param_value) {
        ("deck.play", _, _) => Some(HighlightTarget::DeckPlay(d)),
        ("deck.cue_press", _, _) => Some(HighlightTarget::DeckCue(d)),
        ("deck.toggle_loop", _, _) => Some(HighlightTarget::DeckLoop(d)),
        ("deck.loop_size", _, _) => Some(HighlightTarget::DeckLoopEncoder(d)),
        ("deck.loop_in", _, _) => Some(HighlightTarget::DeckLoopIn(d)),
        ("deck.loop_out", _, _) => Some(HighlightTarget::DeckLoopOut(d)),
        ("deck.beat_jump_backward", _, _) => Some(HighlightTarget::DeckBeatJumpBack(d)),
        ("deck.beat_jump_forward", _, _) => Some(HighlightTarget::DeckBeatJumpForward(d)),
        ("deck.slip", _, _) => Some(HighlightTarget::DeckSlip(d)),
        ("deck.key_match", _, _) => Some(HighlightTarget::DeckKeyMatch(d)),
        ("deck.hot_cue_mode", _, _) => Some(HighlightTarget::DeckHotCueMode(d)),
        ("deck.slicer_mode", _, _) => Some(HighlightTarget::DeckSlicerMode(d)),
        ("deck.hot_cue_press", Some("slot"), Some(s)) => {
            Some(HighlightTarget::DeckHotCue(d, s))
        }
        ("deck.slicer_trigger", Some("pad"), Some(s)) => {
            Some(HighlightTarget::DeckSlicerPad(d, s))
        }
        ("deck.slicer_reset", _, _) => Some(HighlightTarget::DeckSlicerReset(d)),
        ("deck.stem_mute", Some("stem"), Some(s)) => {
            Some(HighlightTarget::DeckStemMute(d, s))
        }
        ("deck.stem_solo", Some("stem"), Some(s)) => {
            Some(HighlightTarget::DeckStemSolo(d, s))
        }
        ("deck.stem_link", Some("stem"), Some(s)) => {
            Some(HighlightTarget::DeckStemLink(d, s))
        }
        ("mixer.volume", _, _) => Some(HighlightTarget::MixerVolume(d)),
        ("mixer.filter", _, _) => Some(HighlightTarget::MixerFilter(d)),
        ("mixer.eq_hi", _, _) => Some(HighlightTarget::MixerEqHi(d)),
        ("mixer.eq_mid", _, _) => Some(HighlightTarget::MixerEqMid(d)),
        ("mixer.eq_lo", _, _) => Some(HighlightTarget::MixerEqLo(d)),
        ("mixer.cue", _, _) => Some(HighlightTarget::MixerCue(d)),
        ("mixer.crossfader", _, _) => Some(HighlightTarget::MixerCrossfader),
        ("mixer.cue_mix", _, _) => Some(HighlightTarget::CueMix),
        ("global.master_volume", _, _) => Some(HighlightTarget::MasterVolume),
        ("global.cue_volume", _, _) => Some(HighlightTarget::CueVolume),
        ("global.bpm", _, _) => Some(HighlightTarget::BpmSlider),
        ("browser.scroll", _, _) if def.uses_physical_deck => {
            Some(HighlightTarget::BrowserEncoderDeck(d))
        }
        ("browser.scroll", _, _) => Some(HighlightTarget::BrowserEncoder),
        ("browser.select", _, _) => Some(HighlightTarget::BrowserSelect),
        ("global.fx_scroll", _, _) => Some(HighlightTarget::FxEncoder),
        ("global.fx_select", _, _) => Some(HighlightTarget::FxSelect),
        ("deck.fx_macro", Some("macro"), Some(m)) => {
            Some(HighlightTarget::DeckFxMacro(d, m))
        }
        ("deck.load_selected", _, _) => Some(HighlightTarget::DeckLoad(d)),
        ("deck.suggestion_energy", _, _) => Some(HighlightTarget::SuggestionEnergy(d)),
        ("global.settings_toggle", _, _) => Some(HighlightTarget::SettingsButton),
        ("side.browse_mode", _, _) => Some(HighlightTarget::SideBrowseMode(d)),
        // Special actions (_shift, _layer_toggle) don't have UI highlights
        _ => None,
    }
}

// ============================================================================
// Captured Event (unchanged from previous version)
// ============================================================================

/// Protocol-agnostic captured event during learn mode.
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    /// Protocol-agnostic control address
    pub address: ControlAddress,
    /// Value (0-127 scale, normalized from any source)
    pub value: u8,
    /// Known hardware type (Some for HID, None for MIDI — triggers detection)
    pub hardware_type: Option<HardwareType>,
    /// Source device name for display
    pub source_device: Option<String>,
}

impl CapturedEvent {
    /// Format for display.
    pub fn display(&self) -> String {
        match &self.address {
            ControlAddress::Midi(midi_addr) => {
                let (msg_type, ch, num) = match midi_addr {
                    mesh_midi::MidiAddress::Note { channel, note } => ("Note", *channel, *note),
                    mesh_midi::MidiAddress::CC { channel, cc } => ("CC", *channel, *cc),
                };
                format!("{} ch{} 0x{:02X} val={}", msg_type, ch, num, self.value)
            }
            ControlAddress::Hid { name, .. } => {
                format!("HID {} val={}", name, self.value)
            }
        }
    }

    /// Check if this is a MIDI Note event.
    pub fn is_midi_note(&self) -> bool {
        matches!(
            &self.address,
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { .. })
        )
    }

    /// Check if this is a MIDI CC event.
    pub fn is_midi_cc(&self) -> bool {
        matches!(
            &self.address,
            ControlAddress::Midi(mesh_midi::MidiAddress::CC { .. })
        )
    }

    /// Get MIDI channel (returns 0 for HID events).
    pub fn midi_channel(&self) -> u8 {
        match &self.address {
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { channel, .. }) => *channel,
            ControlAddress::Midi(mesh_midi::MidiAddress::CC { channel, .. }) => *channel,
            ControlAddress::Hid { .. } => 0,
        }
    }

    /// Get MIDI note/CC number (returns 0 for HID events).
    pub fn midi_number(&self) -> u8 {
        match &self.address {
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { note, .. }) => *note,
            ControlAddress::Midi(mesh_midi::MidiAddress::CC { cc, .. }) => *cc,
            ControlAddress::Hid { .. } => 0,
        }
    }
}

// Keep old type alias for compatibility
pub type CapturedMidiEvent = CapturedEvent;

// ============================================================================
// Messages
// ============================================================================

/// Messages for MIDI learn mode.
#[derive(Debug, Clone)]
pub enum MidiLearnMessage {
    // Lifecycle
    Start,
    Cancel,
    Save,
    SaveComplete(Result<(), String>),

    // Setup phase
    SetTopology(TopologyChoice),
    SetCompactMode(bool),
    SetPadMode(PadModeSource),
    ConfirmSetup,

    // Tree navigation (keyboard/touch fallback — encoder handled in tick.rs)
    ScrollTree(i32),
    SelectRow(usize),
    ToggleSection,
    ClearMapping,

    // Capture (routed from tick.rs)
    MidiCaptured(CapturedEvent),
}

// ============================================================================
// Active Mapping (for live execution during learn mode)
// ============================================================================

/// A mapping that is currently active for live execution.
///
/// Built from tree nodes that have been mapped. When an event arrives
/// matching `(ControlAddress, shift_held)`, this mapping tells tick.rs
/// what action to execute.
#[derive(Debug, Clone)]
pub struct ActiveMapping {
    pub action: String,
    pub display_name: String,
    pub deck_index: Option<usize>,
    pub physical_deck: Option<usize>,
    pub param_key: Option<&'static str>,
    pub param_value: Option<usize>,
    pub hardware_type: HardwareType,
}

// ============================================================================
// State
// ============================================================================

/// MIDI Learn mode state.
///
/// Wraps the `LearnTree` and manages the phase-based workflow:
/// NavCapture → Setup → TreeNavigation → Verification
pub struct MidiLearnState {
    /// Whether learn mode is active
    pub is_active: bool,
    /// Current phase
    pub mode: LearnMode,

    // --- Nav Capture ---
    /// 0 = waiting for browse encoder, 1 = waiting for browse press
    pub nav_capture_step: usize,
    /// Captured browse encoder mapping
    pub nav_encoder_mapping: Option<MappedControl>,
    /// Captured browse select mapping
    pub nav_select_mapping: Option<MappedControl>,

    // --- Setup ---
    pub topology_choice: TopologyChoice,
    pub compact_mode: bool,
    pub pad_mode_source: PadModeSource,

    // --- Tree (built after ConfirmSetup) ---
    pub tree: Option<LearnTree>,

    // --- Highlight ---
    pub highlight_target: Option<HighlightTarget>,

    // --- Hardware Detection ---
    pub detection_buffer: Option<MidiSampleBuffer>,
    pub detected_hardware: Option<HardwareType>,
    pub last_captured: Option<CapturedEvent>,
    last_capture_time: Option<Instant>,

    // --- Browse Navigation Addresses ---
    pub browse_encoder_address: Option<ControlAddress>,
    pub browse_select_address: Option<ControlAddress>,

    // --- Shift Tracking ---
    pub shift_held: [bool; 2],
    pub shift_addresses: [Option<ControlAddress>; 2],

    // --- Active Mappings (for live execution) ---
    pub active_mappings: HashMap<(ControlAddress, bool), ActiveMapping>,

    // --- Status ---
    pub status: String,

    // --- Port / device info ---
    pub captured_port_name: Option<String>,
    pub existing_profile_name: Option<String>,
}

impl Default for MidiLearnState {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiLearnState {
    pub fn new() -> Self {
        Self {
            is_active: false,
            mode: LearnMode::NavCapture,
            nav_capture_step: 0,
            nav_encoder_mapping: None,
            nav_select_mapping: None,
            topology_choice: TopologyChoice::default(),
            compact_mode: false,
            pad_mode_source: PadModeSource::default(),
            tree: None,
            highlight_target: None,
            detection_buffer: None,
            detected_hardware: None,
            last_captured: None,
            last_capture_time: None,
            browse_encoder_address: None,
            browse_select_address: None,
            shift_held: [false; 2],
            shift_addresses: [None, None],
            active_mappings: HashMap::new(),
            status: String::new(),
            captured_port_name: None,
            existing_profile_name: None,
        }
    }

    /// Start MIDI learn mode.
    pub fn start(&mut self) {
        *self = Self::new();
        self.is_active = true;
        self.mode = LearnMode::NavCapture;
        self.nav_capture_step = 0;
        self.status = "Map your BROWSE encoder (turn it)".to_string();
    }

    /// Cancel and reset learn mode.
    pub fn cancel(&mut self) {
        self.is_active = false;
        *self = Self::new();
    }

    /// Build the tree from setup choices and switch to tree navigation.
    pub fn confirm_setup(&mut self) {
        let topology = self.topology_choice.to_topology(
            self.compact_mode,
            self.pad_mode_source,
        );
        let mut tree = LearnTree::build(topology);

        // Pre-fill navigation section with browse encoder/select from NavCapture
        if let Some(ref mapping) = self.nav_encoder_mapping {
            if let Some(node) = tree.find_mapping_node_mut(
                "browser.scroll", None, None, None,
            ) {
                if let TreeNode::Mapping { mapped, status, .. } = node {
                    *mapped = Some(mapping.clone());
                    *status = MappingStatus::New;
                }
            }
        }
        if let Some(ref mapping) = self.nav_select_mapping {
            if let Some(node) = tree.find_mapping_node_mut(
                "browser.select", None, None, None,
            ) {
                if let TreeNode::Mapping { mapped, status, .. } = node {
                    *mapped = Some(mapping.clone());
                    *status = MappingStatus::New;
                }
            }
        }

        // Expand navigation section and set cursor past it
        tree.expand_navigation();

        // Rebuild flat list and position cursor at first non-nav section
        tree.rebuild_flat_list();
        // Move cursor to first non-navigation section
        for (i, flat) in tree.flat_nodes.iter().enumerate() {
            if flat.node_type == FlatNodeType::Section && flat.depth == 0 {
                let node = tree.node_at_path(&flat.tree_path);
                if let TreeNode::Section { section_id, .. } = node {
                    if *section_id != "navigation" {
                        tree.cursor = i;
                        break;
                    }
                }
            }
        }

        self.tree = Some(tree);
        self.mode = LearnMode::TreeNavigation;
        self.status = "Browse the tree. Select a mapping to assign a control.".to_string();
        self.update_highlight();
        self.rebuild_active_mappings();
    }

    // -------------------------------------------------------------------
    // Highlight management
    // -------------------------------------------------------------------

    /// Update the highlight target based on the current tree cursor.
    pub fn update_highlight(&mut self) {
        self.highlight_target = match &self.tree {
            Some(tree) => {
                tree.current_node().and_then(|node| {
                    if let TreeNode::Mapping { def, deck_index, .. } = node {
                        highlight_for_mapping(def, *deck_index)
                    } else {
                        None
                    }
                })
            }
            None => None,
        };
    }

    // -------------------------------------------------------------------
    // Active mapping management (for live execution)
    // -------------------------------------------------------------------

    /// Rebuild the active_mappings lookup from all mapped tree nodes.
    ///
    /// Also extracts shift button addresses into `shift_addresses` for shift tracking.
    pub fn rebuild_active_mappings(&mut self) {
        self.active_mappings.clear();
        self.shift_addresses = [None, None];

        if let Some(ref tree) = self.tree {
            for (def, deck_idx, ctrl) in tree.all_mapped_nodes() {
                // Shift buttons: extract addresses for shift tracking
                if def.action == "_shift" {
                    let idx = match def.id {
                        "mod.shift_left" => 0,
                        "mod.shift_right" => 1,
                        _ => continue,
                    };
                    self.shift_addresses[idx] = Some(ctrl.address.clone());
                    continue;
                }

                // Skip other special actions (layer toggle)
                if def.action.starts_with('_') {
                    continue;
                }

                let key = (ctrl.address.clone(), ctrl.shift_held);
                let display = match deck_idx {
                    Some(d) => format!("{} Deck {}", def.label, d + 1),
                    None => def.label.to_string(),
                };
                let physical = if def.uses_physical_deck { deck_idx } else { None };
                let deck = if !def.uses_physical_deck { deck_idx } else { None };
                self.active_mappings.insert(key, ActiveMapping {
                    action: def.action.to_string(),
                    display_name: display,
                    deck_index: deck,
                    physical_deck: physical,
                    param_key: def.param_key,
                    param_value: def.param_value,
                    hardware_type: ctrl.hardware_type,
                });
            }
        }
    }

    // -------------------------------------------------------------------
    // Config generation (tree → MidiConfig)
    // -------------------------------------------------------------------

    /// Generate a complete `MidiConfig` from the current tree state.
    ///
    /// Walks all mapped tree nodes and builds `ControlMapping` + `FeedbackMapping`
    /// entries, with shift merging (two mappings sharing the same address with
    /// different shift states become a single mapping with `shift_action`).
    pub fn generate_config(&self) -> MidiConfig {
        let tree = match &self.tree {
            Some(t) => t,
            None => return MidiConfig { devices: vec![] },
        };

        let topology = &tree.topology;

        // Collect all mapped nodes
        let mapped_nodes = tree.all_mapped_nodes();

        // Build shift buttons from modifier section
        let mut shift_buttons = Vec::new();
        let mut layer_toggle_left: Option<ControlAddress> = None;
        let mut layer_toggle_right: Option<ControlAddress> = None;

        for (def, _deck_idx, ctrl) in &mapped_nodes {
            match def.id {
                "mod.shift_left" => {
                    shift_buttons.push(ShiftButtonConfig {
                        control: ctrl.address.clone(),
                        physical_deck: 0,
                    });
                }
                "mod.shift_right" => {
                    shift_buttons.push(ShiftButtonConfig {
                        control: ctrl.address.clone(),
                        physical_deck: 1,
                    });
                }
                "mod.layer_toggle_left" => {
                    layer_toggle_left = Some(ctrl.address.clone());
                }
                "mod.layer_toggle_right" => {
                    layer_toggle_right = Some(ctrl.address.clone());
                }
                _ => {}
            }
        }

        // Build deck target configuration
        // Clone layer toggle addresses before moving into DeckTargetConfig
        // (we need them again later for feedback mappings)
        let layer_toggle_left_fb = layer_toggle_left.clone();
        let layer_toggle_right_fb = layer_toggle_right.clone();

        let deck_target = if topology.has_layer_toggle {
            // Layer mode: 2 physical decks → 4 virtual via toggle
            let toggle_left = layer_toggle_left
                .unwrap_or(ControlAddress::Midi(mesh_midi::MidiAddress::Note { channel: 0, note: 0 }));
            let toggle_right = layer_toggle_right
                .unwrap_or(ControlAddress::Midi(mesh_midi::MidiAddress::Note { channel: 1, note: 0 }));
            DeckTargetConfig::Layer {
                toggle_left,
                toggle_right,
                layer_a: vec![0, 1],
                layer_b: vec![2, 3],
            }
        } else {
            // Direct mode: channel-to-deck 1:1
            let mut channel_to_deck = HashMap::new();
            for i in 0..topology.deck_count {
                channel_to_deck.insert(i as u8, i);
            }
            DeckTargetConfig::Direct { channel_to_deck }
        };

        // Build mappings and feedback, grouping by address for shift merging
        let mut mappings: Vec<ControlMapping> = Vec::new();
        let mut feedback: Vec<FeedbackMapping> = Vec::new();

        // Group mappings by (address) for shift merging
        // Key: address, Value: (non-shift mapping index, shift mapping index)
        let mut address_groups: HashMap<ControlAddress, (Option<usize>, Option<usize>)> = HashMap::new();

        for (def, deck_idx, ctrl) in &mapped_nodes {
            // Skip special actions
            if def.action.starts_with('_') { continue; }

            let behavior = Self::resolve_behavior(def, ctrl.hardware_type);
            let encoder_mode = ctrl.hardware_type.default_encoder_mode();

            let mut params = HashMap::new();
            if let Some(key) = def.param_key {
                if let Some(val) = def.param_value {
                    params.insert(key.to_string(), serde_yaml::Value::Number(serde_yaml::Number::from(val as u64)));
                }
            }

            let (physical_deck, deck_index) = if def.uses_physical_deck {
                (*deck_idx, None)
            } else {
                (None, *deck_idx)
            };

            let cm = ControlMapping {
                control: ctrl.address.clone(),
                action: def.action.to_string(),
                physical_deck,
                deck_index,
                params: params.clone(),
                behavior,
                shift_action: None,
                encoder_mode,
                hardware_type: Some(ctrl.hardware_type),
                mode: def.mode_condition.map(|s| s.to_string()),
            };

            let idx = mappings.len();
            mappings.push(cm);

            // Track for shift merging
            let group = address_groups.entry(ctrl.address.clone()).or_insert((None, None));
            if ctrl.shift_held {
                group.1 = Some(idx);
            } else {
                group.0 = Some(idx);
            }

            // Build feedback mapping if the def has a feedback state
            if let Some(feedback_state) = def.feedback_state {
                let (on_color, off_color) = Self::hid_feedback_colors(&ctrl.address);

                feedback.push(FeedbackMapping {
                    state: feedback_state.to_string(),
                    physical_deck,
                    deck_index,
                    params,
                    output: ctrl.address.clone(),
                    on_value: 127,
                    off_value: 0,
                    alt_on_value: None,
                    on_color,
                    off_color,
                    alt_on_color: None,
                    mode: def.mode_condition.map(|s| s.to_string()),
                });
            }
        }

        // Shift merging: combine entries sharing the same address
        // where one is shift and one is non-shift
        let mut remove_indices = Vec::new();
        for (_addr, (non_shift, shift)) in &address_groups {
            if let (Some(ns_idx), Some(s_idx)) = (non_shift, shift) {
                // Move shift action into the non-shift mapping
                let shift_action = mappings[*s_idx].action.clone();
                mappings[*ns_idx].shift_action = Some(shift_action);
                remove_indices.push(*s_idx);
            }
        }
        // Remove merged shift entries (in reverse order to preserve indices)
        remove_indices.sort_unstable();
        remove_indices.reverse();
        for idx in remove_indices {
            mappings.remove(idx);
        }

        // Add layer toggle feedback
        if topology.has_layer_toggle {
            if let Some(ref addr) = layer_toggle_left_fb {
                let (on_color, off_color) = Self::hid_feedback_colors(addr);
                feedback.push(FeedbackMapping {
                    state: "deck.layer_active".to_string(),
                    physical_deck: Some(0),
                    deck_index: None,
                    params: HashMap::new(),
                    output: addr.clone(),
                    on_value: 127,
                    off_value: 0,
                    alt_on_value: Some(64),
                    on_color,
                    off_color,
                    alt_on_color: Some([0, 127, 0]),
                    mode: None,
                });
            }
            if let Some(ref addr) = layer_toggle_right_fb {
                let (on_color, off_color) = Self::hid_feedback_colors(addr);
                feedback.push(FeedbackMapping {
                    state: "deck.layer_active".to_string(),
                    physical_deck: Some(1),
                    deck_index: None,
                    params: HashMap::new(),
                    output: addr.clone(),
                    on_value: 127,
                    off_value: 0,
                    alt_on_value: Some(64),
                    on_color,
                    off_color,
                    alt_on_color: Some([0, 127, 0]),
                    mode: None,
                });
            }
        }

        // Profile name: use existing or generate from port name + timestamp
        let profile_name = self.existing_profile_name.clone()
            .unwrap_or_else(|| {
                let base = self.captured_port_name.as_deref().unwrap_or("learned");
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{}-{}", base, ts)
            });

        // Port match from captured port name
        let port_match = self.captured_port_name.clone()
            .unwrap_or_else(|| "Unknown Device".to_string());

        let learned_port_name = self.captured_port_name.clone()
            .map(|p| mesh_midi::normalize_port_name(&p));

        let color_note_offsets = mesh_midi::detect_color_note_offsets(
            &port_match
        );

        let profile = DeviceProfile {
            name: profile_name,
            port_match,
            learned_port_name,
            device_type: None,
            hid_product_match: None,
            hid_device_id: None,
            deck_target,
            pad_mode_source: topology.pad_mode_source,
            shift_buttons,
            mappings,
            feedback,
            momentary_mode_buttons: topology.compact_mode,
            color_note_offsets,
        };

        MidiConfig {
            devices: vec![profile],
        }
    }

    /// Resolve the appropriate ControlBehavior for a mapping.
    ///
    /// Most mappings use the def's declared behavior. But if a continuous
    /// hardware type (knob/fader) is assigned to a button action, we force
    /// Continuous behavior so the adapter can handle it.
    fn resolve_behavior(def: &MappingDef, hw_type: HardwareType) -> ControlBehavior {
        match def.control_type {
            ControlType::Button => {
                if hw_type.is_continuous() {
                    // Continuous hardware on button action → keep as Momentary
                    // (the adapter layer handles threshold crossing)
                    ControlBehavior::Momentary
                } else {
                    def.behavior
                }
            }
            ControlType::Encoder => ControlBehavior::Continuous,
            ControlType::Knob | ControlType::Fader => ControlBehavior::Continuous,
        }
    }

    /// Get HID feedback colors for a control address.
    ///
    /// Returns (on_color, off_color) for HID devices.
    /// For MIDI devices, returns (None, None).
    fn hid_feedback_colors(addr: &ControlAddress) -> (Option<[u8; 3]>, Option<[u8; 3]>) {
        match addr {
            ControlAddress::Hid { .. } => {
                // Default HID feedback: bright blue on, dim off
                (Some([0, 60, 127]), Some([0, 5, 10]))
            }
            _ => (None, None),
        }
    }

    // -------------------------------------------------------------------
    // Existing config loading (midi.yaml → tree)
    // -------------------------------------------------------------------

    /// Load an existing `MidiConfig` into the tree, pre-filling mapped controls.
    ///
    /// Called when entering learn mode with an existing `midi.yaml`:
    /// 1. Infers topology from the first DeviceProfile
    /// 2. Builds tree with that topology
    /// 3. Walks profile mappings and matches to tree nodes
    /// 4. Marks matched nodes as `Existing`
    pub fn load_existing_config(&mut self, config: &MidiConfig) {
        let profile = match config.devices.first() {
            Some(p) => p,
            None => return,
        };

        // Infer topology from profile
        let (deck_count, has_layer_toggle) = match &profile.deck_target {
            DeckTargetConfig::Layer { .. } => (2, true),
            DeckTargetConfig::Direct { channel_to_deck } => {
                let max_deck = channel_to_deck.values().max().copied().unwrap_or(1);
                (max_deck + 1, false)
            }
        };

        let topology = TopologyConfig {
            deck_count,
            has_layer_toggle,
            compact_mode: profile.momentary_mode_buttons,
            pad_mode_source: profile.pad_mode_source,
        };

        // Set setup choices to match existing config
        self.topology_choice = match (deck_count, has_layer_toggle) {
            (2, false) => TopologyChoice::TwoDecks,
            (2, true) | (_, true) => TopologyChoice::TwoDecksLayer,
            _ => TopologyChoice::FourDecks,
        };
        self.compact_mode = profile.momentary_mode_buttons;
        self.pad_mode_source = profile.pad_mode_source;
        self.existing_profile_name = Some(profile.name.clone());

        // Preserve port info
        if let Some(ref lpn) = profile.learned_port_name {
            self.captured_port_name = Some(lpn.clone());
        }

        // Build tree
        let mut tree = LearnTree::build(topology);

        // Load shift buttons into modifier nodes
        for sb in &profile.shift_buttons {
            let def_id = match sb.physical_deck {
                0 => "mod.shift_left",
                _ => "mod.shift_right",
            };
            if let Some(node) = tree.find_mapping_node_mut("_shift", None, None, None) {
                if let TreeNode::Mapping { def, mapped, status, .. } = node {
                    if def.id == def_id {
                        *mapped = Some(MappedControl {
                            address: sb.control.clone(),
                            hardware_type: HardwareType::Button,
                            shift_held: false,
                            source_device: None,
                        });
                        *status = MappingStatus::Existing;
                    }
                }
            }
        }

        // Load layer toggle buttons
        if let DeckTargetConfig::Layer { ref toggle_left, ref toggle_right, .. } = profile.deck_target {
            if let Some(node) = tree.find_mapping_node_mut("_layer_toggle", None, None, None) {
                if let TreeNode::Mapping { def, mapped, status, .. } = node {
                    if def.id == "mod.layer_toggle_left" {
                        *mapped = Some(MappedControl {
                            address: toggle_left.clone(),
                            hardware_type: HardwareType::Button,
                            shift_held: false,
                            source_device: None,
                        });
                        *status = MappingStatus::Existing;
                    }
                }
            }
            if let Some(node) = tree.find_mapping_node_mut("_layer_toggle", None, None, None) {
                if let TreeNode::Mapping { def, mapped, status, .. } = node {
                    if def.id == "mod.layer_toggle_right" {
                        *mapped = Some(MappedControl {
                            address: toggle_right.clone(),
                            hardware_type: HardwareType::Button,
                            shift_held: false,
                            source_device: None,
                        });
                        *status = MappingStatus::Existing;
                    }
                }
            }
        }

        // Load control mappings into tree nodes
        for cm in &profile.mappings {
            let deck_idx = if cm.physical_deck.is_some() {
                cm.physical_deck
            } else {
                cm.deck_index
            };

            // Extract param info
            let param_key_str: Option<String> = cm.params.keys().next().cloned();
            let param_value: Option<usize> = param_key_str.as_ref().and_then(|k| {
                cm.params.get(k).and_then(|v| v.as_u64()).map(|n| n as usize)
            });

            let hw_type = cm.hardware_type.unwrap_or(HardwareType::Unknown);

            // Find matching tree node
            if let Some(node) = tree.find_mapping_node_mut(
                &cm.action,
                deck_idx,
                param_key_str.as_deref(),
                param_value,
            ) {
                if let TreeNode::Mapping { mapped, original, status, .. } = node {
                    let ctrl = MappedControl {
                        address: cm.control.clone(),
                        hardware_type: hw_type,
                        shift_held: false,
                        source_device: None,
                    };
                    *mapped = Some(ctrl.clone());
                    *original = Some(ctrl);
                    *status = MappingStatus::Existing;
                }
            }

            // Handle shift_action: find the node for that action and mark it
            if let Some(ref shift_action) = cm.shift_action {
                if let Some(node) = tree.find_mapping_node_mut(
                    shift_action,
                    deck_idx,
                    None,
                    None,
                ) {
                    if let TreeNode::Mapping { mapped, original, status, .. } = node {
                        let ctrl = MappedControl {
                            address: cm.control.clone(),
                            hardware_type: hw_type,
                            shift_held: true,
                            source_device: None,
                        };
                        *mapped = Some(ctrl.clone());
                        *original = Some(ctrl);
                        *status = MappingStatus::Existing;
                    }
                }
            }
        }

        // Extract browse encoder/select from navigation section
        if let Some(node) = tree.find_mapping_node_mut("browser.scroll", None, None, None) {
            if let TreeNode::Mapping { mapped: Some(ref ctrl), .. } = node {
                self.browse_encoder_address = Some(ctrl.address.clone());
                self.nav_encoder_mapping = Some(ctrl.clone());
            }
        }
        if let Some(node) = tree.find_mapping_node_mut("browser.select", None, None, None) {
            if let TreeNode::Mapping { mapped: Some(ref ctrl), .. } = node {
                self.browse_select_address = Some(ctrl.address.clone());
                self.nav_select_mapping = Some(ctrl.clone());
            }
        }

        // Expand nav section and set cursor
        tree.expand_navigation();
        tree.rebuild_flat_list();

        // Move cursor to first non-navigation section
        for (i, flat) in tree.flat_nodes.iter().enumerate() {
            if flat.node_type == FlatNodeType::Section && flat.depth == 0 {
                let node = tree.node_at_path(&flat.tree_path);
                if let TreeNode::Section { section_id, .. } = node {
                    if *section_id != "navigation" {
                        tree.cursor = i;
                        break;
                    }
                }
            }
        }

        self.tree = Some(tree);
        self.mode = LearnMode::TreeNavigation;
        self.status = "Existing config loaded. Edit and save.".to_string();
        self.update_highlight();
        self.rebuild_active_mappings();
    }

    // -------------------------------------------------------------------
    // Hardware detection (preserved from previous version)
    // -------------------------------------------------------------------

    /// Check if a captured event should be accepted (debounce + filter).
    pub fn should_capture(&self, event: &CapturedEvent) -> bool {
        // Filter Note Off / button release events
        if event.is_midi_note() && event.value == 0 {
            return false;
        }
        // Filter HID button releases
        if matches!(&event.address, ControlAddress::Hid { .. }) && event.value == 0 {
            return false;
        }
        // Check debounce
        if let Some(last_time) = self.last_capture_time {
            if last_time.elapsed() < CAPTURE_DEBOUNCE {
                return false;
            }
        }
        true
    }

    /// Mark that a capture just happened (for debouncing).
    pub fn mark_captured(&mut self) {
        self.last_capture_time = Some(Instant::now());
    }

    /// Start hardware detection for a captured event.
    ///
    /// For HID events (known hardware type): finalizes immediately.
    /// For MIDI events: starts MidiSampleBuffer for CC type detection.
    pub fn start_capture(&mut self, event: CapturedEvent) {
        self.last_captured = Some(event.clone());

        // HID path: hardware type already known
        if let Some(hw_type) = event.hardware_type {
            self.detected_hardware = Some(hw_type);
            self.detection_buffer = None;
            self.finalize_with_hardware(hw_type, event.address.clone(), event.source_device.clone());
            return;
        }

        // MIDI path: start sampling for hardware detection
        let channel = event.midi_channel();
        let number = event.midi_number();
        let is_note = event.is_midi_note();

        self.detection_buffer = Some(MidiSampleBuffer::new(channel, number, is_note));

        // Add first sample
        if let Some(ref mut buffer) = self.detection_buffer {
            let is_note_on = is_note && event.value > 0;
            buffer.add_sample(event.value, is_note_on, Some(number));
        }

        self.status = format!("Sampling: {} (move control...)", event.display());

        // For Note events (buttons), complete immediately
        if is_note {
            self.finalize_mapping();
        }
    }

    /// Add a sample to the active detection buffer (MIDI only).
    ///
    /// Returns true if the buffer is now complete and ready to finalize.
    pub fn add_detection_sample(&mut self, event: &CapturedEvent) -> bool {
        if let Some(ref mut buffer) = self.detection_buffer {
            let channel = event.midi_channel();
            let number = event.midi_number();
            let is_note = event.is_midi_note();

            if buffer.matches(channel, number, is_note) {
                let is_note_on = is_note && event.value > 0;
                buffer.add_sample(event.value, is_note_on, Some(number));
                self.last_captured = Some(event.clone());

                let count = buffer.sample_count();
                let progress = (buffer.elapsed_ratio() * 100.0) as u8;
                self.status = format!("Sampling... {} samples ({}%)", count, progress);

                return buffer.is_complete();
            }
        }
        false
    }

    /// Check if detection buffer is complete.
    pub fn is_detection_complete(&self) -> bool {
        self.detection_buffer
            .as_ref()
            .map(|b| b.is_complete())
            .unwrap_or(false)
    }

    /// Finalize mapping with detected hardware type from MidiSampleBuffer.
    pub fn finalize_mapping(&mut self) {
        if let Some(ref buffer) = self.detection_buffer {
            let hw_type = buffer.analyze();
            self.detected_hardware = Some(hw_type);

            let address = if buffer.is_note() {
                ControlAddress::Midi(mesh_midi::MidiAddress::Note {
                    channel: buffer.get_channel(),
                    note: buffer.get_number(),
                })
            } else {
                ControlAddress::Midi(mesh_midi::MidiAddress::CC {
                    channel: buffer.get_channel(),
                    cc: buffer.get_number(),
                })
            };

            self.status = format!("Mapped as {:?}", hw_type);
            log::info!("Learn: Detected {:?} ({:?})", hw_type, address);

            self.detection_buffer = None;
            self.finalize_with_hardware(hw_type, address, None);
        }
    }

    /// Common finalize path: store the mapping in the appropriate place.
    fn finalize_with_hardware(
        &mut self,
        hw_type: HardwareType,
        address: ControlAddress,
        source_device: Option<String>,
    ) {
        let ctrl = MappedControl {
            address: address.clone(),
            hardware_type: hw_type,
            shift_held: self.shift_held[0] || self.shift_held[1],
            source_device,
        };

        match self.mode {
            LearnMode::NavCapture => {
                if self.nav_capture_step == 0 {
                    // Browse encoder captured
                    self.browse_encoder_address = Some(address);
                    self.nav_encoder_mapping = Some(ctrl);
                    self.nav_capture_step = 1;
                    self.last_capture_time = None;
                    self.status = "Map your BROWSE press (push the encoder)".to_string();
                    log::info!("Learn: Browse encoder captured");
                } else {
                    // Browse select captured
                    self.browse_select_address = Some(address);
                    self.nav_select_mapping = Some(ctrl);
                    self.mode = LearnMode::Setup;
                    self.last_capture_time = None;
                    self.status = "Select your deck topology".to_string();
                    log::info!("Learn: Browse select captured, entering setup");
                }
            }
            LearnMode::TreeNavigation => {
                if let Some(ref mut tree) = self.tree {
                    if tree.record_mapping(ctrl) {
                        log::info!("Learn: Recorded mapping on tree node");
                        tree.advance_to_next();
                    }
                }
                self.last_capture_time = None;
                self.update_highlight();
                self.rebuild_active_mappings();
            }
            _ => {}
        }
    }
}

// ============================================================================
// View Functions
// ============================================================================

/// Create the highlight border style.
pub fn highlight_border_style() -> container::Style {
    container::Style {
        border: iced::Border {
            color: Color::from_rgb(1.0, 0.0, 0.0),
            width: 3.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}

/// Format a ControlAddress for display in the tree.
fn format_address(addr: &ControlAddress) -> String {
    match addr {
        ControlAddress::Midi(mesh_midi::MidiAddress::Note { channel, note }) => {
            format!("CH{} Note {}", channel + 1, note)
        }
        ControlAddress::Midi(mesh_midi::MidiAddress::CC { channel, cc }) => {
            format!("CH{} CC {}", channel + 1, cc)
        }
        ControlAddress::Hid { name, .. } => {
            format!("HID {}", name)
        }
    }
}

/// Render the bottom drawer for MIDI learn mode.
pub fn view_drawer(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    if !state.is_active {
        return Space::new().height(0).into();
    }

    // Header
    let title = text("MIDI LEARN").size(sz(16.0));

    let progress_text = match state.mode {
        LearnMode::NavCapture => "Step 1: Map Navigation".to_string(),
        LearnMode::Setup => "Step 2: Configure Topology".to_string(),
        LearnMode::TreeNavigation => {
            if let Some(ref tree) = state.tree {
                let (mapped, total) = tree.total_progress();
                format!("{}/{} mapped", mapped, total)
            } else {
                String::new()
            }
        }
        LearnMode::Verification => "Review Changes".to_string(),
    };
    let progress = text(progress_text).size(sz(12.0)).color(Color::from_rgb(0.6, 0.6, 0.7));

    let cancel_btn = button(text("Cancel").size(sz(12.0)))
        .on_press(MidiLearnMessage::Cancel)
        .style(button::secondary);

    let header = row![
        title,
        Space::new().width(10),
        progress,
        Space::new().width(Length::Fill),
        cancel_btn,
    ]
    .align_y(Alignment::Center);

    // Content varies by mode
    let content: Element<MidiLearnMessage> = match state.mode {
        LearnMode::NavCapture => view_nav_capture(state),
        LearnMode::Setup => view_setup(state),
        LearnMode::TreeNavigation => view_tree(state),
        LearnMode::Verification => view_verification(state),
    };

    let drawer_content = column![header, content]
        .spacing(10)
        .padding(15)
        .width(Length::Fill);

    container(drawer_content)
        .style(|_theme| container::Style {
            background: Some(Color::from_rgba(0.1, 0.1, 0.15, 0.95).into()),
            border: iced::Border {
                color: Color::from_rgb(0.3, 0.3, 0.4),
                width: 1.0,
                radius: iced::border::Radius::default()
                    .top_left(8.0)
                    .top_right(8.0),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
}

/// View for NavCapture phase — map browse encoder and press.
fn view_nav_capture(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let prompt = if state.nav_capture_step == 0 {
        "Turn your BROWSE encoder (the knob you'll use to scroll through mappings)"
    } else {
        "Press your BROWSE button (push the encoder to select items)"
    };

    let prompt_text = text(prompt)
        .size(sz(14.0))
        .color(Color::from_rgb(0.9, 0.9, 0.95));

    let status_text = text(&state.status)
        .size(sz(11.0))
        .color(Color::from_rgb(0.5, 0.5, 0.6));

    // Show captured info
    let captured_display = if let Some(ref event) = state.last_captured {
        text(format!("Last: {}", event.display()))
            .size(sz(11.0))
            .color(Color::from_rgb(0.4, 0.6, 0.4))
    } else {
        text("Waiting for input...")
            .size(sz(11.0))
            .color(Color::from_rgb(0.4, 0.4, 0.5))
    };

    column![prompt_text, status_text, captured_display]
        .spacing(8)
        .into()
}

/// View for Setup phase — topology, compact mode, pad mode.
fn view_setup(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let dim = Color::from_rgb(0.5, 0.5, 0.6);

    // --- Topology question ---
    let topo_label = text("Deck Layout").size(sz(13.0));
    let topo_desc = text("How many physical deck sections does your controller have?")
        .size(sz(10.0))
        .color(dim);
    let topo_buttons: Vec<Element<MidiLearnMessage>> = TopologyChoice::ALL
        .iter()
        .map(|choice| {
            let is_selected = *choice == state.topology_choice;
            button(text(choice.label()).size(sz(11.0)))
                .on_press(MidiLearnMessage::SetTopology(*choice))
                .style(if is_selected { button::primary } else { button::secondary })
                .into()
        })
        .collect();
    let topo_options = row(topo_buttons).spacing(5);
    let topo_selected_desc = text(state.topology_choice.description())
        .size(sz(10.0))
        .color(dim);

    // --- Compact mode question ---
    let compact_label = text("Compact Mode").size(sz(13.0));
    let compact_desc = text(
        "Default is performance mode (play, cue, loops on every control). \
         Compact controllers share pads between modes — hold a mode button \
         to overlay hot cues or slicer on the same pads."
    )
        .size(sz(10.0))
        .color(dim);
    let compact_off = button(text("Off").size(sz(11.0)))
        .on_press(MidiLearnMessage::SetCompactMode(false))
        .style(if !state.compact_mode { button::primary } else { button::secondary });
    let compact_on = button(text("On").size(sz(11.0)))
        .on_press(MidiLearnMessage::SetCompactMode(true))
        .style(if state.compact_mode { button::primary } else { button::secondary });
    let compact_row = row![compact_off, Space::new().width(5), compact_on]
        .align_y(Alignment::Center);

    // --- Pad mode question ---
    let pad_label = text("Pad Mode Source").size(sz(13.0));
    let pad_desc = text(
        "App: Pads always send the same MIDI notes — the app decides what they do. \
         Controller: Each pad mode sends different MIDI notes — map slicer pads separately."
    )
        .size(sz(10.0))
        .color(dim);
    let pad_app = button(text("App").size(sz(11.0)))
        .on_press(MidiLearnMessage::SetPadMode(PadModeSource::App))
        .style(if state.pad_mode_source == PadModeSource::App { button::primary } else { button::secondary });
    let pad_controller = button(text("Controller").size(sz(11.0)))
        .on_press(MidiLearnMessage::SetPadMode(PadModeSource::Controller))
        .style(if state.pad_mode_source == PadModeSource::Controller { button::primary } else { button::secondary });
    let pad_row = row![pad_app, Space::new().width(5), pad_controller]
        .align_y(Alignment::Center);

    // Confirm button
    let confirm_btn = button(text("Build Mapping Tree").size(sz(12.0)))
        .on_press(MidiLearnMessage::ConfirmSetup)
        .style(button::primary);

    column![
        topo_label,
        topo_desc,
        topo_options,
        topo_selected_desc,
        Space::new().height(2),
        compact_label,
        compact_desc,
        compact_row,
        Space::new().height(2),
        pad_label,
        pad_desc,
        pad_row,
        Space::new().height(5),
        confirm_btn,
    ]
    .spacing(4)
    .into()
}

/// View for TreeNavigation phase — the main tree view.
fn view_tree(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let tree = match &state.tree {
        Some(t) => t,
        None => return Space::new().height(0).into(),
    };

    // Build tree rows (flat_map: each node emits 1 row + optional description row when cursor)
    let rows: Vec<Element<MidiLearnMessage>> = tree
        .flat_nodes
        .iter()
        .enumerate()
        .flat_map(|(i, flat)| {
            let is_cursor = i == tree.cursor;
            let node = tree.node_at_path(&flat.tree_path);
            let indent = flat.depth as f32 * sz(20.0);

            // Track description to show below cursor
            let mut cursor_description: Option<&str> = None;

            let row_content: Element<MidiLearnMessage> = match node {
                TreeNode::Section { label, expanded, .. } => {
                    let chevron = if *expanded { "▼ " } else { "▶ " };
                    let (mapped, total) = node.section_progress();
                    let badge = if total > 0 {
                        format!("{}/{}", mapped, total)
                    } else {
                        String::new()
                    };
                    row![
                        Space::new().width(indent),
                        text(chevron).size(sz(12.0)),
                        text(label).size(sz(13.0)),
                        Space::new().width(Length::Fill),
                        text(badge).size(sz(11.0)).color(Color::from_rgb(0.5, 0.5, 0.6)),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                }
                TreeNode::Mapping { def, mapped, status, .. } => {
                    if is_cursor && !def.description.is_empty() {
                        cursor_description = Some(def.description);
                    }
                    let (dot, dot_color) = match status {
                        MappingStatus::Unmapped => ("○", Color::from_rgb(0.4, 0.4, 0.5)),
                        MappingStatus::Existing => ("◆", Color::from_rgb(0.3, 0.4, 0.6)),
                        MappingStatus::New => ("●", Color::from_rgb(0.2, 0.8, 0.3)),
                        MappingStatus::Changed => ("◈", Color::from_rgb(0.9, 0.6, 0.1)),
                    };
                    let addr_text = mapped
                        .as_ref()
                        .map(|m| format_address(&m.address))
                        .unwrap_or_else(|| {
                            if is_cursor && *status == MappingStatus::Unmapped {
                                "(press control...)".to_string()
                            } else {
                                String::new()
                            }
                        });
                    let addr_color = if mapped.is_some() {
                        Color::from_rgb(0.5, 0.6, 0.5)
                    } else {
                        Color::from_rgb(0.4, 0.4, 0.5)
                    };
                    row![
                        Space::new().width(indent),
                        text(dot).size(sz(12.0)).color(dot_color),
                        Space::new().width(4),
                        text(def.label).size(sz(12.0)),
                        Space::new().width(Length::Fill),
                        text(addr_text).size(sz(11.0)).color(addr_color),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                }
                TreeNode::Done => {
                    row![
                        text("✓").size(sz(12.0)).color(Color::from_rgb(0.2, 0.8, 0.3)),
                        Space::new().width(4),
                        text("Done — Save Mappings").size(sz(13.0)),
                    ]
                    .align_y(Alignment::Center)
                    .into()
                }
            };

            // Wrap in a clickable button
            let styled_row: Element<MidiLearnMessage> = if is_cursor {
                container(
                    button(row_content)
                        .on_press(MidiLearnMessage::SelectRow(i))
                        .style(button::text)
                        .width(Length::Fill),
                )
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.2, 0.25, 0.35, 0.8).into()),
                    border: iced::Border {
                        color: Color::from_rgb(0.3, 0.4, 0.6),
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .width(Length::Fill)
                .into()
            } else {
                button(row_content)
                    .on_press(MidiLearnMessage::SelectRow(i))
                    .style(button::text)
                    .width(Length::Fill)
                    .into()
            };

            // Emit the main row + optional description below cursor
            let mut elements = vec![styled_row];
            if let Some(desc) = cursor_description {
                let desc_indent = (flat.depth as f32 + 1.0) * sz(20.0);
                let desc_row: Element<MidiLearnMessage> = row![
                    Space::new().width(desc_indent),
                    text(desc)
                        .size(sz(10.0))
                        .color(Color::from_rgb(0.5, 0.55, 0.65)),
                ]
                .into();
                elements.push(desc_row);
            }
            elements
        })
        .collect();

    let tree_column = column(rows).spacing(2);

    let tree_scroll = scrollable(tree_column)
        .height(Length::Fixed(sz(300.0)))
        .width(Length::Fill)
        .id("midi_learn_tree");

    // Action log footer
    let log_entries: Vec<Element<MidiLearnMessage>> = tree
        .action_log
        .entries()
        .map(|entry| {
            let badge_color = match entry.status {
                LogStatus::Mapped => Color::from_rgb(0.2, 0.6, 0.9),
                LogStatus::Captured => Color::from_rgb(0.2, 0.8, 0.3),
            };
            let badge = match entry.status {
                LogStatus::Mapped => "mapped",
                LogStatus::Captured => "captured",
            };
            row![
                text(&entry.control_display)
                    .size(sz(10.0))
                    .color(Color::from_rgb(0.5, 0.5, 0.6)),
                Space::new().width(4),
                text("→").size(sz(10.0)).color(Color::from_rgb(0.4, 0.4, 0.5)),
                Space::new().width(4),
                text(&entry.action_name).size(sz(10.0)),
                Space::new().width(4),
                text(format!("[{}]", badge))
                    .size(sz(10.0))
                    .color(badge_color),
                Space::new().width(12),
            ]
            .align_y(Alignment::Center)
            .into()
        })
        .collect();

    let log_row: Element<MidiLearnMessage> = if log_entries.is_empty() {
        text("Actions will appear here as you map controls")
            .size(sz(10.0))
            .color(Color::from_rgb(0.35, 0.35, 0.4))
            .into()
    } else {
        row(log_entries).into()
    };

    let log_container = container(log_row)
        .padding([4, 8])
        .style(|_| container::Style {
            background: Some(Color::from_rgba(0.08, 0.08, 0.12, 0.8).into()),
            border: iced::Border {
                color: Color::from_rgb(0.2, 0.2, 0.3),
                width: 1.0,
                radius: 3.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill);

    // Status line
    let status = text(&state.status)
        .size(sz(11.0))
        .color(Color::from_rgb(0.5, 0.5, 0.6));

    column![tree_scroll, log_container, status]
        .spacing(6)
        .into()
}

/// View for Verification phase — review changes and save.
fn view_verification(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let tree = match &state.tree {
        Some(t) => t,
        None => return Space::new().height(0).into(),
    };

    let (mapped, _total) = tree.total_progress();
    let changed = tree.changed_nodes();

    let summary = text(format!(
        "{} mappings total, {} new/changed",
        mapped,
        changed.len()
    ))
    .size(sz(14.0));

    // List changed mappings
    let change_rows: Vec<Element<MidiLearnMessage>> = changed
        .iter()
        .take(20) // Limit display
        .map(|(def, deck_idx, ctrl, status)| {
            let status_label = match status {
                MappingStatus::New => "NEW",
                MappingStatus::Changed => "CHANGED",
                _ => "",
            };
            let status_color = match status {
                MappingStatus::New => Color::from_rgb(0.2, 0.8, 0.3),
                MappingStatus::Changed => Color::from_rgb(0.9, 0.6, 0.1),
                _ => Color::from_rgb(0.5, 0.5, 0.6),
            };
            let deck_label = deck_idx
                .map(|d| format!(" Deck {}", d + 1))
                .unwrap_or_default();
            row![
                text(format!("[{}]", status_label))
                    .size(sz(11.0))
                    .color(status_color),
                Space::new().width(8),
                text(format!("{}{}", def.label, deck_label)).size(sz(12.0)),
                Space::new().width(Length::Fill),
                text(format_address(&ctrl.address))
                    .size(sz(11.0))
                    .color(Color::from_rgb(0.5, 0.5, 0.6)),
            ]
            .align_y(Alignment::Center)
            .into()
        })
        .collect();

    let unchanged_count = mapped - changed.len();
    let unchanged_text = if unchanged_count > 0 {
        text(format!("+ {} unchanged mappings preserved", unchanged_count))
            .size(sz(11.0))
            .color(Color::from_rgb(0.4, 0.4, 0.5))
    } else {
        text("").size(sz(1.0))
    };

    let save_btn = button(text("Save").size(sz(13.0)))
        .on_press(MidiLearnMessage::Save)
        .style(button::primary);

    let back_btn = button(text("← Back to Tree").size(sz(12.0)))
        .on_press(MidiLearnMessage::ScrollTree(0)) // Will be handled to go back to tree mode
        .style(button::secondary);

    let button_row = row![back_btn, Space::new().width(Length::Fill), save_btn]
        .align_y(Alignment::Center);

    let changes_col = column(change_rows).spacing(4);

    let scroll = scrollable(changes_col)
        .height(Length::Fixed(sz(250.0)))
        .width(Length::Fill);

    column![
        summary,
        Space::new().height(5),
        scroll,
        unchanged_text,
        Space::new().height(8),
        button_row,
    ]
    .spacing(6)
    .into()
}
