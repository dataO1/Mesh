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
    }

    #[derive(Debug, Clone, Default)]
    pub struct EffectMetadata {
        pub name: String,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("PD effects not available (compiled without pd-effects feature)")]
    pub struct PdError;

    pub struct PdManager;

    impl PdManager {
        pub fn new(_path: &Path) -> Result<Self, String> { Ok(Self) }
        pub fn discovered_effects(&self) -> &[DiscoveredEffect] { &[] }
        pub fn available_effects(&self) -> Vec<&DiscoveredEffect> { vec![] }
        pub fn rescan_effects(&mut self) {}
        pub fn create_effect(
            &mut self, _id: &str,
        ) -> Result<Box<dyn crate::effect::Effect>, String> {
            Err("PD effects not available (compiled without pd-effects feature)".into())
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
