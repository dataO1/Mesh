//! Smart suggestion engine — re-exports from mesh-core.
//!
//! The scoring logic lives in `mesh_core::suggestions` so it can be shared
//! with mesh-cue (graph view). This module re-exports query types so existing
//! `use crate::suggestions::...` imports throughout mesh-player continue to work.

pub use mesh_core::suggestions::query::*;
