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
//! Per-track flow (`analyze`, round-7.7 A5):
//!
//!   1. Compute a per-mel-frame energy envelope from the precomputed mel
//!      (sum of shifted-positive dB across bands, with optional bass-band
//!      boost). Pick the top `ENERGY_PEAK_WINDOWS` non-overlapping 30 s
//!      windows by rolling-sum energy. Fall back to evenly-spaced clips
//!      for tracks too short to host two non-overlapping windows.
//!   2. Slice each window into `ENERGY_PEAK_CLIPS_PER_WINDOW` contiguous
//!      10 s clips (1000 frames each at 24 kHz / hop 240).
//!   3. For each clip: apply `(mel - mean) / std` from the sidecar stats,
//!      run ONNX, collect the (1024-d Conformer hidden, 512-d joint-space)
//!      pair.
//!   4. Mean-pool clips within each window → one (1024, 512) candidate per
//!      window. Project each 1024-d candidate through V18.X → pick the
//!      higher-scoring candidate as the track-level embedding.
//!   5. Same per-track ONNX cost as the prior 6 evenly-spaced clips
//!      (`ENERGY_PEAK_WINDOWS × ENERGY_PEAK_CLIPS_PER_WINDOW = 6`).
//!
//! Genre / classification heads from the prior MAEST integration have
//! been removed — MuQ-MuLan emits embeddings only.

use std::path::Path;
use ndarray::Array3;
use ort::session::Session;
use ort::value::Tensor;
use mesh_core::db::MlAnalysisData;
use serde::Deserialize;

use super::aggression_axis::IntensityAxis;
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
    /// 512-dim MuQ-MuLan joint-space audio embedding. L2-normalized.
    /// Used by mesh-cue similarity / suggestion graph / future text-tower.
    /// Empty on failure.
    pub embedding: Vec<f32>,
    /// Round-7.7: 1024-dim MuQ-MuLan Conformer hidden state. Pre-projection.
    /// Used by the V18.X+ intensity probe per the MuQ paper recipe.
    /// Empty on pre-round-7.7 (single-output) ONNX or on failure.
    pub embedding_1024: Vec<f32>,
    /// V18.X intensity scalar + the axis version it was projected with:
    /// `project_normalised(embedding_1024)` through the same axis instance
    /// that did the A5 window pick. `None` when the 1024-d head or the
    /// axis is unavailable (legacy ONNX) — callers then skip the
    /// `intensity_score` store and the track scores as neutral.
    pub intensity: Option<(f32, String)>,
}

/// MuQ-MuLan joint-space embedding dim — fixed by the model.
pub const MUQ_MULAN_EMBEDDING_DIM: usize = 512;

/// MuQ-MuLan Conformer hidden state dim (pre-projection, round-7.7+).
pub const MUQ_MULAN_HIDDEN_DIM: usize = 1024;

/// One clip = 10 s of mel @ 24 kHz / hop 240 (post `[..., :-1]` trim).
const MUQ_MULAN_CLIP_FRAMES: usize = (MUQ_TARGET_SR as usize) * 10 / MUQ_HOP;

/// Maximum number of 10 s clips to evaluate per track in the *fallback*
/// (evenly-spaced) path. Steady-state A5 inference uses
/// `ENERGY_PEAK_WINDOWS × ENERGY_PEAK_CLIPS_PER_WINDOW` clips, which equals
/// this number by construction (2 × 3 = 6) so per-track ONNX cost is
/// unchanged across both paths.
///
/// PyTorch's `extract_audio_latents` averages every non-overlapping 10 s
/// window. For DJ-typical 3–6 minute tracks that's 18–36 clips per track,
/// dominating per-track cost. We cap at 6 so a 4-minute track samples
/// roughly intro / verse / break / build / chorus / outro — the same
/// spirit MAEST uses with its 4-window cap.
///
/// **History.** Briefly tried 12 clips. A full-library 12-clip reanalysis
/// produced an identical Spearman score on the 47-anchor eval set
/// (+0.358 V11 either way). 12 clips also slightly *demoted* peak-time
/// tracks like Charlotte De Witte "How You Move" (rank 35 → 76) because
/// the wider sample picks up more breakdowns/builds and dilutes peak-only
/// energy. Reverted to 6: same metric, half the per-track cost
/// (~1.7 s vs ~3.5 s on CPU).
const MUQ_MULAN_MAX_CLIPS: usize = 6;

