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
mod detection;
mod input;
mod mapping;
mod messages;
mod normalize;
mod output;

pub use config::{
    default_midi_config_path, load_midi_config, normalize_port_name, port_matches, save_midi_config,
    ControlBehavior, ControlMapping, DeckTargetConfig, DeviceProfile, EncoderMode, FeedbackMapping,
    HardwareType, MidiConfig, MidiControlConfig, PadModeSource,
};
pub use connection::{MidiConnection, MidiConnectionError};
pub use detection::{MidiSample, MidiSampleBuffer};
pub use deck_target::{DeckTargetMode, DeckTargetState, LayerSelection};
pub use input::{MidiInputEvent, MidiInputHandler};
pub use mapping::{ActionRegistry, MappingEngine};
pub use messages::{DeckAction, GlobalAction, MidiMessage, MixerAction, BrowserAction};
pub use normalize::{normalize_cc_value, ControlRange};
pub use output::{ActionMode, DeckFeedbackState, FeedbackState, MidiOutputHandler, MixerFeedbackState};

use flume::{Receiver, Sender};
use std::collections::HashMap;
use std::sync::Arc;

/// A connected MIDI device with its handlers and state
struct ConnectedDevice {
    /// The device profile from config
    profile: DeviceProfile,
    /// Input handler (owns midir connection)
    input_handler: MidiInputHandler,
    /// Output handler for LED feedback (optional - some devices are input-only)
    output_handler: Option<MidiOutputHandler>,
    /// Deck targeting state for this device
    deck_target_state: DeckTargetState,
}

/// Main MIDI controller manager
///
/// Handles device connection, input processing, and LED feedback.
/// Supports multiple simultaneous MIDI devices.
/// Designed to integrate with iced via async subscription.
pub struct MidiController {
    /// Loaded configuration
    config: MidiConfig,
    /// Connected devices keyed by normalized port name
    connected_devices: HashMap<String, ConnectedDevice>,
    /// Receiver for parsed MIDI messages (shared by all devices)
    message_rx: Receiver<MidiMessage>,
    /// Sender for MIDI messages (passed to all handlers)
    message_tx: Sender<MidiMessage>,
    /// Current shift state (global across all devices)
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
    /// Attempts to connect to all MIDI devices matching the config.
    /// Returns Ok even if no devices are found (graceful degradation).
    pub fn new(config_path: Option<&std::path::Path>) -> Result<Self, MidiError> {
        Self::new_with_options(config_path, false)
    }

    /// Create a new MIDI controller with options
    ///
    /// - `capture_raw`: Enable raw event capture for MIDI learn mode
    pub fn new_with_options(
        config_path: Option<&std::path::Path>,
        capture_raw: bool,
    ) -> Result<Self, MidiError> {
        let config_path = config_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_midi_config_path);

        let config = load_midi_config(&config_path);

        // Create shared channel for MIDI messages (all devices feed into this)
        let (message_tx, message_rx) = flume::bounded(256);

        let mut controller = Self {
            config,
            connected_devices: HashMap::new(),
            message_rx,
            message_tx: message_tx.clone(),
            shift_held: false,
        };

        // Try to connect to all matching devices
        controller.try_connect_all(capture_raw)?;

