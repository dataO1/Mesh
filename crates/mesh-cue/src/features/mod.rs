//! Audio Feature Extraction Module
//!
//! This module provides 16-dimensional audio feature extraction for similarity search.
//! Features are extracted using Essentia algorithms and can be used with CozoDB's
//! HNSW vector index for fast nearest-neighbor queries.
//!
//! ## Feature Dimensions
//!
//! The 16-dimensional feature vector is organized into four groups:
//!
//! - **Rhythm (4 dims):** BPM, confidence, beat strength, rhythm regularity
//! - **Harmony (4 dims):** Key (circular X/Y encoding), mode, harmonic complexity
//! - **Energy (4 dims):** LUFS, dynamic range, energy mean, energy variance
//! - **Timbre (4 dims):** Spectral centroid, bandwidth, rolloff, flatness
//!
//! ## Thread Safety
//!
//! Essentia's C++ library has global state and is NOT thread-safe.
//! All extraction must run in isolated subprocesses using procspawn.

mod extraction;

pub use extraction::{
    extract_audio_features,
    extract_audio_features_in_subprocess,
    FeatureExtractionError,
};

// Re-export AudioFeatures from mesh_core for convenience
pub use mesh_core::db::AudioFeatures;
