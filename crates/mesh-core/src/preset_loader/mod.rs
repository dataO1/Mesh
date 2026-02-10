//! Centralized preset loader for background-thread multiband building
//!
//! Moves expensive plugin creation (CLAP/PD instantiation, DSP init) off the
//! UI thread and replaces 300-1000+ individual engine commands with a single
//! `SwapMultiband` command containing a fully-built `MultibandHost`.
//!
//! # Architecture
//!
//! ```text
//! UI Thread (fast)                   Loader Thread (slow)              Audio Thread
//! ─────────────────                  ────────────────────              ────────────
//! StemPresetConfig
//!   → MultibandBuildSpec   ──send──▶ build_multiband()
//!      (pure data)                    ├ create effects (CLAP/PD)
//!                                     ├ set all params
//!                                     ├ configure bands, dry/wet
//!                                     ├ add macro mappings
//!                                     └ return MultibandHost  ──sub──▶ UI receives result
//!                                                                      └ send SwapMultiband ──▶ atomic swap
//! ```

mod build;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use crate::effect::multiband::{EffectLocation, MacroMapping, MultibandHost};
use crate::types::Stem;

// Re-export build function for testing
pub use build::build_multiband;

// ─────────────────────────────────────────────────────────────────────────────
// Build Spec Types (pure data, no trait objects, Send + Sync)
// ─────────────────────────────────────────────────────────────────────────────

/// Specification for building a MultibandHost on the loader thread.
///
/// This is a pure data struct with no trait objects — it's `Send + Sync` and
/// can be cheaply constructed on the UI thread from a `StemPresetConfig`.
#[derive(Debug, Clone)]
pub struct MultibandBuildSpec {
    /// Crossover frequencies (N-1 for N bands)
    pub crossover_freqs: Vec<f32>,
    /// Band specifications
    pub bands: Vec<BandBuildSpec>,
    /// Pre-FX chain effects
    pub pre_fx: Vec<EffectBuildSpec>,
    /// Post-FX chain effects
    pub post_fx: Vec<EffectBuildSpec>,
    /// Pre-FX chain dry/wet (0.0 = dry, 1.0 = wet)
    pub pre_fx_chain_dry_wet: f32,
    /// Post-FX chain dry/wet (0.0 = dry, 1.0 = wet)
    pub post_fx_chain_dry_wet: f32,
    /// Global dry/wet for entire effect rack (0.0 = dry, 1.0 = wet)
    pub global_dry_wet: f32,
    /// Macro mappings: (macro_index, mapping spec)
    pub macro_mappings: Vec<(usize, MacroMappingSpec)>,
}

/// Specification for a single frequency band.
#[derive(Debug, Clone)]
pub struct BandBuildSpec {
    /// Band gain (linear, 0.0-2.0 typical)
    pub gain: f32,
    /// Whether this band is muted
    pub muted: bool,
    /// Whether this band is soloed
    pub soloed: bool,
    /// Chain dry/wet for entire band (0.0 = dry, 1.0 = wet)
    pub chain_dry_wet: f32,
    /// Effects in this band's chain
    pub effects: Vec<EffectBuildSpec>,
}

/// Effect source type for plugin creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectSourceType {
    /// Pure Data patch
    Pd,
    /// CLAP plugin
    Clap,
}

/// Specification for creating a single effect.
///
/// Contains all the data needed to create and configure an effect
/// without any trait objects.
#[derive(Debug, Clone)]
pub struct EffectBuildSpec {
    /// Plugin identifier (folder name for PD, plugin ID for CLAP)
    pub plugin_id: String,
    /// Effect source type
    pub source: EffectSourceType,
    /// All parameter values indexed by param_index
    pub params: Vec<(usize, f32)>,
    /// Whether the effect is bypassed
    pub bypass: bool,
    /// Per-effect dry/wet mix (0.0 = dry, 1.0 = wet)
    pub dry_wet: f32,
}

/// Specification for a macro mapping (pure data, no location enum dependency).
#[derive(Debug, Clone)]
pub struct MacroMappingSpec {
    /// Which chain the effect is in
    pub location: EffectLocation,
    /// Which effect in the chain
    pub effect_index: usize,
    /// Which parameter on the effect
    pub param_index: usize,
    /// Minimum value of the mapping range
    pub min_value: f32,
    /// Maximum value of the mapping range
    pub max_value: f32,
}

