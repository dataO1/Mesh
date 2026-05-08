//! IntensityAxis loader + projection — shared between mesh-cue (analysis)
//! and mesh-player (runtime ranking).
//!
//! See `documents/intensity-axis-pipeline-runbook.md` for the pipeline and
//! `documents/round-7-6-pipeline-spec.md` for V18+ details.
//!
//! Two on-disk schemas are supported (auto-detected at load):
//!
//! - **V15-class schema** (`variant_id`, `intensity_axis_vec`, `sub_axes`,
//!   `intensity_formula`, …). L2-unit-norm 512-d projection vector. Was
//!   the deployed shape for round-6/round-7.5. Sub-axes were forward-
//!   looking for a "more dark / less distorted" UI that never shipped.
//!   `project()` is a single dot product.
//!
//! - **V18-class schema** (`version`, `model_type ∈ {linear, mlp}`,
//!   `intensity_axis_vec` + `bias` for linear, or
//!   `mlp.{W1, b1, W2, b2}` + `mlp.activation` for MLP).
//!   Round-7.6 V18 onwards. The MLP variant is V18.1, which closed
//!   ~0.6 pp on V18 by escalating the student to a 2-layer MLP per
//!   spec §765-768.
//!
//! V18+ files don't carry sub-axes; round-7.6 didn't produce them, and
//! they were never used at runtime anyway. The Rust struct keeps the
//! field as `Vec<SubAxis>` defaulted to empty for back-compat with the
//! `axis_eval` dev binary.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

/// Required dimensionality of every audio embedding. Matches MuQ-MuLan's
/// joint space; loud-fail if a variant disagrees on input dim.
pub const EMBEDDING_DIM: usize = 512;

/// Canonical filename used in both the shipped models dir and the per-
/// collection override location.
pub const AXIS_FILENAME: &str = "muq-mulan-aggression-axis.json";

/// Discriminated union of supported model kinds. Picked at load time.
#[derive(Debug, Clone)]
pub enum ModelKind {
    /// `intensity = audio_emb · vec + bias`. Used by V15 (no bias) and
    /// V18 linear (with bias). Matches V15's `intensity_axis_vec` field
    /// after we fold in the L2 norm + zero-bias defaults.
    Linear { vec: Vec<f32>, bias: f32 },

    /// Two-layer MLP: `intensity = W2 · gelu(W1 · audio_emb + b1) + b2`.
    /// `W1` is row-major `[hidden][in_dim]`; `b1` is `[hidden]`; `W2` is
    /// row-major `[1][hidden]`; `b2` is scalar. Used by V18.1.
    Mlp {
        w1: Vec<Vec<f32>>,
        b1: Vec<f32>,
        w2: Vec<f32>,         // (1, hidden) flattened to (hidden,)
        b2: f32,
    },
}

/// Parsed intensity-axis variant, ready for projection.
///
/// The V15-class fields (`name`, `rationale`, `intensity_formula`, etc.)
/// are kept on the outer struct so dev tooling like `axis_eval` doesn't
/// need to learn the discriminated-union shape; for V18+ files we synthesise
/// reasonable defaults from the V18 JSON's `version` field.
#[derive(Debug, Clone)]
pub struct IntensityAxis {
    pub variant_id: String,
    pub name: String,
    pub rationale: String,
    pub model: String,
    pub embedding_dim: usize,
    pub method: String,
    pub intensity_formula: String,
    pub sub_axes: Vec<SubAxis>,
    pub generated_at: String,
    pub model_kind: ModelKind,
}

/// One named polar sub-axis. Empty for V18+ — round-7.6 didn't produce
/// these and they were never wired into the runtime UI anyway. Kept on
/// the type so `axis_eval` and any future "darker / less distorted"
/// slider UI can still read them when an old V15-class file is loaded.
#[derive(Debug, Clone, Deserialize)]
pub struct SubAxis {
    pub name: String,
    pub axis_vec: Vec<f32>,
    #[serde(default)]
    pub prompts_positive: Vec<String>,
    #[serde(default)]
    pub prompts_negative: Vec<String>,
    #[serde(default)]
    pub n_positive: usize,
    #[serde(default)]
    pub n_negative: usize,
    #[serde(default)]
    pub weight_in_intensity: f32,
}

