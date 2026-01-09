//! MIDI output for LED feedback
//!
//! Sends MIDI messages to update controller LEDs based on application state.

use crate::config::{DeviceProfile, FeedbackMapping, MidiControlConfig};
use crate::deck_target::{DeckTargetState, LayerSelection};
use midir::MidiOutputConnection;
use std::collections::HashMap;

/// Application state for LED feedback
///
/// This struct is populated by the app and passed to update() to update LEDs.
#[derive(Debug, Clone, Default)]
pub struct FeedbackState {
    /// Per-deck state
    pub decks: [DeckFeedbackState; 4],
    /// Per-channel mixer state
    pub mixer: [MixerFeedbackState; 4],
}

/// Per-deck feedback state
#[derive(Debug, Clone, Default)]
pub struct DeckFeedbackState {
    /// Is the deck currently playing?
    pub is_playing: bool,
    /// Is the deck currently cueing?
    pub is_cueing: bool,
    /// Which hot cues are set? (bitmap, bit N = cue N is set)
    pub hot_cues_set: u8,
    /// Is loop active?
    pub loop_active: bool,
    /// Is slip mode active?
    pub slip_active: bool,
    /// Is slicer mode active?
    pub slicer_active: bool,
    /// Current slicer slice (0-15)
    pub slicer_current_slice: u8,
    /// Is key match enabled?
    pub key_match_enabled: bool,
}

/// Per-channel mixer feedback state
#[derive(Debug, Clone, Default)]
pub struct MixerFeedbackState {
    /// Is headphone cue (PFL) enabled?
    pub cue_enabled: bool,
}

/// MIDI output handler for LED feedback
pub struct MidiOutputHandler {
    /// MIDI output connection
    connection: MidiOutputConnection,
    /// Feedback mappings from config
    feedback_mappings: Vec<FeedbackMapping>,
    /// Last sent values (to avoid redundant sends)
    last_values: HashMap<MidiOutputKey, u8>,
}

/// Key for tracking last sent values
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MidiOutputKey {
    channel: u8,
    note_or_cc: u8,
    is_note: bool,
}

impl MidiOutputKey {
    fn from_control(control: &MidiControlConfig) -> Self {
        match control {
            MidiControlConfig::Note { channel, note } => Self {
                channel: *channel,
                note_or_cc: *note,
                is_note: true,
            },
            MidiControlConfig::ControlChange { channel, cc } => Self {
                channel: *channel,
                note_or_cc: *cc,
                is_note: false,
            },
        }
    }
}

impl MidiOutputHandler {
    /// Create a new output handler
    pub fn new(connection: MidiOutputConnection, profile: &DeviceProfile) -> Self {
        Self {
            connection,
            feedback_mappings: profile.feedback.clone(),
            last_values: HashMap::new(),
        }
    }

    /// Update LEDs based on current application state
    ///
    /// Only sends MIDI messages when values have changed to avoid flooding.
    pub fn update(&mut self, state: &FeedbackState, deck_target: &DeckTargetState) {
        // Collect updates first to avoid borrow checker issues
        let updates: Vec<_> = self
            .feedback_mappings
            .iter()
            .map(|mapping| {
                let value = Self::evaluate_state_static(mapping, state, deck_target);
                let midi_value = if value {
                    mapping.on_value
                } else {
                    mapping.off_value
                };
                (mapping.output.clone(), midi_value)
            })
            .collect();

        // Now send all updates
        for (output, midi_value) in updates {
            self.send_if_changed(&output, midi_value);
        }
    }

