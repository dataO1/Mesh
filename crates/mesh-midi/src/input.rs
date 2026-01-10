//! MIDI input handling
//!
//! Receives raw MIDI bytes from midir callback, parses them with midly,
//! and sends processed messages to the iced app via flume channel.

use crate::config::MidiControlConfig;
use crate::mapping::MappingEngine;
use crate::messages::MidiMessage;
use crate::MidiConnectionError;
use flume::Sender;
use midir::MidiInputConnection;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Raw MIDI input event (before action mapping)
#[derive(Debug, Clone, Copy)]
pub enum MidiInputEvent {
    /// Note On message
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// Note Off message
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// Control Change message
    ControlChange { channel: u8, cc: u8, value: u8 },
}

impl MidiInputEvent {
    /// Parse raw MIDI bytes into an event
    ///
    /// MIDI message format:
    /// - Note Off: 0x8n nn vv (n=channel, nn=note, vv=velocity)
    /// - Note On: 0x9n nn vv
    /// - Control Change: 0xBn cc vv (cc=controller, vv=value)
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.is_empty() {
            return None;
        }

        let status = data[0];
        let channel = status & 0x0F;
        let message_type = status & 0xF0;

        match message_type {
            0x80 if data.len() >= 3 => Some(Self::NoteOff {
                channel,
                note: data[1],
                velocity: data[2],
            }),
            0x90 if data.len() >= 3 => {
                // Note On with velocity 0 is treated as Note Off
                if data[2] == 0 {
                    Some(Self::NoteOff {
                        channel,
                        note: data[1],
                        velocity: 0,
                    })
                } else {
                    Some(Self::NoteOn {
                        channel,
                        note: data[1],
                        velocity: data[2],
                    })
                }
            }
            0xB0 if data.len() >= 3 => Some(Self::ControlChange {
                channel,
                cc: data[1],
                value: data[2],
            }),
            _ => None, // Ignore other message types (pitch bend, aftertouch, etc.)
        }
    }

    /// Get the MIDI channel
    pub fn channel(&self) -> u8 {
        match self {
            Self::NoteOn { channel, .. } => *channel,
            Self::NoteOff { channel, .. } => *channel,
            Self::ControlChange { channel, .. } => *channel,
        }
    }

    /// Check if this event matches a control config
    pub fn matches(&self, control: &MidiControlConfig) -> bool {
        match (self, control) {
            (
                Self::NoteOn { channel, note, .. } | Self::NoteOff { channel, note, .. },
                MidiControlConfig::Note {
                    channel: cc,
                    note: cn,
                },
            ) => channel == cc && note == cn,
            (
                Self::ControlChange { channel, cc, .. },
                MidiControlConfig::ControlChange {
                    channel: ctrl_ch,
                    cc: ctrl_cc,
                },
            ) => channel == ctrl_ch && cc == ctrl_cc,
            _ => false,
        }
    }

    /// Check if this is a "press" event (Note On or CC > threshold)
    pub fn is_press(&self) -> bool {
        match self {
            Self::NoteOn { velocity, .. } => *velocity > 0,
            Self::ControlChange { value, .. } => *value > 63,
            Self::NoteOff { .. } => false,
        }
    }

    /// Check if this is a "release" event (Note Off or CC < threshold)
    pub fn is_release(&self) -> bool {
        match self {
            Self::NoteOff { .. } => true,
            Self::NoteOn { velocity, .. } => *velocity == 0,
            Self::ControlChange { value, .. } => *value < 64,
        }
    }

    /// Get the value (velocity for notes, value for CC)
    pub fn value(&self) -> u8 {
        match self {
            Self::NoteOn { velocity, .. } => *velocity,
            Self::NoteOff { velocity, .. } => *velocity,
            Self::ControlChange { value, .. } => *value,
        }
    }
}

/// Callback data passed to midir
struct CallbackData {
    message_tx: Sender<MidiMessage>,
    /// Optional raw event sender for learn mode
    raw_event_tx: Option<Sender<MidiInputEvent>>,
    mapping_engine: Arc<MappingEngine>,
    shift_control: Option<MidiControlConfig>,
    shift_held: Arc<AtomicBool>,
}

/// MIDI input handler
///
/// Owns the midir connection and processes incoming MIDI messages.
pub struct MidiInputHandler {
    /// The midir connection (kept alive for the duration)
    _connection: MidiInputConnection<CallbackData>,
    /// Shift state (shared with callback)
    shift_held: Arc<AtomicBool>,
    /// Raw event receiver for learn mode
    raw_event_rx: Option<flume::Receiver<MidiInputEvent>>,
}

impl MidiInputHandler {
    /// Connect to a MIDI port with our callback
    ///
    /// This is the preferred way to create a MidiInputHandler.
    pub fn connect(
        port_match: &str,
        message_tx: Sender<MidiMessage>,
        mapping_engine: Arc<MappingEngine>,
        shift_control: Option<MidiControlConfig>,
    ) -> Result<Self, MidiConnectionError> {
        Self::connect_with_raw_events(port_match, message_tx, mapping_engine, shift_control, false)
    }