        Ok(controller)
    }

    /// Create a MIDI controller for learn mode - connects to ALL available ports
    ///
    /// Unlike `new_with_options`, this ignores the config and connects to every
    /// available MIDI input port. Use this when you need to discover which device
    /// the user is interacting with (MIDI learn mode).
    pub fn new_for_learn_mode() -> Result<Self, MidiError> {
        // Create shared channel for MIDI messages
        let (message_tx, message_rx) = flume::bounded(256);

        let mut controller = Self {
            config: MidiConfig::default(),
            connected_devices: HashMap::new(),
            message_rx,
            message_tx: message_tx.clone(),
            shift_held: false,
        };

        // Connect to ALL available ports
        controller.connect_all_ports_for_learn()?;

        Ok(controller)
    }

    /// Connect to all available MIDI input ports (for learn mode)
    fn connect_all_ports_for_learn(&mut self) -> Result<(), MidiError> {
        let available_ports = match MidiConnection::list_input_ports() {
            Ok(ports) => ports,
            Err(e) => {
                log::warn!("MIDI Learn: Failed to list input ports: {}", e);
                return Ok(());
            }
        };

        if available_ports.is_empty() {
            log::info!("MIDI Learn: No input ports available on system");
            return Ok(());
        }

        log::info!("MIDI Learn: Connecting to all {} available ports...", available_ports.len());

        for port_name in &available_ports {
            // Skip "Midi Through" virtual ports - they just echo back
            if port_name.to_lowercase().contains("midi through") {
                log::debug!("MIDI Learn: Skipping virtual port '{}'", port_name);
                continue;
            }

            let normalized = normalize_port_name(port_name);

            // Create a minimal profile for this port (no mappings, just raw capture)
            let learn_profile = DeviceProfile {
                name: normalized.clone(),
                port_match: normalized.clone(),
                learned_port_name: Some(normalized.clone()),
                deck_target: DeckTargetConfig::default(),
                pad_mode_source: PadModeSource::default(),
                shift: None,
                mappings: vec![],
                feedback: vec![],
            };

            // Create a dummy mapping engine (won't be used, we only want raw events)
            let mapping_engine = Arc::new(MappingEngine::new(&learn_profile));

            match MidiInputHandler::connect_with_raw_events(
                port_name,
                self.message_tx.clone(),
                mapping_engine,
                None, // No shift button in learn mode
                true, // Always capture raw in learn mode
            ) {
                Ok(input_handler) => {
                    log::info!("MIDI Learn: Connected to '{}'", port_name);

                    self.connected_devices.insert(
                        normalized.clone(),
                        ConnectedDevice {
                            profile: learn_profile,
                            input_handler,
                            output_handler: None, // No output needed for learn
                            deck_target_state: DeckTargetState::default(),
                        },
                    );
                }
                Err(e) => {
                    log::debug!("MIDI Learn: Failed to connect to '{}': {}", port_name, e);
                }
            }
        }

        if self.connected_devices.is_empty() {
            log::warn!("MIDI Learn: Could not connect to any MIDI ports");
        } else {
            log::info!(
                "MIDI Learn: Connected to {} device(s), ready to capture",
                self.connected_devices.len()
            );
        }

        Ok(())
    }

    /// Try to connect to all MIDI devices matching config profiles
    ///
    /// Unlike the old single-device model, this connects to ALL matching devices,
    /// not just the first one found.
    fn try_connect_all(&mut self, capture_raw: bool) -> Result<(), MidiError> {
        // Get all available ports first
        let available_ports = match MidiConnection::list_input_ports() {
            Ok(ports) => ports,
            Err(e) => {
                log::warn!("MIDI: Failed to list input ports: {}", e);
                return Ok(());
            }
        };

        if available_ports.is_empty() {
            log::info!("MIDI: No input ports available on system");
            return Ok(());
        }

        log::debug!("MIDI: Available ports: {:?}", available_ports);

        // Try to match each profile to an available port
        for profile in &self.config.devices {
            // Find a matching port using our matching logic
            let matching_port = available_ports.iter().find(|port| port_matches(port, profile));

            if let Some(port_name) = matching_port {
                let normalized = normalize_port_name(port_name);

                // Skip if we've already connected to this port
                if self.connected_devices.contains_key(&normalized) {
                    log::debug!(
                        "MIDI: Port '{}' already connected, skipping duplicate profile '{}'",
                        normalized,
                        profile.name
                    );
                    continue;
                }

                // Create mapping engine for this profile
                let mapping_engine = Arc::new(MappingEngine::new(profile));

                // Try to connect input handler
                match MidiInputHandler::connect_with_raw_events(
                    port_name, // Use actual port name for connection
                    self.message_tx.clone(),
                    mapping_engine,
                    profile.shift.clone(),
                    capture_raw,
                ) {
                    Ok(input_handler) => {
                        log::info!(
                            "MIDI: Connected '{}' to port '{}'",
                            profile.name,
                            port_name
                        );

                        // Set up deck targeting from profile
                        let deck_target_state = DeckTargetState::from_config(&profile.deck_target);

                        // Try to connect output for LED feedback
                        let output_handler =
                            MidiConnection::connect_output(port_name).map(|out_conn| {
                                log::info!("MIDI: Output connected for '{}'", profile.name);
                                MidiOutputHandler::new(out_conn, profile)
                            });

                        self.connected_devices.insert(
                            normalized.clone(),
                            ConnectedDevice {
                                profile: profile.clone(),
                                input_handler,
                                output_handler,
                                deck_target_state,
                            },
                        );
                    }
                    Err(e) => {
                        log::debug!(
                            "MIDI: Failed to connect '{}' to port '{}': {}",
                            profile.name,
                            port_name,
                            e
                        );
                    }
                }
            } else {
                log::debug!(
                    "MIDI: No matching port for profile '{}' (port_match: '{}', learned: {:?})",
                    profile.name,
                    profile.port_match,
                    profile.learned_port_name
                );
            }
        }

        // Summary logging
        if self.connected_devices.is_empty() {
            log::info!("MIDI: No matching devices found. Available ports:");
            for port in &available_ports {
                log::info!("  - {}", port);
            }
            log::info!("MIDI: Running without MIDI support");
        } else {
            log::info!(
                "MIDI: Connected to {} device(s)",
                self.connected_devices.len()
            );
        }

        Ok(())
    }

    /// Check if any MIDI device is connected
    pub fn is_connected(&self) -> bool {
        !self.connected_devices.is_empty()
    }

    /// Get the number of connected devices
    pub fn connected_count(&self) -> usize {
        self.connected_devices.len()
    }

    /// Get the names of connected devices
    pub fn connected_device_names(&self) -> Vec<&str> {
        self.connected_devices
            .values()
            .map(|d| d.profile.name.as_str())
            .collect()
    }

    /// Get the first connected port name (for learn mode)
    pub fn first_connected_port(&self) -> Option<&str> {
        self.connected_devices.keys().next().map(|s| s.as_str())
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
    /// Toggles layer on all connected devices for consistency.
    pub fn toggle_layer(&mut self, physical_deck: usize) {
        for device in self.connected_devices.values_mut() {
            device.deck_target_state.toggle_layer(physical_deck);
        }
        // Log using first device's state (they should all be in sync)
        if let Some(device) = self.connected_devices.values().next() {
            log::debug!(
                "MIDI: Layer toggled for physical deck {}, now {:?}",
                physical_deck,
                device.deck_target_state.get_layer(physical_deck)
            );
        }
    }

    /// Get the current virtual deck for a physical deck
    ///
    /// Uses the first connected device's state (devices are kept in sync).
    pub fn resolve_deck(&self, physical_deck: usize) -> usize {
        self.connected_devices
            .values()
            .next()
            .map(|d| d.deck_target_state.resolve_deck(physical_deck))
            .unwrap_or(physical_deck)
    }

    /// Get current layer selection for a physical deck
    ///
    /// Uses the first connected device's state (devices are kept in sync).
    pub fn get_layer(&self, physical_deck: usize) -> LayerSelection {
        self.connected_devices
            .values()
            .next()
            .map(|d| d.deck_target_state.get_layer(physical_deck))
            .unwrap_or(LayerSelection::A)
    }

    /// Update LED feedback based on current application state
    ///
    /// Call this periodically (e.g., in iced Tick handler) to update controller LEDs.
    /// Updates all connected devices.
    pub fn update_feedback(&mut self, state: &FeedbackState) {
        for device in self.connected_devices.values_mut() {
            if let Some(ref mut output) = device.output_handler {
                output.update(state, &device.deck_target_state);
            }
        }
    }

    /// Set shift state (for coordinating with app's shift state)
    ///
    /// Shift state is global across all devices.
    pub fn set_shift(&mut self, held: bool) {
        self.shift_held = held;
    }

    /// Get current shift state
    pub fn is_shift_held(&self) -> bool {
        self.shift_held
    }

    /// Get the pad mode source from the first connected device
    ///
    /// Returns `PadModeSource::App` (default) if no device is connected.
    pub fn pad_mode_source(&self) -> PadModeSource {
        self.connected_devices
            .values()
            .next()
            .map(|d| d.profile.pad_mode_source)
            .unwrap_or_default()
    }

    /// Drain all pending raw MIDI events from all devices (for learn mode)
    ///
    /// Returns an iterator over raw events. Only available if created
    /// with `new_with_options(..., capture_raw: true)`.
    pub fn drain_raw_events(&self) -> impl Iterator<Item = MidiInputEvent> + '_ {
        self.connected_devices.values().flat_map(|device| {
            std::iter::from_fn({
                let handler = &device.input_handler;
                move || handler.drain_raw_events().next()
            })
        })
    }

    /// Drain all pending raw MIDI events with their source device (for learn mode)
    ///
    /// Returns tuples of (event, normalized_port_name). This allows MIDI learn mode
    /// to capture which device sent the first event and store it in the config.
    pub fn drain_raw_events_with_source(&self) -> Vec<(MidiInputEvent, String)> {
        let mut events = Vec::new();
        for (port_name, device) in &self.connected_devices {
            for event in device.input_handler.drain_raw_events() {
                events.push((event, port_name.clone()));
            }
        }
        events
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
