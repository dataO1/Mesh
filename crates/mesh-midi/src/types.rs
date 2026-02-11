//! Protocol-agnostic control types
//!
//! These types abstract over MIDI and HID (and any future protocol),
//! allowing the mapping engine, feedback system, and learn mode to
//! work identically regardless of the physical controller protocol.

use crate::config::HardwareType;
use serde::{Deserialize, Serialize};

/// Protocol-agnostic control address
///
/// Uniquely identifies a control on any device (MIDI note, MIDI CC, HID named control).
/// Used as the key in mapping lookups and feedback routing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ControlAddress {
    /// MIDI control (Note or CC)
    Midi(MidiAddress),
    /// HID named control (e.g., "grid_1", "fader_2", "encoder")
    Hid { name: String },
}

/// MIDI-specific address (channel + note/CC)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidiAddress {
    /// Note On/Off message
    Note {
        /// MIDI channel (0-15)
        channel: u8,
        /// Note number (0-127)
        note: u8,
    },
    /// Control Change message
    #[serde(rename = "control_change")]
    CC {
        /// MIDI channel (0-15)
        channel: u8,
        /// CC number (0-127)
        cc: u8,
    },
}

/// Abstract input event from any device
///
/// This is the protocol-agnostic representation of a control interaction.
/// MIDI adapters convert raw MIDI bytes into this; HID drivers produce this directly.
#[derive(Clone, Debug)]
pub struct ControlEvent {
    /// Which control was activated
    pub address: ControlAddress,
    /// The value/state of the control
    pub value: ControlValue,
}

/// Abstract control value
///
/// Represents the semantic meaning of a control interaction, not raw protocol bytes.
#[derive(Clone, Debug)]
pub enum ControlValue {
    /// Button pressed or released
    Button(bool),
    /// Absolute position (0.0-1.0 normalized)
    Absolute(f64),
    /// Relative movement (encoder delta: positive = CW, negative = CCW)
    Relative(i32),
}

impl ControlValue {
    /// Check if this is a "press" event
    pub fn is_press(&self) -> bool {
        match self {
            ControlValue::Button(pressed) => *pressed,
            ControlValue::Absolute(v) => *v > 0.5,
            ControlValue::Relative(d) => *d > 0,
        }
    }

    /// Check if this is a "release" event
    pub fn is_release(&self) -> bool {
        match self {
            ControlValue::Button(pressed) => !*pressed,
            ControlValue::Absolute(v) => *v <= 0.5,
            ControlValue::Relative(d) => *d < 0,
        }
    }

    /// Get absolute value (0.0-1.0), converting button to 0/1
    pub fn as_absolute(&self) -> f64 {
        match self {
            ControlValue::Button(true) => 1.0,
            ControlValue::Button(false) => 0.0,
            ControlValue::Absolute(v) => *v,
            ControlValue::Relative(d) => {
                // Not meaningful for relative, but provide something
                if *d > 0 { 1.0 } else if *d < 0 { 0.0 } else { 0.5 }
            }
        }
    }

    /// Get as MIDI-scale u8 (0-127)
    pub fn as_midi_value(&self) -> u8 {
        match self {
            ControlValue::Button(true) => 127,
            ControlValue::Button(false) => 0,
            ControlValue::Absolute(v) => (v.clamp(0.0, 1.0) * 127.0).round() as u8,
            ControlValue::Relative(d) => {
                // Encode as relative MIDI: 1-63 = CW, 65-127 = CCW
                if *d > 0 {
                    (*d).min(63) as u8
                } else if *d < 0 {
                    (128 + *d).max(65) as u8
                } else {
                    0
                }
            }
        }
    }

    /// Get relative delta, converting button to +1/-1
    pub fn as_delta(&self) -> i32 {
        match self {
            ControlValue::Button(true) => 1,
            ControlValue::Button(false) => 0,
            ControlValue::Absolute(_) => 0,
            ControlValue::Relative(d) => *d,
        }
    }
}

/// Abstract feedback command to any device
///
/// Each command targets a specific control by name (matching the HID control name
/// from `ControlDescriptor`). The device driver maps this to the correct byte
/// offset in the output report.
#[derive(Clone, Debug)]
pub enum FeedbackCommand {
    /// Set single-color LED brightness (0-127)
    SetLed { control: String, brightness: u8 },
    /// Set RGB LED color (0-127 per channel)
    SetRgb { control: String, r: u8, g: u8, b: u8 },
    /// Set text display content (7-segment, screen, etc.)
    SetDisplay { text: String },
}