    /// Connect to a MIDI port with optional raw event capture
    ///
    /// When `capture_raw` is true, raw MIDI events are also sent to a separate channel
    /// that can be read via `drain_raw_events()`. This is used for MIDI learn mode.
    pub fn connect_with_raw_events(
        port_match: &str,
        message_tx: Sender<MidiMessage>,
        mapping_engine: Arc<MappingEngine>,
        shift_control: Option<MidiControlConfig>,
        capture_raw: bool,
    ) -> Result<Self, MidiConnectionError> {
        let (midi_in, port) = crate::connection::MidiConnection::find_input_port(port_match)?;

        let shift_held = Arc::new(AtomicBool::new(false));

        // Create raw event channel if requested
        let (raw_event_tx, raw_event_rx) = if capture_raw {
            let (tx, rx) = flume::bounded(256);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };

        let callback_data = CallbackData {
            message_tx,
            raw_event_tx,
            mapping_engine,
            shift_control,
            shift_held: shift_held.clone(),
        };

        let connection = midi_in
            .connect(
                &port,
                "mesh-midi-input",
                Self::midi_callback,
                callback_data,
            )
            .map_err(|e| MidiConnectionError::ConnectionError(e.to_string()))?;

        log::info!("MIDI: Input handler connected (raw capture: {})", capture_raw);

        Ok(Self {
            _connection: connection,
            shift_held,
            raw_event_rx,
        })
    }

    /// The midir callback function
    ///
    /// Called from the MIDI driver thread whenever a message is received.
    /// Must be fast and non-blocking.
    fn midi_callback(_timestamp: u64, data: &[u8], callback_data: &mut CallbackData) {
        // Parse raw MIDI
        let event = match MidiInputEvent::parse(data) {
            Some(e) => e,
            None => return,
        };

        // Log incoming MIDI event
        log::debug!("[MIDI IN] {:?}", event);

        // Send raw event for learn mode (if channel is set up)
        if let Some(ref raw_tx) = callback_data.raw_event_tx {
            let _ = raw_tx.try_send(event);
        }

        // Check for shift button
        if let Some(ref shift_ctrl) = callback_data.shift_control {
            if event.matches(shift_ctrl) {
                let held = event.is_press();
                callback_data.shift_held.store(held, Ordering::Relaxed);
                log::debug!("[MIDI IN] -> Shift {}", if held { "pressed" } else { "released" });

                // Send shift state change to app
                let _ = callback_data
                    .message_tx
                    .try_send(MidiMessage::ShiftChanged { held });
                return;
            }
        }

        // Map event to action
        let shift = callback_data.shift_held.load(Ordering::Relaxed);
        if let Some(ref message) = callback_data.mapping_engine.map_event(&event, shift) {
            log::debug!("[MIDI IN] -> {:?}", message);
            // Send to app (non-blocking)
            if callback_data.message_tx.try_send(message.clone()).is_err() {
                log::warn!("MIDI: Message channel full, dropping message");
            }
        } else {
            log::trace!("[MIDI IN] -> (no mapping)");
        }
    }

    /// Check if shift is currently held
    pub fn is_shift_held(&self) -> bool {
        self.shift_held.load(Ordering::Relaxed)
    }

    /// Drain all pending raw MIDI events (for learn mode)
    ///
    /// Returns an iterator over raw events. Only available if connected
    /// with `capture_raw: true`.
    pub fn drain_raw_events(&self) -> impl Iterator<Item = MidiInputEvent> + '_ {
        std::iter::from_fn(|| {
            self.raw_event_rx
                .as_ref()
                .and_then(|rx| rx.try_recv().ok())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_note_on() {
        let data = [0x90, 0x3C, 0x7F]; // Note On, channel 0, note 60, velocity 127
        let event = MidiInputEvent::parse(&data).unwrap();
        match event {
            MidiInputEvent::NoteOn {
                channel,
                note,
                velocity,
            } => {
                assert_eq!(channel, 0);
                assert_eq!(note, 0x3C);
                assert_eq!(velocity, 0x7F);
            }
            _ => panic!("Expected NoteOn"),
        }
    }

    #[test]
    fn test_parse_note_off() {
        let data = [0x80, 0x3C, 0x40]; // Note Off, channel 0, note 60, velocity 64
        let event = MidiInputEvent::parse(&data).unwrap();
        match event {
            MidiInputEvent::NoteOff {
                channel,
                note,
                velocity,
            } => {
                assert_eq!(channel, 0);
                assert_eq!(note, 0x3C);
                assert_eq!(velocity, 0x40);
            }
            _ => panic!("Expected NoteOff"),
        }
    }

    #[test]
    fn test_parse_note_on_zero_velocity() {
        // Note On with velocity 0 should be treated as Note Off
        let data = [0x91, 0x3C, 0x00]; // Note On, channel 1, note 60, velocity 0
        let event = MidiInputEvent::parse(&data).unwrap();
        match event {
            MidiInputEvent::NoteOff { channel, note, .. } => {
                assert_eq!(channel, 1);
                assert_eq!(note, 0x3C);
            }
            _ => panic!("Expected NoteOff for velocity 0"),
        }
    }

    #[test]
    fn test_parse_cc() {
        let data = [0xB2, 0x07, 0x64]; // CC, channel 2, controller 7, value 100
        let event = MidiInputEvent::parse(&data).unwrap();
        match event {
            MidiInputEvent::ControlChange { channel, cc, value } => {
                assert_eq!(channel, 2);
                assert_eq!(cc, 0x07);
                assert_eq!(value, 0x64);
            }
            _ => panic!("Expected ControlChange"),
        }
    }

    #[test]
    fn test_matches_note() {
        let event = MidiInputEvent::NoteOn {
            channel: 0,
            note: 0x0B,
            velocity: 127,
        };
        let control = MidiControlConfig::Note {
            channel: 0,
            note: 0x0B,
        };
        assert!(event.matches(&control));

        let other_control = MidiControlConfig::Note {
            channel: 0,
            note: 0x0C,
        };
        assert!(!event.matches(&other_control));
    }

    #[test]
    fn test_matches_cc() {
        let event = MidiInputEvent::ControlChange {
            channel: 1,
            cc: 0x13,
            value: 64,
        };
        let control = MidiControlConfig::ControlChange {
            channel: 1,
            cc: 0x13,
        };
        assert!(event.matches(&control));
    }
}
