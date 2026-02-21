//! Unified 4-deck waveform canvas - thin wrapper over mesh_widgets
//!
//! This module provides the player-specific integration for the 4-deck
//! waveform display from mesh_widgets. It wraps the GPU shader renderer
//! with mesh-player's specific Message types.

use super::app::Message;
use iced::Element;
use mesh_widgets::WaveformAction;

// Re-export types from mesh_widgets
pub use mesh_widgets::PlayerCanvasState;

/// Create the unified 4-deck waveform display using GPU shader rendering
///
/// This displays all 4 deck waveforms in a 2x2 grid rendered entirely on the GPU.
/// Peak data is uploaded once at track load; only a 384-byte uniform buffer
/// is updated per frame, eliminating CPU lyon tessellation overhead.
///
/// Grid layout: Deck 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
pub fn view_player_canvas(state: &PlayerCanvasState) -> Element<'_, Message> {
    mesh_widgets::waveform_player_shader(state, |action| match action {
        WaveformAction::Seek(deck_idx, pos) => Message::DeckSeek(deck_idx, pos),
        WaveformAction::SetZoom(deck_idx, bars) => Message::DeckSetZoom(deck_idx, bars),
    })
}

/// DEPRECATED: Canvas-based 4-deck waveform display (CPU lyon tessellation)
///
/// This is the old canvas renderer kept for fallback/debugging.
/// Use `view_player_canvas()` instead, which uses GPU shader rendering.
#[allow(dead_code)]
pub fn view_player_canvas_legacy(state: &PlayerCanvasState) -> Element<'_, Message> {
    mesh_widgets::waveform_player(
        state,
        |deck_idx, pos| Message::DeckSeek(deck_idx, pos),
        |deck_idx, bars| Message::DeckSetZoom(deck_idx, bars),
    )
}
