//! Effects Editor Modal for mesh-cue
//!
//! Provides a full multiband effects editor for creating and managing presets.
//! This is the editing counterpart to mesh-player's preset selector.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  FX PRESETS                                                    [×]     │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  [Load Preset ▾] [Save Preset] [New] [Delete]                          │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                         │
//! │  ┌────────────────── MultibandEditorState Widget ──────────────────┐   │
//! │  │                                                                 │   │
//! │  │  (Crossover bar, bands, effects, macros - from mesh-widgets)   │   │
//! │  │                                                                 │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                                                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

mod state;
mod view;

pub use state::EffectsEditorState;
pub use view::effects_editor_view;
