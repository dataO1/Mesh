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
    /// charon-audio crate - pure Rust implementation
    /// Available when compiled with --features charon-backend
    Charon,

    /// Direct ONNX Runtime via ort crate (recommended)
    #[default]
    OnnxRuntime,
}

impl BackendType {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Charon => "Charon (not ready)",
            Self::OnnxRuntime => "ONNX Runtime",
        }
    }

    /// Description for UI
    pub fn description(&self) -> &'static str {
        match self {
            // charon-audio v0.1.0 has placeholder inference - not usable
            Self::Charon => "charon-audio v0.1.0 inference not implemented yet",
            Self::OnnxRuntime => "Direct ONNX Runtime inference - recommended",
        }
    }

    /// Check if this backend is currently available
    pub fn is_available(&self) -> bool {
        match self {
            // charon-audio v0.1.0 has placeholder inference (returns input copies)
            // Real inference not implemented yet - use OrtBackend instead
            Self::Charon => false,
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
}

impl ModelType {
    /// Display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Demucs 4-stem",
            Self::Demucs4StemsFt => "Demucs 4-stem Fine-tuned",
        }
    }

    /// Description for UI
    pub fn description(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => "Vocals, Drums, Bass, Other - fast (~163MB)",
            Self::Demucs4StemsFt => "Fine-tuned for better quality (~163MB)",
        }
    }

    /// Model filename (must match the name used during ONNX export, since external data
    /// files reference it by name)
    ///
    /// On Windows with DirectML, uses the DirectML-compatible model variants which are
    /// exported with opset 20 and native GroupNormalization instead of InstanceNorm workarounds.
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "htdemucs_directml.onnx"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "htdemucs.onnx"
                }
            }
            Self::Demucs4StemsFt => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "htdemucs_ft_directml.onnx"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "htdemucs_ft.onnx"
                }
            }
        }
    }

    /// Base download URL (GitHub releases) - returns the .onnx file URL
    /// The .onnx.data file is at the same URL with .data appended
    ///
    /// On Windows with DirectML, downloads the DirectML-compatible model variants.
    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_directml.onnx"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs.onnx"
                }
            }
            Self::Demucs4StemsFt => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_ft_directml.onnx"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_ft.onnx"
                }
            }
        }
    }

    /// Whether this model has an external data file (.onnx.data)
    /// ONNX models >2GB use external data storage for weights
    pub fn has_external_data(&self) -> bool {
        // All HTDemucs models use external data storage
        true
    }

    /// External data filename (e.g., "htdemucs.onnx.data")
    ///
    /// On Windows with DirectML, uses the DirectML-compatible model data file.
    pub fn data_filename(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "htdemucs_directml.onnx.data"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "htdemucs.onnx.data"
                }
            }
            Self::Demucs4StemsFt => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "htdemucs_ft_directml.onnx.data"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "htdemucs_ft.onnx.data"
                }
            }
        }
    }

    /// Download URL for the external data file
    ///
    /// On Windows with DirectML, downloads the DirectML-compatible data file.
    pub fn data_download_url(&self) -> &'static str {
        match self {
            Self::Demucs4Stems => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_directml.onnx.data"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs.onnx.data"
                }
            }
            Self::Demucs4StemsFt => {
                #[cfg(all(target_os = "windows", feature = "directml"))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_ft_directml.onnx.data"
                }
                #[cfg(not(all(target_os = "windows", feature = "directml")))]
                {
                    "https://github.com/dataO1/Mesh/releases/download/models/htdemucs_ft.onnx.data"
                }
            }
        }
    }

    /// Approximate model size in bytes (both .onnx and .onnx.data combined)
    pub fn size_bytes(&self) -> u64 {
        match self {
            Self::Demucs4Stems => 163_000_000,   // ~163MB
            Self::Demucs4StemsFt => 163_000_000, // ~163MB (same architecture)
        }
    }

    /// All available models
    pub fn all() -> &'static [Self] {
        &[Self::Demucs4Stems, Self::Demucs4StemsFt]
    }

    /// Number of output stems (always 4 for supported models)
    pub fn stem_count(&self) -> usize {
        4
    }
}
