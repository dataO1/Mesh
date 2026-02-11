//! Deck targeting for MIDI controllers
//!
//! Supports two modes:
//! - **Direct**: Each MIDI channel maps directly to a deck (for 4-deck controllers)
//! - **Layer**: Toggle buttons switch which virtual deck physical controls target
//!
//! # Layer Mode (DDJ-SB2 style)
//!
//! ```text
//! Physical Deck 0 ──► Layer A: Deck 0  │  Layer B: Deck 2
//! Physical Deck 1 ──► Layer A: Deck 1  │  Layer B: Deck 3
//!
//! [DECK 1/3 toggle] switches left side between Deck 0 and Deck 2
//! [DECK 2/4 toggle] switches right side between Deck 1 and Deck 3
//! ```

use crate::config::DeckTargetConfig;
use std::collections::HashMap;

/// Deck targeting mode (runtime state)
#[derive(Debug, Clone)]
pub enum DeckTargetMode {
    /// Direct channel-to-deck mapping
    Direct {
        /// Map MIDI channel to deck index
        channel_to_deck: HashMap<u8, usize>,
    },
    /// Layer toggle mode
    Layer {
        /// Virtual deck indices for Layer A
        layer_a: Vec<usize>,
        /// Virtual deck indices for Layer B
        layer_b: Vec<usize>,
    },
}

/// Layer selection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerSelection {
    /// Layer A (default): decks 1 & 2
    #[default]
    A,
    /// Layer B (toggled): decks 3 & 4
    B,
}

impl LayerSelection {
    /// Toggle to the other layer
    pub fn toggle(&mut self) {
        *self = match self {
            Self::A => Self::B,
            Self::B => Self::A,
        };
    }
}

/// Deck targeting state manager
#[derive(Debug, Clone)]
pub struct DeckTargetState {
    /// Current targeting mode
    mode: DeckTargetMode,
    /// Current layer for each physical deck (only used in Layer mode)
    /// Index 0 = left physical deck, Index 1 = right physical deck
    physical_deck_layers: [LayerSelection; 2],
}

impl Default for DeckTargetState {
    fn default() -> Self {
        // Default to direct 1:1 mapping
        let mut channel_to_deck = HashMap::new();
        for i in 0..4 {
            channel_to_deck.insert(i, i as usize);
        }
        Self {
            mode: DeckTargetMode::Direct { channel_to_deck },
            physical_deck_layers: [LayerSelection::A, LayerSelection::A],
        }
    }
}

impl DeckTargetState {
    /// Create from configuration
    pub fn from_config(config: &DeckTargetConfig) -> Self {
        let mode = match config {
            DeckTargetConfig::Direct { channel_to_deck } => DeckTargetMode::Direct {
                channel_to_deck: channel_to_deck.clone(),
            },
            DeckTargetConfig::Layer {
                layer_a, layer_b, ..
            } => DeckTargetMode::Layer {
                layer_a: layer_a.clone(),
                layer_b: layer_b.clone(),
            },
        };

        Self {
            mode,
            physical_deck_layers: [LayerSelection::A, LayerSelection::A],
        }
    }

    /// Resolve which virtual deck a control targets
    ///
    /// # Arguments
    /// * `physical_deck` - Physical deck index (0 = left, 1 = right)
    ///
    /// # Returns
    /// Virtual deck index (0-3)
    pub fn resolve_deck(&self, physical_deck: usize) -> usize {
        match &self.mode {
            DeckTargetMode::Direct { channel_to_deck } => {
                // In direct mode, physical_deck is used as channel
                channel_to_deck
                    .get(&(physical_deck as u8))
                    .copied()
                    .unwrap_or(physical_deck)
            }
            DeckTargetMode::Layer { layer_a, layer_b } => {
                let physical_idx = physical_deck.min(1); // Clamp to 0 or 1
                let layer = self.physical_deck_layers[physical_idx];

                let decks = match layer {
                    LayerSelection::A => layer_a,
                    LayerSelection::B => layer_b,
                };

                decks.get(physical_idx).copied().unwrap_or(physical_idx)
            }
        }
    }

    /// Resolve which virtual deck a MIDI channel targets (for Direct mode)
    ///
    /// # Arguments
    /// * `channel` - MIDI channel (0-15)
    ///
    /// # Returns
    /// Virtual deck index (0-3)
    pub fn resolve_deck_from_channel(&self, channel: u8) -> usize {
        match &self.mode {
            DeckTargetMode::Direct { channel_to_deck } => {
                channel_to_deck.get(&channel).copied().unwrap_or(0)
            }
            DeckTargetMode::Layer { .. } => {
                // In layer mode, use channel to determine physical deck
                // Channel 0 = left, Channel 1 = right
                let physical_deck = (channel & 1) as usize;
                self.resolve_deck(physical_deck)
            }
        }
    }

