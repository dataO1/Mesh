//! Controller support for mesh DJ application
//!
//! This crate provides protocol-agnostic controller support:
//! - MIDI device connection and input handling via midir
//! - HID device support (Kontrol F1, etc.) via hidapi
//! - Abstract control event layer (ControlEvent, ControlAddress)
//! - Configurable control-to-action mapping
//! - Deck layer targeting (for 2-deck controllers accessing 4 virtual decks)
//! - LED/RGB feedback output
//! - Async channel bridge for iced subscriptions
//!
//! # Architecture
//!
//! ```text
//! MIDI Device → midir callback ─┐
//!                                ├─→ ControlEvent → MappingEngine → MidiMessage → App
//! HID Device  → I/O thread ─────┘
//!
//! App → FeedbackState → FeedbackEvaluator → per-device output adapters
//! ```

mod config;
mod deck_target;
mod detection;
pub mod feedback;
pub mod hid;
mod mapping;
mod messages;
pub mod midi;
mod normalize;
mod shared_state;
pub mod types;

// Re-export config types
pub use config::{
    default_midi_config_path, load_midi_config, normalize_port_name, port_matches,
    save_midi_config, ControlBehavior, ControlMapping, DeckTargetConfig, DeviceProfile,
    EncoderMode, FeedbackMapping, HardwareType, MidiConfig, MidiControlConfig, PadModeSource,
    ShiftButtonConfig,
};

// Re-export MIDI backend types
pub use midi::connection::{MidiConnection, MidiConnectionError};
pub use midi::input::{MidiInputEvent, MidiInputHandler};
pub use midi::output::MidiOutputHandler;

// Re-export core types
pub use detection::{MidiSample, MidiSampleBuffer};
pub use deck_target::{DeckTargetMode, DeckTargetState, LayerSelection};
pub use mapping::{ActionRegistry, MappingEngine};
pub use messages::{DeckAction, GlobalAction, MidiMessage, MixerAction, BrowserAction};
pub use normalize::{normalize_cc_value, ControlRange};
pub use shared_state::{SharedState, SharedMidiState};

// Re-export abstract types
pub use types::{ControlAddress, ControlDescriptor, ControlEvent, ControlValue, FeedbackCommand, MidiAddress};

// Re-export feedback types
pub use feedback::{
    evaluate_feedback, ActionMode, DeckFeedbackState, FeedbackChangeTracker,
    FeedbackResult, FeedbackState, MixerFeedbackState,
};

use flume::{Receiver, Sender};
use std::collections::HashMap;
use std::sync::Arc;

// ═══════════════════════════════════════════════════════════════════════
// Connected device types
// ═══════════════════════════════════════════════════════════════════════

/// A connected MIDI device with its handlers and state
struct ConnectedMidiDevice {
    /// The device profile from config
    profile: DeviceProfile,
    /// Input handler (owns midir connection)
    input_handler: MidiInputHandler,
    /// Output handler for LED feedback (optional - some devices are input-only)
    output_handler: Option<MidiOutputHandler>,
    /// Shared state for this device (shift, layers)
    shared_state: Arc<SharedState>,
}

/// A connected HID device with its I/O thread and feedback channel
struct ConnectedHidDevice {
    /// Device info (VID, PID, name, path)
    info: hid::HidDeviceInfo,
    /// HID connection (owns I/O thread lifetime)
    _connection: hid::HidConnection,
    /// Output handler for LED feedback
    output_handler: hid::HidOutputHandler,
    /// Control descriptors from the driver (collected before driver moves to I/O thread)
    control_descriptors: Vec<ControlDescriptor>,
    /// Device profile from config (if matched)
    profile: Option<DeviceProfile>,
    /// Mapping engine built from matched profile
    mapping_engine: Option<Arc<MappingEngine>>,
    /// Shared state for shift/layer (shared with mapping engine)
    shared_state: Option<Arc<SharedState>>,
}

// ═══════════════════════════════════════════════════════════════════════
// Controller manager
// ═══════════════════════════════════════════════════════════════════════

