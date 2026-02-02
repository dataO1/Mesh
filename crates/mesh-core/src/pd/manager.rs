//! PdManager - manages the global PD instance and effect creation
//!
//! This is the main entry point for the PD integration. It handles:
//! - Creating and managing the SINGLE global PdInstance (libpd limitation)
//! - Discovering available effects at startup
//! - Creating PdEffect instances for effect chains
//!
//! # Important: Single PdInstance
//!
//! libpd can only be initialized once per process. All effects from all decks
//! share this single PdInstance. Per-effect isolation is achieved via the $0
//! prefix in patch communication (e.g., $0-param0).

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::effect::Effect;
use crate::types::SAMPLE_RATE;

use super::discovery::{DiscoveredEffect, EffectDiscovery};
use super::effect::PdEffect;
use super::error::{PdError, PdResult};
use super::instance::PdInstance;

/// Manager for the global PD instance and effects
///
/// Provides a unified interface for the mesh audio engine to interact
/// with Pure Data. All effects share a single PdInstance (libpd limitation).
pub struct PdManager {
    /// The single global PD instance (lazily initialized, shared by all effects)
    instance: Option<Arc<Mutex<PdInstance>>>,

    /// Discovered effects from the effects folder
    discovered_effects: Vec<DiscoveredEffect>,

    /// Effect discovery service
    discovery: EffectDiscovery,

    /// Sample rate for the instance
    sample_rate: i32,
}

impl PdManager {
    /// Create a new PD manager
    ///
    /// # Arguments
    /// * `collection_path` - Path to the mesh collection root
    pub fn new(collection_path: &Path) -> PdResult<Self> {
        let discovery = EffectDiscovery::new(collection_path);

        // Ensure effects folder structure exists
        if let Err(e) = discovery.ensure_folders_exist() {
            log::warn!("Failed to create effects folders: {}", e);
        }

        // Discover available effects
        let discovered_effects = discovery.discover();

        log::info!(
            "PdManager initialized: {} effects discovered ({} available)",
            discovered_effects.len(),
            discovered_effects.iter().filter(|e| e.available).count()
        );

        Ok(Self {
            instance: None,
            discovered_effects,
            discovery,
            sample_rate: SAMPLE_RATE as i32,
        })
    }

    /// Initialize the global PD instance
    ///
    /// This is called lazily when the first PD effect is created.
    /// Subsequent calls are no-ops since libpd only supports one instance.
    fn init_instance(&mut self) -> PdResult<()> {
        log::info!("[PD-DEBUG] init_instance() called");

        if self.instance.is_some() {
            log::info!("[PD-DEBUG] Instance already exists, returning early");
            return Ok(()); // Already initialized
        }

        log::info!("[PD-DEBUG] Creating new PdInstance...");
        let mut instance = PdInstance::new(self.sample_rate)?;
        log::info!("[PD-DEBUG] PdInstance created successfully");

        // Add search paths for externals and models
        instance.add_search_path(self.discovery.externals_path())?;
        instance.add_search_path(self.discovery.models_path())?;

        self.instance = Some(Arc::new(Mutex::new(instance)));

        log::info!("[PD] Global instance initialized");

        Ok(())
    }

    /// Create a PD effect
    ///
    /// # Arguments
    /// * `effect_id` - The effect identifier (folder name)
    ///
    /// # Returns
    /// A boxed Effect trait object that can be added to an effect chain
    ///
    /// # Note
    /// All effects share the single global PdInstance. Per-effect isolation
    /// is achieved via the $0 prefix in patch communication.
    pub fn create_effect(&mut self, effect_id: &str) -> PdResult<Box<dyn Effect>> {
        // Find the effect and clone necessary data to avoid borrow conflicts
        // (we need mutable self for init_instance, but also need effect data)
        let (patch_path, metadata) = {
            let effect = self
                .discovered_effects
                .iter()
                .find(|e| e.id == effect_id)
                .ok_or_else(|| PdError::EffectNotFound(effect_id.to_string()))?;

            // Check if available
            if !effect.available {
                return Err(PdError::MissingExternal {
                    effect_id: effect_id.to_string(),
                    external: effect.missing_deps.first().cloned().unwrap_or_default(),
                });
            }

            // Clone what we need before the borrow ends
            (effect.patch_path.clone(), effect.metadata.clone())
        };

        // Ensure global instance is initialized (requires &mut self)
        self.init_instance()?;

        // Get the shared instance
        let instance = self
            .instance
            .as_ref()
            .ok_or_else(|| {
                PdError::InitializationFailed("PD instance not initialized".to_string())
            })?
            .clone();

        // Create the PD effect
        let mut pd_effect =
            PdEffect::new(instance, patch_path, &metadata, effect_id.to_string())?;

        // Open the patch
        pd_effect.open()?;

        Ok(Box::new(pd_effect))
    }

