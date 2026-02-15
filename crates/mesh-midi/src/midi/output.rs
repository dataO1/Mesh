//! MIDI output for LED feedback
//!
//! Translates protocol-agnostic feedback results into MIDI messages.
//! The evaluation logic lives in `crate::feedback`; this module only
//! handles the MIDI-specific byte encoding and sending.
//!
//! Supports two LED color modes:
//! - **Velocity mode** (default): LED brightness/color via note velocity (0-127)
//! - **Note-offset mode** (`color_note_offsets`): LED color layer via note number offset,
//!   binary on/off via velocity. Used by Allen & Heath Xone K series. The K3 has full
//!   RGB LEDs with a 16-color palette — actual colors are set in the Xone Controller
//!   Editor; MIDI only selects which of 3 layers (and thus which configured color) to show.

use crate::config::{ColorNoteOffsets, DeviceProfile, MidiControlConfig};
use crate::deck_target::DeckTargetState;
use crate::feedback::{
    evaluate_feedback, FeedbackChangeTracker, FeedbackResult, FeedbackState,
};
use crate::types::ControlAddress;
use midir::MidiOutputConnection;
use std::collections::HashMap;

// Re-export feedback types for backwards compatibility
pub use crate::feedback::{ActionMode, DeckFeedbackState, MixerFeedbackState};

/// MIDI output handler for LED feedback
pub struct MidiOutputHandler {
    /// MIDI output connection
    connection: MidiOutputConnection,
    /// Feedback mappings from config (owned copy for evaluation)
    feedback_mappings: Vec<crate::config::FeedbackMapping>,
    /// Change tracker (replaces old last_values HashMap)
    change_tracker: FeedbackChangeTracker,
    /// Note-offset LED color mode (e.g., Xone K series)
    color_note_offsets: Option<ColorNoteOffsets>,
    /// Last note offset sent per address (for correct note-off in note-offset mode)
    last_note_offsets: HashMap<ControlAddress, u8>,
}

impl MidiOutputHandler {
    /// Create a new output handler
    pub fn new(connection: MidiOutputConnection, profile: &DeviceProfile) -> Self {
        Self {
            connection,
            feedback_mappings: profile.feedback.clone(),
            change_tracker: FeedbackChangeTracker::new(),
            color_note_offsets: profile.color_note_offsets.clone(),
            last_note_offsets: HashMap::new(),
        }
    }

    /// Update LEDs based on current application state
    ///
    /// Evaluates all feedback mappings, then sends MIDI messages for changed values.
    pub fn update(&mut self, state: &FeedbackState, deck_target: &DeckTargetState) {
        let results = evaluate_feedback(&self.feedback_mappings, state, deck_target);
        self.apply_feedback(&results);
    }

    /// Apply evaluated feedback results, sending MIDI for changed values
    pub fn apply_feedback(&mut self, results: &[FeedbackResult]) {
        for result in results {
            // Only handle MIDI addresses
            let Some(midi_ctrl) = result.address.as_midi_control_config() else {
                continue;
            };

            if let Some(ref offsets) = self.color_note_offsets {
                // Note-offset mode: color via note offset, binary on/off.
                // Each color is a different MIDI note (base + offset), so we must
                // turn off the OLD note before turning on a NEW one when colors change.
                let is_on = result.value >= 64;
                let new_offset = if is_on {
                    rgb_to_note_offset(result.color, offsets)
                } else {
                    0
                };
                // Encode on/off + color offset into a single tracker value:
                // 0 = off, offset+1 = on with that color
                let state = if is_on { new_offset.wrapping_add(1) } else { 0 };
                if self.change_tracker.update(&result.address, state, None) {
                    // Turn off the previous color note before changing state
                    if let Some(old_offset) = self.last_note_offsets.remove(&result.address) {
                        self.send_midi_note_offset(&midi_ctrl, 0, old_offset);
                    }
                    if is_on {
                        self.send_midi_note_offset(&midi_ctrl, 127, new_offset);
                        self.last_note_offsets.insert(result.address.clone(), new_offset);
                    }
                }
            } else {
                // Standard velocity mode
                if self.change_tracker.update(&result.address, result.value, None) {
                    self.send_midi(&midi_ctrl, result.value);
                }
            }
        }
    }

