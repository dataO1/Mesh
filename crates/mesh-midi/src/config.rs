//! MIDI configuration schema and loader
//!
//! Configuration is stored as YAML in the mesh collection folder.
//! Default location: ~/Music/mesh-collection/midi.yaml

use crate::types::ControlAddress;
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

/// Configuration for a specific controller device (MIDI, HID, or mixed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    /// Human-readable device name
    pub name: String,

    /// Port name substring to match (case-insensitive)
    /// Used as fallback when learned_port_name doesn't match
    pub port_match: String,

    /// Exact port name captured during MIDI learn (normalized, without hardware ID)
    /// e.g., "DDJ-SB2 MIDI 1" - used for precise matching before falling back to port_match
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_port_name: Option<String>,

    /// Device type identifier for HID devices (e.g., "kontrol_f1")
    /// Used to select the correct HID driver at connection time
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,

    /// HID product name substring to match (case-insensitive)
    /// Matched against USB product descriptor for HID device association
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid_product_match: Option<String>,

    /// USB serial number for exact HID device matching
    /// Distinguishes multiple identical devices (e.g., two Kontrol F1s)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hid_device_id: Option<String>,

    /// Deck targeting configuration
    #[serde(default)]
    pub deck_target: DeckTargetConfig,

    /// How pad button actions are determined (app-driven vs controller-driven)
    #[serde(default)]
    pub pad_mode_source: PadModeSource,

    /// Per-physical-deck shift button configurations
    #[serde(default)]
    pub shift_buttons: Vec<ShiftButtonConfig>,

    /// Control-to-action mappings
    #[serde(default)]
    pub mappings: Vec<ControlMapping>,

    /// LED feedback mappings
    #[serde(default)]
    pub feedback: Vec<FeedbackMapping>,

    /// Note-offset LED color mode (e.g., Xone K series: red=+0, amber=+36, green=+72).
    /// When set, the MIDI output handler adds color offsets to note numbers instead of
    /// using velocity for brightness. LEDs become binary on/off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_note_offsets: Option<ColorNoteOffsets>,
}

/// Note-offset LED color configuration for controllers that use note number
/// offsets to select pre-configured LED colors.
///
/// On the Xone K series, each button has up to 3 layers (note offsets +0, +36, +72).
/// Each layer's color is set in the Xone Controller Editor from a 16-color RGB palette.
/// MIDI can only turn layers on/off — the actual displayed color depends on the
/// editor configuration. The K2 has fixed red/amber/green LEDs; the K3 has full RGB
/// LEDs where any palette color can be assigned per layer.
///
/// The offset values here correspond to the layer note numbers. Software chooses which
/// layer to activate based on the desired state, and the user configures matching colors
/// in the Xone Controller Editor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorNoteOffsets {
    /// Note offset for layer 1 (default: red on K2, configurable on K3)
    pub red: u8,
    /// Note offset for layer 2 (default: amber on K2, configurable on K3)
    pub amber: u8,
    /// Note offset for layer 3 (default: green on K2, configurable on K3)
    pub green: u8,
}

/// Known controller LED color modes.
///
/// Maps controller name substrings (lowercase) to their note-offset configurations.
/// Checked in order; first match wins.
const KNOWN_LED_COLOR_MODES: &[(&str, ColorNoteOffsets)] = &[
    // Allen & Heath Xone K series — 3 layers via note offsets
    ("xone", ColorNoteOffsets { red: 0, amber: 36, green: 72 }),
];

/// Auto-detect note-offset LED color mode from a controller/port name.
///
/// Checks against a built-in table of known controllers. Returns `None` for
/// standard velocity-mode controllers.
pub fn detect_color_note_offsets(name: &str) -> Option<ColorNoteOffsets> {
    let lower = name.to_lowercase();
    KNOWN_LED_COLOR_MODES
        .iter()
        .find(|(pattern, _)| lower.contains(pattern))
        .map(|(_, offsets)| offsets.clone())
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
        toggle_left: ControlAddress,
        /// Toggle button for right physical deck (Deck 2/4)
        toggle_right: ControlAddress,
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

/// Per-physical-deck shift button configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShiftButtonConfig {
    /// Control address for this shift button (MIDI or HID)
    pub control: ControlAddress,
    /// Which physical deck this shift button belongs to (0 = left, 1 = right)
    pub physical_deck: usize,
}

