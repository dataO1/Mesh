//! Protocol-agnostic feedback evaluation
//!
//! Evaluates application state against feedback mappings to determine what
//! each feedback-capable control should display. The results are then
//! translated to protocol-specific output by MIDI or HID output handlers.

use crate::config::FeedbackMapping;
use crate::deck_target::{DeckTargetState, LayerSelection};
use crate::types::ControlAddress;
use std::collections::HashMap;

/// Compute a smooth pulse brightness for HID RGB LEDs (0.15-1.0) from beat phase.
///
/// Never fully off — keeps a dim glow at the trough for a breathing effect.
fn pulse_brightness(beat_phase: f32) -> f32 {
    let phase = (beat_phase * std::f32::consts::TAU).cos();
    0.575 + 0.425 * phase
}

/// Compute a pulse factor for MIDI velocity (0.0-1.0) from beat phase.
///
/// Reaches 0.0 at mid-beat so binary on/off LEDs visibly blink.
fn pulse_value(beat_phase: f32) -> f32 {
    let phase = (beat_phase * std::f32::consts::TAU).cos();
    0.5 + 0.5 * phase
}

/// Interpolate a single color channel between `off` and `on` by factor `t`.
fn lerp_u8(off: u8, on: u8, t: f32) -> u8 {
    (off as f32 + (on as f32 - off as f32) * t) as u8
}

/// Compute a beat-pulsed FeedbackResult.
///
/// Uses two separate curves: `pulse_brightness` for the RGB color (smooth HID glow)
/// and `pulse_value` for the MIDI velocity (reaches 0 so binary LEDs blink).
fn beat_pulse_result(
    address: ControlAddress,
    beat_phase: f32,
    on_color: [u8; 3],
    off_color: [u8; 3],
    on_value: u8,
    off_value: u8,
) -> FeedbackResult {
    let tc = pulse_brightness(beat_phase);
    let tv = pulse_value(beat_phase);
    FeedbackResult {
        address,
        value: lerp_u8(off_value, on_value, tv),
        color: Some([
            lerp_u8(off_color[0], on_color[0], tc),
            lerp_u8(off_color[1], on_color[1], tc),
            lerp_u8(off_color[2], on_color[2], tc),
        ]),
    }
}

/// Per-stem LED colors for mute button feedback.
///
/// On HID devices (e.g. Kontrol F1), these RGB values are used directly.
/// On note-offset MIDI devices (e.g. Xone K3), they map to one of 3 layers:
/// - Green-dominant → green layer (user configures matching color in Xone Editor)
/// - Red-dominant → red layer
/// - Neutral/blue → amber layer
///
/// With 4 stems and 3 layers, Drums and Other share the amber layer on the Xone K.
/// On HID devices all 4 get distinct RGB colors.
const STEM_LED_COLORS: [[u8; 3]; 4] = [
    [20, 230, 60],   // Vocals — vivid green (→ green layer on Xone K)
    [40, 110, 240],  // Drums — vivid blue (→ amber layer on Xone K)
    [240, 120, 20],  // Bass — vivid orange (→ red layer on Xone K)
    [180, 50, 255],  // Other — vivid purple (→ amber layer on Xone K)
];

/// Hardcoded transport & mode LED colors (survive remapping).
/// Each pair is (bright, dim) — dim is shown when the state is inactive.
const PLAY_COLOR: [u8; 3] = [0, 200, 0];       // Green
const PLAY_COLOR_DIM: [u8; 3] = [0, 30, 0];    // Dim green
const CUE_COLOR: [u8; 3] = [220, 120, 0];      // Orange
const CUE_COLOR_DIM: [u8; 3] = [35, 18, 0];    // Dim orange
const LOOP_COLOR: [u8; 3] = [0, 180, 200];      // Cyan (active)
const LOOP_COLOR_DIM: [u8; 3] = [0, 22, 25];    // Dim cyan (inactive)
const HOT_CUE_COLOR: [u8; 3] = [200, 140, 0];   // Amber (cue set)
const HOT_CUE_COLOR_DIM: [u8; 3] = [12, 12, 12]; // Near-off (no cue)
const SLICER_COLOR: [u8; 3] = [0, 180, 200];     // Cyan (assigned preset)
const SLICER_COLOR_DIM: [u8; 3] = [0, 12, 14];   // Dim cyan (empty slot)
const BROWSE_COLOR: [u8; 3] = [200, 200, 200];    // White (browse mode active)
const BROWSE_COLOR_DIM: [u8; 3] = [20, 20, 20];   // Dim white (browse mode inactive)

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
    /// Per-side browse mode active state (0 = left, 1 = right)
    pub browse_active: [bool; 2],
}

