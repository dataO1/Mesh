//! MIDI Learn Mode — Tree-based mapping system
//!
//! Replaces the linear phase-based wizard with a collapsible tree:
//! - Browse encoder/press mapped first for controller navigation
//! - Topology setup (deck count, layer toggle, performance style)
//! - Collapsible tree with sections for each mapping category
//! - Live mapping: once mapped, controls execute their action
//! - Verification window before saving

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use iced::widget::{button, column, container, row, scrollable, text, Id, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_widgets::sz;
use mesh_midi::{
    ControlAddress, ControlBehavior, ControlMapping, DeckTargetConfig,
    DeviceProfile, FeedbackMapping, HardwareType, MidiConfig,
    MidiSampleBuffer, PadModeSource, ShiftButtonConfig,
};
use mesh_midi::learn_defs::{ControlType, MappingDef, TopologyConfig};
use crate::ui::midi_learn_tree::{
    LearnTree, LogStatus, MappedControl, MappingStatus, TreeNode,
};

/// Debounce duration for MIDI capture (prevents release/encoder spam from double-mapping)
const CAPTURE_DEBOUNCE: Duration = Duration::from_millis(1000);

/// Approximate row height in the tree view (for scroll offset calculation)
const TREE_ROW_HEIGHT: f32 = 22.0;
/// Visible height of the tree scrollable area
const TREE_VISIBLE_HEIGHT: f32 = 300.0;

/// Scrollable ID for the MIDI learn tree view.
pub static LEARN_TREE_SCROLLABLE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("midi_learn_tree_scroll"));

/// Create a Task to snap the tree scrollable to keep the cursor centered.
pub fn scroll_tree_to_cursor<Message: 'static>(cursor: usize, total_items: usize) -> iced::Task<Message> {
    let total_height = total_items as f32 * TREE_ROW_HEIGHT;
    let max_scroll = (total_height - TREE_VISIBLE_HEIGHT).max(0.0);
    if max_scroll <= 0.0 {
        return iced::Task::none();
    }

    let visible_rows = (TREE_VISIBLE_HEIGHT / TREE_ROW_HEIGHT).floor();
    let cursor_y = cursor as f32 * TREE_ROW_HEIGHT;
    let center_offset = (visible_rows / 2.0).floor() * TREE_ROW_HEIGHT;
    let target = (cursor_y - center_offset + TREE_ROW_HEIGHT / 2.0).clamp(0.0, max_scroll);
    let relative_y = (target / max_scroll).clamp(0.0, 1.0);

    let offset = iced::widget::scrollable::RelativeOffset { x: 0.0, y: relative_y };
    iced::widget::operation::snap_to(LEARN_TREE_SCROLLABLE_ID.clone(), offset)
}

// ============================================================================
// Phase / Mode
// ============================================================================

