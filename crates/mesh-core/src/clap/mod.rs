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
pub use plugin::{ClapPluginWrapper, ParamChangeEvent, ParamChangeReceiver};
pub use effect::ClapEffect;
// ClapGuiHandle is defined in this file, no need to re-export

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::path::PathBuf;

use clack_host::bundle::PluginBundle;

/// Handle for CLAP plugin GUI operations
///
/// This struct holds a reference to the plugin wrapper (shared with the audio-thread
/// effect via `Arc<Mutex<>>`), enabling GUI operations from the main/UI thread while
/// the effect processes audio on another thread.
///
/// # Usage
///
/// ```ignore
/// let (effect, gui_handle) = clap_manager.create_effect_with_gui_handle("org.example.plugin")?;
///
/// // Send effect to audio engine
/// engine.add_effect(effect);
///
/// // Store gui_handle for later GUI operations
/// if gui_handle.supports_gui() {
///     gui_handle.open_gui(parent_window)?;
/// }
/// ```
pub struct ClapGuiHandle {
    /// Plugin identifier
    pub plugin_id: String,
    /// Shared reference to the plugin wrapper (same as in ClapEffect)
    pub wrapper: Arc<Mutex<ClapPluginWrapper>>,
    /// CLAP parameter IDs for mapping
    pub param_ids: Vec<u32>,
    /// Receiver for parameter change notifications from plugin GUI
    /// Used for learning mode - when the user adjusts a plugin GUI control,
    /// we detect the change here and can assign it to a knob.
    pub param_change_receiver: ParamChangeReceiver,
}