// --- Raw on-disk schemas (one per supported variant). The public
// `IntensityAxis` is built from one of these depending on which fields
// the JSON had. ----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct V15Raw {
    variant_id: String,
    name: String,
    rationale: String,
    model: String,
    embedding_dim: usize,
    method: String,
    intensity_formula: String,
    intensity_axis_vec: Vec<f32>,
    #[serde(default)]
    sub_axes: Vec<SubAxis>,
    generated_at: String,
}

#[derive(Debug, Deserialize)]
struct V18Raw {
    version: String,
    embedding: String,
    embedding_dim: usize,
    #[serde(default = "default_model_type_linear")]
    model_type: String,
    // Linear-only fields (model_type == "linear"):
    #[serde(default)]
    intensity_axis_vec: Option<Vec<f32>>,
    #[serde(default)]
    bias: Option<f32>,
    // MLP-only fields (model_type == "mlp"):
    #[serde(default)]
    mlp: Option<V18MlpRaw>,
    #[serde(default)]
    trained_at: String,
}

fn default_model_type_linear() -> String { "linear".to_string() }

#[derive(Debug, Deserialize)]
struct V18MlpRaw {
    hidden_dim: usize,
    activation: String,
    #[serde(rename = "W1")]
    w1: Vec<Vec<f32>>,
    #[serde(rename = "b1")]
    b1: Vec<f32>,
    #[serde(rename = "W2")]
    w2: Vec<Vec<f32>>,
    #[serde(rename = "b2")]
    b2: f32,
}

// ---------------------------------------------------------------------

impl IntensityAxis {
    /// Read + validate a variant JSON from disk. Auto-detects V15-class
    /// vs V18-class schema by inspecting the top-level keys.
    pub fn load(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("read {:?}: {}", path, e))?;

        // Peek at the JSON to decide which schema to parse against. V15
        // has `variant_id`; V18 has `version` (and usually `model_type`).
        let head: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {:?}: {}", path, e))?;
        let is_v18 = head.get("version").is_some()
            && head.get("variant_id").is_none();

