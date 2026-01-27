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

    /// Number of random time shifts for improved quality (1-5)
    /// Higher values improve separation quality (~0.2 SDR per shift) but
    /// increase processing time proportionally. Set to 1 to disable.
    #[serde(default = "default_shifts")]
    pub shifts: u8,
}

fn default_shifts() -> u8 {
    1
}

impl Default for SeparationConfig {
    fn default() -> Self {
        Self {
            backend: BackendType::OnnxRuntime, // ORT is currently the only working backend
            model: ModelType::Demucs4Stems,
            use_gpu: true, // Try GPU, fall back to CPU
            segment_length_secs: 10.0,
            shifts: 1, // Disabled by default (1 = no averaging)
        }
    }
}

impl SeparationConfig {
    /// Validate configuration values
    pub fn validate(&mut self) {
        // Clamp segment length to reasonable range
        self.segment_length_secs = self.segment_length_secs.clamp(5.0, 60.0);
        // Clamp shifts to reasonable range (1-5)
        self.shifts = self.shifts.clamp(1, 5);
    }

    /// Display name for shifts value
    pub fn shifts_display_name(shifts: u8) -> &'static str {
        match shifts {
            1 => "Off (1×)",
            2 => "Low (2×)",
            3 => "Medium (3×)",
            4 => "High (4×)",
            5 => "Maximum (5×)",
            _ => "Unknown",
        }
    }

    /// Description for shifts value
    pub fn shifts_description(shifts: u8) -> &'static str {
        match shifts {
            1 => "Fastest - no shift averaging",
            2 => "2× slower, slightly better quality",
            3 => "3× slower, better quality",
            4 => "4× slower, high quality",
            5 => "5× slower, best quality (~0.2 SDR improvement)",
            _ => "",
        }
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
    /// ~163MB, fastest option
    #[default]
    Demucs4Stems,

    /// Fine-tuned Demucs with 4 stems - better quality
    /// ~163MB, same speed as standard but ~1-3% better SDR
    Demucs4StemsFt,

    /// Demucs with 6 stems (+ piano, guitar)
    /// ~200MB, slightly slower
    Demucs6Stems,
}

impl ModelType {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Demucs 4-stem",
            Self::Demucs4StemsFt => "Demucs 4-stem Fine-tuned",
            Self::Demucs6Stems => "Demucs 6-stem",
        }
    }

    /// Description for UI
    pub fn description(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Vocals, Drums, Bass, Other - fast (~163MB)",
            Self::Demucs4StemsFt => "Fine-tuned for better quality (~163MB)",
            Self::Demucs6Stems => "Adds Piano and Guitar stems (~200MB)",
        }
    }

    /// Model filename (must match the name used during ONNX export, since external data
    /// files reference it by name)
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "htdemucs.onnx",
            Self::Demucs4StemsFt => "htdemucs_ft.onnx",
            Self::Demucs6Stems => "htdemucs_6s.onnx",
        }
    }

    /// Download URL (GitHub releases)
    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                "https://github.com/dataO1/Mesh/releases/download/models/htdemucs.onnx"
            }
            Self::Demucs4StemsFt => {
                "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_ft.onnx"
            }
            Self::Demucs6Stems => {
                "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_6s.onnx"
            }
        }
    }

    /// Approximate model size in bytes
    pub fn size_bytes(&self) -> u64 {
        match self {
            Self::Demucs4Stems => 163_000_000,    // ~163MB
            Self::Demucs4StemsFt => 163_000_000,  // ~163MB (same architecture)
            Self::Demucs6Stems => 200_000_000,    // ~200MB
        }
    }

    /// All available models
    pub fn all() -> &'static [Self] {
        &[Self::Demucs4Stems, Self::Demucs4StemsFt, Self::Demucs6Stems]
    }

    /// Number of output stems
    pub fn stem_count(&self) -> usize {
        match self {
            Self::Demucs4Stems => 4,
            Self::Demucs4StemsFt => 4,
            Self::Demucs6Stems => 6,
        }
    }
}
