//! Effect discovery - scans the effects/pd folder for available PD effects
//!
//! Discovery happens once at startup. Effects must have:
//! - A folder in effects/pd/ (folder name = effect ID)
//! - A .pd file matching the folder name
//! - A metadata.json file with effect metadata

use std::path::{Path, PathBuf};

use super::error::{PdError, PdResult};
use super::metadata::EffectMetadata;

/// Information about a discovered effect
#[derive(Debug, Clone)]
pub struct DiscoveredEffect {
    /// Effect identifier (folder name)
    pub id: String,

    /// Path to the .pd patch file
    pub patch_path: PathBuf,

    /// Path to the metadata.json file
    pub metadata_path: PathBuf,

    /// Parsed metadata
    pub metadata: EffectMetadata,

    /// List of missing dependencies (externals)
    pub missing_deps: Vec<String>,

    /// Whether the effect is available (no missing deps)
    pub available: bool,
}

impl DiscoveredEffect {
    /// Get the effect display name
    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    /// Get the effect category
    pub fn category(&self) -> &str {
        &self.metadata.category
    }

    /// Get the latency in samples
    pub fn latency_samples(&self) -> u32 {
        self.metadata.latency_samples
    }
}

/// Effect discovery service
///
/// Scans the effects/pd folder structure and validates effect availability.
pub struct EffectDiscovery {
    /// Root PD effects folder (e.g., ~/Music/mesh-collection/effects/pd/)
    pd_effects_path: PathBuf,

    /// Path to shared externals folder (effects/pd/externals/)
    externals_path: PathBuf,

    /// Path to shared models folder (effects/pd/models/)
    models_path: PathBuf,
}

impl EffectDiscovery {
    /// Create a new discovery service
    ///
    /// # Arguments
    /// * `collection_path` - Path to the mesh collection root
    pub fn new(collection_path: &Path) -> Self {
        let pd_effects_path = collection_path.join("effects").join("pd");
        let externals_path = pd_effects_path.join("externals");
        let models_path = pd_effects_path.join("models");

        Self {
            pd_effects_path,
            externals_path,
            models_path,
        }
    }

    /// Get the PD effects folder path
    pub fn effects_path(&self) -> &Path {
        &self.pd_effects_path
    }

    /// Get the externals folder path
    pub fn externals_path(&self) -> &Path {
        &self.externals_path
    }

    /// Get the models folder path
    pub fn models_path(&self) -> &Path {
        &self.models_path
    }

    /// Discover all available effects
    ///
    /// Scans the effects folder for valid effect directories.
    /// Effects with missing dependencies are included but marked as unavailable.
    pub fn discover(&self) -> Vec<DiscoveredEffect> {
        let mut effects = Vec::new();

        // Check if PD effects folder exists
        if !self.pd_effects_path.exists() {
            log::info!(
                "PD effects folder does not exist: {}",
                self.pd_effects_path.display()
            );
            return effects;
        }

        // Iterate over directories in effects/pd/
        let entries = match std::fs::read_dir(&self.pd_effects_path) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("Failed to read effects folder: {}", e);
                return effects;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Skip non-directories
            if !path.is_dir() {
                continue;
            }

            // Skip special folders (externals, models, _template)
            let folder_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            if folder_name == "externals" || folder_name == "models" {
                continue;
            }

            // Try to load this effect
            match self.load_effect(&path, &folder_name) {
                Ok(effect) => {
                    if effect.available {
                        log::info!(
                            "Discovered PD effect: {} ({})",
                            effect.metadata.name,
                            effect.id
                        );
                    } else {
                        log::warn!(
                            "PD effect '{}' unavailable: missing {}",
                            effect.id,
                            effect.missing_deps.join(", ")
                        );
                    }
                    effects.push(effect);
                }
                Err(e) => {
                    log::warn!("Skipping invalid effect folder '{}': {}", folder_name, e);
                }
            }
        }

        // Sort by category, then name
        effects.sort_by(|a, b| {
            a.metadata
                .category
                .cmp(&b.metadata.category)
                .then_with(|| a.metadata.name.cmp(&b.metadata.name))
        });

        log::info!(
            "Effect discovery complete: {} total, {} available",
            effects.len(),
            effects.iter().filter(|e| e.available).count()
        );

