//! Domain layer for mesh-player
//!
//! This module orchestrates all services and manages application state.
//! It provides a clean separation between:
//! - **UI Layer**: Display and user input only
//! - **Domain Layer**: Business logic, service orchestration, state management
//! - **Service Layer**: Low-level services (database, audio engine, USB)
//!
//! ## Key Responsibilities
//!
//! - Manages active storage source (local vs USB)
//! - Loads track metadata from the correct database
//! - Coordinates track loading with the TrackLoader
//! - Forwards commands to the audio engine
//! - Owns services: CommandSender, TrackLoader, PeaksComputer, UsbManager
//!
//! ## Usage
//!
//! ```ignore
//! // UI calls domain methods instead of accessing services directly
//! domain.load_track_to_deck(0, &path);
//! domain.set_active_storage(StorageSource::Usb { index: 0 });
//! domain.toggle_play(deck);
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;

use basedrop::Shared;
use mesh_core::audio_file::{StemBuffers, TrackMetadata};
use mesh_core::config::LoudnessConfig;
use mesh_core::db::DatabaseService;
use mesh_core::effect::Effect;
use mesh_core::engine::{EngineCommand, LinkedStemData, PreparedTrack, SlicerPreset};
use mesh_core::loader::LinkedStemResultReceiver;
use mesh_core::clap::{ClapManager, ClapPluginCategory, DiscoveredClapPlugin};
use mesh_core::pd::{DiscoveredEffect, PdManager};
use mesh_core::usb::get_or_open_usb_database;
use mesh_core::types::{Stem, StereoBuffer};
use mesh_core::usb::{UsbCommand, UsbManager, UsbMessage};
use mesh_widgets::{PeaksComputer, PeaksResultReceiver};

use crate::audio::CommandSender;
use crate::loader::{TrackLoader, TrackLoadResultReceiver};

/// Type alias for USB message receiver
pub type UsbMessageReceiver = Arc<Mutex<Receiver<UsbMessage>>>;

/// Active storage source for track browsing and loading
#[derive(Clone)]
pub enum StorageSource {
    /// Local collection (default)
    Local,
    /// USB storage with its own database
    Usb {
        /// USB device index
        index: usize,
        /// Path to USB collection root
        path: PathBuf,
        /// Database service for this USB
        db: Arc<DatabaseService>,
    },
}

impl std::fmt::Debug for StorageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => write!(f, "Local"),
            Self::Usb { index, path, .. } => {
                f.debug_struct("Usb")
                    .field("index", index)
                    .field("path", path)
                    .finish_non_exhaustive()
            }
        }
    }
}

impl Default for StorageSource {
    fn default() -> Self {
        Self::Local
    }
}

/// Domain layer that orchestrates all services
///
/// This struct manages the interaction between different services
/// and ensures the correct database is used for each operation.
pub struct MeshDomain {
    /// Local database service (always available)
    local_db: Arc<DatabaseService>,

    /// Currently active storage source
    active_storage: StorageSource,

    /// Local collection path
    local_collection_path: PathBuf,

    // =========================================================================
    // Services (moved from MeshApp)
    // =========================================================================

    /// Command sender for lock-free communication with audio engine
    /// Uses an SPSC ringbuffer - no mutex, no dropouts, guaranteed delivery
    command_sender: Option<CommandSender>,

    /// Background track loader (avoids blocking UI/audio during loads)
    track_loader: TrackLoader,

    /// Background peak computer (offloads expensive waveform peak computation)
    peaks_computer: PeaksComputer,

    /// USB manager for device detection and browsing
    usb_manager: UsbManager,

    /// Linked stem result receiver (engine owns the loader, we receive results)
    linked_stem_receiver: Option<LinkedStemResultReceiver>,

    /// PD effect manager (discovers and creates Pure Data effects)
    pd_manager: PdManager,

    /// CLAP effect manager (discovers and creates CLAP plugin effects)
    clap_manager: ClapManager,

    // =========================================================================
    // Domain State (caches and computed values)
    // =========================================================================

    /// Stem buffers for waveform recomputation (Shared for RT-safe deallocation)
    deck_stems: [Option<Shared<StemBuffers>>; 4],

    /// Linked stem buffers per deck per stem [deck_idx][stem_idx]
    /// Used for zoomed waveform visualization of active linked stems
    deck_linked_stems: [[Option<Shared<StereoBuffer>>; 4]; 4],

    /// Track LUFS per deck (cached from TrackLoaded for LinkedStemLoaded)
    /// Used to avoid race conditions when passing host_lufs to LinkStem command
    track_lufs_per_deck: [Option<f32>; 4],

    /// Global BPM (cached for UI display; authoritative value is in audio engine)
    global_bpm: f64,
}