    /// Get the list of discovered effects
    pub fn discovered_effects(&self) -> &[DiscoveredEffect] {
        &self.discovered_effects
    }

    /// Get only available effects (no missing dependencies)
    pub fn available_effects(&self) -> Vec<&DiscoveredEffect> {
        self.discovered_effects
            .iter()
            .filter(|e| e.available)
            .collect()
    }

    /// Get an effect by ID
    pub fn get_effect(&self, effect_id: &str) -> Option<&DiscoveredEffect> {
        self.discovered_effects.iter().find(|e| e.id == effect_id)
    }

    /// Check if the global PD instance is initialized
    pub fn is_initialized(&self) -> bool {
        self.instance.is_some()
    }

    /// Get the effects folder path
    pub fn effects_path(&self) -> &Path {
        self.discovery.effects_path()
    }

    /// Get the externals folder path
    pub fn externals_path(&self) -> &Path {
        self.discovery.externals_path()
    }

    /// Get the models folder path
    pub fn models_path(&self) -> &Path {
        self.discovery.models_path()
    }

    /// Re-scan for effects (e.g., after user adds new effects)
    ///
    /// Note: This doesn't affect already-loaded effects.
    pub fn rescan_effects(&mut self) {
        self.discovered_effects = self.discovery.discover();

        log::info!(
            "Effects rescanned: {} total, {} available",
            self.discovered_effects.len(),
            self.discovered_effects.iter().filter(|e| e.available).count()
        );
    }
}

impl Default for PdManager {
    fn default() -> Self {
        // Use default collection path
        let collection_path = crate::config::default_collection_path();
        Self::new(&collection_path).unwrap_or_else(|e| {
            log::error!("Failed to create PdManager: {}", e);
            Self {
                instance: None,
                discovered_effects: Vec::new(),
                discovery: EffectDiscovery::new(&collection_path),
                sample_rate: SAMPLE_RATE as i32,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup_test_collection(temp_dir: &TempDir) -> PathBuf {
        let collection = temp_dir.path().to_path_buf();
        let effects = collection.join("effects");
        let externals = effects.join("externals");

        std::fs::create_dir_all(&externals).unwrap();

        // Create a simple test effect
        let effect = effects.join("test-effect");
        std::fs::create_dir_all(&effect).unwrap();
        std::fs::write(
            effect.join("metadata.json"),
            r#"{
                "name": "Test Effect",
                "category": "Test",
                "latency_samples": 0
            }"#,
        )
        .unwrap();
        std::fs::write(
            effect.join("test-effect.pd"),
            r#"#N canvas 0 0 450 300 12;
#X obj 50 50 inlet~;
#X obj 150 50 inlet~;
#X obj 50 200 outlet~;
#X obj 150 200 outlet~;
#X connect 0 0 2 0;
#X connect 1 0 3 0;
"#,
        )
        .unwrap();

        collection
    }

    #[test]
    fn test_manager_discovery() {
        let temp_dir = TempDir::new().unwrap();
        let collection = setup_test_collection(&temp_dir);

        let manager = PdManager::new(&collection).unwrap();

        assert_eq!(manager.discovered_effects().len(), 1);
        assert!(manager.get_effect("test-effect").is_some());
        assert!(manager.get_effect("nonexistent").is_none());
    }

    #[test]
    fn test_available_effects() {
        let temp_dir = TempDir::new().unwrap();
        let collection = setup_test_collection(&temp_dir);

        let manager = PdManager::new(&collection).unwrap();
        let available = manager.available_effects();

        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, "test-effect");
    }
}