    /// Evaluate a state condition (static method to avoid borrow issues)
    fn evaluate_state_static(
        mapping: &FeedbackMapping,
        state: &FeedbackState,
        deck_target: &DeckTargetState,
    ) -> bool {
        // Determine which deck to check
        let deck_idx = if let Some(physical_deck) = mapping.physical_deck {
            // Layer-resolved: check the deck currently targeted by this physical deck
            deck_target.resolve_deck(physical_deck)
        } else {
            // Direct deck mapping
            mapping.deck_index.unwrap_or(0)
        };

        let deck_idx = deck_idx.min(3);
        let deck_state = &state.decks[deck_idx];

        // Check for layer indicator (special case)
        if mapping.state == "deck.layer_active" {
            if let Some(ref layer_str) = mapping.layer {
                let physical_deck = mapping.physical_deck.unwrap_or(0);
                let current_layer = deck_target.get_layer(physical_deck);
                let target_layer = match layer_str.to_uppercase().as_str() {
                    "A" => LayerSelection::A,
                    "B" => LayerSelection::B,
                    _ => LayerSelection::A,
                };
                return current_layer == target_layer;
            }
        }

        // Evaluate state based on state string
        match mapping.state.as_str() {
            "deck.is_playing" => deck_state.is_playing,
            "deck.is_cueing" => deck_state.is_cueing,
            "deck.loop_active" => deck_state.loop_active,
            "deck.slip_active" => deck_state.slip_active,
            "deck.slicer_active" => deck_state.slicer_active,
            "deck.key_match_enabled" => deck_state.key_match_enabled,

            "deck.hot_cue_set" => {
                // Check if specific hot cue is set
                let slot = mapping
                    .params
                    .get("slot")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u8;
                (deck_state.hot_cues_set & (1 << slot)) != 0
            }

            "deck.slicer_slice_active" => {
                // Check if specific slice is currently playing
                let slice = mapping
                    .params
                    .get("slice")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u8;
                deck_state.slicer_active && deck_state.slicer_current_slice == slice
            }

            "mixer.cue_enabled" => {
                let channel = mapping.deck_index.unwrap_or(0).min(3);
                state.mixer[channel].cue_enabled
            }

            _ => {
                log::debug!("MIDI output: Unknown state '{}'", mapping.state);
                false
            }
        }
    }

    /// Send MIDI message if value has changed
    fn send_if_changed(&mut self, control: &MidiControlConfig, value: u8) {
        let key = MidiOutputKey::from_control(control);

        // Check if value changed
        if self.last_values.get(&key) == Some(&value) {
            return; // No change, skip
        }

        // Build and send MIDI message
        let message = match control {
            MidiControlConfig::Note { channel, note } => {
                if value > 0 {
                    // Note On
                    vec![0x90 | channel, *note, value]
                } else {
                    // Note Off
                    vec![0x80 | channel, *note, 0]
                }
            }
            MidiControlConfig::ControlChange { channel, cc } => {
                vec![0xB0 | channel, *cc, value]
            }
        };

        if let Err(e) = self.connection.send(&message) {
            log::warn!("MIDI output: Failed to send message: {}", e);
            return;
        }

        // Update last value
        self.last_values.insert(key, value);
    }

    /// Force send a MIDI message (bypass change detection)
    pub fn send(&mut self, control: &MidiControlConfig, value: u8) {
        let message = match control {
            MidiControlConfig::Note { channel, note } => {
                if value > 0 {
                    vec![0x90 | channel, *note, value]
                } else {
                    vec![0x80 | channel, *note, 0]
                }
            }
            MidiControlConfig::ControlChange { channel, cc } => {
                vec![0xB0 | channel, *cc, value]
            }
        };

        if let Err(e) = self.connection.send(&message) {
            log::warn!("MIDI output: Failed to send message: {}", e);
        }

        let key = MidiOutputKey::from_control(control);
        self.last_values.insert(key, value);
    }

    /// Clear all LEDs (send off value for all tracked controls)
    pub fn clear_all(&mut self) {
        let keys: Vec<_> = self.last_values.keys().copied().collect();
        for key in keys {
            let control = if key.is_note {
                MidiControlConfig::Note {
                    channel: key.channel,
                    note: key.note_or_cc,
                }
            } else {
                MidiControlConfig::ControlChange {
                    channel: key.channel,
                    cc: key.note_or_cc,
                }
            };
            self.send(&control, 0);
        }
        self.last_values.clear();
    }
}

impl Drop for MidiOutputHandler {
    fn drop(&mut self) {
        // Clear all LEDs on disconnect
        self.clear_all();
    }
}
