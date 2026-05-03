//! Loader for the MuQ-MuLan-derived intensity axis JSON.
//!
//! See `documents/aggression-axis-text-tower-plan.md` for the full design.
//! In short: at design time we run MuQ-MuLan's text tower over polar prompts
//! ("aggressive heavy techno" vs "calm peaceful ambient", etc.) and combine
//! the resulting 512-d unit vectors into a single `intensity_axis_vec` per
//! variant. Mesh ships one variant as the active axis (canonical filename
//! `muq-mulan-aggression-axis.json`); all variants live in
//! `models/aggression-axes/` for the eval CLI.
//!
//! The runtime path uses **only** `intensity_axis_vec` — one dot product
//! against the audio embedding. Sub-axes are kept in memory for the
//! diagnostics binaries (`axis_eval`, `aggression_inspect`).

use std::path::Path;
use serde::Deserialize;

/// Required dimensionality of every axis vector. Matches MuQ-MuLan's joint
/// space; loud-fail if a variant JSON disagrees.
const EMBEDDING_DIM: usize = 512;

/// Parsed intensity-axis variant, ready for projection.
#[derive(Debug, Clone, Deserialize)]
pub struct IntensityAxis {
    pub variant_id: String,
    pub name: String,
    pub rationale: String,
    pub model: String,
    pub embedding_dim: usize,
    pub method: String,
    pub intensity_formula: String,
    pub intensity_axis_vec: Vec<f32>,
    pub sub_axes: Vec<SubAxis>,
    pub generated_at: String,
}

/// One named polar sub-axis contributing to the variant's intensity vector.
/// Kept for provenance + diagnostics; never used by the runtime suggestion
/// path (which sees only `IntensityAxis::intensity_axis_vec`).
#[derive(Debug, Clone, Deserialize)]
pub struct SubAxis {
    pub name: String,
    pub axis_vec: Vec<f32>,
    pub prompts_positive: Vec<String>,
    pub prompts_negative: Vec<String>,
    #[serde(default)]
    pub n_positive: usize,
    #[serde(default)]
    pub n_negative: usize,
    #[serde(default)]
    pub weight_in_intensity: f32,
}

impl IntensityAxis {
    /// Read + validate a variant JSON from disk.
    ///
    /// Validates: embedding_dim==512, intensity_axis_vec length matches,
    /// intensity_axis_vec is unit-norm within tolerance, and each sub_axis
    /// vector is also unit-norm within tolerance. Loud-fails otherwise —
    /// silently mis-projecting onto a non-unit axis would skew rankings.
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("read {:?}: {}", path, e))?;
        let axis: Self = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {:?}: {}", path, e))?;
        axis.validate(path)?;
        Ok(axis)
    }

    fn validate(&self, src: &Path) -> Result<(), String> {
        if self.embedding_dim != EMBEDDING_DIM {
            return Err(format!(
                "{:?}: embedding_dim {} ≠ {}",
                src, self.embedding_dim, EMBEDDING_DIM
            ));
        }
        if self.intensity_axis_vec.len() != EMBEDDING_DIM {
            return Err(format!(
                "{:?}: intensity_axis_vec length {} ≠ {}",
                src, self.intensity_axis_vec.len(), EMBEDDING_DIM
            ));
        }
        let norm = vector_norm(&self.intensity_axis_vec);
        if (norm - 1.0).abs() > 1e-3 {
            return Err(format!(
                "{:?}: intensity_axis_vec norm {} ≠ 1.0 (variant generator did not l2-normalise)",
                src, norm
            ));
        }
        for sub in &self.sub_axes {
            if sub.axis_vec.len() != EMBEDDING_DIM {
                return Err(format!(
                    "{:?}: sub_axis '{}' length {} ≠ {}",
                    src, sub.name, sub.axis_vec.len(), EMBEDDING_DIM
                ));
            }
            let sn = vector_norm(&sub.axis_vec);
            if (sn - 1.0).abs() > 1e-3 {
                return Err(format!(
                    "{:?}: sub_axis '{}' norm {} ≠ 1.0",
                    src, sub.name, sn
                ));
            }
        }
        Ok(())
    }

    /// Project a 512-d audio embedding onto the intensity axis.
    /// Returns a scalar roughly in [-1, 1] (since both vectors are unit-norm).
    /// Caller is responsible for percentile-ranking across the library if a
    /// normalised score is wanted.
    pub fn project(&self, audio_embedding: &[f32]) -> f32 {
        if audio_embedding.len() != EMBEDDING_DIM {
            return 0.0;
        }
        audio_embedding
            .iter()
            .zip(self.intensity_axis_vec.iter())
            .map(|(a, b)| a * b)
            .sum()
    }

    /// Project onto every sub-axis (for diagnostics + the eval CLI).
    /// Returned tuples are `(sub_axis_name, projection_score)`.
    pub fn project_sub_axes(&self, audio_embedding: &[f32]) -> Vec<(String, f32)> {
        if audio_embedding.len() != EMBEDDING_DIM {
            return Vec::new();
        }
        self.sub_axes
            .iter()
            .map(|sub| {
                let score: f32 = audio_embedding
                    .iter()
                    .zip(sub.axis_vec.iter())
                    .map(|(a, b)| a * b)
                    .sum();
                (sub.name.clone(), score)
            })
            .collect()
    }
}

