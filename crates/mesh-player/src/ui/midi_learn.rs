//! MIDI Learn Mode - Integrated mapping tool
//!
//! Provides a guided workflow for creating MIDI controller profiles:
//! - Step-by-step control mapping with visual highlighting
//! - Live testing (mappings work immediately while learning)
//! - Config generation with same-note LED assumption
//!
//! Entry points:
//! - Settings tab "MIDI Learn" button
//! - `--midi-learn` command line flag

use std::collections::HashMap;
use std::time::{Duration, Instant};
use iced::widget::{button, column, container, row, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};
use mesh_midi::{
    ControlAddress, ControlBehavior, ControlMapping, DeckTargetConfig, DeviceProfile,
    FeedbackMapping, HardwareType, MidiAddress, MidiConfig, MidiSampleBuffer, ShiftButtonConfig,
};

/// Debounce duration for MIDI capture (prevents release/encoder spam from double-mapping)
/// 1 second gives time for button release and encoder settling
const CAPTURE_DEBOUNCE: Duration = Duration::from_millis(1000);

/// Phase of the MIDI learn workflow
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LearnPhase {
    /// Initial setup questions (controller name, deck count, etc.)
    #[default]
    Setup,
    /// Mapping transport controls (play, cue, sync, loop, beat jump, modes)
    Transport,
    /// Mapping performance pads (hot cues / slicer)
    Pads,
    /// Mapping stem mute buttons
    Stems,
    /// Mapping mixer controls (volume, filter, EQ)
    Mixer,
    /// Mapping browser and global controls
    Browser,
    /// Review and save
    Review,
}

impl LearnPhase {
    /// Get human-readable phase name
    pub fn name(&self) -> &'static str {
        match self {
            LearnPhase::Setup => "Setup",
            LearnPhase::Transport => "Transport",
            LearnPhase::Pads => "Performance Pads",
            LearnPhase::Stems => "Stem Controls",
            LearnPhase::Mixer => "Mixer",
            LearnPhase::Browser => "Browser",
            LearnPhase::Review => "Review",
        }
    }
}

/// Setup phase sub-steps
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupStep {
    #[default]
    ControllerName,
    DeckCount,
    LayerToggle,
    PadModeSource,
    /// Mode button behavior: permanent toggle vs momentary overlay
    ModeButtonBehavior,
    /// Left physical deck shift button
    ShiftButtonLeft,
    /// Right physical deck shift button
    ShiftButtonRight,
    /// Left layer toggle button (only when has_layer_toggle)
    ToggleButtonLeft,
    /// Right layer toggle button (only when has_layer_toggle)
    ToggleButtonRight,
}

/// UI element to highlight during learning
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightTarget {
    // Transport controls
    DeckPlay(usize),
    DeckCue(usize),
    DeckLoop(usize),

    // Loop size encoder (negative = halve, positive = double)
    DeckLoopEncoder(usize),

    // Beat jump
    DeckBeatJumpBack(usize),
    DeckBeatJumpForward(usize),

    // Mode buttons (per-deck)
    DeckHotCueMode(usize),
    DeckSlicerMode(usize),

    // Per-side mode buttons (4-deck momentary: side 0=left, 1=right)
    SideHotCueMode(usize),
    SideSlicerMode(usize),
    SideBrowseMode(usize),

    // Performance pads (deck, slot)
    DeckHotCue(usize, usize),
    DeckSlicerPad(usize, usize),

    // Stem controls (deck, stem_index 0-3)
    DeckStemMute(usize, usize),

    // Mixer controls (channel)
    MixerVolume(usize),
    MixerFilter(usize),
    MixerEqHi(usize),
    MixerEqMid(usize),
    MixerEqLo(usize),
    MixerCue(usize),

    // Master section
    MasterVolume,
    CueVolume,
    CueMix,

    // Browser (global — when not in layer mode)
    BrowserEncoder,
    BrowserSelect,
    // Browser (per physical deck — when in layer mode)
    BrowserEncoderDeck(usize),
    BrowserSelectDeck(usize),

    // FX preset browsing (separate encoder from browser)
    FxEncoder,
    FxSelect,

    // FX macro knobs (deck_index, macro_index 0-3)
    DeckFxMacro(usize, usize),

    // Deck load buttons (4-deck non-layered mode only)
    DeckLoad(usize),
}

impl HighlightTarget {
    /// Get human-readable description for the UI prompt
    pub fn description(&self) -> String {
        match self {
            HighlightTarget::DeckPlay(d) => format!("Press PLAY button on deck {}", d + 1),
            HighlightTarget::DeckCue(d) => format!("Press CUE button on deck {}", d + 1),
            HighlightTarget::DeckLoop(d) => format!("Press LOOP toggle on deck {}", d + 1),
            HighlightTarget::DeckLoopEncoder(d) => format!("Turn LOOP SIZE encoder on deck {} (halve/double)", d + 1),
            HighlightTarget::DeckBeatJumpBack(d) => format!("Press BEAT JUMP BACK on deck {}", d + 1),
            HighlightTarget::DeckBeatJumpForward(d) => format!("Press BEAT JUMP FORWARD on deck {}", d + 1),
            HighlightTarget::DeckHotCueMode(d) => format!("Press HOT CUE mode button on deck {}", d + 1),
            HighlightTarget::DeckSlicerMode(d) => format!("Press SLICER mode button on deck {}", d + 1),
            HighlightTarget::SideHotCueMode(side) => {
                let side_name = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side HOT CUE mode button", side_name)
            }
            HighlightTarget::SideSlicerMode(side) => {
                let side_name = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side SLICER mode button", side_name)
            }
            HighlightTarget::SideBrowseMode(side) => {
                let side_name = if *side == 0 { "LEFT" } else { "RIGHT" };
                format!("Press {} side BROWSE mode button (toggle)", side_name)
            }
            HighlightTarget::DeckHotCue(d, s) => {
                format!("Press HOT CUE pad {} on deck {}", s + 1, d + 1)
            }
            HighlightTarget::DeckSlicerPad(d, s) => {
                format!("Press SLICER pad {} on deck {}", s + 1, d + 1)
            }
            HighlightTarget::DeckStemMute(d, s) => {
                let stem_name = ["VOCALS", "DRUMS", "BASS", "OTHER"][*s];
                format!("Press {} mute on deck {}", stem_name, d + 1)
            }
            HighlightTarget::MixerVolume(ch) => format!("Move VOLUME fader on channel {}", ch + 1),
            HighlightTarget::MixerFilter(ch) => format!("Turn FILTER knob on channel {}", ch + 1),
            HighlightTarget::MixerEqHi(ch) => format!("Turn EQ HIGH knob on channel {}", ch + 1),
            HighlightTarget::MixerEqMid(ch) => format!("Turn EQ MID knob on channel {}", ch + 1),
            HighlightTarget::MixerEqLo(ch) => format!("Turn EQ LOW knob on channel {}", ch + 1),
            HighlightTarget::MixerCue(ch) => format!("Press CUE (headphone) button on channel {}", ch + 1),
            HighlightTarget::MasterVolume => "Move the MASTER volume fader".to_string(),
            HighlightTarget::CueVolume => "Move the CUE/HEADPHONE volume knob".to_string(),
            HighlightTarget::CueMix => "Move the CUE/MASTER MIX knob".to_string(),
            HighlightTarget::BrowserEncoder => "Turn the BROWSE encoder".to_string(),
            HighlightTarget::BrowserSelect => "Press the BROWSE encoder (or select button)".to_string(),
            HighlightTarget::BrowserEncoderDeck(d) => {
                let side = if *d == 0 { "LEFT" } else { "RIGHT" };
                format!("Turn the {} BROWSE encoder (or skip)", side)
            }
            HighlightTarget::BrowserSelectDeck(d) => {
                let side = if *d == 0 { "LEFT" } else { "RIGHT" };
                format!("Press the {} deck BROWSE select button", side)
            }
            HighlightTarget::DeckLoad(d) => format!("Press the LOAD button for deck {}", d + 1),
            HighlightTarget::FxEncoder => "Turn the FX SCROLL encoder (or skip)".to_string(),
            HighlightTarget::FxSelect => "Press the FX encoder to SELECT (or skip)".to_string(),
            HighlightTarget::DeckFxMacro(d, m) => {
                format!("Turn FX MACRO {} knob on deck {}", m + 1, d + 1)
            }
        }
    }
}

/// A learned mapping (input captured during learn mode)
#[derive(Debug, Clone)]
pub struct LearnedMapping {
    /// The target this mapping is for
    pub target: HighlightTarget,
    /// Protocol-agnostic control address
    pub address: ControlAddress,
    /// Detected or known hardware type (Button, Knob, Fader, Encoder, etc.)
    pub hardware_type: HardwareType,
    /// Source device name (for display and config generation)
    pub source_device: Option<String>,
}