/// Action button mode (what the pad grid currently controls)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActionMode {
    /// Performance mode — pads do their primary action (default, no mode button held)
    #[default]
    Performance,
    /// Hot cue mode — pads trigger/set hot cues
    HotCue,
    /// Slicer mode — pads queue slices for playback
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
    /// Current loop length in beats (for 7-segment display)
    pub loop_length_beats: f32,
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

/// Check if a feedback mapping's mode condition matches the deck's current action mode
fn mode_matches(mapping: &FeedbackMapping, state: &FeedbackState, deck_target: &DeckTargetState) -> bool {
    match mapping.mode.as_deref() {
        None => true, // Unconditional — always active
        Some("performance") => {
            let deck_idx = resolve_feedback_deck(mapping, deck_target);
            state.decks[deck_idx].action_mode == ActionMode::Performance
        }
        Some("hot_cue") => {
            let deck_idx = resolve_feedback_deck(mapping, deck_target);
            state.decks[deck_idx].action_mode == ActionMode::HotCue
        }
        Some("slicer") => {
            let deck_idx = resolve_feedback_deck(mapping, deck_target);
            state.decks[deck_idx].action_mode == ActionMode::Slicer
        }
        Some(_) => false, // Unknown mode — treat as inactive
    }
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
        .filter_map(|mapping| {
            let address = mapping.output.clone();

            // Mode-conditional feedback: skip mappings whose mode doesn't match
            // the deck's current action mode. This prevents mode-gated off results
            // from overwriting unconditional results for the same output address.
            if !mode_matches(mapping, state, deck_target) {
                return None;
            }

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
                return Some(FeedbackResult { address, value, color });
            }

            // Play button: hardcoded green, dim when stopped, pulsing when playing
            if mapping.state == "deck.is_playing" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                return Some(if deck_state.is_playing {
                    beat_pulse_result(
                        address, state.beat_phase,
                        PLAY_COLOR, PLAY_COLOR_DIM,
                        mapping.on_value, mapping.off_value,
                    )
                } else {
                    FeedbackResult { address, value: mapping.off_value, color: Some(PLAY_COLOR_DIM) }
                });
            }

            // Cue button: hardcoded orange, dim when inactive, bright when cueing
            if mapping.state == "deck.is_cueing" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                return Some(if deck_state.is_cueing {
                    FeedbackResult { address, value: mapping.on_value, color: Some(CUE_COLOR) }
                } else {
                    FeedbackResult { address, value: mapping.off_value, color: Some(CUE_COLOR_DIM) }
                });
            }

            // Loop button: hardcoded cyan, dim when inactive, steady bright when active (no pulsing)
            if mapping.state == "deck.loop_encoder" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                return Some(if deck_state.loop_active {
                    FeedbackResult { address, value: mapping.on_value, color: Some(LOOP_COLOR) }
                } else {
                    FeedbackResult { address, value: mapping.off_value, color: Some(LOOP_COLOR_DIM) }
                });
            }

            // Hot cue set: hardcoded amber, dim when no cue, bright when set
            if mapping.state == "deck.hot_cue_set" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                let slot = mapping.params.get("slot")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u8;
                let is_set = (deck_state.hot_cues_set & (1 << slot)) != 0;
                return Some(if is_set {
                    FeedbackResult { address, value: mapping.on_value, color: Some(HOT_CUE_COLOR) }
                } else {
                    FeedbackResult { address, value: mapping.off_value, color: Some(HOT_CUE_COLOR_DIM) }
                });
            }

            // Slicer preset: hardcoded cyan, dim when empty, bright when assigned, pulsing when active
            if mapping.state == "deck.slicer_slice_active" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                let pad = mapping.params.get("pad")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u8;
                let is_assigned = (deck_state.slicer_presets_assigned & (1 << pad)) != 0;
                let is_active = deck_state.slicer_selected_preset == pad;

                return Some(if is_active && is_assigned {
                    // Active preset: pulse between bright and dim cyan
                    beat_pulse_result(
                        address, state.beat_phase,
                        SLICER_COLOR, SLICER_COLOR_DIM,
                        mapping.on_value, mapping.off_value,
                    )
                } else if is_assigned {
                    // Assigned but not active: steady bright
                    FeedbackResult { address, value: mapping.on_value, color: Some(SLICER_COLOR) }
                } else {
                    // Empty slot: dim
                    FeedbackResult { address, value: mapping.off_value, color: Some(SLICER_COLOR_DIM) }
                });
            }

            // Browse mode: per-side state, white when active, dim when inactive
            if mapping.state == "side.browse_mode" {
                let side = mapping.physical_deck.unwrap_or(0).min(1);
                let active = state.browse_active[side];
                return Some(if active {
                    FeedbackResult { address, value: mapping.on_value, color: Some(BROWSE_COLOR) }
                } else {
                    FeedbackResult { address, value: mapping.off_value, color: Some(BROWSE_COLOR_DIM) }
                });
            }

            // Stem mute: per-stem color from STEM_LED_COLORS
            // Active (unmuted) → full vivid stem color, Muted → dim version
            if mapping.state == "deck.stem_muted" {
                let deck_idx = resolve_feedback_deck(mapping, deck_target);
                let deck_state = &state.decks[deck_idx];
                let stem = mapping.params.get("stem")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let is_muted = (deck_state.stems_muted & (1 << stem)) != 0;
                let c = STEM_LED_COLORS.get(stem).copied().unwrap_or([200, 0, 0]);
                let (value, color) = if is_muted {
                    // Muted: dim version of stem color (÷8)
                    let dim = [c[0] / 8, c[1] / 8, c[2] / 8];
                    (mapping.off_value, Some(dim))
                } else {
                    // Active (unmuted): full vivid stem color
                    (mapping.on_value, Some(c))
                };
                return Some(FeedbackResult { address, value, color });
            }

            let active = evaluate_state(mapping, state, deck_target);
            let (value, color) = if active {
                (mapping.on_value, mapping.on_color)
            } else {
                (mapping.off_value, mapping.off_color)
            };
            Some(FeedbackResult { address, value, color })
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
/// Remembers last-sent values and colors per control address to avoid redundant sends.
/// Used by both MIDI and HID output handlers.
pub struct FeedbackChangeTracker {
    last_values: HashMap<ControlAddress, (u8, Option<[u8; 3]>)>,
}

