//! Protocol-agnostic feedback evaluation
//!
//! Evaluates application state against feedback mappings to determine what
//! each feedback-capable control should display. The results are then
//! translated to protocol-specific output by MIDI or HID output handlers.

use crate::config::FeedbackMapping;
use crate::deck_target::{DeckTargetState, LayerSelection};
use crate::types::ControlAddress;
use std::collections::HashMap;

/// Compute a smooth pulse brightness (0.0-1.0) from beat phase
///
/// `beat_phase` is 0.0-1.0 within a beat (from beatgrid-aligned playhead).
/// Uses a cosine curve so brightness peaks at beat start and dips mid-beat.
fn pulse_brightness(beat_phase: f32) -> f32 {
    let phase = (beat_phase * std::f32::consts::TAU).cos();
    0.575 + 0.425 * phase
}

/// Application state for LED feedback
///
/// This struct is populated by the app and passed to the feedback evaluator.
#[derive(Debug, Clone, Default)]
pub struct FeedbackState {
    /// Per-deck state
    pub decks: [DeckFeedbackState; 4],
    /// Per-channel mixer state
    pub mixer: [MixerFeedbackState; 4],
    /// Beat phase from master deck (0.0-1.0, beatgrid-aligned, for tempo-synced animations)
    pub beat_phase: f32,
}

/// Action button mode (what the pad grid currently controls)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActionMode {
    #[default]
    HotCue,
    Slicer,
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
    /// Which slicer presets have patterns assigned? (bitmap, bit N = preset N has a pattern)
    pub slicer_presets_assigned: u8,
    /// Currently selected/active slicer preset (0-7)
    pub slicer_selected_preset: u8,
    /// Is key match enabled?
    pub key_match_enabled: bool,
    /// Which stems are muted? (bitmap, bit N = stem N is muted)
    pub stems_muted: u8,
    /// Current action button mode
    pub action_mode: ActionMode,
}

/// Per-channel mixer feedback state
#[derive(Debug, Clone, Default)]
pub struct MixerFeedbackState {
    /// Is headphone cue (PFL) enabled?
    pub cue_enabled: bool,
}

/// Result of evaluating a single feedback mapping
#[derive(Debug, Clone)]
pub struct FeedbackResult {
    /// The control address to send feedback to
    pub address: ControlAddress,
    /// The value to send (0-127, used for MIDI velocity / LED brightness)
    pub value: u8,
    /// RGB color for HID devices with RGB LEDs (overrides value-based brightness)
    pub color: Option<[u8; 3]>,
}

/// Evaluate all feedback mappings against current state
///
/// Returns a list of (address, value) pairs. The output handler filters
/// for its protocol and applies change detection before sending.
pub fn evaluate_feedback(
    mappings: &[FeedbackMapping],
    state: &FeedbackState,
    deck_target: &DeckTargetState,
) -> Vec<FeedbackResult> {
    mappings
        .iter()
        .map(|mapping| {
            let address = mapping.output.clone();

            // Special handling for layer indicator LEDs with alt_on_value
            if mapping.state == "deck.layer_active" {
                let physical_deck = mapping.physical_deck.unwrap_or(0);
                let current_layer = deck_target.get_layer(physical_deck);
                let (value, color) = match current_layer {
                    LayerSelection::A => (mapping.on_value, mapping.on_color),
                    LayerSelection::B => (
                        mapping.alt_on_value.unwrap_or(mapping.on_value),
                        mapping.alt_on_color.or(mapping.on_color),
                    ),
                };
                return FeedbackResult { address, value, color };
            }

            // Slicer preset overlay: when deck is in slicer mode, hot_cue_set
            // pads show slicer preset state instead (assigned/active with pulsing)
            if mapping.state == "deck.hot_cue_set" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                if deck_state.action_mode == ActionMode::Slicer {
                    let slot = mapping.params.get("slot")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8;
                    let is_assigned = (deck_state.slicer_presets_assigned & (1 << slot)) != 0;
                    let is_active = deck_state.slicer_selected_preset == slot;

                    let (value, color) = if is_active && is_assigned {
                        // Active preset: pulse between on and off colors, synced to beatgrid
                        let t = pulse_brightness(state.beat_phase);
                        let on = mapping.on_color.unwrap_or([0, 127, 0]);
                        let off = mapping.off_color.unwrap_or([20, 20, 20]);
                        let r = (off[0] as f32 + (on[0] as f32 - off[0] as f32) * t) as u8;
                        let g = (off[1] as f32 + (on[1] as f32 - off[1] as f32) * t) as u8;
                        let b = (off[2] as f32 + (on[2] as f32 - off[2] as f32) * t) as u8;
                        let v = (mapping.off_value as f32 + (mapping.on_value as f32 - mapping.off_value as f32) * t) as u8;
                        (v, Some([r, g, b]))
                    } else if is_assigned {
                        // Assigned but not active: steady on
                        (mapping.on_value, mapping.on_color)
                    } else {
                        // No preset: off
                        (mapping.off_value, mapping.off_color)
                    };
                    return FeedbackResult { address, value, color };
                }
            }

            let active = evaluate_state(mapping, state, deck_target);
            let (value, color) = if active {
                (mapping.on_value, mapping.on_color)
            } else {
                (mapping.off_value, mapping.off_color)
            };
            FeedbackResult { address, value, color }
        })
        .collect()
}

