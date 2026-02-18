//! Mesh Core - Shared library for DJ Player and Cue Software

pub mod audio;
pub mod config;
pub mod types;
pub mod effect;
pub mod audio_file;
pub mod timestretch;
#[cfg(feature = "pd-effects")]
pub mod pd;

// Stub module when pd-effects feature is disabled — provides no-op types
// so downstream code compiles without #[cfg] guards everywhere.
#[cfg(not(feature = "pd-effects"))]
pub mod pd {
    use std::path::{Path, PathBuf};

    #[derive(Debug)]
    pub struct DiscoveredEffect {
        pub id: String,
        pub patch_path: PathBuf,
        pub metadata_path: PathBuf,
        pub metadata: EffectMetadata,
        pub missing_deps: Vec<String>,
        pub available: bool,
    }

    impl DiscoveredEffect {
        pub fn name(&self) -> &str { &self.metadata.name }
        pub fn category(&self) -> &str { &self.metadata.category }
    }

    #[derive(Debug, Clone, Default)]
    pub struct ParamMetadata {
        pub name: String,
        pub default: f32,
        pub min: Option<f32>,
        pub max: Option<f32>,
        pub unit: Option<String>,
    }

    #[derive(Debug, Clone, Default)]
    pub struct EffectMetadata {
        pub name: String,
        pub category: String,
        pub author: Option<String>,
        pub version: Option<String>,
        pub description: Option<String>,
        pub latency_samples: u32,
        pub sample_rate: u32,
        pub requires_externals: Vec<String>,
        pub params: Vec<ParamMetadata>,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("PD effects not available (compiled without pd-effects feature)")]
    pub struct PdError;

    pub struct PdManager;

    impl PdManager {
        pub fn new(_path: &Path) -> Result<Self, String> { Ok(Self) }
        pub fn discovered_effects(&self) -> &[DiscoveredEffect] { &[] }
        pub fn available_effects(&self) -> Vec<&DiscoveredEffect> { vec![] }
        pub fn get_effect(&self, _effect_id: &str) -> Option<&DiscoveredEffect> { None }
        pub fn rescan_effects(&mut self) {}
        pub fn create_effect(
            &mut self, _id: &str,
        ) -> Result<Box<dyn crate::effect::Effect>, PdError> {
            Err(PdError)
        }
    }

    impl Default for PdManager {
        fn default() -> Self { Self }
    }
}
pub mod clap;
pub mod engine;
pub mod playlist;
pub mod music;
pub mod loader;
pub mod usb;
pub mod db;
pub mod services;
pub mod export;
pub mod preset_loader;

pub use types::*;
