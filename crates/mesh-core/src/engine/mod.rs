//! Audio engine - Deck, Mixer, latency compensation
//!
//! This module contains the core audio engine components for the DJ player:
//! - Deck: Individual track player with stems and effect chains
//! - Mixer: Combines deck outputs with volume/filter controls
//! - Global latency compensation across all stems
//! - AudioEngine: Main engine tying everything together

mod deck;
mod engine;
mod latency;
mod mixer;

pub use deck::*;
pub use engine::*;
pub use latency::*;
pub use mixer::*;