/// Protocol-agnostic captured event during learn mode
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
    /// Format for display
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

    /// Check if this is a MIDI Note event
    pub fn is_midi_note(&self) -> bool {
        matches!(&self.address, ControlAddress::Midi(mesh_midi::MidiAddress::Note { .. }))
    }

    /// Check if this is a MIDI CC event
    pub fn is_midi_cc(&self) -> bool {
        matches!(&self.address, ControlAddress::Midi(mesh_midi::MidiAddress::CC { .. }))
    }

    /// Get MIDI channel (returns 0 for HID events)
    pub fn midi_channel(&self) -> u8 {
        match &self.address {
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { channel, .. }) => *channel,
            ControlAddress::Midi(mesh_midi::MidiAddress::CC { channel, .. }) => *channel,
            ControlAddress::Hid { .. } => 0,
        }
    }

    /// Get MIDI note/CC number (returns 0 for HID events)
    pub fn midi_number(&self) -> u8 {
        match &self.address {
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { note, .. }) => *note,
            ControlAddress::Midi(mesh_midi::MidiAddress::CC { cc, .. }) => *cc,
            ControlAddress::Hid { .. } => 0,
        }
    }
}

impl LearnedMapping {
    /// Check if this mapping is for a MIDI Note (or HID button)
    pub fn address_is_note(&self) -> bool {
        match &self.address {
            ControlAddress::Midi(mesh_midi::MidiAddress::Note { .. }) => true,
            ControlAddress::Hid { .. } => self.hardware_type == HardwareType::Button,
            _ => false,
        }
    }

    /// Check if this is a continuous control (MIDI CC knob/fader, or HID continuous)
    pub fn is_continuous(&self) -> bool {
        self.hardware_type.is_continuous()
    }
}

// Keep old type alias for in-progress migration of message types
pub type CapturedMidiEvent = CapturedEvent;

/// Messages for MIDI learn mode
#[derive(Debug, Clone)]
pub enum MidiLearnMessage {
    /// Start MIDI learn mode
    Start,
    /// Cancel and exit learn mode
    Cancel,
    /// Go to next step
    Next,
    /// Go to previous step
    Back,
    /// Skip current step
    Skip,
    /// Save the learned mappings
    Save,
    /// Save completed (with result)
    SaveComplete(Result<(), String>),

    // Setup phase inputs
    /// Update controller name input
    SetControllerName(String),
    /// Set number of physical decks (2 or 4)
    SetDeckCount(usize),
    /// Set whether controller has layer toggle buttons
    SetHasLayerToggle(bool),
    /// Set pad mode source (controller vs app driven)
    SetPadModeSource(mesh_midi::PadModeSource),
    /// Set mode button behavior (true = momentary overlay, false = permanent toggle)
    SetModeButtonBehavior(bool),
    /// Left shift button detected (or skipped)
    ShiftLeftDetected(Option<CapturedMidiEvent>),
    /// Right shift button detected (or skipped)
    ShiftRightDetected(Option<CapturedMidiEvent>),
    /// Left toggle button detected (or skipped)
    ToggleLeftDetected(Option<CapturedMidiEvent>),
    /// Right toggle button detected (or skipped)
    ToggleRightDetected(Option<CapturedMidiEvent>),

    /// MIDI event captured (used during mapping phase)
    MidiCaptured(CapturedMidiEvent),
}

/// MIDI Learn mode state
pub struct MidiLearnState {
    /// Whether learn mode is active
    pub is_active: bool,
    /// Current workflow phase
    pub phase: LearnPhase,
    /// Current step within the phase
    pub current_step: usize,
    /// Total steps in current phase
    pub total_steps: usize,
    /// UI element to highlight
    pub highlight_target: Option<HighlightTarget>,
    /// All learned mappings so far
    pub pending_mappings: Vec<LearnedMapping>,
    /// Last captured event (for display)
    pub last_captured: Option<CapturedEvent>,
    /// Timestamp of last successful capture (for debouncing)
    last_capture_time: Option<Instant>,

    // Hardware detection state
    /// Active sample buffer for hardware type detection (None when not sampling)
    pub detection_buffer: Option<MidiSampleBuffer>,
    /// Last detected hardware type (for display)
    pub detected_hardware: Option<HardwareType>,

    // Setup phase state
    /// Controller name (user input)
    pub controller_name: String,
    /// Number of physical decks (2 or 4)
    pub deck_count: usize,
    /// Whether controller has layer toggle buttons
    pub has_layer_toggle: bool,
    /// How pad button actions are determined (controller vs app driven)
    pub pad_mode_source: mesh_midi::PadModeSource,
    /// Whether mode buttons use momentary behavior (hold-to-activate overlay)
    pub momentary_mode_buttons: bool,
    /// Left shift button mapping (physical deck 0)
    pub shift_mapping_left: Option<CapturedMidiEvent>,
    /// Right shift button mapping (physical deck 1)
    pub shift_mapping_right: Option<CapturedMidiEvent>,
    /// Left layer toggle button mapping
    pub toggle_mapping_left: Option<CapturedMidiEvent>,
    /// Right layer toggle button mapping
    pub toggle_mapping_right: Option<CapturedMidiEvent>,
    /// Current setup step
    pub setup_step: SetupStep,

    /// Status message
    pub status: String,

    /// Actual port name captured from first MIDI event (normalized, without hardware ID)
    /// Used for precise device matching on reconnection
    pub captured_port_name: Option<String>,
}

impl Default for MidiLearnState {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiLearnState {
    /// Create a new MIDI learn state (inactive)
    pub fn new() -> Self {
        Self {
            is_active: false,
            phase: LearnPhase::Setup,
            current_step: 0,
            total_steps: 0,
            highlight_target: None,
            pending_mappings: Vec::new(),
            last_captured: None,
            last_capture_time: None,
            detection_buffer: None,
            detected_hardware: None,
            controller_name: String::new(),
            deck_count: 2,
            has_layer_toggle: false,
            pad_mode_source: Default::default(),
            momentary_mode_buttons: false,
            shift_mapping_left: None,
            shift_mapping_right: None,
            toggle_mapping_left: None,
            toggle_mapping_right: None,
            setup_step: SetupStep::ControllerName,
            status: String::new(),
            captured_port_name: None,
        }
    }

    /// Number of mixer channels to learn.
    /// Mixer controls (volume, EQ, filter, cue, stem mutes, FX macros) are physically
    /// per-channel on hardware, independent of the layer toggle. So with 2 physical
    /// decks + layer toggle = 4 mixer channels.
    fn num_mixer_channels(&self) -> usize {
        if self.has_layer_toggle {
            self.deck_count * 2
        } else {
            self.deck_count
        }
    }

    /// Check if a captured event should be accepted
    ///
    /// Filters out:
    /// - Note Off events (we only capture on press, release uses same note)
    /// - HID button releases (value == 0)
    /// - Events during debounce period (1 second after last capture)
    pub fn should_capture(&self, event: &CapturedEvent) -> bool {
        // Filter Note Off / button release events
        if event.is_midi_note() && event.value == 0 {
            return false;
        }
        // Filter HID button releases
        if matches!(&event.address, ControlAddress::Hid { .. }) && event.value == 0 {
            return false;
        }

        // Check debounce - must wait 1 second between captures
        if let Some(last_time) = self.last_capture_time {
            if last_time.elapsed() < CAPTURE_DEBOUNCE {
                return false;
            }
        }

        true
    }

    /// Mark that a capture just happened (for debouncing)
    pub fn mark_captured(&mut self) {
        self.last_capture_time = Some(Instant::now());
    }

    /// Start MIDI learn mode
    pub fn start(&mut self) {
        *self = Self::new();
        self.is_active = true;
        self.phase = LearnPhase::Setup;
        self.setup_step = SetupStep::ControllerName;
        self.status = "Enter your controller name".to_string();
    }

    /// Cancel and reset learn mode
    pub fn cancel(&mut self) {
        self.is_active = false;
        *self = Self::new();
    }

    /// Advance to the next step
    pub fn advance(&mut self) {
        match self.phase {
            LearnPhase::Setup => self.advance_setup(),
            LearnPhase::Transport => self.advance_transport(),
            LearnPhase::Pads => self.advance_pads(),
            LearnPhase::Stems => self.advance_stems(),
            LearnPhase::Mixer => self.advance_mixer(),
            LearnPhase::Browser => self.advance_browser(),
            LearnPhase::Review => {
                // Can't advance past review
            }
        }
    }

    /// Go back to the previous step
    pub fn go_back(&mut self) {
        match self.phase {
            LearnPhase::Setup => self.go_back_setup(),
            LearnPhase::Transport => self.go_back_transport(),
            LearnPhase::Pads => self.go_back_pads(),
            LearnPhase::Stems => self.go_back_stems(),
            LearnPhase::Mixer => self.go_back_mixer(),
            LearnPhase::Browser => self.go_back_browser(),
            LearnPhase::Review => {
                // Go back to last step of browser phase
                self.phase = LearnPhase::Browser;
                self.total_steps = self.browser_step_count();
                self.current_step = self.total_steps - 1;
                self.update_browser_target();
            }
        }
    }

