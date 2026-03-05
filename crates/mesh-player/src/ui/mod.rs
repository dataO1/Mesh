//! UI module for Mesh DJ Player
//!
//! Built with iced - a cross-platform GUI library for Rust.
//! Uses a message-passing architecture to communicate with the audio thread.

pub mod app;
pub mod handlers;
pub mod collection_browser;
pub mod deck_view;
pub mod message;
pub mod network;
pub mod system_update;
pub mod midi_learn;
pub mod midi_learn_tree;
pub mod mixer_view;
pub mod player_canvas;
pub mod settings;
pub mod state;

pub use app::MeshApp;
