//! UI module for Mesh DJ Player
//!
//! Built with iced - a cross-platform GUI library for Rust.
//! Uses a message-passing architecture to communicate with the audio thread.

pub mod app;
pub mod handlers;
pub mod collection_browser;
pub mod deck_view;
pub mod effect_picker;
pub mod message;
pub mod midi_learn;
pub mod mixer_view;
pub mod player_canvas;
pub mod settings;
pub mod state;
pub mod theme;

pub use app::MeshApp;
