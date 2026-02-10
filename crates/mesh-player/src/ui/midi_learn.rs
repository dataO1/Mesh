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
    ControlBehavior, ControlMapping, DeckTargetConfig, DeviceProfile, FeedbackMapping,
    HardwareType, MidiConfig, MidiControlConfig, MidiSampleBuffer,
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
    ShiftButton,
}

/// UI element to highlight during learning
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightTarget {
    // Transport controls
    DeckPlay(usize),
    DeckCue(usize),
    DeckLoop(usize),

    // Loop controls
    DeckLoopHalve(usize),
    DeckLoopDouble(usize),

    // Beat jump
    DeckBeatJumpBack(usize),
    DeckBeatJumpForward(usize),

    // Mode buttons
    DeckHotCueMode(usize),
    DeckSlicerMode(usize),

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

    // Browser
    BrowserEncoder,
    BrowserSelect,

    // FX preset browsing (separate encoder from browser)
    FxEncoder,
    FxSelect,

    // FX macro knobs (deck_index, macro_index 0-3)
    DeckFxMacro(usize, usize),

    // Load buttons (deck)
    DeckLoad(usize),
}

impl HighlightTarget {
    /// Get human-readable description for the UI prompt
    pub fn description(&self) -> String {
        match self {
            HighlightTarget::DeckPlay(d) => format!("Press PLAY button on deck {}", d + 1),
            HighlightTarget::DeckCue(d) => format!("Press CUE button on deck {}", d + 1),
            HighlightTarget::DeckLoop(d) => format!("Press LOOP toggle on deck {}", d + 1),
            HighlightTarget::DeckLoopHalve(d) => format!("Press LOOP HALVE (÷2) on deck {}", d + 1),
            HighlightTarget::DeckLoopDouble(d) => format!("Press LOOP DOUBLE (×2) on deck {}", d + 1),
            HighlightTarget::DeckBeatJumpBack(d) => format!("Press BEAT JUMP BACK on deck {}", d + 1),
            HighlightTarget::DeckBeatJumpForward(d) => format!("Press BEAT JUMP FORWARD on deck {}", d + 1),
            HighlightTarget::DeckHotCueMode(d) => format!("Press HOT CUE mode button on deck {}", d + 1),
            HighlightTarget::DeckSlicerMode(d) => format!("Press SLICER mode button on deck {}", d + 1),
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
            HighlightTarget::FxEncoder => "Turn the FX SCROLL encoder (or skip)".to_string(),
            HighlightTarget::FxSelect => "Press the FX encoder to SELECT (or skip)".to_string(),
            HighlightTarget::DeckFxMacro(d, m) => {
                format!("Turn FX MACRO {} knob on deck {}", m + 1, d + 1)
            }
            HighlightTarget::DeckLoad(d) => format!("Press LOAD button for deck {}", d + 1),
        }
    }
}

/// A learned MIDI mapping (input captured during learn mode)
#[derive(Debug, Clone)]
pub struct LearnedMapping {
    /// The target this mapping is for
    pub target: HighlightTarget,
    /// MIDI channel (0-15)
    pub channel: u8,
    /// Note number (for Note messages) or CC number (for CC messages)
    pub number: u8,
    /// Whether this is a Note or CC message
    pub is_note: bool,
    /// Detected hardware type (Button, Knob, Fader, Encoder, etc.)
    pub hardware_type: HardwareType,
}

/// Raw MIDI event captured during learn mode
#[derive(Debug, Clone)]
pub struct CapturedMidiEvent {
    /// MIDI channel (0-15)
    pub channel: u8,
    /// Note or CC number
    pub number: u8,
    /// Value (0-127)
    pub value: u8,
    /// True if Note On/Off, false if CC
    pub is_note: bool,
}

