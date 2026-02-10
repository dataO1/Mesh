//! Control-to-action mapping engine
//!
//! Maps MIDI events to application actions based on the device profile configuration.

use crate::config::{ControlBehavior, ControlMapping, DeviceProfile, EncoderMode, MidiControlConfig};
use crate::deck_target::DeckTargetState;
use crate::input::MidiInputEvent;
use crate::messages::{BrowserAction, DeckAction, GlobalAction, MidiMessage, MixerAction};
use crate::normalize::{encoder_to_delta, normalize_cc_value, range_for_action, ControlRange};
use std::collections::HashMap;

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
        actions.insert(
            "mixer.volume".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "mixer.filter".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Bipolar,
            },
        );
        for action in ["mixer.eq_hi", "mixer.eq_mid", "mixer.eq_lo"] {
            actions.insert(
                action.to_string(),
                ActionInfo {
                    deck_targetable: false,
                    value_range: ControlRange::Eq,
                },
            );
        }
        actions.insert(
            "mixer.cue".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "mixer.crossfader".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );

        // Browser actions
        actions.insert(
            "browser.scroll".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "browser.select".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "browser.back".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );

        // Global actions
        actions.insert(
            "global.bpm".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Custom {
                    min: 60.0,
                    max: 200.0,
                },
            },
        );
        actions.insert(
            "global.master_volume".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "global.cue_volume".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "mixer.cue_mix".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );

        // FX preset browsing (global)
        actions.insert(
            "global.fx_scroll".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );
        actions.insert(
            "global.fx_select".to_string(),
            ActionInfo {
                deck_targetable: false,
                value_range: ControlRange::Unit,
            },
        );

        // FX macro knobs (per-deck)
        actions.insert(
            "deck.fx_macro".to_string(),
            ActionInfo {
                deck_targetable: true,
                value_range: ControlRange::Unit,
            },
        );

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

/// Mapping engine - converts MIDI events to app messages
pub struct MappingEngine {
    /// Control-to-mapping lookup (key is (channel, note/cc))
    note_mappings: HashMap<(u8, u8), ControlMapping>,
    cc_mappings: HashMap<(u8, u8), ControlMapping>,
    /// Deck target state for resolving physical → virtual deck
    deck_target: DeckTargetState,
    /// Action registry for value ranges
    action_registry: ActionRegistry,
    /// Debounce state with interior mutability for use from Arc<Self>
    debounce: std::sync::Mutex<DebounceState>,
    /// CC-as-button state for edge detection (key is (channel, cc), value is "is_pressed")
    /// Used when continuous hardware (knob) is mapped to momentary action (button)
    cc_button_state: std::sync::Mutex<HashMap<(u8, u8), bool>>,
}