/// Round-7.7 A5: number of non-overlapping energy-peak windows to evaluate
/// per track. 2 mirrors the training distribution shape (Deezer's preview
/// picker grabs roughly one 30 s window per track; we pick two candidates
/// and let V18.X choose the more-intense one), at the cost of one extra
/// V18.X projection call per track (~µs, negligible).
pub const ENERGY_PEAK_WINDOWS: usize = 2;

/// Round-7.7 A5: number of contiguous 10 s clips per energy-peak window.
/// 3 matches MuQ-MuLan's training preview length (30 s = 3 × 10 s clips,
/// mean-pooled inside `MuQMuLan.from_pretrained()`).
pub const ENERGY_PEAK_CLIPS_PER_WINDOW: usize = 3;

/// Round-7.7 A5: width in mel frames of one energy-peak candidate window.
/// 30 s @ 24 kHz / hop 240 = 3000 frames.
pub const ENERGY_PEAK_WINDOW_FRAMES: usize = ENERGY_PEAK_CLIPS_PER_WINDOW * MUQ_MULAN_CLIP_FRAMES;

/// Round-7.7 A5: lowest mel bands counted as "bass" for the energy
/// envelope's bass-band boost. 16 / 128 ≈ 0–500 Hz at 24 kHz with the
/// MuQ-MuLan filterbank — kick + sub-bass region for DJ genres.
pub const ENERGY_BASS_BANDS: usize = 16;

/// Round-7.7 A5: per-band weight applied to bass bands when summing the
/// energy envelope. Modest (1.5×) so kick-driven drops don't lose to
/// vocal-prominent breakdowns when the eval target is bass-heavy genres
/// (DnB, dubstep, techno) — but small enough that the 7×-more numerous
/// non-bass bands still dominate the sum on truly silent-bass loud-treble
/// frames.
pub const ENERGY_BASS_BOOST: f32 = 1.5;

/// Round-7.7 A5: floor of the dB-scale mel for the energy envelope
/// (see `preprocessing::compute_mel_spectrogram`, `top_db = 80`). Shifting
/// by this floor keeps envelope values non-negative so the rolling-sum
/// peak picker never has to reason about sign flips during masking.
pub const ENERGY_DB_FLOOR: f32 = -80.0;

/// ONNX input tensor name (set by `export.py`).
const MUQ_MULAN_INPUT_NAME: &str = "mel";

/// ONNX output tensor names (set by `export.py`).
///
/// Round-7.7: the export now emits two named outputs from a single
/// forward pass — `audio_embedding_1024` (Conformer hidden, intensity
/// probe) and `audio_embedding_512` (joint-space, similarity). This
/// crate currently consumes the 512-d only (similarity / clustering /
/// suggestion graph); the 1024-d wiring lands when the V18.X retrain
/// completes.
///
/// Pre-round-7.7 ONNX exports had a single output named just
/// `audio_embedding`; the by-name lookup falls back to that for back-
/// compat with the v18.1-shipped ONNX.
const MUQ_MULAN_OUTPUT_NAME_512: &str = "audio_embedding_512";
const MUQ_MULAN_OUTPUT_NAME_LEGACY_512: &str = "audio_embedding";
const MUQ_MULAN_OUTPUT_NAME_1024: &str = "audio_embedding_1024";

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
///
/// `intensity_axis` is loaded best-effort: if the active variant JSON is
/// missing next to the ONNX, the analyzer still works for embedding
/// extraction; only intensity scoring degrades to "axis unknown" until the
/// axis JSON is provided.
pub struct MlAnalyzer {
    session: Session,
    stats: ResolvedStats,
    intensity_axis: Option<IntensityAxis>,
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

