//! ONNX-based ML inference using the MuQ-MuLan-large audio tower.
//!
//! Runs the exported MuQ-MuLan audio side via `ort` (ONNX Runtime). Each
//! `MlAnalyzer` owns a pre-loaded session plus the mel-normalization stats
//! pulled from the `*.norm.json` sidecar that ships next to the ONNX.
//!
//! # Architecture
//!
//! MuQ-MuLan is a CLIP-style dual encoder (audio + text). We export only
//! the audio side; one forward pass on a 10 s clip's normalized mel
//! produces a 512-dim joint-space embedding (l2-normalized). The text
//! tower is deferred to a follow-up.
//!
//! Per-track flow (`analyze`):
//!
//!   1. Slice the precomputed mel into evenly-spaced 10 s clips
//!      (1000 frames each at 24 kHz / hop 240).
//!   2. For each clip: apply `(mel - mean) / std` from the sidecar stats,
//!      run ONNX, collect the 512-d output.
//!   3. Average clip embeddings → one 512-d vector per track.
//!
//! Genre / classification heads from the prior MAEST integration have
//! been removed — MuQ-MuLan emits embeddings only.

use std::path::Path;
use ndarray::Array3;
use ort::session::Session;
use ort::value::Tensor;
use mesh_core::db::MlAnalysisData;
use serde::Deserialize;

use super::models::MlModelType;
use super::preprocessing::{MelSpectrogramResult, MUQ_HOP, MUQ_N_MELS, MUQ_TARGET_SR};

/// Combined result of a full ML analysis run.
///
/// `data` carries the `MlAnalysisData` shape the rest of the codebase
/// expects (genre fields stay empty — the model has no genre head).
/// `embedding` is the 512-d joint-space audio embedding, averaged across
/// per-clip outputs. Empty if inference failed.
pub struct MlAnalysisResult {
    pub data: MlAnalysisData,
    /// 512-dim MuQ-MuLan audio embedding (l2-normalized output of the
    /// audio tower, averaged across the per-clip outputs). Empty on failure.
    pub embedding: Vec<f32>,
}

/// MuQ-MuLan joint-space embedding dim — fixed by the model.
pub const MUQ_MULAN_EMBEDDING_DIM: usize = 512;

/// One clip = 10 s of mel @ 24 kHz / hop 240 (post `[..., :-1]` trim).
const MUQ_MULAN_CLIP_FRAMES: usize = (MUQ_TARGET_SR as usize) * 10 / MUQ_HOP;

/// Maximum number of 10 s clips to evaluate per track.
///
/// PyTorch's `extract_audio_latents` averages every non-overlapping 10 s
/// window. For DJ-typical 3–6 minute tracks that's 18–36 clips per track,
/// dominating per-track cost. We cap at 6 evenly-spaced clips so a
/// 4-minute track samples roughly intro / verse / break / build / chorus /
/// outro — the same spirit MAEST uses with its 4-window cap.
///
/// At ~290 ms / clip on CPU (per the spike bench) this gives ~1.7 s of
/// pure ML per track on CPU; fast enough to fit alongside the existing
/// rayon-parallel reanalysis loop.
const MUQ_MULAN_MAX_CLIPS: usize = 6;

/// ONNX input tensor name (set by `export.py`).
const MUQ_MULAN_INPUT_NAME: &str = "mel";

/// ONNX output tensor name (set by `export.py`). Single output.
const MUQ_MULAN_OUTPUT_INDEX: usize = 0;

/// Mel-normalization stats loaded from the `*.norm.json` sidecar. Mirrors
/// the structure `convert-muq-mulan/export.py::extract_norm_stats` writes.
#[derive(Debug, Clone, Deserialize)]
struct NormStats {
    /// Per-MelSTFT-frame mean used by the model (`(x - mean) / std`).
    /// Either a scalar or a per-band vector — kept as JSON for flexibility.
    melspec_2048_mean: serde_json::Value,
    melspec_2048_std: serde_json::Value,
    sample_rate: u32,
    n_mels: usize,
    hop_length: usize,
}

