//! Pure Data integration via libpd-rs
//!
//! This module provides a bridge between mesh's effect system and Pure Data,
//! allowing users to load custom PD patches as audio effects.
//!
//! # Architecture
//!
//! The PD integration follows a layered architecture for separation of concerns:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      PdManager                               │
//! │  - Manages per-deck PdInstance instances                    │
//! │  - Handles effect discovery and creation                    │
//! │  - Provides thread-safe access to PD resources              │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!          ┌──────────────────┼──────────────────┐
//!          ▼                  ▼                  ▼
//! ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
//! │   PdInstance    │ │   PdInstance    │ │   PdInstance    │
//! │   (Deck 0)      │ │   (Deck 1)      │ │   (Deck 2...)   │
//! │                 │ │                 │ │                 │
//! │ ┌─────────────┐ │ │ ┌─────────────┐ │ │ ┌─────────────┐ │
//! │ │  PdEffect   │ │ │ │  PdEffect   │ │ │ │  PdEffect   │ │
//! │ │  (patch 1)  │ │ │ │  (patch 1)  │ │ │ │  (patch 1)  │ │
//! │ └─────────────┘ │ │ └─────────────┘ │ │ └─────────────┘ │
//! │ ┌─────────────┐ │ │                 │ │                 │
//! │ │  PdEffect   │ │ │                 │ │                 │
//! │ │  (patch 2)  │ │ │                 │ │                 │
//! │ └─────────────┘ │ │                 │ │                 │
//! └─────────────────┘ └─────────────────┘ └─────────────────┘
//! ```
//!
//! # Effect Discovery
//!
//! Effects are discovered from the mesh collection's `effects/` folder:
//!
//! ```text
//! ~/Music/mesh-collection/effects/
//! ├── externals/           # Shared PD externals (nn~, etc.)
//! ├── models/              # Shared neural models
//! └── my-effect/           # Effect folder
//!     ├── my-effect.pd     # PD patch (must match folder name)
//!     └── metadata.json    # Effect metadata
//! ```
//!
//! # PD Patch Contract
//!
//! Effects must follow this contract for mesh compatibility:
//!
//! - **Inlets**: Two signal inlets (left, right audio)
//! - **Outlets**: Two signal outlets (left, right audio)
//! - **Parameters**: `[r $0-param0]` through `[r $0-param7]` (0.0-1.0)
//! - **Bypass**: `[r $0-bypass]` (0 = process, 1 = bypass)
//!
//! The `$0-` prefix ensures instance isolation when multiple effects run.
//!
//! # Example
//!
//! ```ignore
//! use mesh_core::pd::{PdManager, PdError};
//!
//! // Create manager with collection path
//! let mut manager = PdManager::new(&collection_path)?;
//!
//! // Discover available effects
//! let effects = manager.discover_effects();
//!
//! // Create an effect instance for deck 0
//! let effect = manager.create_effect(0, "rave-percussion")?;
//!
//! // Add to effect chain (effect implements the Effect trait)
//! chain.add_effect(effect);
//! ```

mod error;
mod instance;
mod effect;
mod metadata;
mod discovery;
mod manager;

// Re-export public API
pub use error::{PdError, PdResult};
pub use instance::PdInstance;
pub use effect::PdEffect;
pub use metadata::{EffectMetadata, ParamMetadata};
pub use discovery::{EffectDiscovery, DiscoveredEffect};
pub use manager::PdManager;