/// Resolve deck index from a feedback mapping (physical_deck → layer-resolved, or direct deck_index)
fn resolve_feedback_deck(mapping: &FeedbackMapping, deck_target: &DeckTargetState) -> usize {
    let deck_idx = if let Some(physical_deck) = mapping.physical_deck {
        deck_target.resolve_deck(physical_deck)
    } else {
        mapping.deck_index.unwrap_or(0)
    };
    deck_idx.min(3)
}

/// Evaluate a single state condition
fn evaluate_state(
    mapping: &FeedbackMapping,
    state: &FeedbackState,
    deck_target: &DeckTargetState,
) -> bool {
    let deck_idx = resolve_feedback_deck(mapping, deck_target);
    let deck_state = &state.decks[deck_idx];

    match mapping.state.as_str() {
        "deck.is_playing" => deck_state.is_playing,
        "deck.is_cueing" => deck_state.is_cueing,
        "deck.loop_active" => deck_state.loop_active,
        "deck.slip_active" => deck_state.slip_active,
        "deck.slicer_active" => deck_state.slicer_active,
        "deck.key_match_enabled" => deck_state.key_match_enabled,

        "deck.hot_cue_set" => {
            let slot = mapping
                .params
                .get("slot")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8;
            (deck_state.hot_cues_set & (1 << slot)) != 0
        }

        "deck.slicer_slice_active" => {
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

        // Action mode states (for mode indicator LEDs)
        "deck.hot_cue_mode" => deck_state.action_mode == ActionMode::HotCue,
        "deck.slicer_mode" => deck_state.action_mode == ActionMode::Slicer,

        // Stem mute states
        "deck.stem_muted" => {
            let stem = mapping
                .params
                .get("stem")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8;
            (deck_state.stems_muted & (1 << stem)) != 0
        }

        // Layer active is handled above in evaluate_feedback()
        "deck.layer_active" => true,

        _ => {
            log::trace!("Feedback: Unknown state '{}'", mapping.state);
            false
        }
    }
}

/// Change tracker for feedback output
///
/// Remembers last-sent values per control address to avoid redundant sends.
/// Used by both MIDI and HID output handlers.
pub struct FeedbackChangeTracker {
    last_values: HashMap<ControlAddress, u8>,
}

impl FeedbackChangeTracker {
    pub fn new() -> Self {
        Self {
            last_values: HashMap::new(),
        }
    }

    /// Check if value has changed and update tracker
    ///
    /// Returns `Some(value)` if the value changed (should send), `None` if unchanged.
    pub fn update(&mut self, address: &ControlAddress, value: u8) -> Option<u8> {
        if self.last_values.get(address) == Some(&value) {
            None
        } else {
            self.last_values.insert(address.clone(), value);
            Some(value)
        }
    }

    /// Clear all tracked state
    pub fn clear(&mut self) {
        self.last_values.clear();
    }

    /// Get all tracked addresses (for clearing all LEDs on disconnect)
    pub fn tracked_addresses(&self) -> impl Iterator<Item = &ControlAddress> {
        self.last_values.keys()
    }
}

impl Default for FeedbackChangeTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feedback_change_tracker() {
        let mut tracker = FeedbackChangeTracker::new();
        let addr = ControlAddress::Hid { device_id: "test".to_string(), name: "test".to_string() };

        // First update should always return Some
        assert_eq!(tracker.update(&addr, 127), Some(127));

        // Same value should return None
        assert_eq!(tracker.update(&addr, 127), None);

        // Different value should return Some
        assert_eq!(tracker.update(&addr, 0), Some(0));
    }

    #[test]
    fn test_evaluate_feedback_playing() {
        use crate::config::FeedbackMapping;
        use crate::types::MidiAddress;

        let mappings = vec![FeedbackMapping {
            state: "deck.is_playing".to_string(),
            physical_deck: Some(0),
            deck_index: None,
            params: Default::default(),
            output: ControlAddress::Midi(MidiAddress::Note { channel: 0, note: 0x0B }),
            on_value: 127,
            off_value: 0,
            alt_on_value: None,
            on_color: None,
            off_color: None,
            alt_on_color: None,
        }];

        let mut state = FeedbackState::default();
        state.decks[0].is_playing = true;

        let deck_target = DeckTargetState::default();
        let results = evaluate_feedback(&mappings, &state, &deck_target);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 127);
    }

    #[test]
    fn test_evaluate_feedback_not_playing() {
        use crate::config::FeedbackMapping;
        use crate::types::MidiAddress;

        let mappings = vec![FeedbackMapping {
            state: "deck.is_playing".to_string(),
            physical_deck: Some(0),
            deck_index: None,
            params: Default::default(),
            output: ControlAddress::Midi(MidiAddress::Note { channel: 0, note: 0x0B }),
            on_value: 127,
            off_value: 0,
            alt_on_value: None,
            on_color: None,
            off_color: None,
            alt_on_color: None,
        }];

        let state = FeedbackState::default(); // is_playing defaults to false
        let deck_target = DeckTargetState::default();
        let results = evaluate_feedback(&mappings, &state, &deck_target);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 0);
    }
}