/// Resolved (i.e. broadcast-ready) normalization stats. We expand whatever
/// the JSON gave us into per-mel-band f32 vectors so the inner loop is
/// branch-free.
struct ResolvedStats {
    /// Length 1 (scalar broadcast) or `n_mels`.
    mean: Vec<f32>,
    /// Length 1 (scalar broadcast) or `n_mels`.
    std: Vec<f32>,
}

impl ResolvedStats {
    fn from_norm(norm: &NormStats) -> Result<Self, String> {
        Ok(Self {
            mean: extract_stat(&norm.melspec_2048_mean, "melspec_2048_mean")?,
            std: extract_stat(&norm.melspec_2048_std, "melspec_2048_std")?,
        })
    }
}

fn extract_stat(value: &serde_json::Value, label: &str) -> Result<Vec<f32>, String> {
    if let Some(f) = value.as_f64() {
        return Ok(vec![f as f32]);
    }
    if let Some(arr) = value.as_array() {
        let v: Result<Vec<f32>, _> = arr
            .iter()
            .map(|x| {
                x.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| format!("{}: non-numeric element in array: {:?}", label, x))
            })
            .collect();
        return v;
    }
    Err(format!(
        "{}: expected number or array of numbers, got {:?}",
        label, value
    ))
}

/// ML analysis engine with a pre-loaded MuQ-MuLan ONNX session + stats.
pub struct MlAnalyzer {
    session: Session,
    stats: ResolvedStats,
}

// Safety: ort::Session is Send+Sync by design.
unsafe impl Send for MlAnalyzer {}
unsafe impl Sync for MlAnalyzer {}

impl MlAnalyzer {
    /// Create a new analyzer by loading the MuQ-MuLan ONNX from `model_dir`
    /// plus its sibling `*.norm.json` sidecar. Both files must be present.
    pub fn new(model_dir: &Path) -> Result<Self, String> {
        let onnx_path = model_dir.join(MlModelType::MuQMulanLarge.filename());
        let norm_path = model_dir.join(MlModelType::MuQMulanLarge.norm_filename());

        if !onnx_path.exists() {
            return Err(format!("MuQ-MuLan ONNX not found: {:?}", onnx_path));
        }
        if !norm_path.exists() {
            return Err(format!(
                "MuQ-MuLan norm sidecar not found: {:?} (re-run convert-muq-mulan-model)",
                norm_path,
            ));
        }

        let norm_json = std::fs::read_to_string(&norm_path)
            .map_err(|e| format!("Failed to read norm sidecar {:?}: {}", norm_path, e))?;
        let norm: NormStats = serde_json::from_str(&norm_json)
            .map_err(|e| format!("Failed to parse norm sidecar {:?}: {}", norm_path, e))?;

        // Cross-check the sidecar describes the model we actually built our
        // mel pipeline against. Mismatch means someone produced an ONNX
        // with different MelSTFT params; stop loudly rather than silently
        // mis-feeding the model.
        if norm.sample_rate != MUQ_TARGET_SR as u32 {
            return Err(format!(
                "MuQ-MuLan sidecar sample_rate {} ≠ Rust mel pipeline sr {}",
                norm.sample_rate, MUQ_TARGET_SR as u32,
            ));
        }
        if norm.n_mels != MUQ_N_MELS {
            return Err(format!(
                "MuQ-MuLan sidecar n_mels {} ≠ Rust mel pipeline n_mels {}",
                norm.n_mels, MUQ_N_MELS,
            ));
        }
        if norm.hop_length != MUQ_HOP {
            return Err(format!(
                "MuQ-MuLan sidecar hop_length {} ≠ Rust mel pipeline hop {}",
                norm.hop_length, MUQ_HOP,
            ));
        }

        let stats = ResolvedStats::from_norm(&norm)?;
        if stats.mean.len() != 1 && stats.mean.len() != MUQ_N_MELS {
            return Err(format!(
                "MuQ-MuLan mel mean has length {} (expected 1 or {})",
                stats.mean.len(), MUQ_N_MELS,
            ));
        }
        if stats.std.len() != 1 && stats.std.len() != MUQ_N_MELS {
            return Err(format!(
                "MuQ-MuLan mel std has length {} (expected 1 or {})",
                stats.std.len(), MUQ_N_MELS,
            ));
        }

        let session = Session::builder()
            .and_then(|b| b.with_intra_threads(1))
            .and_then(|b| b.commit_from_file(&onnx_path))
            .map_err(|e| format!("Failed to load MuQ-MuLan ONNX: {}", e))?;

        log::info!(
            "Loaded MuQ-MuLan-large audio tower from {:?} (mean_len={}, std_len={})",
            onnx_path, stats.mean.len(), stats.std.len()
        );

        Ok(Self { session, stats })
    }

