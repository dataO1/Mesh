//! Audio stem separation module
//!
//! Provides an abstracted API for separating mixed audio into individual stems
//! (vocals, drums, bass, other). The backend can be swapped without changing
//! the calling code.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                  SeparationService                       │
//! │  • Manages model downloads                              │
//! │  • Coordinates separation jobs                          │
//! │  • Handles temp file cleanup                            │
//! └─────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │              SeparationBackend (trait)                   │
//! │  • separate() - core separation logic                   │
//! │  • supports_gpu() - hardware capability                 │
//! └─────────────────────────────────────────────────────────┘
//!                              │
//!               ┌──────────────┴──────────────┐
//!               ▼                              ▼
//!     ┌─────────────────┐            ┌─────────────────┐
//!     │  CharonBackend  │            │   OrtBackend    │
//!     │  (charon-audio) │            │  (future: ort)  │
//!     └─────────────────┘            └─────────────────┘
//! ```

mod backend;
mod config;
mod error;
mod model;
mod service;

pub use backend::{CharonBackend, OrtBackend, SeparationBackend, StemData};
pub use config::{BackendType, ModelType, SeparationConfig};
pub use error::SeparationError;
pub use model::ModelManager;
pub use service::{SeparationProgress, SeparationService, SeparationStage};
