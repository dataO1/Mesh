//! Background loading utilities for Mesh DJ software
//!
//! This module provides shared loading infrastructure that can be used by
//! both mesh-player and mesh-cue, avoiding code duplication.
//!
//! # Linked Stem Loading
//!
//! The `LinkedStemLoader` handles loading linked stems from other tracks:
//! - Extracts a single stem from an 8-channel file
//! - Pre-stretches to match the host deck's BPM
//! - Pre-aligns to host timeline using drop markers
//! - Generates waveform peaks for UI display
//!
//! # Message-Driven Architecture
//!
//! The loader is designed for iced's message-driven architecture:
//! - Use `result_receiver()` to get a clonable receiver for subscriptions
//! - Results arrive as messages, no polling needed

mod linked_stem;

pub use linked_stem::{
    HostTrackParams,
    LinkedStemLoadRequest,
    LinkedStemLoadResult,
    LinkedStemLoader,
    LinkedStemResultReceiver,
};