    /// Run inference on a precomputed dB-scale mel spectrogram.
    ///
    /// Slices the mel into up to `MUQ_MULAN_MAX_CLIPS` evenly-spaced 1000-frame
    /// clips (10 s @ 24 kHz/hop 240), normalizes each with the sidecar stats,
    /// runs ONNX per clip, and averages the per-clip 512-d outputs.
    pub fn analyze(
        &mut self,
        mel: &MelSpectrogramResult,
    ) -> Result<MlAnalysisResult, String> {
        if mel.n_bands != MUQ_N_MELS {
            return Err(format!(
                "Mel has {} bands; MuQ-MuLan needs {}",
                mel.n_bands, MUQ_N_MELS,
            ));
        }
        if mel.frames.is_empty() {
            return Err("Mel spectrogram is empty".to_string());
        }

        let clips = extract_clips(&mel.frames, MUQ_MULAN_CLIP_FRAMES, MUQ_MULAN_MAX_CLIPS);
        if clips.is_empty() {
            return Err("Audio too short for MuQ-MuLan analysis".to_string());
        }
        log::debug!(
            "ML: running MuQ-MuLan on {} clips ({} mel frames total)",
            clips.len(),
            mel.frames.len()
        );

        let mut clip_embeddings: Vec<Vec<f32>> = Vec::with_capacity(clips.len());
        for clip in &clips {
            clip_embeddings.push(self.run_clip(clip)?);
        }

        let embedding = average_embeddings(&clip_embeddings);

        Ok(MlAnalysisResult {
            // Genre / classification fields stay empty — MuQ-MuLan has no
            // genre head. Downstream code already tolerates None / empty here.
            data: MlAnalysisData {
                top_genre: None,
                genre_scores: Vec::new(),
            },
            embedding,
        })
    }

    /// Run one clip: normalize mel → ONNX → 512-d.
    fn run_clip(&mut self, clip: &[Vec<f32>]) -> Result<Vec<f32>, String> {
        let n_frames = clip.len();
        if n_frames == 0 {
            return Err("Empty clip".to_string());
        }

        // ONNX expects (batch=1, n_mels=128, time). Build the tensor data
        // band-major while applying `(x - mean) / std` from the sidecar.
        let mut flat = vec![0.0f32; MUQ_N_MELS * n_frames];
        for (band_idx, slot) in flat.chunks_exact_mut(n_frames).enumerate() {
            let mean = self.stats.mean.get(band_idx).copied()
                .unwrap_or_else(|| self.stats.mean[0]);
            let std = self.stats.std.get(band_idx).copied()
                .unwrap_or_else(|| self.stats.std[0]);
            // Guard against /0 if a stat ends up zero — fall back to identity.
            let std = if std.abs() < 1e-12 { 1.0 } else { std };
            for (t, frame) in clip.iter().enumerate() {
                slot[t] = (frame[band_idx] - mean) / std;
            }
        }

        let input = Array3::from_shape_vec((1, MUQ_N_MELS, n_frames), flat)
            .map_err(|e| format!("MuQ-MuLan input shape error: {}", e))?;
        let input_tensor = Tensor::from_array(input)
            .map_err(|e| format!("MuQ-MuLan tensor creation error: {}", e))?;

        let outputs = self.session.run(
            ort::inputs![MUQ_MULAN_INPUT_NAME => input_tensor]
        ).map_err(|e| format!("MuQ-MuLan inference error: {}", e))?;

        let collected: Vec<_> = outputs.iter().collect();
        let (_, emb_value) = collected.get(MUQ_MULAN_OUTPUT_INDEX).ok_or_else(|| {
            format!(
                "MuQ-MuLan output index {} missing (got {} outputs)",
                MUQ_MULAN_OUTPUT_INDEX, collected.len()
            )
        })?;
        let (shape, data) = emb_value.try_extract_tensor::<f32>()
            .map_err(|e| format!("MuQ-MuLan output extraction error: {}", e))?;

        // ort 2.x's `Shape` derefs to `[i64]`. Pin it through a slice to
        // pattern-match. Expect either [1, 512] or [512].
        let dims: &[i64] = shape.as_ref();
        let dim = match dims {
            [_, d] => *d as usize,
            [d] => *d as usize,
            other => return Err(format!("Unexpected MuQ-MuLan output shape {:?}", other)),
        };
        if dim != MUQ_MULAN_EMBEDDING_DIM {
            return Err(format!(
                "MuQ-MuLan output dim {} ≠ expected {}",
                dim, MUQ_MULAN_EMBEDDING_DIM,
            ));
        }
        Ok(data.iter().copied().take(MUQ_MULAN_EMBEDDING_DIM).collect())
    }
}

