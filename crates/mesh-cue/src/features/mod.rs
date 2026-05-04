//! Audio Feature Extraction Module
//!
//! Extracts audio features using Essentia algorithms (key, BPM, LUFS).
//! Intensity scoring no longer goes through DSP components — it's now
//! projected at query time from MuQ-MuLan embeddings via the V15 axis
//! (see `crates/mesh-core/src/intensity_axis.rs` and the
//! `intensity-axis-pipeline-runbook.md`).
//!
//! ## Thread Safety
//!
//! Essentia's C++ library has global state and is NOT thread-safe.
//! All extraction must run in isolated subprocesses using procspawn.

mod extraction;

pub use extraction::{
    AudioFeatures,
    extract_audio_features,
    extract_audio_features_in_subprocess,
    FeatureExtractionError,
};
