//! Shared configuration utilities for mesh applications
//!
//! This module provides common configuration infrastructure shared between
//! mesh-player and mesh-cue, including:
//!
//! - Generic YAML config loading/saving
//! - Collection path utilities
//! - Loudness normalization configuration
//!
//! # Usage
//!
//! ```ignore
//! use mesh_core::config::{load_config, save_config, default_collection_path, LoudnessConfig};
//!
//! // Load app-specific config using generic loader
//! let config: MyAppConfig = load_config(&config_path);
//!
//! // Save config
//! save_config(&config, &config_path)?;
//! ```

mod io;
mod loudness;
mod paths;

pub use io::{load_config, save_config};
pub use loudness::LoudnessConfig;
pub use paths::{default_collection_path, default_config_path};
