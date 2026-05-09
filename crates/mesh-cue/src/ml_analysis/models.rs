//! ML model management for audio analysis.
//!
//! Locates the MuQ-MuLan-large audio-tower ONNX (and its `*.norm.json`
//! mel-normalization sidecar) on disk. Unlike the prior MAEST integration
//! there is no public download URL — the model is produced locally by the
//! `convert-muq-mulan-model` Nix app. We look in this order:
//!
//!   1. `MESH_MUQ_MULAN_MODEL_DIR` env var (test/CI override)
//!   2. `<exe_dir>/models/`            (release artifact layout)
//!   3. `<exe_dir>/../../models/`      (cargo `target/<profile>/<bin>` layout)
//!   4. `<cwd>/models/`                (developer "run from repo root")
//!   5. `~/.cache/mesh-cue/ml-models/` (long-term user cache)
//!
//! On first successful load the file is hard-linked / copied into the
//! cache so subsequent runs find it via path 5 even if the binary moves.

use std::fs;
use std::path::{Path, PathBuf};

/// Round-7.7: cache invalidation predicate. Returns true if `src` is
/// materially different from `cache` and the cache should be refreshed.
///
/// Two cheap signals (no content hash — would defeat the point of a
/// long-term cache for a 1.2 GB ONNX):
///
/// 1. **Size mismatch.** A ONNX export change (new output tensor, layer
///    swap, opset bump) almost always changes the byte count.
/// 2. **`src` mtime newer than cache mtime.** Catches in-place
///    re-exports that happen to land on the same byte size.
///
/// On any I/O error reading metadata, treats the cache as stale (safer
/// to over-copy than serve a stale model that produces wrong outputs).
fn is_stale(src: &Path, cache: &Path) -> bool {
    let (Ok(src_meta), Ok(cache_meta)) = (src.metadata(), cache.metadata()) else {
        return true;
    };
    if src_meta.len() != cache_meta.len() {
        return true;
    }
    match (src_meta.modified(), cache_meta.modified()) {
        (Ok(s), Ok(c)) => s > c,
        _ => true,
    }
}

/// Filename of the audio-tower ONNX produced by `convert-muq-mulan-model`.
pub const MUQ_MULAN_ONNX_FILENAME: &str = "muq-mulan-audio-tower.onnx";

/// Sibling sidecar with mel-normalization stats and `MelSTFT` parameters.
pub const MUQ_MULAN_NORM_FILENAME: &str = "muq-mulan-audio-tower.onnx.norm.json";

/// Active intensity-axis JSON, derived from MuQ-MuLan's text tower against
/// polar prompts. Lives next to the ONNX. Swapping which variant is "active"
/// is a single file copy/symlink — see `models/aggression-axes/` for the
/// full variant pool and `documents/aggression-axis-text-tower-plan.md`
/// for the design.
pub const MUQ_MULAN_AGGRESSION_AXIS_FILENAME: &str = "muq-mulan-aggression-axis.json";

/// ML model variants. Only one model on this branch — the dual-encoder
/// MuQ-MuLan audio tower (text tower deferred). Kept as an enum so future
/// model swaps don't require changing every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlModelType {
    /// MuQ-MuLan audio tower exported via `nix run .#convert-muq-mulan-model`.
    /// Input: dB-scale mel spectrogram `[1, 128, 1000]` per 10 s clip @ 24 kHz.
    /// Output: 512-dim joint-space audio embedding (l2-normalized).
    /// See `documents/embedding-models-research.md::Phase 2`.
    MuQMulanLarge,
}

impl MlModelType {
    pub fn filename(&self) -> &'static str {
        match self {
            MlModelType::MuQMulanLarge => MUQ_MULAN_ONNX_FILENAME,
        }
    }

    pub fn norm_filename(&self) -> &'static str {
        match self {
            MlModelType::MuQMulanLarge => MUQ_MULAN_NORM_FILENAME,
        }
    }

    pub fn aggression_axis_filename(&self) -> &'static str {
        match self {
            MlModelType::MuQMulanLarge => MUQ_MULAN_AGGRESSION_AXIS_FILENAME,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            MlModelType::MuQMulanLarge => "MuQ-MuLan-large (audio tower, 512-d)",
        }
    }

    pub fn base_models() -> &'static [MlModelType] {
        &[MlModelType::MuQMulanLarge]
    }
}

/// Locates the MuQ-MuLan ONNX + sidecar across the candidate dirs.
pub struct MlModelManager {
    /// Long-term cache: `~/.cache/mesh-cue/ml-models/`.
    cache_dir: PathBuf,
}