        // Active intensity axis: per-cache user override → embedded default.
        // The embedded default (V18.1 MLP at the time of writing) is baked
        // into the binary at build time via include_bytes!, so this never
        // returns None in practice — embedding-time peak-clip selection
        // always has an axis to consult.
        let axis_path = model_dir.join(MlModelType::MuQMulanLarge.aggression_axis_filename());
        let intensity_axis = if axis_path.exists() {
            match IntensityAxis::load(&axis_path) {
                Ok(axis) => {
                    log::info!("Loaded intensity axis from user override {:?}", axis_path);
                    Some(axis)
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load user override {:?}: {} — falling back to embedded default",
                        axis_path, e
                    );
                    IntensityAxis::embedded_default().ok()
                }
            }
        } else {
            log::info!("No user override at {:?} — using embedded default", axis_path);
            IntensityAxis::embedded_default().ok()
        };

        log::info!(
            "Loaded MuQ-MuLan-large audio tower from {:?} (mean_len={}, std_len={}, axis={})",
            onnx_path, stats.mean.len(), stats.std.len(),
            intensity_axis.as_ref().map(|a| a.variant_id.as_str()).unwrap_or("<none>"),
        );

        Ok(Self { session, stats, intensity_axis })
    }

    /// Reference to the loaded intensity axis (or None if absent / load-failed).
    pub fn intensity_axis(&self) -> Option<&IntensityAxis> {
        self.intensity_axis.as_ref()
    }

    /// Run inference on a precomputed dB-scale mel spectrogram.
    ///
    /// **Round-7.7 A5 — energy-pruned candidate-window selection.**
    ///
    /// Round-7.6 V18.1 was trained on 30 s Deezer previews — Deezer's preview
    /// picker grabs roughly the catchiest 30 s of each track, usually the
    /// drop. So the teacher and the 3 jurors learned to score peak-energy
    /// windows. Pre-A5 deploys mean-pooled 6 evenly-spaced 10 s clips, then
    /// (round-7.6, 2026-05-08) used the V18.X-projected single highest clip
    /// — better, but still picking from clip starts that mostly miss the
    /// drop on long tracks with quiet intros.
    ///
    /// A5 pre-selects clip *starts* using a per-frame energy envelope:
    ///   1. Sum shifted-positive dB across mel bands, with bass bands 0..16
    ///      weighted `ENERGY_BASS_BOOST` so kick drops outrank vocal breaks.
    ///   2. Pick `ENERGY_PEAK_WINDOWS` non-overlapping 30 s windows by
    ///      rolling-sum energy (greedy argmax with window-wide masking).
    ///   3. Slice each window into `ENERGY_PEAK_CLIPS_PER_WINDOW` contiguous
    ///      10 s clips. Run ONNX on each (6 inferences total — same per-track
    ///      cost as the prior 6 evenly-spaced clips).
    ///   4. Mean-pool clips within each window → one (1024-d, 512-d)
    ///      candidate per window. Project each 1024-d candidate through
    ///      V18.X → pick the higher-scoring candidate as the track-level
    ///      embedding.
    ///
    /// This matches the training pipeline exactly within each window
    /// (`MuQMuLan.from_pretrained()` mean-pools 3 × 10 s clips of each 30 s
    /// preview) and adds the "two candidates, V18.X picks" step on top.
    ///
    /// **Fallback paths.**
    ///
    /// - Tracks shorter than `ENERGY_PEAK_WINDOWS × ENERGY_PEAK_WINDOW_FRAMES`
    ///   (~60 s) can't host two non-overlapping 30 s windows → fall back to
    ///   `extract_clips` (evenly-spaced) with `n_per_window = 1`. The
    ///   per-window mean degenerates to identity, and the V18.X-pick step
    ///   degenerates to peak-clip-by-intensity over the evenly-spaced clips
    ///   — exactly the round-7.6 behaviour.
    /// - Legacy single-output ONNX (pre-round-7.7, no 1024-d head) → mean-
    ///   pool 512-d across all candidates; intensity slot stays empty so the
    ///   downstream path surfaces the migration prompt.
    /// - Intensity axis not loaded (build / cache edge case) → mean-pool
    ///   both heads across candidates. Never fires in practice because the
    ///   embedded default is always available.
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

        // Energy-pruned window selection (A5). Falls back to evenly-spaced
        // clips when the track is too short to host `ENERGY_PEAK_WINDOWS`
        // non-overlapping `ENERGY_PEAK_WINDOW_FRAMES`-wide windows.
        let want_n = ENERGY_PEAK_WINDOWS;
        let want_w = ENERGY_PEAK_WINDOW_FRAMES;
        let clips_per_window = ENERGY_PEAK_CLIPS_PER_WINDOW;

        let window_starts = if mel.frames.len() >= want_n * want_w {
            let energy = compute_energy_envelope(&mel.frames, true);
            select_energy_peak_windows(&energy, want_w, want_n)
        } else {
            Vec::new()
        };

        let (clips, n_per_window) = if window_starts.len() == want_n {
            log::debug!(
                "ML: A5 energy-pruned — picked {} windows at frames {:?}, {} clips/window",
                want_n, window_starts, clips_per_window,
            );
            (
                extract_clips_at_windows(
                    &mel.frames, &window_starts, want_w,
                    MUQ_MULAN_CLIP_FRAMES, clips_per_window,
                ),
                clips_per_window,
            )
        } else {
            log::debug!(
                "ML: track too short for A5 ({} frames < {}) — fallback to evenly-spaced clips",
                mel.frames.len(), want_n * want_w,
            );
            (
                extract_clips(&mel.frames, MUQ_MULAN_CLIP_FRAMES, MUQ_MULAN_MAX_CLIPS),
                1,
            )
        };

        if clips.is_empty() {
            return Err("Audio too short for MuQ-MuLan analysis".to_string());
        }

        // Each clip yields (1024-d Conformer hidden, 512-d joint-space).
        // The 1024-d slot is empty under a legacy single-output ONNX
        // (pre-round-7.7); the intensity-axis fallback below handles that.
        let mut clip_pairs: Vec<(Vec<f32>, Vec<f32>)> = Vec::with_capacity(clips.len());
        for clip in &clips {
            clip_pairs.push(self.run_clip(clip)?);
        }

        // Group clips into per-window candidates by mean-pooling. With
        // `n_per_window = 1` (short-track fallback) every clip is its own
        // candidate and the V18.X-pick step below degenerates to round-7.6
        // peak-clip-by-intensity.
        let any_1024 = clip_pairs.iter().any(|(h, _)| !h.is_empty());
        let candidates: Vec<(Vec<f32>, Vec<f32>)> = clip_pairs
            .chunks(n_per_window)
            .map(|chunk| {
                let mean_1024 = if any_1024 {
                    let h: Vec<Vec<f32>> = chunk
                        .iter()
                        .filter_map(|(h, _)| (!h.is_empty()).then(|| h.clone()))
                        .collect();
                    if h.is_empty() { Vec::new() } else { average_embeddings(&h) }
                } else {
                    Vec::new()
                };
                let mean_512 = average_embeddings(
                    &chunk.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
                );
                (mean_1024, mean_512)
            })
            .collect();

        let n_cand = candidates.len();
        let (embedding_1024, embedding) = match (self.intensity_axis.as_ref(), n_cand, any_1024) {
            (Some(axis), n, true) if n > 1 => {
                // Multi-candidate + 1024-d available + axis loaded → V18.X picks.
                let mut peak_idx = 0usize;
                let mut peak_score = f32::NEG_INFINITY;
                for (i, (h_1024, _)) in candidates.iter().enumerate() {
                    if h_1024.is_empty() { continue; }
                    let s = axis.project(h_1024);
                    if s > peak_score {
                        peak_score = s;
                        peak_idx = i;
                    }
                }
                log::debug!(
                    "ML: A5 picked candidate {}/{} (V18.X score={:.4}, {} clips/candidate)",
                    peak_idx + 1, n_cand, peak_score, n_per_window,
                );
                candidates.into_iter().nth(peak_idx).unwrap()
            }
            (_, 1, _) => {
                // Single candidate — nothing to pick.
                candidates.into_iter().next().unwrap()
            }
            (_, _, false) => {
                // Legacy ONNX (no 1024-d). Mean-pool 512-d across candidates;
                // intensity slot stays empty so downstream surfaces the
                // migration prompt.
                log::debug!(
                    "ML: no 1024-d output (legacy ONNX) — mean-pool {} candidates on 512-d only",
                    n_cand,
                );
                let mean_512 = average_embeddings(
                    &candidates.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
                );
                (Vec::new(), mean_512)
            }
            _ => {
                // Multi-candidate + 1024-d available BUT no axis loaded.
                // Mean-pool both heads. Shouldn't fire with embedded default.
                log::debug!(
                    "ML: no intensity axis loaded — mean-pool {} candidates on both heads",
                    n_cand,
                );
                let mean_1024 = average_embeddings(
                    &candidates
                        .iter()
                        .filter_map(|(h, _)| (!h.is_empty()).then(|| h.clone()))
                        .collect::<Vec<_>>(),
                );
                let mean_512 = average_embeddings(
                    &candidates.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
                );
                (mean_1024, mean_512)
            }
        };

        // V18.X scalar: project the final 1024-d hidden through the same
        // axis that did the A5 window pick, so the stored scalar is exactly
        // consistent with the embedding it summarises.
        let intensity = match (self.intensity_axis.as_ref(), embedding_1024.is_empty()) {
            (Some(axis), false) => Some((
                axis.project_normalised(&embedding_1024),
                axis.variant_id.clone(),
            )),
            _ => None,
        };

        Ok(MlAnalysisResult {
            data: MlAnalysisData {
                top_genre: None,
                genre_scores: Vec::new(),
            },
            embedding,
            embedding_1024,
            intensity,
        })
    }

    /// Run one clip: normalize mel → ONNX → (1024-d Conformer hidden, 512-d joint-space).
    ///
    /// Round-7.7: multi-output ONNX returns BOTH heads from a single forward
    /// pass. Pre-round-7.7 (legacy) ONNX returns only the 512-d, in which
    /// case the 1024-d slot is empty and the intensity probe falls back to
    /// the deprecated 512-d path.
    fn run_clip(&mut self, clip: &[Vec<f32>]) -> Result<(Vec<f32>, Vec<f32>), String> {
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

        // Helper: extract a named output as Vec<f32> with dim check.
        let collected: Vec<_> = outputs.iter().collect();
        let extract_by_name = |target_name: &str, expected_dim: usize| -> Result<Option<Vec<f32>>, String> {
            let entry = collected.iter().find(|(name, _)| *name == target_name);
            let Some((_, v)) = entry else { return Ok(None); };
            let (shape, data) = v.try_extract_tensor::<f32>()
                .map_err(|e| format!("MuQ-MuLan {} extraction error: {}", target_name, e))?;
            let dims: &[i64] = shape.as_ref();
            let dim = match dims {
                [_, d] => *d as usize,
                [d] => *d as usize,
                other => return Err(format!("Unexpected MuQ-MuLan {} shape {:?}", target_name, other)),
            };
            if dim != expected_dim {
                return Err(format!(
                    "MuQ-MuLan {} output dim {} ≠ expected {}",
                    target_name, dim, expected_dim,
                ));
            }
            Ok(Some(data.iter().copied().take(expected_dim).collect()))
        };

        // 512-d: round-7.7 named `audio_embedding_512`; legacy named `audio_embedding`.
        let emb_512 = extract_by_name(MUQ_MULAN_OUTPUT_NAME_512, MUQ_MULAN_EMBEDDING_DIM)?
            .or(extract_by_name(MUQ_MULAN_OUTPUT_NAME_LEGACY_512, MUQ_MULAN_EMBEDDING_DIM)?)
            .ok_or_else(|| {
                let names: Vec<&str> = collected.iter().map(|(n, _)| *n).collect();
                format!(
                    "MuQ-MuLan ONNX has no `{}` or `{}` output (got: {:?})",
                    MUQ_MULAN_OUTPUT_NAME_512, MUQ_MULAN_OUTPUT_NAME_LEGACY_512, names,
                )
            })?;

        // 1024-d: round-7.7 only — empty Vec on legacy single-output ONNX.
        let emb_1024 = extract_by_name(MUQ_MULAN_OUTPUT_NAME_1024, MUQ_MULAN_HIDDEN_DIM)?
            .unwrap_or_default();

        Ok((emb_1024, emb_512))
    }
}

