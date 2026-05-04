//! IntensityAxis loader + projection — shared between mesh-cue (analysis)
//! and mesh-player (runtime ranking).
//!
//! See `documents/intensity-axis-pipeline-runbook.md` for the full pipeline.
//! At deploy time we ship a single JSON describing a 512-d unit vector in
//! MuQ-MuLan's joint audio/text space. Per-track intensity = dot(axis, emb).
//!
//! Currently shipping V15 (linear probe trained on round-5 BT priors) at
//! `<collection_root>/muq-mulan-aggression-axis.json`. The runtime loads
//! this file once, holds it in memory, and projects per-track on demand.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

/// Required dimensionality of every axis vector. Matches MuQ-MuLan's joint
/// space; loud-fail if a variant JSON disagrees.
pub const EMBEDDING_DIM: usize = 512;

/// Canonical filename used in both the shipped models dir and the per-
/// collection override location.
pub const AXIS_FILENAME: &str = "muq-mulan-aggression-axis.json";

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

/// One named polar sub-axis. Kept for sub-control UI ("less distorted",
/// "darker", etc.) — not used by the headline intensity projection.
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
                "{:?}: intensity_axis_vec norm {} ≠ 1.0",
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
    /// Returns a scalar; both vectors are unit-norm so result is in [-1, 1].
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

    /// Project onto every sub-axis (for diagnostics + sub-control UI).
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

/// Per-collection runtime intensity provider. Loaded once at startup and
/// shared between threads via `Arc`. Projection is deterministic + cheap
/// (one dot product), so callers project on demand rather than caching
/// per-track results.
#[derive(Debug, Clone)]
pub struct IntensityProvider {
    pub axis: Arc<IntensityAxis>,
}

impl IntensityProvider {
    /// Resolve and load the axis from the collection folder. Falls back to
    /// the cache path if the per-collection file is absent.
    pub fn load_for_collection(collection_root: &Path) -> Result<Self, String> {
        let candidate = collection_root.join(AXIS_FILENAME);
        if candidate.exists() {
            let axis = IntensityAxis::load(&candidate)?;
            log::info!(
                "Loaded intensity axis '{}' ({}) from {:?}",
                axis.variant_id, axis.name, candidate
            );
            return Ok(Self { axis: Arc::new(axis) });
        }
        Err(format!("no intensity axis at {:?}", candidate))
    }

    /// Project a single embedding through the loaded axis.
    pub fn project(&self, emb: &[f32]) -> f32 {
        self.axis.project(emb)
    }

    /// Project a normalised intensity in [0, 1] (for compositing with other
    /// per-track signals). Maps the raw [-1, 1] cosine into [0, 1] via
    /// `(x + 1) / 2`.
    pub fn project_normalised(&self, emb: &[f32]) -> f32 {
        ((self.axis.project(emb) + 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

/// Batch-project a list of (track_id, embedding) pairs, returning the
/// normalised intensity per track. Skips tracks whose embedding length
/// is not 512 (likely a dim-migration in flight).
pub fn batch_project_normalised(
    provider: &IntensityProvider,
    items: impl IntoIterator<Item = (i64, Vec<f32>)>,
) -> HashMap<i64, f32> {
    let mut out = HashMap::new();
    for (tid, emb) in items {
        if emb.len() != EMBEDDING_DIM {
            continue;
        }
        out.insert(tid, provider.project_normalised(&emb));
    }
    out
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
    fn project_returns_dot_product() {
        let axis = IntensityAxis {
            variant_id: "T".into(),
            name: "T".into(),
            rationale: "T".into(),
            model: "T".into(),
            embedding_dim: 512,
            method: "T".into(),
            intensity_formula: "T".into(),
            intensity_axis_vec: unit_axis(),
            sub_axes: vec![],
            generated_at: "T".into(),
        };
        let mut emb = vec![0.0; 512];
        emb[0] = 0.5;
        assert!((axis.project(&emb) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn provider_normalises_to_unit_interval() {
        let provider = IntensityProvider {
            axis: Arc::new(IntensityAxis {
                variant_id: "T".into(),
                name: "T".into(),
                rationale: "T".into(),
                model: "T".into(),
                embedding_dim: 512,
                method: "T".into(),
                intensity_formula: "T".into(),
                intensity_axis_vec: unit_axis(),
                sub_axes: vec![],
                generated_at: "T".into(),
            }),
        };
        let mut emb = vec![0.0; 512];
        emb[0] = 1.0;
        assert!((provider.project_normalised(&emb) - 1.0).abs() < 1e-6);
        emb[0] = -1.0;
        assert!(provider.project_normalised(&emb).abs() < 1e-6);
        emb[0] = 0.0;
        assert!((provider.project_normalised(&emb) - 0.5).abs() < 1e-6);
    }
}
