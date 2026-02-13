//! ML-based audio analysis module
//!
//! Provides voice detection, genre classification, mood/theme tagging, and
//! derived arousal/valence using Essentia-based preprocessing + EffNet ONNX models.
//!
//! # Architecture
//!
//! - **Voice detection** (`voice.rs`): Pure Rust RMS energy on vocal stem — no model needed
//! - **Preprocessing** (`preprocessing.rs`): Pure Rust mel spectrogram computation
//! - **Model management** (`models.rs`): Download + cache ONNX models from Essentia Hub
//! - **Inference** (`inference.rs`): ort-based EffNet embedding → classification heads
//! - **Arousal/valence**: Derived from Jamendo mood predictions (no separate A/V model)

pub mod voice;
pub mod preprocessing;
pub mod models;
pub mod inference;

// Re-export key types
pub use voice::compute_vocal_presence;
pub use inference::MlAnalyzer;
pub use models::{MlModelManager, MlModelType};
