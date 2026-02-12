//! MIDI output for LED feedback
//!
//! Translates protocol-agnostic feedback results into MIDI messages.
//! The evaluation logic lives in `crate::feedback`; this module only
//! handles the MIDI-specific byte encoding and sending.

use crate::config::{DeviceProfile, MidiControlConfig};
use crate::deck_target::DeckTargetState;
use crate::feedback::{
    evaluate_feedback, FeedbackChangeTracker, FeedbackResult, FeedbackState,
};
use crate::types::ControlAddress;
use midir::MidiOutputConnection;

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
}

impl MidiOutputHandler {
    /// Create a new output handler
    pub fn new(connection: MidiOutputConnection, profile: &DeviceProfile) -> Self {
        Self {
            connection,
            feedback_mappings: profile.feedback.clone(),
            change_tracker: FeedbackChangeTracker::new(),
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
            if let Some(midi_ctrl) = result.address.as_midi_control_config() {
                if let Some(value) = self.change_tracker.update(&result.address, result.value) {
                    self.send_midi(&midi_ctrl, value);
                }
            }
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
        self.change_tracker.update(&address, value);
    }

    /// Clear all LEDs (send off value for all tracked controls)
    pub fn clear_all(&mut self) {
        // Collect addresses to avoid borrow issues
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