        effects
    }

    /// Load a single effect from a folder
    fn load_effect(&self, folder_path: &Path, effect_id: &str) -> PdResult<DiscoveredEffect> {
        // Check for metadata.json
        let metadata_path = folder_path.join("metadata.json");
        if !metadata_path.exists() {
            return Err(PdError::InvalidMetadata {
                effect_id: effect_id.to_string(),
                reason: "metadata.json not found".to_string(),
            });
        }

        // Check for .pd file matching folder name
        let patch_path = folder_path.join(format!("{}.pd", effect_id));
        if !patch_path.exists() {
            return Err(PdError::PatchNotFound(patch_path));
        }

        // Parse metadata
        let metadata = EffectMetadata::from_file(&metadata_path)?;

        // Check for missing externals
        let missing_deps = self.check_externals(&metadata.requires_externals);
        let available = missing_deps.is_empty();

        Ok(DiscoveredEffect {
            id: effect_id.to_string(),
            patch_path,
            metadata_path,
            metadata,
            missing_deps,
            available,
        })
    }

    /// Check which required externals are missing
    fn check_externals(&self, required: &[String]) -> Vec<String> {
        let mut missing = Vec::new();

        for external in required {
            if !self.external_exists(external) {
                missing.push(external.clone());
            }
        }

        missing
    }

    /// Check if an external exists in the externals folder
    fn external_exists(&self, name: &str) -> bool {
        // Check common external file patterns
        let patterns = [
            format!("{}.pd_linux", name),
            format!("{}.pd_darwin", name),
            format!("{}.dll", name),
            format!("{}.pd", name), // Abstraction
        ];

        for pattern in &patterns {
            if self.externals_path.join(pattern).exists() {
                return true;
            }
        }

        false
    }

    /// Get an effect by ID
    pub fn get_effect<'a>(effects: &'a [DiscoveredEffect], id: &str) -> Option<&'a DiscoveredEffect> {
        effects.iter().find(|e| e.id == id)
    }

    /// Get only available effects
    pub fn available_effects(effects: &[DiscoveredEffect]) -> Vec<&DiscoveredEffect> {
        effects.iter().filter(|e| e.available).collect()
    }

    /// Create the effects folder structure if it doesn't exist
    pub fn ensure_folders_exist(&self) -> PdResult<()> {
        std::fs::create_dir_all(&self.pd_effects_path)?;
        std::fs::create_dir_all(&self.externals_path)?;
        std::fs::create_dir_all(&self.models_path)?;

        log::debug!("Ensured PD effects folders exist at {}", self.pd_effects_path.display());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_effects(temp_dir: &TempDir) -> PathBuf {
        let collection = temp_dir.path().to_path_buf();
        let pd_effects = collection.join("effects").join("pd");
        let externals = pd_effects.join("externals");

        fs::create_dir_all(&externals).unwrap();

        // Create a test external
        fs::write(externals.join("test~.pd_linux"), b"").unwrap();

        // Create a valid effect
        let effect1 = pd_effects.join("test-effect");
        fs::create_dir_all(&effect1).unwrap();
        fs::write(
            effect1.join("metadata.json"),
            r#"{
                "name": "Test Effect",
                "category": "Test",
                "latency_samples": 64
            }"#,
        )
        .unwrap();
        fs::write(effect1.join("test-effect.pd"), b"#N canvas 0 0 450 300;").unwrap();

        // Create an effect with missing dependency
        let effect2 = pd_effects.join("missing-dep");
        fs::create_dir_all(&effect2).unwrap();
        fs::write(
            effect2.join("metadata.json"),
            r#"{
                "name": "Missing Dep Effect",
                "category": "Test",
                "latency_samples": 128,
                "requires_externals": ["nonexistent~"]
            }"#,
        )
        .unwrap();
        fs::write(effect2.join("missing-dep.pd"), b"#N canvas 0 0 450 300;").unwrap();

        collection
    }

    #[test]
    fn test_discover_effects() {
        let temp_dir = TempDir::new().unwrap();
        let collection = setup_test_effects(&temp_dir);

        let discovery = EffectDiscovery::new(&collection);
        let effects = discovery.discover();

        assert_eq!(effects.len(), 2);

        // First effect should be available
        let test_effect = effects.iter().find(|e| e.id == "test-effect").unwrap();
        assert!(test_effect.available);
        assert!(test_effect.missing_deps.is_empty());
        assert_eq!(test_effect.metadata.name, "Test Effect");

        // Second effect should be unavailable
        let missing = effects.iter().find(|e| e.id == "missing-dep").unwrap();
        assert!(!missing.available);
        assert_eq!(missing.missing_deps, vec!["nonexistent~"]);
    }

    #[test]
    fn test_external_exists() {
        let temp_dir = TempDir::new().unwrap();
        let collection = setup_test_effects(&temp_dir);

        let discovery = EffectDiscovery::new(&collection);

        assert!(discovery.external_exists("test~"));
        assert!(!discovery.external_exists("nonexistent~"));
    }

    #[test]
    fn test_available_effects_filter() {
        let temp_dir = TempDir::new().unwrap();
        let collection = setup_test_effects(&temp_dir);

        let discovery = EffectDiscovery::new(&collection);
        let effects = discovery.discover();
        let available = EffectDiscovery::available_effects(&effects);

        assert_eq!(available.len(), 1);
        assert_eq!(available[0].id, "test-effect");
    }
}