// ============================================================================
// Clip extraction + averaging
// ============================================================================

/// Extract up to `max_clips` evenly-spaced `clip_frames`-frame clips from the
/// mel spectrogram. Pads short inputs with zeros to one full clip.
///
/// Even spacing (vs sliding) covers the full track structure with far fewer
/// inference passes — we keep MAEST's pattern here for direct parity in the
/// per-track ML cost profile.
fn extract_clips(
    frames: &[Vec<f32>],
    clip_frames: usize,
    max_clips: usize,
) -> Vec<Vec<Vec<f32>>> {
    if frames.is_empty() || clip_frames == 0 {
        return Vec::new();
    }
    if frames.len() < clip_frames {
        let n_bands = frames[0].len();
        let mut padded = frames.to_vec();
        while padded.len() < clip_frames {
            padded.push(vec![0.0; n_bands]);
        }
        return vec![padded];
    }

    let max_starts = frames.len() - clip_frames;
    let n_clips = max_clips.max(1);

    if n_clips == 1 || max_starts == 0 {
        let start = max_starts / 2;
        return vec![frames[start..start + clip_frames].to_vec()];
    }

    let mut clips = Vec::with_capacity(n_clips);
    for i in 0..n_clips {
        let start = (i * max_starts) / (n_clips - 1);
        clips.push(frames[start..start + clip_frames].to_vec());
    }
    clips
}

