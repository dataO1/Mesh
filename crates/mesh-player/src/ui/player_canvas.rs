//! Unified 4-deck waveform canvas - thin wrapper over mesh_widgets
//!
//! This module provides the player-specific integration for the 4-deck
//! waveform canvas from mesh_widgets. It wraps the generic view function
//! with mesh-player's specific Message types.

use super::app::Message;
use iced::Element;

// Re-export types from mesh_widgets
pub use mesh_widgets::PlayerCanvasState;

/// Create the unified 4-deck waveform canvas element
///
/// This displays all 4 deck waveforms in a single canvas:
/// - **Zoomed grid** (2x2): Deck 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
/// - **Overview stack**: Decks 1-4 stacked vertically below the grid
///
/// # Arguments
///
/// * `state` - The PlayerCanvasState containing all 4 decks' waveform data
///
/// # Returns
///
/// An `Element<Message>` that renders the unified waveform canvas
pub fn view_player_canvas(state: &PlayerCanvasState) -> Element<Message> {
    mesh_widgets::waveform_player(
        state,
        |deck_idx, pos| Message::DeckSeek(deck_idx, pos),
        |deck_idx, bars| Message::DeckSetZoom(deck_idx, bars),
    )
}