/// Unified controller manager for MIDI and HID devices
///
/// Handles device connection, input processing, and LED feedback for
/// all supported controller protocols. Supports multiple simultaneous devices.
/// Designed to integrate with iced via async subscription.
pub struct ControllerManager {
    /// Loaded configuration
    config: MidiConfig,
    /// Connected MIDI devices keyed by normalized port name
    midi_devices: HashMap<String, ConnectedMidiDevice>,
    /// Connected HID devices keyed by device path
    hid_devices: HashMap<String, ConnectedHidDevice>,
    /// Receiver for parsed messages (shared by all MIDI devices)
    message_rx: Receiver<MidiMessage>,
    /// Sender for messages (passed to all MIDI handlers)
    message_tx: Sender<MidiMessage>,
    /// Receiver for raw HID control events (for learn mode)
    hid_event_rx: Option<Receiver<ControlEvent>>,
    /// Sender for HID control events (passed to I/O threads)
    hid_event_tx: Option<Sender<ControlEvent>>,
}

/// Backwards-compatible type alias
pub type MidiController = ControllerManager;

/// Error type for controller operations
#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("Failed to load config: {0}")]
    ConfigError(#[from] anyhow::Error),

    #[error("MIDI connection error: {0}")]
    ConnectionError(#[from] MidiConnectionError),

    #[error("No device found matching config")]
    NoDeviceFound,

    #[error("Output error: {0}")]
    OutputError(String),

    #[error("HID error: {0}")]
    HidError(String),
}

/// Extract shift button controls from a device profile
fn extract_shift_buttons(profile: &DeviceProfile) -> Vec<(ControlAddress, usize)> {
    profile
        .shift_buttons
        .iter()
        .map(|sb| (sb.control.clone(), sb.physical_deck))
        .collect()
}

/// Extract layer toggle controls from a device profile's deck target config
fn extract_toggle_controls(profile: &DeviceProfile) -> Vec<(ControlAddress, usize)> {
    match &profile.deck_target {
        DeckTargetConfig::Layer {
            toggle_left,
            toggle_right,
            ..
        } => vec![
            (toggle_left.clone(), 0),
            (toggle_right.clone(), 1),
        ],
        DeckTargetConfig::Direct { .. } => vec![],
    }
}

impl ControllerManager {
    /// Create a new controller manager from config file
    ///
    /// Attempts to connect to all MIDI and HID devices matching the config.
    /// Returns Ok even if no devices are found (graceful degradation).
    pub fn new(config_path: Option<&std::path::Path>) -> Result<Self, MidiError> {
        Self::new_with_options(config_path, false)
    }

    /// Create a new controller manager with options
    ///
    /// - `capture_raw`: Enable raw event capture for learn mode
    pub fn new_with_options(
        config_path: Option<&std::path::Path>,
        capture_raw: bool,
    ) -> Result<Self, MidiError> {
        let config_path = config_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_midi_config_path);

        let config = load_midi_config(&config_path);

        // Create shared channel for MIDI messages (all MIDI devices feed into this)
        let (message_tx, message_rx) = flume::bounded(256);

        // Create HID event channel (always available — used for learn mode capture
        // and normal mode processing when HID mapping support is added)
        let (hid_tx, hid_rx) = flume::bounded(256);

        let mut controller = Self {
            config,
            midi_devices: HashMap::new(),
            hid_devices: HashMap::new(),
            message_rx,
            message_tx: message_tx.clone(),
            hid_event_rx: Some(hid_rx),
            hid_event_tx: Some(hid_tx),
        };

        // Try to connect to all matching devices
        controller.try_connect_all_midi(capture_raw)?;
        controller.try_connect_all_hid();

        Ok(controller)
    }

    /// Create a controller manager for learn mode - connects to ALL available devices
    ///
    /// Connects to every available MIDI input port and all known HID devices.
    /// Use this when you need to discover which device the user is interacting with.
    pub fn new_for_learn_mode() -> Result<Self, MidiError> {
        // Create shared channel for MIDI messages
        let (message_tx, message_rx) = flume::bounded(256);

        // Create channel for HID events in learn mode
        let (hid_tx, hid_rx) = flume::bounded(256);

        let mut controller = Self {
            config: MidiConfig::default(),
            midi_devices: HashMap::new(),
            hid_devices: HashMap::new(),
            message_rx,
            message_tx: message_tx.clone(),
            hid_event_rx: Some(hid_rx),
            hid_event_tx: Some(hid_tx),
        };

        // Connect to ALL available MIDI ports
        controller.connect_all_midi_for_learn()?;
        // Connect to ALL known HID devices
        controller.connect_all_hid_for_learn();

        Ok(controller)
    }

    // ─── MIDI connection methods ───────────────────────────────────────

    /// Connect to all available MIDI input ports (for learn mode)
    fn connect_all_midi_for_learn(&mut self) -> Result<(), MidiError> {
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
                device_type: None,
                hid_product_match: None,
                deck_target: DeckTargetConfig::default(),
                pad_mode_source: PadModeSource::default(),
                shift_buttons: vec![],
                mappings: vec![],
                feedback: vec![],
            };

            // Create shared state (default - no layers, no shift)
            let shared_state = Arc::new(SharedState::default());

            // Create a dummy mapping engine (won't be used, we only want raw events)
            let mapping_engine = Arc::new(MappingEngine::new(&learn_profile, shared_state.clone()));

            match MidiInputHandler::connect_with_raw_events(
                port_name,
                self.message_tx.clone(),
                mapping_engine,
                shared_state.clone(),
                vec![], // No shift buttons in learn mode
                vec![], // No toggle controls in learn mode
                true,   // Always capture raw in learn mode
            ) {
                Ok(input_handler) => {
                    log::info!("MIDI Learn: Connected to '{}'", port_name);

                    self.midi_devices.insert(
                        normalized.clone(),
                        ConnectedMidiDevice {
                            profile: learn_profile,
                            input_handler,
                            output_handler: None,
                            shared_state,
                        },
                    );
                }
                Err(e) => {
                    log::debug!("MIDI Learn: Failed to connect to '{}': {}", port_name, e);
                }
            }
        }

        if self.midi_devices.is_empty() {
            log::info!("MIDI Learn: No MIDI ports connected");
        } else {
            log::info!(
                "MIDI Learn: Connected to {} MIDI device(s)",
                self.midi_devices.len()
            );
        }

        Ok(())
    }

    /// Try to connect to all MIDI devices matching config profiles
    fn try_connect_all_midi(&mut self, capture_raw: bool) -> Result<(), MidiError> {
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

        for profile in &self.config.devices {
            let matching_port = available_ports.iter().find(|port| port_matches(port, profile));

            if let Some(port_name) = matching_port {
                let normalized = normalize_port_name(port_name);

                if self.midi_devices.contains_key(&normalized) {
                    log::debug!(
                        "MIDI: Port '{}' already connected, skipping duplicate profile '{}'",
                        normalized, profile.name
                    );
                    continue;
                }

                let deck_target_state = DeckTargetState::from_config(&profile.deck_target);
                let shared_state = Arc::new(SharedState::new(deck_target_state));
                let shift_buttons = extract_shift_buttons(profile);
                let toggle_controls = extract_toggle_controls(profile);
                let mapping_engine = Arc::new(MappingEngine::new(profile, shared_state.clone()));

                match MidiInputHandler::connect_with_raw_events(
                    port_name,
                    self.message_tx.clone(),
                    mapping_engine,
                    shared_state.clone(),
                    shift_buttons,
                    toggle_controls,
                    capture_raw,
                ) {
                    Ok(input_handler) => {
                        log::info!("MIDI: Connected '{}' to port '{}'", profile.name, port_name);

                        let output_handler =
                            MidiConnection::connect_output(port_name).map(|out_conn| {
                                log::info!("MIDI: Output connected for '{}'", profile.name);
                                MidiOutputHandler::new(out_conn, profile)
                            });

                        self.midi_devices.insert(
                            normalized.clone(),
                            ConnectedMidiDevice {
                                profile: profile.clone(),
                                input_handler,
                                output_handler,
                                shared_state,
                            },
                        );
                    }
                    Err(e) => {
                        log::debug!(
                            "MIDI: Failed to connect '{}' to port '{}': {}",
                            profile.name, port_name, e
                        );
                    }
                }
            } else {
                log::debug!(
                    "MIDI: No matching port for profile '{}' (port_match: '{}', learned: {:?})",
                    profile.name, profile.port_match, profile.learned_port_name
                );
            }
        }

        if self.midi_devices.is_empty() {
            if !available_ports.is_empty() {
                log::info!("MIDI: No matching devices found. Available ports:");
                for port in &available_ports {
                    log::info!("  - {}", port);
                }
            }
        } else {
            log::info!("MIDI: Connected to {} device(s)", self.midi_devices.len());
        }

        Ok(())
    }

    // ─── HID connection methods ────────────────────────────────────────

    /// Try to connect to all known HID devices
    fn try_connect_all_hid(&mut self) {
        let devices = hid::enumerate_devices();
        if devices.is_empty() {
            log::info!("HID: No known devices found");
            return;
        }

        for info in &devices {
            if self.hid_devices.contains_key(&info.path) {
                continue;
            }

            let event_tx = match &self.hid_event_tx {
                Some(tx) => tx.clone(),
                None => continue,
            };

            match hid::connect_device(info, event_tx) {
                Ok((connection, control_descriptors)) => {
                    log::info!("HID: Connected to '{}' at {}", info.product_name, info.path);
                    let output_handler = hid::HidOutputHandler::new(connection.feedback_sender());

                    // Look up matching DeviceProfile by hid_product_match
                    let matched_profile = self.config.devices.iter().find(|p| {
                        p.hid_product_match.as_ref().map_or(false, |pattern| {
                            info.product_name.to_lowercase().contains(&pattern.to_lowercase())
                        })
                    });

                    let (profile, mapping_engine, shared_state) = if let Some(p) = matched_profile {
                        log::info!(
                            "HID: Matched profile '{}' for '{}' ({} mappings)",
                            p.name, info.product_name, p.mappings.len()
                        );
                        let deck_target_state = DeckTargetState::from_config(&p.deck_target);
                        let state = Arc::new(SharedState::new(deck_target_state));
                        let engine = Arc::new(MappingEngine::new(p, state.clone()));
                        (Some(p.clone()), Some(engine), Some(state))
                    } else {
                        log::debug!("HID: No matching profile for '{}'", info.product_name);
                        (None, None, None)
                    };

                    self.hid_devices.insert(
                        info.path.clone(),
                        ConnectedHidDevice {
                            info: info.clone(),
                            _connection: connection,
                            output_handler,
                            control_descriptors,
                            profile,
                            mapping_engine,
                            shared_state,
                        },
                    );
                }
                Err(e) => {
                    log::debug!("HID: Failed to connect to '{}': {}", info.product_name, e);
                }
            }
        }

        if !self.hid_devices.is_empty() {
            log::info!("HID: Connected to {} device(s)", self.hid_devices.len());
        }
    }

    /// Connect to all known HID devices (for learn mode)
    fn connect_all_hid_for_learn(&mut self) {
        let devices = hid::enumerate_devices();
        if devices.is_empty() {
            log::info!("HID Learn: No known HID devices found");
            return;
        }

        for info in &devices {
            let event_tx = match &self.hid_event_tx {
                Some(tx) => tx.clone(),
                None => continue,
            };

            match hid::connect_device(info, event_tx) {
                Ok((connection, control_descriptors)) => {
                    log::info!("HID Learn: Connected to '{}' at {}", info.product_name, info.path);
                    let output_handler = hid::HidOutputHandler::new(connection.feedback_sender());
                    self.hid_devices.insert(
                        info.path.clone(),
                        ConnectedHidDevice {
                            info: info.clone(),
                            _connection: connection,
                            output_handler,
                            control_descriptors,
                            profile: None,
                            mapping_engine: None,
                            shared_state: None,
                        },
                    );
                }
                Err(e) => {
                    log::debug!("HID Learn: Failed to connect to '{}': {}", info.product_name, e);
                }
            }
        }

        if !self.hid_devices.is_empty() {
            log::info!("HID Learn: Connected to {} device(s)", self.hid_devices.len());
        }
    }

    // ─── Public API ────────────────────────────────────────────────────

    /// Check if any device is connected (MIDI or HID)
    pub fn is_connected(&self) -> bool {
        !self.midi_devices.is_empty() || !self.hid_devices.is_empty()
    }

    /// Get the number of connected devices (MIDI + HID)
    pub fn connected_count(&self) -> usize {
        self.midi_devices.len() + self.hid_devices.len()
    }

    /// Get the names of connected devices
    pub fn connected_device_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.midi_devices
            .values()
            .map(|d| d.profile.name.as_str())
            .collect();
        names.extend(self.hid_devices.values().map(|d| d.info.product_name.as_str()));
        names
    }

    /// Get the first connected port/device name (for learn mode)
    pub fn first_connected_port(&self) -> Option<&str> {
        self.midi_devices
            .keys()
            .next()
            .map(|s| s.as_str())
            .or_else(|| self.hid_devices.values().next().map(|d| d.info.product_name.as_str()))
    }

    /// Get the message receiver for manual polling
    pub fn message_receiver(&self) -> Receiver<MidiMessage> {
        self.message_rx.clone()
    }

    /// Try to receive a pending message (non-blocking)
    pub fn try_recv(&self) -> Option<MidiMessage> {
        self.message_rx.try_recv().ok()
    }

    /// Drain all pending messages (MIDI + HID mapped events)
    ///
    /// MIDI messages arrive pre-processed from the input callback.
    /// HID events are processed here: shift/layer detection → mapping engine.
    pub fn drain(&self) -> Vec<MidiMessage> {
        let mut messages = Vec::new();

        // Drain MIDI messages (already processed by callback)
        while let Ok(msg) = self.message_rx.try_recv() {
            messages.push(msg);
        }

        // Process HID events: shift/layer detection + mapping engine
        // Only consume from the channel if at least one HID device has a profile.
        // In learn mode, devices have no profiles — events must stay in the channel
        // for drain_hid_events() to pick up.
        let has_hid_mappings = self.hid_devices.values().any(|d| d.profile.is_some());
        if has_hid_mappings {
            if let Some(ref rx) = self.hid_event_rx {
                let events: Vec<ControlEvent> = rx.try_iter().collect();
                'event_loop: for event in events {
                    // Check each HID device for shift/toggle/mapping
                    for device in self.hid_devices.values() {
                        let profile = match &device.profile {
                            Some(p) => p,
                            None => continue,
                        };
                        let shared_state = match &device.shared_state {
                            Some(s) => s,
                            None => continue,
                        };

                        // Check for shift buttons
                        for sb in &profile.shift_buttons {
                            if sb.control == event.address {
                                let held = event.value.is_press();
                                shared_state.set_shift_for_deck(sb.physical_deck, held);
                                log::debug!(
                                    "[HID] -> Shift {} (physical deck {})",
                                    if held { "pressed" } else { "released" },
                                    sb.physical_deck
                                );
                                messages.push(MidiMessage::ShiftChanged {
                                    held,
                                    physical_deck: sb.physical_deck,
                                });
                                continue 'event_loop;
                            }
                        }

                        // Check for layer toggle buttons
                        let toggle_controls = extract_toggle_controls(profile);
                        for (toggle_addr, physical_deck) in &toggle_controls {
                            if *toggle_addr == event.address {
                                if event.value.is_press() {
                                    shared_state.toggle_layer(*physical_deck);
                                    log::debug!(
                                        "[HID] -> Layer toggle (physical deck {})",
                                        physical_deck
                                    );
                                    messages.push(MidiMessage::LayerToggle {
                                        physical_deck: *physical_deck,
                                    });
                                }
                                continue 'event_loop;
                            }
                        }

                        // Not a shift/toggle — route through mapping engine
                        if let Some(ref engine) = device.mapping_engine {
                            if let Some(msg) = engine.map_event(&event) {
                                messages.push(msg);
                                continue 'event_loop;
                            }
                        }
                    }
                }
            }
        }

        messages
    }

    /// Get the shared state from the first device with one (MIDI or HID)
    fn first_shared_state(&self) -> Option<&Arc<SharedState>> {
        self.midi_devices
            .values()
            .next()
            .map(|d| &d.shared_state)
            .or_else(|| {
                self.hid_devices
                    .values()
                    .find_map(|d| d.shared_state.as_ref())
            })
    }

    /// Get the current virtual deck for a physical deck
    pub fn resolve_deck(&self, physical_deck: usize) -> usize {
        self.first_shared_state()
            .map(|s| s.resolve_deck(physical_deck))
            .unwrap_or(physical_deck)
    }

    /// Get current layer selection for a physical deck
    pub fn get_layer(&self, physical_deck: usize) -> LayerSelection {
        self.first_shared_state()
            .map(|s| s.get_layer(physical_deck))
            .unwrap_or(LayerSelection::A)
    }

    /// Check if we're in layer mode
    pub fn is_layer_mode(&self) -> bool {
        self.first_shared_state()
            .map(|s| s.is_layer_mode())
            .unwrap_or(false)
    }

    /// Update LED feedback based on current application state
    pub fn update_feedback(&mut self, state: &FeedbackState) {
        // Update MIDI devices
        for device in self.midi_devices.values_mut() {
            if let Some(ref mut output) = device.output_handler {
                if let Ok(deck_target) = device.shared_state.deck_target.read() {
                    output.update(state, &deck_target);
                }
            }
        }

        // Update HID devices — evaluate feedback for each device with a profile
        for device in self.hid_devices.values_mut() {
            if let Some(ref profile) = device.profile {
                if !profile.feedback.is_empty() {
                    let deck_target = device.shared_state.as_ref()
                        .and_then(|s| s.deck_target.read().ok().map(|dt| dt.clone()))
                        .unwrap_or_default();
                    let results = evaluate_feedback(&profile.feedback, state, &deck_target);
                    device.output_handler.apply_feedback(&results);
                }
            }
        }
    }

    /// Get the pad mode source from the first connected device
    pub fn pad_mode_source(&self) -> PadModeSource {
        self.midi_devices
            .values()
            .next()
            .map(|d| d.profile.pad_mode_source)
            .unwrap_or_default()
    }

    /// Drain all pending raw MIDI events from all devices (for learn mode)
    pub fn drain_raw_events(&self) -> impl Iterator<Item = MidiInputEvent> + '_ {
        self.midi_devices.values().flat_map(|device| {
            std::iter::from_fn({
                let handler = &device.input_handler;
                move || handler.drain_raw_events().next()
            })
        })
    }

    /// Drain all pending raw MIDI events with their source device (for learn mode)
    pub fn drain_raw_events_with_source(&self) -> Vec<(MidiInputEvent, String)> {
        let mut events = Vec::new();
        for (port_name, device) in &self.midi_devices {
            for event in device.input_handler.drain_raw_events() {
                events.push((event, port_name.clone()));
            }
        }
        events
    }

    /// Drain all pending HID control events (for learn mode)
    pub fn drain_hid_events(&self) -> Vec<ControlEvent> {
        match &self.hid_event_rx {
            Some(rx) => {
                let mut events = Vec::new();
                while let Ok(event) = rx.try_recv() {
                    log::info!("[HID Learn] Event: {:?} = {:?}", event.address, event.value);
                    events.push(event);
                }
                events
            }
            None => Vec::new(),
        }
    }

    /// Get descriptors for all controls on connected HID devices
    ///
    /// Returns control descriptors from all connected HID device drivers.
    /// These are collected at connect time (before the driver moves to the I/O thread).
    pub fn hid_control_descriptors(&self) -> Vec<&ControlDescriptor> {
        self.hid_devices
            .values()
            .flat_map(|d| d.control_descriptors.iter())
            .collect()
    }

    /// Look up a control descriptor by its address
    ///
    /// Searches across all connected HID devices for a matching descriptor.
    /// Used by learn mode to get the hardware type for HID controls.
    pub fn hid_descriptor_for(&self, address: &ControlAddress) -> Option<&ControlDescriptor> {
        self.hid_devices
            .values()
            .flat_map(|d| d.control_descriptors.iter())
            .find(|desc| desc.address == *address)
    }

    /// Get the first connected HID device name (if any)
    pub fn first_hid_device_name(&self) -> Option<&str> {
        self.hid_devices.values().next().map(|d| d.info.product_name.as_str())
    }
}