impl MeshDomain {
    /// Create a new domain layer with all services
    ///
    /// # Arguments
    /// * `local_db` - Database service for local collection
    /// * `local_collection_path` - Path to local collection root
    /// * `command_sender` - Lock-free command channel for engine control (None for offline mode)
    /// * `linked_stem_receiver` - Receiver for linked stem load results (engine owns the loader)
    /// * `sample_rate` - Audio system's sample rate for track loading
    /// * `initial_global_bpm` - Initial global BPM from config
    pub fn new(
        local_db: Arc<DatabaseService>,
        local_collection_path: PathBuf,
        command_sender: Option<CommandSender>,
        linked_stem_receiver: Option<LinkedStemResultReceiver>,
        sample_rate: u32,
        initial_global_bpm: f64,
    ) -> Self {
        // Initialize PD effect manager (discovers effects at startup)
        let pd_manager = PdManager::new(&local_collection_path)
            .unwrap_or_else(|e| {
                log::warn!("Failed to initialize PdManager: {}. PD effects will be unavailable.", e);
                PdManager::default()
            });

        // Initialize CLAP effect manager (scans system + collection CLAP directories)
        let mut clap_manager = ClapManager::new();
        // Add mesh-collection/plugins/clap as a search path
        let collection_clap_path = local_collection_path.join("plugins").join("clap");
        if collection_clap_path.exists() {
            log::info!("Adding CLAP search path: {:?}", collection_clap_path);
            clap_manager.add_search_path(collection_clap_path);
        } else {
            // Create the directory for user convenience
            if let Err(e) = std::fs::create_dir_all(&collection_clap_path) {
                log::warn!("Failed to create CLAP plugins directory: {}", e);
            } else {
                log::info!("Created CLAP plugins directory: {:?}", collection_clap_path);
                clap_manager.add_search_path(collection_clap_path);
            }
        }
        clap_manager.scan_plugins();
        log::info!(
            "ClapManager initialized: found {} plugins ({} available)",
            clap_manager.discovered_plugins().len(),
            clap_manager.available_plugins().len()
        );

        Self {
            local_db: local_db.clone(),
            active_storage: StorageSource::Local,
            local_collection_path,
            // Services
            command_sender,
            track_loader: TrackLoader::spawn(sample_rate),
            peaks_computer: PeaksComputer::spawn(),
            usb_manager: UsbManager::spawn(Some(local_db)),
            linked_stem_receiver,
            pd_manager,
            clap_manager,
            // Domain state
            deck_stems: [None, None, None, None],
            deck_linked_stems: std::array::from_fn(|_| [None, None, None, None]),
            track_lufs_per_deck: [None, None, None, None],
            global_bpm: initial_global_bpm,
        }
    }

    // =========================================================================
    // Storage Management
    // =========================================================================

    /// Get the currently active storage source
    pub fn active_storage(&self) -> &StorageSource {
        &self.active_storage
    }

    /// Set the active storage source
    ///
    /// This determines which database is used for metadata lookups.
    pub fn set_active_storage(&mut self, source: StorageSource) {
        log::info!("Domain: Switching storage to {:?}", match &source {
            StorageSource::Local => "Local".to_string(),
            StorageSource::Usb { index, path, .. } => format!("USB {} at {:?}", index, path),
        });
        self.active_storage = source;
    }

    /// Switch to local storage
    pub fn switch_to_local(&mut self) {
        self.set_active_storage(StorageSource::Local);
    }

    /// Switch to USB storage
    ///
    /// Uses the centralized USB database cache - the database is typically already
    /// cached from UsbManager's preload when the device was mounted.
    pub fn switch_to_usb(&mut self, index: usize, usb_collection_path: &Path) -> Result<(), String> {
        // Get cached database or open if not cached yet
        let db = get_or_open_usb_database(usb_collection_path)
            .ok_or_else(|| format!("Failed to open USB database at {:?}", usb_collection_path))?;

        self.set_active_storage(StorageSource::Usb {
            index,
            path: usb_collection_path.to_path_buf(),
            db,
        });

        Ok(())
    }

    /// Get the active database service
    ///
    /// Returns the database for the currently active storage source.
    pub fn active_db(&self) -> &DatabaseService {
        match &self.active_storage {
            StorageSource::Local => &self.local_db,
            StorageSource::Usb { db, .. } => db,
        }
    }

    /// Get the local database service
    pub fn local_db(&self) -> &DatabaseService {
        &self.local_db
    }

    /// Get the active collection path
    pub fn active_collection_path(&self) -> &Path {
        match &self.active_storage {
            StorageSource::Local => &self.local_collection_path,
            StorageSource::Usb { path, .. } => path,
        }
    }

    // =========================================================================
    // Track Metadata
    // =========================================================================

