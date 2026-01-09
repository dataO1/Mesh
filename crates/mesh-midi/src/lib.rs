//! MIDI controller support for mesh DJ application
//!
//! This crate provides:
//! - MIDI device connection and input handling via midir
//! - MIDI message parsing via midly
//! - Configurable control-to-action mapping
//! - Deck layer targeting (for 2-deck controllers accessing 4 virtual decks)
//! - LED feedback output
//! - Async channel bridge for iced subscriptions
//!
//! # Architecture
//!
//! ```text
//! MIDI Device → midir callback → flume channel → iced subscription → app.update()
//! ```
//!
//! The midir callback is synchronous, but we bridge to async via flume's
//! `recv_async()` which works with iced's `subscription::channel`.

mod config;
mod connection;
mod deck_target;
mod input;
mod mapping;
mod messages;
mod normalize;
mod output;

pub use config::{
    default_midi_config_path, load_midi_config, ControlBehavior, ControlMapping,
    DeckTargetConfig, DeviceProfile, FeedbackMapping, MidiConfig, MidiControlConfig,
};
pub use connection::{MidiConnection, MidiConnectionError};
pub use deck_target::{DeckTargetMode, DeckTargetState, LayerSelection};
pub use input::{MidiInputEvent, MidiInputHandler};
pub use mapping::{ActionRegistry, MappingEngine};
pub use messages::{DeckAction, GlobalAction, MidiMessage, MixerAction, BrowserAction};
pub use normalize::{normalize_cc_value, ControlRange};
pub use output::{FeedbackState, MidiOutputHandler};

use flume::{Receiver, Sender};
use std::sync::Arc;

/// Main MIDI controller manager
///
/// Handles device connection, input processing, and LED feedback.
/// Designed to integrate with iced via async subscription.
pub struct MidiController {
    /// Loaded configuration
    config: MidiConfig,
    /// Active device profile (if connected)
    active_profile: Option<DeviceProfile>,
    /// Receiver for parsed MIDI messages (for iced subscription)
    message_rx: Receiver<MidiMessage>,
    /// Input handler (owns midir connection)
    input_handler: Option<MidiInputHandler>,
    /// Output handler for LED feedback
    output_handler: Option<MidiOutputHandler>,
    /// Current deck targeting state
    deck_target_state: DeckTargetState,
    /// Current shift state
    shift_held: bool,
}

/// Error type for MIDI controller operations
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("Failed to load MIDI config: {0}")]
    ConfigError(#[from] anyhow::Error),

    #[error("MIDI connection error: {0}")]
    ConnectionError(#[from] MidiConnectionError),

    #[error("No MIDI device found matching config")]
    NoDeviceFound,

    #[error("MIDI output error: {0}")]
    OutputError(String),
}

impl MidiController {
    /// Create a new MIDI controller from config file
    ///
    /// Attempts to connect to a MIDI device matching the config.
    /// Returns Ok even if no device is found (graceful degradation).
    pub fn new(config_path: Option<&std::path::Path>) -> Result<Self, MidiError> {
        let config_path = config_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_midi_config_path);

        let config = load_midi_config(&config_path);

        // Create channel for MIDI messages
        let (message_tx, message_rx) = flume::bounded(256);

        let mut controller = Self {
            config,
            active_profile: None,
            message_rx,
            input_handler: None,
            output_handler: None,
            deck_target_state: DeckTargetState::default(),
            shift_held: false,
        };

        // Try to connect to a device
        controller.try_connect(message_tx)?;

        Ok(controller)
    }

    /// Try to connect to a MIDI device matching config
    fn try_connect(&mut self, message_tx: Sender<MidiMessage>) -> Result<(), MidiError> {
        for profile in &self.config.devices {
            // Create mapping engine
            let mapping_engine = Arc::new(MappingEngine::new(profile));

            // Try to connect input handler
            match MidiInputHandler::connect(
                &profile.port_match,
                message_tx.clone(),
                mapping_engine,
                profile.shift.clone(),
            ) {
                Ok(input_handler) => {
                    log::info!(
                        "MIDI: Connected to device matching '{}'",
                        profile.port_match
                    );

                    // Set up deck targeting from profile
                    self.deck_target_state = DeckTargetState::from_config(&profile.deck_target);

                    self.input_handler = Some(input_handler);

                    // Try to connect output for LED feedback
                    if let Some(out_conn) = MidiConnection::connect_output(&profile.port_match) {
                        self.output_handler = Some(MidiOutputHandler::new(out_conn, profile));
                    }

                    self.active_profile = Some(profile.clone());
                    return Ok(());
                }
                Err(e) => {
                    log::debug!(
                        "MIDI: No device found matching '{}': {}",
                        profile.port_match,
                        e
                    );
                }
            }
        }

        log::info!("MIDI: No matching devices found, running without MIDI support");
        Ok(())
    }

    /// Check if a MIDI device is connected
    pub fn is_connected(&self) -> bool {
        self.active_profile.is_some()
    }

    /// Get the message receiver for manual polling
    ///
    /// Use this if you need direct access to the receiver.
    pub fn message_receiver(&self) -> Receiver<MidiMessage> {
        self.message_rx.clone()
    }

    /// Try to receive a pending MIDI message (non-blocking)
    ///
    /// Call this in your Tick handler to process MIDI input.
    /// Returns None if no messages are pending.
    pub fn try_recv(&self) -> Option<MidiMessage> {
        self.message_rx.try_recv().ok()
    }

    /// Drain all pending MIDI messages
    ///
    /// Returns an iterator over all pending messages.
    pub fn drain(&self) -> impl Iterator<Item = MidiMessage> + '_ {
        std::iter::from_fn(|| self.try_recv())
    }

    /// Handle a layer toggle event
    ///
    /// Called when a deck layer toggle button is pressed.
    pub fn toggle_layer(&mut self, physical_deck: usize) {
        self.deck_target_state.toggle_layer(physical_deck);
        log::debug!(
            "MIDI: Layer toggled for physical deck {}, now {:?}",
            physical_deck,
            self.deck_target_state.get_layer(physical_deck)
        );
    }

    /// Get the current virtual deck for a physical deck
    pub fn resolve_deck(&self, physical_deck: usize) -> usize {
        self.deck_target_state.resolve_deck(physical_deck)
    }

    /// Get current layer selection for a physical deck
    pub fn get_layer(&self, physical_deck: usize) -> LayerSelection {
        self.deck_target_state.get_layer(physical_deck)
    }

    /// Update LED feedback based on current application state
    ///
    /// Call this periodically (e.g., in iced Tick handler) to update controller LEDs.
    pub fn update_feedback(&mut self, state: &FeedbackState) {
        if let Some(ref mut output) = self.output_handler {
            output.update(state, &self.deck_target_state);
        }
    }

    /// Set shift state (for coordinating with app's shift state)
    pub fn set_shift(&mut self, held: bool) {
        self.shift_held = held;
    }

    /// Get current shift state
    pub fn is_shift_held(&self) -> bool {
        self.shift_held
    }
}

// Note: MIDI integration with iced is done by polling in the Tick handler,
// which provides sub-frame latency (16ms tick @ 60fps is fast enough for MIDI).
// This avoids complex subscription machinery while keeping the code simple.
//
// Usage in app:
// ```
// Message::Tick => {
//     if let Some(ref midi) = self.midi_controller {
//         while let Some(msg) = midi.try_recv() {
//             self.handle_midi_message(msg);
//         }
//     }
// }
// ```
