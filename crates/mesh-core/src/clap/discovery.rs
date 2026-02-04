//! CLAP plugin discovery
//!
//! Scans standard CLAP plugin directories to find available plugins.
//! Caches plugin metadata for fast subsequent lookups.

use std::path::{Path, PathBuf};
use std::collections::HashMap;

use super::error::{ClapError, ClapResult};

/// Standard CLAP plugin search paths on Linux
const CLAP_SEARCH_PATHS: &[&str] = &[
    // User plugins
    "~/.clap",
    // System plugins
    "/usr/lib/clap",
    "/usr/local/lib/clap",
    // Flatpak/snap paths
    "~/.var/app/*/data/clap",
];

/// Plugin category derived from CLAP features
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClapPluginCategory {
    /// Audio effects (filters, delays, etc.)
    AudioEffect,
    /// Dynamics processors (compressors, limiters, gates)
    Dynamics,
    /// Distortion and saturation
    Distortion,
    /// Filters and EQ
    Filter,
    /// Reverb and spatial effects
    Reverb,
    /// Delay effects
    Delay,
    /// Modulation effects (chorus, flanger, phaser)
    Modulation,
    /// Analyzer and metering
    Analyzer,
    /// Instruments/synthesizers
    Instrument,
    /// Utility plugins
    Utility,
    /// Unknown/other
    Other,
}

impl Default for ClapPluginCategory {
    fn default() -> Self {
        Self::Other
    }
}

impl std::fmt::Display for ClapPluginCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AudioEffect => write!(f, "Effect"),
            Self::Dynamics => write!(f, "Dynamics"),
            Self::Distortion => write!(f, "Distortion"),
            Self::Filter => write!(f, "Filter"),
            Self::Reverb => write!(f, "Reverb"),
            Self::Delay => write!(f, "Delay"),
            Self::Modulation => write!(f, "Modulation"),
            Self::Analyzer => write!(f, "Analyzer"),
            Self::Instrument => write!(f, "Instrument"),
            Self::Utility => write!(f, "Utility"),
            Self::Other => write!(f, "Other"),
        }
    }
}

/// Information about a discovered CLAP plugin
#[derive(Debug, Clone)]
pub struct DiscoveredClapPlugin {
    /// Unique plugin identifier (e.g., "org.lsp-plug.compressor-stereo")
    pub id: String,
    /// Display name
    pub name: String,
    /// Plugin vendor/author
    pub vendor: String,
    /// Plugin version string
    pub version: String,
    /// Path to the .clap bundle
    pub bundle_path: PathBuf,
    /// Plugin category
    pub category: ClapPluginCategory,
    /// Number of parameters
    pub param_count: usize,
    /// Reported latency in samples (0 if not known)
    pub latency_samples: u32,
    /// Description if available
    pub description: Option<String>,
    /// Plugin features (CLAP feature strings)
    pub features: Vec<String>,
    /// Whether the plugin loaded successfully
    pub available: bool,
    /// Error message if plugin failed to load
    pub error_message: Option<String>,
}

impl DiscoveredClapPlugin {
    /// Create a placeholder for a plugin that failed to load
    pub fn unavailable(bundle_path: PathBuf, error: String) -> Self {
        let name = bundle_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        Self {
            id: format!("unknown:{}", name),
            name,
            vendor: "Unknown".to_string(),
            version: "0.0.0".to_string(),
            bundle_path,
            category: ClapPluginCategory::Other,
            param_count: 0,
            latency_samples: 0,
            description: None,
            features: vec![],
            available: false,
            error_message: Some(error),
        }
    }

    /// Get category as string for UI display
    pub fn category_name(&self) -> &str {
        match self.category {
            ClapPluginCategory::AudioEffect => "Effect",
            ClapPluginCategory::Dynamics => "Dynamics",
            ClapPluginCategory::Distortion => "Distortion",
            ClapPluginCategory::Filter => "Filter",
            ClapPluginCategory::Reverb => "Reverb",
            ClapPluginCategory::Delay => "Delay",
            ClapPluginCategory::Modulation => "Modulation",
            ClapPluginCategory::Analyzer => "Analyzer",
            ClapPluginCategory::Instrument => "Instrument",
            ClapPluginCategory::Utility => "Utility",
            ClapPluginCategory::Other => "Other",
        }
    }
}