    /// Load track metadata from the active database
    ///
    /// This is the primary method for getting metadata before loading a track.
    /// It automatically uses the correct database based on active storage.
    ///
    /// Returns TrackMetadata with stem_links properly converted (ID → path).
    pub fn load_track_metadata(&self, path: &str) -> Option<TrackMetadata> {
        let db = self.active_db();
        // Use DatabaseService API that returns TrackMetadata with stem_links converted
        match db.get_track_metadata(path) {
            Ok(Some(metadata)) => Some(metadata),
            Ok(None) => {
                log::warn!("Track not found in active database: {}", path);
                None
            }
            Err(e) => {
                log::error!("Failed to load track metadata: {}", e);
                None
            }
        }
    }

    /// Load track metadata, falling back to defaults if not found
    pub fn load_track_metadata_or_default(&self, path: &str) -> TrackMetadata {
        self.load_track_metadata(path).unwrap_or_default()
    }

    // =========================================================================
    // Storage Detection
    // =========================================================================

    /// Check if a path belongs to a USB storage
    pub fn is_usb_path(&self, path: &Path) -> bool {
        // USB paths typically start with /run/media or similar
        // This is a heuristic - UsbManager has more precise detection
        if let Some(path_str) = path.to_str() {
            path_str.starts_with("/run/media/")
                || path_str.starts_with("/media/")
                || path_str.starts_with("/mnt/")
        } else {
            false
        }
    }

    /// Check if currently browsing USB storage
    pub fn is_browsing_usb(&self) -> bool {
        matches!(self.active_storage, StorageSource::Usb { .. })
    }

    // =========================================================================
    // Service Accessors
    // =========================================================================

    /// Check if audio engine is connected
    pub fn is_audio_connected(&self) -> bool {
        self.command_sender.is_some()
    }