// ============================================================================
// Clip extraction + averaging
// ============================================================================

/// Extract up to `max_clips` evenly-spaced `clip_frames`-frame clips from the
/// mel spectrogram. Pads short inputs with zeros to one full clip.
///
/// Used by the round-7.7 A5 fallback path for tracks too short to host
/// `ENERGY_PEAK_WINDOWS` non-overlapping windows. Steady-state A5 uses
/// `extract_clips_at_windows` against energy-peak window starts instead.
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

// ============================================================================
// Round-7.7 A5: energy-pruned clip selection
// ============================================================================

/// Sum mel-band magnitudes per frame to produce a per-frame energy envelope
/// for the A5 peak picker.
///
/// The mel input is dB-scale (`preprocessing::compute_mel_spectrogram` runs
/// `10·log10(power)` with a `top_db = 80` floor). We shift by `ENERGY_DB_FLOOR`
/// to make per-band contributions non-negative (so the rolling-sum picker
/// never has to reason about sign flips during masking) and then sum across
/// bands. With `bass_boost`, bands `0..ENERGY_BASS_BANDS` are weighted by
/// `ENERGY_BASS_BOOST` so kick / sub-bass-driven drops outrank vocal-
/// prominent breakdowns on bass-heavy DJ genres.
///
/// Mel-band-sum is monotone in spectral energy — what the peak picker needs
/// to RANK loud regions, not measure absolute loudness — so it sidesteps
/// the cross-crate plumbing of `features::extraction::compute_energy_statistics`
/// (sample-domain RMS) without losing usable signal for window selection.
pub fn compute_energy_envelope(frames: &[Vec<f32>], bass_boost: bool) -> Vec<f32> {
    frames
        .iter()
        .map(|frame| {
            let mut sum = 0.0f32;
            for (band_idx, &v) in frame.iter().enumerate() {
                let shifted = (v - ENERGY_DB_FLOOR).max(0.0);
                let weight = if bass_boost && band_idx < ENERGY_BASS_BANDS {
                    ENERGY_BASS_BOOST
                } else {
                    1.0
                };
                sum += shifted * weight;
            }
            sum
        })
        .collect()
}

