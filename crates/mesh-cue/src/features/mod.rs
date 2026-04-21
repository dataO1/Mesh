//! Audio Feature Extraction Module
//!
//! Extracts audio features using Essentia algorithms for intensity scoring
//! and IntensityComponents computation. The features are used locally to seed
//! multi-frame intensity analysis — similarity search uses EffNet PCA embeddings.
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
    compute_intensity_components,
    FeatureExtractionError,
};