    /// Record a captured event for the current target
    ///
    /// For HID events with known hardware_type: finalizes immediately (no detection needed).
    /// For MIDI events: starts hardware detection via MidiSampleBuffer.
    pub fn record_mapping(&mut self, event: CapturedEvent) {
        self.last_captured = Some(event.clone());

        if self.highlight_target.is_some() {
            // HID path: hardware type is already known from the driver
            if let Some(hw_type) = event.hardware_type {
                self.detected_hardware = Some(hw_type);
                self.pending_mappings.push(LearnedMapping {
                    target: self.highlight_target.unwrap(),
                    address: event.address.clone(),
                    hardware_type: hw_type,
                    source_device: event.source_device.clone(),
                });

                self.status = format!("Mapped as {:?} (HID)", hw_type);
                log::info!(
                    "Learn: Mapped {:?} as {:?} (HID: {:?})",
                    self.highlight_target.unwrap(),
                    hw_type,
                    event.address
                );

                self.detection_buffer = None;
                self.advance();
                return;
            }

            // MIDI path: start sampling for hardware detection
            let channel = event.midi_channel();
            let number = event.midi_number();
            let is_note = event.is_midi_note();

            self.detection_buffer = Some(MidiSampleBuffer::new(channel, number, is_note));

            // Add the first sample
            let is_note_on = is_note && event.value > 0;
            if let Some(ref mut buffer) = self.detection_buffer {
                buffer.add_sample(event.value, is_note_on, Some(number));
            }

            self.status = format!("Sampling: {} (move control...)", event.display());

            // For Note events (buttons), complete immediately
            if is_note {
                self.finalize_mapping();
            }
        }
    }

    /// Add a sample to the active detection buffer (MIDI only)
    ///
    /// Returns true if the buffer is now complete and ready to finalize
    pub fn add_detection_sample(&mut self, event: &CapturedEvent) -> bool {
        if let Some(ref mut buffer) = self.detection_buffer {
            let channel = event.midi_channel();
            let number = event.midi_number();
            let is_note = event.is_midi_note();

            // Check if event matches the control being sampled
            if buffer.matches(channel, number, is_note) {
                let is_note_on = is_note && event.value > 0;
                buffer.add_sample(event.value, is_note_on, Some(number));
                self.last_captured = Some(event.clone());

                // Update status with sample count
                let count = buffer.sample_count();
                let progress = (buffer.elapsed_ratio() * 100.0) as u8;
                self.status = format!("Sampling... {} samples ({}%)", count, progress);

                return buffer.is_complete();
            }
        }
        false
    }

    /// Check if detection buffer is complete (timed out or sufficient samples)
    pub fn is_detection_complete(&self) -> bool {
        self.detection_buffer
            .as_ref()
            .map(|b| b.is_complete())
            .unwrap_or(false)
    }

    /// Finalize the mapping with detected hardware type (MIDI detection path)
    pub fn finalize_mapping(&mut self) {
        if let (Some(target), Some(ref buffer)) = (self.highlight_target, &self.detection_buffer) {
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

            self.pending_mappings.push(LearnedMapping {
                target,
                address: address.clone(),
                hardware_type: hw_type,
                source_device: None, // MIDI source captured at port level
            });

            self.status = format!("Mapped as {:?}", hw_type);
            log::info!(
                "Learn: Mapped {:?} as {:?} ({:?})",
                target,
                hw_type,
                address,
            );

            // Clear buffer and advance
            self.detection_buffer = None;
            self.advance();
        }
    }

    /// Remove any existing mappings for a given target
    ///
    /// Used by go_back methods to remove mappings before re-learning.
    /// This is idempotent - safe to call even if no mapping exists.
    fn remove_mappings_for_target(&mut self, target: HighlightTarget) {
        self.pending_mappings.retain(|m| m.target != target);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Phase transitions
    // ─────────────────────────────────────────────────────────────────────

    fn advance_setup(&mut self) {
        match self.setup_step {
            SetupStep::ControllerName => {
                if !self.controller_name.is_empty() {
                    self.setup_step = SetupStep::DeckCount;
                    self.status = "How many physical decks?".to_string();
                }
            }
            SetupStep::DeckCount => {
                self.setup_step = SetupStep::LayerToggle;
                self.status = "Does your controller have layer toggle buttons?".to_string();
            }
            SetupStep::LayerToggle => {
                self.setup_step = SetupStep::PadModeSource;
                self.status = "How does your controller handle pad modes?".to_string();
            }
            SetupStep::PadModeSource => {
                self.setup_step = SetupStep::ModeButtonBehavior;
                self.status = "How should mode buttons (Hot Cue / Slicer) behave?".to_string();
            }
            SetupStep::ModeButtonBehavior => {
                self.setup_step = SetupStep::ShiftButtonLeft;
                self.status = "Press LEFT deck SHIFT button (or skip)".to_string();
            }
            SetupStep::ShiftButtonLeft => {
                self.setup_step = SetupStep::ShiftButtonRight;
                self.status = "Press RIGHT deck SHIFT button (or skip)".to_string();
            }
            SetupStep::ShiftButtonRight => {
                if self.has_layer_toggle {
                    self.setup_step = SetupStep::ToggleButtonLeft;
                    self.status = "Press LEFT LAYER TOGGLE button (or skip)".to_string();
                } else {
                    // No layer toggle — done with setup
                    self.enter_transport_phase();
                }
            }
            SetupStep::ToggleButtonLeft => {
                self.setup_step = SetupStep::ToggleButtonRight;
                self.status = "Press RIGHT LAYER TOGGLE button (or skip)".to_string();
            }
            SetupStep::ToggleButtonRight => {
                // Done with setup, move to transport
                self.enter_transport_phase();
            }
        }
    }

    fn go_back_setup(&mut self) {
        match self.setup_step {
            SetupStep::ControllerName => {
                // Can't go back from first step
            }
            SetupStep::DeckCount => {
                self.setup_step = SetupStep::ControllerName;
                self.status = "Enter your controller name".to_string();
            }
            SetupStep::LayerToggle => {
                self.setup_step = SetupStep::DeckCount;
                self.status = "How many physical decks?".to_string();
            }
            SetupStep::PadModeSource => {
                self.setup_step = SetupStep::LayerToggle;
                self.status = "Does your controller have layer toggle buttons?".to_string();
            }
            SetupStep::ModeButtonBehavior => {
                self.setup_step = SetupStep::PadModeSource;
                self.status = "How does your controller handle pad modes?".to_string();
            }
            SetupStep::ShiftButtonLeft => {
                self.setup_step = SetupStep::ModeButtonBehavior;
                self.status = "How should mode buttons (Hot Cue / Slicer) behave?".to_string();
            }
            SetupStep::ShiftButtonRight => {
                self.setup_step = SetupStep::ShiftButtonLeft;
                self.status = "Press LEFT deck SHIFT button (or skip)".to_string();
            }
            SetupStep::ToggleButtonLeft => {
                self.setup_step = SetupStep::ShiftButtonRight;
                self.status = "Press RIGHT deck SHIFT button (or skip)".to_string();
            }
            SetupStep::ToggleButtonRight => {
                self.setup_step = SetupStep::ToggleButtonLeft;
                self.status = "Press LEFT LAYER TOGGLE button (or skip)".to_string();
            }
        }
    }

    /// Number of extra virtual decks that need dedicated loop controls.
    /// In layer mode, loop controls are per-virtual-deck (like mixer channels),
    /// not per-physical-deck, since mixers typically have one encoder per channel.
    fn extra_loop_decks(&self) -> usize {
        self.num_mixer_channels().saturating_sub(self.deck_count)
    }

    fn transport_step_count(&self) -> usize {
        // 6 controls per physical deck (play, cue, loop, loop encoder, beat jump back/fwd)
        // + 2 extra loop controls (toggle + size) per additional virtual deck in layer mode
        self.deck_count * 6 + self.extra_loop_decks() * 2
    }

    fn enter_transport_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Transport;
        self.current_step = 0;
        self.total_steps = self.transport_step_count();
        self.update_transport_target();
    }