    /// Send a MIDI feedback message with a note offset added to the note number
    fn send_midi_note_offset(&mut self, control: &MidiControlConfig, value: u8, note_offset: u8) {
        if let MidiControlConfig::Note { channel, note } = control {
            let offset_note = note.wrapping_add(note_offset);
            log::debug!(
                "[MIDI OUT] Note ch={} note={:#04x}+{} val={}",
                channel, note, note_offset, value
            );
            let message = if value > 0 {
                vec![0x90 | channel, offset_note, value]
            } else {
                vec![0x80 | channel, offset_note, 0]
            };
            if let Err(e) = self.connection.send(&message) {
                log::warn!("MIDI output: Failed to send message: {}", e);
            }
        } else {
            // CC controls don't support note offsets, fall back to standard
            self.send_midi(control, value);
        }
    }

    /// Send a raw MIDI feedback message
    fn send_midi(&mut self, control: &MidiControlConfig, value: u8) {
        let message = match control {
            MidiControlConfig::Note { channel, note } => {
                log::debug!(
                    "[MIDI OUT] Note ch={} note={:#04x} val={}",
                    channel, note, value
                );
                if value > 0 {
                    vec![0x90 | channel, *note, value]
                } else {
                    vec![0x80 | channel, *note, 0]
                }
            }
            MidiControlConfig::ControlChange { channel, cc } => {
                log::debug!(
                    "[MIDI OUT] CC ch={} cc={:#04x} val={}",
                    channel, cc, value
                );
                vec![0xB0 | channel, *cc, value]
            }
        };

        if let Err(e) = self.connection.send(&message) {
            log::warn!("MIDI output: Failed to send message: {}", e);
        }
    }

    /// Force send a MIDI message (bypass change detection)
    pub fn send(&mut self, control: &MidiControlConfig, value: u8) {
        self.send_midi(control, value);
        // Update tracker so subsequent update() won't re-send
        let address = ControlAddress::from(control);
        self.change_tracker.update(&address, value, None);
    }

    /// Clear all LEDs (send off value for all tracked controls)
    pub fn clear_all(&mut self) {
        // Turn off note-offset LEDs using their last-known offsets
        let offset_entries: Vec<(ControlAddress, u8)> = self
            .last_note_offsets
            .drain()
            .collect();
        for (address, offset) in &offset_entries {
            if let Some(control) = address.as_midi_control_config() {
                self.send_midi_note_offset(&control, 0, *offset);
            }
        }

        // Turn off remaining standard (non-offset) controls
        let addresses: Vec<ControlAddress> = self
            .change_tracker
            .tracked_addresses()
            .cloned()
            .collect();
        for address in &addresses {
            if let Some(control) = address.as_midi_control_config() {
                self.send_midi(&control, 0);
            }
        }
        self.change_tracker.clear();
    }
}

impl Drop for MidiOutputHandler {
    fn drop(&mut self) {
        self.clear_all();
    }
}

/// Map an RGB color to one of three note-offset layers for LED controllers.
///
/// On the Xone K series, each button has 3 layers activated by note offsets.
/// The actual displayed color depends on the Xone Controller Editor configuration.
/// This function selects which layer to activate based on the RGB color's hue:
///
/// - Warm colors (red, orange, bronze) → layer 1 (red offset)
/// - Neutral colors (amber, yellow, white, equal mix) → layer 2 (amber offset)
/// - Cool colors (green, lime, cyan, blue, lavender) → layer 3 (green offset)
fn rgb_to_note_offset(color: Option<[u8; 3]>, offsets: &ColorNoteOffsets) -> u8 {
    match color {
        Some([r, g, b]) => {
            // Find the dominant channel(s) to classify the hue
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            if max == 0 || (max - min) < 20 {
                // Very dark or near-grey/white → amber (neutral layer)
                return offsets.amber;
            }
            if g >= r && g >= b {
                // Green dominant (green, lime, cyan-ish) → green layer
                offsets.green
            } else if r >= g && r >= b && r > b + 30 {
                // Red dominant and clearly warmer than blue (red, orange, bronze) → red layer
                offsets.red
            } else {
                // Blue dominant, purple, lavender, or mixed → amber layer
                offsets.amber
            }
        }
        None => offsets.red,
    }
}