impl MlModelManager {
    pub fn new() -> Result<Self, String> {
        let base = dirs::cache_dir()
            .ok_or_else(|| "Could not determine cache directory".to_string())?;
        Ok(Self {
            cache_dir: base.join("mesh-cue").join("ml-models"),
        })
    }

    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Long-term cache path (where we'd store the model after the user
    /// runs the conversion script). May not exist yet.
    pub fn cache_path(&self, model: MlModelType) -> PathBuf {
        self.cache_dir.join(model.filename())
    }

    /// Resolve the on-disk ONNX path, searching all candidate dirs in
    /// priority order. Returns `None` if the model isn't anywhere.
    pub fn model_path(&self, model: MlModelType) -> Option<PathBuf> {
        for dir in self.search_dirs() {
            let candidate = dir.join(model.filename());
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    /// Resolve the sidecar `*.norm.json` next to a found ONNX.
    /// Returns `None` if the ONNX itself wasn't found.
    pub fn norm_path(&self, model: MlModelType) -> Option<PathBuf> {
        self.model_path(model).map(|onnx| {
            onnx.with_file_name(model.norm_filename())
        })
    }

    /// Resolve the active aggression-axis JSON next to a found ONNX.
    /// Returns `None` if the ONNX itself wasn't found. The axis file may
    /// still be missing — callers should treat that as a degraded but
    /// non-fatal state (intensity scoring just won't produce a signal).
    pub fn aggression_axis_path(&self, model: MlModelType) -> Option<PathBuf> {
        self.model_path(model).map(|onnx| {
            onnx.with_file_name(model.aggression_axis_filename())
        })
    }

    /// Whether we can locate everything needed to run inference for `model`
    /// (ONNX + norm sidecar both present at the same dir). The aggression
    /// axis is intentionally NOT a hard requirement — its absence degrades
    /// intensity scoring but doesn't prevent the rest of the pipeline.
    pub fn is_available(&self, model: MlModelType) -> bool {
        let Some(onnx) = self.model_path(model) else { return false };
        let sidecar = onnx.with_file_name(model.norm_filename());
        sidecar.exists()
    }

    pub fn are_base_models_available(&self) -> bool {
        MlModelType::base_models().iter().all(|m| self.is_available(*m))
    }

    /// One-time (or refresh) installer: if the ONNX/sidecar exists somewhere
    /// on disk outside the cache (e.g. `models/` from a fresh
    /// `convert-muq-mulan-model` run), copy them into the long-term cache so
    /// the user doesn't need to keep the build dir around.
    ///
    /// **Re-copies on staleness** (round-7.7 fix): a non-cache source that is
    /// newer or differently-sized than the cached copy invalidates the
    /// cache. Without this, a project-side ONNX update (e.g. switching from
    /// single-output to multi-output) wouldn't propagate — mesh-cue would
    /// keep loading the stale cached model and silently miss the new outputs.
    ///
    /// No-op if the source is already the cache, or if the cache is already
    /// fresh against the source.
    ///
    /// Returns the cache-resident ONNX path on success.
    pub fn install_to_cache(&self, model: MlModelType) -> Result<PathBuf, String> {
        let cache_onnx = self.cache_path(model);
        let Some(src_onnx) = self.model_path(model) else {
            // No source found anywhere — fall back to whatever's in the cache
            // (will be reported as missing by `is_available` if also absent).
            if cache_onnx.exists() {
                return Ok(cache_onnx);
            }
            return Err(format!(
                "{} not found in any search dir; run `nix run .#convert-muq-mulan-model` to produce it",
                model.filename(),
            ));
        };
        if src_onnx == cache_onnx {
            return Ok(cache_onnx);
        }
        // Decide whether the cache needs refreshing.
        let cache_stale = !cache_onnx.exists() || is_stale(&src_onnx, &cache_onnx);
        if !cache_stale {
            return Ok(cache_onnx);
        }
        let src_sidecar = src_onnx.with_file_name(model.norm_filename());
        if !src_sidecar.exists() {
            return Err(format!(
                "Found {} but its norm sidecar {} is missing — re-run the converter",
                src_onnx.display(),
                src_sidecar.display(),
            ));
        }
        fs::create_dir_all(&self.cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;
        let cache_sidecar = cache_onnx.with_file_name(model.norm_filename());
        if cache_onnx.exists() {
            log::warn!(
                "Cached {} is stale (src len={}, mtime={:?}; cache len={}, mtime={:?}) — refreshing",
                model.filename(),
                src_onnx.metadata().ok().and_then(|m| Some(m.len())).unwrap_or(0),
                src_onnx.metadata().ok().and_then(|m| m.modified().ok()),
                cache_onnx.metadata().ok().and_then(|m| Some(m.len())).unwrap_or(0),
                cache_onnx.metadata().ok().and_then(|m| m.modified().ok()),
            );
        }
        fs::copy(&src_onnx, &cache_onnx)
            .map_err(|e| format!("Failed to copy ONNX to cache: {}", e))?;
        fs::copy(&src_sidecar, &cache_sidecar)
            .map_err(|e| format!("Failed to copy sidecar to cache: {}", e))?;

        // Best-effort: also copy the aggression axis if the source dir has
        // one. Missing axis is non-fatal — intensity scoring degrades but
        // ML inference still works.
        //
        // Same staleness rule as the ONNX above — the project axis JSON is
        // the source of truth; if it's newer / different size from the cache,
        // refresh. Otherwise leave the user's per-collection override alone.
        let src_axis = src_onnx.with_file_name(model.aggression_axis_filename());
        if src_axis.exists() {
            let cache_axis = cache_onnx.with_file_name(model.aggression_axis_filename());
            let axis_stale = !cache_axis.exists() || is_stale(&src_axis, &cache_axis);
            if axis_stale {
                if cache_axis.exists() {
                    log::warn!(
                        "Cached {} is stale — refreshing from {}",
                        model.aggression_axis_filename(),
                        src_axis.display(),
                    );
                }
                if let Err(e) = fs::copy(&src_axis, &cache_axis) {
                    log::warn!(
                        "Failed to copy aggression axis to cache (non-fatal): {}",
                        e,
                    );
                } else {
                    log::info!(
                        "Installed aggression axis {} → {}",
                        src_axis.display(),
                        cache_axis.display(),
                    );
                }
            }
        } else {
            log::info!(
                "No aggression-axis JSON next to ONNX — intensity scoring will be inactive until one is provided",
            );
        }

        log::info!(
            "Installed {} → {}",
            src_onnx.display(),
            cache_onnx.display()
        );
        Ok(cache_onnx)
    }

    /// Ensure each base model is present in *some* search dir (no download —
    /// the MuQ-MuLan ONNX is produced by a local Nix app, not fetched).
    pub fn ensure_all_models(&self) -> Result<(), String> {
        for &model in MlModelType::base_models() {
            if !self.is_available(model) {
                return Err(format!(
                    "{} not available — run `nix run .#convert-muq-mulan-model` to generate it",
                    model.display_name(),
                ));
            }
        }
        Ok(())
    }

    /// Same as `ensure_all_models` but with the byte-progress callback shape
    /// the previous MAEST flow used. There is no download step on the
    /// MuQ-MuLan path so the callback is never invoked — we keep the
    /// signature so the caller in `mod.rs` can stay generic.
    pub fn ensure_all_models_with_progress(
        &self,
        _progress: impl Fn(&'static str, u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<(), String> {
        self.ensure_all_models()
    }

    /// Directories to search in priority order. See module docstring.
    fn search_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // 1. Env override.
        if let Ok(p) = std::env::var("MESH_MUQ_MULAN_MODEL_DIR") {
            dirs.push(PathBuf::from(p));
        }

        // 2-4. Relative to the executable. Walk up several levels to cover:
        //   - release artifact:           <bin>/../models/
        //   - cargo run:                  target/<profile>/<bin> → ../../models/
        //   - cargo test (lib):           target/<profile>/deps/<bin> → ../../../models/
        //   - cargo test (workspace bin): same as above
        if let Ok(exe) = std::env::current_exe() {
            let mut cur = exe.parent().map(|p| p.to_path_buf());
            for _ in 0..4 {
                let Some(d) = cur.clone() else { break };
                dirs.push(d.join("models"));
                cur = d.parent().map(|p| p.to_path_buf());
            }
        }

        // 5. Working directory (developer running from repo root or crate dir).
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd.join("models"));
            // Also the parent (cwd may be a crate subdir like crates/mesh-cue).
            if let Some(parent) = cwd.parent() {
                dirs.push(parent.join("models"));
            }
            if let Some(grand) = cwd.parent().and_then(|p| p.parent()) {
                dirs.push(grand.join("models"));
            }
        }

        // 5. Long-term cache.
        dirs.push(self.cache_dir.clone());

        dirs
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use super::*;

    #[test]
    fn test_model_filenames() {
        assert_eq!(MlModelType::MuQMulanLarge.filename(), MUQ_MULAN_ONNX_FILENAME);
        assert_eq!(MlModelType::MuQMulanLarge.norm_filename(), MUQ_MULAN_NORM_FILENAME);
    }

    #[test]
    fn test_base_models_list() {
        assert_eq!(MlModelType::base_models().len(), 1);
    }

    #[test]
    fn test_search_dirs_includes_cache_and_cwd() {
        let mgr = MlModelManager::with_cache_dir("/tmp/test-cache".into());
        let dirs = mgr.search_dirs();
        assert!(dirs.iter().any(|p| p == Path::new("/tmp/test-cache")));
    }
}