impl MappingEngine {
    /// Create a new mapping engine from device profile
    pub fn new(profile: &DeviceProfile) -> Self {
        let mut note_mappings = HashMap::new();
        let mut cc_mappings = HashMap::new();

        for mapping in &profile.mappings {
            match &mapping.control {
                MidiControlConfig::Note { channel, note } => {
                    note_mappings.insert((*channel, *note), mapping.clone());
                }
                MidiControlConfig::ControlChange { channel, cc } => {
                    cc_mappings.insert((*channel, *cc), mapping.clone());
                }
            }
        }

        let deck_target = DeckTargetState::from_config(&profile.deck_target);

        log::info!(
            "MIDI: Loaded {} note mappings, {} CC mappings",
            note_mappings.len(),
            cc_mappings.len()
        );

        Self {
            note_mappings,
            cc_mappings,
            deck_target,
            action_registry: ActionRegistry::new(),
            debounce: std::sync::Mutex::new(DebounceState {
                last_browser_select: None,
                last_deck_load: [None; 4],
            }),
            cc_button_state: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Map a MIDI event to an app message
    pub fn map_event(&self, event: &MidiInputEvent, shift_held: bool) -> Option<MidiMessage> {
        let mapping = match self.find_mapping(event) {
            Some(m) => m,
            None => {
                // Log unmapped events to help debug missing mappings
                match event {
                    MidiInputEvent::NoteOn { channel, note, .. }
                    | MidiInputEvent::NoteOff { channel, note, .. } => {
                        log::debug!("[MIDI] No mapping for Note ch={} note={}", channel, note);
                    }
                    MidiInputEvent::ControlChange { channel, cc, .. } => {
                        log::debug!("[MIDI] No mapping for CC ch={} cc={}", channel, cc);
                    }
                }
                return None;
            }
        };

        // Check if this is CC-as-button (continuous hardware mapped to momentary action)
        // Use edge detection to prevent repeated triggers
        if let MidiInputEvent::ControlChange { channel, cc, value } = event {
            if self.needs_cc_edge_detection(mapping) {
                return self.handle_cc_as_button(*channel, *cc, *value, mapping, shift_held);
            }
        }

        // Determine which action to use (shift or normal)
        let action = if shift_held {
            mapping.shift_action.as_ref().unwrap_or(&mapping.action)
        } else {
            &mapping.action
        };

        // Resolve deck index
        let deck = self.resolve_deck(mapping);

        // Convert to MidiMessage based on action
        self.action_to_message(action, event, mapping, deck)
    }

    /// Check if this mapping needs CC-as-button edge detection
    fn needs_cc_edge_detection(&self, mapping: &ControlMapping) -> bool {
        // Only apply edge detection for:
        // 1. Momentary behavior (button actions)
        // 2. Continuous hardware type (knob, fader, encoder)
        if mapping.behavior != ControlBehavior::Momentary {
            return false;
        }

        match mapping.hardware_type {
            Some(hw_type) => hw_type.is_continuous(),
            None => false, // No hardware type info - use default behavior
        }
    }

    /// Handle CC event mapped to button action with edge detection
    fn handle_cc_as_button(
        &self,
        channel: u8,
        cc: u8,
        value: u8,
        mapping: &ControlMapping,
        shift_held: bool,
    ) -> Option<MidiMessage> {
        const THRESHOLD: u8 = 63;

        let key = (channel, cc);
        let is_pressed = value > THRESHOLD;

        // Get current state
        let was_pressed = {
            let state = self.cc_button_state.lock().unwrap();
            state.get(&key).copied().unwrap_or(false)
        };

        // Check for edge transition
        let (is_press_edge, _is_release_edge) = if is_pressed && !was_pressed {
            // Rising edge: knob crossed above threshold
            (true, false)
        } else if !is_pressed && was_pressed {
            // Falling edge: knob crossed below threshold
            (false, true)
        } else {
            // No edge - ignore
            return None;
        };

        // Update state
        {
            let mut state = self.cc_button_state.lock().unwrap();
            state.insert(key, is_pressed);
        }

        // Determine which action to use
        let action = if shift_held {
            mapping.shift_action.as_ref().unwrap_or(&mapping.action)
        } else {
            &mapping.action
        };

        // Resolve deck index
        let deck = self.resolve_deck(mapping);

        // Create synthetic Note event for the action handler
        let synthetic_event = if is_press_edge {
            MidiInputEvent::NoteOn {
                channel,
                note: cc, // Use CC number as note
                velocity: 127,
            }
        } else {
            MidiInputEvent::NoteOff {
                channel,
                note: cc,
                velocity: 0,
            }
        };

        log::debug!(
            "[MIDI] CC-as-button edge: ch={} cc={} value={} -> {}",
            channel,
            cc,
            value,
            if is_press_edge { "PRESS" } else { "RELEASE" }
        );

        self.action_to_message(action, &synthetic_event, mapping, deck)
    }

    /// Find the mapping for an event
    fn find_mapping(&self, event: &MidiInputEvent) -> Option<&ControlMapping> {
        match event {
            MidiInputEvent::NoteOn { channel, note, .. }
            | MidiInputEvent::NoteOff { channel, note, .. } => {
                self.note_mappings.get(&(*channel, *note))
            }
            MidiInputEvent::ControlChange { channel, cc, .. } => {
                self.cc_mappings.get(&(*channel, *cc))
            }
        }
    }

    /// Resolve physical deck to virtual deck
    fn resolve_deck(&self, mapping: &ControlMapping) -> usize {
        if let Some(deck_index) = mapping.deck_index {
            // Direct deck mapping (for mixer controls)
            deck_index
        } else if let Some(physical_deck) = mapping.physical_deck {
            // Layer-resolved deck (for transport/pads)
            self.deck_target.resolve_deck(physical_deck)
        } else {
            // Default to deck 0
            0
        }
    }

    /// Convert action string to MidiMessage
    fn action_to_message(
        &self,
        action: &str,
        event: &MidiInputEvent,
        mapping: &ControlMapping,
        deck: usize,
    ) -> Option<MidiMessage> {
        // Get param helper
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
                if event.is_press() {
                    Some(MidiMessage::deck_play(deck))
                } else {
                    None
                }
            }
            "deck.cue_press" => {
                if event.is_press() {
                    Some(MidiMessage::deck_cue_press(deck))
                } else {
                    Some(MidiMessage::deck_cue_release(deck))
                }
            }
            "deck.sync" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::Sync,
                    })
                } else {
                    None
                }
            }

            // Hot Cues
            "deck.hot_cue_press" | "deck.pad_press" => {
                let slot = get_param("slot").or_else(|| get_param("pad")).unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::hot_cue_press(deck, slot))
                } else {
                    Some(MidiMessage::hot_cue_release(deck, slot))
                }
            }
            "deck.hot_cue_clear" => {
                let slot = get_param("slot").or_else(|| get_param("pad")).unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::HotCueClear { slot },
                    })
                } else {
                    None
                }
            }

            // Loop
            "deck.toggle_loop" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::ToggleLoop,
                    })
                } else {
                    None
                }
            }
            "deck.loop_halve" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::LoopHalve,
                    })
                } else {
                    None
                }
            }
            "deck.loop_double" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::LoopDouble,
                    })
                } else {
                    None
                }
            }
            "deck.loop_in" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::LoopIn,
                    })
                } else {
                    None
                }
            }
            "deck.loop_out" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::LoopOut,
                    })
                } else {
                    None
                }
            }

            // Beat Jump
            "deck.beat_jump_forward" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::BeatJumpForward,
                    })
                } else {
                    None
                }
            }
            "deck.beat_jump_backward" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::BeatJumpBackward,
                    })
                } else {
                    None
                }
            }

            // Slicer
            "deck.slicer_trigger" => {
                let pad = get_param("pad").unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SlicerTrigger { pad },
                    })
                } else {
                    None
                }
            }
            "deck.slicer_assign" => {
                let pad = get_param("pad").unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SlicerAssign { pad },
                    })
                } else {
                    None
                }
            }
            "deck.slicer_mode" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SetSlicerMode { enabled: true },
                    })
                } else {
                    None
                }
            }
            "deck.hot_cue_mode" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SetHotCueMode { enabled: true },
                    })
                } else {
                    None
                }
            }
            "deck.slicer_reset" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SlicerReset,
                    })
                } else {
                    None
                }
            }

            // Stem control
            "deck.stem_mute" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::ToggleStemMute { stem },
                    })
                } else {
                    None
                }
            }
            "deck.stem_solo" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::ToggleStemSolo { stem },
                    })
                } else {
                    None
                }
            }
            "deck.stem_select" => {
                let stem = get_param("stem").unwrap_or(0);
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SelectStem { stem },
                    })
                } else {
                    None
                }
            }

            // Misc deck
            "deck.slip" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::ToggleSlip,
                    })
                } else {
                    None
                }
            }
            "deck.key_match" => {
                if event.is_press() {
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::ToggleKeyMatch,
                    })
                } else {
                    None
                }
            }
            "deck.load_selected" => {
                if event.is_press() {
                    // Debounce: require 300ms between load actions per deck
                    let now = std::time::Instant::now();
                    if deck < 4 {
                        let mut debounce = self.debounce.lock().unwrap();
                        if let Some(last) = debounce.last_deck_load[deck] {
                            if now.duration_since(last) < std::time::Duration::from_millis(300) {
                                log::debug!("MIDI: deck.load_selected debounced for deck {}", deck);
                                return None;
                            }
                        }
                        debounce.last_deck_load[deck] = Some(now);
                    }
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::LoadSelected,
                    })
                } else {
                    None
                }
            }

            // Mixer - continuous controls
            "mixer.volume" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let range = range_for_action(action);
                    let normalized = normalize_cc_value(*value, range, None);
                    Some(MidiMessage::mixer_volume(deck, normalized))
                } else {
                    None
                }
            }
            "mixer.filter" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let range = range_for_action(action);
                    // Filter often benefits from center deadzone
                    let normalized = normalize_cc_value(*value, range, Some(3));
                    Some(MidiMessage::mixer_filter(deck, normalized))
                } else {
                    None
                }
            }
            "mixer.eq_hi" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let range = range_for_action(action);
                    let normalized = normalize_cc_value(*value, range, None);
                    Some(MidiMessage::Mixer {
                        channel: deck,
                        action: MixerAction::SetEqHi(normalized),
                    })
                } else {
                    None
                }
            }
            "mixer.eq_mid" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let range = range_for_action(action);
                    let normalized = normalize_cc_value(*value, range, None);
                    Some(MidiMessage::Mixer {
                        channel: deck,
                        action: MixerAction::SetEqMid(normalized),
                    })
                } else {
                    None
                }
            }
            "mixer.eq_lo" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let range = range_for_action(action);
                    let normalized = normalize_cc_value(*value, range, None);
                    Some(MidiMessage::Mixer {
                        channel: deck,
                        action: MixerAction::SetEqLo(normalized),
                    })
                } else {
                    None
                }
            }
            "mixer.cue" => {
                if event.is_press() {
                    Some(MidiMessage::Mixer {
                        channel: deck,
                        action: MixerAction::ToggleCue,
                    })
                } else {
                    None
                }
            }
            "mixer.crossfader" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let normalized = normalize_cc_value(*value, ControlRange::Unit, None);
                    Some(MidiMessage::Mixer {
                        channel: 0, // Crossfader is global
                        action: MixerAction::SetCrossfader(normalized),
                    })
                } else {
                    None
                }
            }

            // Browser
            "browser.scroll" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let mode = mapping.encoder_mode.unwrap_or(EncoderMode::Relative);
                    let delta = encoder_to_delta(*value, mode);
                    if delta != 0 {
                        Some(MidiMessage::browser_scroll(delta))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            "browser.select" => {
                if event.is_press() {
                    // Debounce: require 300ms between select actions to prevent repeated loads
                    let now = std::time::Instant::now();
                    let mut debounce = self.debounce.lock().unwrap();
                    if let Some(last) = debounce.last_browser_select {
                        if now.duration_since(last) < std::time::Duration::from_millis(300) {
                            log::debug!("MIDI: browser.select debounced");
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
                if event.is_press() {
                    Some(MidiMessage::Browser(BrowserAction::Back))
                } else {
                    None
                }
            }

            // FX macro knobs (per-deck continuous)
            "deck.fx_macro" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let macro_idx = mapping
                        .params
                        .get("macro")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    let normalized = normalize_cc_value(*value, ControlRange::Unit, None);
                    Some(MidiMessage::Deck {
                        deck,
                        action: DeckAction::SetFxMacro {
                            macro_index: macro_idx,
                            value: normalized,
                        },
                    })
                } else {
                    None
                }
            }

            // Global actions
            "global.master_volume" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let normalized = normalize_cc_value(*value, ControlRange::Unit, None);
                    Some(MidiMessage::Global(GlobalAction::SetMasterVolume(normalized)))
                } else {
                    None
                }
            }
            "global.cue_volume" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let normalized = normalize_cc_value(*value, ControlRange::Unit, None);
                    Some(MidiMessage::Global(GlobalAction::SetCueVolume(normalized)))
                } else {
                    None
                }
            }
            "mixer.cue_mix" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let normalized = normalize_cc_value(*value, ControlRange::Unit, None);
                    Some(MidiMessage::Global(GlobalAction::SetCueMix(normalized)))
                } else {
                    None
                }
            }

            // FX preset browsing
            "global.fx_scroll" => {
                if let MidiInputEvent::ControlChange { value, .. } = event {
                    let mode = mapping.encoder_mode.unwrap_or(EncoderMode::Relative);
                    let delta = encoder_to_delta(*value, mode);
                    if delta != 0 {
                        Some(MidiMessage::Global(GlobalAction::FxScroll(delta)))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            "global.fx_select" => {
                if event.is_press() {
                    Some(MidiMessage::Global(GlobalAction::FxSelect))
                } else {
                    None
                }
            }

            _ => {
                log::debug!("MIDI: Unknown action '{}'", action);
                None
            }
        }
    }

    /// Update deck target state (for layer toggle)
    pub fn toggle_layer(&mut self, physical_deck: usize) {
        self.deck_target.toggle_layer(physical_deck);
    }

    /// Get current deck for a physical deck
    pub fn resolve_deck_for_physical(&self, physical_deck: usize) -> usize {
        self.deck_target.resolve_deck(physical_deck)
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
