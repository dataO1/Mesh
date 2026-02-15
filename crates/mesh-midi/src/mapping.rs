//! Control-to-action mapping engine
//!
//! Maps abstract control events to application actions based on device configuration.
//! Works identically for MIDI and HID input.

use crate::config::{ControlBehavior, ControlMapping, DeviceProfile, EncoderMode};
use crate::messages::{BrowserAction, DeckAction, GlobalAction, MidiMessage, MixerAction};
use crate::normalize::{encoder_to_delta, normalize_cc_value, range_for_action, ControlRange};
use crate::shared_state::SharedState;
use crate::types::{ControlAddress, ControlEvent, ControlValue};
use std::collections::HashMap;
use std::sync::Arc;

/// Action registry - defines available mappable actions
///
/// The system knows the expected value range for each action.
pub struct ActionRegistry {
    /// Map of action ID to metadata
    actions: HashMap<String, ActionInfo>,
}

/// Information about a mappable action
#[derive(Debug, Clone)]
pub struct ActionInfo {
    /// Whether this action targets a deck
    pub deck_targetable: bool,
    /// Value range for continuous controls
    pub value_range: ControlRange,
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionRegistry {
    /// Create a new registry with all default actions
    pub fn new() -> Self {
        let mut actions = HashMap::new();

        // Deck actions (deck_targetable = true)
        for action in [
            "deck.play",
            "deck.cue_press",
            "deck.cue_release",
            "deck.sync",
            "deck.hot_cue_press",
            "deck.hot_cue_clear",
            "deck.toggle_loop",
            "deck.loop_halve",
            "deck.loop_double",
            "deck.loop_size",
            "deck.loop_in",
            "deck.loop_out",
            "deck.beat_jump_forward",
            "deck.beat_jump_backward",
            "deck.slicer_trigger",
            "deck.slicer_assign",
            "deck.slicer_mode",
            "deck.hot_cue_mode",
            "deck.slicer_reset",
            "deck.stem_mute",
            "deck.stem_solo",
            "deck.stem_select",
            "deck.slip",
            "deck.key_match",
            "deck.load_selected",
            "deck.browse_back",
            "deck.pad_press",
            "deck.pad_release",
        ] {
            actions.insert(
                action.to_string(),
                ActionInfo {
                    deck_targetable: true,
                    value_range: ControlRange::Unit,
                },
            );
        }

        // Mixer actions
        actions.insert("mixer.volume".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("mixer.filter".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Bipolar });
        for action in ["mixer.eq_hi", "mixer.eq_mid", "mixer.eq_lo"] {
            actions.insert(action.to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Eq });
        }
        actions.insert("mixer.cue".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("mixer.crossfader".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });

        // Browser actions
        actions.insert("browser.scroll".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("browser.select".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("browser.back".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });

        // Global actions
        actions.insert("global.bpm".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Custom { min: 60.0, max: 200.0 } });
        actions.insert("global.master_volume".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("global.cue_volume".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("mixer.cue_mix".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("global.fx_scroll".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });
        actions.insert("global.fx_select".to_string(), ActionInfo { deck_targetable: false, value_range: ControlRange::Unit });

        // FX macro knobs (per-deck)
        actions.insert("deck.fx_macro".to_string(), ActionInfo { deck_targetable: true, value_range: ControlRange::Unit });

        // Suggestion energy direction (per-deck routed, controls global slider)
        actions.insert("deck.suggestion_energy".to_string(), ActionInfo { deck_targetable: true, value_range: ControlRange::Unit });

        Self { actions }
    }

    /// Get info for an action
    pub fn get(&self, action: &str) -> Option<&ActionInfo> {
        self.actions.get(action)
    }
}

/// Debounce state for actions that shouldn't fire repeatedly
struct DebounceState {
    /// Last time browser.select was triggered
    last_browser_select: Option<std::time::Instant>,
    /// Last time deck.load_selected was triggered per deck
    last_deck_load: [Option<std::time::Instant>; 4],
}

/// Mapping engine - converts control events to app messages
///
/// Accepts protocol-agnostic `ControlEvent`s and maps them to `MidiMessage`s
/// based on the device configuration. Works for both MIDI and HID input.
///
/// Supports multiple mappings per control address for mode-conditional routing:
/// when momentary mode buttons are used, a pad can have both a default action
/// (mode: None) and overlay actions (mode: "hot_cue", "slicer") that activate
/// only while the mode button is held.
pub struct MappingEngine {
    /// Unified address-to-mappings lookup (multiple mappings per address for mode overlays)
    address_mappings: HashMap<ControlAddress, Vec<ControlMapping>>,
    /// Shared state for shift/layer resolution (shared with input callback)
    shared_state: Arc<SharedState>,
    /// Action registry for value ranges
    _action_registry: ActionRegistry,
    /// Debounce state with interior mutability for use from Arc<Self>
    debounce: std::sync::Mutex<DebounceState>,
    /// Continuous-as-button state for edge detection
    /// Used when continuous hardware (knob/fader) is mapped to momentary action (button)
    button_edge_state: std::sync::Mutex<HashMap<ControlAddress, bool>>,
    /// Per-deck overlay mode state (None = performance mode, Some("hot_cue"/"slicer") = overlay active)
    mode_held: std::sync::Mutex<[Option<String>; 4]>,
    /// Whether mode buttons use momentary behavior (hold-to-activate overlay)
    momentary_mode_buttons: bool,
}

impl MappingEngine {
    /// Create a new mapping engine from device profile with shared state
    pub fn new(profile: &DeviceProfile, shared_state: Arc<SharedState>) -> Self {
        let mut address_mappings: HashMap<ControlAddress, Vec<ControlMapping>> = HashMap::new();

        for mapping in &profile.mappings {
            address_mappings
                .entry(mapping.control.clone())
                .or_default()
                .push(mapping.clone());
        }

        log::info!(
            "Mapping: Loaded {} control addresses ({} total mappings)",
            address_mappings.len(),
            profile.mappings.len(),
        );

        Self {
            address_mappings,
            shared_state,
            _action_registry: ActionRegistry::new(),
            debounce: std::sync::Mutex::new(DebounceState {
                last_browser_select: None,
                last_deck_load: [None; 4],
            }),
            button_edge_state: std::sync::Mutex::new(HashMap::new()),
            mode_held: std::sync::Mutex::new([None, None, None, None]),
            momentary_mode_buttons: profile.momentary_mode_buttons,
        }
    }

    /// Map a control event to one or more app messages
    ///
    /// Returns multiple messages when a per-side mode button targets multiple decks.
    /// This is the preferred entry point for the drain loop.
    pub fn map_event_multi(&self, event: &ControlEvent) -> Vec<MidiMessage> {
        let mappings = match self.address_mappings.get(&event.address) {
            Some(m) => m,
            None => {
                log::debug!("[Mapping] No mapping for {:?}", event.address);
                return Vec::new();
            }
        };

        // Select the best mapping based on current mode state
        let mapping = self.select_mapping(mappings);

        // Determine effective shift: per-deck if mapping has physical_deck, else global
        let shift_held = if let Some(pd) = mapping.physical_deck {
            self.shared_state.is_shift_held_for_deck(pd)
        } else {
            self.shared_state.is_shift_held_global()
        };

        // Check if this is continuous-as-button (continuous hardware mapped to momentary action)
        if matches!(event.value, ControlValue::Absolute(_)) && self.needs_edge_detection(mapping) {
            return self.handle_continuous_as_button(event, mapping, shift_held).into_iter().collect();
        }

        // Determine which action to use (shift or normal)
        let action = if shift_held {
            mapping.shift_action.as_ref().unwrap_or(&mapping.action)
        } else {
            &mapping.action
        };

        // Check for mode button actions that target multiple decks
        if self.momentary_mode_buttons && (action == "deck.hot_cue_mode" || action == "deck.slicer_mode") {
            return self.handle_mode_action_multi(action, event, mapping);
        }

        // Resolve deck index
        let deck = self.resolve_deck(mapping);

        // Convert to MidiMessage based on action
        self.action_to_message(action, event, mapping, deck).into_iter().collect()
    }

    /// Map a control event to an app message (single-message convenience)
    ///
    /// This is the primary entry point for all protocols (MIDI, HID, etc.)
    /// Shift state is read from the shared state based on the mapping's physical_deck.
    pub fn map_event(&self, event: &ControlEvent) -> Option<MidiMessage> {
        let mappings = match self.address_mappings.get(&event.address) {
            Some(m) => m,
            None => {
                log::debug!("[Mapping] No mapping for {:?}", event.address);
                return None;
            }
        };

        // Select the best mapping based on current mode state
        let mapping = self.select_mapping(mappings);

        // Determine effective shift: per-deck if mapping has physical_deck, else global
        let shift_held = if let Some(pd) = mapping.physical_deck {
            self.shared_state.is_shift_held_for_deck(pd)
        } else {
            self.shared_state.is_shift_held_global()
        };

        // Check if this is continuous-as-button (continuous hardware mapped to momentary action)
        // Use edge detection to prevent repeated triggers
        if matches!(event.value, ControlValue::Absolute(_)) && self.needs_edge_detection(mapping) {
            return self.handle_continuous_as_button(event, mapping, shift_held);
        }

        // Determine which action to use (shift or normal)
        let action = if shift_held {
            mapping.shift_action.as_ref().unwrap_or(&mapping.action)
        } else {
            &mapping.action
        };

        // Handle momentary mode buttons
        if self.momentary_mode_buttons && (action == "deck.hot_cue_mode" || action == "deck.slicer_mode") {
            let msgs = self.handle_mode_action_multi(action, event, mapping);
            return msgs.into_iter().next();
        }

        // Resolve deck index
        let deck = self.resolve_deck(mapping);

        // Convert to MidiMessage based on action
        self.action_to_message(action, event, mapping, deck)
    }

    /// Select the best mapping from a list based on current mode state
    ///
    /// Priority: mode-conditional mapping matching current held mode > unconditional (mode: None)
    fn select_mapping<'a>(&self, mappings: &'a [ControlMapping]) -> &'a ControlMapping {
        if mappings.len() == 1 {
            return &mappings[0];
        }

        // Determine deck for mode check (use first mapping's deck)
        let deck = self.resolve_deck(&mappings[0]);
        let current_mode = self.get_mode_held(deck);

        // Find a mode-conditional mapping that matches current held mode
        if let Some(ref mode) = current_mode {
            if let Some(m) = mappings.iter().find(|m| m.mode.as_ref() == Some(mode)) {
                return m;
            }
        }

        // Fall back to unconditional mapping (mode: None)
        mappings.iter().find(|m| m.mode.is_none()).unwrap_or(&mappings[0])
    }