/// Single control-to-action mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMapping {
    /// Control address (MIDI note/CC or HID named control)
    pub control: ControlAddress,

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

    /// Output control address (MIDI note/CC or HID named control)
    pub output: ControlAddress,

    /// Value to send when state is true/active (MIDI velocity / LED brightness)
    pub on_value: u8,

    /// Value to send when state is false/inactive
    pub off_value: u8,

    /// Alternative on_value for Layer B (e.g., different LED color)
    /// When set and state is "deck.layer_active", Layer A uses on_value, Layer B uses alt_on_value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_on_value: Option<u8>,

    /// RGB color when state is active (for HID devices with RGB LEDs)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_color: Option<[u8; 3]>,

    /// RGB color when state is inactive
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_color: Option<[u8; 3]>,

    /// RGB color for Layer B active state (alternative to on_color)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt_on_color: Option<[u8; 3]>,
}

/// Get the default MIDI config file path
///
/// Returns: ~/Music/mesh-collection/midi.yaml
pub fn default_midi_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
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

/// Normalize a MIDI port name by removing hardware-specific identifiers
///
/// ALSA port names include dynamic IDs that change between systems/reconnections:
///
/// 1. Bracketed hardware IDs: `[hw:3,0,0]`
/// 2. ALSA sequencer client:port IDs: trailing `28:0` or `20:0`
///
/// Examples:
/// - "DDJ-SB2 MIDI 1 [hw:3,0,0]" -> "DDJ-SB2 MIDI 1"
/// - "DDJ-SB2:DDJ-SB2 MIDI 1 28:0" -> "DDJ-SB2:DDJ-SB2 MIDI 1"
/// - "Launchpad Mini MK3 [hw:1,0,0]" -> "Launchpad Mini MK3"
///
/// This allows matching devices by their stable name portion.
pub fn normalize_port_name(name: &str) -> String {
    let mut result = name.trim();

    // Remove bracketed hardware ID suffix (e.g., "[hw:3,0,0]")
    if let Some(bracket_pos) = result.rfind('[') {
        result = result[..bracket_pos].trim();
    }

    // Remove trailing ALSA sequencer client:port ID (e.g., "28:0" or "20:0")
    // Pattern: space followed by digits, colon, digits at end of string
    if let Some(last_space) = result.rfind(' ') {
        let suffix = &result[last_space + 1..];
        // Check if suffix matches pattern: digits:digits
        if suffix.contains(':') {
            let parts: Vec<&str> = suffix.split(':').collect();
            if parts.len() == 2
                && parts[0].chars().all(|c| c.is_ascii_digit())
                && parts[1].chars().all(|c| c.is_ascii_digit())
            {
                result = result[..last_space].trim();
            }
        }
    }

    result.to_string()
}

