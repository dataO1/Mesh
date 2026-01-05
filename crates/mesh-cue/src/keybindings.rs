//! Keybindings configuration for mesh-cue
//!
//! Configurable keyboard shortcuts stored in YAML format.
//! Default location: ~/Music/mesh-collection/keybindings.yaml

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Root keybindings configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    /// Keybindings for editing mode (collection editor)
    pub editing: EditingKeybindings,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            editing: EditingKeybindings::default(),
        }
    }
}

/// Keybindings for editing mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EditingKeybindings {
    /// Play/pause toggle
    pub play_pause: Vec<String>,
    /// Beat jump forward
    pub beat_jump_forward: Vec<String>,
    /// Beat jump backward
    pub beat_jump_backward: Vec<String>,
    /// Nudge beat grid forward (later)
    pub grid_nudge_forward: Vec<String>,
    /// Nudge beat grid backward (earlier)
    pub grid_nudge_backward: Vec<String>,
    /// Increase beat jump size
    pub increase_jump_size: Vec<String>,
    /// Decrease beat jump size
    pub decrease_jump_size: Vec<String>,
    /// Main cue button (CDJ-style: set/return to cue point)
    pub cue_button: Vec<String>,
    /// Hot cue buttons (1-8)
    pub hot_cue_1: Vec<String>,
    pub hot_cue_2: Vec<String>,
    pub hot_cue_3: Vec<String>,
    pub hot_cue_4: Vec<String>,
    pub hot_cue_5: Vec<String>,
    pub hot_cue_6: Vec<String>,
    pub hot_cue_7: Vec<String>,
    pub hot_cue_8: Vec<String>,
    /// Delete hot cue buttons (shift+1-8)
    pub delete_hot_cue_1: Vec<String>,
    pub delete_hot_cue_2: Vec<String>,
    pub delete_hot_cue_3: Vec<String>,
    pub delete_hot_cue_4: Vec<String>,
    pub delete_hot_cue_5: Vec<String>,
    pub delete_hot_cue_6: Vec<String>,
    pub delete_hot_cue_7: Vec<String>,
    pub delete_hot_cue_8: Vec<String>,
}

impl Default for EditingKeybindings {
    fn default() -> Self {
        Self {
            play_pause: vec!["Space".into()],
            beat_jump_forward: vec!["Right".into()],
            beat_jump_backward: vec!["Left".into()],
            grid_nudge_forward: vec!["Shift+Right".into()],
            grid_nudge_backward: vec!["Shift+Left".into()],
            increase_jump_size: vec!["Up".into()],
            decrease_jump_size: vec!["Down".into()],
            cue_button: vec!["c".into()],
            hot_cue_1: vec!["1".into()],
            hot_cue_2: vec!["2".into()],
            hot_cue_3: vec!["3".into()],
            hot_cue_4: vec!["4".into()],
            hot_cue_5: vec!["5".into()],
            hot_cue_6: vec!["6".into()],
            hot_cue_7: vec!["7".into()],
            hot_cue_8: vec!["8".into()],
            delete_hot_cue_1: vec!["Shift+1".into()],
            delete_hot_cue_2: vec!["Shift+2".into()],
            delete_hot_cue_3: vec!["Shift+3".into()],
            delete_hot_cue_4: vec!["Shift+4".into()],
            delete_hot_cue_5: vec!["Shift+5".into()],
            delete_hot_cue_6: vec!["Shift+6".into()],
            delete_hot_cue_7: vec!["Shift+7".into()],
            delete_hot_cue_8: vec!["Shift+8".into()],
        }
    }
}

/// Get the default keybindings file path
///
/// Returns: ~/Music/mesh-collection/keybindings.yaml
pub fn default_keybindings_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Music")
        .join("mesh-collection")
        .join("keybindings.yaml")
}

/// Load keybindings from a YAML file
///
/// If the file doesn't exist, returns default keybindings.
/// If the file exists but is invalid, logs a warning and returns defaults.
pub fn load_keybindings(path: &Path) -> KeybindingsConfig {
    log::info!("load_keybindings: Loading from {:?}", path);

    if !path.exists() {
        log::info!("load_keybindings: File doesn't exist, using defaults");
        return KeybindingsConfig::default();
    }

    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_yaml::from_str::<KeybindingsConfig>(&contents) {
            Ok(config) => {
                log::info!("load_keybindings: Loaded custom keybindings");
                config
            }
            Err(e) => {
                log::warn!("load_keybindings: Failed to parse: {}, using defaults", e);
                KeybindingsConfig::default()
            }
        },
        Err(e) => {
            log::warn!("load_keybindings: Failed to read file: {}, using defaults", e);
            KeybindingsConfig::default()
        }
    }
}

