//! MIDI protocol backend
//!
//! Handles MIDI device connection, input parsing, and LED output via midir.
//! This is one of potentially several protocol backends (alongside HID).

pub mod connection;
pub mod input;
pub mod output;

pub use connection::{MidiConnection, MidiConnectionError};
pub use input::{MidiInputEvent, MidiInputHandler};
pub use output::MidiOutputHandler;