    fn update_transport_target(&mut self) {
        let base_steps = self.deck_count * 6;

        self.highlight_target = Some(if self.current_step < base_steps {
            let deck = self.current_step / 6;
            let control = self.current_step % 6;
            match control {
                0 => HighlightTarget::DeckPlay(deck),
                1 => HighlightTarget::DeckCue(deck),
                2 => HighlightTarget::DeckLoop(deck),
                3 => HighlightTarget::DeckLoopEncoder(deck),
                4 => HighlightTarget::DeckBeatJumpBack(deck),
                5 => HighlightTarget::DeckBeatJumpForward(deck),
                _ => unreachable!(),
            }
        } else {
            // Extra loop controls for additional virtual decks (layer mode)
            let extra_step = self.current_step - base_steps;
            let deck = self.deck_count + extra_step / 2;
            match extra_step % 2 {
                0 => HighlightTarget::DeckLoop(deck),
                1 => HighlightTarget::DeckLoopEncoder(deck),
                _ => unreachable!(),
            }
        });

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    fn advance_transport(&mut self) {
        self.current_step += 1;
        if self.current_step >= self.total_steps {
            self.enter_pads_phase();
        } else {
            self.update_transport_target();
        }
    }

    fn go_back_transport(&mut self) {
        // Remove mapping for current target (if any exists)
        if let Some(target) = self.highlight_target {
            self.remove_mappings_for_target(target);
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            self.update_transport_target();
        } else {
            // Go back to last setup step
            self.phase = LearnPhase::Setup;
            self.highlight_target = None;
            if self.has_layer_toggle {
                self.setup_step = SetupStep::ToggleButtonRight;
                self.status = "Press RIGHT LAYER TOGGLE button (or skip)".to_string();
            } else {
                self.setup_step = SetupStep::ShiftButtonRight;
                self.status = "Press RIGHT deck SHIFT button (or skip)".to_string();
            }
        }
    }

    /// Whether to use per-side mode buttons (4-deck momentary)
    fn use_side_mode_buttons(&self) -> bool {
        self.deck_count == 4 && self.momentary_mode_buttons
    }

    /// Steps per deck in the pads phase (for standard per-deck layout)
    fn pads_steps_per_deck(&self) -> usize {
        if self.pad_mode_source == mesh_midi::PadModeSource::Controller {
            // Controller mode: hot cue mode + 8 hot cue pads + slicer mode + 8 slicer pads = 18
            18
        } else {
            // App mode: hot cue mode + 8 pads + slicer mode = 10
            10
        }
    }

    /// Steps for a secondary deck (no mode button) in 4-deck momentary
    fn pads_steps_secondary_deck(&self) -> usize {
        if self.pad_mode_source == mesh_midi::PadModeSource::Controller {
            // 8 hot cue pads + 8 slicer pads = 16
            16
        } else {
            // 8 pads only
            8
        }
    }

    /// Total steps in the pads phase
    fn pads_total_steps(&self) -> usize {
        if self.use_side_mode_buttons() {
            // Decks 0,1 (primary): full steps. Decks 2,3 (secondary): no mode buttons.
            2 * self.pads_steps_per_deck() + 2 * self.pads_steps_secondary_deck()
        } else {
            self.deck_count * self.pads_steps_per_deck()
        }
    }

    fn enter_pads_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Pads;
        self.current_step = 0;
        self.total_steps = self.pads_total_steps();
        self.update_pads_target();
    }

    fn update_pads_target(&mut self) {
        if self.use_side_mode_buttons() {
            self.update_pads_target_side_mode();
        } else {
            self.update_pads_target_per_deck();
        }
    }