fn vector_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_axis() -> Vec<f32> {
        let mut v = vec![0.0; 512];
        v[0] = 1.0;
        v
    }

    #[test]
    fn validate_accepts_unit_norm_axis() {
        let axis = IntensityAxis {
            variant_id: "T1".into(),
            name: "test".into(),
            rationale: "test".into(),
            model: "test".into(),
            embedding_dim: 512,
            method: "test".into(),
            intensity_formula: "test".into(),
            intensity_axis_vec: unit_axis(),
            sub_axes: vec![],
            generated_at: "test".into(),
        };
        assert!(axis.validate(Path::new("memory")).is_ok());
    }

    #[test]
    fn validate_rejects_wrong_dim() {
        let axis = IntensityAxis {
            variant_id: "T1".into(),
            name: "test".into(),
            rationale: "test".into(),
            model: "test".into(),
            embedding_dim: 256,
            method: "test".into(),
            intensity_formula: "test".into(),
            intensity_axis_vec: vec![0.0; 256],
            sub_axes: vec![],
            generated_at: "test".into(),
        };
        let err = axis.validate(Path::new("memory")).unwrap_err();
        assert!(err.contains("embedding_dim"));
    }

    #[test]
    fn validate_rejects_non_unit_norm() {
        let mut bad = unit_axis();
        bad[0] = 2.0;
        let axis = IntensityAxis {
            variant_id: "T1".into(),
            name: "test".into(),
            rationale: "test".into(),
            model: "test".into(),
            embedding_dim: 512,
            method: "test".into(),
            intensity_formula: "test".into(),
            intensity_axis_vec: bad,
            sub_axes: vec![],
            generated_at: "test".into(),
        };
        let err = axis.validate(Path::new("memory")).unwrap_err();
        assert!(err.contains("norm"));
    }

    #[test]
    fn project_returns_dot_product() {
        let mut audio = vec![0.0; 512];
        audio[0] = 0.5;
        audio[1] = 0.5;
        let axis = IntensityAxis {
            variant_id: "T1".into(),
            name: "test".into(),
            rationale: "test".into(),
            model: "test".into(),
            embedding_dim: 512,
            method: "test".into(),
            intensity_formula: "test".into(),
            intensity_axis_vec: unit_axis(),
            sub_axes: vec![],
            generated_at: "test".into(),
        };
        assert!((axis.project(&audio) - 0.5).abs() < 1e-6);
    }
}
