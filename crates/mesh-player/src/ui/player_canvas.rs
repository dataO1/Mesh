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
/// This displays all 4 deck waveforms in a 2x2 grid where each quadrant contains:
/// - **Header row** (16px): Deck number indicator + track load status
/// - **Zoomed waveform** (120px): Detail view centered on playhead
/// - **Overview waveform** (35px): Full track view with cue markers
///
/// Grid layout: Deck 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
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