        let axis = if is_v18 {
            let v: V18Raw = serde_json::from_str(&raw)
                .map_err(|e| format!("parse v18 {:?}: {}", path, e))?;
            Self::from_v18(v, path)?
        } else {
            let v: V15Raw = serde_json::from_str(&raw)
                .map_err(|e| format!("parse v15 {:?}: {}", path, e))?;
            Self::from_v15(v, path)?
        };
        Ok(axis)
    }

    fn from_v15(v: V15Raw, src: &Path) -> Result<Self, String> {
        if v.embedding_dim != EMBEDDING_DIM {
            return Err(format!("{:?}: embedding_dim {} ≠ {}", src, v.embedding_dim, EMBEDDING_DIM));
        }
        if v.intensity_axis_vec.len() != EMBEDDING_DIM {
            return Err(format!(
                "{:?}: intensity_axis_vec length {} ≠ {}",
                src, v.intensity_axis_vec.len(), EMBEDDING_DIM
            ));
        }
        let norm = vector_norm(&v.intensity_axis_vec);
        if (norm - 1.0).abs() > 1e-3 {
            return Err(format!("{:?}: intensity_axis_vec norm {} ≠ 1.0", src, norm));
        }
        for sub in &v.sub_axes {
            if sub.axis_vec.len() != EMBEDDING_DIM {
                return Err(format!(
                    "{:?}: sub_axis '{}' length {} ≠ {}",
                    src, sub.name, sub.axis_vec.len(), EMBEDDING_DIM
                ));
            }
            let sn = vector_norm(&sub.axis_vec);
            if (sn - 1.0).abs() > 1e-3 {
                return Err(format!(
                    "{:?}: sub_axis '{}' norm {} ≠ 1.0", src, sub.name, sn
                ));
            }
        }
        Ok(Self {
            variant_id: v.variant_id,
            name: v.name,
            rationale: v.rationale,
            model: v.model,
            embedding_dim: v.embedding_dim,
            method: v.method,
            intensity_formula: v.intensity_formula,
            sub_axes: v.sub_axes,
            generated_at: v.generated_at,
            model_kind: ModelKind::Linear { vec: v.intensity_axis_vec, bias: 0.0 },
        })
    }

    fn from_v18(v: V18Raw, src: &Path) -> Result<Self, String> {
        if v.embedding_dim != EMBEDDING_DIM {
            return Err(format!("{:?}: embedding_dim {} ≠ {}", src, v.embedding_dim, EMBEDDING_DIM));
        }
        let model_kind = match v.model_type.as_str() {
            "linear" => {
                let vec = v.intensity_axis_vec.ok_or_else(|| format!(
                    "{:?}: model_type=linear but intensity_axis_vec missing", src))?;
                if vec.len() != EMBEDDING_DIM {
                    return Err(format!(
                        "{:?}: intensity_axis_vec length {} ≠ {}",
                        src, vec.len(), EMBEDDING_DIM));
                }
                let bias = v.bias.unwrap_or(0.0);
                ModelKind::Linear { vec, bias }
            }
            "mlp" => {
                let mlp = v.mlp.ok_or_else(|| format!(
                    "{:?}: model_type=mlp but mlp block missing", src))?;
                if mlp.activation.to_lowercase() != "gelu" {
                    return Err(format!(
                        "{:?}: unsupported activation '{}' (only 'gelu' is recognised)",
                        src, mlp.activation));
                }
                if mlp.w1.len() != mlp.hidden_dim {
                    return Err(format!(
                        "{:?}: mlp.W1 has {} rows, expected hidden_dim={}",
                        src, mlp.w1.len(), mlp.hidden_dim));
                }
                for (i, row) in mlp.w1.iter().enumerate() {
                    if row.len() != EMBEDDING_DIM {
                        return Err(format!(
                            "{:?}: mlp.W1[{}] has {} cols, expected {}",
                            src, i, row.len(), EMBEDDING_DIM));
                    }
                }
                if mlp.b1.len() != mlp.hidden_dim {
                    return Err(format!(
                        "{:?}: mlp.b1 has {} elements, expected hidden_dim={}",
                        src, mlp.b1.len(), mlp.hidden_dim));
                }
                if mlp.w2.len() != 1 || mlp.w2[0].len() != mlp.hidden_dim {
                    return Err(format!(
                        "{:?}: mlp.W2 shape {:?}, expected (1, {})",
                        src, (mlp.w2.len(), mlp.w2.first().map(|r| r.len()).unwrap_or(0)),
                        mlp.hidden_dim));
                }
                ModelKind::Mlp {
                    w1: mlp.w1,
                    b1: mlp.b1,
                    w2: mlp.w2.into_iter().next().unwrap(),
                    b2: mlp.b2,
                }
            }
            other => return Err(format!("{:?}: unknown model_type '{}'", src, other)),
        };

        // V18 files don't carry V15-class metadata fields. Synthesise
        // reasonable defaults so downstream `axis.name` etc. accesses
        // keep working.
        Ok(Self {
            variant_id: v.version.clone(),
            name: v.version.clone(),
            rationale: format!("Round-7.6 V18+ {} model over {}",
                               v.model_type, v.embedding),
            model: v.embedding,
            embedding_dim: v.embedding_dim,
            method: format!("v18_{}", v.model_type),
            intensity_formula: match &v.model_type[..] {
                "linear" => "audio_emb @ vec + bias".to_string(),
                "mlp" => "W2 @ gelu(W1 @ audio_emb + b1) + b2".to_string(),
                _ => "?".to_string(),
            },
            sub_axes: Vec::new(),
            generated_at: v.trained_at,
            model_kind,
        })
    }

    /// Project a 512-d audio embedding onto the intensity axis. The return
    /// value's range depends on the variant:
    ///
    /// - V15 / Linear with unit-norm vec: result ∈ [-1, 1] (cosine).
    /// - V18 Linear or MLP: result is on whatever scale the training
    ///   produced — typically clustered around the consensus' [0, 1]
    ///   range with some spillover. Use `project_normalised` to clamp.
    pub fn project(&self, audio_embedding: &[f32]) -> f32 {
        if audio_embedding.len() != EMBEDDING_DIM {
            return 0.0;
        }
        match &self.model_kind {
            ModelKind::Linear { vec, bias } => {
                let dot: f32 = audio_embedding.iter().zip(vec.iter()).map(|(a, b)| a * b).sum();
                dot + bias
            }
            ModelKind::Mlp { w1, b1, w2, b2 } => {
                // h = W1 @ x + b1
                let hidden = w1.len();
                let mut h = vec![0f32; hidden];
                for (i, row) in w1.iter().enumerate() {
                    let mut acc = b1[i];
                    for (j, &xj) in audio_embedding.iter().enumerate() {
                        acc += row[j] * xj;
                    }
                    h[i] = acc;
                }
                // h = gelu(h)
                for hi in h.iter_mut() {
                    *hi = gelu(*hi);
                }
                // y = w2 · h + b2
                let mut y = *b2;
                for (wi, hi) in w2.iter().zip(h.iter()) {
                    y += wi * hi;
                }
                y
            }
        }
    }

    /// Return the underlying linear projection vector, or `None` if the
    /// model is non-linear (V18.1 MLP). Use this when downstream code
    /// genuinely needs a `Vec<f32>` (e.g. the V11-era pca_aggression_axis
    /// cache table that stores per-dimension weights). For projection,
    /// always prefer `project()` which dispatches over the model kind.
    pub fn as_linear_vec(&self) -> Option<&[f32]> {
        match &self.model_kind {
            ModelKind::Linear { vec, .. } => Some(vec.as_slice()),
            ModelKind::Mlp { .. } => None,
        }
    }

    /// Project onto every sub-axis. Empty for V18+ (no sub-axes).
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
/// shared between threads via `Arc`. Projection is deterministic and cheap
/// (one dot product for V15/V18-linear, ~70k FMAs for V18.1 MLP h=128 —
/// still well under a microsecond per track), so callers project on demand
/// rather than caching per-track results.
#[derive(Debug, Clone)]
pub struct IntensityProvider {
    pub axis: Arc<IntensityAxis>,
}

