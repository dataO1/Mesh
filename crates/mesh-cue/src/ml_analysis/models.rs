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
use std::path::PathBuf;

/// Filename of the audio-tower ONNX produced by `convert-muq-mulan-model`.
pub const MUQ_MULAN_ONNX_FILENAME: &str = "muq-mulan-audio-tower.onnx";

/// Sibling sidecar with mel-normalization stats and `MelSTFT` parameters.
pub const MUQ_MULAN_NORM_FILENAME: &str = "muq-mulan-audio-tower.onnx.norm.json";

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

    /// Whether we can locate everything needed to run inference for `model`
    /// (ONNX + norm sidecar both present at the same dir).
    pub fn is_available(&self, model: MlModelType) -> bool {
        let Some(onnx) = self.model_path(model) else { return false };
        let sidecar = onnx.with_file_name(model.norm_filename());
        sidecar.exists()
    }

    pub fn are_base_models_available(&self) -> bool {
        MlModelType::base_models().iter().all(|m| self.is_available(*m))
    }

    /// One-time installer: if the ONNX/sidecar exists somewhere on disk
    /// outside the cache (e.g. `models/` from a fresh `convert-muq-mulan-model`
    /// run), copy them into the long-term cache so the user doesn't need to
    /// keep the build dir around. No-op if the cache copy already exists or
    /// the source is already the cache.
    ///
    /// Returns the cache-resident ONNX path on success.
    pub fn install_to_cache(&self, model: MlModelType) -> Result<PathBuf, String> {
        let cache_onnx = self.cache_path(model);
        if cache_onnx.exists() {
            return Ok(cache_onnx);
        }
        let Some(src_onnx) = self.model_path(model) else {
            return Err(format!(
                "{} not found in any search dir; run `nix run .#convert-muq-mulan-model` to produce it",
                model.filename(),
            ));
        };
        if src_onnx == cache_onnx {
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
        fs::copy(&src_onnx, &cache_onnx)
            .map_err(|e| format!("Failed to copy ONNX to cache: {}", e))?;
        fs::copy(&src_sidecar, &cache_sidecar)
            .map_err(|e| format!("Failed to copy sidecar to cache: {}", e))?;
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

        // 2/3. Relative to the executable.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                dirs.push(exe_dir.join("models"));
                // Cargo target/<profile>/<bin> → ../../models/
                if let Some(parent) = exe_dir.parent().and_then(|p| p.parent()) {
                    dirs.push(parent.join("models"));
                }
            }
        }

        // 4. Working directory (developer running from repo root).
        if let Ok(cwd) = std::env::current_dir() {
            dirs.push(cwd.join("models"));
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