/// Save keybindings to a YAML file
pub fn save_keybindings(config: &KeybindingsConfig, path: &Path) -> anyhow::Result<()> {
    log::info!("save_keybindings: Saving to {:?}", path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let yaml = serde_yaml::to_string(config)?;
    std::fs::write(path, yaml)?;

    log::info!("save_keybindings: Saved successfully");
    Ok(())
}

/// Convert an iced keyboard key + modifiers to a string for matching
///
/// Format: "Shift+Ctrl+Alt+KeyName"
pub fn key_to_string(key: &iced::keyboard::Key, modifiers: &iced::keyboard::Modifiers) -> String {
    use iced::keyboard::{Key, key::Named};

    let mut parts = Vec::new();
    if modifiers.shift() {
        parts.push("Shift");
    }
    if modifiers.control() {
        parts.push("Ctrl");
    }
    if modifiers.alt() {
        parts.push("Alt");
    }

    let key_name = match key {
        Key::Named(named) => match named {
            Named::Space => "Space".to_string(),
            Named::ArrowUp => "Up".to_string(),
            Named::ArrowDown => "Down".to_string(),
            Named::ArrowLeft => "Left".to_string(),
            Named::ArrowRight => "Right".to_string(),
            Named::Enter => "Enter".to_string(),
            Named::Escape => "Escape".to_string(),
            Named::Tab => "Tab".to_string(),
            Named::Backspace => "Backspace".to_string(),
            Named::Delete => "Delete".to_string(),
            Named::Home => "Home".to_string(),
            Named::End => "End".to_string(),
            Named::PageUp => "PageUp".to_string(),
            Named::PageDown => "PageDown".to_string(),
            Named::F1 => "F1".to_string(),
            Named::F2 => "F2".to_string(),
            Named::F3 => "F3".to_string(),
            Named::F4 => "F4".to_string(),
            Named::F5 => "F5".to_string(),
            Named::F6 => "F6".to_string(),
            Named::F7 => "F7".to_string(),
            Named::F8 => "F8".to_string(),
            Named::F9 => "F9".to_string(),
            Named::F10 => "F10".to_string(),
            Named::F11 => "F11".to_string(),
            Named::F12 => "F12".to_string(),
            _ => return String::new(), // Ignore other named keys
        },
        Key::Character(c) => c.to_string(),
        _ => return String::new(),
    };

    if parts.is_empty() {
        key_name
    } else {
        parts.push(&key_name);
        parts.join("+")
    }
}

impl EditingKeybindings {
    /// Check if a key matches any hot cue binding and return the index (0-7)
    pub fn match_hot_cue(&self, key_str: &str) -> Option<usize> {
        let bindings = [
            &self.hot_cue_1,
            &self.hot_cue_2,
            &self.hot_cue_3,
            &self.hot_cue_4,
            &self.hot_cue_5,
            &self.hot_cue_6,
            &self.hot_cue_7,
            &self.hot_cue_8,
        ];
        for (i, binding) in bindings.iter().enumerate() {
            if binding.iter().any(|b| b == key_str) {
                return Some(i);
            }
        }
        None
    }

    /// Check if a key matches any delete hot cue binding and return the index (0-7)
    pub fn match_delete_hot_cue(&self, key_str: &str) -> Option<usize> {
        let bindings = [
            &self.delete_hot_cue_1,
            &self.delete_hot_cue_2,
            &self.delete_hot_cue_3,
            &self.delete_hot_cue_4,
            &self.delete_hot_cue_5,
            &self.delete_hot_cue_6,
            &self.delete_hot_cue_7,
            &self.delete_hot_cue_8,
        ];
        for (i, binding) in bindings.iter().enumerate() {
            if binding.iter().any(|b| b == key_str) {
                return Some(i);
            }
        }
        None
    }

    /// Check if a key matches the main cue button
    pub fn match_cue_button(&self, key_str: &str) -> bool {
        self.cue_button.iter().any(|b| b == key_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_keybindings() {
        let config = KeybindingsConfig::default();
        assert!(config.editing.play_pause.contains(&"Space".to_string()));
        assert!(config.editing.beat_jump_forward.contains(&"Right".to_string()));
    }

    #[test]
    fn test_yaml_roundtrip() {
        let config = KeybindingsConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: KeybindingsConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.editing.play_pause, config.editing.play_pause);
    }

    #[test]
    fn test_match_hot_cue() {
        let bindings = EditingKeybindings::default();
        assert_eq!(bindings.match_hot_cue("1"), Some(0));
        assert_eq!(bindings.match_hot_cue("8"), Some(7));
        assert_eq!(bindings.match_hot_cue("9"), None);
    }

    #[test]
    fn test_match_delete_hot_cue() {
        let bindings = EditingKeybindings::default();
        assert_eq!(bindings.match_delete_hot_cue("Shift+1"), Some(0));
        assert_eq!(bindings.match_delete_hot_cue("Shift+8"), Some(7));
        assert_eq!(bindings.match_delete_hot_cue("1"), None);
    }
}