/// Check if a port name matches a learned port name or port_match pattern
///
/// First tries exact match against normalized port name, then falls back
/// to case-insensitive substring match against port_match.
/// Both sides are normalized to handle hardware ID differences.
pub fn port_matches(actual_port: &str, profile: &DeviceProfile) -> bool {
    let normalized_actual = normalize_port_name(actual_port);

    // First: try exact match against learned_port_name (if set)
    // Normalize both sides to handle config files with old hardware IDs
    if let Some(ref learned) = profile.learned_port_name {
        let normalized_learned = normalize_port_name(learned);
        if normalized_actual.eq_ignore_ascii_case(&normalized_learned) {
            return true;
        }
    }

    // Fallback: substring match against port_match (also normalize it)
    let normalized_port_match = normalize_port_name(&profile.port_match);
    normalized_actual
        .to_lowercase()
        .contains(&normalized_port_match.to_lowercase())
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
        protocol: "midi"
        type: "note"
        channel: 0
        note: 0x72
      toggle_right:
        protocol: "midi"
        type: "note"
        channel: 1
        note: 0x72
      layer_a: [0, 1]
      layer_b: [2, 3]
    shift_buttons:
      - control:
          protocol: "midi"
          type: "note"
          channel: 0
          note: 0x63
        physical_deck: 0
    mappings:
      - control:
          protocol: "midi"
          type: "note"
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

    #[test]
    fn test_yaml_parsing_hid() {
        let yaml = r#"
devices:
  - name: "Kontrol F1"
    port_match: ""
    device_type: "kontrol_f1"
    hid_product_match: "Kontrol F1"
    mappings:
      - control:
          protocol: "hid"
          name: "grid_1"
        action: "deck.pad_press"
        physical_deck: 0
        behavior: momentary
        params:
          pad: 0
    feedback:
      - state: "deck.hot_cue_set"
        physical_deck: 0
        params:
          slot: 0
        output:
          protocol: "hid"
          name: "grid_1"
        on_value: 127
        off_value: 0
        on_color: [127, 0, 0]
        off_color: [0, 0, 0]
"#;

        let config: MidiConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.devices[0].device_type, Some("kontrol_f1".to_string()));
        assert_eq!(config.devices[0].mappings[0].action, "deck.pad_press");
        assert_eq!(config.devices[0].feedback[0].on_color, Some([127, 0, 0]));
    }

    #[test]
    fn test_normalize_port_name() {
        use super::normalize_port_name;

        // With hardware ID in brackets
        assert_eq!(
            normalize_port_name("DDJ-SB2 MIDI 1 [hw:3,0,0]"),
            "DDJ-SB2 MIDI 1"
        );
        assert_eq!(
            normalize_port_name("Launchpad Mini MK3 [hw:1,0,0]"),
            "Launchpad Mini MK3"
        );

        // ALSA sequencer client:port ID format (e.g., "28:0" or "20:0")
        assert_eq!(
            normalize_port_name("DDJ-SB2:DDJ-SB2 MIDI 1 28:0"),
            "DDJ-SB2:DDJ-SB2 MIDI 1"
        );
        assert_eq!(
            normalize_port_name("DDJ-SB2:DDJ-SB2 MIDI 1 20:0"),
            "DDJ-SB2:DDJ-SB2 MIDI 1"
        );
        assert_eq!(
            normalize_port_name("Midi Through:Midi Through Port-0 14:0"),
            "Midi Through:Midi Through Port-0"
        );

        // Without hardware ID (no change except trim)
        assert_eq!(normalize_port_name("DDJ-SB2 MIDI 1"), "DDJ-SB2 MIDI 1");
        assert_eq!(normalize_port_name("  Padded Name  "), "Padded Name");

        // Edge cases
        assert_eq!(normalize_port_name(""), "");
        assert_eq!(normalize_port_name("[only brackets]"), "");
    }

    #[test]
    fn test_port_matches() {
        use super::port_matches;

        let profile_with_learned = DeviceProfile {
            name: "My SB2".to_string(),
            port_match: "sb2".to_string(),
            learned_port_name: Some("DDJ-SB2:DDJ-SB2 MIDI 1".to_string()),
            device_type: None,
            hid_product_match: None,
            hid_device_id: None,
            deck_target: DeckTargetConfig::default(),
            pad_mode_source: PadModeSource::default(),
            shift_buttons: vec![],
            mappings: vec![],
            feedback: vec![],
            color_note_offsets: None,
        };

        // Exact match with learned_port_name (different ALSA client IDs)
        assert!(port_matches("DDJ-SB2:DDJ-SB2 MIDI 1 28:0", &profile_with_learned));
        assert!(port_matches("DDJ-SB2:DDJ-SB2 MIDI 1 20:0", &profile_with_learned));

        // Case-insensitive exact match
        assert!(port_matches("ddj-sb2:ddj-sb2 midi 1", &profile_with_learned));

        // Also works with bracket format
        let profile_bracket_format = DeviceProfile {
            name: "My SB2".to_string(),
            port_match: "sb2".to_string(),
            learned_port_name: Some("DDJ-SB2 MIDI 1".to_string()),
            device_type: None,
            hid_product_match: None,
            hid_device_id: None,
            deck_target: DeckTargetConfig::default(),
            pad_mode_source: PadModeSource::default(),
            shift_buttons: vec![],
            mappings: vec![],
            feedback: vec![],
            color_note_offsets: None,
        };
        assert!(port_matches("DDJ-SB2 MIDI 1 [hw:3,0,0]", &profile_bracket_format));
        assert!(port_matches("DDJ-SB2 MIDI 1 [hw:1,0,0]", &profile_bracket_format));

        let profile_without_learned = DeviceProfile {
            name: "My SB2".to_string(),
            port_match: "sb2".to_string(),
            learned_port_name: None,
            device_type: None,
            hid_product_match: None,
            hid_device_id: None,
            deck_target: DeckTargetConfig::default(),
            pad_mode_source: PadModeSource::default(),
            shift_buttons: vec![],
            mappings: vec![],
            feedback: vec![],
            color_note_offsets: None,
        };

        // Fallback to substring match
        assert!(port_matches("DDJ-SB2:DDJ-SB2 MIDI 1 20:0", &profile_without_learned));
        assert!(port_matches("DDJ-SB2 MIDI 1 [hw:3,0,0]", &profile_without_learned));
        assert!(port_matches("Pioneer DDJ-SB2", &profile_without_learned));

        // No match
        assert!(!port_matches("Launchpad Mini", &profile_without_learned));
    }
}
