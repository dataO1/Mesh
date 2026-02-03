//! CLAP plugin hosting via clack-host
//!
//! This module provides CLAP (CLever Audio Plugin) hosting for mesh,
//! enabling loading third-party audio effects written to the CLAP standard.
//!
//! # Architecture
//!
//! The CLAP integration follows a layered architecture:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      ClapManager                            │
//! │  - Manages plugin discovery and caching                     │
//! │  - Creates ClapEffect instances                             │
//! │  - Provides thread-safe access to plugins                   │
//! └─────────────────────────────────────────────────────────────┘
//!                              │
//!          ┌──────────────────┴──────────────────┐
//!          ▼                                     ▼
//! ┌─────────────────┐                   ┌─────────────────┐
//! │   ClapEffect    │                   │   ClapEffect    │
//! │   (single)      │                   │   (single)      │
//! │                 │                   │                 │
//! │  ┌───────────┐  │                   │  ┌───────────┐  │
//! │  │ ClapPlugin│  │                   │  │ ClapPlugin│  │
//! │  │ Wrapper   │  │                   │  │ Wrapper   │  │
//! │  └───────────┘  │                   │  └───────────┘  │
//! └─────────────────┘                   └─────────────────┘
//! ```
//!
//! # Plugin Discovery
//!
//! CLAP plugins are discovered from standard paths:
//!
//! ```text
//! ~/.clap/                      # User plugins
//! /usr/lib/clap/                # System plugins
//! /usr/local/lib/clap/          # Local system plugins
//! ```
//!
//! # Effect Trait Integration
//!
//! `ClapEffect` implements the `Effect` trait for seamless integration:
//!
//! - **process()** - Routes audio through the CLAP plugin
//! - **latency_samples()** - Reports plugin latency for compensation
//! - **set_param()** - Maps mesh's 8 parameters to plugin params
//! - **set_bypass()** - Controls plugin bypass state
//!
//! # MultibandHost
//!
//! For multiband processing, see `crate::effect::MultibandHost` which is
//! effect-agnostic and can contain any effect type (CLAP, PD, native).
//!
//! # Example
//!
//! ```ignore
//! use mesh_core::clap::{ClapManager, ClapError};
//!
//! // Create manager and scan for plugins
//! let mut manager = ClapManager::new();
//! manager.scan_plugins();
//!
//! // List available plugins
//! for plugin in manager.available_plugins() {
//!     println!("{}: {}", plugin.id, plugin.name);
//! }
//!
//! // Create an effect instance
//! let effect = manager.create_effect("org.lsp-plug.compressor-stereo")?;
//!
//! // Add to effect chain (effect implements the Effect trait)
//! chain.add_effect(effect);
//! ```

mod error;
mod discovery;
mod plugin;
mod effect;
// Note: multiband module moved to crate::effect::multiband (effect-agnostic)

// Re-export public API
pub use error::{ClapError, ClapResult};
pub use discovery::{ClapDiscovery, ClapPluginCategory, DiscoveredClapPlugin};
pub use plugin::ClapPluginWrapper;
pub use effect::ClapEffect;

use std::sync::Arc;
use std::collections::HashMap;
use std::path::PathBuf;

use clack_host::bundle::PluginBundle;

/// Manager for CLAP plugin hosting
///
/// Handles plugin discovery, caching, and effect creation.
pub struct ClapManager {
    /// Plugin discovery instance
    discovery: ClapDiscovery,
    /// Cache of loaded plugin bundles (path -> Arc for sharing)
    bundle_cache: HashMap<PathBuf, Arc<PluginBundle>>,
}

impl ClapManager {
    /// Create a new CLAP manager
    pub fn new() -> Self {
        Self {
            discovery: ClapDiscovery::new(),
            bundle_cache: HashMap::new(),
        }
    }

    /// Scan for available CLAP plugins
    ///
    /// This scans all standard CLAP directories and caches the results.
    pub fn scan_plugins(&mut self) -> &[DiscoveredClapPlugin] {
        self.discovery.scan()
    }

    /// Force a rescan of plugin directories
    pub fn rescan_plugins(&mut self) -> &[DiscoveredClapPlugin] {
        self.discovery.rescan()
    }

    /// Get all discovered plugins (including unavailable)
    pub fn discovered_plugins(&self) -> &[DiscoveredClapPlugin] {
        self.discovery.discovered_plugins()
    }

    /// Get only available (successfully loaded) plugins
    pub fn available_plugins(&self) -> Vec<&DiscoveredClapPlugin> {
        self.discovery.available_plugins()
    }

    /// Get a plugin by ID
    pub fn get_plugin(&self, plugin_id: &str) -> Option<&DiscoveredClapPlugin> {
        self.discovery.get_plugin(plugin_id)
    }