/// CLAP plugin discovery and caching
pub struct ClapDiscovery {
    /// Search paths for CLAP plugins
    search_paths: Vec<PathBuf>,
    /// Discovered plugins (plugin_id -> info)
    plugins: HashMap<String, DiscoveredClapPlugin>,
    /// All discovered plugins in order
    plugins_list: Vec<DiscoveredClapPlugin>,
    /// Whether discovery has been run
    scanned: bool,
}

impl ClapDiscovery {
    /// Create a new discovery instance with default search paths
    pub fn new() -> Self {
        let search_paths = Self::default_search_paths();
        Self {
            search_paths,
            plugins: HashMap::new(),
            plugins_list: Vec::new(),
            scanned: false,
        }
    }

    /// Create with custom search paths (for testing)
    pub fn with_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            search_paths: paths,
            plugins: HashMap::new(),
            plugins_list: Vec::new(),
            scanned: false,
        }
    }

    /// Get default search paths, expanding ~ to home directory
    fn default_search_paths() -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home"));

        CLAP_SEARCH_PATHS
            .iter()
            .filter_map(|p| {
                if p.starts_with("~") {
                    Some(home.join(&p[2..]))
                } else {
                    Some(PathBuf::from(p))
                }
            })
            .filter(|p| p.exists())
            .collect()
    }

    /// Add a custom search path
    pub fn add_search_path(&mut self, path: PathBuf) {
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
            // Invalidate cache
            self.scanned = false;
        }
    }

    /// Get current search paths
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Scan for CLAP plugins
    ///
    /// This scans all search paths for .clap bundles and loads their metadata.
    /// Results are cached - call `rescan()` to force a refresh.
    pub fn scan(&mut self) -> &[DiscoveredClapPlugin] {
        if self.scanned {
            return &self.plugins_list;
        }

        self.plugins.clear();
        self.plugins_list.clear();

        log::info!(
            "Starting CLAP plugin scan ({} search path(s))",
            self.search_paths.len()
        );
        for (i, path) in self.search_paths.iter().enumerate() {
            log::debug!("  Search path {}: {:?}", i + 1, path);
        }

        if self.search_paths.is_empty() {
            log::warn!("No CLAP search paths configured. Add paths or install plugins to ~/.clap");
        }

        for search_path in &self.search_paths.clone() {
            if let Err(e) = self.scan_directory(search_path) {
                log::warn!("Failed to scan CLAP directory {:?}: {}", search_path, e);
            }
        }

        // Sort by name for consistent ordering
        self.plugins_list.sort_by(|a, b| a.name.cmp(&b.name));

        let available = self.plugins_list.iter().filter(|p| p.available).count();
        log::info!(
            "CLAP scan complete: {} plugin(s) found, {} available",
            self.plugins_list.len(),
            available
        );

        self.scanned = true;
        &self.plugins_list
    }

    /// Force a rescan of plugin directories
    pub fn rescan(&mut self) -> &[DiscoveredClapPlugin] {
        self.scanned = false;
        self.scan()
    }

    /// Scan a single directory for .clap bundles
    fn scan_directory(&mut self, dir: &Path) -> ClapResult<()> {
        if !dir.exists() {
            return Ok(());
        }

        log::info!("Scanning CLAP directory: {:?}", dir);

        let mut bundle_count = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Check for .clap extension (bundles are directories on Linux)
            if path.extension().map(|e| e == "clap").unwrap_or(false) {
                bundle_count += 1;
                log::debug!("Found CLAP bundle: {:?}", path);

                match self.scan_bundle(&path) {
                    Ok(plugins) => {
                        for plugin in plugins {
                            log::info!(
                                "Discovered CLAP plugin: {} ({}) from {:?}",
                                plugin.name,
                                plugin.id,
                                path
                            );
                            self.plugins.insert(plugin.id.clone(), plugin.clone());
                            self.plugins_list.push(plugin);
                        }
                    }
                    Err(e) => {
                        let error_str = e.to_string();
                        // Check for common missing library errors
                        if error_str.contains("cannot open shared object file") {
                            log::error!(
                                "CLAP plugin {:?} failed to load - missing system library. \
                                Install the required dependency and restart.",
                                path.file_name().unwrap_or_default()
                            );
                            log::error!("  Error: {}", error_str);
                        } else {
                            log::warn!("Failed to scan CLAP bundle {:?}: {}", path, e);
                        }
                        // Add as unavailable
                        let unavailable = DiscoveredClapPlugin::unavailable(path, error_str);
                        self.plugins_list.push(unavailable);
                    }
                }
            }
        }

        if bundle_count == 0 {
            log::info!("No .clap bundles found in {:?}", dir);
        } else {
            log::info!("Scanned {} CLAP bundle(s) in {:?}", bundle_count, dir);
        }

        Ok(())
    }

    /// Convert a CStr to String, handling potential UTF-8 errors
    fn cstr_to_string(cstr: &std::ffi::CStr) -> String {
        cstr.to_str().unwrap_or("").to_string()
    }

    /// Scan a single .clap bundle for plugins
    ///
    /// A bundle can contain multiple plugins, so this returns a Vec.
    fn scan_bundle(&self, bundle_path: &Path) -> ClapResult<Vec<DiscoveredClapPlugin>> {
        // Use clack-host to load the bundle and enumerate plugins
        use clack_host::bundle::PluginBundle;

        let bundle = unsafe {
            PluginBundle::load(bundle_path).map_err(|e| ClapError::BundleLoadFailed {
                path: bundle_path.to_path_buf(),
                reason: format!("{:?}", e),
            })?
        };

        let factory = bundle.get_plugin_factory().ok_or_else(|| {
            ClapError::BundleLoadFailed {
                path: bundle_path.to_path_buf(),
                reason: "No plugin factory found".to_string(),
            }
        })?;

        let mut plugins = Vec::new();

        for descriptor in factory.plugin_descriptors() {
            let id = descriptor.id().map(Self::cstr_to_string).unwrap_or_default();
            let name = descriptor.name().map(Self::cstr_to_string).unwrap_or_else(|| id.clone());
            let vendor = descriptor.vendor().map(Self::cstr_to_string).unwrap_or_default();
            let version = descriptor.version().map(Self::cstr_to_string).unwrap_or_default();
            let description = descriptor.description().map(Self::cstr_to_string);

            // Collect features
            let features: Vec<String> = descriptor
                .features()
                .map(Self::cstr_to_string)
                .collect();

            // Determine category from features
            let category = Self::category_from_features(&features);

            plugins.push(DiscoveredClapPlugin {
                id,
                name,
                vendor,
                version,
                bundle_path: bundle_path.to_path_buf(),
                category,
                param_count: 0, // Will be filled when plugin is loaded
                latency_samples: 0,
                description,
                features,
                available: true,
                error_message: None,
            });
        }

        Ok(plugins)
    }

    /// Determine plugin category from CLAP feature strings
    ///
    /// Specific categories (compressor, reverb, etc.) take priority over
    /// generic "audio-effect" features. This ensures plugins that advertise
    /// both get categorized by their specific function.
    fn category_from_features(features: &[String]) -> ClapPluginCategory {
        // Lowercase all features once for efficient comparison
        let features_lower: Vec<String> = features.iter().map(|f| f.to_lowercase()).collect();

        // First pass: check for specific categories (highest priority)
        for f in &features_lower {
            // Dynamics
            if f.contains("compressor")
                || f.contains("limiter")
                || f.contains("gate")
                || f.contains("expander")
                || f.contains("dynamics")
            {
                return ClapPluginCategory::Dynamics;
            }

            // Distortion
            if f.contains("distortion")
                || f.contains("overdrive")
                || f.contains("saturation")
                || f.contains("waveshaper")
                || f.contains("clipper")
            {
                return ClapPluginCategory::Distortion;
            }

            // Filter/EQ
            if f.contains("filter") || f.contains("equalizer") || f.contains("eq") {
                return ClapPluginCategory::Filter;
            }

            // Reverb
            if f.contains("reverb") {
                return ClapPluginCategory::Reverb;
            }

            // Delay
            if f.contains("delay") || f.contains("echo") {
                return ClapPluginCategory::Delay;
            }

            // Modulation
            if f.contains("chorus")
                || f.contains("flanger")
                || f.contains("phaser")
                || f.contains("modulation")
                || f.contains("vibrato")
                || f.contains("tremolo")
            {
                return ClapPluginCategory::Modulation;
            }

            // Analyzer
            if f.contains("analyzer") || f.contains("meter") || f.contains("spectrum") {
                return ClapPluginCategory::Analyzer;
            }

            // Instrument
            if f.contains("instrument") || f.contains("synthesizer") || f.contains("synth") {
                return ClapPluginCategory::Instrument;
            }

            // Utility
            if f.contains("utility") || f.contains("tool") {
                return ClapPluginCategory::Utility;
            }
        }

        // Second pass: check for generic audio-effect (lower priority)
        for f in &features_lower {
            if f.contains("audio-effect") || f.contains("effect") {
                return ClapPluginCategory::AudioEffect;
            }
        }

        ClapPluginCategory::Other
    }

    /// Get a plugin by ID
    pub fn get_plugin(&self, plugin_id: &str) -> Option<&DiscoveredClapPlugin> {
        self.plugins.get(plugin_id)
    }

    /// Get all discovered plugins
    pub fn discovered_plugins(&self) -> &[DiscoveredClapPlugin] {
        &self.plugins_list
    }

    /// Get only available (successfully loaded) plugins
    pub fn available_plugins(&self) -> Vec<&DiscoveredClapPlugin> {
        self.plugins_list.iter().filter(|p| p.available).collect()
    }

    /// Get plugins by category
    pub fn plugins_by_category(&self, category: ClapPluginCategory) -> Vec<&DiscoveredClapPlugin> {
        self.plugins_list
            .iter()
            .filter(|p| p.available && p.category == category)
            .collect()
    }

    /// Check if any plugins were found
    pub fn has_plugins(&self) -> bool {
        !self.plugins_list.is_empty()
    }

    /// Get count of available plugins
    pub fn available_count(&self) -> usize {
        self.plugins_list.iter().filter(|p| p.available).count()
    }
}

impl Default for ClapDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_category_from_features() {
        let features = vec!["audio-effect".to_string(), "compressor".to_string()];
        assert_eq!(
            ClapDiscovery::category_from_features(&features),
            ClapPluginCategory::Dynamics
        );

        let features = vec!["audio-effect".to_string(), "reverb".to_string()];
        assert_eq!(
            ClapDiscovery::category_from_features(&features),
            ClapPluginCategory::Reverb
        );

        let features = vec!["audio-effect".to_string()];
        assert_eq!(
            ClapDiscovery::category_from_features(&features),
            ClapPluginCategory::AudioEffect
        );
    }

    #[test]
    fn test_default_search_paths() {
        let paths = ClapDiscovery::default_search_paths();
        // At least the home path should be attempted
        // (it may not exist, but should be in the list conceptually)
        assert!(paths.iter().all(|p| !p.to_string_lossy().contains("~")));
    }

    #[test]
    fn test_unavailable_plugin() {
        let plugin = DiscoveredClapPlugin::unavailable(
            PathBuf::from("/usr/lib/clap/broken.clap"),
            "Load failed".to_string(),
        );
        assert!(!plugin.available);
        assert!(plugin.error_message.is_some());
        assert_eq!(plugin.name, "broken");
    }
}