impl ClapGuiHandle {
    /// Check if the plugin supports GUI
    pub fn supports_gui(&self) -> bool {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            wrapper.supports_gui()
        } else {
            false
        }
    }

    /// Get the preferred GUI size
    pub fn get_gui_size(&self) -> Option<(u32, u32)> {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            wrapper.get_gui_size().ok()
        } else {
            None
        }
    }

    /// Create the plugin GUI (must be called before show)
    ///
    /// # Arguments
    /// * `is_floating` - True for floating window, false for embedded
    pub fn create_gui(&self, is_floating: bool) -> ClapResult<()> {
        let mut wrapper = self.wrapper.lock()
            .map_err(|_| ClapError::LockFailed { plugin_id: self.plugin_id.clone() })?;
        wrapper.create_gui(is_floating)
    }

    /// Show the plugin GUI
    pub fn show_gui(&self) -> ClapResult<()> {
        let mut wrapper = self.wrapper.lock()
            .map_err(|_| ClapError::LockFailed { plugin_id: self.plugin_id.clone() })?;
        wrapper.show_gui()
    }

    /// Hide the plugin GUI
    pub fn hide_gui(&self) -> ClapResult<()> {
        let mut wrapper = self.wrapper.lock()
            .map_err(|_| ClapError::LockFailed { plugin_id: self.plugin_id.clone() })?;
        wrapper.hide_gui()
    }

    /// Destroy the plugin GUI
    pub fn destroy_gui(&self) {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            wrapper.destroy_gui();
        }
    }

    /// Get the plugin ID
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Start learning mode - snapshot all current parameter values
    ///
    /// Call this when entering learning mode. This caches all parameter values
    /// so that subsequent polls can detect changes by comparing current values
    /// to the snapshot.
    pub fn start_learning_mode(&self) {
        log::info!("[CLAP_LEARN] ClapGuiHandle::start_learning_mode() called for plugin_id={}", self.plugin_id);

        // Drain any stale param change events from previous sessions
        let mut stale_count = 0;
        while self.param_change_receiver.try_recv().is_ok() {
            stale_count += 1;
        }
        if stale_count > 0 {
            log::info!("[CLAP_LEARN] Drained {} stale param change events", stale_count);
        }

        match self.wrapper.lock() {
            Ok(mut wrapper) => {
                log::info!("[CLAP_LEARN] Lock acquired, calling wrapper.start_learning_mode()");
                wrapper.start_learning_mode();
            }
            Err(e) => {
                log::warn!("[CLAP_LEARN] Failed to lock wrapper for start_learning_mode: {:?}", e);
            }
        }
    }

    /// Stop learning mode and clear the parameter cache
    pub fn stop_learning_mode(&self) {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            wrapper.stop_learning_mode();
        }
    }

    /// Poll for parameter changes from the plugin GUI
    ///
    /// Returns all pending parameter changes. Call this periodically from the UI
    /// thread to detect when the plugin's GUI modifies parameters (for learning mode).
    ///
    /// This method first triggers a parameter flush on the wrapper (if the plugin
    /// requested one via host->params->request_flush()), then drains the channel.
    pub fn poll_param_changes(&self) -> Vec<ParamChangeEvent> {
        // First, trigger any pending flush from the plugin GUI
        match self.wrapper.lock() {
            Ok(mut wrapper) => {
                wrapper.poll_gui_param_changes();
            }
            Err(e) => {
                log::warn!("[CLAP_LEARN] Failed to lock wrapper for {}: {:?}", self.plugin_id, e);
            }
        }

        // Now drain the channel for any param change events
        let mut changes = Vec::new();
        while let Ok(change) = self.param_change_receiver.try_recv() {
            log::info!(
                "[CLAP_LEARN] GUI handle received param change: plugin={}, param_id={}, value={}",
                self.plugin_id,
                change.param_id,
                change.value
            );
            changes.push(change);
        }

        if !changes.is_empty() {
            log::info!("[CLAP_LEARN] poll_param_changes returning {} changes", changes.len());
        }

        changes
    }

    /// Get the parameter name for a CLAP param ID
    pub fn param_name_for_id(&self, param_id: u32) -> Option<String> {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            // Find the param in the wrapper's info
            let params = wrapper.query_params();
            params.iter()
                .find(|p| p.id == param_id)
                .map(|p| p.name.clone())
        } else {
            None
        }
    }

    /// Get the current value of a parameter by its CLAP param ID
    ///
    /// Returns the value in the plugin's native range (not normalized).
    pub fn get_param_value(&self, param_id: u32) -> Option<f64> {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            wrapper.get_param_value(param_id)
        } else {
            None
        }
    }

    /// Get parameter info (min, max, default) for a CLAP param ID
    pub fn get_param_info(&self, param_id: u32) -> Option<(f64, f64, f64)> {
        if let Ok(mut wrapper) = self.wrapper.lock() {
            let params = wrapper.query_params();
            params.iter()
                .find(|p| p.id == param_id)
                .map(|p| (p.min, p.max, p.default))
        } else {
            None
        }
    }
}

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

    /// Create a CLAP effect with a GUI handle for plugin window hosting
    ///
    /// Returns both the effect (to be sent to the audio engine) and a GUI handle
    /// that can be used to open the plugin's native GUI and monitor parameter changes.
    ///
    /// The GUI handle shares the same `ClapPluginWrapper` as the effect via `Arc<Mutex<>>`,
    /// so parameter changes from either the GUI or the audio thread are synchronized.
    ///
    /// The GUI handle receives the parameter change notification channel, enabling
    /// learning mode: when the user adjusts a control in the plugin GUI, the change
    /// is detected via `gui_handle.poll_param_changes()`.
    pub fn create_effect_with_gui_handle(
        &mut self,
        plugin_id: &str,
    ) -> ClapResult<(Box<dyn crate::effect::Effect>, ClapGuiHandle)> {
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

        // Get or load the plugin bundle
        let bundle = self.get_or_load_bundle(&plugin_info.bundle_path)?;

        // Create the ClapEffect with a separate receiver for the GUI handle
        // The effect gets a dummy receiver, and the real receiver goes to the GUI handle
        let (effect, param_change_receiver) =
            ClapEffect::from_plugin_with_separate_receiver(&plugin_info, bundle)?;

        // Create GUI handle with the real param change receiver for learning mode
        let gui_handle = ClapGuiHandle {
            plugin_id: plugin_id.to_string(),
            wrapper: Arc::clone(effect.wrapper()),
            param_ids: effect.clap_param_ids().to_vec(),
            param_change_receiver,
        };

        Ok((Box::new(effect), gui_handle))
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
