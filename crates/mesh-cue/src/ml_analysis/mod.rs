//! ML-based audio analysis module
//!
//! Provides genre classification, mood/theme tagging, voice/instrumental detection,
//! and derived arousal/valence using Essentia-based preprocessing + EffNet ONNX models.
//!
//! # Architecture
//!
//! - **Preprocessing** (`preprocessing.rs`): Pure Rust mel spectrogram computation
//! - **Model management** (`models.rs`): Download + cache ONNX models from Essentia Hub
//! - **Inference** (`inference.rs`): ort-based EffNet embedding → classification heads
//!   (includes voice/instrumental classifier — replaces old RMS-based detection)
//! - **Arousal/valence**: Derived from Jamendo mood predictions (no separate A/V model)

pub mod preprocessing;
pub mod models;
pub mod inference;
pub mod beat_inference;

// Re-export key types
pub use inference::MlAnalyzer;
pub use beat_inference::BeatThisAnalyzer;
pub use models::{MlModelManager, MlModelType};
