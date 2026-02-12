//! MIDI port discovery and connection
//!
//! Uses midir for cross-platform MIDI I/O (ALSA on Linux, CoreMIDI on macOS, WinMM on Windows).

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

/// Error type for MIDI connection operations
#[derive(Debug, thiserror::Error)]
pub enum MidiConnectionError {
    #[error("Failed to initialize MIDI input: {0}")]
    InputInitError(String),

    #[error("Failed to initialize MIDI output: {0}")]
    OutputInitError(String),

    #[error("No MIDI input ports available")]
    NoInputPorts,

    #[error("No MIDI port found matching pattern: {0}")]
    PortNotFound(String),

    #[error("Failed to connect to MIDI port: {0}")]
    ConnectionError(String),

    #[error("Failed to get port info: {0}")]
    PortInfoError(String),
}

/// Holds MIDI input and output connections
pub struct MidiConnection {
    /// Input connection (receives MIDI messages)
    pub input: MidiInputConnection<()>,
    /// Output connection (sends MIDI messages for LED feedback)
    pub output: Option<MidiOutputConnection>,
}

impl MidiConnection {
    /// Find and connect to a MIDI device matching the given pattern
    ///
    /// Returns the input connection and optionally the output connection.
    /// The pattern is matched case-insensitively as a substring of port names.
    pub fn find_and_connect(
        port_match: &str,
    ) -> Result<(MidiInputConnection<()>, Option<MidiOutputConnection>), MidiConnectionError> {
        let pattern = port_match.to_lowercase();

        // Create MIDI input
        let midi_in = MidiInput::new("mesh-midi-in")
            .map_err(|e| MidiConnectionError::InputInitError(e.to_string()))?;

        // Find matching input port
        let in_ports = midi_in.ports();
        if in_ports.is_empty() {
            return Err(MidiConnectionError::NoInputPorts);
        }

        let input_port = in_ports
            .iter()
            .find(|port| {
                midi_in
                    .port_name(port)
                    .map(|name| name.to_lowercase().contains(&pattern))
                    .unwrap_or(false)
            })
            .ok_or_else(|| MidiConnectionError::PortNotFound(port_match.to_string()))?;

        let input_port_name = midi_in
            .port_name(input_port)
            .map_err(|e| MidiConnectionError::PortInfoError(e.to_string()))?;

        log::info!("MIDI: Found input port: {}", input_port_name);

        // Connect to input port with dummy callback (will be replaced by MidiInputHandler)
        // Note: We pass a dummy callback here; the actual callback is set in MidiInputHandler
        let input_conn = midi_in
            .connect(
                input_port,
                "mesh-midi-input",
                |_timestamp, _message, _| {},
                (),
            )
            .map_err(|e| MidiConnectionError::ConnectionError(e.to_string()))?;

        // Try to find and connect to matching output port
        let output_conn = Self::try_connect_output(&pattern);

        Ok((input_conn, output_conn))
    }

    /// Find and connect to a MIDI device, returning just the input for callback setup
    ///
    /// This is the preferred method - returns MidiInput so caller can set up callback.
    pub fn find_input_port(
        port_match: &str,
    ) -> Result<(MidiInput, midir::MidiInputPort), MidiConnectionError> {
        let pattern = port_match.to_lowercase();

        let midi_in = MidiInput::new("mesh-midi-in")
            .map_err(|e| MidiConnectionError::InputInitError(e.to_string()))?;

        let in_ports = midi_in.ports();
        if in_ports.is_empty() {
            return Err(MidiConnectionError::NoInputPorts);
        }

        let input_port = in_ports
            .into_iter()
            .find(|port| {
                midi_in
                    .port_name(port)
                    .map(|name| name.to_lowercase().contains(&pattern))
                    .unwrap_or(false)
            })
            .ok_or_else(|| MidiConnectionError::PortNotFound(port_match.to_string()))?;

        let port_name = midi_in
            .port_name(&input_port)
            .map_err(|e| MidiConnectionError::PortInfoError(e.to_string()))?;

        log::info!("MIDI: Found input port: {}", port_name);

        Ok((midi_in, input_port))
    }

    /// Try to connect to a matching MIDI output port
    fn try_connect_output(pattern: &str) -> Option<MidiOutputConnection> {
        let midi_out = match MidiOutput::new("mesh-midi-out") {
            Ok(out) => out,
            Err(e) => {
                log::warn!("MIDI: Failed to initialize output: {}", e);
                return None;
            }
        };

        let out_ports = midi_out.ports();

        let output_port = out_ports.iter().find(|port| {
            midi_out
                .port_name(port)
                .map(|name| name.to_lowercase().contains(pattern))
                .unwrap_or(false)
        })?;

        let port_name = midi_out.port_name(output_port).ok()?;
        log::info!("MIDI: Found output port: {}", port_name);

        match midi_out.connect(output_port, "mesh-midi-output") {
            Ok(conn) => {
                log::info!("MIDI: Connected to output port");
                Some(conn)
            }
            Err(e) => {
                log::warn!("MIDI: Failed to connect to output: {}", e);
                None
            }
        }
    }

    /// Find and connect to output port only
    pub fn connect_output(port_match: &str) -> Option<MidiOutputConnection> {
        Self::try_connect_output(&port_match.to_lowercase())
    }

    /// List all available MIDI input ports
    pub fn list_input_ports() -> Result<Vec<String>, MidiConnectionError> {
        let midi_in = MidiInput::new("mesh-midi-list")
            .map_err(|e| MidiConnectionError::InputInitError(e.to_string()))?;

        let ports: Vec<String> = midi_in
            .ports()
            .iter()
            .filter_map(|port| midi_in.port_name(port).ok())
            .collect();

        Ok(ports)
    }

    /// List all available MIDI output ports
    pub fn list_output_ports() -> Result<Vec<String>, MidiConnectionError> {
        let midi_out = MidiOutput::new("mesh-midi-list")
            .map_err(|e| MidiConnectionError::OutputInitError(e.to_string()))?;

        let ports: Vec<String> = midi_out
            .ports()
            .iter()
            .filter_map(|port| midi_out.port_name(port).ok())
            .collect();

        Ok(ports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_ports() {
        // This test just verifies we can enumerate ports without crashing
        // Actual port availability depends on the system
        let _input_ports = MidiConnection::list_input_ports();
        let _output_ports = MidiConnection::list_output_ports();
    }
}