    /// Toggle layer for a physical deck
    ///
    /// # Arguments
    /// * `physical_deck` - Physical deck index (0 = left, 1 = right)
    pub fn toggle_layer(&mut self, physical_deck: usize) {
        if let DeckTargetMode::Layer { .. } = &self.mode {
            let idx = physical_deck.min(1);
            self.physical_deck_layers[idx].toggle();
        }
    }

    /// Get current layer for a physical deck
    pub fn get_layer(&self, physical_deck: usize) -> LayerSelection {
        let idx = physical_deck.min(1);
        self.physical_deck_layers[idx]
    }

    /// Set layer for a physical deck
    pub fn set_layer(&mut self, physical_deck: usize, layer: LayerSelection) {
        let idx = physical_deck.min(1);
        self.physical_deck_layers[idx] = layer;
    }

    /// Check if we're in layer mode
    pub fn is_layer_mode(&self) -> bool {
        matches!(self.mode, DeckTargetMode::Layer { .. })
    }

    /// Get all virtual decks that a physical deck can target
    ///
    /// In Direct mode, returns just the mapped deck.
    /// In Layer mode, returns both Layer A and Layer B decks.
    pub fn possible_decks(&self, physical_deck: usize) -> Vec<usize> {
        match &self.mode {
            DeckTargetMode::Direct { channel_to_deck } => {
                channel_to_deck
                    .get(&(physical_deck as u8))
                    .copied()
                    .map(|d| vec![d])
                    .unwrap_or_default()
            }
            DeckTargetMode::Layer { layer_a, layer_b } => {
                let idx = physical_deck.min(1);
                let mut decks = Vec::new();
                if let Some(&d) = layer_a.get(idx) {
                    decks.push(d);
                }
                if let Some(&d) = layer_b.get(idx) {
                    decks.push(d);
                }
                decks
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_mode() {
        let mut channel_to_deck = HashMap::new();
        channel_to_deck.insert(0, 0);
        channel_to_deck.insert(1, 1);
        channel_to_deck.insert(2, 2);
        channel_to_deck.insert(3, 3);

        let config = DeckTargetConfig::Direct { channel_to_deck };
        let state = DeckTargetState::from_config(&config);

        assert_eq!(state.resolve_deck_from_channel(0), 0);
        assert_eq!(state.resolve_deck_from_channel(1), 1);
        assert_eq!(state.resolve_deck_from_channel(2), 2);
        assert_eq!(state.resolve_deck_from_channel(3), 3);
    }

    #[test]
    fn test_layer_mode() {
        let config = DeckTargetConfig::Layer {
            toggle_left: crate::types::ControlAddress::Midi(crate::types::MidiAddress::Note { channel: 0, note: 0x72 }),
            toggle_right: crate::types::ControlAddress::Midi(crate::types::MidiAddress::Note { channel: 1, note: 0x72 }),
            layer_a: vec![0, 1],
            layer_b: vec![2, 3],
        };
        let mut state = DeckTargetState::from_config(&config);

        // Default: Layer A
        assert_eq!(state.resolve_deck(0), 0); // Left -> Deck 1
        assert_eq!(state.resolve_deck(1), 1); // Right -> Deck 2

        // Toggle left to Layer B
        state.toggle_layer(0);
        assert_eq!(state.resolve_deck(0), 2); // Left -> Deck 3
        assert_eq!(state.resolve_deck(1), 1); // Right still -> Deck 2

        // Toggle right to Layer B
        state.toggle_layer(1);
        assert_eq!(state.resolve_deck(0), 2); // Left -> Deck 3
        assert_eq!(state.resolve_deck(1), 3); // Right -> Deck 4

        // Toggle left back to Layer A
        state.toggle_layer(0);
        assert_eq!(state.resolve_deck(0), 0); // Left -> Deck 1
        assert_eq!(state.resolve_deck(1), 3); // Right still -> Deck 4
    }

    #[test]
    fn test_layer_selection() {
        let mut layer = LayerSelection::A;
        assert_eq!(layer, LayerSelection::A);

        layer.toggle();
        assert_eq!(layer, LayerSelection::B);

        layer.toggle();
        assert_eq!(layer, LayerSelection::A);
    }
}