impl MacroMappingSpec {
    /// Convert to a MacroMapping for the MultibandHost
    pub fn to_macro_mapping(&self) -> MacroMapping {
        MacroMapping {
            location: self.location,
            effect_index: self.effect_index,
            param_index: self.param_index,
            min_value: self.min_value,
            max_value: self.max_value,
            name: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset Load Request / Result
// ─────────────────────────────────────────────────────────────────────────────

/// Request to build a MultibandHost on the loader thread.
pub struct PresetLoadRequest {
    /// Monotonic ID for stale detection
    pub id: u64,
    /// Deck index (0-3)
    pub deck: usize,
    /// Stem to load onto
    pub stem: Stem,
    /// Build specification (pure data)
    pub spec: MultibandBuildSpec,
    /// Audio buffer size for the MultibandHost
    pub buffer_size: usize,
}

/// Result of a preset load — a fully-built MultibandHost ready for swap.
pub struct PresetLoadResult {
    /// Monotonic ID matching the request
    pub id: u64,
    /// Deck index (0-3)
    pub deck: usize,
    /// Stem this was built for
    pub stem: Stem,
    /// The fully-configured MultibandHost (or error message)
    pub result: Result<MultibandHost, String>,
}

/// Type alias for the result receiver (used with iced subscriptions).
pub type PresetLoadResultReceiver = Arc<Mutex<Receiver<PresetLoadResult>>>;

// ─────────────────────────────────────────────────────────────────────────────
// PresetLoader — Background thread
// ─────────────────────────────────────────────────────────────────────────────

/// Background thread that builds MultibandHost instances from build specs.
///
/// Follows the same pattern as `TrackLoader` and `LinkedStemLoader`:
/// - `spawn()` creates the thread with its own ClapManager + PdManager
/// - `load()` sends a request (non-blocking)
/// - `result_receiver()` returns an `Arc<Mutex<Receiver>>` for subscriptions
///
/// The loader thread owns its own plugin managers because plugin creation
/// is not thread-safe — each thread needs its own instances.
pub struct PresetLoader {
    /// Channel to send load requests
    tx: Sender<PresetLoadRequest>,
    /// Channel to receive load results
    rx: PresetLoadResultReceiver,
    /// Monotonic counter for stale detection
    next_id: AtomicU64,
    /// Thread handle (for graceful shutdown)
    _handle: JoinHandle<()>,
}

impl PresetLoader {
    /// Spawn the preset loader thread.
    ///
    /// # Arguments
    /// * `collection_path` - Path to mesh collection root (for PD effects)
    /// * `clap_extra_paths` - Additional CLAP search paths (e.g., collection/effects/clap)
    pub fn spawn(collection_path: PathBuf, clap_extra_paths: Vec<PathBuf>) -> Self {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<PresetLoadRequest>();
        let (result_tx, result_rx) = std::sync::mpsc::channel::<PresetLoadResult>();

        let handle = thread::Builder::new()
            .name("preset-loader".to_string())
            .spawn(move || {
                loader_thread(request_rx, result_tx, collection_path, clap_extra_paths);
            })
            .expect("Failed to spawn preset loader thread");

        log::info!("[PRESET_LOADER] Spawned background preset loader thread");

        Self {
            tx: request_tx,
            rx: Arc::new(Mutex::new(result_rx)),
            next_id: AtomicU64::new(1),
            _handle: handle,
        }
    }

    /// Request loading a preset (non-blocking).
    ///
    /// Returns the request ID for stale detection.
    pub fn load(&self, deck: usize, stem: Stem, spec: MultibandBuildSpec, buffer_size: usize) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = PresetLoadRequest {
            id,
            deck,
            stem,
            spec,
            buffer_size,
        };

        if let Err(e) = self.tx.send(request) {
            log::error!("[PRESET_LOADER] Failed to send load request: {}", e);
        } else {
            log::info!("[PRESET_LOADER] Sent load request id={} for deck {} stem {:?}", id, deck, stem);
        }

        id
    }

    /// Get the result receiver for subscription-based message handling.
    pub fn result_receiver(&self) -> PresetLoadResultReceiver {
        self.rx.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loader Thread
// ─────────────────────────────────────────────────────────────────────────────

/// The background loader thread function.
///
/// Creates its own ClapManager + PdManager instances for thread-safe
/// plugin creation. Receives build specs, creates MultibandHosts, and
/// sends them back.
fn loader_thread(
    rx: Receiver<PresetLoadRequest>,
    tx: Sender<PresetLoadResult>,
    collection_path: PathBuf,
    clap_extra_paths: Vec<PathBuf>,
) {
    log::info!("[PRESET_LOADER] Loader thread started, initializing plugin managers...");

    // Create our own PdManager (PD is not thread-safe, needs own instance)
    let mut pd_manager = crate::pd::PdManager::new(&collection_path)
        .unwrap_or_else(|e| {
            log::warn!("[PRESET_LOADER] Failed to init PdManager: {}. PD effects unavailable.", e);
            crate::pd::PdManager::default()
        });

    // Create our own ClapManager (plugin bundles may be shared but wrappers are not)
    let mut clap_manager = crate::clap::ClapManager::new();
    for path in &clap_extra_paths {
        if path.exists() {
            clap_manager.add_search_path(path.clone());
        }
    }
    clap_manager.scan_plugins();

    log::info!(
        "[PRESET_LOADER] Plugin managers ready. PD effects: {}, CLAP plugins: {}",
        pd_manager.available_effects().len(),
        clap_manager.available_plugins().len()
    );

    while let Ok(request) = rx.recv() {
        let start = std::time::Instant::now();
        log::info!(
            "[PRESET_LOADER] Building multiband id={} for deck {} stem {:?} ({} bands, {} pre-fx, {} post-fx)",
            request.id, request.deck, request.stem,
            request.spec.bands.len(),
            request.spec.pre_fx.len(),
            request.spec.post_fx.len(),
        );

        let result = build::build_multiband(
            &request.spec,
            request.buffer_size,
            &mut clap_manager,
            &mut pd_manager,
        );

        let elapsed = start.elapsed();
        match &result {
            Ok(_) => log::info!(
                "[PRESET_LOADER] Built multiband id={} in {:?}",
                request.id, elapsed
            ),
            Err(e) => log::error!(
                "[PRESET_LOADER] Failed to build multiband id={}: {} (took {:?})",
                request.id, e, elapsed
            ),
        }

        let _ = tx.send(PresetLoadResult {
            id: request.id,
            deck: request.deck,
            stem: request.stem,
            result,
        });
    }

    log::info!("[PRESET_LOADER] Loader thread shutting down");
}