    /// Handle a mode button action, potentially targeting multiple decks (per-side mode buttons)
    fn handle_mode_action_multi(
        &self,
        action: &str,
        event: &ControlEvent,
        mapping: &ControlMapping,
    ) -> Vec<MidiMessage> {
        let enabled = event.value.is_press();
        let mode_name = if action == "deck.hot_cue_mode" { "hot_cue" } else { "slicer" };

        // Get target decks from params (per-side mode buttons) or single deck
        let target_decks = self.get_deck_list_param(mapping);

        // Update mode held state
        let mode_val = if enabled { Some(mode_name) } else { None };
        self.set_mode_held_multi(&target_decks, mode_val);

        // Generate messages for each target deck
        target_decks.iter().map(|&deck| {
            let deck_action = if action == "deck.hot_cue_mode" {
                DeckAction::SetHotCueMode { enabled }
            } else {
                DeckAction::SetSlicerMode { enabled }
            };
            MidiMessage::Deck { deck, action: deck_action }
        }).collect()
    }

    /// Get target deck list from mapping params, or resolve single deck
    fn get_deck_list_param(&self, mapping: &ControlMapping) -> Vec<usize> {
        if let Some(decks_val) = mapping.params.get("decks") {
            if let Some(seq) = decks_val.as_sequence() {
                return seq.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as usize))
                    .collect();
            }
        }
        vec![self.resolve_deck(mapping)]
    }

    /// Set mode for multiple decks
    fn set_mode_held_multi(&self, decks: &[usize], mode: Option<&str>) {
        let mut state = self.mode_held.lock().unwrap();
        for &d in decks {
            if d < 4 {
                state[d] = mode.map(|s| s.to_string());
            }
        }
    }

    /// Get current mode held for a deck
    fn get_mode_held(&self, deck: usize) -> Option<String> {
        if deck < 4 {
            self.mode_held.lock().unwrap()[deck].clone()
        } else {
            None
        }
    }

    /// Check if this mapping needs continuous-as-button edge detection
    fn needs_edge_detection(&self, mapping: &ControlMapping) -> bool {
        if mapping.behavior != ControlBehavior::Momentary {
            return false;
        }
        match mapping.hardware_type {
            Some(hw_type) => hw_type.is_continuous(),
            None => false,
        }
    }

    /// Handle continuous event mapped to button action with edge detection
    fn handle_continuous_as_button(
        &self,
        event: &ControlEvent,
        mapping: &ControlMapping,
        shift_held: bool,
    ) -> Option<MidiMessage> {
        let is_pressed = event.value.as_absolute() > 0.5;

        // Get current state
        let was_pressed = {
            let state = self.button_edge_state.lock().unwrap();
            state.get(&event.address).copied().unwrap_or(false)
        };

        // Check for edge transition
        let is_press_edge = if is_pressed && !was_pressed {
            true
        } else if !is_pressed && was_pressed {
            false
        } else {
            return None; // No edge - ignore
        };

        // Update state
        {
            let mut state = self.button_edge_state.lock().unwrap();
            state.insert(event.address.clone(), is_pressed);
        }

        let action = if shift_held {
            mapping.shift_action.as_ref().unwrap_or(&mapping.action)
        } else {
            &mapping.action
        };

        let deck = self.resolve_deck(mapping);

        // Create synthetic button event
        let synthetic = ControlEvent {
            address: event.address.clone(),
            value: ControlValue::Button(is_press_edge),
        };

        log::debug!(
            "[Mapping] Continuous-as-button edge: {:?} -> {}",
            event.address,
            if is_press_edge { "PRESS" } else { "RELEASE" }
        );

        self.action_to_message(action, &synthetic, mapping, deck)
    }

    /// Resolve physical deck to virtual deck
    fn resolve_deck(&self, mapping: &ControlMapping) -> usize {
        if let Some(deck_index) = mapping.deck_index {
            deck_index
        } else if let Some(physical_deck) = mapping.physical_deck {
            self.shared_state.resolve_deck(physical_deck)
        } else {
            0
        }
    }

    /// Convert action string + control event to MidiMessage
    fn action_to_message(
        &self,
        action: &str,
        event: &ControlEvent,
        mapping: &ControlMapping,
        deck: usize,
    ) -> Option<MidiMessage> {
        let get_param = |key: &str| -> Option<usize> {
            mapping
                .params
                .get(key)
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        };

        match action {
            // Transport
            "deck.play" => {
                if event.value.is_press() { Some(MidiMessage::deck_play(deck)) } else { None }
            }
            "deck.cue_press" => {
                if event.value.is_press() {
                    Some(MidiMessage::deck_cue_press(deck))
                } else {
                    Some(MidiMessage::deck_cue_release(deck))
                }
            }
            "deck.sync" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::Sync }) } else { None }
            }

            // Hot Cues
            "deck.hot_cue_press" | "deck.pad_press" => {
                let slot = get_param("slot").or_else(|| get_param("pad")).unwrap_or(0);
                if event.value.is_press() {
                    Some(MidiMessage::hot_cue_press(deck, slot))
                } else {
                    Some(MidiMessage::hot_cue_release(deck, slot))
                }
            }
            "deck.hot_cue_clear" => {
                let slot = get_param("slot").or_else(|| get_param("pad")).unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::HotCueClear { slot } }) } else { None }
            }

            // Loop
            "deck.toggle_loop" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::ToggleLoop }) } else { None }
            }
            "deck.loop_halve" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::LoopHalve }) } else { None }
            }
            "deck.loop_double" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::LoopDouble }) } else { None }
            }
            "deck.loop_size" => {
                // Encoder: use delta from ControlValue or fall back to MIDI interpretation
                let delta = match &event.value {
                    ControlValue::Relative(d) => *d,
                    ControlValue::Absolute(v) => {
                        // MIDI CC as encoder: convert raw value to delta
                        let mode = mapping.encoder_mode.unwrap_or(EncoderMode::Relative);
                        encoder_to_delta((*v * 127.0).round() as u8, mode)
                    }
                    _ => 0,
                };
                if delta != 0 {
                    Some(MidiMessage::Deck { deck, action: DeckAction::LoopSize(delta) })
                } else {
                    None
                }
            }
            "deck.loop_in" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::LoopIn }) } else { None }
            }
            "deck.loop_out" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::LoopOut }) } else { None }
            }

            // Beat Jump
            "deck.beat_jump_forward" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::BeatJumpForward }) } else { None }
            }
            "deck.beat_jump_backward" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::BeatJumpBackward }) } else { None }
            }

            // Slicer
            "deck.slicer_trigger" => {
                let pad = get_param("pad").unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SlicerTrigger { pad } }) } else { None }
            }
            "deck.slicer_assign" => {
                let pad = get_param("pad").unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SlicerAssign { pad } }) } else { None }
            }
            "deck.slicer_mode" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SetSlicerMode { enabled: true } }) } else { None }
            }
            "deck.hot_cue_mode" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SetHotCueMode { enabled: true } }) } else { None }
            }
            "deck.slicer_reset" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SlicerReset }) } else { None }
            }

            // Stem control
            "deck.stem_mute" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::ToggleStemMute { stem } }) } else { None }
            }
            "deck.stem_solo" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::ToggleStemSolo { stem } }) } else { None }
            }
            "deck.stem_select" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::SelectStem { stem } }) } else { None }
            }

            // Misc deck
            "deck.slip" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::ToggleSlip }) } else { None }
            }
            "deck.key_match" => {
                if event.value.is_press() { Some(MidiMessage::Deck { deck, action: DeckAction::ToggleKeyMatch }) } else { None }
            }
            "deck.load_selected" => {
                if event.value.is_press() {
                    let now = std::time::Instant::now();
                    if deck < 4 {
                        let mut debounce = self.debounce.lock().unwrap();
                        if let Some(last) = debounce.last_deck_load[deck] {
                            if now.duration_since(last) < std::time::Duration::from_millis(300) {
                                log::debug!("Mapping: deck.load_selected debounced for deck {}", deck);
                                return None;
                            }
                        }
                        debounce.last_deck_load[deck] = Some(now);
                    }
                    Some(MidiMessage::Deck { deck, action: DeckAction::LoadSelected })
                } else {
                    None
                }
            }
            "deck.browse_back" => {
                if event.value.is_press() {
                    Some(MidiMessage::Deck { deck, action: DeckAction::BrowseBack })
                } else {
                    None
                }
            }

            // Mixer - continuous controls
            "mixer.volume" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::mixer_volume(deck, v))
            }
            "mixer.filter" => {
                let normalized = self.extract_continuous_value(event, action, mapping, Some(3));
                normalized.map(|v| MidiMessage::mixer_filter(deck, v))
            }
            "mixer.eq_hi" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Mixer { channel: deck, action: MixerAction::SetEqHi(v) })
            }
            "mixer.eq_mid" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Mixer { channel: deck, action: MixerAction::SetEqMid(v) })
            }
            "mixer.eq_lo" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Mixer { channel: deck, action: MixerAction::SetEqLo(v) })
            }
            "mixer.cue" => {
                if event.value.is_press() { Some(MidiMessage::Mixer { channel: deck, action: MixerAction::ToggleCue }) } else { None }
            }
            "mixer.crossfader" => {
                let normalized = self.extract_continuous_value(event, "mixer.crossfader", mapping, None);
                normalized.map(|v| MidiMessage::Mixer { channel: 0, action: MixerAction::SetCrossfader(v) })
            }

            // Browser
            "browser.scroll" => {
                let delta = self.extract_encoder_delta(event, mapping);
                if delta != 0 { Some(MidiMessage::browser_scroll(delta)) } else { None }
            }
            "browser.select" => {
                if event.value.is_press() {
                    let now = std::time::Instant::now();
                    let mut debounce = self.debounce.lock().unwrap();
                    if let Some(last) = debounce.last_browser_select {
                        if now.duration_since(last) < std::time::Duration::from_millis(300) {
                            log::debug!("Mapping: browser.select debounced");
                            return None;
                        }
                    }
                    debounce.last_browser_select = Some(now);
                    Some(MidiMessage::Browser(BrowserAction::Select))
                } else {
                    None
                }
            }
            "browser.back" => {
                if event.value.is_press() { Some(MidiMessage::Browser(BrowserAction::Back)) } else { None }
            }

            // FX macro knobs (per-deck continuous)
            "deck.fx_macro" => {
                let macro_idx = mapping.params.get("macro").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Deck {
                    deck,
                    action: DeckAction::SetFxMacro { macro_index: macro_idx, value: v },
                })
            }

            // Suggestion energy direction (per-deck routed, controls global slider)
            "deck.suggestion_energy" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Deck {
                    deck,
                    action: DeckAction::SetSuggestionEnergy(v),
                })
            }

            // Global actions
            "global.master_volume" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Global(GlobalAction::SetMasterVolume(v)))
            }
            "global.cue_volume" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Global(GlobalAction::SetCueVolume(v)))
            }
            "mixer.cue_mix" => {
                let normalized = self.extract_continuous_value(event, action, mapping, None);
                normalized.map(|v| MidiMessage::Global(GlobalAction::SetCueMix(v)))
            }

            // FX preset browsing
            "global.fx_scroll" => {
                let delta = self.extract_encoder_delta(event, mapping);
                if delta != 0 { Some(MidiMessage::Global(GlobalAction::FxScroll(delta))) } else { None }
            }
            "global.fx_select" => {
                if event.value.is_press() { Some(MidiMessage::Global(GlobalAction::FxSelect)) } else { None }
            }

            _ => {
                log::debug!("Mapping: Unknown action '{}'", action);
                None
            }
        }
    }

    /// Extract a normalized continuous value from a control event
    ///
    /// For MIDI (Absolute with 0-127 scale): applies range normalization + deadzone
    /// For HID (Absolute with 0.0-1.0 scale): maps to range directly
    /// For buttons/relative: returns None (not a continuous event)
    fn extract_continuous_value(
        &self,
        event: &ControlEvent,
        action: &str,
        _mapping: &ControlMapping,
        center_deadzone: Option<u8>,
    ) -> Option<f32> {
        match &event.value {
            ControlValue::Absolute(v) => {
                // Convert from 0.0-1.0 to MIDI scale, apply range normalization
                let midi_value = (*v * 127.0).round() as u8;
                let range = range_for_action(action);
                Some(normalize_cc_value(midi_value, range, center_deadzone))
            }
            _ => None,
        }
    }

    /// Extract encoder delta from a control event
    fn extract_encoder_delta(&self, event: &ControlEvent, mapping: &ControlMapping) -> i32 {
        match &event.value {
            ControlValue::Relative(d) => *d,
            ControlValue::Absolute(v) => {
                // MIDI CC as encoder: convert raw value to delta
                let mode = mapping.encoder_mode.unwrap_or(EncoderMode::Relative);
                encoder_to_delta((*v * 127.0).round() as u8, mode)
            }
            _ => 0,
        }
    }

    /// Get current deck for a physical deck
    pub fn resolve_deck_for_physical(&self, physical_deck: usize) -> usize {
        self.shared_state.resolve_deck(physical_deck)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_registry() {
        let registry = ActionRegistry::new();

        let volume = registry.get("mixer.volume").unwrap();
        assert!(!volume.deck_targetable);

        let play = registry.get("deck.play").unwrap();
        assert!(play.deck_targetable);

        let filter = registry.get("mixer.filter").unwrap();
        assert_eq!(filter.value_range, ControlRange::Bipolar);
    }
}