impl CapturedMidiEvent {
    /// Format for display
    pub fn display(&self) -> String {
        let msg_type = if self.is_note { "Note" } else { "CC" };
        format!(
            "{} ch{} 0x{:02X} val={}",
            msg_type, self.channel, self.number, self.value
        )
    }
}

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
    /// Shift button detected (or skipped)
    ShiftDetected(Option<CapturedMidiEvent>),

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
    /// Last captured MIDI event (for display)
    pub last_captured: Option<CapturedMidiEvent>,
    /// Timestamp of last successful capture (for debouncing)
    last_capture_time: Option<Instant>,

    // Hardware detection state
    /// Active sample buffer for hardware type detection (None when not sampling)
    pub detection_buffer: Option<MidiSampleBuffer>,
    /// Last detected hardware type (for display)
    pub detected_hardware: Option<HardwareType>,

    // Encoder press capture state
    /// True when waiting for user to press an encoder after rotation was captured
    pub awaiting_encoder_press: bool,
    /// The target we just mapped (to associate encoder press with it)
    encoder_rotation_target: Option<HighlightTarget>,

    // Setup phase state
    /// Controller name (user input)
    pub controller_name: String,
    /// Number of physical decks (2 or 4)
    pub deck_count: usize,
    /// Whether controller has layer toggle buttons
    pub has_layer_toggle: bool,
    /// How pad button actions are determined (controller vs app driven)
    pub pad_mode_source: mesh_midi::PadModeSource,
    /// Shift button mapping (if detected)
    pub shift_mapping: Option<CapturedMidiEvent>,
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
            awaiting_encoder_press: false,
            encoder_rotation_target: None,
            controller_name: String::new(),
            deck_count: 2,
            has_layer_toggle: false,
            pad_mode_source: Default::default(),
            shift_mapping: None,
            setup_step: SetupStep::ControllerName,
            status: String::new(),
            captured_port_name: None,
        }
    }

    /// Check if a MIDI event should be captured
    ///
    /// Filters out:
    /// - Note Off events (we only capture on press, release uses same note)
    /// - Events during debounce period (1 second after last capture)
    /// - CC values below threshold (filters encoder noise)
    pub fn should_capture(&self, event: &CapturedMidiEvent) -> bool {
        // Filter Note Off events - we only want button presses
        // The mapping system knows release uses the same note
        if event.is_note && event.value == 0 {
            return false;
        }

        // For CC events, require a minimum value to filter initial encoder touches
        // Value > 10 for absolute, or significant relative movement
        if !event.is_note && event.value < 10 && event.value > 117 {
            // Values 0-10 or 118-127 might be noise/initial state
            // Actually for relative encoders, 64 is center, so this logic needs refinement
            // For now, just accept any CC - the debounce will handle rapid values
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
                self.total_steps = 5 + self.deck_count;
                self.current_step = self.total_steps - 1;
                self.update_browser_target();
            }
        }
    }

    /// Record a captured MIDI event for the current target
    ///
    /// This starts the hardware detection process by creating a sample buffer.
    /// Call `add_sample` for subsequent events, then `finalize_mapping` when complete.
    pub fn record_mapping(&mut self, event: CapturedMidiEvent) {
        self.last_captured = Some(event.clone());

        if self.highlight_target.is_some() {
            // Start sampling for hardware detection
            self.detection_buffer = Some(MidiSampleBuffer::new(
                event.channel,
                event.number,
                event.is_note,
            ));

            // Add the first sample
            let is_note_on = event.is_note && event.value > 0;
            if let Some(ref mut buffer) = self.detection_buffer {
                buffer.add_sample(event.value, is_note_on, Some(event.number));
            }

            self.status = format!("Sampling: {} (move control...)", event.display());

            // For Note events (buttons), complete immediately
            if event.is_note {
                self.finalize_mapping();
            }
        }
    }

    /// Add a sample to the active detection buffer
    ///
    /// Returns true if the buffer is now complete and ready to finalize
    pub fn add_detection_sample(&mut self, event: &CapturedMidiEvent) -> bool {
        if let Some(ref mut buffer) = self.detection_buffer {
            // Check if event matches the control being sampled
            if buffer.matches(event.channel, event.number, event.is_note) {
                let is_note_on = event.is_note && event.value > 0;
                buffer.add_sample(event.value, is_note_on, Some(event.number));
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

    /// Check if a target is an encoder that should prompt for press capture
    fn should_prompt_encoder_press(target: HighlightTarget, hw_type: HardwareType) -> bool {
        // Only prompt for encoder press if we detected an encoder-type control
        if !matches!(hw_type, HardwareType::Encoder | HardwareType::JogWheel) {
            return false;
        }

        // These targets are encoders that typically have a press function
        matches!(target, HighlightTarget::BrowserEncoder | HighlightTarget::FxEncoder)
    }

    /// Finalize the mapping with detected hardware type
    pub fn finalize_mapping(&mut self) {
        if let (Some(target), Some(ref buffer)) = (self.highlight_target, &self.detection_buffer) {
            let hw_type = buffer.analyze();
            self.detected_hardware = Some(hw_type);

            self.pending_mappings.push(LearnedMapping {
                target,
                channel: buffer.get_channel(),
                number: buffer.get_number(),
                is_note: buffer.is_note(),
                hardware_type: hw_type,
            });

            self.status = format!("Mapped as {:?}", hw_type);
            log::info!(
                "MIDI Learn: Mapped {:?} as {:?} (ch={}, num={})",
                target,
                hw_type,
                buffer.get_channel(),
                buffer.get_number()
            );

            // Clear buffer
            self.detection_buffer = None;

            // Check if we should prompt for encoder press
            if Self::should_prompt_encoder_press(target, hw_type) {
                self.awaiting_encoder_press = true;
                self.encoder_rotation_target = Some(target);
                self.last_capture_time = None; // Reset debounce so press is captured immediately
                let press_target = match target {
                    HighlightTarget::FxEncoder => HighlightTarget::FxSelect,
                    _ => HighlightTarget::BrowserSelect,
                };
                self.highlight_target = Some(press_target);
                self.status = "Now PRESS the encoder (or skip if it doesn't click)".to_string();
                self.last_captured = None; // Clear to show waiting state
                log::info!("MIDI Learn: Prompting for encoder press");
            } else {
                self.advance();
            }
        }
    }

    /// Record an encoder press mapping (called when awaiting_encoder_press is true)
    pub fn record_encoder_press(&mut self, event: CapturedMidiEvent) {
        if !self.awaiting_encoder_press {
            return;
        }

        // Determine the correct press target based on which encoder was rotated
        let press_target = match self.encoder_rotation_target {
            Some(HighlightTarget::FxEncoder) => HighlightTarget::FxSelect,
            _ => HighlightTarget::BrowserSelect,
        };

        // Store the encoder press mapping
        self.pending_mappings.push(LearnedMapping {
            target: press_target,
            channel: event.channel,
            number: event.number,
            is_note: event.is_note,
            hardware_type: HardwareType::Button, // Encoder press is always a button
        });

        self.last_captured = Some(event.clone());
        self.status = format!("Encoder press mapped: {}", event.display());
        log::info!(
            "MIDI Learn: Encoder press mapped (ch={}, num={}, is_note={})",
            event.channel,
            event.number,
            event.is_note
        );

        // Clear encoder press state and advance
        self.awaiting_encoder_press = false;
        self.encoder_rotation_target = None;
        self.mark_captured();
        self.advance();
    }

    /// Skip encoder press capture
    pub fn skip_encoder_press(&mut self) {
        if self.awaiting_encoder_press {
            log::info!("MIDI Learn: Encoder press skipped");
            self.awaiting_encoder_press = false;
            self.encoder_rotation_target = None;
            self.advance();
        }
    }

    /// Remove any existing mappings for a given target
    ///
    /// Used by go_back methods to remove mappings before re-learning.
    /// This is idempotent - safe to call even if no mapping exists.
    fn remove_mappings_for_target(&mut self, target: HighlightTarget) {
        self.pending_mappings.retain(|m| {
            if m.target == target {
                return false;
            }
            // If removing browser encoder, also remove its paired press mapping
            if target == HighlightTarget::BrowserEncoder && m.target == HighlightTarget::BrowserSelect {
                return false;
            }
            // If removing FX encoder, also remove its paired press mapping
            if target == HighlightTarget::FxEncoder && m.target == HighlightTarget::FxSelect {
                return false;
            }
            true
        });
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
                self.setup_step = SetupStep::ShiftButton;
                self.status = "Press your SHIFT button (or skip)".to_string();
            }
            SetupStep::ShiftButton => {
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
            SetupStep::ShiftButton => {
                self.setup_step = SetupStep::PadModeSource;
                self.status = "How does your controller handle pad modes?".to_string();
            }
        }
    }

    fn enter_transport_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Transport;
        self.current_step = 0;
        // 7 controls per deck:
        // play, cue, loop, loop halve, loop double, beat jump back, beat jump forward
        // (mode buttons moved to pads phase for better workflow)
        self.total_steps = self.deck_count * 7;
        self.update_transport_target();
    }

    fn update_transport_target(&mut self) {
        let deck = self.current_step / 7;
        let control = self.current_step % 7;

        self.highlight_target = Some(match control {
            0 => HighlightTarget::DeckPlay(deck),
            1 => HighlightTarget::DeckCue(deck),
            2 => HighlightTarget::DeckLoop(deck),
            3 => HighlightTarget::DeckLoopHalve(deck),
            4 => HighlightTarget::DeckLoopDouble(deck),
            5 => HighlightTarget::DeckBeatJumpBack(deck),
            6 => HighlightTarget::DeckBeatJumpForward(deck),
            _ => unreachable!(),
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
            // Go back to setup
            self.phase = LearnPhase::Setup;
            self.setup_step = SetupStep::ShiftButton;
            self.highlight_target = None;
            self.status = "Press your SHIFT button (or skip)".to_string();
        }
    }

    /// Calculate the number of steps per deck in the pads phase
    fn pads_steps_per_deck(&self) -> usize {
        if self.pad_mode_source == mesh_midi::PadModeSource::Controller {
            // Controller mode: hot cue mode + 8 hot cue pads + slicer mode + 8 slicer pads = 18
            18
        } else {
            // App mode: hot cue mode + 8 pads + slicer mode = 10
            10
        }
    }

    fn enter_pads_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Pads;
        self.current_step = 0;
        self.total_steps = self.deck_count * self.pads_steps_per_deck();
        self.update_pads_target();
    }

    fn update_pads_target(&mut self) {
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
            self.current_step = self.deck_count * 7 - 1;
            self.total_steps = self.deck_count * 7;
            self.update_transport_target();
        }
    }

    fn enter_stems_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Stems;
        self.current_step = 0;
        // 4 stem mute buttons per deck
        self.total_steps = self.deck_count * 4;
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
            let steps_per_deck = self.pads_steps_per_deck();
            self.phase = LearnPhase::Pads;
            self.current_step = self.deck_count * steps_per_deck - 1;
            self.total_steps = self.deck_count * steps_per_deck;
            self.update_pads_target();
        }
    }

    fn enter_mixer_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Mixer;
        self.current_step = 0;
        // 10 controls per channel: 6 mixer (volume, filter, eq hi/mid/lo, cue) + 4 FX macros
        self.total_steps = self.deck_count * 10;
        self.update_mixer_target();
    }

    fn update_mixer_target(&mut self) {
        let deck = self.current_step / 10;
        let control = self.current_step % 10;

        self.highlight_target = Some(match control {
            0 => HighlightTarget::MixerVolume(deck),
            1 => HighlightTarget::MixerFilter(deck),
            2 => HighlightTarget::MixerEqHi(deck),
            3 => HighlightTarget::MixerEqMid(deck),
            4 => HighlightTarget::MixerEqLo(deck),
            5 => HighlightTarget::MixerCue(deck),
            n @ 6..=9 => HighlightTarget::DeckFxMacro(deck, n - 6),
            _ => unreachable!(),
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
            // Go back to stems
            self.phase = LearnPhase::Stems;
            self.current_step = self.deck_count * 4 - 1;
            self.total_steps = self.deck_count * 4;
            self.update_stems_target();
        }
    }

    fn enter_browser_phase(&mut self) {
        self.last_capture_time = None;
        self.phase = LearnPhase::Browser;
        self.current_step = 0;
        // Browser: FX encoder + browser encoder + master vol + cue vol + cue mix + load buttons per deck
        // = 1 (FX encoder) + 1 (browser encoder) + 3 (master) + deck_count (load)
        self.total_steps = 5 + self.deck_count;
        self.update_browser_target();
    }

    fn update_browser_target(&mut self) {
        self.highlight_target = Some(match self.current_step {
            0 => HighlightTarget::FxEncoder,
            1 => HighlightTarget::BrowserEncoder,
            2 => HighlightTarget::MasterVolume,
            3 => HighlightTarget::CueVolume,
            4 => HighlightTarget::CueMix,
            n => HighlightTarget::DeckLoad(n - 5),
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
        // Clear encoder press state if active
        if self.awaiting_encoder_press {
            self.awaiting_encoder_press = false;
            self.encoder_rotation_target = None;
        }

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
        let setup_steps = 5; // name, deck count, layer toggle, pad mode, shift
        let transport_steps = self.deck_count * 7; // play, cue, loop, loop halve/double, beat jump
        // Pads phase includes mode buttons + pads:
        // Controller mode: (hot cue mode + 8 pads + slicer mode + 8 pads) = 18 per deck
        // App mode: (hot cue mode + 8 pads + slicer mode) = 10 per deck
        let pads_steps = self.deck_count * self.pads_steps_per_deck();
        let stems_steps = self.deck_count * 4; // 4 stem mute buttons per deck
        let mixer_steps = self.deck_count * 10; // volume, filter, eq hi/mid/lo, cue, 4 FX macros
        let browser_steps = 5 + self.deck_count; // FX encoder, browser encoder, master vol, cue vol, cue mix, load buttons
        let total = setup_steps + transport_steps + pads_steps + stems_steps + mixer_steps + browser_steps;

        let current = match self.phase {
            LearnPhase::Setup => match self.setup_step {
                SetupStep::ControllerName => 0,
                SetupStep::DeckCount => 1,
                SetupStep::LayerToggle => 2,
                SetupStep::PadModeSource => 3,
                SetupStep::ShiftButton => 4,
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
        let mut mappings = Vec::new();
        let mut feedback = Vec::new();

        for learned in &self.pending_mappings {
            let control = if learned.is_note {
                MidiControlConfig::note(learned.channel, learned.number)
            } else {
                MidiControlConfig::cc(learned.channel, learned.number)
            };

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
                    ("deck.toggle_loop".to_string(), Some(d), None, ControlBehavior::Toggle, Some("deck.loop_active"))
                }

                // Loop controls
                HighlightTarget::DeckLoopHalve(d) => {
                    ("deck.loop_halve".to_string(), Some(d), None, ControlBehavior::Momentary, None)
                }
                HighlightTarget::DeckLoopDouble(d) => {
                    ("deck.loop_double".to_string(), Some(d), None, ControlBehavior::Momentary, None)
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
                    ("deck.hot_cue_mode".to_string(), Some(d), None, ControlBehavior::Toggle, Some("deck.hot_cue_mode"))
                }
                HighlightTarget::DeckSlicerMode(d) => {
                    ("deck.slicer_mode".to_string(), Some(d), None, ControlBehavior::Toggle, Some("deck.slicer_mode"))
                }

                // Hot cue pads - layer-resolved
                HighlightTarget::DeckHotCue(d, _slot) => {
                    ("deck.hot_cue_press".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.hot_cue_set"))
                }

                // Slicer pads - layer-resolved (only in controller mode)
                HighlightTarget::DeckSlicerPad(d, _slice) => {
                    ("deck.slicer_trigger".to_string(), Some(d), None, ControlBehavior::Momentary, Some("deck.slicer_slice_active"))
                }

                // Stem mute buttons
                HighlightTarget::DeckStemMute(d, _stem) => {
                    ("deck.stem_mute".to_string(), Some(d), None, ControlBehavior::Toggle, Some("deck.stem_muted"))
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
                HighlightTarget::DeckLoad(d) => {
                    ("deck.load_selected".to_string(), Some(d), None, ControlBehavior::Momentary, None)
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
                _ => {}
            }

            // Use detected hardware type to determine encoder mode
            let encoder_mode = if !learned.is_note {
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

            mappings.push(ControlMapping {
                control: control.clone(),
                action,
                physical_deck,
                deck_index,
                params: params.clone(),
                behavior: actual_behavior,
                shift_action: None,
                encoder_mode,
                hardware_type: Some(learned.hardware_type),
            });

            // Generate LED feedback for buttons with state (same-note assumption)
            if let Some(state_name) = state {
                if learned.is_note {
                    feedback.push(FeedbackMapping {
                        state: state_name.to_string(),
                        physical_deck,
                        deck_index,
                        params,
                        output: control,
                        on_value: 127,
                        off_value: 0,
                        layer: None,
                    });
                }
            }
        }

        // Build deck target config
        let deck_target = if self.has_layer_toggle && self.deck_count == 2 {
            // 2-deck controller with layer toggle
            // We don't have the toggle buttons learned, so use a placeholder
            // The user would need to manually add these or we could add a step
            DeckTargetConfig::Layer {
                toggle_left: MidiControlConfig::note(0, 0x72),  // Placeholder
                toggle_right: MidiControlConfig::note(1, 0x72), // Placeholder
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

        // Build shift config
        let shift = self.shift_mapping.as_ref().map(|event| {
            if event.is_note {
                MidiControlConfig::note(event.channel, event.number)
            } else {
                MidiControlConfig::cc(event.channel, event.number)
            }
        });

        let profile = DeviceProfile {
            name: self.controller_name.clone(),
            // Use captured port name for exact matching, fall back to user-entered name for fuzzy match
            port_match: self.captured_port_name.clone().unwrap_or_else(|| self.controller_name.clone()),
            learned_port_name: self.captured_port_name.clone(),
            deck_target,
            pad_mode_source: self.pad_mode_source,
            shift,
            mappings,
            feedback,
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
        SetupStep::ShiftButton => {
            let label = text("Press your SHIFT button (or skip if none):").size(14);
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
        if state.awaiting_encoder_press {
            text(format!("Detected: {:?} - now press it!", hw)).size(12)
        } else {
            text(format!("Detected: {:?}", hw)).size(12)
        }
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
