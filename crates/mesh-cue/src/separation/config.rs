//! Separation configuration types

use serde::{Deserialize, Serialize};

/// Configuration for audio stem separation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeparationConfig {
    /// Which backend to use for separation
    pub backend: BackendType,

    /// Which model to use
    pub model: ModelType,

    /// Whether to attempt GPU acceleration
    pub use_gpu: bool,

    /// Segment length in seconds for processing (affects memory usage)
    pub segment_length_secs: f64,
}

impl Default for SeparationConfig {
    fn default() -> Self {
        Self {
            backend: BackendType::OnnxRuntime, // ORT is currently the only working backend
            model: ModelType::Demucs4Stems,
            use_gpu: true, // Try GPU, fall back to CPU
            segment_length_secs: 10.0,
        }
    }
}

impl SeparationConfig {
    /// Validate configuration values
    pub fn validate(&mut self) {
        // Clamp segment length to reasonable range
        self.segment_length_secs = self.segment_length_secs.clamp(5.0, 60.0);
    }
}

/// Available separation backends
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackendType {
    /// charon-audio crate (currently unavailable due to rayon version conflict)
    Charon,

    /// Direct ONNX Runtime via ort crate (recommended)
    #[default]
    OnnxRuntime,
}

impl BackendType {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Charon => "Charon (unavailable)",
            Self::OnnxRuntime => "ONNX Runtime",
        }
    }

    /// Description for UI
    pub fn description(&self) -> &'static str {
        match self {
            Self::Charon => "Pure Rust via charon-audio (blocked by rayon conflict)",
            Self::OnnxRuntime => "Direct ONNX Runtime inference - recommended",
        }
    }

    /// Check if this backend is currently available
    pub fn is_available(&self) -> bool {
        match self {
            Self::Charon => false, // Blocked by rayon version conflict
            Self::OnnxRuntime => true,
        }
    }

    /// All backend types (for UI enumeration)
    pub fn all() -> &'static [Self] {
        &[Self::OnnxRuntime, Self::Charon]
    }

    /// Only available backends (for UI selection)
    pub fn available() -> Vec<Self> {
        Self::all().iter().copied().filter(|b| b.is_available()).collect()
    }
}

/// Available separation models
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ModelType {
    /// Standard Demucs with 4 stems (vocals, drums, bass, other)
    /// ~150MB, fastest option
    #[default]
    Demucs4Stems,

    /// Demucs with 6 stems (+ piano, guitar)
    /// ~200MB, slightly slower
    Demucs6Stems,
}

impl ModelType {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Demucs 4-stem (150MB)",
            Self::Demucs6Stems => "Demucs 6-stem (200MB)",
        }
    }

    /// Description for UI
    pub fn description(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Vocals, Drums, Bass, Other - fast and efficient",
            Self::Demucs6Stems => "Adds Piano and Guitar stems - slightly slower",
        }
    }

    /// Model filename (must match the name used during ONNX export, since external data
    /// files reference it by name)
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "htdemucs.onnx",
            Self::Demucs6Stems => "htdemucs_6s.onnx",
        }
    }

    /// Download URL (GitHub releases)
    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                // Hosted on Mesh GitHub releases (MIT licensed, converted from Meta's Demucs)
                "https://github.com/dataO1/Mesh/releases/download/models/htdemucs.onnx"
            }
            Self::Demucs6Stems => {
                // TODO: Find or create 6-stem ONNX model
                "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_6s.onnx"
            }
        }
    }

    /// Approximate model size in bytes
    pub fn size_bytes(&self) -> u64 {
        match self {
            Self::Demucs4Stems => 171_000_000, // ~171MB
            Self::Demucs6Stems => 200_000_000, // ~200MB
        }
    }

    /// All available models
    pub fn all() -> &'static [Self] {
        &[Self::Demucs4Stems, Self::Demucs6Stems]
    }

    /// Number of output stems
    pub fn stem_count(&self) -> usize {
        match self {
            Self::Demucs4Stems => 4,
            Self::Demucs6Stems => 6,
        }
    }
}