/// Pick `n_windows` non-overlapping window-start positions that maximise the
/// rolling sum of `energy` over `window_frames`-wide windows.
///
/// Returned starts are sorted ascending. Returns an empty vec if `energy` is
/// shorter than `window_frames` — the caller should fall back to the
/// evenly-spaced clip path.
///
/// Algorithm: O(n) sliding-window rolling-sum precompute, then `n_windows`
/// rounds of greedy argmax with window-wide masking around each pick. Total
/// cost is O(n + n_windows · n_starts) — for a 5-minute track at 24 kHz /
/// hop 240 (~30 k frames) and `n_windows = 2`, that's ~60 k comparisons,
/// dwarfed by the ONNX inferences they gate.
pub fn select_energy_peak_windows(
    energy: &[f32],
    window_frames: usize,
    n_windows: usize,
) -> Vec<usize> {
    if window_frames == 0 || n_windows == 0 || energy.len() < window_frames {
        return Vec::new();
    }

    let n_starts = energy.len() - window_frames + 1;
    let mut rolling = vec![0.0f32; n_starts];
    let mut current: f32 = energy[..window_frames].iter().sum();
    rolling[0] = current;
    for i in 1..n_starts {
        current += energy[i + window_frames - 1] - energy[i - 1];
        rolling[i] = current;
    }

    let mut starts: Vec<usize> = Vec::with_capacity(n_windows);
    let mut taken = vec![false; n_starts];
    for _ in 0..n_windows {
        let mut best_idx: Option<usize> = None;
        let mut best_score = f32::NEG_INFINITY;
        for (i, (&score, &gone)) in rolling.iter().zip(taken.iter()).enumerate() {
            if !gone && score > best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }
        let Some(picked) = best_idx else { break };
        starts.push(picked);
        // Mask a window-wide neighbourhood so the next pick can't overlap:
        // any start in `[picked - (window-1), picked + window)` would share
        // at least one frame with the picked window.
        let lo = picked.saturating_sub(window_frames - 1);
        let hi = (picked + window_frames).min(n_starts);
        for slot in &mut taken[lo..hi] {
            *slot = true;
        }
    }

    starts.sort_unstable();
    starts
}

