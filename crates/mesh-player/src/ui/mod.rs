//! UI module for Mesh DJ Player
//!
//! Built with iced - a cross-platform GUI library for Rust.
//! Uses a message-passing architecture to communicate with the audio thread.

pub mod app;
pub mod deck_view;
pub mod mixer_view;
pub mod waveform;

pub use app::MeshApp;