    /// Pads target for standard per-deck mode buttons
    fn update_pads_target_per_deck(&mut self) {
        // Per-deck layout:
        // Controller mode (18 steps): hot cue mode, 8 hot cue pads, slicer mode, 8 slicer pads
        // App mode (10 steps): hot cue mode, 8 pads, slicer mode
        let steps_per_deck = self.pads_steps_per_deck();
        let deck = self.current_step / steps_per_deck;
        let step_within_deck = self.current_step % steps_per_deck;

        self.highlight_target = Some(if self.pad_mode_source == mesh_midi::PadModeSource::Controller {
            // Controller mode layout:
            // 0: hot cue mode, 1-8: hot cue pads, 9: slicer mode, 10-17: slicer pads
            match step_within_deck {
                0 => HighlightTarget::DeckHotCueMode(deck),
                1..=8 => HighlightTarget::DeckHotCue(deck, step_within_deck - 1),
                9 => HighlightTarget::DeckSlicerMode(deck),
                10..=17 => HighlightTarget::DeckSlicerPad(deck, step_within_deck - 10),
                _ => unreachable!(),
            }
        } else {
            // App mode layout:
            // 0: hot cue mode, 1-8: pads, 9: slicer mode
            match step_within_deck {
                0 => HighlightTarget::DeckHotCueMode(deck),
                1..=8 => HighlightTarget::DeckHotCue(deck, step_within_deck - 1),
                9 => HighlightTarget::DeckSlicerMode(deck),
                _ => unreachable!(),
            }
        });

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    /// Pads target for 4-deck momentary (per-side mode buttons)
    ///
    /// Layout: Deck 0 (full) → Deck 1 (full) → Deck 2 (no mode btns) → Deck 3 (no mode btns)
    /// Primary decks (0,1) use SideHotCueMode/SideSlicerMode; secondary decks (2,3) skip mode buttons.
    fn update_pads_target_side_mode(&mut self) {
        let primary_steps = self.pads_steps_per_deck();
        let secondary_steps = self.pads_steps_secondary_deck();
        let is_controller = self.pad_mode_source == mesh_midi::PadModeSource::Controller;

        // Determine which deck and step-within-deck we're on
        let (deck, step_within, has_mode_buttons) = if self.current_step < primary_steps {
            // Deck 0 (primary, side 0)
            (0, self.current_step, true)
        } else if self.current_step < primary_steps * 2 {
            // Deck 1 (primary, side 1)
            (1, self.current_step - primary_steps, true)
        } else if self.current_step < primary_steps * 2 + secondary_steps {
            // Deck 2 (secondary, side 0)
            (2, self.current_step - primary_steps * 2, false)
        } else {
            // Deck 3 (secondary, side 1)
            (3, self.current_step - primary_steps * 2 - secondary_steps, false)
        };

        self.highlight_target = Some(if has_mode_buttons {
            // Primary deck: side mode button + pads + side slicer mode [+ slicer pads]
            let side = deck; // deck 0 → side 0, deck 1 → side 1
            if is_controller {
                match step_within {
                    0 => HighlightTarget::SideHotCueMode(side),
                    1..=8 => HighlightTarget::DeckHotCue(deck, step_within - 1),
                    9 => HighlightTarget::SideSlicerMode(side),
                    10..=17 => HighlightTarget::DeckSlicerPad(deck, step_within - 10),
                    _ => unreachable!(),
                }
            } else {
                match step_within {
                    0 => HighlightTarget::SideHotCueMode(side),
                    1..=8 => HighlightTarget::DeckHotCue(deck, step_within - 1),
                    9 => HighlightTarget::SideSlicerMode(side),
                    _ => unreachable!(),
                }
            }
        } else {
            // Secondary deck: pads only (no mode buttons)
            if is_controller {
                match step_within {
                    0..=7 => HighlightTarget::DeckHotCue(deck, step_within),
                    8..=15 => HighlightTarget::DeckSlicerPad(deck, step_within - 8),
                    _ => unreachable!(),
                }
            } else {
                HighlightTarget::DeckHotCue(deck, step_within)
            }
        });

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    fn advance_pads(&mut self) {
        self.current_step += 1;
        if self.current_step >= self.total_steps {
            self.enter_stems_phase();
        } else {
            self.update_pads_target();
        }
    }

    fn go_back_pads(&mut self) {
        // Remove mapping for current target (if any exists)
        if let Some(target) = self.highlight_target {
            self.remove_mappings_for_target(target);
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            self.update_pads_target();
        } else {
            // Go back to transport
            self.phase = LearnPhase::Transport;
            self.current_step = self.deck_count * 6 - 1;
            self.total_steps = self.deck_count * 6;
            self.update_transport_target();
        }
    }

    fn enter_stems_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Stems;
        self.current_step = 0;
        // 4 stem mute buttons per channel (per virtual deck, not layer-resolved)
        self.total_steps = self.num_mixer_channels() * 4;
        self.update_stems_target();
    }

    fn update_stems_target(&mut self) {
        let deck = self.current_step / 4;
        let stem = self.current_step % 4;

        self.highlight_target = Some(HighlightTarget::DeckStemMute(deck, stem));

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    fn advance_stems(&mut self) {
        self.current_step += 1;
        if self.current_step >= self.total_steps {
            self.enter_mixer_phase();
        } else {
            self.update_stems_target();
        }
    }

    fn go_back_stems(&mut self) {
        // Remove mapping for current target (if any exists)
        if let Some(target) = self.highlight_target {
            self.remove_mappings_for_target(target);
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            self.update_stems_target();
        } else {
            // Go back to pads
            let pads_total = self.pads_total_steps();
            self.phase = LearnPhase::Pads;
            self.current_step = pads_total - 1;
            self.total_steps = pads_total;
            self.update_pads_target();
        }
    }

    /// Total steps in the mixer phase:
    /// 6 channel controls per virtual deck + 4 FX macros per physical deck
    fn mixer_step_count(&self) -> usize {
        self.num_mixer_channels() * 6 + self.deck_count * 4
    }

    fn enter_mixer_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Mixer;
        self.current_step = 0;
        self.total_steps = self.mixer_step_count();
        self.update_mixer_target();
    }

    fn update_mixer_target(&mut self) {
        let channel_steps = self.num_mixer_channels() * 6;

        self.highlight_target = Some(if self.current_step < channel_steps {
            // Channel controls: 6 per virtual deck (volume, filter, eq hi/mid/lo, cue)
            let deck = self.current_step / 6;
            let control = self.current_step % 6;
            match control {
                0 => HighlightTarget::MixerVolume(deck),
                1 => HighlightTarget::MixerFilter(deck),
                2 => HighlightTarget::MixerEqHi(deck),
                3 => HighlightTarget::MixerEqMid(deck),
                4 => HighlightTarget::MixerEqLo(deck),
                5 => HighlightTarget::MixerCue(deck),
                _ => unreachable!(),
            }
        } else {
            // FX macros: 4 per physical deck (layer-resolved)
            let macro_step = self.current_step - channel_steps;
            let deck = macro_step / 4;
            let macro_idx = macro_step % 4;
            HighlightTarget::DeckFxMacro(deck, macro_idx)
        });

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    fn advance_mixer(&mut self) {
        self.current_step += 1;
        if self.current_step >= self.total_steps {
            self.enter_browser_phase();
        } else {
            self.update_mixer_target();
        }
    }

    fn go_back_mixer(&mut self) {
        // Remove mapping for current target (if any exists)
        if let Some(target) = self.highlight_target {
            self.remove_mappings_for_target(target);
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            self.update_mixer_target();
        } else {
            // Go back to stems (uses num_mixer_channels, same as enter_stems_phase)
            let channels = self.num_mixer_channels();
            self.phase = LearnPhase::Stems;
            self.current_step = channels * 4 - 1;
            self.total_steps = channels * 4;
            self.update_stems_target();
        }
    }

    /// Calculate number of steps in the browser phase
    fn browser_step_count(&self) -> usize {
        if self.has_layer_toggle {
            // FxEncoder, FxSelect, BrowserEncoder(0), BrowserSelect(0),
            // BrowserEncoder(1), BrowserSelect(1),
            // MasterVolume, CueVolume, CueMix
            9
        } else if self.deck_count == 4 {
            if self.momentary_mode_buttons {
                // FxEncoder, FxSelect, SideBrowseMode(0), SideBrowseMode(1),
                // DeckLoad(0-3), MasterVolume, CueVolume, CueMix
                // (No BrowserEncoderDeck — same physical encoder as DeckLoopEncoder,
                //  browser.scroll auto-generated from transport mapping with mode:browse)
                11
            } else {
                // FxEncoder, FxSelect, BrowserEncoderDeck(0), BrowserEncoderDeck(1),
                // DeckLoad(0-3), MasterVolume, CueVolume, CueMix
                11
            }
        } else {
            // FxEncoder, FxSelect, BrowserEncoder, BrowserSelect,
            // MasterVolume, CueVolume, CueMix
            7
        }
    }

    fn enter_browser_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Browser;
        self.current_step = 0;
        self.total_steps = self.browser_step_count();
        self.update_browser_target();
    }

    fn update_browser_target(&mut self) {
        self.highlight_target = Some(if self.has_layer_toggle {
            // Per-physical-deck browse layout:
            // 0: FxEncoder, 1: FxSelect,
            // 2: BrowserEncoder(left), 3: BrowserSelect(left),
            // 4: BrowserEncoder(right), 5: BrowserSelect(right),
            // 6: MasterVolume, 7: CueVolume, 8: CueMix,
            // 9 steps total (0-8)
            match self.current_step {
                0 => HighlightTarget::FxEncoder,
                1 => HighlightTarget::FxSelect,
                2 => HighlightTarget::BrowserEncoderDeck(0),
                3 => HighlightTarget::BrowserSelectDeck(0),
                4 => HighlightTarget::BrowserEncoderDeck(1),
                5 => HighlightTarget::BrowserSelectDeck(1),
                6 => HighlightTarget::MasterVolume,
                7 => HighlightTarget::CueVolume,
                _ => HighlightTarget::CueMix,
            }
        } else if self.deck_count == 4 && self.momentary_mode_buttons {
            // 4-deck momentary: browse mode buttons (no separate encoder — same as loop encoder)
            // 0: FxEncoder, 1: FxSelect,
            // 2: SideBrowseMode(0), 3: SideBrowseMode(1),
            // 4-7: DeckLoad(0..3), 8: MasterVolume, 9: CueVolume, 10: CueMix
            // 11 steps total (0-10)
            // (browser.scroll auto-generated from DeckLoopEncoder with mode:browse)
            match self.current_step {
                0 => HighlightTarget::FxEncoder,
                1 => HighlightTarget::FxSelect,
                2 => HighlightTarget::SideBrowseMode(0),
                3 => HighlightTarget::SideBrowseMode(1),
                4 => HighlightTarget::DeckLoad(0),
                5 => HighlightTarget::DeckLoad(1),
                6 => HighlightTarget::DeckLoad(2),
                7 => HighlightTarget::DeckLoad(3),
                8 => HighlightTarget::MasterVolume,
                9 => HighlightTarget::CueVolume,
                _ => HighlightTarget::CueMix,
            }
        } else if self.deck_count == 4 {
            // 4-deck non-layered: left/right browse encoders + dedicated load per deck
            // 0: FxEncoder, 1: FxSelect, 2: BrowserEncoderDeck(0), 3: BrowserEncoderDeck(1),
            // 4-7: DeckLoad(0..3), 8: MasterVolume, 9: CueVolume, 10: CueMix
            // 11 steps total (0-10)
            match self.current_step {
                0 => HighlightTarget::FxEncoder,
                1 => HighlightTarget::FxSelect,
                2 => HighlightTarget::BrowserEncoderDeck(0),
                3 => HighlightTarget::BrowserEncoderDeck(1),
                4 => HighlightTarget::DeckLoad(0),
                5 => HighlightTarget::DeckLoad(1),
                6 => HighlightTarget::DeckLoad(2),
                7 => HighlightTarget::DeckLoad(3),
                8 => HighlightTarget::MasterVolume,
                9 => HighlightTarget::CueVolume,
                _ => HighlightTarget::CueMix,
            }
        } else {
            // Global browse layout:
            // 0: FxEncoder, 1: FxSelect, 2: BrowserEncoder, 3: BrowserSelect,
            // 4: MasterVolume, 5: CueVolume, 6: CueMix
            // 7 steps total (0-6)
            match self.current_step {
                0 => HighlightTarget::FxEncoder,
                1 => HighlightTarget::FxSelect,
                2 => HighlightTarget::BrowserEncoder,
                3 => HighlightTarget::BrowserSelect,
                4 => HighlightTarget::MasterVolume,
                5 => HighlightTarget::CueVolume,
                _ => HighlightTarget::CueMix,
            }
        });

        if let Some(ref target) = self.highlight_target {
            self.status = target.description();
        }
    }

    fn advance_browser(&mut self) {
        self.current_step += 1;
        if self.current_step >= self.total_steps {
            self.enter_review_phase();
        } else {
            self.update_browser_target();
        }
    }

    fn go_back_browser(&mut self) {
        // Remove mapping for current target (if any exists)
        if let Some(target) = self.highlight_target {
            self.remove_mappings_for_target(target);
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            self.update_browser_target();
        } else {
            // Go back to mixer
            self.phase = LearnPhase::Mixer;
            self.current_step = self.deck_count * 10 - 1;
            self.total_steps = self.deck_count * 10;
            self.update_mixer_target();
        }
    }

    fn enter_review_phase(&mut self) {
        self.phase = LearnPhase::Review;
        self.current_step = 0;
        self.total_steps = 0;
        self.highlight_target = None;
        self.status = format!(
            "Review: {} mappings learned. Press Save to write config.",
            self.pending_mappings.len()
        );
    }

    /// Get the overall progress as (current, total)
    pub fn overall_progress(&self) -> (usize, usize) {
        // name, deck count, layer toggle, pad mode, mode behavior, shift left, shift right
        // + optionally toggle left, toggle right
        let setup_steps = if self.has_layer_toggle { 9 } else { 7 };
        let transport_steps = self.transport_step_count();
        let pads_steps = self.pads_total_steps();
        let mixer_channels = self.num_mixer_channels();
        let stems_steps = mixer_channels * 4; // 4 stem mute buttons per channel
        let mixer_steps = self.mixer_step_count(); // 6 channel controls per virtual deck + 4 FX macros per physical deck
        let browser_steps = self.browser_step_count();
        let total = setup_steps + transport_steps + pads_steps + stems_steps + mixer_steps + browser_steps;

        let current = match self.phase {
            LearnPhase::Setup => match self.setup_step {
                SetupStep::ControllerName => 0,
                SetupStep::DeckCount => 1,
                SetupStep::LayerToggle => 2,
                SetupStep::PadModeSource => 3,
                SetupStep::ModeButtonBehavior => 4,
                SetupStep::ShiftButtonLeft => 5,
                SetupStep::ShiftButtonRight => 6,
                SetupStep::ToggleButtonLeft => 7,
                SetupStep::ToggleButtonRight => 8,
            },
            LearnPhase::Transport => setup_steps + self.current_step,
            LearnPhase::Pads => setup_steps + transport_steps + self.current_step,
            LearnPhase::Stems => setup_steps + transport_steps + pads_steps + self.current_step,
            LearnPhase::Mixer => setup_steps + transport_steps + pads_steps + stems_steps + self.current_step,
            LearnPhase::Browser => {
                setup_steps + transport_steps + pads_steps + stems_steps + mixer_steps + self.current_step
            }
            LearnPhase::Review => total,
        };

        (current, total)
    }

    /// Generate a MidiConfig from the learned mappings
    ///
    /// Creates a DeviceProfile with:
    /// - Control mappings for all learned controls
    /// - LED feedback mappings (same-note assumption for buttons)
    /// - Deck target config based on deck_count and has_layer_toggle settings
    pub fn generate_config(&self) -> MidiConfig {
        /// State-specific RGB colors for HID feedback mappings.
        /// Returns (on_color, off_color, alt_on_color, alt_on_value).
        fn hid_feedback_colors(state: &str) -> (Option<[u8; 3]>, Option<[u8; 3]>, Option<[u8; 3]>, Option<u8>) {
            let dim = Some([8, 8, 8]);
            match state {
                // Play: light green on, dim off (evaluator handles pulsing)
                "deck.is_playing" => (Some([0, 180, 0]), dim, None, None),
                // Cue: orange when cueing, dim off
                "deck.is_cueing" => (Some([200, 100, 0]), dim, None, None),
                // Loop encoder: green on (playing), red when loop active (alt)
                "deck.loop_encoder" => (Some([0, 180, 0]), dim, Some([180, 0, 0]), Some(127)),
                // Hot cue set: amber when set, dim off (evaluator handles slicer overlay)
                "deck.hot_cue_set" => (Some([200, 140, 0]), Some([12, 12, 12]), None, None),
                // Slicer slice active: cyan when active, dim off
                "deck.slicer_slice_active" => (Some([0, 160, 180]), dim, None, None),
                // Mode buttons: blue when active, dim off
                "deck.hot_cue_mode" => (Some([0, 100, 200]), dim, None, None),
                "deck.slicer_mode" => (Some([160, 0, 180]), dim, None, None),
                // Stem mute: evaluator overrides with per-stem color, but set off color
                "deck.stem_muted" => (Some([0, 127, 0]), Some([6, 6, 6]), None, None),
                // Mixer cue (PFL): yellow when enabled
                "mixer.cue_enabled" => (Some([200, 180, 0]), dim, None, None),
                // Browse mode: white when active, dim off
                "side.browse_mode" => (Some([200, 200, 200]), Some([20, 20, 20]), None, None),
                // Default: green on, dim off
                _ => (Some([0, 127, 0]), dim, None, None),
            }
        }

        let mut mappings = Vec::new();
        let mut feedback = Vec::new();

        for learned in &self.pending_mappings {
            let control = learned.address.clone();
            let is_note = learned.address_is_note();

            // Determine action and deck targeting based on target
            let (action, physical_deck, deck_index, behavior, state) = match learned.target {
                // Transport controls - layer-resolved
                HighlightTarget::DeckPlay(d) => {
                    ("deck.play".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.is_playing"))
                }
                HighlightTarget::DeckCue(d) => {
                    ("deck.cue_press".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.is_cueing"))
                }
                HighlightTarget::DeckLoop(d) => {
                    ("deck.toggle_loop".to_string(), None, Some(d), ControlBehavior::Toggle, Some("deck.loop_encoder"))
                }

                // Loop size encoder (negative delta = halve, positive = double)
                HighlightTarget::DeckLoopEncoder(d) => {
                    ("deck.loop_size".to_string(), None, Some(d), ControlBehavior::Continuous, None)
                }

                // Beat jump
                HighlightTarget::DeckBeatJumpBack(d) => {
                    ("deck.beat_jump_backward".to_string(), Some(d), None, ControlBehavior::Momentary, None)
                }
                HighlightTarget::DeckBeatJumpForward(d) => {
                    ("deck.beat_jump_forward".to_string(), Some(d), None, ControlBehavior::Momentary, None)
                }

                // Mode buttons
                HighlightTarget::DeckHotCueMode(d) => {
                    let behavior = if self.momentary_mode_buttons { ControlBehavior::Momentary } else { ControlBehavior::Toggle };
                    ("deck.hot_cue_mode".to_string(), Some(d), None, behavior, Some("deck.hot_cue_mode"))
                }
                HighlightTarget::DeckSlicerMode(d) => {
                    let behavior = if self.momentary_mode_buttons { ControlBehavior::Momentary } else { ControlBehavior::Toggle };
                    ("deck.slicer_mode".to_string(), Some(d), None, behavior, Some("deck.slicer_mode"))
                }

                // Hot cue pads - layer-resolved
                HighlightTarget::DeckHotCue(d, _slot) => {
                    ("deck.hot_cue_press".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.hot_cue_set"))
                }

                // Slicer pads - layer-resolved (only in controller mode)
                HighlightTarget::DeckSlicerPad(d, _slice) => {
                    ("deck.slicer_trigger".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.slicer_slice_active"))
                }

                // Stem mute buttons — direct deck_index (not layer-resolved)
                HighlightTarget::DeckStemMute(d, _stem) => {
                    ("deck.stem_mute".to_string(), None, Some(d), ControlBehavior::Toggle, Some("deck.stem_muted"))
                }

                // Mixer controls - direct deck index (not layer-resolved)
                HighlightTarget::MixerVolume(ch) => {
                    ("mixer.volume".to_string(), None, Some(ch), ControlBehavior::Continuous, None)
                }
                HighlightTarget::MixerFilter(ch) => {
                    ("mixer.filter".to_string(), None, Some(ch), ControlBehavior::Continuous, None)
                }
                HighlightTarget::MixerEqHi(ch) => {
                    ("mixer.eq_hi".to_string(), None, Some(ch), ControlBehavior::Continuous, None)
                }
                HighlightTarget::MixerEqMid(ch) => {
                    ("mixer.eq_mid".to_string(), None, Some(ch), ControlBehavior::Continuous, None)
                }
                HighlightTarget::MixerEqLo(ch) => {
                    ("mixer.eq_lo".to_string(), None, Some(ch), ControlBehavior::Continuous, None)
                }
                HighlightTarget::MixerCue(ch) => {
                    ("mixer.cue".to_string(), None, Some(ch), ControlBehavior::Toggle, Some("mixer.cue_enabled"))
                }

                // Master controls
                HighlightTarget::MasterVolume => {
                    ("global.master_volume".to_string(), None, None, ControlBehavior::Continuous, None)
                }
                HighlightTarget::CueVolume => {
                    ("global.cue_volume".to_string(), None, None, ControlBehavior::Continuous, None)
                }
                HighlightTarget::CueMix => {
                    ("mixer.cue_mix".to_string(), None, None, ControlBehavior::Continuous, None)
                }

                // Browser controls
                HighlightTarget::BrowserEncoder => {
                    ("browser.scroll".to_string(), None, None, ControlBehavior::Continuous, None)
                }
                HighlightTarget::BrowserSelect => {
                    ("browser.select".to_string(), None, None, ControlBehavior::Momentary, None)
                }
                HighlightTarget::FxEncoder => {
                    ("global.fx_scroll".to_string(), None, None, ControlBehavior::Continuous, None)
                }
                HighlightTarget::FxSelect => {
                    ("global.fx_select".to_string(), None, None, ControlBehavior::Momentary, None)
                }
                HighlightTarget::DeckFxMacro(d, _m) => {
                    ("deck.fx_macro".to_string(), Some(d), None, ControlBehavior::Continuous, None)
                }
                // Per-physical-deck browser encoder (non-momentary only;
                // in momentary mode, browse.scroll is auto-generated from DeckLoopEncoder)
                HighlightTarget::BrowserEncoderDeck(pd) => {
                    ("browser.scroll".to_string(), Some(pd), None, ControlBehavior::Continuous, None)
                }
                HighlightTarget::BrowserSelectDeck(pd) => {
                    ("deck.load_selected".to_string(), Some(pd), None, ControlBehavior::Momentary, None)
                }
                // Per-deck load buttons (4-deck non-layered mode)
                HighlightTarget::DeckLoad(d) => {
                    ("deck.load_selected".to_string(), None, Some(d), ControlBehavior::Momentary, None)
                }

                // Per-side mode buttons (4-deck momentary mode)
                HighlightTarget::SideHotCueMode(side) => {
                    let primary_deck = if side == 0 { 0 } else { 1 };
                    let behavior = if self.momentary_mode_buttons { ControlBehavior::Momentary } else { ControlBehavior::Toggle };
                    ("deck.hot_cue_mode".to_string(), Some(primary_deck), None, behavior, Some("deck.hot_cue_mode"))
                }
                HighlightTarget::SideSlicerMode(side) => {
                    let primary_deck = if side == 0 { 0 } else { 1 };
                    let behavior = if self.momentary_mode_buttons { ControlBehavior::Momentary } else { ControlBehavior::Toggle };
                    ("deck.slicer_mode".to_string(), Some(primary_deck), None, behavior, Some("deck.slicer_mode"))
                }
                HighlightTarget::SideBrowseMode(side) => {
                    ("side.browse_mode".to_string(), Some(side), None, ControlBehavior::Toggle, Some("side.browse_mode"))
                }
            };

            // Create control mapping
            let mut params = HashMap::new();
            match learned.target {
                HighlightTarget::DeckHotCue(_, slot) => {
                    params.insert("slot".to_string(), serde_yaml::Value::Number(slot.into()));
                }
                HighlightTarget::DeckSlicerPad(_, slice) => {
                    params.insert("pad".to_string(), serde_yaml::Value::Number(slice.into()));
                }
                HighlightTarget::DeckStemMute(_, stem) => {
                    params.insert("stem".to_string(), serde_yaml::Value::Number(stem.into()));
                }
                HighlightTarget::DeckFxMacro(_, m) => {
                    params.insert("macro".to_string(), serde_yaml::Value::Number(m.into()));
                }
                HighlightTarget::SideHotCueMode(side) | HighlightTarget::SideSlicerMode(side) => {
                    // Per-side mode buttons control two decks
                    let decks: Vec<serde_yaml::Value> = if side == 0 {
                        vec![serde_yaml::Value::Number(0.into()), serde_yaml::Value::Number(2.into())]
                    } else {
                        vec![serde_yaml::Value::Number(1.into()), serde_yaml::Value::Number(3.into())]
                    };
                    params.insert("decks".to_string(), serde_yaml::Value::Sequence(decks));
                }
                HighlightTarget::SideBrowseMode(side) => {
                    params.insert("side".to_string(), serde_yaml::Value::Number(side.into()));
                }
                // Note: BrowserEncoderDeck no longer appears in momentary mode — browse.scroll
                // is auto-generated from DeckLoopEncoder instead
                _ => {}
            }

            // Use detected hardware type to determine encoder mode
            let encoder_mode = if !is_note {
                learned.hardware_type.default_encoder_mode()
            } else {
                None
            };

            // Adjust behavior based on detected hardware type
            let actual_behavior = if learned.hardware_type.is_continuous() && behavior == ControlBehavior::Momentary {
                // Continuous hardware mapped to button action - keep momentary for adapter
                ControlBehavior::Momentary
            } else if learned.hardware_type.is_continuous() {
                ControlBehavior::Continuous
            } else {
                behavior
            };

            // Per-deck browser select gets browse_back as shift action
            let shift_action = match learned.target {
                HighlightTarget::BrowserSelectDeck(_) => Some("deck.browse_back".to_string()),
                _ => None,
            };

            // Determine mode condition for pad mappings under momentary mode
            let mode = if self.momentary_mode_buttons {
                match learned.target {
                    HighlightTarget::DeckHotCue(_, _) => Some("hot_cue".to_string()),
                    HighlightTarget::DeckSlicerPad(_, _) => Some("slicer".to_string()),
                    _ => None,
                }
            } else {
                None
            };

            // Feedback mode: stem mute feedback gets "performance" mode so it only
            // shows in performance mode (not in hot_cue/slicer overlay modes where
            // the same pads show different state)
            let feedback_mode = if self.momentary_mode_buttons {
                match learned.target {
                    HighlightTarget::DeckHotCue(_, _) => Some("hot_cue".to_string()),
                    HighlightTarget::DeckSlicerPad(_, _) => Some("slicer".to_string()),
                    HighlightTarget::DeckStemMute(_, _) => Some("performance".to_string()),
                    _ => None,
                }
            } else {
                None
            };

            mappings.push(ControlMapping {
                control: control.clone(),
                action,
                physical_deck,
                deck_index,
                params: params.clone(),
                behavior: actual_behavior,
                shift_action,
                encoder_mode,
                hardware_type: Some(learned.hardware_type),
                mode: mode.clone(),
            });

            // For dual-purpose encoders in momentary mode, auto-generate a
            // browser.scroll mapping from the loop encoder (same physical control).
            // Only for the first 2 decks (0,1) to avoid duplicates from decks 2,3
            // which share the same physical encoder on each side.
            if self.momentary_mode_buttons {
                if let HighlightTarget::DeckLoopEncoder(d) = learned.target {
                    if d < 2 {
                        mappings.push(ControlMapping {
                            control: control.clone(),
                            action: "browser.scroll".to_string(),
                            physical_deck: Some(d),
                            deck_index: None,
                            params: HashMap::new(),
                            behavior: ControlBehavior::Continuous,
                            shift_action: None,
                            encoder_mode,
                            hardware_type: Some(learned.hardware_type),
                            mode: Some("browse".to_string()),
                        });
                    }
                }
            }

            // Generate LED feedback for buttons with state (same-note/same-control assumption)
            if let Some(state_name) = state {
                let is_hid = matches!(&control, ControlAddress::Hid { .. });
                if is_note || is_hid {
                    // HID buttons get state-specific RGB colors, MIDI gets value-based feedback
                    let (on_color, off_color, alt_on_color, alt_on_value) = if is_hid {
                        hid_feedback_colors(state_name)
                    } else {
                        (None, None, None, None)
                    };
                    feedback.push(FeedbackMapping {
                        state: state_name.to_string(),
                        physical_deck,
                        deck_index,
                        params,
                        output: control,
                        on_value: 127,
                        off_value: 0,
                        alt_on_value,
                        on_color,
                        off_color,
                        alt_on_color,
                        mode: feedback_mode,
                    });
                }
            }
        }

        // Build deck target config
        let deck_target = if self.has_layer_toggle && self.deck_count == 2 {
            // Build toggle controls from learned mappings
            let make_toggle_addr = |event: &Option<CapturedEvent>| -> ControlAddress {
                match event {
                    Some(e) => e.address.clone(),
                    None => ControlAddress::Midi(MidiAddress::Note { channel: 0, note: 0x00 }), // Fallback (skipped)
                }
            };

            DeckTargetConfig::Layer {
                toggle_left: make_toggle_addr(&self.toggle_mapping_left),
                toggle_right: make_toggle_addr(&self.toggle_mapping_right),
                layer_a: vec![0, 1],
                layer_b: vec![2, 3],
            }
        } else {
            // Direct mapping (default)
            let mut channel_to_deck = HashMap::new();
            for i in 0..4 {
                channel_to_deck.insert(i, i as usize);
            }
            DeckTargetConfig::Direct { channel_to_deck }
        };

        // Build shift buttons from per-side mappings
        let mut shift_buttons = Vec::new();
        let make_shift_addr = |event: &CapturedEvent| -> ControlAddress {
            event.address.clone()
        };
        if let Some(ref event) = self.shift_mapping_left {
            shift_buttons.push(ShiftButtonConfig {
                control: make_shift_addr(event),
                physical_deck: 0,
            });
        }
        if let Some(ref event) = self.shift_mapping_right {
            shift_buttons.push(ShiftButtonConfig {
                control: make_shift_addr(event),
                physical_deck: 1,
            });
        }

        // Add layer toggle LED feedback with alt_on_value for color differentiation
        if self.has_layer_toggle {
            if let Some(ref event) = self.toggle_mapping_left {
                let is_hid = matches!(&event.address, ControlAddress::Hid { .. });
                if event.is_midi_note() || is_hid {
                    let (on_color, alt_on_color) = if is_hid {
                        (Some([127, 0, 0]), Some([0, 127, 0]))  // Red=Layer A, Green=Layer B
                    } else {
                        (None, None)
                    };
                    feedback.push(FeedbackMapping {
                        state: "deck.layer_active".to_string(),
                        physical_deck: Some(0),
                        deck_index: None,
                        params: HashMap::new(),
                        output: event.address.clone(),
                        on_value: 127,    // Layer A (MIDI)
                        off_value: 0,
                        alt_on_value: Some(50), // Layer B (MIDI)
                        on_color,
                        off_color: Some([20, 20, 20]),
                        alt_on_color,
                        mode: None,
                    });
                }
            }
            if let Some(ref event) = self.toggle_mapping_right {
                let is_hid = matches!(&event.address, ControlAddress::Hid { .. });
                if event.is_midi_note() || is_hid {
                    let (on_color, alt_on_color) = if is_hid {
                        (Some([127, 0, 0]), Some([0, 127, 0]))  // Red=Layer A, Green=Layer B
                    } else {
                        (None, None)
                    };
                    feedback.push(FeedbackMapping {
                        state: "deck.layer_active".to_string(),
                        physical_deck: Some(1),
                        deck_index: None,
                        params: HashMap::new(),
                        output: event.address.clone(),
                        on_value: 127,    // Layer A (MIDI)
                        off_value: 0,
                        alt_on_value: Some(50), // Layer B (MIDI)
                        on_color,
                        off_color: Some([20, 20, 20]),
                        alt_on_color,
                        mode: None,
                    });
                }
            }
        }

        // Detect if any learned mappings came from an HID device
        let hid_source = self.pending_mappings.iter()
            .find_map(|m| {
                if let ControlAddress::Hid { .. } = &m.address {
                    m.source_device.clone()
                } else {
                    None
                }
            });

        // Extract device_id from the first HID address in pending mappings
        let hid_device_id = self.pending_mappings.iter().find_map(|m| {
            if let ControlAddress::Hid { device_id, .. } = &m.address {
                Some(device_id.clone())
            } else {
                None
            }
        });

        let profile = DeviceProfile {
            name: self.controller_name.clone(),
            // Use captured port name for exact matching, fall back to user-entered name for fuzzy match
            port_match: self.captured_port_name.clone().unwrap_or_else(|| self.controller_name.clone()),
            learned_port_name: self.captured_port_name.clone(),
            device_type: hid_source.as_ref().map(|_| "hid".to_string()),
            hid_product_match: hid_source,
            hid_device_id,
            deck_target,
            pad_mode_source: self.pad_mode_source,
            shift_buttons,
            mappings,
            feedback,
            momentary_mode_buttons: self.momentary_mode_buttons,
            color_note_offsets: mesh_midi::detect_color_note_offsets(&self.controller_name),
        };

        MidiConfig {
            devices: vec![profile],
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UI Views
// ─────────────────────────────────────────────────────────────────────────────

/// Create the highlight border style
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

/// Render the bottom drawer for MIDI learn mode
pub fn view_drawer(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    if !state.is_active {
        return Space::new().height(0).into();
    }

    let (current, total) = state.overall_progress();
    let progress_text = format!(
        "Step {}/{} • {}",
        current + 1,
        total,
        state.phase.name()
    );

    // Header row with title and action buttons
    let title = text("MIDI LEARN").size(16);
    let progress = text(progress_text).size(12);

    let save_btn = button(text("Save").size(12))
        .on_press_maybe(if state.phase == LearnPhase::Review {
            Some(MidiLearnMessage::Save)
        } else {
            None
        })
        .style(if state.phase == LearnPhase::Review {
            button::primary
        } else {
            button::secondary
        });

    let cancel_btn = button(text("Cancel").size(12))
        .on_press(MidiLearnMessage::Cancel)
        .style(button::secondary);

    let header = row![
        title,
        Space::new().width(10),
        progress,
        Space::new().width(Length::Fill),
        save_btn,
        Space::new().width(5),
        cancel_btn,
    ]
    .align_y(Alignment::Center);

    // Content depends on phase
    let content: Element<MidiLearnMessage> = match state.phase {
        LearnPhase::Setup => view_setup_phase(state),
        LearnPhase::Review => view_review_phase(state),
        _ => view_mapping_phase(state),
    };

    // Navigation row
    let back_btn = button(text("← Back").size(12))
        .on_press(MidiLearnMessage::Back)
        .style(button::secondary);

    let skip_btn = button(text("Skip →").size(12))
        .on_press(MidiLearnMessage::Skip)
        .style(button::secondary);

    let nav_row = row![back_btn, Space::new().width(Length::Fill), skip_btn].width(Length::Fill);

    let drawer_content = column![header, content, nav_row]
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

/// View for the setup phase
fn view_setup_phase(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    match state.setup_step {
        SetupStep::ControllerName => {
            let label = text("Controller Name:").size(14);
            let input = text_input("e.g., DDJ-400", &state.controller_name)
                .on_input(MidiLearnMessage::SetControllerName)
                .width(Length::Fixed(200.0));
            let next_btn = button(text("Next").size(12))
                .on_press_maybe(if !state.controller_name.is_empty() {
                    Some(MidiLearnMessage::Next)
                } else {
                    None
                })
                .style(button::primary);

            row![label, Space::new().width(10), input, Space::new().width(10), next_btn]
                .align_y(Alignment::Center)
                .into()
        }
        SetupStep::DeckCount => {
            let label = text("Physical decks:").size(14);
            let two_btn = button(text("2 decks").size(12))
                .on_press(MidiLearnMessage::SetDeckCount(2))
                .style(if state.deck_count == 2 {
                    button::primary
                } else {
                    button::secondary
                });
            let four_btn = button(text("4 decks").size(12))
                .on_press(MidiLearnMessage::SetDeckCount(4))
                .style(if state.deck_count == 4 {
                    button::primary
                } else {
                    button::secondary
                });
            let next_btn = button(text("Next").size(12))
                .on_press(MidiLearnMessage::Next)
                .style(button::primary);

            row![
                label,
                Space::new().width(10),
                two_btn,
                Space::new().width(5),
                four_btn,
                Space::new().width(20),
                next_btn
            ]
            .align_y(Alignment::Center)
            .into()
        }
        SetupStep::LayerToggle => {
            let label = text("Layer toggle buttons (for 4-deck mode on 2-deck controller)?").size(14);
            let yes_btn = button(text("Yes").size(12))
                .on_press(MidiLearnMessage::SetHasLayerToggle(true))
                .style(if state.has_layer_toggle {
                    button::primary
                } else {
                    button::secondary
                });
            let no_btn = button(text("No").size(12))
                .on_press(MidiLearnMessage::SetHasLayerToggle(false))
                .style(if !state.has_layer_toggle {
                    button::primary
                } else {
                    button::secondary
                });
            let next_btn = button(text("Next").size(12))
                .on_press(MidiLearnMessage::Next)
                .style(button::primary);

            row![
                label,
                Space::new().width(10),
                yes_btn,
                Space::new().width(5),
                no_btn,
                Space::new().width(20),
                next_btn
            ]
            .align_y(Alignment::Center)
            .into()
        }
        SetupStep::PadModeSource => {
            // Explanation of pad mode source
            let label = text("Pad buttons behavior:").size(14);
            let explanation = text(
                "Controller-driven: Pads send different MIDI notes in hot cue vs slicer mode (e.g. DDJ-SB2). \
                 App-driven: Same notes in all modes, app decides what they do."
            ).size(11);

            let controller_btn = button(text("Controller").size(12))
                .on_press(MidiLearnMessage::SetPadModeSource(mesh_midi::PadModeSource::Controller))
                .style(if state.pad_mode_source == mesh_midi::PadModeSource::Controller {
                    button::primary
                } else {
                    button::secondary
                });
            let app_btn = button(text("App").size(12))
                .on_press(MidiLearnMessage::SetPadModeSource(mesh_midi::PadModeSource::App))
                .style(if state.pad_mode_source == mesh_midi::PadModeSource::App {
                    button::primary
                } else {
                    button::secondary
                });
            let next_btn = button(text("Next").size(12))
                .on_press(MidiLearnMessage::Next)
                .style(button::primary);

            column![
                row![
                    label,
                    Space::new().width(10),
                    controller_btn,
                    Space::new().width(5),
                    app_btn,
                    Space::new().width(20),
                    next_btn
                ].align_y(Alignment::Center),
                explanation
            ]
            .spacing(5)
            .into()
        }
        SetupStep::ModeButtonBehavior => {
            let label = text("Mode button behavior:").size(14);
            let explanation = text(
                "Permanent: Mode buttons toggle between Hot Cue and Slicer mode. Pads always act according to the current mode.\n\
                 Momentary: Hold to temporarily activate Hot Cue or Slicer mode. When released, pads return to their primary action. Best for compact controllers."
            ).size(11);

            let permanent_btn = button(text("Permanent").size(12))
                .on_press(MidiLearnMessage::SetModeButtonBehavior(false))
                .style(if !state.momentary_mode_buttons {
                    button::primary
                } else {
                    button::secondary
                });
            let momentary_btn = button(text("Momentary").size(12))
                .on_press(MidiLearnMessage::SetModeButtonBehavior(true))
                .style(if state.momentary_mode_buttons {
                    button::primary
                } else {
                    button::secondary
                });
            let next_btn = button(text("Next").size(12))
                .on_press(MidiLearnMessage::Next)
                .style(button::primary);

            column![
                row![
                    label,
                    Space::new().width(10),
                    permanent_btn,
                    Space::new().width(5),
                    momentary_btn,
                    Space::new().width(20),
                    next_btn
                ].align_y(Alignment::Center),
                explanation
            ]
            .spacing(5)
            .into()
        }
        SetupStep::ShiftButtonLeft | SetupStep::ShiftButtonRight => {
            let side = if state.setup_step == SetupStep::ShiftButtonLeft { "LEFT" } else { "RIGHT" };
            let label = text(format!("Press {} deck SHIFT button (or skip):", side)).size(14);
            let last_midi = if let Some(ref event) = state.last_captured {
                text(format!("Detected: {}", event.display())).size(12)
            } else {
                text("Waiting for MIDI input...").size(12)
            };

            row![label, Space::new().width(20), last_midi]
                .align_y(Alignment::Center)
                .into()
        }
        SetupStep::ToggleButtonLeft | SetupStep::ToggleButtonRight => {
            let side = if state.setup_step == SetupStep::ToggleButtonLeft { "LEFT" } else { "RIGHT" };
            let label = text(format!("Press {} LAYER TOGGLE button (or skip):", side)).size(14);
            let last_midi = if let Some(ref event) = state.last_captured {
                text(format!("Detected: {}", event.display())).size(12)
            } else {
                text("Waiting for MIDI input...").size(12)
            };

            row![label, Space::new().width(20), last_midi]
                .align_y(Alignment::Center)
                .into()
        }
    }
}

/// View for mapping phases (Transport, Pads, Mixer, Browser)
fn view_mapping_phase(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let prompt = text(&state.status).size(14);

    let last_midi = if let Some(ref event) = state.last_captured {
        text(format!("Last: {} ✓", event.display())).size(12)
    } else {
        text("Waiting for MIDI input...").size(12)
    };

    // Show detected hardware type if available
    let hw_info = if let Some(hw) = state.detected_hardware {
        text(format!("Detected: {:?}", hw)).size(12)
    } else {
        text("").size(12)
    };

    row![
        prompt,
        Space::new().width(Length::Fill),
        hw_info,
        Space::new().width(20),
        last_midi
    ]
    .align_y(Alignment::Center)
    .into()
}

/// View for the review phase
fn view_review_phase(state: &MidiLearnState) -> Element<'_, MidiLearnMessage> {
    let summary = text(format!(
        "Controller: {} • {} physical decks • {} mappings learned",
        state.controller_name,
        state.deck_count,
        state.pending_mappings.len()
    ))
    .size(14);

    let hint = text("Your mappings are working in live mode. Press Save to write the config file.")
        .size(12);

    column![summary, hint].spacing(5).into()
}