/// Slice the mel into `clips_per_window` contiguous `clip_frames`-frame clips
/// inside each window in `window_starts`. Output is flat — `clips_per_window`
/// successive entries belong to the same window — so callers can re-group via
/// `chunks(clips_per_window)`.
///
/// Caller must ensure each `start + window_frames <= frames.len()` and that
/// `clips_per_window * clip_frames <= window_frames`. `select_energy_peak_windows`
/// upholds the first invariant by construction; the second is a constants-
/// only check enforced at module-load time by `static_assertions` in tests.
fn extract_clips_at_windows(
    frames: &[Vec<f32>],
    window_starts: &[usize],
    window_frames: usize,
    clip_frames: usize,
    clips_per_window: usize,
) -> Vec<Vec<Vec<f32>>> {
    debug_assert!(
        clips_per_window * clip_frames <= window_frames,
        "clips_per_window * clip_frames ({} * {}) > window_frames ({})",
        clips_per_window, clip_frames, window_frames,
    );
    let mut out = Vec::with_capacity(window_starts.len() * clips_per_window);
    for &ws in window_starts {
        debug_assert!(
            ws + window_frames <= frames.len(),
            "window {}..{} out of bounds for {} frames",
            ws, ws + window_frames, frames.len(),
        );
        for c in 0..clips_per_window {
            let clip_start = ws + c * clip_frames;
            let clip_end = clip_start + clip_frames;
            out.push(frames[clip_start..clip_end].to_vec());
        }
    }
    out
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
    fn test_energy_envelope_shifted_positive_and_bass_boost() {
        // Frame with band 0 at 0 dB (loud), all other bands at the floor.
        // Shifted-positive: band 0 contributes 80, the rest contribute 0.
        // No bass boost: sum = 80. With bass boost: sum = 80 * 1.5 = 120.
        let frame: Vec<f32> = (0..MUQ_N_MELS)
            .map(|b| if b == 0 { 0.0 } else { ENERGY_DB_FLOOR })
            .collect();
        let frames = vec![frame; 3];

        let env_no_boost = compute_energy_envelope(&frames, false);
        assert_eq!(env_no_boost.len(), 3);
        for v in &env_no_boost {
            assert!((v - 80.0).abs() < 1e-3, "no-boost energy = {}", v);
        }

        let env_boost = compute_energy_envelope(&frames, true);
        for v in &env_boost {
            assert!((v - 120.0).abs() < 1e-3, "boost energy = {}", v);
        }
    }

    #[test]
    fn test_energy_envelope_below_floor_clamped() {
        // Mel values below the documented floor (shouldn't happen in practice
        // but guard against -inf-style stats from a buggy upstream): the
        // shifted-positive `.max(0.0)` keeps them from pulling the sum
        // negative.
        let frame = vec![ENERGY_DB_FLOOR - 50.0; MUQ_N_MELS];
        let env = compute_energy_envelope(&[frame], false);
        assert_eq!(env, vec![0.0]);
    }

    #[test]
    fn test_select_energy_peak_windows_two_distinct_peaks() {
        // 100 frames with two clear plateaus; window = 20 frames.
        let mut energy = vec![0.0f32; 100];
        for v in &mut energy[5..15] { *v = 1.0; }   // plateau A around frame 10
        for v in &mut energy[55..65] { *v = 1.0; }  // plateau B around frame 60
        let starts = select_energy_peak_windows(&energy, 20, 2);
        assert_eq!(starts.len(), 2, "starts = {:?}", starts);
        // First window covers plateau A → start in [0, 15] (so [start, start+20) covers 5..15).
        assert!(starts[0] <= 15, "starts[0] = {}", starts[0]);
        // Second window covers plateau B → start in [45, 65].
        assert!(starts[1] >= 45 && starts[1] <= 65, "starts[1] = {}", starts[1]);
    }

    #[test]
    fn test_select_energy_peak_windows_non_overlap_constraint() {
        // Two adjacent strong peaks closer than window width — second pick
        // must skip past the masked neighbourhood.
        let mut energy = vec![0.1f32; 100];
        energy[20] = 10.0;  // strongest
        energy[25] = 9.0;   // second-best, but overlaps the 20..40 window
        let starts = select_energy_peak_windows(&energy, 20, 2);
        assert_eq!(starts.len(), 2);
        let gap = starts[1].abs_diff(starts[0]);
        assert!(gap >= 20, "non-overlap violated: starts = {:?}, gap = {}", starts, gap);
    }

    #[test]
    fn test_select_energy_peak_windows_short_input_returns_empty() {
        let energy = vec![1.0f32; 10];
        assert!(select_energy_peak_windows(&energy, 20, 2).is_empty());
    }

    #[test]
    fn test_select_energy_peak_windows_n_zero_returns_empty() {
        let energy = vec![1.0f32; 100];
        assert!(select_energy_peak_windows(&energy, 20, 0).is_empty());
    }

    #[test]
    fn test_extract_clips_at_windows_layout_matches_spec() {
        // 6000 frames, 2 windows × 3 clips × 1000 frames/clip. Tag each frame
        // with its index so we can verify the slicing boundaries.
        let frames: Vec<Vec<f32>> = (0..6000)
            .map(|i| vec![i as f32; MUQ_N_MELS])
            .collect();
        let starts = vec![0usize, 3000];
        let clips = extract_clips_at_windows(&frames, &starts, 3000, 1000, 3);
        assert_eq!(clips.len(), 6, "2 windows × 3 clips");
        for clip in &clips {
            assert_eq!(clip.len(), 1000);
        }
        // First clip in window 0 starts at frame 0.
        assert_eq!(clips[0][0][0], 0.0);
        // Third clip in window 0 starts at frame 2000.
        assert_eq!(clips[2][0][0], 2000.0);
        // First clip in window 1 starts at frame 3000.
        assert_eq!(clips[3][0][0], 3000.0);
        // Last clip ends just before frame 6000.
        assert_eq!(clips[5][999][0], 5999.0);
    }

    #[test]
    fn test_a5_constants_consistent() {
        // The A5 cap (n_windows × clips/window) MUST equal the fallback cap
        // so per-track ONNX cost is invariant across both paths.
        assert_eq!(
            ENERGY_PEAK_WINDOWS * ENERGY_PEAK_CLIPS_PER_WINDOW,
            MUQ_MULAN_MAX_CLIPS,
            "A5 windows × clips/window must equal fallback MUQ_MULAN_MAX_CLIPS",
        );
        // Window must be wide enough to host its clips.
        assert!(
            ENERGY_PEAK_CLIPS_PER_WINDOW * MUQ_MULAN_CLIP_FRAMES <= ENERGY_PEAK_WINDOW_FRAMES,
            "ENERGY_PEAK_WINDOW_FRAMES too small for the clips it must contain",
        );
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
