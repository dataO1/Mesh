//! Message handlers for MeshApp
//!
//! Each handler module is responsible for a specific category of messages.
//! Handlers receive `&mut MeshApp` and return `Task<Message>`.

pub mod mixer;
pub mod settings;
pub mod midi_learn;
pub mod browser;
pub mod track_loading;
pub mod deck_controls;
pub mod tick;
