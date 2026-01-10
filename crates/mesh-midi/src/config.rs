//! MIDI configuration schema and loader
//!
//! Configuration is stored as YAML in the user's config directory.
//! Default location: ~/.config/mesh-player/midi.yaml

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Root MIDI configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiConfig {
    /// Device profiles (matched by port name)
    pub devices: Vec<DeviceProfile>,
}

/// Configuration for a specific MIDI device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    /// Human-readable device name
    pub name: String,

    /// Port name substring to match (case-insensitive)
    pub port_match: String,

    /// Deck targeting configuration
    #[serde(default)]
    pub deck_target: DeckTargetConfig,

    /// How pad button actions are determined (app-driven vs controller-driven)
    #[serde(default)]
    pub pad_mode_source: PadModeSource,

    /// Shift button configuration
    pub shift: Option<MidiControlConfig>,

    /// Control-to-action mappings
    #[serde(default)]
    pub mappings: Vec<ControlMapping>,

    /// LED feedback mappings
    #[serde(default)]
    pub feedback: Vec<FeedbackMapping>,
}

/// Deck targeting mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DeckTargetConfig {
    /// Direct channel-to-deck mapping (for 4-deck controllers)
    Direct {
        /// Map MIDI channel to deck index
        channel_to_deck: HashMap<u8, usize>,
    },

    /// Layer toggle mode (for 2-deck controllers accessing 4 virtual decks)
    Layer {
        /// Toggle button for left physical deck (Deck 1/3)
        toggle_left: MidiControlConfig,
        /// Toggle button for right physical deck (Deck 2/4)
        toggle_right: MidiControlConfig,
        /// Virtual deck indices for Layer A (default layer)
        layer_a: Vec<usize>,
        /// Virtual deck indices for Layer B (toggled layer)
        layer_b: Vec<usize>,
    },
}

impl Default for DeckTargetConfig {
    fn default() -> Self {
        // Default to direct 1:1 mapping
        let mut channel_to_deck = HashMap::new();
        for i in 0..4 {
            channel_to_deck.insert(i, i as usize);
        }
        DeckTargetConfig::Direct { channel_to_deck }
    }
}

/// MIDI control identifier (Note or CC)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type")]
pub enum MidiControlConfig {
    /// Note On/Off message
    Note {
        /// MIDI channel (0-15)
        channel: u8,
        /// Note number (0-127)
        note: u8,
    },
    /// Control Change message
    ControlChange {
        /// MIDI channel (0-15)
        channel: u8,
        /// CC number (0-127)
        cc: u8,
    },
}

impl MidiControlConfig {
    /// Create a Note control
    pub fn note(channel: u8, note: u8) -> Self {
        Self::Note { channel, note }
    }

    /// Create a CC control
    pub fn cc(channel: u8, cc: u8) -> Self {
        Self::ControlChange { channel, cc }
    }

    /// Get the MIDI channel
    pub fn channel(&self) -> u8 {
        match self {
            Self::Note { channel, .. } => *channel,
            Self::ControlChange { channel, .. } => *channel,
        }
    }
}

/// Single control-to-action mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMapping {
    /// MIDI control identifier
    pub control: MidiControlConfig,

    /// Action to trigger (e.g., "deck.play", "mixer.volume")
    pub action: String,

    /// Physical deck index (for layer-resolved controls)
    /// Use this for controls that should follow layer toggle
    pub physical_deck: Option<usize>,

    /// Direct deck index (for non-layer-resolved controls like mixer faders)
    pub deck_index: Option<usize>,

    /// Additional parameters for the action
    #[serde(default)]
    pub params: HashMap<String, serde_yaml::Value>,

    /// Control behavior (momentary, toggle, etc.)
    #[serde(default)]
    pub behavior: ControlBehavior,

    /// Action to trigger when shift is held
    pub shift_action: Option<String>,

    /// Encoder mode for CC controls
    pub encoder_mode: Option<EncoderMode>,

    /// Detected or configured hardware type
    /// Auto-detected during MIDI learn, used for adapter behavior
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_type: Option<HardwareType>,
}

/// Control behavior type
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlBehavior {
    /// Momentary: press triggers action, release triggers release action
    #[default]
    Momentary,
    /// Toggle: each press toggles state
    Toggle,
    /// Continuous: value changes trigger action with value
    Continuous,
}

/// Encoder interpretation mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EncoderMode {
    /// Absolute value (0-127)
    Absolute,
    /// Relative: 1-63 = clockwise, 65-127 = counter-clockwise
    Relative,
    /// Relative with 64 as center: <64 = CCW, >64 = CW
    RelativeSigned,
}

/// Detected or manually specified MIDI hardware control type
///
/// Used during MIDI learn to auto-detect the physical control type and
/// configure appropriate behavior and encoder modes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HardwareType {
    /// Unknown hardware type - needs manual configuration
    #[default]
    Unknown,
    /// Physical button (Note On/Off pairs)
    Button,
    /// Rotary potentiometer/knob (absolute CC 0-127)
    Knob,
    /// Linear fader (absolute CC 0-127, monotonic movement)
    Fader,
    /// High-resolution 14-bit fader (CC pair: MSB + LSB)
    Fader14Bit,
    /// Rotary encoder (relative CC values centered around 64)
    Encoder,
    /// Jog wheel (high-rate relative CC for scratching/nudging)
    JogWheel,
}