/// Describes a control available on a device
///
/// Used by learn mode to show human-readable names and skip hardware detection
/// for HID devices (where the control type is already known from the driver).
#[derive(Clone, Debug)]
pub struct ControlDescriptor {
    /// Protocol-agnostic address
    pub address: ControlAddress,
    /// Human-readable name: "Grid Pad 1", "Volume Fader 1", etc.
    pub name: String,
    /// Physical control type
    pub control_type: HardwareType,
    /// Whether this control has a single-color LED
    pub has_led: bool,
    /// Whether this control has an RGB LED
    pub has_rgb: bool,
}

// === Conversion between MidiControlConfig and ControlAddress ===

impl From<&crate::config::MidiControlConfig> for ControlAddress {
    fn from(ctrl: &crate::config::MidiControlConfig) -> Self {
        match ctrl {
            crate::config::MidiControlConfig::Note { channel, note } => {
                ControlAddress::Midi(MidiAddress::Note {
                    channel: *channel,
                    note: *note,
                })
            }
            crate::config::MidiControlConfig::ControlChange { channel, cc } => {
                ControlAddress::Midi(MidiAddress::CC {
                    channel: *channel,
                    cc: *cc,
                })
            }
        }
    }
}

impl From<crate::config::MidiControlConfig> for ControlAddress {
    fn from(ctrl: crate::config::MidiControlConfig) -> Self {
        ControlAddress::from(&ctrl)
    }
}

impl ControlAddress {
    /// Try to convert back to MidiControlConfig (returns None for HID addresses)
    pub fn as_midi_control_config(&self) -> Option<crate::config::MidiControlConfig> {
        match self {
            ControlAddress::Midi(MidiAddress::Note { channel, note }) => {
                Some(crate::config::MidiControlConfig::Note {
                    channel: *channel,
                    note: *note,
                })
            }
            ControlAddress::Midi(MidiAddress::CC { channel, cc }) => {
                Some(crate::config::MidiControlConfig::ControlChange {
                    channel: *channel,
                    cc: *cc,
                })
            }
            ControlAddress::Hid { .. } => None,
        }
    }
}

// === Conversion between MidiInputEvent and ControlEvent ===

impl From<&crate::midi::input::MidiInputEvent> for ControlEvent {
    fn from(event: &crate::midi::input::MidiInputEvent) -> Self {
        use crate::midi::input::MidiInputEvent;
        match event {
            MidiInputEvent::NoteOn { channel, note, velocity } => ControlEvent {
                address: ControlAddress::Midi(MidiAddress::Note {
                    channel: *channel,
                    note: *note,
                }),
                value: ControlValue::Button(*velocity > 0),
            },
            MidiInputEvent::NoteOff { channel, note, .. } => ControlEvent {
                address: ControlAddress::Midi(MidiAddress::Note {
                    channel: *channel,
                    note: *note,
                }),
                value: ControlValue::Button(false),
            },
            MidiInputEvent::ControlChange { channel, cc, value } => ControlEvent {
                address: ControlAddress::Midi(MidiAddress::CC {
                    channel: *channel,
                    cc: *cc,
                }),
                value: ControlValue::Absolute(*value as f64 / 127.0),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_value_button() {
        let press = ControlValue::Button(true);
        assert!(press.is_press());
        assert!(!press.is_release());
        assert_eq!(press.as_midi_value(), 127);
        assert_eq!(press.as_absolute(), 1.0);

        let release = ControlValue::Button(false);
        assert!(!release.is_press());
        assert!(release.is_release());
        assert_eq!(release.as_midi_value(), 0);
    }

    #[test]
    fn test_control_value_absolute() {
        let mid = ControlValue::Absolute(0.5);
        assert!(!mid.is_press());
        assert_eq!(mid.as_midi_value(), 64);

        let max = ControlValue::Absolute(1.0);
        assert!(max.is_press());
        assert_eq!(max.as_midi_value(), 127);
    }

    #[test]
    fn test_control_value_relative() {
        let cw = ControlValue::Relative(3);
        assert_eq!(cw.as_delta(), 3);
        assert_eq!(cw.as_midi_value(), 3);

        let ccw = ControlValue::Relative(-2);
        assert_eq!(ccw.as_delta(), -2);
        assert_eq!(ccw.as_midi_value(), 126); // 128 + (-2) = 126
    }

    #[test]
    fn test_control_address_serde() {
        let midi_note = ControlAddress::Midi(MidiAddress::Note { channel: 0, note: 60 });
        let yaml = serde_yaml::to_string(&midi_note).unwrap();
        let parsed: ControlAddress = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, midi_note);

        let hid = ControlAddress::Hid { name: "grid_1".to_string() };
        let yaml = serde_yaml::to_string(&hid).unwrap();
        let parsed: ControlAddress = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, hid);
    }

    #[test]
    fn test_midi_address_serde() {
        let cc = MidiAddress::CC { channel: 1, cc: 7 };
        let yaml = serde_yaml::to_string(&cc).unwrap();
        let parsed: MidiAddress = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, cc);
    }
}