impl IntensityProvider {
    /// Resolve and load the axis from the collection folder.
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

    /// Project a normalised intensity in [0, 1]. For V15 (cosine-shaped),
    /// maps [-1, 1] → [0, 1] via `(x + 1) / 2`. For V18+ (already roughly
    /// in [0, 1] from the regression target), just clamps.
    pub fn project_normalised(&self, emb: &[f32]) -> f32 {
        let raw = self.axis.project(emb);
        let mapped = match &self.axis.model_kind {
            ModelKind::Linear { bias, .. } if *bias == 0.0 => (raw + 1.0) * 0.5,
            _ => raw,
        };
        mapped.clamp(0.0, 1.0)
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

/// Exact GELU activation (matches torch.nn.GELU default `approximate='none'`):
/// `gelu(x) = 0.5 * x * (1 + erf(x / sqrt(2)))`.
///
/// Uses libm-free Abramowitz-Stegun erf approximation (max abs error ~1.5e-7),
/// well below the f32 precision needed for the V18.1 student. Deterministic.
fn gelu(x: f32) -> f32 {
    const SQRT_2: f32 = std::f32::consts::SQRT_2;
    0.5 * x * (1.0 + erf(x / SQRT_2))
}

/// erf approximation per Abramowitz & Stegun 7.1.26 (max |err| ≈ 1.5e-7).
/// Matches Python's `scipy.special.erf` and `torch.erf` to f32 precision.
fn erf(x: f32) -> f32 {
    // Constants from A&S 7.1.26.
    const A1: f32 = 0.254829592;
    const A2: f32 = -0.284496736;
    const A3: f32 = 1.421413741;
    const A4: f32 = -1.453152027;
    const A5: f32 = 1.061405429;
    const P: f32 = 0.3275911;
    let sign = if x >= 0.0 { 1.0 } else { -1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + P * ax);
    let y = 1.0 - (((((A5 * t + A4) * t) + A3) * t + A2) * t + A1) * t * (-ax * ax).exp();
    sign * y
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
    fn project_returns_dot_product_v15() {
        let axis = IntensityAxis {
            variant_id: "T".into(),
            name: "T".into(),
            rationale: "T".into(),
            model: "T".into(),
            embedding_dim: 512,
            method: "T".into(),
            intensity_formula: "T".into(),
            sub_axes: vec![],
            generated_at: "T".into(),
            model_kind: ModelKind::Linear { vec: unit_axis(), bias: 0.0 },
        };
        let mut emb = vec![0.0; 512];
        emb[0] = 0.5;
        assert!((axis.project(&emb) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn project_v18_linear_with_bias() {
        let axis = IntensityAxis {
            variant_id: "V18".into(),
            name: "V18".into(),
            rationale: "T".into(),
            model: "muq-mulan".into(),
            embedding_dim: 512,
            method: "v18_linear".into(),
            intensity_formula: "audio_emb @ vec + bias".into(),
            sub_axes: vec![],
            generated_at: "T".into(),
            model_kind: ModelKind::Linear { vec: unit_axis(), bias: 0.25 },
        };
        let mut emb = vec![0.0; 512];
        emb[0] = 0.5;
        assert!((axis.project(&emb) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn project_v18_mlp() {
        // tiny MLP: hidden=2, in_dim=512, all-zero except first column.
        // x = [1, 0, ...] → W1 x = [1, -1] → +b1 = [2, 0]
        // gelu([2, 0]) = [≈1.954, 0]
        // W2 [1, 1] · [1.954, 0] = 1.954, +b2=0.05 ≈ 2.004
        let mut w1_row0 = vec![0.0; 512]; w1_row0[0] = 1.0;
        let mut w1_row1 = vec![0.0; 512]; w1_row1[0] = -1.0;
        let axis = IntensityAxis {
            variant_id: "V18.1".into(),
            name: "V18.1".into(),
            rationale: "T".into(),
            model: "muq-mulan".into(),
            embedding_dim: 512,
            method: "v18_mlp".into(),
            intensity_formula: "T".into(),
            sub_axes: vec![],
            generated_at: "T".into(),
            model_kind: ModelKind::Mlp {
                w1: vec![w1_row0, w1_row1],
                b1: vec![1.0, 1.0],
                w2: vec![1.0, 1.0],
                b2: 0.05,
            },
        };
        let mut emb = vec![0.0; 512];
        emb[0] = 1.0;
        let y = axis.project(&emb);
        // gelu(2) ≈ 1.9545, gelu(0) = 0.
        let expected = 1.0 * gelu(2.0) + 1.0 * gelu(0.0) + 0.05;
        assert!((y - expected).abs() < 1e-5,
                "got {}, expected {}", y, expected);
    }

    #[test]
    fn provider_normalises_to_unit_interval_v15() {
        let provider = IntensityProvider {
            axis: Arc::new(IntensityAxis {
                variant_id: "T".into(),
                name: "T".into(),
                rationale: "T".into(),
                model: "T".into(),
                embedding_dim: 512,
                method: "T".into(),
                intensity_formula: "T".into(),
                sub_axes: vec![],
                generated_at: "T".into(),
                model_kind: ModelKind::Linear { vec: unit_axis(), bias: 0.0 },
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

    #[test]
    fn gelu_matches_torch_to_f32() {
        // Spot-check a few values against torch.nn.functional.gelu (no approx).
        // The Abramowitz-Stegun erf approximation has max error ~1.5e-7 on
        // erf itself, but the multiplications in `gelu = 0.5x(1+erf(x/√2))`
        // can stack to ~5e-5 worst case at moderate |x|. Tolerance is set
        // to 1e-4 which is still 100× tighter than the consensus' 1/20-bucket
        // (5e-2) resolution and the per-track Spearman noise floor.
        for (x, expected) in [(0.0_f32, 0.0_f32), (1.0, 0.8413448), (2.0, 1.9545977),
                              (-1.0, -0.15865526), (3.0, 2.9959502)] {
            let y: f32 = gelu(x);
            assert!((y - expected).abs() < 1e-4, "gelu({}) = {} != {}", x, y, expected);
        }
    }
}