/// Current phase of the MIDI learn workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LearnMode {
    /// Map browse encoder and press first (before any questions)
    #[default]
    NavCapture,
    /// Topology setup questions (deck count, performance style, pad mode, encoder count)
    Setup,
    /// Main tree view — browse and map controls
    TreeNavigation,
    /// Reset confirmation dialog — confirm clearing all mappings
    ResetConfirm,
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
    SetOverlayMode(bool),
    SetPadMode(PadModeSource),
    ConfirmSetup,

    // Tree navigation (keyboard/touch fallback — encoder handled in tick.rs)
    ScrollTree(i32),
    SelectRow(usize),
    ToggleSection,
    ClearMapping,

    // Reset confirmation
    ResetMappings,
    ConfirmReset,
    CancelReset,

    // Capture (routed from tick.rs)
    MidiCaptured(CapturedEvent),

    // Deferred scroll (re-scroll after layout recalculation from fold/unfold)
    RefreshScroll,
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
    /// Mode condition from the catalog def — enables same address across different modes
    pub mode_condition: Option<&'static str>,
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
    /// Step counter for NavCapture phase:
    /// 0 = encoder count question, 1..N = capture encoders, N+1 = capture select
    pub nav_capture_step: usize,
    /// Captured browse encoder mappings (one per encoder)
    pub nav_encoder_mappings: Vec<MappedControl>,
    /// Captured browse select mapping
    pub nav_select_mapping: Option<MappedControl>,

    // --- Setup ---
    /// Cursor position in the setup menu (encoder-navigable flat list)
    pub setup_cursor: usize,
    /// Cursor position in the verification view (0 = Back, 1 = Save)
    pub verify_cursor: usize,
    pub topology_choice: TopologyChoice,
    pub overlay_mode: bool,
    pub pad_mode_source: PadModeSource,
    /// Deck indices where loop encoder is auto-shared: (shared_deck, source_deck)
    pub loop_shared_decks: Vec<(usize, usize)>,

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
    pub browse_encoder_addresses: Vec<ControlAddress>,
    pub browse_select_addresses: Vec<ControlAddress>,

    // --- Shift Tracking ---
    pub shift_held: [bool; 2],
    pub shift_addresses: [Option<ControlAddress>; 2],

    // --- Active Mappings (for live execution) ---
    pub active_mappings: HashMap<(ControlAddress, bool), Vec<ActiveMapping>>,

    // --- Status ---
    pub status: String,

    // --- Port / device info ---
    pub captured_port_name: Option<String>,
    pub existing_profile_name: Option<String>,

    // --- HID device identification (preserved from existing config or inferred) ---
    pub existing_device_type: Option<String>,
    pub existing_hid_product_match: Option<String>,
    pub existing_hid_device_id: Option<String>,

    // --- Reset confirmation ---
    pub has_existing_config: bool,
    pub reset_confirm_cursor: usize, // 0 = Cancel, 1 = Reset
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
            nav_encoder_mappings: Vec::new(),
            nav_select_mapping: None,
            setup_cursor: 0,
            verify_cursor: 1, // Default to Save
            topology_choice: TopologyChoice::default(),
            overlay_mode: false,
            pad_mode_source: PadModeSource::default(),
            loop_shared_decks: Vec::new(),
            tree: None,
            highlight_target: None,
            detection_buffer: None,
            detected_hardware: None,
            last_captured: None,
            last_capture_time: None,
            browse_encoder_addresses: Vec::new(),
            browse_select_addresses: Vec::new(),
            shift_held: [false; 2],
            shift_addresses: [None, None],
            active_mappings: HashMap::new(),
            status: String::new(),
            captured_port_name: None,
            existing_profile_name: None,
            existing_device_type: None,
            existing_hid_product_match: None,
            existing_hid_device_id: None,
            has_existing_config: false,
            reset_confirm_cursor: 0,
        }
    }

    /// Start MIDI learn mode.
    pub fn start(&mut self) {
        *self = Self::new();
        self.is_active = true;
        self.mode = LearnMode::NavCapture;
        self.nav_capture_step = 0;
        self.status = "Turn your main BROWSE encoder".to_string();
    }

    /// Cancel and reset learn mode.
    pub fn cancel(&mut self) {
        self.is_active = false;
        *self = Self::new();
    }

    /// Total number of selectable items in the setup menu.
    const SETUP_ITEM_COUNT: usize = 8;
    // 0-2:   TopologyChoice (TwoDecks, TwoDecksLayer, FourDecks)
    // 3-4:   Performance style (Toggle, Overlay)
    // 5-6:   Pad mode (App, Controller)
    // 7:     Confirm

    /// Scroll the setup cursor by `delta` (positive = down, negative = up).
    pub fn setup_scroll(&mut self, delta: i32) {
        let max = Self::SETUP_ITEM_COUNT - 1;
        let new = (self.setup_cursor as i32 + delta).clamp(0, max as i32) as usize;
        self.setup_cursor = new;
    }

    /// Select the item at the current setup cursor position.
    /// Returns true if setup should be confirmed (cursor was on Confirm).
    pub fn setup_select(&mut self) -> bool {
        match self.setup_cursor {
            0 => self.topology_choice = TopologyChoice::TwoDecks,
            1 => self.topology_choice = TopologyChoice::TwoDecksLayer,
            2 => self.topology_choice = TopologyChoice::FourDecks,
            3 => self.overlay_mode = false,
            4 => self.overlay_mode = true,
            5 => self.pad_mode_source = PadModeSource::App,
            6 => self.pad_mode_source = PadModeSource::Controller,
            7 => return true, // Confirm
            _ => {}
        }
        false
    }

    /// Build the tree from setup choices and switch to tree navigation.
    pub fn confirm_setup(&mut self) {
        let topology = self.topology_choice.to_topology(
            self.pad_mode_source,
        );
        let deck_count = topology.physical_deck_count();
        let mut tree = LearnTree::build(topology);

        // Pre-fill navigation section with first browse encoder from NavCapture
        if let Some(mapping) = self.nav_encoder_mappings.first() {
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

        // Smart loop encoder pre-fill: use all captured browse encoders
        let enc_count = self.nav_encoder_mappings.len();
        self.loop_shared_decks.clear();

        if enc_count > 0 {
            for pd in 0..deck_count {
                let enc_idx = match (enc_count, deck_count) {
                    (1, _) => 0,                                // single encoder → all decks share
                    (2, 4) => pd % 2,                           // 2 encoders + 4 decks → per side
                    _ => pd.min(enc_count - 1),                 // 1:1 or clamp
                };
                // Track which decks are sharing (not the primary owner)
                // Source deck = enc_idx (the deck that "owns" this encoder)
                if enc_count < deck_count && pd >= enc_count {
                    self.loop_shared_decks.push((pd, enc_idx));
                } else if enc_count == 1 && pd > 0 {
                    self.loop_shared_decks.push((pd, 0));
                } else if enc_count == 2 && deck_count == 4 && pd >= 2 {
                    self.loop_shared_decks.push((pd, enc_idx));
                }
                if let Some(enc) = self.nav_encoder_mappings.get(enc_idx) {
                    if let Some(node) = tree.find_mapping_node_mut(
                        "deck.loop_size", Some(pd), None, None,
                    ) {
                        if let TreeNode::Mapping { mapped, status, .. } = node {
                            *mapped = Some(enc.clone());
                            *status = MappingStatus::New;
                        }
                    }
                }
            }
        }

        // Pre-fill deck.toggle_loop and deck.load_selected from browse encoder press.
        // In 2-physical-deck setups, each browse encoder press doubles as loop toggle + load.
        if deck_count == 2 {
            if let Some(ref select) = self.nav_select_mapping {
                for action in &["deck.toggle_loop", "deck.load_selected"] {
                    if let Some(node) = tree.find_mapping_node_mut(action, Some(0), None, None) {
                        if let TreeNode::Mapping { mapped, status, .. } = node {
                            if *status == MappingStatus::Unmapped {
                                *mapped = Some(select.clone());
                                *status = MappingStatus::New;
                            }
                        }
                    }
                }
            }
        }

        // Insert Reset node if editing an existing config
        if self.has_existing_config {
            tree.roots.insert(0, TreeNode::Reset);
        }

        // Expand navigation section and set cursor past it
        tree.expand_navigation();

        // expand_navigation already positioned cursor on first unmapped mapping (shift buttons)

        self.tree = Some(tree);
        self.mode = LearnMode::TreeNavigation;
        self.status = "Browse the tree. Select a mapping to assign a control.".to_string();
        self.update_highlight();
        self.rebuild_active_mappings();
    }

    // -------------------------------------------------------------------
    // Highlight management
    // -------------------------------------------------------------------

    /// Get the cursor position and total item count for auto-scrolling.
    pub fn tree_scroll_info(&self) -> Option<(usize, usize)> {
        self.tree.as_ref().map(|t| (t.cursor, t.flat_nodes.len()))
    }

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

        // Rebuild browse encoder + select addresses from NavCapture + tree-mapped nodes
        self.browse_encoder_addresses.clear();
        self.browse_select_addresses.clear();
        if let Some(first) = self.nav_encoder_mappings.first() {
            self.browse_encoder_addresses.push(first.address.clone());
        }
        if let Some(ref select) = self.nav_select_mapping {
            self.browse_select_addresses.push(select.address.clone());
        }

        if let Some(ref tree) = self.tree {
            for (def, deck_idx, ctrl) in tree.all_mapped_nodes() {
                // Sync additional browse encoder addresses from tree
                if def.action == "browser.scroll"
                    && !self.browse_encoder_addresses.contains(&ctrl.address)
                {
                    self.browse_encoder_addresses.push(ctrl.address.clone());
                }

                // Sync additional browse select addresses from tree
                if def.action == "browser.select"
                    && !self.browse_select_addresses.contains(&ctrl.address)
                {
                    self.browse_select_addresses.push(ctrl.address.clone());
                }

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
                self.active_mappings.entry(key).or_default().push(ActiveMapping {
                    action: def.action.to_string(),
                    display_name: display,
                    deck_index: deck,
                    physical_deck: physical,
                    param_key: def.param_key,
                    param_value: def.param_value,
                    hardware_type: ctrl.hardware_type,
                    mode_condition: def.mode_condition,
                });
            }
        }
    }

    // -------------------------------------------------------------------
    // Config generation (tree → MidiConfig)
    // -------------------------------------------------------------------

    /// Generate a complete `MidiConfig` from the current tree state.
    ///
    /// Groups mapped nodes by source device and creates one `DeviceProfile`
    /// per device. Handles shift merging, feedback, and auto-generated
    /// browse-mode loop encoder mappings per-device.
    pub fn generate_config(&self) -> MidiConfig {
        let tree = match &self.tree {
            Some(t) => t,
            None => return MidiConfig { devices: vec![] },
        };

        let topology = &tree.topology;

        // Collect all mapped nodes
        let mapped_nodes = tree.all_mapped_nodes();

        // --- Extract layer toggle addresses (needed for shared deck_target) ---
        let mut layer_toggle_left: Option<ControlAddress> = None;
        let mut layer_toggle_right: Option<ControlAddress> = None;
        for (def, _deck_idx, ctrl) in &mapped_nodes {
            match def.id {
                "mod.layer_toggle_left" => layer_toggle_left = Some(ctrl.address.clone()),
                "mod.layer_toggle_right" => layer_toggle_right = Some(ctrl.address.clone()),
                _ => {}
            }
        }

        // Build deck target configuration (shared across all profiles)
        let layer_toggle_left_fb = layer_toggle_left.clone();
        let layer_toggle_right_fb = layer_toggle_right.clone();

        let deck_target = if topology.has_layer_toggle {
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
            let mut channel_to_deck = HashMap::new();
            for i in 0..topology.deck_count {
                channel_to_deck.insert(i as u8, i);
            }
            DeckTargetConfig::Direct { channel_to_deck }
        };

        // --- Group mapped nodes by source device ---
        let mut device_groups: std::collections::BTreeMap<String, Vec<(&mesh_midi::learn_defs::MappingDef, Option<usize>, &MappedControl)>> = std::collections::BTreeMap::new();
        for entry @ (_, _, ctrl) in &mapped_nodes {
            let key = Self::device_key(ctrl);
            device_groups.entry(key).or_default().push(*entry);
        }

        if device_groups.is_empty() {
            return MidiConfig { devices: vec![] };
        }

        // --- Build one DeviceProfile per device ---
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut profiles: Vec<DeviceProfile> = Vec::new();
        for (device_key, group) in &device_groups {
            // Extract shift buttons for this device
            let mut shift_buttons = Vec::new();
            for (def, _deck_idx, ctrl) in group {
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
                    _ => {}
                }
            }

            // Build mappings and feedback for this device
            let mut mappings: Vec<ControlMapping> = Vec::new();
            let mut feedback: Vec<FeedbackMapping> = Vec::new();
            let mut address_groups: HashMap<ControlAddress, (Option<usize>, Option<usize>)> = HashMap::new();

            for (def, deck_idx, ctrl) in group {
                // Skip special actions (shift, layer toggle, etc.)
                if def.action.starts_with('_') { continue; }

                let behavior = Self::resolve_behavior(def, ctrl.hardware_type);
                let encoder_mode = ctrl.hardware_type.default_encoder_mode();

                let mut params = HashMap::new();
                let is_extra_browse = (def.action == "browser.scroll" || def.action == "browser.select")
                    && def.param_key == Some("index");
                if !is_extra_browse {
                    if let Some(key) = def.param_key {
                        if let Some(val) = def.param_value {
                            params.insert(key.to_string(), serde_yaml::Value::Number(serde_yaml::Number::from(val as u64)));
                        }
                    }
                }

                let (physical_deck, deck_index) = if is_extra_browse {
                    (def.param_value, None)
                } else if def.uses_physical_deck {
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

                // Browse mode: derive "side" param from physical_deck (% 2 for 4-deck support)
                if def.action == "side.browse_mode" {
                    if let Some(pd) = physical_deck {
                        mappings[idx].params.insert(
                            "side".to_string(),
                            serde_yaml::Value::Number(((pd % 2) as u64).into()),
                        );
                    }
                }

                // Mode buttons: pair deck N with deck N+2 (e.g. deck 0+2, deck 1+3)
                // Works for all topologies — guard `paired < 4` prevents OOB
                if (def.action == "deck.hot_cue_mode" || def.action == "deck.slicer_mode")
                    && physical_deck.is_some()
                {
                    let pd = physical_deck.unwrap();
                    let paired = pd + 2;
                    if paired < 4 {
                        let decks_seq = vec![
                            serde_yaml::Value::Number((pd as u64).into()),
                            serde_yaml::Value::Number((paired as u64).into()),
                        ];
                        mappings[idx].params.insert(
                            "decks".to_string(),
                            serde_yaml::Value::Sequence(decks_seq),
                        );
                    }
                }

                let group_entry = address_groups.entry(ctrl.address.clone()).or_insert((None, None));
                if ctrl.shift_held {
                    group_entry.1 = Some(idx);
                } else {
                    group_entry.0 = Some(idx);
                }

                if let Some(feedback_state) = def.feedback_state {
                    // Browse mode: map physical_deck to side (% 2) for feedback
                    let fb_physical_deck = if feedback_state == "side.browse_mode" {
                        physical_deck.map(|pd| pd % 2)
                    } else {
                        physical_deck
                    };
                    feedback.push(FeedbackMapping {
                        state: feedback_state.to_string(),
                        physical_deck: fb_physical_deck,
                        deck_index,
                        params,
                        output: ctrl.address.clone(),
                        on_value: 127,
                        off_value: 0,
                        alt_on_value: None,
                        on_color: None,
                        off_color: None,
                        alt_on_color: None,
                        mode: def.mode_condition.map(|s| s.to_string()),
                    });
                }
            }

            // Shift merging within this device
            let mut remove_indices = Vec::new();
            for (_addr, (non_shift, shift)) in &address_groups {
                if let (Some(ns_idx), Some(s_idx)) = (non_shift, shift) {
                    let shift_action = mappings[*s_idx].action.clone();
                    mappings[*ns_idx].shift_action = Some(shift_action);
                    remove_indices.push(*s_idx);
                }
            }
            remove_indices.sort_unstable();
            remove_indices.reverse();
            for idx in remove_indices {
                mappings.remove(idx);
            }

            // Auto-propagate browser.back: if any browser.select has shift_action=browser.back,
            // apply the same shift_action to all other browser.select mappings
            let has_back = mappings.iter().any(|m|
                m.action == "browser.select" && m.shift_action.as_deref() == Some("browser.back")
            );
            if has_back {
                for m in &mut mappings {
                    if m.action == "browser.select" && m.shift_action.is_none() {
                        m.shift_action = Some("browser.back".to_string());
                    }
                }
            }

            // Add layer toggle feedback if this device owns the toggle
            if topology.has_layer_toggle {
                if let Some(ref addr) = layer_toggle_left_fb {
                    if group.iter().any(|(def, _, _)| def.id == "mod.layer_toggle_left") {
                        feedback.push(FeedbackMapping {
                            state: "deck.layer_active".to_string(),
                            physical_deck: Some(0),
                            deck_index: None,
                            params: HashMap::new(),
                            output: addr.clone(),
                            on_value: 127,
                            off_value: 0,
                            alt_on_value: Some(64),
                            on_color: None,
                            off_color: None,
                            alt_on_color: None,
                            mode: None,
                        });
                    }
                }
                if let Some(ref addr) = layer_toggle_right_fb {
                    if group.iter().any(|(def, _, _)| def.id == "mod.layer_toggle_right") {
                        feedback.push(FeedbackMapping {
                            state: "deck.layer_active".to_string(),
                            physical_deck: Some(1),
                            deck_index: None,
                            params: HashMap::new(),
                            output: addr.clone(),
                            on_value: 127,
                            off_value: 0,
                            alt_on_value: Some(64),
                            on_color: None,
                            off_color: None,
                            alt_on_color: None,
                            mode: None,
                        });
                    }
                }
            }

            // Reverse auto-gen: extra browse encoders → deck.loop_size
            // When a browser.scroll (mode:"browse") control has no deck.loop_size
            // counterpart, infer a loop_size mapping so the encoder doubles as
            // loop length outside browse mode.
            let browse_needing_loop: Vec<_> = mappings.iter()
                .filter(|m| m.action == "browser.scroll" && m.mode.as_deref() == Some("browse"))
                .filter(|m| !mappings.iter().any(|other|
                    other.action == "deck.loop_size" && other.control == m.control
                ))
                .map(|m| (m.control.clone(), m.physical_deck.unwrap_or(0), m.encoder_mode, m.hardware_type))
                .collect();
            for (control, pd, encoder_mode, hw_type) in browse_needing_loop {
                mappings.push(ControlMapping {
                    control,
                    action: "deck.loop_size".to_string(),
                    physical_deck: None,
                    deck_index: Some(pd),
                    params: HashMap::new(),
                    behavior: ControlBehavior::Continuous,
                    shift_action: None,
                    encoder_mode,
                    hardware_type: hw_type,
                    mode: None,
                });
            }

            // NOTE: No reverse auto-gen for toggle_loop from browser.select.
            // Unlike loop_size (encoder rotation naturally doubles as browse scroll),
            // toggle_loop is a deliberate button assignment — auto-inferring it from
            // the navigation browse press causes unwanted loop toggling.

            // Side-pair loop_size and toggle_loop across layer-complementary decks.
            // When 2+ controls map the same action, each is assigned to a "side"
            // (even decks 0,2 or odd decks 1,3) based on its lowest deck_index.
            // Excess entries are removed and a `decks` param is added for the pair.
            // Single-control case: multi-dispatch engine handles all entries as-is.
            let virtual_deck_count = if topology.has_layer_toggle {
                (topology.deck_count * 2).min(4)
            } else {
                topology.deck_count.min(4)
            };

            if virtual_deck_count > 2 {
                for action_name in &["deck.loop_size"] {
                    // Find unique controls and their lowest (primary) deck_index
                    let mut ctrl_primary: HashMap<ControlAddress, usize> = HashMap::new();
                    for m in mappings.iter() {
                        if m.action == *action_name {
                            if let Some(di) = m.deck_index {
                                ctrl_primary.entry(m.control.clone())
                                    .and_modify(|e| { if di < *e { *e = di } })
                                    .or_insert(di);
                            }
                        }
                    }

                    if ctrl_primary.len() >= 2 {
                        // Multiple controls — assign each to a side, deduplicate
                        let mut sorted: Vec<_> = ctrl_primary.iter().collect();
                        sorted.sort_by_key(|(_, primary)| **primary);

                        let mut side_ctrl: [Option<ControlAddress>; 2] = [None, None];
                        for (ctrl, primary) in &sorted {
                            let side = *primary % 2;
                            if side_ctrl[side].is_none() {
                                side_ctrl[side] = Some((*ctrl).clone());
                            }
                        }

                        // Save one template per control
                        let templates: HashMap<ControlAddress, ControlMapping> = mappings.iter()
                            .filter(|m| m.action == *action_name)
                            .map(|m| (m.control.clone(), m.clone()))
                            .collect();

                        // Remove all entries for this action
                        mappings.retain(|m| m.action != *action_name);

                        // Re-add one entry per side-owning control with decks param
                        for side in 0..2usize {
                            if let Some(ref ctrl) = side_ctrl[side] {
                                if let Some(template) = templates.get(ctrl) {
                                    let mut entry = template.clone();
                                    entry.deck_index = Some(side);
                                    let paired = side + 2;
                                    if paired < virtual_deck_count {
                                        let decks_seq = vec![
                                            serde_yaml::Value::Number((side as u64).into()),
                                            serde_yaml::Value::Number((paired as u64).into()),
                                        ];
                                        entry.params.insert(
                                            "decks".to_string(),
                                            serde_yaml::Value::Sequence(decks_seq),
                                        );
                                    }
                                    mappings.push(entry);
                                }
                            }
                        }
                    } else if topology.has_layer_toggle {
                        // Single control with layer toggle — pair each entry with its complement
                        for m in mappings.iter_mut() {
                            if m.action == *action_name
                                && m.deck_index.is_some()
                                && !m.params.contains_key("decks")
                            {
                                let di = m.deck_index.unwrap();
                                let paired = di + topology.deck_count;
                                if paired < virtual_deck_count {
                                    let decks_array = vec![
                                        serde_yaml::Value::Number((di as u64).into()),
                                        serde_yaml::Value::Number((paired as u64).into()),
                                    ];
                                    m.params.insert(
                                        "decks".to_string(),
                                        serde_yaml::Value::Sequence(decks_array),
                                    );
                                }
                            }
                        }
                    }
                    // Single control without layer toggle: multi-dispatch handles all entries
                }
            }

            // Auto-generate browser.scroll with mode:"browse" on loop encoders
            // Skip controls that already have a browser.scroll (e.g. from extra browse encoders)
            let loop_entries: Vec<_> = mappings.iter()
                .filter(|m| m.action == "deck.loop_size")
                .filter(|m| !mappings.iter().any(|other|
                    other.action == "browser.scroll" && other.control == m.control
                ))
                .map(|m| (m.control.clone(), m.physical_deck.unwrap_or(0), m.encoder_mode, m.hardware_type))
                .collect();
            for (control, pd, encoder_mode, hw_type) in loop_entries {
                mappings.push(ControlMapping {
                    control,
                    action: "browser.scroll".to_string(),
                    physical_deck: Some(pd),
                    deck_index: None,
                    params: HashMap::new(),
                    behavior: ControlBehavior::Continuous,
                    shift_action: None,
                    encoder_mode,
                    hardware_type: hw_type,
                    mode: Some("browse".to_string()),
                });
            }

            // Auto-generate browser.select with mode:"browse" on loop toggle buttons
            // Skip controls that already have a browser.select
            let loop_toggle_entries: Vec<_> = mappings.iter()
                .filter(|m| m.action == "deck.toggle_loop")
                .filter(|m| !mappings.iter().any(|other|
                    other.action == "browser.select" && other.control == m.control
                ))
                .map(|m| (m.control.clone(), m.physical_deck.unwrap_or(0), m.hardware_type))
                .collect();
            for (control, pd, hw_type) in loop_toggle_entries {
                mappings.push(ControlMapping {
                    control,
                    action: "browser.select".to_string(),
                    physical_deck: Some(pd),
                    deck_index: None,
                    params: HashMap::new(),
                    behavior: ControlBehavior::Momentary,
                    shift_action: None,
                    encoder_mode: None,
                    hardware_type: hw_type,
                    mode: Some("browse".to_string()),
                });
            }

            // Auto-generate slicer pad mappings + feedback from hot cue pads.
            // Slicer pads always share the same physical buttons as hot cue pads,
            // just with mode:"slicer" instead of mode:"hot_cue".
            let hot_cue_entries: Vec<_> = mappings.iter()
                .filter(|m| m.action == "deck.hot_cue_press" && m.mode.as_deref() == Some("hot_cue"))
                .map(|m| {
                    let slot = m.params.get("slot")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    (m.control.clone(), m.physical_deck, m.deck_index, m.hardware_type, slot)
                })
                .collect();
            for (control, pd, di, hw_type, slot) in &hot_cue_entries {
                let mut params = HashMap::new();
                params.insert("pad".to_string(), serde_yaml::Value::Number((*slot).into()));
                mappings.push(ControlMapping {
                    control: control.clone(),
                    action: "deck.slicer_trigger".to_string(),
                    physical_deck: *pd,
                    deck_index: *di,
                    params: params.clone(),
                    behavior: ControlBehavior::Momentary,
                    shift_action: None,
                    encoder_mode: None,
                    hardware_type: *hw_type,
                    mode: Some("slicer".to_string()),
                });
                feedback.push(FeedbackMapping {
                    state: "deck.slicer_slice_active".to_string(),
                    physical_deck: *pd,
                    deck_index: *di,
                    params,
                    output: control.clone(),
                    on_value: 127,
                    off_value: 0,
                    alt_on_value: None,
                    on_color: None,
                    off_color: None,
                    alt_on_color: None,
                    mode: Some("slicer".to_string()),
                });
            }

            // Propagate physical_deck from deck.toggle_loop to browser.select on same control.
            // The main browse select (nav.browse_select) comes from a Once section so it
            // has physical_deck: None. But when it shares a control with a per-deck
            // toggle_loop, it needs physical_deck for context-aware loading.
            if topology.deck_count == 2 {
                let toggle_pd: Vec<_> = mappings.iter()
                    .filter(|m| m.action == "deck.toggle_loop" && m.physical_deck.is_some())
                    .map(|m| (m.control.clone(), m.physical_deck.unwrap()))
                    .collect();
                for (control, pd) in &toggle_pd {
                    for m in mappings.iter_mut() {
                        if m.action == "browser.select" && m.control == *control
                            && m.mode.as_deref() == Some("browse")
                            && m.physical_deck.is_none()
                        {
                            m.physical_deck = Some(*pd);
                        }
                    }
                }
            }

            // Auto-generate deck.toggle_loop (performance mode) on per-deck browse encoder press buttons.
            // In browse mode the encoder press does browser.select; in performance mode it toggles loop.
            // Only for 2-physical-deck setups where each deck has its own encoder.
            if topology.deck_count == 2 {
                let browse_press_entries: Vec<_> = mappings.iter()
                    .filter(|m| m.action == "browser.select" && m.mode.as_deref() == Some("browse"))
                    .filter(|m| m.physical_deck.is_some())
                    .filter(|m| !mappings.iter().any(|other|
                        other.action == "deck.toggle_loop" && other.control == m.control
                    ))
                    .map(|m| (m.control.clone(), m.physical_deck.unwrap(), m.hardware_type))
                    .collect();
                for (control, pd, hw_type) in browse_press_entries {
                    mappings.push(ControlMapping {
                        control,
                        action: "deck.toggle_loop".to_string(),
                        physical_deck: Some(pd),
                        deck_index: None,
                        params: HashMap::new(),
                        behavior: ControlBehavior::Toggle,
                        shift_action: None,
                        encoder_mode: None,
                        hardware_type: hw_type,
                        mode: None, // default/performance mode
                    });
                }
            }

            // --- Infer device identification from the device key ---
            let is_hid = device_key.starts_with("hid:");
            let (device_type, hid_product_match, hid_device_id, port_match, learned_port_name) = if is_hid {
                let did = device_key.strip_prefix("hid:").unwrap_or(device_key).to_string();
                let product = group.iter()
                    .find_map(|(_, _, ctrl)| ctrl.source_device.clone())
                    .unwrap_or_else(|| self.captured_port_name.clone().unwrap_or_else(|| "HID Device".to_string()));
                (
                    Some("hid".to_string()),
                    Some(product.clone()),
                    Some(did),
                    product.clone(),
                    None,
                )
            } else {
                let port = device_key.clone();
                let normalized = mesh_midi::normalize_port_name(&port);
                (None, None, None, port, Some(normalized))
            };

            let profile_name = if profiles.is_empty() {
                // First profile: use existing name if available
                self.existing_profile_name.clone().unwrap_or_else(|| format!("{}-{}", port_match, ts))
            } else {
                format!("{}-{}", port_match, ts)
            };

            let color_note_offsets = mesh_midi::detect_color_note_offsets(&port_match);

            profiles.push(DeviceProfile {
                name: profile_name,
                port_match,
                learned_port_name,
                device_type,
                hid_product_match,
                hid_device_id,
                deck_target: deck_target.clone(),
                pad_mode_source: topology.pad_mode_source,
                shift_buttons,
                mappings,
                feedback,
                momentary_mode_buttons: self.overlay_mode,
                color_note_offsets,
            });
        }

        // Cross-device side-pairing for loop_size and toggle_loop.
        // When multiple devices have encoders mapped to the same action,
        // assign each control to a side (decks 0,2 or 1,3) and trim excess.
        let virtual_deck_count = if topology.has_layer_toggle {
            (topology.deck_count * 2).min(4)
        } else {
            topology.deck_count.min(4)
        };

        if virtual_deck_count > 2 {
            for action_name in &["deck.loop_size"] {
                // Collect all (control, deck_index) across ALL device profiles
                let mut ctrl_primary: HashMap<ControlAddress, usize> = HashMap::new();
                for profile in &profiles {
                    for m in &profile.mappings {
                        if m.action == *action_name {
                            if let Some(di) = m.deck_index {
                                ctrl_primary.entry(m.control.clone())
                                    .and_modify(|e| { if di < *e { *e = di } })
                                    .or_insert(di);
                            }
                        }
                    }
                }

                if ctrl_primary.len() < 2 { continue; }

                // Assign each control to side 0 or 1 based on lowest deck_index
                let mut sorted: Vec<_> = ctrl_primary.iter().collect();
                sorted.sort_by_key(|(_, primary)| **primary);

                let mut side_ctrl: [Option<ControlAddress>; 2] = [None, None];
                for (ctrl, primary) in &sorted {
                    let side = *primary % 2;
                    if side_ctrl[side].is_none() {
                        side_ctrl[side] = Some((*ctrl).clone());
                    }
                }

                // Build the target deck list per side-owning control
                let mut ctrl_target: HashMap<ControlAddress, Vec<usize>> = HashMap::new();
                for side in 0..2usize {
                    if let Some(ref ctrl) = side_ctrl[side] {
                        let mut decks = vec![side];
                        if side + 2 < virtual_deck_count {
                            decks.push(side + 2);
                        }
                        ctrl_target.insert(ctrl.clone(), decks);
                    }
                }

                // Rewrite each profile: keep one entry per side-owning control
                for profile in &mut profiles {
                    let templates: HashMap<ControlAddress, ControlMapping> = profile.mappings.iter()
                        .filter(|m| m.action == *action_name)
                        .map(|m| (m.control.clone(), m.clone()))
                        .collect();

                    if templates.is_empty() { continue; }

                    // Remove all entries for this action
                    profile.mappings.retain(|m| m.action != *action_name);

                    // Re-add one entry per control that owns a side
                    for (ctrl, template) in &templates {
                        if let Some(decks) = ctrl_target.get(ctrl) {
                            let mut entry = template.clone();
                            entry.deck_index = Some(decks[0]);
                            if decks.len() > 1 {
                                let decks_seq: Vec<serde_yaml::Value> = decks.iter()
                                    .map(|&d| serde_yaml::Value::Number((d as u64).into()))
                                    .collect();
                                entry.params.insert(
                                    "decks".to_string(),
                                    serde_yaml::Value::Sequence(decks_seq),
                                );
                            }
                            profile.mappings.push(entry);
                        }
                        // else: control doesn't own a side → entry dropped
                    }
                }
            }
        }

        // Cross-device browser.back propagation: if any device has a browser.select
        // with shift_action="browser.back", propagate to all other browser.select
        // entries across all devices that lack a shift_action.
        let any_has_back = profiles.iter().any(|p|
            p.mappings.iter().any(|m|
                m.action == "browser.select" && m.shift_action.as_deref() == Some("browser.back")
            )
        );
        if any_has_back {
            for profile in &mut profiles {
                for m in &mut profile.mappings {
                    if m.action == "browser.select" && m.shift_action.is_none() {
                        m.shift_action = Some("browser.back".to_string());
                    }
                }
            }
        }

        MidiConfig { devices: profiles }
    }

    /// Derive a device grouping key from a mapped control.
    /// HID controls group by device_id, MIDI by source port name.
    fn device_key(ctrl: &MappedControl) -> String {
        match &ctrl.address {
            ControlAddress::Hid { device_id, .. } => format!("hid:{}", device_id),
            _ => ctrl.source_device.clone().unwrap_or_else(|| "unknown".to_string()),
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
        if config.devices.is_empty() { return; }

        let first_profile = &config.devices[0];

        // Infer topology from first profile (all profiles share topology)
        let (deck_count, has_layer_toggle) = match &first_profile.deck_target {
            DeckTargetConfig::Layer { .. } => (2, true),
            DeckTargetConfig::Direct { channel_to_deck } => {
                let max_deck = channel_to_deck.values().max().copied().unwrap_or(1);
                (max_deck + 1, false)
            }
        };

        let topology = TopologyConfig {
            deck_count,
            has_layer_toggle,
            pad_mode_source: first_profile.pad_mode_source,
        };

        // Set setup choices to match existing config
        self.topology_choice = match (deck_count, has_layer_toggle) {
            (2, false) => TopologyChoice::TwoDecks,
            (2, true) | (_, true) => TopologyChoice::TwoDecksLayer,
            _ => TopologyChoice::FourDecks,
        };
        self.overlay_mode = first_profile.momentary_mode_buttons;
        self.pad_mode_source = first_profile.pad_mode_source;
        self.existing_profile_name = Some(first_profile.name.clone());

        // Preserve port info from first profile (for backward compat fallback)
        if let Some(ref lpn) = first_profile.learned_port_name {
            self.captured_port_name = Some(lpn.clone());
        }
        self.existing_device_type = first_profile.device_type.clone();
        self.existing_hid_product_match = first_profile.hid_product_match.clone();
        self.existing_hid_device_id = first_profile.hid_device_id.clone();

        // Build tree
        let mut tree = LearnTree::build(topology);

        // --- Load mappings from ALL profiles ---
        self.nav_encoder_mappings.clear();
        self.browse_encoder_addresses.clear();

        // Collect all loop_size mappings across profiles for encoder inference
        let mut all_loop_size_mappings: Vec<&ControlMapping> = Vec::new();

        for profile in &config.devices {
            let source = Self::source_device_for_profile(profile);

            // Load shift buttons
            for sb in &profile.shift_buttons {
                let def_id = match sb.physical_deck {
                    0 => "mod.shift_left",
                    _ => "mod.shift_right",
                };
                if let Some(node) = tree.find_by_id_mut(def_id) {
                    if let TreeNode::Mapping { mapped, status, .. } = node {
                        *mapped = Some(MappedControl {
                            address: sb.control.clone(),
                            hardware_type: HardwareType::Button,
                            shift_held: false,
                            source_device: source.clone(),
                        });
                        *status = MappingStatus::Existing;
                    }
                }
            }

            // Load layer toggle buttons
            if let DeckTargetConfig::Layer { ref toggle_left, ref toggle_right, .. } = profile.deck_target {
                if let Some(node) = tree.find_by_id_mut("mod.layer_toggle_left") {
                    if let TreeNode::Mapping { mapped, status, .. } = node {
                        if mapped.is_none() { // Don't overwrite if already loaded from another profile
                            *mapped = Some(MappedControl {
                                address: toggle_left.clone(),
                                hardware_type: HardwareType::Button,
                                shift_held: false,
                                source_device: source.clone(),
                            });
                            *status = MappingStatus::Existing;
                        }
                    }
                }
                if let Some(node) = tree.find_by_id_mut("mod.layer_toggle_right") {
                    if let TreeNode::Mapping { mapped, status, .. } = node {
                        if mapped.is_none() {
                            *mapped = Some(MappedControl {
                                address: toggle_right.clone(),
                                hardware_type: HardwareType::Button,
                                shift_held: false,
                                source_device: source.clone(),
                            });
                            *status = MappingStatus::Existing;
                        }
                    }
                }
            }

            // Load control mappings
            for cm in &profile.mappings {
                let deck_idx = if cm.physical_deck.is_some() {
                    cm.physical_deck
                } else {
                    cm.deck_index
                };

                let param_key_str: Option<String> = cm.params.keys().next().cloned();
                let param_value: Option<usize> = param_key_str.as_ref().and_then(|k| {
                    cm.params.get(k).and_then(|v| v.as_u64()).map(|n| n as usize)
                });

                let hw_type = cm.hardware_type.unwrap_or(HardwareType::Unknown);

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
                            source_device: source.clone(),
                        };
                        *mapped = Some(ctrl.clone());
                        *original = Some(ctrl);
                        *status = MappingStatus::Existing;
                    }
                }

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
                                source_device: source.clone(),
                            };
                            *mapped = Some(ctrl.clone());
                            *original = Some(ctrl);
                            *status = MappingStatus::Existing;
                        }
                    }
                }
            }

            // Load browse encoder/select mappings
            let browse_scroll_mappings: Vec<_> = profile.mappings.iter()
                .filter(|m| m.action == "browser.scroll" && m.mode.is_none())
                .collect();
            for cm in &browse_scroll_mappings {
                let hw_type = cm.hardware_type.unwrap_or(HardwareType::Encoder);
                let ctrl = MappedControl {
                    address: cm.control.clone(),
                    hardware_type: hw_type,
                    shift_held: false,
                    source_device: source.clone(),
                };
                self.browse_encoder_addresses.push(cm.control.clone());

                let target_node = match cm.physical_deck {
                    None | Some(0) => {
                        self.nav_encoder_mappings.push(ctrl.clone());
                        tree.find_mapping_node_mut("browser.scroll", None, None, None)
                    }
                    Some(idx) => {
                        tree.find_mapping_node_mut("browser.scroll", None, Some("index"), Some(idx))
                    }
                };
                if let Some(node) = target_node {
                    if let TreeNode::Mapping { mapped, status, .. } = node {
                        *mapped = Some(ctrl);
                        *status = MappingStatus::Existing;
                    }
                }
            }

            // Collect loop_size mappings for encoder inference
            all_loop_size_mappings.extend(
                profile.mappings.iter().filter(|m| m.action == "deck.loop_size")
            );
        }

        // Extract browse select addresses from tree (loaded across all profiles)
        self.browse_select_addresses.clear();
        if let Some(node) = tree.find_mapping_node_mut("browser.select", None, None, None) {
            if let TreeNode::Mapping { mapped: Some(ref ctrl), .. } = node {
                self.browse_select_addresses.push(ctrl.address.clone());
                self.nav_select_mapping = Some(ctrl.clone());
            }
        }

        // Infer shared decks from deck.loop_size addresses across all profiles
        self.loop_shared_decks.clear();
        for (i, m1) in all_loop_size_mappings.iter().enumerate() {
            let deck1 = m1.physical_deck.or(m1.deck_index).unwrap_or(i);
            for m2 in &all_loop_size_mappings[..i] {
                let deck2 = m2.physical_deck.or(m2.deck_index).unwrap_or(0);
                if m1.control == m2.control {
                    self.loop_shared_decks.push((deck1, deck2));
                    break;
                }
            }
        }

        // Insert Reset node at top (only for existing configs)
        self.has_existing_config = true;
        tree.roots.insert(0, TreeNode::Reset);

        // Expand nav section — cursor lands on first unmapped mapping (if any)
        tree.expand_navigation();

        self.tree = Some(tree);
        self.mode = LearnMode::TreeNavigation;
        self.status = "Existing config loaded. Edit and save.".to_string();
        self.update_highlight();
        self.rebuild_active_mappings();
    }

    /// Derive a source_device string for a profile (used when loading config).
    /// For HID profiles, uses the product match name. For MIDI, uses the port name.
    fn source_device_for_profile(profile: &DeviceProfile) -> Option<String> {
        if profile.device_type.as_deref() == Some("hid") {
            profile.hid_product_match.clone()
                .or_else(|| Some(profile.name.clone()))
        } else {
            profile.learned_port_name.clone()
                .or_else(|| Some(profile.port_match.clone()))
        }
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
            let source = self.last_captured.as_ref().and_then(|c| c.source_device.clone());
            self.finalize_with_hardware(hw_type, address, source);
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
                    // First browse encoder captured
                    self.browse_encoder_addresses.push(address);
                    self.nav_encoder_mappings.push(ctrl);
                    self.nav_capture_step = 1;
                    self.last_capture_time = Some(Instant::now());
                    self.status = "Press your BROWSE button — this will select tracks, confirm choices, and open folders".to_string();
                    log::info!("Learn: Browse encoder captured");
                } else if self.nav_capture_step == 1 {
                    // Reject leftover encoder CCs from fast rotation
                    if self.browse_encoder_addresses.iter().any(|a| *a == address) {
                        return;
                    }
                    // Browse select captured
                    self.browse_select_addresses.push(address);
                    self.nav_select_mapping = Some(ctrl);
                    self.mode = LearnMode::Setup;
                    self.last_capture_time = None;
                    self.status = "Configure your deck layout".to_string();
                    log::info!("Learn: Browse select captured, entering setup");
                }
            }
            LearnMode::TreeNavigation => {
                if let Some(ref mut tree) = self.tree {
                    // Check if we're about to map a browser.select (extra browse encoder press).
                    // In 2-deck setups, auto-fill deck.toggle_loop + deck.load_selected
                    // for the corresponding physical deck from the encoder press control.
                    let auto_fill_deck = if matches!(self.topology_choice, TopologyChoice::TwoDecks | TopologyChoice::TwoDecksLayer) {
                        tree.current_node().and_then(|node| {
                            if let TreeNode::Mapping { def, .. } = node {
                                if def.action == "browser.select" {
                                    // Main select (no index) → deck 0, index N → deck N
                                    Some(def.param_value.unwrap_or(0))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                    } else {
                        None
                    };

                    if tree.record_mapping(ctrl.clone()) {
                        log::info!("Learn: Recorded mapping on tree node");

                        // Auto-populate deck.toggle_loop and deck.load_selected from
                        // browse encoder presses — the encoder press doubles as loop
                        // toggle (performance mode) and context-aware load (browse mode).
                        if let Some(deck_idx) = auto_fill_deck {
                            if deck_idx < 2 {
                                for action in &["deck.toggle_loop", "deck.load_selected"] {
                                    if let Some(node) = tree.find_mapping_node_mut(
                                        action, Some(deck_idx), None, None,
                                    ) {
                                        if let TreeNode::Mapping { mapped, status, .. } = node {
                                            if *status == MappingStatus::Unmapped {
                                                *mapped = Some(ctrl.clone());
                                                *status = MappingStatus::New;
                                                log::info!("Learn: Auto-filled {} for deck {} from browse select", action, deck_idx);
                                            }
                                        }
                                    }
                                }
                            }
                        }

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
        LearnMode::ResetConfirm => "Reset Confirmation".to_string(),
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
        LearnMode::ResetConfirm => view_reset_confirm(state),
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

/// View for NavCapture phase — capture first browse encoder, then select button.
fn view_nav_capture(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let prompt_text = text(&state.status)
        .size(sz(14.0))
        .color(Color::from_rgb(0.9, 0.9, 0.95));

    let step_label = if state.nav_capture_step == 0 {
        "Step 1/2: Browse encoder"
    } else {
        "Step 2/2: Select button"
    };
    let step = text(step_label)
        .size(sz(11.0))
        .color(Color::from_rgb(0.5, 0.5, 0.6));

    let captured_display = if let Some(ref event) = state.last_captured {
        text(format!("Last: {}", event.display()))
            .size(sz(11.0))
            .color(Color::from_rgb(0.4, 0.6, 0.4))
    } else {
        text("Waiting for input...")
            .size(sz(11.0))
            .color(Color::from_rgb(0.4, 0.4, 0.5))
    };

    column![prompt_text, step, captured_display]
        .spacing(8)
        .into()
}

/// Render a setup option row: dot + label, with cursor highlight and click support.
fn setup_option_row<'a>(
    label: &'a str,
    is_selected: bool,
    is_cursor: bool,
    cursor_idx: usize,
) -> Element<'a, MidiLearnMessage> {
    let dot = if is_selected { "●" } else { "○" };
    let dot_color = if is_selected {
        Color::from_rgb(0.2, 0.8, 0.3)
    } else {
        Color::from_rgb(0.4, 0.4, 0.5)
    };
    let label_color = if is_cursor {
        Color::from_rgb(0.95, 0.95, 1.0)
    } else {
        Color::from_rgb(0.8, 0.8, 0.85)
    };

    let content = row![
        text(dot).size(sz(12.0)).color(dot_color),
        Space::new().width(6),
        text(label).size(sz(12.0)).color(label_color),
    ]
    .align_y(Alignment::Center);

    let clickable = button(content)
        .on_press(MidiLearnMessage::SelectRow(cursor_idx))
        .style(button::text)
        .width(Length::Fill);

    if is_cursor {
        container(clickable)
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
        container(clickable).width(Length::Fill).into()
    }
}

/// View for Setup phase — encoder-navigable flat list.
///
/// Items 0-2: topology, 3-4: performance style, 5-6: pad mode, 7: confirm.
/// Browse encoder scrolls, select button activates.
fn view_setup(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let dim = Color::from_rgb(0.5, 0.5, 0.6);
    let cursor = state.setup_cursor;

    let mut items: Vec<Element<MidiLearnMessage>> = Vec::new();

    // --- Topology ---
    items.push(text("Deck Layout").size(sz(13.0)).into());
    items.push(
        text("How many physical deck sections does your controller have?")
            .size(sz(10.0)).color(dim).into()
    );
    for (i, choice) in TopologyChoice::ALL.iter().enumerate() {
        items.push(setup_option_row(
            choice.label(),
            *choice == state.topology_choice,
            cursor == i,
            i,
        ));
    }
    // Show description of selected topology below the options
    items.push(
        text(state.topology_choice.description())
            .size(sz(10.0)).color(dim).into()
    );

    items.push(Space::new().height(2).into());

    // --- Performance style ---
    items.push(text("Performance Style").size(sz(13.0)).into());
    items.push(
        text("How should pad mode buttons (hot cue, slicer) work?")
            .size(sz(10.0)).color(dim).into()
    );
    items.push(setup_option_row("Toggle", !state.overlay_mode, cursor == 3, 3));
    items.push(setup_option_row("Overlay", state.overlay_mode, cursor == 4, 4));

    items.push(Space::new().height(2).into());

    // --- Pad mode ---
    items.push(text("Pad Mode Source").size(sz(13.0)).into());
    items.push(
        text("App: the app decides what pads do. Controller: each pad mode sends different MIDI notes.")
            .size(sz(10.0)).color(dim).into()
    );
    items.push(setup_option_row("App", state.pad_mode_source == PadModeSource::App, cursor == 5, 5));
    items.push(setup_option_row("Controller", state.pad_mode_source == PadModeSource::Controller, cursor == 6, 6));

    items.push(Space::new().height(4).into());

    // --- Confirm ---
    let confirm_label = row![
        text("▶").size(sz(12.0)).color(Color::from_rgb(0.2, 0.8, 0.3)),
        Space::new().width(6),
        text("Build Mapping Tree").size(sz(13.0)),
    ]
    .align_y(Alignment::Center);

    let confirm_btn = button(confirm_label)
        .on_press(MidiLearnMessage::ConfirmSetup)
        .style(button::text)
        .width(Length::Fill);

    if cursor == 7 {
        items.push(
            container(confirm_btn)
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
        );
    } else {
        items.push(container(confirm_btn).width(Length::Fill).into());
    }

    column(items).spacing(3).into()
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
                TreeNode::Mapping { def, deck_index, mapped, status, .. } => {
                    if is_cursor && !def.description.is_empty() {
                        cursor_description = Some(def.description);
                    }

                    // Check if this is a shared loop encoder node
                    let shared_source = if def.action == "deck.loop_size" {
                        deck_index.and_then(|di| {
                            state.loop_shared_decks.iter()
                                .find(|(shared, _)| *shared == di)
                                .map(|(_, source)| *source)
                        })
                    } else {
                        None
                    };

                    let (dot, dot_color) = match status {
                        MappingStatus::Unmapped => ("○", Color::from_rgb(0.4, 0.4, 0.5)),
                        MappingStatus::Existing => ("◆", Color::from_rgb(0.3, 0.4, 0.6)),
                        MappingStatus::New => ("●", Color::from_rgb(0.2, 0.8, 0.3)),
                        MappingStatus::Changed => ("◈", Color::from_rgb(0.9, 0.6, 0.1)),
                    };
                    let addr_text = if let Some(source) = shared_source {
                        format!("(shared with Deck {})", source + 1)
                    } else {
                        mapped
                            .as_ref()
                            .map(|m| format_address(&m.address))
                            .unwrap_or_else(|| {
                                if is_cursor && *status == MappingStatus::Unmapped {
                                    "(press control...)".to_string()
                                } else {
                                    String::new()
                                }
                            })
                    };
                    let addr_color = if shared_source.is_some() {
                        Color::from_rgb(0.4, 0.4, 0.5) // gray for shared
                    } else if mapped.is_some() {
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
                TreeNode::Reset => {
                    row![
                        text("⚠").size(sz(12.0)).color(Color::from_rgb(0.9, 0.3, 0.3)),
                        Space::new().width(4),
                        text("Reset All Mappings").size(sz(13.0)).color(Color::from_rgb(0.9, 0.3, 0.3)),
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
        .height(Length::Fixed(sz(TREE_VISIBLE_HEIGHT)))
        .width(Length::Fill)
        .id(LEARN_TREE_SCROLLABLE_ID.clone());

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

/// View for Reset Confirmation — confirm clearing all mappings.
fn view_reset_confirm(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let warning = text("Clear all mappings and start fresh?")
        .size(sz(14.0))
        .color(Color::from_rgb(0.9, 0.3, 0.3));

    let subtext = text("The saved config file will not be affected.")
        .size(sz(12.0))
        .color(Color::from_rgb(0.5, 0.5, 0.6));

    let cancel_style = if state.reset_confirm_cursor == 0 { button::primary } else { button::secondary };
    let reset_style = if state.reset_confirm_cursor == 1 { button::danger } else { button::secondary };

    let cancel_btn = button(text("← Cancel").size(sz(13.0)))
        .on_press(MidiLearnMessage::CancelReset)
        .style(cancel_style);

    let reset_btn = button(text("Reset All").size(sz(13.0)))
        .on_press(MidiLearnMessage::ConfirmReset)
        .style(reset_style);

    let button_row = row![cancel_btn, Space::new().width(Length::Fill), reset_btn]
        .align_y(Alignment::Center);

    column![
        Space::new().height(20),
        warning,
        Space::new().height(8),
        subtext,
        Space::new().height(20),
        button_row,
    ]
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

    let back_style = if state.verify_cursor == 0 { button::primary } else { button::secondary };
    let save_style = if state.verify_cursor == 1 { button::primary } else { button::secondary };

    let save_btn = button(text("Save").size(sz(13.0)))
        .on_press(MidiLearnMessage::Save)
        .style(save_style);

    let back_btn = button(text("← Back to Tree").size(sz(12.0)))
        .on_press(MidiLearnMessage::ScrollTree(0)) // Will be handled to go back to tree mode
        .style(back_style);

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