    /// Get plugins by category
    pub fn plugins_by_category(&self, category: ClapPluginCategory) -> Vec<&DiscoveredClapPlugin> {
        self.discovery.plugins_by_category(category)
    }

    /// Check if any plugins are available
    pub fn has_plugins(&self) -> bool {
        self.discovery.has_plugins()
    }

    /// Create a CLAP effect instance
    ///
    /// This loads the plugin and returns a boxed Effect that can be added
    /// to an effect chain.
    ///
    /// # Arguments
    /// * `plugin_id` - The CLAP plugin identifier (e.g., "org.lsp-plug.compressor-stereo")
    ///
    /// # Returns
    /// A boxed Effect implementing the Effect trait, or an error if the plugin
    /// cannot be loaded.
    pub fn create_effect(&mut self, plugin_id: &str) -> ClapResult<Box<dyn crate::effect::Effect>> {
        // Clone the plugin info to release the borrow on self early
        let plugin_info = self.discovery.get_plugin(plugin_id).cloned().ok_or_else(|| {
            ClapError::PluginNotFound {
                plugin_id: plugin_id.to_string(),
                bundle_path: PathBuf::new(),
            }
        })?;

        if !plugin_info.available {
            return Err(ClapError::PluginNotFound {
                plugin_id: plugin_id.to_string(),
                bundle_path: plugin_info.bundle_path.clone(),
            });
        }

        // Get or load the plugin bundle (now we can borrow self mutably)
        let bundle = self.get_or_load_bundle(&plugin_info.bundle_path)?;

        // Create and return the ClapEffect
        let effect = ClapEffect::from_plugin(&plugin_info, bundle)?;
        Ok(Box::new(effect))
    }

    /// Load a plugin bundle from path, caching for reuse
    fn get_or_load_bundle(&mut self, path: &PathBuf) -> ClapResult<Arc<PluginBundle>> {
        // Check cache first
        if let Some(bundle) = self.bundle_cache.get(path) {
            return Ok(Arc::clone(bundle));
        }

        // Load the bundle
        let bundle = unsafe {
            PluginBundle::load(path).map_err(|e| ClapError::BundleLoadFailed {
                path: path.clone(),
                reason: format!("{:?}", e),
            })?
        };

        let bundle = Arc::new(bundle);
        self.bundle_cache.insert(path.clone(), Arc::clone(&bundle));
        Ok(bundle)
    }

    /// Create a multiband host effect with native LR24 crossover
    ///
    /// This creates an effect-agnostic MultibandHost container that can hold
    /// any effect type (CLAP, PD, native). The MultibandHost now includes a
    /// built-in Linkwitz-Riley 24dB/oct crossover for frequency band splitting.
    ///
    /// # Arguments
    /// * `_crossover_plugin_id` - Deprecated, ignored. Native crossover is always used.
    /// * `buffer_size` - Processing buffer size in samples
    ///
    /// # Returns
    /// A MultibandHost effect with native LR24 crossover.
    pub fn create_multiband(
        &mut self,
        _crossover_plugin_id: Option<&str>,
        buffer_size: usize,
    ) -> ClapResult<crate::effect::MultibandHost> {
        use crate::effect::MultibandHost;

        // Create the multiband host with native LR24 crossover
        let host = MultibandHost::new(buffer_size);
        log::info!("MultibandHost created with native LR24 crossover");

        Ok(host)
    }

    /// Create a simple multiband host without a crossover
    ///
    /// This creates a MultibandHost that starts in single-band mode.
    /// Effects are processed in series. Useful for testing or as a
    /// general effect container that can be expanded to multiband later.
    pub fn create_multiband_simple(&self, buffer_size: usize) -> crate::effect::MultibandHost {
        crate::effect::MultibandHost::new(buffer_size)
    }

    /// Add a custom search path for plugins
    pub fn add_search_path(&mut self, path: PathBuf) {
        self.discovery.add_search_path(path);
    }

    /// Get the current search paths
    pub fn search_paths(&self) -> &[PathBuf] {
        self.discovery.search_paths()
    }
}

impl Default for ClapManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let manager = ClapManager::new();
        // Manager should start with no plugins (not scanned yet)
        assert!(manager.discovered_plugins().is_empty());
    }

    #[test]
    fn test_plugin_not_found() {
        let mut manager = ClapManager::new();
        let result = manager.create_effect("nonexistent.plugin");
        match result {
            Err(ClapError::PluginNotFound { .. }) => {} // Expected
            Err(e) => panic!("Unexpected error type: {:?}", e),
            Ok(_) => panic!("Expected PluginNotFound error"),
        }
    }
}