    /// Send a command to the audio engine
    ///
    /// Returns true if the command was sent, false if the engine is not connected.
    pub fn send_command(&mut self, command: EngineCommand) -> bool {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(command);
            true
        } else {
            false
        }
    }

    /// Get the track loader's result receiver for subscriptions
    pub fn track_loader_result_receiver(&self) -> TrackLoadResultReceiver {
        self.track_loader.result_receiver()
    }

    /// Get the peaks computer's result receiver for subscriptions
    pub fn peaks_result_receiver(&self) -> PeaksResultReceiver {
        self.peaks_computer.result_receiver()
    }

    /// Get the USB manager's message receiver for subscriptions
    pub fn usb_message_receiver(&self) -> UsbMessageReceiver {
        self.usb_manager.message_receiver()
    }

    /// Get the linked stem result receiver for subscriptions
    ///
    /// Returns None if audio engine is not connected (offline mode).
    pub fn linked_stem_result_receiver(&self) -> Option<&LinkedStemResultReceiver> {
        self.linked_stem_receiver.as_ref()
    }

    /// Send a command to the USB manager
    pub fn send_usb_command(&self, command: UsbCommand) -> Result<(), String> {
        self.usb_manager
            .send(command)
            .map_err(|e| format!("Failed to send USB command: {}", e))
    }

    // =========================================================================
    // Domain State Accessors
    // =========================================================================

    /// Get current global BPM
    pub fn global_bpm(&self) -> f64 {
        self.global_bpm
    }

    /// Set global BPM (updates cache, doesn't send to engine - caller handles that)
    pub fn set_global_bpm(&mut self, bpm: f64) {
        self.global_bpm = bpm;
    }

    /// Get deck stem buffers for waveform rendering
    pub fn deck_stems(&self) -> &[Option<Shared<StemBuffers>>; 4] {
        &self.deck_stems
    }

    /// Set deck stems (called when track is loaded)
    pub fn set_deck_stems(&mut self, deck: usize, stems: Option<Shared<StemBuffers>>) {
        if deck < 4 {
            self.deck_stems[deck] = stems;
        }
    }

    /// Get linked stem buffer for a specific deck and stem
    pub fn deck_linked_stem(&self, deck: usize, stem: usize) -> Option<&Shared<StereoBuffer>> {
        self.deck_linked_stems.get(deck)?.get(stem)?.as_ref()
    }

    /// Set linked stem buffer (called when linked stem is loaded)
    pub fn set_deck_linked_stem(&mut self, deck: usize, stem: usize, buffer: Option<Shared<StereoBuffer>>) {
        if deck < 4 && stem < 4 {
            self.deck_linked_stems[deck][stem] = buffer;
        }
    }

    /// Get track LUFS for a deck
    pub fn track_lufs(&self, deck: usize) -> Option<f32> {
        self.track_lufs_per_deck.get(deck).copied().flatten()
    }

    /// Set track LUFS (called when track is loaded)
    pub fn set_track_lufs(&mut self, deck: usize, lufs: Option<f32>) {
        if deck < 4 {
            self.track_lufs_per_deck[deck] = lufs;
        }
    }

    // =========================================================================
    // Track Loading (delegates to TrackLoader)
    // =========================================================================

    /// Request loading a track (non-blocking)
    ///
    /// Uses the active database to load metadata, then sends to background loader.
    pub fn request_track_load(&mut self, deck_idx: usize, path: PathBuf) -> Result<(), String> {
        let metadata = self.load_track_metadata_or_default(path.to_str().unwrap_or_default());
        self.track_loader
            .load(deck_idx, path, metadata)
            .map_err(|e| format!("Failed to request track load: {}", e))
    }

    /// Update track loader's sample rate (if audio system rate changes)
    pub fn set_track_loader_sample_rate(&self, sample_rate: u32) {
        self.track_loader.set_sample_rate(sample_rate);
    }

    // =========================================================================
    // Peaks Computing (delegates to PeaksComputer)
    // =========================================================================

    /// Request peaks computation (non-blocking)
    pub fn request_peaks_compute(&self, request: mesh_widgets::PeaksComputeRequest) -> Result<(), String> {
        self.peaks_computer
            .compute(request)
            .map_err(|e| format!("Failed to request peaks compute: {}", e))
    }

    // =========================================================================
    // Deck Control - Playback
    // =========================================================================

    /// Toggle play/pause on a deck
    pub fn toggle_play(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::TogglePlay { deck });
        }
    }

    /// Start playback on a deck
    pub fn play(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::Play { deck });
        }
    }

    /// Pause playback on a deck
    pub fn pause(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::Pause { deck });
        }
    }

    /// Seek to a specific sample position
    pub fn seek(&mut self, deck: usize, position: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::Seek { deck, position });
        }
    }

    // =========================================================================
    // Deck Control - CDJ-Style Cueing
    // =========================================================================

    /// CDJ-style cue button press
    pub fn cue_press(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::CuePress { deck });
        }
    }

    /// CDJ-style cue button release
    pub fn cue_release(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::CueRelease { deck });
        }
    }

    /// Set cue point at current position
    pub fn set_cue_point(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetCuePoint { deck });
        }
    }

    // =========================================================================
    // Deck Control - Hot Cues
    // =========================================================================

    /// Hot cue button press (set/jump depending on state)
    pub fn hot_cue_press(&mut self, deck: usize, slot: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::HotCuePress { deck, slot });
        }
    }

    /// Hot cue button release
    pub fn hot_cue_release(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::HotCueRelease { deck });
        }
    }

    /// Clear a hot cue slot
    pub fn clear_hot_cue(&mut self, deck: usize, slot: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ClearHotCue { deck, slot });
        }
    }

    // =========================================================================
    // Deck Control - Loop
    // =========================================================================

    /// Toggle loop on/off
    pub fn toggle_loop(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ToggleLoop { deck });
        }
    }

    /// Adjust loop length (direction: positive = longer, negative = shorter)
    pub fn adjust_loop_length(&mut self, deck: usize, direction: i32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AdjustLoopLength { deck, direction });
        }
    }

    /// Toggle slip mode
    pub fn toggle_slip(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ToggleSlip { deck });
        }
    }

    // =========================================================================
    // Deck Control - Beat Jump
    // =========================================================================

    /// Jump forward by beat_jump_size beats
    pub fn beat_jump_forward(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::BeatJumpForward { deck });
        }
    }

    /// Jump backward by beat_jump_size beats
    pub fn beat_jump_backward(&mut self, deck: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::BeatJumpBackward { deck });
        }
    }

    // =========================================================================
    // Deck Control - Stem Control
    // =========================================================================

    /// Toggle mute for a stem
    pub fn toggle_stem_mute(&mut self, deck: usize, stem: Stem) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ToggleStemMute { deck, stem });
        }
    }

    /// Toggle solo for a stem
    pub fn toggle_stem_solo(&mut self, deck: usize, stem: Stem) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ToggleStemSolo { deck, stem });
        }
    }

    // =========================================================================
    // Deck Control - Key Matching
    // =========================================================================

    /// Enable/disable automatic key matching for a deck
    pub fn set_key_match_enabled(&mut self, deck: usize, enabled: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetKeyMatchEnabled { deck, enabled });
        }
    }

    // =========================================================================
    // Deck Control - Slicer
    // =========================================================================

    /// Enable/disable slicer for a stem on a deck
    pub fn set_slicer_enabled(&mut self, deck: usize, stem: Stem, enabled: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetSlicerEnabled { deck, stem, enabled });
        }
    }

    /// Unified slicer button action from UI
    pub fn slicer_button_action(&mut self, deck: usize, stem: Stem, button_idx: usize, shift_held: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SlicerButtonAction {
                deck,
                stem,
                button_idx,
                shift_held,
            });
        }
    }

    /// Reset slicer queue to default order
    pub fn slicer_reset_queue(&mut self, deck: usize, stem: Stem) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SlicerResetQueue { deck, stem });
        }
    }

    // =========================================================================
    // Deck Control - Linked Stems
    // =========================================================================

    /// Toggle linked stem playback (mute/unmute)
    pub fn toggle_linked_stem(&mut self, deck: usize, stem: Stem) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::ToggleLinkedStem { deck, stem });
        }
    }

    // =========================================================================
    // Mixer Controls
    // =========================================================================

    /// Set channel volume (0.0 - 1.0)
    pub fn set_volume(&mut self, deck: usize, volume: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetVolume { deck, volume });
        }
    }

    /// Enable/disable cue listen (headphone monitoring) for a deck
    pub fn set_cue_listen(&mut self, deck: usize, enabled: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetCueListen { deck, enabled });
        }
    }

    /// Set high EQ (-1.0 to 1.0, 0.0 = neutral)
    pub fn set_eq_hi(&mut self, deck: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetEqHi { deck, value });
        }
    }

    /// Set mid EQ (-1.0 to 1.0, 0.0 = neutral)
    pub fn set_eq_mid(&mut self, deck: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetEqMid { deck, value });
        }
    }

    /// Set low EQ (-1.0 to 1.0, 0.0 = neutral)
    pub fn set_eq_lo(&mut self, deck: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetEqLo { deck, value });
        }
    }

    /// Set filter position (-1.0 to 1.0, 0.0 = bypass)
    pub fn set_filter(&mut self, deck: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetFilter { deck, value });
        }
    }

    /// Set master output volume (0.0 - 1.0)
    pub fn set_master_volume(&mut self, volume: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMasterVolume { volume });
        }
    }

    /// Set cue/master mix for headphone output (0.0 = cue only, 1.0 = master only)
    pub fn set_cue_mix(&mut self, mix: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetCueMix { mix });
        }
    }

    /// Set cue/headphone output volume (0.0 - 1.0)
    pub fn set_cue_volume(&mut self, volume: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetCueVolume { volume });
        }
    }

    // =========================================================================
    // Global Controls
    // =========================================================================

    /// Set global BPM and send to audio engine
    pub fn set_global_bpm_with_engine(&mut self, bpm: f64) {
        self.global_bpm = bpm;
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetGlobalBpm(bpm));
        }
    }

    // =========================================================================
    // Track Loading (to Audio Engine)
    // =========================================================================

    /// Apply a loaded track to a deck - handles all domain state updates
    ///
    /// This method encapsulates all domain-side logic when a track is loaded:
    /// 1. Stores stem buffers for waveform recomputation
    /// 2. Caches track LUFS for linked stem gain matching
    /// 3. Clears any linked stems from the previous track
    /// 4. Sends the prepared track to the audio engine
    ///
    /// The UI should call this after receiving TrackLoaded, then update its
    /// own display state (waveforms, deck views, etc.) separately.
    pub fn apply_loaded_track(
        &mut self,
        deck: usize,
        stems: Shared<StemBuffers>,
        track_lufs: Option<f32>,
        prepared: PreparedTrack,
    ) {
        // Store stem buffers for potential waveform recomputation
        self.set_deck_stems(deck, Some(stems));

        // Cache track LUFS for use in LinkedStemLoaded (avoids race conditions)
        self.set_track_lufs(deck, track_lufs);

        // Clear linked stems from previous track
        for stem_idx in 0..4 {
            self.set_deck_linked_stem(deck, stem_idx, None);
        }

        // Send prepared track to audio engine
        self.load_track_to_engine(deck, prepared);
    }

    /// Send a prepared track to the audio engine (low-level)
    ///
    /// Prefer using `apply_loaded_track()` which handles all domain state.
    /// This method is for cases where you need direct engine control.
    fn load_track_to_engine(&mut self, deck: usize, prepared: PreparedTrack) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::LoadTrack {
                deck,
                track: Box::new(prepared),
            });
        }
    }

    // =========================================================================
    // Linked Stem Loading
    // =========================================================================

    /// Send linked stem data to the audio engine
    ///
    /// Called after the engine has loaded and aligned the linked stem.
    /// The LinkedStemData is boxed because it contains audio buffers.
    pub fn link_stem(&mut self, deck: usize, stem: Stem, linked_data: LinkedStemData, host_lufs: Option<f32>) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::LinkStem {
                deck,
                stem,
                linked_stem: Box::new(linked_data),
                host_lufs,
            });
        }
    }

    /// Request loading a linked stem (processed by audio engine)
    ///
    /// The engine handles BPM-matching, alignment, and time-stretching.
    pub fn load_linked_stem(
        &mut self,
        deck: usize,
        stem_idx: usize,
        path: PathBuf,
        host_bpm: f64,
        host_drop_marker: u64,
        host_duration: u64,
    ) -> bool {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::LoadLinkedStem(Box::new(
                mesh_core::engine::LoadLinkedStemRequest {
                    deck,
                    stem_idx,
                    path,
                    host_bpm,
                    host_drop_marker,
                    host_duration,
                },
            )));
            true
        } else {
            false
        }
    }

    // =========================================================================
    // Settings / Configuration Commands
    // =========================================================================

    /// Enable/disable phase sync for beat matching
    pub fn set_phase_sync(&mut self, enabled: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetPhaseSync(enabled));
        }
    }

    /// Set loudness configuration (triggers recalculation for all decks)
    pub fn set_loudness_config(&mut self, config: LoudnessConfig) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetLoudnessConfig(config));
        }
    }

    /// Set slicer buffer bars for a specific deck and stem
    pub fn set_slicer_buffer_bars(&mut self, deck: usize, stem: Stem, bars: u32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetSlicerBufferBars { deck, stem, bars });
        }
    }

    /// Apply slicer buffer bars to all decks and stems
    pub fn apply_slicer_buffer_bars_all(&mut self, bars: u32) {
        let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
        for deck in 0..4 {
            for &stem in &stems {
                self.set_slicer_buffer_bars(deck, stem, bars);
            }
        }
    }

    /// Set slicer presets for all decks
    ///
    /// Presets are loaded from shared config and define per-stem patterns.
    pub fn set_slicer_presets(&mut self, presets: [SlicerPreset; 8]) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetSlicerPresets {
                presets: Box::new(presets),
            });
        }
    }

    // =========================================================================
    // Engine Initialization
    // =========================================================================

    /// Initialize audio engine with configuration
    ///
    /// Called once after the domain is created to send initial settings
    /// to the audio engine. This includes BPM, phase sync, slicer presets,
    /// slicer buffer bars, and loudness config.
    pub fn initialize_engine(
        &mut self,
        global_bpm: f64,
        phase_sync: bool,
        slicer_presets: [SlicerPreset; 8],
        slicer_buffer_bars: u32,
        loudness_config: LoudnessConfig,
    ) {
        // Set initial global BPM (also updates domain state)
        self.set_global_bpm_with_engine(global_bpm);

        // Set initial phase sync
        self.set_phase_sync(phase_sync);

        // Set slicer presets
        self.set_slicer_presets(slicer_presets);

        // Set slicer buffer bars for all decks and stems
        self.apply_slicer_buffer_bars_all(slicer_buffer_bars);

        // Set loudness config
        self.set_loudness_config(loudness_config);
    }

    // =========================================================================
    // Effect Management (PD effects and generic effects)
    // =========================================================================

    /// Get the list of discovered PD effects
    ///
    /// Returns all effects found in the collection's effects/ folder.
    /// Effects with missing dependencies are included but marked unavailable.
    pub fn discovered_effects(&self) -> &[DiscoveredEffect] {
        self.pd_manager.discovered_effects()
    }

    /// Get only available effects (no missing dependencies)
    pub fn available_effects(&self) -> Vec<&DiscoveredEffect> {
        self.pd_manager.available_effects()
    }

    /// Add a PD effect to a stem's effect chain
    ///
    /// Creates the effect via PdManager and sends it to the audio engine.
    /// Returns Ok(()) on success, or an error message on failure.
    ///
    /// # Arguments
    /// * `deck` - Deck index (0-3)
    /// * `stem` - Which stem to add the effect to
    /// * `effect_id` - Effect identifier (folder name in effects/)
    /// * `band_index` - Band index in the multiband container (0-7)
    pub fn add_pd_effect(&mut self, deck: usize, stem: Stem, effect_id: &str, band_index: usize) -> Result<(), String> {
        // Create the effect via PdManager (this does the non-RT-safe work)
        // Note: All effects share the single global PdInstance; deck is only used
        // for routing in the audio engine, not for PD isolation.
        let effect = self
            .pd_manager
            .create_effect(effect_id)
            .map_err(|e| format!("Failed to create PD effect '{}': {}", effect_id, e))?;

        // Send to audio engine via command (add to specified band of multiband container)
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandBandEffect {
                deck,
                stem,
                band_index,
                effect,
            });
            log::info!("Added PD effect '{}' to deck {} stem {:?} band {}", effect_id, deck, stem, band_index);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    // =========================================================================
    // CLAP Plugin Management
    // =========================================================================

    /// Get all discovered CLAP plugins
    ///
    /// Returns all plugins found in standard CLAP directories.
    /// Plugins that failed to load are included but marked unavailable.
    pub fn discovered_clap_plugins(&self) -> &[DiscoveredClapPlugin] {
        self.clap_manager.discovered_plugins()
    }

    /// Get only available CLAP plugins (loaded successfully)
    pub fn available_clap_plugins(&self) -> Vec<&DiscoveredClapPlugin> {
        self.clap_manager.available_plugins()
    }

    /// Get CLAP plugins by category
    pub fn clap_plugins_by_category(&self, category: ClapPluginCategory) -> Vec<&DiscoveredClapPlugin> {
        self.clap_manager.plugins_by_category(category)
    }

    /// Check if any CLAP plugins are available
    pub fn has_clap_plugins(&self) -> bool {
        self.clap_manager.has_plugins()
    }

    /// Add a CLAP effect to a stem's effect chain
    ///
    /// Creates the effect via ClapManager and sends it to the audio engine.
    /// Returns Ok(()) on success, or an error message on failure.
    ///
    /// # Arguments
    /// * `deck` - Deck index (0-3)
    /// * `stem` - Which stem to add the effect to
    /// * `plugin_id` - CLAP plugin identifier (e.g., "org.lsp-plug.compressor-stereo")
    /// * `band_index` - Band index in the multiband container (0-7)
    pub fn add_clap_effect(&mut self, deck: usize, stem: Stem, plugin_id: &str, band_index: usize) -> Result<(), String> {
        // Create the effect via ClapManager (this does the non-RT-safe work)
        let effect = self
            .clap_manager
            .create_effect(plugin_id)
            .map_err(|e| format!("Failed to create CLAP effect '{}': {}", plugin_id, e))?;

        // Send to audio engine via command (add to specified band of multiband container)
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandBandEffect {
                deck,
                stem,
                band_index,
                effect,
            });
            log::info!("Added CLAP effect '{}' to deck {} stem {:?} band {}", plugin_id, deck, stem, band_index);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    /// Rescan for CLAP plugins
    pub fn rescan_clap_plugins(&mut self) {
        self.clap_manager.rescan_plugins();
    }

    /// Set a crossover effect for a stem's multiband container
    ///
    /// The crossover splits audio into frequency bands. If not set, the multiband
    /// container operates in single-band passthrough mode.
    ///
    /// # Arguments
    /// * `deck` - Deck index (0-3)
    /// * `stem` - Which stem to configure
    /// * `crossover_plugin_id` - CLAP crossover plugin ID (e.g., LSP Crossover)
    pub fn set_multiband_crossover(
        &mut self,
        deck: usize,
        stem: Stem,
        crossover_plugin_id: &str,
    ) -> Result<(), String> {
        // Create the crossover effect
        let crossover = self.clap_manager
            .create_effect(crossover_plugin_id)
            .map_err(|e| format!("Failed to create crossover effect: {}", e))?;

        // Set it on the stem's multiband container
        // Note: This would need a new engine command - for now log a warning
        log::warn!(
            "set_multiband_crossover not yet implemented via engine command for deck {} stem {:?}",
            deck, stem
        );
        // TODO: Add EngineCommand::SetMultibandCrossoverEffect
        drop(crossover);
        Ok(())
    }

    /// Add a generic effect to a stem's multiband container (band 0)
    ///
    /// This accepts any Box<dyn Effect>, allowing both PD effects and
    /// native Rust effects to be added through the same interface.
    pub fn add_effect(&mut self, deck: usize, stem: Stem, effect: Box<dyn Effect>) {
        self.add_effect_to_band(deck, stem, 0, effect);
    }

    /// Add an effect to a specific band in a stem's multiband container
    pub fn add_effect_to_band(&mut self, deck: usize, stem: Stem, band_index: usize, effect: Box<dyn Effect>) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandBandEffect {
                deck,
                stem,
                band_index,
                effect,
            });
        }
    }

    /// Remove an effect from band 0 of a stem's multiband container by index
    pub fn remove_effect(&mut self, deck: usize, stem: Stem, effect_index: usize) {
        self.remove_effect_from_band(deck, stem, 0, effect_index);
    }

    /// Remove an effect from a specific band in a stem's multiband container
    pub fn remove_effect_from_band(&mut self, deck: usize, stem: Stem, band_index: usize, effect_index: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::RemoveMultibandBandEffect {
                deck,
                stem,
                band_index,
                effect_index,
            });
        }
    }

    /// Set bypass state for an effect in band 0 of a stem's multiband container
    pub fn set_effect_bypass(&mut self, deck: usize, stem: Stem, effect_index: usize, bypass: bool) {
        self.set_band_effect_bypass(deck, stem, 0, effect_index, bypass);
    }

    /// Set bypass state for an effect in a specific band
    pub fn set_band_effect_bypass(&mut self, deck: usize, stem: Stem, band_index: usize, effect_index: usize, bypass: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandEffectBypass {
                deck,
                stem,
                band_index,
                effect_index,
                bypass,
            });
        }
    }

    /// Set a parameter value on an effect in band 0
    ///
    /// # Arguments
    /// * `deck` - Deck index
    /// * `stem` - Stem the effect is on
    /// * `effect_index` - Index of the effect in the band
    /// * `param_index` - Parameter index (0-7 for PD effects)
    /// * `value` - Normalized value (typically 0.0-1.0)
    pub fn set_effect_param(
        &mut self,
        deck: usize,
        stem: Stem,
        effect_index: usize,
        param_index: usize,
        value: f32,
    ) {
        self.set_band_effect_param(deck, stem, 0, effect_index, param_index, value);
    }

    /// Set a parameter value on an effect in a specific band
    pub fn set_band_effect_param(
        &mut self,
        deck: usize,
        stem: Stem,
        band_index: usize,
        effect_index: usize,
        param_index: usize,
        value: f32,
    ) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandEffectParam {
                deck,
                stem,
                band_index,
                effect_index,
                param_index,
                value,
            });
        }
    }

    // =========================================================================
    // Pre-FX Chain Management (before multiband split)
    // =========================================================================

    /// Add a PD effect to the pre-fx chain
    pub fn add_pd_effect_pre_fx(&mut self, deck: usize, stem: Stem, effect_id: &str) -> Result<(), String> {
        let effect = self
            .pd_manager
            .create_effect(effect_id)
            .map_err(|e| format!("Failed to create PD effect '{}': {}", effect_id, e))?;

        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandPreFx { deck, stem, effect });
            log::info!("Added PD pre-fx '{}' to deck {} stem {:?}", effect_id, deck, stem);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    /// Add a CLAP effect to the pre-fx chain
    pub fn add_clap_effect_pre_fx(&mut self, deck: usize, stem: Stem, plugin_id: &str) -> Result<(), String> {
        let effect = self
            .clap_manager
            .create_effect(plugin_id)
            .map_err(|e| format!("Failed to create CLAP effect '{}': {}", plugin_id, e))?;

        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandPreFx { deck, stem, effect });
            log::info!("Added CLAP pre-fx '{}' to deck {} stem {:?}", plugin_id, deck, stem);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    /// Remove a pre-fx effect by index
    pub fn remove_pre_fx_effect(&mut self, deck: usize, stem: Stem, effect_index: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::RemoveMultibandPreFx { deck, stem, effect_index });
        }
    }

    /// Set bypass state for a pre-fx effect
    pub fn set_pre_fx_bypass(&mut self, deck: usize, stem: Stem, effect_index: usize, bypass: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandPreFxBypass { deck, stem, effect_index, bypass });
        }
    }

    /// Set a parameter value on a pre-fx effect
    pub fn set_pre_fx_param(&mut self, deck: usize, stem: Stem, effect_index: usize, param_index: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandPreFxParam { deck, stem, effect_index, param_index, value });
        }
    }

    // =========================================================================
    // Post-FX Chain Management (after band summation)
    // =========================================================================

    /// Add a PD effect to the post-fx chain
    pub fn add_pd_effect_post_fx(&mut self, deck: usize, stem: Stem, effect_id: &str) -> Result<(), String> {
        let effect = self
            .pd_manager
            .create_effect(effect_id)
            .map_err(|e| format!("Failed to create PD effect '{}': {}", effect_id, e))?;

        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandPostFx { deck, stem, effect });
            log::info!("Added PD post-fx '{}' to deck {} stem {:?}", effect_id, deck, stem);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    /// Add a CLAP effect to the post-fx chain
    pub fn add_clap_effect_post_fx(&mut self, deck: usize, stem: Stem, plugin_id: &str) -> Result<(), String> {
        let effect = self
            .clap_manager
            .create_effect(plugin_id)
            .map_err(|e| format!("Failed to create CLAP effect '{}': {}", plugin_id, e))?;

        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::AddMultibandPostFx { deck, stem, effect });
            log::info!("Added CLAP post-fx '{}' to deck {} stem {:?}", plugin_id, deck, stem);
            Ok(())
        } else {
            Err("Audio engine not connected".to_string())
        }
    }

    /// Remove a post-fx effect by index
    pub fn remove_post_fx_effect(&mut self, deck: usize, stem: Stem, effect_index: usize) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::RemoveMultibandPostFx { deck, stem, effect_index });
        }
    }

    /// Set bypass state for a post-fx effect
    pub fn set_post_fx_bypass(&mut self, deck: usize, stem: Stem, effect_index: usize, bypass: bool) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandPostFxBypass { deck, stem, effect_index, bypass });
        }
    }

    /// Set a parameter value on a post-fx effect
    pub fn set_post_fx_param(&mut self, deck: usize, stem: Stem, effect_index: usize, param_index: usize, value: f32) {
        if let Some(ref mut sender) = self.command_sender {
            let _ = sender.send(EngineCommand::SetMultibandPostFxParam { deck, stem, effect_index, param_index, value });
        }
    }

    /// Rescan for effects (e.g., after user adds new effects to the folder)
    pub fn rescan_effects(&mut self) {
        self.pd_manager.rescan_effects();
    }
}