impl HardwareType {
    /// Get recommended ControlBehavior for this hardware type
    pub fn default_behavior(&self) -> ControlBehavior {
        match self {
            Self::Button => ControlBehavior::Momentary,
            Self::Knob | Self::Fader | Self::Fader14Bit => ControlBehavior::Continuous,
            Self::Encoder | Self::JogWheel => ControlBehavior::Continuous,
            Self::Unknown => ControlBehavior::Momentary,
        }
    }

    /// Get recommended EncoderMode for CC controls (None for buttons)
    pub fn default_encoder_mode(&self) -> Option<EncoderMode> {
        match self {
            Self::Knob | Self::Fader | Self::Fader14Bit => Some(EncoderMode::Absolute),
            Self::Encoder | Self::JogWheel => Some(EncoderMode::Relative),
            Self::Button | Self::Unknown => None,
        }
    }

    /// Check if this hardware type uses relative CC values
    pub fn is_relative(&self) -> bool {
        matches!(self, Self::Encoder | Self::JogWheel)
    }

    /// Check if this hardware type is continuous (vs discrete button)
    pub fn is_continuous(&self) -> bool {
        !matches!(self, Self::Button | Self::Unknown)
    }
}

/// How pad button actions are determined
///
/// Controllers like DDJ-SB2 have hardware mode switches that change which MIDI notes
/// the pads send. Other controllers use the same notes and rely on software mode switching.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PadModeSource {
    /// App-driven: check app's action_mode to determine what pad presses do
    /// Use this for controllers with unified pads (same MIDI notes in all modes)
    #[default]
    App,
    /// Controller-driven: MIDI notes directly map to actions
    /// Use this for controllers with separate hot cue/slicer buttons (different MIDI notes per mode)
    Controller,
}

/// LED feedback mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackMapping {
    /// State to monitor (e.g., "deck.is_playing", "deck.hot_cue_set")
    pub state: String,

    /// Physical deck index (for layer-resolved feedback)
    pub physical_deck: Option<usize>,

    /// Direct deck index (for non-layer-resolved feedback)
    pub deck_index: Option<usize>,

    /// Additional parameters (e.g., hot cue slot)
    #[serde(default)]
    pub params: HashMap<String, serde_yaml::Value>,

    /// MIDI output control
    pub output: MidiControlConfig,

    /// Value to send when state is true/active
    pub on_value: u8,

    /// Value to send when state is false/inactive
    pub off_value: u8,

    /// For layer indicator LEDs: which layer activates this LED
    pub layer: Option<String>,
}

/// Get the default MIDI config file path
///
/// Returns: ~/.config/mesh-player/midi.yaml
pub fn default_midi_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("mesh-player")
        .join("midi.yaml")
}

/// Load MIDI configuration from a YAML file
///
/// If the file doesn't exist, returns an empty config (no devices).
/// If the file exists but is invalid, logs a warning and returns empty config.
pub fn load_midi_config(path: &Path) -> MidiConfig {
    log::info!("load_midi_config: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_midi_config: Config file doesn't exist, no MIDI mappings");
        return MidiConfig::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<MidiConfig>(&contents) {
            Ok(config) => {
                log::info!(
                    "load_midi_config: Loaded {} device profile(s)",
                    config.devices.len()
                );
                for device in &config.devices {
                    log::info!(
                        "  - {} (port_match: '{}', {} mappings, {} feedback)",
                        device.name,
                        device.port_match,
                        device.mappings.len(),
                        device.feedback.len()
                    );
                }
                config
            }
            Err(e) => {
                log::warn!("load_midi_config: Failed to parse config: {}", e);
                MidiConfig::default()
            }
        },
        Err(e) => {
            log::warn!("load_midi_config: Failed to read config file: {}", e);
            MidiConfig::default()
        }
    }
}

/// Save MIDI configuration to a YAML file
///
/// Creates parent directories if they don't exist.
pub fn save_midi_config(config: &MidiConfig, path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;

    log::info!("save_midi_config: Saving to {:?}", path);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }

    // Serialize to YAML
    let yaml = serde_yaml::to_string(config).context("Failed to serialize MIDI config to YAML")?;

    // Write to file
    std::fs::write(path, yaml)
        .with_context(|| format!("Failed to write MIDI config file: {:?}", path))?;

    log::info!("save_midi_config: Config saved successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MidiConfig::default();
        assert!(config.devices.is_empty());
    }

    #[test]
    fn test_midi_control_config() {
        let note = MidiControlConfig::note(0, 0x0B);
        assert_eq!(note.channel(), 0);

        let cc = MidiControlConfig::cc(1, 0x13);
        assert_eq!(cc.channel(), 1);
    }

    #[test]
    fn test_yaml_parsing() {
        let yaml = r#"
devices:
  - name: "Test Controller"
    port_match: "Test"
    deck_target:
      type: "Layer"
      toggle_left:
        type: "Note"
        channel: 0
        note: 0x72
      toggle_right:
        type: "Note"
        channel: 1
        note: 0x72
      layer_a: [0, 1]
      layer_b: [2, 3]
    shift:
      type: "Note"
      channel: 0
      note: 0x63
    mappings:
      - control:
          type: "Note"
          channel: 0
          note: 0x0B
        action: "deck.play"
        physical_deck: 0
        behavior: momentary
"#;

        let config: MidiConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.devices[0].name, "Test Controller");
        assert_eq!(config.devices[0].mappings.len(), 1);
        assert_eq!(config.devices[0].mappings[0].action, "deck.play");
    }
}