impl FeedbackChangeTracker {
    pub fn new() -> Self {
        Self {
            last_values: HashMap::new(),
        }
    }

    /// Check if value or color has changed and update tracker
    ///
    /// Returns `true` if the state changed (should send), `false` if unchanged.
    pub fn update(&mut self, address: &ControlAddress, value: u8, color: Option<[u8; 3]>) -> bool {
        let new_state = (value, color);
        if self.last_values.get(address) == Some(&new_state) {
            false
        } else {
            self.last_values.insert(address.clone(), new_state);
            true
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

        // First update should always return true (changed)
        assert!(tracker.update(&addr, 127, None));

        // Same value + color should return false (unchanged)
        assert!(!tracker.update(&addr, 127, None));

        // Different value should return true
        assert!(tracker.update(&addr, 0, None));

        // Same value but different color should return true
        assert!(tracker.update(&addr, 0, Some([127, 0, 0])));

        // Same value + same color should return false
        assert!(!tracker.update(&addr, 0, Some([127, 0, 0])));
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
            mode: None,
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
            mode: None,
        }];

        let state = FeedbackState::default(); // is_playing defaults to false
        let deck_target = DeckTargetState::default();
        let results = evaluate_feedback(&mappings, &state, &deck_target);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].value, 0);
    }
}
