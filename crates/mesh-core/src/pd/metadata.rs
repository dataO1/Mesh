//! Effect metadata parsing
//!
//! Handles parsing of `metadata.json` files that describe PD effects.
//! Metadata provides information about the effect's name, category,
//! latency, required externals, and parameters.

use serde::Deserialize;
use std::path::Path;

use super::error::{PdError, PdResult};

/// Metadata for a single effect parameter
#[derive(Debug, Clone, Deserialize)]
pub struct ParamMetadata {
    /// Parameter display name
    pub name: String,

    /// Default value (0.0-1.0, normalized)
    #[serde(default = "default_param_value")]
    pub default: f32,

    /// Minimum actual value (for display only, PD receives normalized)
    #[serde(default)]
    pub min: Option<f32>,

    /// Maximum actual value (for display only, PD receives normalized)
    #[serde(default)]
    pub max: Option<f32>,

    /// Unit label (e.g., "ms", "Hz", "%")
    #[serde(default)]
    pub unit: Option<String>,
}

fn default_param_value() -> f32 {
    0.5
}

impl Default for ParamMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            default: 0.5,
            min: None,
            max: None,
            unit: None,
        }
    }
}

/// Complete metadata for a PD effect
#[derive(Debug, Clone, Deserialize)]
pub struct EffectMetadata {
    /// Effect display name
    pub name: String,

    /// Effect category (e.g., "Neural", "Filter", "Delay")
    pub category: String,

    /// Effect author (optional)
    #[serde(default)]
    pub author: Option<String>,

    /// Effect version (optional)
    #[serde(default)]
    pub version: Option<String>,

    /// Effect description (optional)
    #[serde(default)]
    pub description: Option<String>,

    /// Fixed latency in samples at the specified sample rate
    pub latency_samples: u32,

    /// Sample rate the latency_samples value is specified for
    /// Defaults to 48000 Hz (mesh standard)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// List of required PD externals (e.g., ["nn~"])
    /// These must exist in effects/externals/
    #[serde(default)]
    pub requires_externals: Vec<String>,

    /// Effect parameters (up to 8)
    #[serde(default)]
    pub params: Vec<ParamMetadata>,
}

fn default_sample_rate() -> u32 {
    48000
}

impl EffectMetadata {
    /// Load metadata from a JSON file
    pub fn from_file(path: &Path) -> PdResult<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content, path)
    }

    /// Parse metadata from JSON string
    pub fn from_json(json: &str, source_path: &Path) -> PdResult<Self> {
        let metadata: EffectMetadata = serde_json::from_str(json).map_err(|e| {
            PdError::InvalidMetadata {
                effect_id: source_path.display().to_string(),
                reason: e.to_string(),
            }
        })?;

        // Validate
        metadata.validate(source_path)?;

        Ok(metadata)
    }

    /// Validate metadata constraints
    fn validate(&self, source_path: &Path) -> PdResult<()> {
        let effect_id = source_path.display().to_string();

        // Name must not be empty
        if self.name.trim().is_empty() {
            return Err(PdError::InvalidMetadata {
                effect_id,
                reason: "name cannot be empty".to_string(),
            });
        }

        // Category must not be empty
        if self.category.trim().is_empty() {
            return Err(PdError::InvalidMetadata {
                effect_id,
                reason: "category cannot be empty".to_string(),
            });
        }

        // Max 8 parameters
        if self.params.len() > 8 {
            return Err(PdError::InvalidMetadata {
                effect_id,
                reason: format!(
                    "too many parameters: {} (max 8)",
                    self.params.len()
                ),
            });
        }

        // Validate parameter defaults are in range
        for (i, param) in self.params.iter().enumerate() {
            if param.default < 0.0 || param.default > 1.0 {
                return Err(PdError::InvalidMetadata {
                    effect_id,
                    reason: format!(
                        "param {} '{}' has invalid default {}: must be 0.0-1.0",
                        i, param.name, param.default
                    ),
                });
            }
        }

        Ok(())
    }

    /// Get latency in samples scaled to the target sample rate
    pub fn latency_at_sample_rate(&self, target_sample_rate: u32) -> u32 {
        if self.sample_rate == target_sample_rate {
            self.latency_samples
        } else {
            // Scale latency proportionally
            let scale = target_sample_rate as f64 / self.sample_rate as f64;
            (self.latency_samples as f64 * scale).round() as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parse_minimal_metadata() {
        let json = r#"{
            "name": "Test Effect",
            "category": "Test",
            "latency_samples": 64
        }"#;

        let metadata = EffectMetadata::from_json(json, &PathBuf::from("test.json")).unwrap();
        assert_eq!(metadata.name, "Test Effect");
        assert_eq!(metadata.category, "Test");
        assert_eq!(metadata.latency_samples, 64);
        assert_eq!(metadata.sample_rate, 48000); // default
        assert!(metadata.params.is_empty());
        assert!(metadata.requires_externals.is_empty());
    }

    #[test]
    fn test_parse_full_metadata() {
        let json = r#"{
            "name": "RAVE Percussion",
            "category": "Neural",
            "author": "mesh",
            "version": "1.0.0",
            "description": "Neural timbral transformation",
            "latency_samples": 4096,
            "sample_rate": 48000,
            "requires_externals": ["nn~"],
            "params": [
                { "name": "L1", "default": 0.5 },
                { "name": "L2", "default": 0.0, "min": -10.0, "max": 10.0 }
            ]
        }"#;

        let metadata = EffectMetadata::from_json(json, &PathBuf::from("test.json")).unwrap();
        assert_eq!(metadata.name, "RAVE Percussion");
        assert_eq!(metadata.requires_externals, vec!["nn~"]);
        assert_eq!(metadata.params.len(), 2);
        assert_eq!(metadata.params[0].name, "L1");
        assert_eq!(metadata.params[1].min, Some(-10.0));
    }

    #[test]
    fn test_latency_scaling() {
        let json = r#"{
            "name": "Test",
            "category": "Test",
            "latency_samples": 4410,
            "sample_rate": 44100
        }"#;

        let metadata = EffectMetadata::from_json(json, &PathBuf::from("test.json")).unwrap();

        // At 44100, latency is 4410 samples (100ms)
        assert_eq!(metadata.latency_at_sample_rate(44100), 4410);

        // At 48000, should scale to 4800 samples (still 100ms)
        assert_eq!(metadata.latency_at_sample_rate(48000), 4800);
    }

    #[test]
    fn test_reject_too_many_params() {
        let json = r#"{
            "name": "Test",
            "category": "Test",
            "latency_samples": 64,
            "params": [
                { "name": "P1" }, { "name": "P2" }, { "name": "P3" }, { "name": "P4" },
                { "name": "P5" }, { "name": "P6" }, { "name": "P7" }, { "name": "P8" },
                { "name": "P9" }
            ]
        }"#;

        let result = EffectMetadata::from_json(json, &PathBuf::from("test.json"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too many parameters"));
    }

    #[test]
    fn test_reject_invalid_default() {
        let json = r#"{
            "name": "Test",
            "category": "Test",
            "latency_samples": 64,
            "params": [{ "name": "P1", "default": 1.5 }]
        }"#;

        let result = EffectMetadata::from_json(json, &PathBuf::from("test.json"));
        assert!(result.is_err());
    }
}