/// Average multiple equal-length vectors into a single vector. The MuQ-MuLan
/// audio tower already l2-normalizes per-clip outputs; the mean-then-implicit
/// renorm (a downstream cosine search re-normalizes) keeps the math close to
/// PyTorch's `extract_audio_latents`.
fn average_embeddings(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return Vec::new();
    }
    let dim = embeddings[0].len();
    let n = embeddings.len() as f32;
    let mut avg = vec![0.0f32; dim];
    for emb in embeddings {
        for (i, &v) in emb.iter().enumerate() {
            if i < dim {
                avg[i] += v;
            }
        }
    }
    for v in &mut avg {
        *v /= n;
    }
    avg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_frames_constant_matches_10s_at_24khz() {
        // Sanity check: 24000 Hz / 240 hop * 10 s = 1000 frames per clip.
        assert_eq!(MUQ_MULAN_CLIP_FRAMES, 1000);
    }

    #[test]
    fn test_extract_clips_short_pads() {
        let frames: Vec<Vec<f32>> = (0..50).map(|i| vec![i as f32; MUQ_N_MELS]).collect();
        let clips = extract_clips(&frames, MUQ_MULAN_CLIP_FRAMES, MUQ_MULAN_MAX_CLIPS);
        assert_eq!(clips.len(), 1, "Short audio should produce 1 padded clip");
        assert_eq!(clips[0].len(), MUQ_MULAN_CLIP_FRAMES);
    }

    #[test]
    fn test_extract_clips_normal() {
        // Long enough audio to support multiple clips.
        let frames: Vec<Vec<f32>> = (0..MUQ_MULAN_CLIP_FRAMES * 4)
            .map(|i| vec![i as f32; MUQ_N_MELS])
            .collect();
        let clips = extract_clips(&frames, MUQ_MULAN_CLIP_FRAMES, MUQ_MULAN_MAX_CLIPS);
        assert!(clips.len() >= 2 && clips.len() <= MUQ_MULAN_MAX_CLIPS);
        for clip in &clips {
            assert_eq!(clip.len(), MUQ_MULAN_CLIP_FRAMES);
        }
    }

    #[test]
    fn test_average_embeddings() {
        let emb1 = vec![1.0, 2.0, 3.0];
        let emb2 = vec![3.0, 4.0, 5.0];
        let avg = average_embeddings(&[emb1, emb2]);
        assert_eq!(avg, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_extract_stat_scalar_and_array() {
        let s = serde_json::json!(0.5);
        assert_eq!(extract_stat(&s, "x").unwrap(), vec![0.5]);
        let a = serde_json::json!([1.0, 2.0, 3.0]);
        assert_eq!(extract_stat(&a, "x").unwrap(), vec![1.0, 2.0, 3.0]);
    }

    /// Smoke-test the full Rust pipeline against the real ONNX + sidecar.
    ///
    /// Loads the model from one of the `MlModelManager` search dirs (the
    /// test resolves it the same way production does), feeds a synthetic
    /// 220 Hz sine and white noise, and confirms:
    /// - the analyzer returns a 512-dim embedding,
    /// - the two distinct inputs produce distinct embeddings (cosine < 0.99).
    ///
    /// Skipped unless `MESH_RUN_MUQ_INTEGRATION_TESTS=1` so CI / packaging
    /// builds without the ONNX still pass `cargo test`.
    #[test]
    fn integration_full_pipeline_against_real_onnx() {
        if std::env::var("MESH_RUN_MUQ_INTEGRATION_TESTS").as_deref() != Ok("1") {
            eprintln!("skipping: set MESH_RUN_MUQ_INTEGRATION_TESTS=1 to enable");
            return;
        }

        use crate::ml_analysis::{models::MlModelManager, preprocessing::compute_mel_spectrogram};
        use super::MlModelType;

        let mgr = MlModelManager::new().expect("model manager");
        let onnx = mgr
            .model_path(MlModelType::MuQMulanLarge)
            .expect("MuQ-MuLan ONNX must be present (run nix run .#convert-muq-mulan-model)");
        let model_dir = onnx.parent().unwrap().to_path_buf();

        let mut analyzer = MlAnalyzer::new(&model_dir).expect("analyzer init");

        // 30 s of audio at 44.1 kHz so we get multiple clips after resample.
        let sr = 44_100.0_f32;
        let n = (sr * 30.0) as usize;
        let sine: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / sr).sin() * 0.5)
            .collect();
        let mut noise = vec![0.0f32; n];
        let mut state: u32 = 0x1234_5678;
        for v in &mut noise {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *v = ((state >> 16) as f32 / 32_768.0 - 1.0) * 0.3;
        }

        let mel_sine = compute_mel_spectrogram(&sine, sr).expect("sine mel");
        let mel_noise = compute_mel_spectrogram(&noise, sr).expect("noise mel");

        let r_sine = analyzer.analyze(&mel_sine).expect("sine analyze");
        let r_noise = analyzer.analyze(&mel_noise).expect("noise analyze");

        assert_eq!(r_sine.embedding.len(), MUQ_MULAN_EMBEDDING_DIM);
        assert_eq!(r_noise.embedding.len(), MUQ_MULAN_EMBEDDING_DIM);

        let dot: f32 = r_sine.embedding.iter().zip(&r_noise.embedding).map(|(a, b)| a * b).sum();
        let na: f32 = r_sine.embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        let nb: f32 = r_noise.embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        let cos = dot / (na * nb).max(1e-12);
        eprintln!("integration cosine(sine, noise) = {:.6}", cos);
        assert!(cos.abs() < 0.99, "sine and noise should not be near-identical embeddings (cos = {:.6})", cos);
    }
}
