//! Smart suggestion engine for the collection browser.
//!
//! Queries the CozoDB HNSW index to find tracks similar to the currently
//! loaded deck seeds, then re-scores them using a unified multi-factor formula
//! with energy-direction-aware harmonic scoring.
//!
//! # Module structure
//! - `config`: Algorithm configuration enums (serialized to YAML)
//! - `scoring`: Pure scoring functions (transition classification, harmonic, intensity)
//! - `query`: Query orchestration (HNSW search + scoring pipeline)

pub mod aggression;
pub mod config;
pub mod scoring;
pub mod query;

pub use query::GraphEdge;
pub use aggression::UncoveredCommunity;
