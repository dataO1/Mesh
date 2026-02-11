//! Shared MIDI state between input callback thread and mapping engine
//!
//! This module provides thread-safe shared state that both the midir input callback
//! and the mapping engine can access. The input callback writes shift/layer state,
//! while the mapping engine reads it to resolve deck targeting and shift actions.

use crate::deck_target::DeckTargetState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

/// Shared state between MIDI input callback and mapping engine
///
/// Both the input callback (midir driver thread) and the mapping engine
/// reference this via `Arc<SharedMidiState>`.
pub struct SharedMidiState {
    /// Global shift: true if ANY shift button is held
    pub shift_held_global: AtomicBool,
    /// Per-physical-deck shift state (index 0 = left, 1 = right)
    pub shift_held_per_deck: [AtomicBool; 2],
    /// Deck targeting state (layer toggle, deck resolution)
    pub deck_target: RwLock<DeckTargetState>,
}

impl SharedMidiState {
    /// Create new shared state from a deck target configuration
    pub fn new(deck_target: DeckTargetState) -> Self {
        Self {
            shift_held_global: AtomicBool::new(false),
            shift_held_per_deck: [AtomicBool::new(false), AtomicBool::new(false)],
            deck_target: RwLock::new(deck_target),
        }
    }

    /// Check if shift is held for a specific physical deck
    pub fn is_shift_held_for_deck(&self, physical_deck: usize) -> bool {
        let idx = physical_deck.min(1);
        self.shift_held_per_deck[idx].load(Ordering::Relaxed)
    }

    /// Check if any shift button is held (global)
    pub fn is_shift_held_global(&self) -> bool {
        self.shift_held_global.load(Ordering::Relaxed)
    }

    /// Update shift state for a physical deck and recalculate global
    pub fn set_shift_for_deck(&self, physical_deck: usize, held: bool) {
        let idx = physical_deck.min(1);
        self.shift_held_per_deck[idx].store(held, Ordering::Relaxed);

        // Recalculate global: any deck shift held = global shift held
        let any_held = self.shift_held_per_deck[0].load(Ordering::Relaxed)
            || self.shift_held_per_deck[1].load(Ordering::Relaxed);
        self.shift_held_global.store(any_held, Ordering::Relaxed);
    }

    /// Toggle layer for a physical deck
    ///
    /// Returns true if the toggle was performed (only works in layer mode)
    pub fn toggle_layer(&self, physical_deck: usize) -> bool {
        if let Ok(mut state) = self.deck_target.write() {
            if state.is_layer_mode() {
                state.toggle_layer(physical_deck);
                return true;
            }
        }
        false
    }

    /// Resolve physical deck to virtual deck
    pub fn resolve_deck(&self, physical_deck: usize) -> usize {
        self.deck_target
            .read()
            .map(|state| state.resolve_deck(physical_deck))
            .unwrap_or(physical_deck)
    }

    /// Get the current layer for a physical deck
    pub fn get_layer(&self, physical_deck: usize) -> crate::deck_target::LayerSelection {
        self.deck_target
            .read()
            .map(|state| state.get_layer(physical_deck))
            .unwrap_or_default()
    }

    /// Check if we're in layer mode
    pub fn is_layer_mode(&self) -> bool {
        self.deck_target
            .read()
            .map(|state| state.is_layer_mode())
            .unwrap_or(false)
    }
}

impl Default for SharedMidiState {
    fn default() -> Self {
        Self::new(DeckTargetState::default())
    }
}
