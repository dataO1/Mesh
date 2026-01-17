//! Main application state and iced implementation

use crate::audio::{AudioState, JackHandle, start_jack_client};
use crate::batch_import::{self, ImportConfig, ImportProgress};
use crate::config::{self, Config};
use crate::export;
use crate::keybindings::{self, KeybindingsConfig};
use super::waveform::{CombinedWaveformView, WaveformView, ZoomedWaveformView, generate_peaks};
use mesh_widgets::HIGHRES_WIDTH;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, stack, text, Space};
use iced::{Color, Element, Length, Task, Theme};
use basedrop::Shared;
use mesh_core::audio_file::{BeatGrid, CuePoint, LoadedTrack, MetadataField, TrackMetadata, update_metadata_in_file};
use mesh_core::db::{MeshDb, TrackQuery};
use mesh_core::playlist::{DatabaseStorage, NodeId, NodeKind};
use mesh_core::types::{PlayState, Stem};
use mesh_widgets::{
    mpsc_subscription, sort_tracks,
    PlaylistBrowserMessage,
    TrackColumn, TrackTableMessage,
    SliceEditorState, ZoomedViewMode,
};
use std::path::PathBuf;
use std::sync::Arc;

// Re-export extracted modules for use by other UI modules
pub use super::message::Message;
pub use super::state::{
    BrowserSide, CollectionState, DragState, ExportPhase, ExportState,
    ImportPhase, ImportState, LinkedStemLoadedMsg, LoadedTrackState,
    ReanalysisState, SettingsState, StemsLoadResult, View,
};
pub use super::utils::{
    build_tree_nodes, get_tracks_for_folder, nudge_beat_grid, regenerate_beat_grid,
    snap_to_nearest_beat, update_waveform_beat_grid, BEAT_GRID_NUDGE_SAMPLES,
};

// State types imported from super::state

// State types (LoadedTrackState, SettingsState, ImportState, etc.) are now in state/ module
// Message enum is now in message.rs

/// Main application
pub struct MeshCueApp {
    /// Current view
    current_view: View,
    /// Collection state
    collection: CollectionState,
    /// Audio playback state
    audio: AudioState,
    /// JACK client handle (keeps audio running)
    #[allow(dead_code)]
    jack_client: Option<JackHandle>,
    /// Global configuration
    config: Arc<Config>,
    /// Path to config file
    config_path: PathBuf,
    /// Settings modal state
    settings: SettingsState,
    /// Whether shift key is currently held (for shift+click actions)
    shift_held: bool,
    /// Whether ctrl key is currently held (for ctrl+click toggle selection)
    ctrl_held: bool,
    /// Keybindings configuration
    keybindings: KeybindingsConfig,
    /// Hot cue keys currently pressed (for filtering key repeat)
    pressed_hot_cue_keys: std::collections::HashSet<usize>,
    /// Main cue key currently pressed
    pressed_cue_key: bool,
    /// Batch import state
    import_state: ImportState,
    /// Delete confirmation state
    delete_state: super::delete_modal::DeleteState,
    /// Context menu state
    context_menu_state: super::context_menu::ContextMenuState,
    /// Global mouse position (window coordinates) for context menu placement
    global_mouse_position: iced::Point,
    /// Stem link selection mode - Some(stem_index) when selecting a source track for linking
    stem_link_selection: Option<usize>,
    /// Re-analysis state
    reanalysis_state: ReanalysisState,
    /// USB export state
    export_state: ExportState,
    /// USB manager for device detection and export operations
    usb_manager: mesh_core::usb::UsbManager,
}

impl MeshCueApp {
    /// Create a new application instance
    pub fn new() -> (Self, Task<Message>) {
        // Load configuration
        let config_path = config::default_config_path();
        let config = config::load_config(&config_path);
        log::info!(
            "Loaded config: BPM range {}-{}",
            config.analysis.bpm.min_tempo,
            config.analysis.bpm.max_tempo
        );

        // Load keybindings
        let keybindings_path = keybindings::default_keybindings_path();
        let keybindings = keybindings::load_keybindings(&keybindings_path);
        log::info!("Loaded keybindings from {:?}", keybindings_path);

        let settings = SettingsState::from_config(&config);

        // Start JACK client for audio preview (lock-free architecture)
        let (audio, jack_client) = match start_jack_client() {
            Ok((audio_state, handle)) => {
                log::info!("JACK audio preview enabled (lock-free)");
                (audio_state, Some(handle))
            }
            Err(e) => {
                log::warn!("JACK not available: {} - audio preview disabled", e);
                (AudioState::disconnected(), None)
            }
        };

        // Initialize collection state with playlist storage
        let mut collection_state = CollectionState::default();

        // Initialize playlist storage at collection root using DatabaseStorage
        // Database is created if it doesn't exist (no filesystem fallback)
        let collection_root = collection_state.collection.path().to_path_buf();
        let db_path = collection_root.join("mesh.db");

        // Ensure collection directory exists
        if let Err(e) = std::fs::create_dir_all(&collection_root) {
            log::warn!("Failed to create collection directory: {}", e);
        }

        let storage_result: Result<(Arc<MeshDb>, Box<dyn mesh_core::playlist::PlaylistStorage>), String> = {
            log::info!("Initializing database storage at {:?}", db_path);
            MeshDb::open(&db_path)
                .map_err(|e| format!("DB error: {}", e))
                .and_then(|db| {
                    let db_arc = Arc::new(db);
                    DatabaseStorage::new(db_arc.clone(), collection_root.clone())
                        .map(|s| (db_arc, Box::new(s) as Box<dyn mesh_core::playlist::PlaylistStorage>))
                        .map_err(|e| format!("Storage error: {}", e))
                })
        };

        match storage_result {
            Ok((db_arc, storage)) => {
                log::info!("Playlist storage initialized at {:?}", collection_root);
                collection_state.db = Some(db_arc);
                collection_state.playlist_storage = Some(storage);
                // Build initial tree
                if let Some(ref storage) = collection_state.playlist_storage {
                    collection_state.tree_nodes = build_tree_nodes(storage.as_ref());
                    // Expand root nodes by default
                    collection_state.browser_left.tree_state.expand(NodeId::tracks());
                    collection_state.browser_left.tree_state.expand(NodeId::playlists());
                    collection_state.browser_right.tree_state.expand(NodeId::tracks());
                    collection_state.browser_right.tree_state.expand(NodeId::playlists());

                    // Set left browser to show tracks (collection) by default
                    collection_state.browser_left.set_current_folder(NodeId::tracks());
                    collection_state.left_tracks = get_tracks_for_folder(storage.as_ref(), &NodeId::tracks());
                }
            }
            Err(e) => {
                log::warn!("Failed to initialize playlist storage: {:?}", e);
            }
        }

        let app = Self {
            current_view: View::Collection,
            collection: collection_state,
            audio,
            jack_client,
            config: Arc::new(config),
            config_path,
            settings,
            shift_held: false,
            ctrl_held: false,
            keybindings,
            pressed_hot_cue_keys: std::collections::HashSet::new(),
            pressed_cue_key: false,
            import_state: ImportState::default(),
            delete_state: Default::default(),
            context_menu_state: Default::default(),
            global_mouse_position: iced::Point::ORIGIN,
            stem_link_selection: None,
            reanalysis_state: ReanalysisState::default(),
            export_state: ExportState::default(),
            usb_manager: mesh_core::usb::UsbManager::spawn(),
        };

        // Initial collection scan and playlist refresh
        let cmd = Task::batch([
            Task::perform(async {}, |_| Message::RefreshCollection),
            Task::perform(async {}, |_| Message::RefreshPlaylists),
        ]);

        (app, cmd)
    }

    /// Application title
    pub fn title(&self) -> String {
        String::from("mesh-cue - Track Preparation")
    }

    /// Save current track if it has been modified
    ///
    /// Returns a Task to perform the save asynchronously, or None if no save is needed.
    fn save_current_track_if_modified(&mut self) -> Option<Task<Message>> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if state.modified {
                if let Some(ref stems) = state.stems {
                    let path = state.path.clone();
                    let stems = stems.clone();
                    let cue_points = state.cue_points.clone();
                    let saved_loops = state.saved_loops.clone();
                    let metadata = TrackMetadata {
                        artist: None, // TODO: Support artist editing
                        bpm: Some(state.bpm),
                        original_bpm: Some(state.bpm),
                        key: Some(state.key.clone()),
                        duration_seconds: None,
                        beat_grid: BeatGrid {
                            beats: state.beat_grid.clone(),
                            first_beat_sample: state.beat_grid.first().copied(),
                        },
                        cue_points: cue_points.clone(),
                        saved_loops: saved_loops.clone(),
                        waveform_preview: None,
                        drop_marker: state.drop_marker,
                        stem_links: state.stem_links.clone(),
                        lufs: None, // Preserve existing LUFS (not re-measured on save)
                    };

                    // Sync metadata to database before async file save
                    if let Some(ref db) = self.collection.db {
                        let path_str = path.to_string_lossy();
                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, "bpm", &state.bpm.to_string()) {
                            log::warn!("Auto-save: Failed to sync BPM to database: {:?}", e);
                        }
                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, "key", &state.key) {
                            log::warn!("Auto-save: Failed to sync key to database: {:?}", e);
                        }
                    }

                    // Mark as saved to prevent re-saving
                    state.modified = false;

                    log::info!("Auto-saving track: {:?}", path);
                    return Some(Task::perform(
                        async move {
                            export::save_track_metadata(&path, &stems, &metadata, &cue_points, &saved_loops)
                                .map_err(|e| e.to_string())
                        },
                        Message::SaveComplete,
                    ));
                }
            }
        }
        None
    }

    /// Update state based on message
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Navigation
            Message::SwitchView(view) => {
                // Auto-save when switching away from Collection view
                let save_task = if self.current_view == View::Collection && view != View::Collection {
                    self.save_current_track_if_modified()
                } else {
                    None
                };

                self.current_view = view;

                let refresh_task = if view == View::Collection {
                    Some(Task::perform(async {}, |_| Message::RefreshCollection))
                } else {
                    None
                };

                // Return combined tasks if any
                match (save_task, refresh_task) {
                    (Some(save), Some(refresh)) => return Task::batch([save, refresh]),
                    (Some(save), None) => return save,
                    (None, Some(refresh)) => return refresh,
                    (None, None) => {}
                }
            }

            // Collection: Browser
            Message::RefreshCollection => {
                // Scan is fast (just reads directory), so do it synchronously
                if let Err(e) = self.collection.collection.scan() {
                    eprintln!("Failed to refresh collection: {}", e);
                }
            }
            Message::SelectTrack(index) => {
                self.collection.selected_track = Some(index);
            }
            Message::LoadTrack(index) => {
                // Auto-save current track if modified before loading new one
                let save_task = self.save_current_track_if_modified();

                // Phase 1: Load metadata first (fast, ~50ms)
                if let Some(track) = self.collection.collection.tracks().get(index) {
                    let path = track.path.clone();
                    log::info!("LoadTrack: Starting two-phase load for {:?}", path);
                    let load_task = Task::perform(
                        async move {
                            LoadedTrack::load_metadata_only(&path)
                                .map(|metadata| (path, metadata))
                                .map_err(|e| e.to_string())
                        },
                        Message::TrackMetadataLoaded,
                    );

                    // Chain save and load tasks
                    if let Some(save) = save_task {
                        return Task::batch([save, load_task]);
                    }
                    return load_task;
                }
            }
            Message::TrackMetadataLoaded(result) => {
                match result {
                    Ok((path, metadata)) => {
                        log::info!("TrackMetadataLoaded: Showing UI, starting audio load");
                        let bpm = metadata.bpm.unwrap_or(120.0);
                        let key = metadata.key.clone().unwrap_or_else(|| "?".to_string());
                        let cue_points = metadata.cue_points.clone();
                        let beat_grid = metadata.beat_grid.beats.clone();

                        // Create combined waveform view (both zoomed + overview in single canvas)
                        let mut combined_waveform = CombinedWaveformView::new();
                        // Initialize overview with beat markers from metadata
                        combined_waveform.overview = WaveformView::from_metadata(&metadata);
                        // Apply grid density from config
                        combined_waveform.overview.set_grid_bars(self.config.display.grid_bars);
                        // Initialize zoomed view (peaks will be computed when stems load)
                        combined_waveform.zoomed = ZoomedWaveformView::from_metadata(
                            bpm,
                            beat_grid.clone(),
                            Vec::new(), // Cue markers will be added after duration is known
                        );
                        // Set drop marker on zoomed view (overview gets it from from_metadata)
                        combined_waveform.zoomed.set_drop_marker(metadata.drop_marker);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path: path.clone(),
                            track: None,
                            stems: None,
                            cue_points,
                            saved_loops: metadata.saved_loops.clone(),
                            bpm,
                            key,
                            beat_grid,
                            drop_marker: metadata.drop_marker,
                            lufs: metadata.lufs,
                            stem_links: metadata.stem_links.clone(),
                            duration_samples: 0, // Will be set when audio loads
                            modified: false,
                            combined_waveform,
                            loading_audio: true,
                            deck_atomics: self.audio.deck_atomics().clone(),
                            last_playhead_update: std::time::Instant::now(),
                            slice_editor: {
                                // Load presets from dedicated file (shared with mesh-player)
                                let collection_path = self.collection.collection.path();
                                let slicer_config = mesh_widgets::load_slicer_presets(collection_path);
                                let mut editor = SliceEditorState::new();
                                slicer_config.apply_to_editor_state(&mut editor);
                                editor
                            },
                        });

                        // Phase 2: Load audio stems in background (slow, ~3s)
                        return Task::perform(
                            async move {
                                LoadedTrack::load_stems(&path)
                                    .map(|stems| Shared::new(&mesh_core::engine::gc::gc_handle(), stems))
                                    .map_err(|e| e.to_string())
                            },
                            |result| Message::TrackStemsLoaded(StemsLoadResult(result)),
                        );
                    }
                    Err(e) => {
                        log::error!("Failed to load track metadata: {}", e);
                    }
                }
            }
            Message::TrackStemsLoaded(StemsLoadResult(result)) => {
                match result {
                    Ok(stems) => {
                        log::info!("TrackStemsLoaded: Audio ready, generating waveform");
                        if let Some(ref mut state) = self.collection.loaded_track {
                            let duration_samples = stems.len() as u64;
                            state.duration_samples = duration_samples;
                            state.loading_audio = false;
                            // Generate waveform from loaded stems (overview)
                            state.combined_waveform.overview.set_stems(&stems, &state.cue_points, &state.beat_grid);

                            // Compute high-resolution peaks for stable zoomed waveform rendering
                            let highres_start = std::time::Instant::now();
                            let highres_peaks = generate_peaks(&stems, HIGHRES_WIDTH);
                            log::info!(
                                "[PERF] mesh-cue highres peaks: {} samples → {} peaks in {:?}",
                                duration_samples,
                                HIGHRES_WIDTH,
                                highres_start.elapsed()
                            );
                            state.combined_waveform.overview.set_highres_peaks(highres_peaks);

                            // Initialize zoomed waveform with stem data
                            state.combined_waveform.zoomed.set_duration(duration_samples);
                            state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                            // Apply zoom level from config
                            state.combined_waveform.zoomed.set_zoom(self.config.display.zoom_bars);
                            state.combined_waveform.zoomed.compute_peaks(&stems, 0, 1600);

                            state.stems = Some(stems.clone());

                            // Create LoadedTrack from metadata + stems for audio engine
                            let duration_seconds = duration_samples as f64 / mesh_core::types::SAMPLE_RATE as f64;
                            let loaded_track = LoadedTrack {
                                path: state.path.clone(),
                                stems: stems.clone(),
                                metadata: TrackMetadata {
                                    artist: None,
                                    bpm: Some(state.bpm),
                                    original_bpm: Some(state.bpm),
                                    key: Some(state.key.clone()),
                                    duration_seconds: Some(duration_seconds),
                                    beat_grid: BeatGrid {
                                        beats: state.beat_grid.clone(),
                                        first_beat_sample: state.beat_grid.first().copied(),
                                    },
                                    cue_points: state.cue_points.clone(),
                                    saved_loops: state.saved_loops.clone(),
                                    waveform_preview: None, // Using live-generated waveform
                                    drop_marker: state.drop_marker,
                                    stem_links: state.stem_links.clone(),
                                    lufs: None, // LUFS read from track, passed to Deck separately
                                },
                                duration_samples: duration_samples as usize,
                                duration_seconds,
                            };

                            // Load track into audio engine (creates PreparedTrack internally)
                            self.audio.load_track(loaded_track);
                            // Set global BPM to track's BPM for original-speed playback (no time-stretching)
                            self.audio.set_global_bpm(state.bpm);
                            // Set default loop length from config
                            self.audio.set_loop_length_index(self.config.display.default_loop_length_index);
                            // Linked stems are auto-loaded by engine from track metadata
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to load track audio: {}", e);
                        if let Some(ref mut state) = self.collection.loaded_track {
                            state.loading_audio = false;
                        }
                    }
                }
            }
            Message::LinkedStemLoaded(msg) => {
                // Extract the result from Arc wrapper
                let result = match Arc::try_unwrap(msg.0) {
                    Ok(r) => r,
                    Err(_) => {
                        log::warn!("LinkedStemLoadResult Arc still shared, skipping");
                        return Task::none();
                    }
                };

                match result.result {
                    Ok(linked_data) => {
                        log::info!(
                            "Linked stem {} loaded: {}",
                            result.stem_idx,
                            linked_data.track_name
                        );

                        // Store peaks for waveform display
                        if let Some(ref mut state) = self.collection.loaded_track {
                            if let Some(peaks) = result.overview_peaks {
                                state.combined_waveform.overview.set_linked_stem_peaks(
                                    result.stem_idx,
                                    peaks,
                                );
                            }
                            if let Some(peaks) = result.highres_peaks {
                                state.combined_waveform.overview.set_linked_highres_peaks(
                                    result.stem_idx,
                                    peaks,
                                );
                            }

                            // Calculate and set LUFS gain for linked stem waveform
                            // This matches what mesh-player does to ensure visual consistency
                            let linked_gain = self.config.analysis.loudness.calculate_gain_linear(linked_data.lufs);
                            state.combined_waveform.overview.set_linked_lufs_gain(result.stem_idx, linked_gain);
                            log::info!(
                                "[LINKED] Set LUFS gain for stem {}: linked_lufs={:?}, gain={:.3} ({:+.1}dB)",
                                result.stem_idx,
                                linked_data.lufs,
                                linked_gain,
                                20.0 * linked_gain.log10()
                            );
                        }

                        // Send LinkStem command to engine with host LUFS to avoid race conditions
                        if let Some(stem) = Stem::from_index(result.stem_idx) {
                            let host_lufs = self.collection.loaded_track.as_ref().and_then(|t| t.lufs);
                            self.audio.link_stem(stem, linked_data, host_lufs);
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "Failed to load linked stem {}: {}",
                            result.stem_idx,
                            e
                        );
                    }
                }
            }
            // Legacy handler (kept for compatibility)
            Message::TrackLoaded(result) => {
                match result {
                    Ok(track) => {
                        let path = track.path.clone();
                        let bpm = track.bpm();
                        let key = track.key().to_string();
                        let cue_points = track.metadata.cue_points.clone();
                        let beat_grid = track.metadata.beat_grid.beats.clone();
                        let duration_samples = track.duration_samples as u64;
                        let stems = track.stems.clone();

                        // Create combined waveform with full track data
                        let mut combined_waveform = CombinedWaveformView::new();
                        combined_waveform.overview = WaveformView::from_track(&track, &cue_points);
                        // Apply grid density from config
                        combined_waveform.overview.set_grid_bars(self.config.display.grid_bars);

                        // Compute high-resolution peaks for stable zoomed waveform rendering
                        let highres_start = std::time::Instant::now();
                        let highres_peaks = generate_peaks(&stems, HIGHRES_WIDTH);
                        log::info!(
                            "[PERF] mesh-cue highres peaks: {} samples → {} peaks in {:?}",
                            duration_samples,
                            HIGHRES_WIDTH,
                            highres_start.elapsed()
                        );
                        combined_waveform.overview.set_highres_peaks(highres_peaks);

                        combined_waveform.zoomed = ZoomedWaveformView::from_metadata(
                            bpm,
                            beat_grid.clone(),
                            Vec::new(),
                        );
                        combined_waveform.zoomed.set_duration(duration_samples);
                        combined_waveform.zoomed.set_drop_marker(track.metadata.drop_marker);
                        combined_waveform.zoomed.compute_peaks(&stems, 0, 1600);

                        // Load track into audio engine (creates PreparedTrack internally)
                        let track_for_audio = LoadedTrack {
                            path: track.path.clone(),
                            stems: track.stems.clone(),
                            metadata: track.metadata.clone(),
                            duration_samples: track.duration_samples,
                            duration_seconds: track.duration_seconds,
                        };
                        self.audio.load_track(track_for_audio);
                        // Set global BPM to track's BPM for original-speed playback (no time-stretching)
                        self.audio.set_global_bpm(bpm);
                        self.audio.set_loop_length_index(self.config.display.default_loop_length_index);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path,
                            track: Some(track.clone()),
                            stems: Some(stems),
                            cue_points,
                            saved_loops: track.metadata.saved_loops.clone(),
                            bpm,
                            key,
                            beat_grid,
                            drop_marker: track.metadata.drop_marker,
                            lufs: track.metadata.lufs,
                            stem_links: track.metadata.stem_links.clone(),
                            duration_samples,
                            modified: false,
                            combined_waveform,
                            loading_audio: false,
                            deck_atomics: self.audio.deck_atomics().clone(),
                            last_playhead_update: std::time::Instant::now(),
                            slice_editor: {
                                // Load presets from dedicated file (shared with mesh-player)
                                let collection_path = self.collection.collection.path();
                                let slicer_config = mesh_widgets::load_slicer_presets(collection_path);
                                let mut editor = SliceEditorState::new();
                                slicer_config.apply_to_editor_state(&mut editor);
                                editor
                            },
                        });
                    }
                    Err(e) => {
                        log::error!("Failed to load track: {}", e);
                    }
                }
            }

            // Collection: Editor
            Message::SetBpm(bpm) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.bpm = bpm;

                    // Regenerate beat grid keeping current first beat position
                    // This allows: nudge grid to align → change BPM → grid recalculates
                    if !state.beat_grid.is_empty() && state.duration_samples > 0 {
                        let first_beat = state.beat_grid[0];
                        state.beat_grid = regenerate_beat_grid(first_beat, bpm, state.duration_samples);
                        update_waveform_beat_grid(state);

                        // Propagate to deck so snapping uses updated grid
                        self.audio.set_beat_grid(state.beat_grid.clone());
                    }

                    state.modified = true;
                }
            }
            Message::SetKey(key) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.key = key;
                    state.modified = true;
                }
            }
            Message::AddCuePoint(position) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    let index = state.cue_points.len() as u8;
                    state.cue_points.push(CuePoint {
                        index,
                        sample_position: position,
                        label: format!("Cue {}", index + 1),
                        color: None,
                    });
                    state.modified = true;
                }
            }
            Message::DeleteCuePoint(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if index < state.cue_points.len() {
                        state.cue_points.remove(index);
                        state.modified = true;
                    }
                }
            }
            Message::SetCueLabel(index, label) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(cue) = state.cue_points.get_mut(index) {
                        cue.label = label;
                        state.modified = true;
                    }
                }
            }
            Message::SaveTrack => {
                if let Some(ref state) = self.collection.loaded_track {
                    // Can't save if stems aren't loaded yet
                    let stems = match &state.stems {
                        Some(s) => s.clone(),
                        None => {
                            log::warn!("Cannot save: audio not loaded yet");
                            return Task::none();
                        }
                    };

                    let path = state.path.clone();
                    let cue_points = state.cue_points.clone();
                    let saved_loops = state.saved_loops.clone();

                    // Build updated metadata from edited fields
                    let metadata = TrackMetadata {
                        artist: None, // TODO: Support artist editing
                        bpm: Some(state.bpm),
                        original_bpm: Some(state.bpm), // Use current BPM if no original
                        key: Some(state.key.clone()),
                        duration_seconds: None,
                        beat_grid: BeatGrid {
                            beats: state.beat_grid.clone(),
                            first_beat_sample: state.beat_grid.first().copied(),
                        },
                        cue_points: cue_points.clone(),
                        saved_loops: saved_loops.clone(),
                        waveform_preview: None, // Will be regenerated during save
                        drop_marker: state.drop_marker,
                        stem_links: state.stem_links.clone(),
                        lufs: None, // Preserve existing LUFS (not re-measured on save)
                    };

                    // Sync metadata to database before async file save
                    if let Some(ref db) = self.collection.db {
                        let path_str = path.to_string_lossy();
                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, "bpm", &state.bpm.to_string()) {
                            log::warn!("Failed to sync BPM to database: {:?}", e);
                        }
                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, "key", &state.key) {
                            log::warn!("Failed to sync key to database: {:?}", e);
                        }
                        log::info!("Synced track metadata to database");
                    }

                    return Task::perform(
                        async move {
                            export::save_track_metadata(&path, &stems, &metadata, &cue_points, &saved_loops)
                                .map_err(|e| e.to_string())
                        },
                        Message::SaveComplete,
                    );
                }
            }
            Message::SaveComplete(result) => {
                match result {
                    Ok(()) => {
                        if let Some(ref mut state) = self.collection.loaded_track {
                            state.modified = false;
                        }
                        log::info!("Track saved successfully");
                    }
                    Err(e) => {
                        log::error!("Failed to save track: {}", e);
                    }
                }
            }

            // Transport
            Message::Play => {
                if self.collection.loaded_track.is_some() {
                    self.audio.play();
                }
                // Clear pressed hot cue keys to prevent spurious release events
                self.pressed_hot_cue_keys.clear();
            }
            Message::Pause => {
                if self.collection.loaded_track.is_some() {
                    self.audio.pause();
                }
            }
            Message::Stop => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    self.audio.pause();
                    self.audio.seek(0);
                    state.update_zoomed_waveform_cache(0);
                }
            }
            Message::Seek(position) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    let seek_pos = (position * state.duration_samples as f64) as u64;
                    self.audio.seek(seek_pos);
                    state.combined_waveform.overview.set_position(position);
                    state.update_zoomed_waveform_cache(seek_pos);
                }
            }
            Message::ToggleLoop => {
                if self.collection.loaded_track.is_some() {
                    self.audio.toggle_loop();
                }
            }
            Message::AdjustLoopLength(delta) => {
                if self.collection.loaded_track.is_some() {
                    self.audio.adjust_loop_length(delta);
                }
            }
            Message::Cue => {
                // CDJ-style cue (only works when stopped):
                // - Set cue point at current position (snapped to beat using UI's grid)
                // - Start preview playback
                if let Some(ref mut state) = self.collection.loaded_track {
                    // Only act when stopped (not playing)
                    if state.is_playing() {
                        return Task::none();
                    }

                    // Snap to nearest beat using UI's current beat grid
                    let current_pos = state.playhead_position();
                    let snapped_pos = snap_to_nearest_beat(current_pos, &state.beat_grid);

                    // Seek to snapped position, set cue point, and start preview
                    self.audio.seek(snapped_pos);
                    self.audio.set_cue_point();
                    self.audio.play();

                    // Update waveform and cue marker
                    if state.duration_samples > 0 {
                        let normalized = snapped_pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);
                        state.combined_waveform.overview.set_cue_position(Some(normalized));
                    }
                    state.update_zoomed_waveform_cache(snapped_pos);
                }
            }
            Message::CueReleased => {
                // CDJ-style cue release: stop preview, return to cue point
                if let Some(ref mut state) = self.collection.loaded_track {
                    let cue_pos = state.cue_point();
                    self.audio.pause();
                    self.audio.seek(cue_pos);

                    // Update waveform
                    if state.duration_samples > 0 {
                        let normalized = cue_pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);
                    }
                    state.update_zoomed_waveform_cache(cue_pos);
                }
            }
            Message::BeatJump(beats) => {
                // Use audio engine's beat jump
                if let Some(ref mut state) = self.collection.loaded_track {
                    if beats > 0 {
                        self.audio.beat_jump_forward();
                    } else {
                        self.audio.beat_jump_backward();
                    }
                    // Position will be updated on next tick via atomics
                    // Trigger waveform update
                    let pos = state.playhead_position();
                    if state.duration_samples > 0 {
                        let normalized = pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);
                    }
                    state.update_zoomed_waveform_cache(pos);
                }
            }
            Message::SetOverviewGridBars(bars) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.combined_waveform.overview.set_grid_bars(bars);
                }
            }
            Message::JumpToCue(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(cue) = state.cue_points.iter().find(|c| c.index == index as u8) {
                        let pos = cue.sample_position;
                        self.audio.seek(pos);

                        // Update waveform
                        if state.duration_samples > 0 {
                            let normalized = pos as f64 / state.duration_samples as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::SetCuePoint(index) => {
                // Snap to beat using UI's current beat grid
                if let Some(ref mut state) = self.collection.loaded_track {
                    let current_pos = state.playhead_position();
                    let snapped_pos = snap_to_nearest_beat(current_pos, &state.beat_grid);

                    // Check if a cue already exists near this position (within ~100ms tolerance)
                    // This prevents duplicate cues at the same beat position
                    const DUPLICATE_TOLERANCE: u64 = 4410; // ~100ms at 44.1kHz
                    let duplicate_exists = state.cue_points.iter().any(|c| {
                        // Skip checking the slot we're about to overwrite
                        if c.index == index as u8 {
                            return false;
                        }
                        (c.sample_position as i64 - snapped_pos as i64).unsigned_abs()
                            < DUPLICATE_TOLERANCE
                    });

                    if duplicate_exists {
                        log::debug!(
                            "Skipping hot cue {} at position {}: duplicate exists nearby",
                            index + 1,
                            snapped_pos
                        );
                        return Task::none();
                    }

                    // Store in cue_points (metadata)
                    state.cue_points.retain(|c| c.index != index as u8);
                    state.cue_points.push(CuePoint {
                        index: index as u8,
                        sample_position: snapped_pos,
                        label: format!("Cue {}", index + 1),
                        color: None,
                    });
                    state.cue_points.sort_by_key(|c| c.index);

                    // Sync to deck so hot cue playback uses updated position immediately
                    self.audio.set_hot_cue(index, snapped_pos as usize);

                    // Update waveform markers (both overview and zoomed)
                    state.combined_waveform.overview.update_cue_markers(&state.cue_points);
                    state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                    state.modified = true;
                }
            }
            Message::ClearCuePoint(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    self.audio.clear_hot_cue(index);
                    state.cue_points.retain(|c| c.index != index as u8);
                    state.combined_waveform.overview.update_cue_markers(&state.cue_points);
                    state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                    state.modified = true;
                }
            }

            // Saved Loops
            Message::SaveLoop(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    // Only save if loop is active (read from atomics)
                    if state.is_loop_active() {
                        let (start, end) = state.loop_bounds();
                        let saved_loop = mesh_core::audio_file::SavedLoop {
                            index: index as u8,
                            start_sample: start,
                            end_sample: end,
                            label: String::new(),
                            color: None,
                        };
                        // Remove any existing loop at this index
                        state.saved_loops.retain(|l| l.index != index as u8);
                        state.saved_loops.push(saved_loop);
                        state.modified = true;
                        log::info!("Saved loop {} at {} - {} samples", index, start, end);
                    }
                }
            }
            Message::JumpToSavedLoop(index) => {
                // Shift+click = clear loop
                if self.shift_held {
                    return self.update(Message::ClearSavedLoop(index));
                }

                if let Some(ref mut state) = self.collection.loaded_track {
                    let loop_data = state.saved_loops.iter().find(|l| l.index == index as u8).cloned();

                    if let Some(saved_loop) = loop_data {
                        // Seek to loop start and activate loop via toggle
                        self.audio.seek(saved_loop.start_sample);
                        self.audio.toggle_loop();

                        // Update waveform positions
                        if state.duration_samples > 0 {
                            let normalized = saved_loop.start_sample as f64 / state.duration_samples as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                        state.update_zoomed_waveform_cache(saved_loop.start_sample);

                        log::info!("Jumped to saved loop {} at {} - {}", index, saved_loop.start_sample, saved_loop.end_sample);
                    }
                }
            }
            Message::ClearSavedLoop(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.saved_loops.retain(|l| l.index != index as u8);
                    state.modified = true;
                    log::info!("Cleared saved loop {}", index);
                }
            }

            // Drop Marker handling
            Message::SetDropMarker => {
                // Shift+click = clear drop marker
                if self.shift_held {
                    return self.update(Message::ClearDropMarker);
                }

                if let Some(ref mut state) = self.collection.loaded_track {
                    let position = state.playhead_position();
                    state.drop_marker = Some(position);
                    state.modified = true;
                    log::info!("Set drop marker at sample {}", position);

                    // Update waveform with new drop marker
                    state.combined_waveform.overview.set_drop_marker(Some(position));
                    state.combined_waveform.zoomed.set_drop_marker(Some(position));
                }
            }

            Message::ClearDropMarker => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.drop_marker = None;
                    state.modified = true;
                    log::info!("Cleared drop marker");

                    // Update waveform
                    state.combined_waveform.overview.set_drop_marker(None);
                    state.combined_waveform.zoomed.set_drop_marker(None);
                }
            }

            // Stem Link handling (prepared mode)
            Message::StartStemLinkSelection(stem_idx) => {
                // Shift+click = clear stem link
                if self.shift_held {
                    return self.update(Message::ClearStemLink(stem_idx));
                }

                // Enter stem link selection mode
                self.stem_link_selection = Some(stem_idx);
                log::info!("Started stem link selection for stem {}", stem_idx);
                // Focus the browser for track selection
                // (browser will highlight when stem_link_selection is Some)
            }

            Message::ConfirmStemLink(stem_idx) => {
                // Called when user confirms track selection in stem link mode
                // Get the source track path from browser selection
                let mut source_path: Option<std::path::PathBuf> = None;

                // Check right browser for selection
                if let Some(ref track_id) = self.collection.browser_right.table_state.last_selected {
                    if let Some(ref storage) = self.collection.playlist_storage {
                        if let Some(node) = storage.get_node(track_id) {
                            source_path = node.track_path.clone();
                        }
                    }
                }

                // If no selection in right browser, check left browser
                if source_path.is_none() {
                    if let Some(ref track_id) = self.collection.browser_left.table_state.last_selected {
                        if let Some(ref storage) = self.collection.playlist_storage {
                            if let Some(node) = storage.get_node(track_id) {
                                source_path = node.track_path.clone();
                            }
                        }
                    }
                }

                // Create the stem link if we have a valid source path
                if let Some(path) = source_path {
                    if let Some(ref mut state) = self.collection.loaded_track {
                        use mesh_core::audio_file::StemLinkReference;
                        let link = StemLinkReference {
                            stem_index: stem_idx as u8,
                            source_path: path.clone(),
                            source_stem: stem_idx as u8, // Same stem from source
                            source_drop_marker: 0, // Will be filled when source is analyzed
                        };

                        // Remove any existing link for this stem
                        state.stem_links.retain(|l| l.stem_index != stem_idx as u8);
                        // Add new link
                        state.stem_links.push(link);
                        state.modified = true;

                        log::info!("Linked stem {} to track {:?}", stem_idx, path);
                    }
                } else {
                    log::warn!("ConfirmStemLink: No track selected in browser");
                }

                // Exit selection mode
                self.stem_link_selection = None;
            }

            Message::ClearStemLink(stem_idx) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.stem_links.retain(|l| l.stem_index != stem_idx as u8);
                    state.modified = true;
                    log::info!("Cleared stem link for stem {}", stem_idx);
                }
                // Also exit selection mode if we were in it
                self.stem_link_selection = None;
            }

            Message::ToggleStemLinkActive(stem_idx) => {
                // Toggle between original and linked stem for playback
                // The audio engine handles the actual toggling
                if let Some(stem) = Stem::from_index(stem_idx) {
                    self.audio.toggle_linked_stem(stem);
                    log::info!("Toggled linked stem active for stem {}", stem_idx);
                }
            }

            // Slice Editor
            Message::SliceEditorCellToggle { step, slice } => {
                // Toggle cell and get sync data if changed
                let sync_data = self.collection.loaded_track.as_mut().and_then(|state| {
                    if state.slice_editor.toggle_cell(step, slice) {
                        // Extract data for audio sync
                        state.slice_editor.selected_stem.and_then(|stem_idx| {
                            state.slice_editor.current_sequence().map(|seq| {
                                (stem_idx, seq.to_engine_sequence())
                            })
                        })
                    } else {
                        None
                    }
                });
                // Sync to audio engine (after releasing borrow)
                if let Some((stem_idx, engine_sequence)) = sync_data {
                    if let Some(stem) = Stem::from_index(stem_idx) {
                        self.audio.slicer_load_sequence(stem, engine_sequence);
                    }
                }
            }
            Message::SliceEditorMuteToggle(step) => {
                // Toggle mute and get sync data if changed
                let sync_data = self.collection.loaded_track.as_mut().and_then(|state| {
                    if state.slice_editor.toggle_mute(step) {
                        // Extract data for audio sync
                        state.slice_editor.selected_stem.and_then(|stem_idx| {
                            state.slice_editor.current_sequence().map(|seq| {
                                (stem_idx, seq.to_engine_sequence())
                            })
                        })
                    } else {
                        None
                    }
                });
                // Sync to audio engine (after releasing borrow)
                if let Some((stem_idx, engine_sequence)) = sync_data {
                    if let Some(stem) = Stem::from_index(stem_idx) {
                        self.audio.slicer_load_sequence(stem, engine_sequence);
                    }
                }
            }
            Message::SliceEditorStemClick(stem_idx) => {
                // Toggle stem and get the new enabled state
                let enabled_change = self.collection.loaded_track.as_mut().map(|state| {
                    let was_enabled = state.slice_editor.stem_enabled[stem_idx];
                    state.slice_editor.click_stem(stem_idx);
                    let now_enabled = state.slice_editor.stem_enabled[stem_idx];
                    (was_enabled, now_enabled)
                });
                // Sync to audio engine (after releasing borrow)
                if let Some((was_enabled, now_enabled)) = enabled_change {
                    if now_enabled != was_enabled {
                        if let Some(stem) = Stem::from_index(stem_idx) {
                            // Set buffer bars before enabling (use config value)
                            if now_enabled {
                                let buffer_bars = self.config.slicer.validated_buffer_bars();
                                self.audio.set_slicer_buffer_bars(stem, buffer_bars);
                            }
                            self.audio.set_slicer_enabled(stem, now_enabled);
                        }
                    }
                }
            }
            Message::SliceEditorPresetSelect(preset_idx) => {
                // Select preset and get preset data for activation
                let preset_data = self.collection.loaded_track.as_mut().map(|state| {
                    state.slice_editor.select_preset(preset_idx);
                    let presets = state.slice_editor.to_engine_presets();
                    // Clone the selected preset's stem configuration for activation
                    let stem_has_pattern: [bool; 4] = std::array::from_fn(|i| {
                        state.slice_editor.presets[preset_idx].stems[i].is_some()
                    });
                    (presets, stem_has_pattern)
                });

                // Sync presets and activate slicer for stems with patterns
                if let Some((presets, stem_has_pattern)) = preset_data {
                    use mesh_core::types::Stem;
                    let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

                    // Send preset data to engine
                    self.audio.set_slicer_presets(presets);

                    // Activate slicer for each stem that has a pattern in this preset
                    // shift_held=false means "select preset" mode (enables slicer + loads pattern)
                    for (idx, &stem) in stems.iter().enumerate() {
                        if stem_has_pattern[idx] {
                            self.audio.slicer_button_action(stem, preset_idx as u8, false);
                        }
                    }
                }
            }
            Message::SaveSlicerPresets => {
                // Save current slice editor presets to dedicated slicer-presets.yaml file
                if let Some(ref state) = self.collection.loaded_track {
                    // Preserve existing buffer_bars when saving presets
                    let buffer_bars = self.config.slicer.validated_buffer_bars();
                    let slicer_config = crate::config::SlicerConfig::from_editor_state_with_buffer(
                        &state.slice_editor,
                        buffer_bars,
                    );

                    // Update in-memory config as well
                    let mut new_config = (*self.config).clone();
                    new_config.slicer = slicer_config.clone();
                    self.config = Arc::new(new_config);

                    // Save to dedicated presets file (shared with mesh-player)
                    let collection_path = self.collection.collection.path().to_path_buf();
                    return Task::perform(
                        async move {
                            mesh_widgets::save_slicer_presets(&slicer_config, &collection_path).ok()
                        },
                        |_| Message::Tick,
                    );
                }
            }

            Message::HotCuePressed(index) => {
                // Shift+click = delete cue point
                if self.shift_held {
                    return self.update(Message::ClearCuePoint(index));
                }

                // CDJ-style hot cue press - use audio engine
                if let Some(ref mut state) = self.collection.loaded_track {
                    self.audio.hot_cue_press(index);
                    // Position and state will update via atomics on next tick
                    let pos = state.playhead_position();
                    let cue_pos = state.cue_point();

                    // Update waveform positions and cue marker
                    if state.duration_samples > 0 {
                        let normalized = pos as f64 / state.duration_samples as f64;
                        let cue_normalized = cue_pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);
                        state.combined_waveform.overview.set_cue_position(Some(cue_normalized));
                    }
                    state.update_zoomed_waveform_cache(pos);
                }
            }
            Message::HotCueReleased(_index) => {
                // Use audio engine for CDJ-style hot cue release
                if let Some(ref mut state) = self.collection.loaded_track {
                    // Check if we were in preview mode BEFORE releasing
                    let was_previewing = state.play_state() == PlayState::Cueing;

                    self.audio.hot_cue_release();

                    // Always pause audio when releasing from preview mode
                    if was_previewing {
                        self.audio.pause();
                    }

                    let pos = state.playhead_position();
                    // Update waveform positions
                    if state.duration_samples > 0 {
                        let normalized = pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);
                    }
                    state.update_zoomed_waveform_cache(pos);
                }
            }

            // Misc
            Message::Tick => {
                // Update UI from audio engine state (atomics)
                if let Some(ref mut state) = self.collection.loaded_track {
                    let pos = self.audio.position();
                    // Update playhead timestamp for smooth interpolation
                    state.touch_playhead();

                    if state.duration_samples > 0 {
                        let normalized = pos as f64 / state.duration_samples as f64;
                        state.combined_waveform.overview.set_position(normalized);

                        // Update loop region overlay (green overlay when loop is active)
                        if state.is_loop_active() {
                            let (loop_start, loop_end) = state.loop_bounds();
                            let start_norm = loop_start as f64 / state.duration_samples as f64;
                            let end_norm = loop_end as f64 / state.duration_samples as f64;
                            state.combined_waveform.overview.set_loop_region(Some((start_norm, end_norm)));
                            state.combined_waveform.zoomed.set_loop_region(Some((start_norm, end_norm)));
                        } else {
                            state.combined_waveform.overview.set_loop_region(None);
                            state.combined_waveform.zoomed.set_loop_region(None);
                        }
                    }

                    // Sync linked stem state from atomics for waveform display
                    let linked_atomics = self.audio.linked_stem_atomics();
                    for stem_idx in 0..4 {
                        let has_linked = linked_atomics.has_linked[stem_idx]
                            .load(std::sync::atomic::Ordering::Relaxed);
                        let is_active = linked_atomics.use_linked[stem_idx]
                            .load(std::sync::atomic::Ordering::Relaxed);
                        state.combined_waveform.set_linked_stem(stem_idx, has_linked, is_active);
                    }

                    // Sync LUFS gain from engine for waveform scaling (single source of truth)
                    let lufs_gain = self.audio.lufs_gain();
                    state.combined_waveform.zoomed.set_lufs_gain(lufs_gain);
                }

                if let Some(ref mut state) = self.collection.loaded_track {
                    // Update zoomed waveform peaks if playhead moved outside cache
                    let pos = self.audio.position();
                    if state.combined_waveform.zoomed.needs_recompute(pos, &state.combined_waveform.linked_active) {
                        if let Some(ref stems) = state.stems {
                            state.combined_waveform.zoomed.compute_peaks(stems, pos, 1600);
                        }
                    }

                    // Sync slicer state from atomics for waveform overlay
                    // Check all 4 stems for active slicer
                    let slicer_atomics = self.audio.slicer_atomics();
                    let duration = state.duration_samples as u64;
                    let mut any_active = false;

                    for stem_idx in 0..4 {
                        let sa = &slicer_atomics[stem_idx];
                        let active = sa.active.load(std::sync::atomic::Ordering::Relaxed);
                        if active && duration > 0 {
                            let buffer_start = sa.buffer_start.load(std::sync::atomic::Ordering::Relaxed);
                            let buffer_end = sa.buffer_end.load(std::sync::atomic::Ordering::Relaxed);
                            let current_slice = sa.current_slice.load(std::sync::atomic::Ordering::Relaxed);

                            // Convert to normalized positions
                            let start_norm = buffer_start as f64 / duration as f64;
                            let end_norm = buffer_end as f64 / duration as f64;

                            // Set slicer overlay on both waveform views
                            state.combined_waveform.overview.set_slicer_region(
                                Some((start_norm, end_norm)),
                                Some(current_slice),
                            );
                            state.combined_waveform.zoomed.set_slicer_region(
                                Some((start_norm, end_norm)),
                                Some(current_slice),
                            );

                            // Set fixed buffer view mode (waveform moves, playhead stays centered)
                            state.combined_waveform.zoomed.set_fixed_buffer_bounds(
                                Some((buffer_start as u64, buffer_end as u64))
                            );
                            state.combined_waveform.zoomed.set_view_mode(ZoomedViewMode::FixedBuffer);
                            // Set zoom level based on slicer buffer size
                            let buffer_bars = self.config.slicer.validated_buffer_bars();
                            state.combined_waveform.zoomed.set_fixed_buffer_zoom(buffer_bars);

                            any_active = true;
                            break; // Only show overlay for first active stem
                        }
                    }

                    // Clear slicer overlay and restore scrolling mode if no stems are active
                    if !any_active {
                        state.combined_waveform.overview.set_slicer_region(None, None);
                        state.combined_waveform.zoomed.set_slicer_region(None, None);
                        state.combined_waveform.zoomed.set_fixed_buffer_bounds(None);
                        state.combined_waveform.zoomed.set_view_mode(ZoomedViewMode::Scrolling);
                    }
                }

                // Poll import progress channel - collect first to avoid borrow issues
                let progress_messages: Vec<_> = self
                    .import_state
                    .progress_rx
                    .as_ref()
                    .map(|rx| {
                        let mut msgs = Vec::new();
                        while let Ok(progress) = rx.try_recv() {
                            msgs.push(progress);
                        }
                        msgs
                    })
                    .unwrap_or_default();

                // Process collected messages
                for progress in progress_messages {
                    let _ = self.update(Message::ImportProgressUpdate(progress));
                }

                // Poll re-analysis progress channel (same pattern as import)
                let reanalysis_messages: Vec<_> = self
                    .reanalysis_state
                    .progress_rx
                    .as_ref()
                    .map(|rx| {
                        let mut msgs = Vec::new();
                        while let Ok(progress) = rx.try_recv() {
                            msgs.push(progress);
                        }
                        msgs
                    })
                    .unwrap_or_default();

                // Process collected re-analysis messages
                for progress in reanalysis_messages {
                    let _ = self.update(Message::ReanalysisProgress(progress));
                }
            }

            // Zoomed Waveform
            Message::SetZoomBars(bars) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.combined_waveform.zoomed.set_zoom(bars);
                    // Recompute peaks at new zoom level
                    if let Some(ref stems) = state.stems {
                        state.combined_waveform.zoomed.compute_peaks(stems, state.playhead_position(), 1600);
                    }
                }
                // Persist zoom level to config (fire-and-forget save)
                let mut new_config = (*self.config).clone();
                new_config.display.zoom_bars = bars;
                self.config = Arc::new(new_config.clone());
                let config_path = self.config_path.clone();
                return Task::perform(
                    async move { config::save_config(&new_config, &config_path).ok() },
                    |_| Message::Tick, // Ignore result
                );
            }

            // Beat Grid Nudge
            Message::NudgeBeatGridLeft => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    nudge_beat_grid(state, -BEAT_GRID_NUDGE_SAMPLES);
                    // Propagate to deck so snapping uses updated grid
                    self.audio.set_beat_grid(state.beat_grid.clone());
                }
            }
            Message::NudgeBeatGridRight => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    nudge_beat_grid(state, BEAT_GRID_NUDGE_SAMPLES);
                    // Propagate to deck so snapping uses updated grid
                    self.audio.set_beat_grid(state.beat_grid.clone());
                }
            }

            // Settings
            Message::OpenSettings => {
                // Reset draft values from current config
                self.settings = SettingsState::from_config(&self.config);
                self.settings.is_open = true;
            }
            Message::CloseSettings => {
                self.settings.is_open = false;
                self.settings.status.clear();
            }
            Message::UpdateSettingsMinTempo(value) => {
                self.settings.draft_min_tempo = value;
            }
            Message::UpdateSettingsMaxTempo(value) => {
                self.settings.draft_max_tempo = value;
            }
            Message::UpdateSettingsParallelProcesses(value) => {
                self.settings.draft_parallel_processes = value;
            }
            Message::UpdateSettingsTrackNameFormat(value) => {
                self.settings.draft_track_name_format = value;
            }
            Message::UpdateSettingsGridBars(value) => {
                self.settings.draft_grid_bars = value;
            }
            Message::UpdateSettingsBpmSource(source) => {
                self.settings.draft_bpm_source = source;
            }
            Message::UpdateSettingsSlicerBufferBars(bars) => {
                self.settings.draft_slicer_buffer_bars = bars;
            }
            Message::SaveSettings => {
                // Parse and validate values
                let min = self.settings.draft_min_tempo.parse::<i32>().unwrap_or(40);
                let max = self.settings.draft_max_tempo.parse::<i32>().unwrap_or(208);
                let parallel = self.settings.draft_parallel_processes.parse::<u8>().unwrap_or(4);

                let mut new_config = (*self.config).clone();
                new_config.analysis.bpm.min_tempo = min;
                new_config.analysis.bpm.max_tempo = max;
                new_config.analysis.bpm.source = self.settings.draft_bpm_source;
                new_config.analysis.parallel_processes = parallel;
                new_config.analysis.validate(); // validates both bpm and parallel_processes

                // Update track name format
                new_config.track_name_format = self.settings.draft_track_name_format.clone();

                // Update display settings (grid bars)
                new_config.display.grid_bars = self.settings.draft_grid_bars;

                // Update slicer buffer bars
                new_config.slicer.buffer_bars = self.settings.draft_slicer_buffer_bars;

                // Update drafts to show validated values
                self.settings.draft_min_tempo = new_config.analysis.bpm.min_tempo.to_string();
                self.settings.draft_max_tempo = new_config.analysis.bpm.max_tempo.to_string();
                self.settings.draft_parallel_processes = new_config.analysis.parallel_processes.to_string();

                // Save to file
                let config_path = self.config_path.clone();
                let config_clone = new_config.clone();

                self.config = Arc::new(new_config);

                return Task::perform(
                    async move {
                        config::save_config(&config_clone, &config_path)
                            .map_err(|e| e.to_string())
                    },
                    Message::SaveSettingsComplete,
                );
            }
            Message::SaveSettingsComplete(result) => {
                match result {
                    Ok(()) => {
                        log::info!("Settings saved successfully");
                        self.settings.status = String::from("Settings saved!");
                        // Close modal after brief delay would be nice, for now just close
                        self.settings.is_open = false;
                    }
                    Err(e) => {
                        log::error!("Failed to save settings: {}", e);
                        self.settings.status = format!("Failed to save: {}", e);
                    }
                }
            }
            Message::KeyPressed(key, modifiers, repeat) => {
                // Track modifier key states for selection actions
                self.shift_held = modifiers.shift();
                self.ctrl_held = modifiers.control();

                // Only handle keybindings in Collection view
                if self.current_view != View::Collection {
                    return Task::none();
                }

                // Enter key loads selected track (works even without a loaded track)
                // With multi-selection, loads the most recently selected track
                if !repeat {
                    if let iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) = &key {
                        log::info!("Enter pressed - checking for selected track");
                        // Check left browser's most recent selection first
                        if let Some(ref track_id) =
                            self.collection.browser_left.table_state.last_selected
                        {
                            log::info!("  Found selection in left browser: {:?}", track_id);
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(track_id) {
                                    log::info!(
                                        "  Node found: kind={:?}, track_path={:?}",
                                        node.kind,
                                        node.track_path
                                    );
                                    if let Some(path) = node.track_path {
                                        return self.update(Message::LoadTrackByPath(path));
                                    }
                                }
                            }
                        }
                        // Then check right browser
                        if let Some(ref track_id) =
                            self.collection.browser_right.table_state.last_selected
                        {
                            log::info!("  Found selection in right browser: {:?}", track_id);
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(track_id) {
                                    log::info!(
                                        "  Node found: kind={:?}, track_path={:?}",
                                        node.kind,
                                        node.track_path
                                    );
                                    if let Some(path) = node.track_path {
                                        return self.update(Message::LoadTrackByPath(path));
                                    }
                                }
                            }
                        }
                    }

                    // Delete key opens delete confirmation modal
                    if let iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete) = &key {
                        log::info!("Delete pressed - checking for selected tracks");
                        // Check which browser has selection (prefer left)
                        if self.collection.browser_left.table_state.has_selection() {
                            return self.update(Message::RequestDelete(BrowserSide::Left));
                        } else if self.collection.browser_right.table_state.has_selection() {
                            return self.update(Message::RequestDelete(BrowserSide::Right));
                        }
                    }
                }

                // Remaining keybindings require a loaded track
                if self.collection.loaded_track.is_none() {
                    return Task::none();
                }

                // Convert key + modifiers to string for matching
                let key_str = keybindings::key_to_string(&key, &modifiers);
                if key_str.is_empty() {
                    return Task::none();
                }

                let bindings = &self.keybindings.editing;

                // Play/Pause (ignore repeat)
                if !repeat && bindings.play_pause.iter().any(|b| b == &key_str) {
                    let is_playing = self.collection.loaded_track.as_ref()
                        .map(|s| s.is_playing()).unwrap_or(false);
                    return self.update(if is_playing { Message::Pause } else { Message::Play });
                }

                // Beat jump forward/backward (allow repeat for continuous jumping)
                if bindings.beat_jump_forward.iter().any(|b| b == &key_str) {
                    let jump_size = self.collection.loaded_track.as_ref()
                        .map(|s| s.beat_jump_size()).unwrap_or(4);
                    return self.update(Message::BeatJump(jump_size));
                }
                if bindings.beat_jump_backward.iter().any(|b| b == &key_str) {
                    let jump_size = self.collection.loaded_track.as_ref()
                        .map(|s| s.beat_jump_size()).unwrap_or(4);
                    return self.update(Message::BeatJump(-jump_size));
                }

                // Beat grid nudge (allow repeat)
                if bindings.grid_nudge_forward.iter().any(|b| b == &key_str) {
                    return self.update(Message::NudgeBeatGridRight);
                }
                if bindings.grid_nudge_backward.iter().any(|b| b == &key_str) {
                    return self.update(Message::NudgeBeatGridLeft);
                }

                // Increase/decrease loop length (also affects beat jump size, ignore repeat)
                if !repeat && bindings.increase_jump_size.iter().any(|b| b == &key_str) {
                    if self.collection.loaded_track.is_some() {
                        self.audio.adjust_loop_length(1); // Double
                    }
                    return Task::none();
                }
                if !repeat && bindings.decrease_jump_size.iter().any(|b| b == &key_str) {
                    if self.collection.loaded_track.is_some() {
                        self.audio.adjust_loop_length(-1); // Halve
                    }
                    return Task::none();
                }

                // Delete hot cues (ignore repeat)
                if !repeat {
                    if let Some(index) = bindings.match_delete_hot_cue(&key_str) {
                        return self.update(Message::ClearCuePoint(index));
                    }
                }

                // Main cue button (filter repeat - only trigger on first press)
                if bindings.match_cue_button(&key_str) {
                    if !repeat && !self.pressed_cue_key {
                        self.pressed_cue_key = true;
                        return self.update(Message::Cue);
                    }
                    return Task::none();
                }

                // Hot cue trigger/set (filter repeat - only trigger on first press)
                if let Some(index) = bindings.match_hot_cue(&key_str) {
                    // Skip if repeat and key already pressed
                    if repeat && self.pressed_hot_cue_keys.contains(&index) {
                        return Task::none();
                    }

                    // Track this key as pressed
                    self.pressed_hot_cue_keys.insert(index);

                    // If cue exists, trigger it; otherwise set it
                    let cue_exists = self.collection.loaded_track.as_ref()
                        .map(|s| s.cue_points.iter().any(|c| c.index == index as u8))
                        .unwrap_or(false);
                    if cue_exists {
                        return self.update(Message::HotCuePressed(index));
                    } else {
                        return self.update(Message::SetCuePoint(index));
                    }
                }
            }
            Message::KeyReleased(key, modifiers) => {
                // ALWAYS update modifier state, regardless of view
                // This fixes Shift+Click not working after Shift is released
                self.shift_held = modifiers.shift();
                self.ctrl_held = modifiers.control();

                // Only handle keybindings in Collection view with a loaded track
                if self.current_view != View::Collection {
                    return Task::none();
                }
                if self.collection.loaded_track.is_none() {
                    return Task::none();
                }

                // Convert key to string for matching
                let key_str = keybindings::key_to_string(&key, &modifiers);
                if key_str.is_empty() {
                    return Task::none();
                }

                let bindings = &self.keybindings.editing;

                // Main cue button release - stop preview, return to cue point
                if bindings.match_cue_button(&key_str) && self.pressed_cue_key {
                    self.pressed_cue_key = false;
                    return self.update(Message::CueReleased);
                }

                // Hot cue release - dispatch HotCueReleased to stop preview
                if let Some(index) = bindings.match_hot_cue(&key_str) {
                    // Only release if this key was tracked as pressed
                    if self.pressed_hot_cue_keys.remove(&index) {
                        // Only send release if cue exists (preview was started)
                        let cue_exists = self.collection.loaded_track.as_ref()
                            .map(|s| s.cue_points.iter().any(|c| c.index == index as u8))
                            .unwrap_or(false);
                        if cue_exists {
                            return self.update(Message::HotCueReleased(index));
                        }
                    }
                }
            }
            Message::ModifiersChanged(modifiers) => {
                // Track modifier key states for Shift+Click and Ctrl+Click selection
                // This fires when modifiers change without another key being pressed
                self.shift_held = modifiers.shift();
                self.ctrl_held = modifiers.control();
                log::debug!(
                    "[MODIFIERS] shift={}, ctrl={}",
                    self.shift_held,
                    self.ctrl_held
                );
            }
            Message::GlobalMouseMoved(position) => {
                self.global_mouse_position = position;
            }

            // Playlist Browsers
            Message::BrowserLeft(browser_msg) => {
                match browser_msg {
                    PlaylistBrowserMessage::Tree(ref tree_msg) => {
                        use mesh_widgets::TreeMessage;

                        // Handle special messages that need storage operations
                        match tree_msg {
                            TreeMessage::CreateChild(parent_id) => {
                                if let Some(ref mut storage) = self.collection.playlist_storage {
                                    match storage.create_playlist(parent_id, "New Playlist") {
                                        Ok(new_id) => {
                                            log::info!("Created playlist: {:?}", new_id);
                                            self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                            // Start editing the new playlist name
                                            self.collection.browser_left.tree_state.start_edit(
                                                new_id,
                                                "New Playlist".to_string(),
                                            );
                                        }
                                        Err(e) => log::error!("Failed to create playlist: {:?}", e),
                                    }
                                }
                            }
                            TreeMessage::StartEdit(id) => {
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    if let Some(node) = storage.get_node(id) {
                                        self.collection.browser_left.tree_state.start_edit(
                                            id.clone(),
                                            node.name.clone(),
                                        );
                                    }
                                }
                            }
                            TreeMessage::CommitEdit => {
                                if let Some((id, new_name)) = self.collection.browser_left.tree_state.commit_edit() {
                                    if let Some(ref mut storage) = self.collection.playlist_storage {
                                        if let Err(e) = storage.rename_playlist(&id, &new_name) {
                                            log::error!("Failed to rename playlist: {:?}", e);
                                        }
                                        self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                    }
                                }
                            }
                            TreeMessage::CancelEdit => {
                                self.collection.browser_left.tree_state.cancel_edit();
                            }
                            TreeMessage::DropReceived(target_id) => {
                                log::debug!("Left tree: DropReceived on {:?}", target_id);
                                // Check if we're dragging track(s) and this is a valid drop target
                                if let Some(ref drag) = self.collection.dragging_track {
                                    log::debug!("  Currently dragging: {}", drag.display_text());
                                    if let Some(ref storage) = self.collection.playlist_storage {
                                        if let Some(target_node) = storage.get_node(target_id) {
                                            log::debug!("  Target node kind: {:?}", target_node.kind);
                                            // Only allow dropping onto playlists
                                            if target_node.kind == NodeKind::Playlist
                                                || target_node.kind == NodeKind::PlaylistsRoot
                                            {
                                                log::info!(
                                                    "Drop on left tree: {} -> {:?}",
                                                    drag.display_text(),
                                                    target_id
                                                );
                                                return self.update(Message::DropTracksOnPlaylist {
                                                    track_ids: drag.track_ids.clone(),
                                                    target_playlist: target_id.clone(),
                                                });
                                            } else {
                                                log::debug!("  Target is not a playlist, ignoring drop");
                                            }
                                        } else {
                                            log::debug!("  Target node not found in storage");
                                        }
                                    }
                                } else {
                                    log::debug!("  Not dragging anything, just mouse release");
                                }
                                // Always end drag on mouse release
                                return self.update(Message::DragTrackEnd);
                            }
                            TreeMessage::RightClick(id, _widget_position) => {
                                // Use global mouse position for accurate menu placement
                                let position = self.global_mouse_position;
                                log::info!("[LEFT TREE] RightClick received: id={:?}, global_position={:?}", id, position);
                                // Show context menu for the tree node
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    if let Some(node) = storage.get_node(id) {
                                        log::info!("[LEFT TREE] Node found: kind={:?}, name={}", node.kind, node.name);
                                        let menu_kind = if node.kind == NodeKind::Collection {
                                            super::context_menu::ContextMenuKind::Collection
                                        } else {
                                            super::context_menu::ContextMenuKind::Playlist {
                                                playlist_id: id.clone(),
                                                playlist_name: node.name.clone(),
                                            }
                                        };
                                        log::info!("[LEFT TREE] Showing context menu: {:?}", menu_kind);
                                        return self.update(Message::ShowContextMenu(menu_kind, position));
                                    } else {
                                        log::warn!("[LEFT TREE] Node not found in storage: {:?}", id);
                                    }
                                } else {
                                    log::warn!("[LEFT TREE] No playlist storage");
                                }
                            }
                            TreeMessage::MouseMoved(_) => {
                                // Widget-relative position, not used (we track global position via subscription)
                            }
                            _ => {
                                // Handle Toggle, Select, EditChanged via the standard handler
                                let folder_changed = self.collection.browser_left.handle_tree_message(tree_msg);
                                if folder_changed {
                                    log::debug!("Left browser folder changed to {:?}", self.collection.browser_left.current_folder);
                                    // Refresh cached tracks for this browser
                                    if let Some(ref folder) = self.collection.browser_left.current_folder {
                                        if let Some(ref storage) = self.collection.playlist_storage {
                                            self.collection.left_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                        }
                                    } else {
                                        self.collection.left_tracks.clear();
                                    }
                                }
                            }
                        }
                    }
                    PlaylistBrowserMessage::Table(table_msg) => {
                        // Handle selection with CURRENT modifier state (not render-time state)
                        // This fixes Shift+Click and Ctrl+Click multi-selection
                        if let TrackTableMessage::Select(track_id) = &table_msg {
                            let modifiers = mesh_widgets::SelectModifiers {
                                shift: self.shift_held,
                                ctrl: self.ctrl_held,
                            };
                            let already_selected = self
                                .collection
                                .browser_left
                                .table_state
                                .is_selected(track_id);
                            log::info!(
                                "[LEFT SELECT] track_id={:?}, shift={}, ctrl={}, already_selected={}, current_selection={}",
                                track_id,
                                self.shift_held,
                                self.ctrl_held,
                                already_selected,
                                self.collection.browser_left.table_state.selected.len()
                            );

                            // If clicking on already-selected track without modifiers,
                            // preserve selection for multi-drag (don't reset to single)
                            if already_selected && !modifiers.shift && !modifiers.ctrl {
                                log::info!("[LEFT SELECT] preserving multi-selection for drag");
                            } else {
                                // Get all track IDs for range selection
                                let all_ids: Vec<NodeId> = self
                                    .collection
                                    .left_tracks
                                    .iter()
                                    .map(|t| t.id.clone())
                                    .collect();
                                // Apply selection with current modifiers
                                self.collection
                                    .browser_left
                                    .table_state
                                    .handle_select(track_id.clone(), modifiers, &all_ids);
                                log::info!(
                                    "[LEFT SELECT] after handle_select: selected={}",
                                    self.collection.browser_left.table_state.selected.len()
                                );
                            }
                        }

                        // Handle table message and check for edit commits
                        if let Some((track_id, column, new_value)) =
                            self.collection.browser_left.handle_table_message(&table_msg)
                        {
                            // Edit committed - save to file
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(&track_id) {
                                    if let Some(ref path) = node.track_path {
                                        let field = match column {
                                            TrackColumn::Artist => MetadataField::Artist,
                                            TrackColumn::Bpm => MetadataField::Bpm,
                                            TrackColumn::Key => MetadataField::Key,
                                            _ => return Task::none(), // Not editable
                                        };
                                        match update_metadata_in_file(path, field, &new_value) {
                                            Ok(_) => {
                                                log::info!("Saved {:?} = '{}' for {:?}", column, new_value, track_id);

                                                // Sync to database - convert TrackColumn to DB field name
                                                let db_field = match column {
                                                    TrackColumn::Artist => "artist",
                                                    TrackColumn::Bpm => "bpm",
                                                    TrackColumn::Key => "key",
                                                    _ => "",
                                                };
                                                if !db_field.is_empty() {
                                                    if let Some(ref db) = self.collection.db {
                                                        let path_str = path.to_string_lossy();
                                                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, db_field, &new_value) {
                                                            log::warn!("Failed to sync metadata to database: {:?}", e);
                                                        } else {
                                                            log::info!("Synced {:?} to database", column);
                                                        }
                                                    }
                                                }

                                                // Refresh tracks to show updated value
                                                if let Some(ref folder) = self.collection.browser_left.current_folder {
                                                    self.collection.left_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                                }
                                            }
                                            Err(e) => log::error!("Failed to save metadata: {:?}", e),
                                        }
                                    }
                                }
                            }
                        }

                        // Handle drag initiation after selection is updated
                        // (drag is cancelled on mouse release if not dropped on valid target)
                        if let TrackTableMessage::Select(_track_id) = &table_msg {
                            // Collect all selected tracks for multi-drag
                            let selected_ids: Vec<NodeId> = self
                                .collection
                                .browser_left
                                .table_state
                                .selected
                                .iter()
                                .cloned()
                                .collect();
                            if !selected_ids.is_empty() {
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    let track_names: Vec<String> = selected_ids
                                        .iter()
                                        .filter_map(|id| storage.get_node(id).map(|n| n.name.clone()))
                                        .collect();
                                    log::debug!(
                                        "Left table: initiating drag for {} track(s)",
                                        selected_ids.len()
                                    );
                                    return self.update(Message::DragTrackStart {
                                        track_ids: selected_ids,
                                        track_names,
                                        browser: BrowserSide::Left,
                                    });
                                }
                            }
                        }
                        // Handle double-click to load track
                        if let TrackTableMessage::Activate(ref track_id) = table_msg {
                            log::info!("Left browser: Track activated (double-click): {:?}", track_id);
                            if let Some(ref storage) = self.collection.playlist_storage {
                                match storage.get_node(track_id) {
                                    Some(node) => {
                                        log::info!("  Node found: kind={:?}, name={}", node.kind, node.name);
                                        match &node.track_path {
                                            Some(path) => {
                                                log::info!("  track_path: {:?}", path);
                                                return self.update(Message::LoadTrackByPath(path.clone()));
                                            }
                                            None => {
                                                log::warn!("  track_path is None! Cannot load track.");
                                            }
                                        }
                                    }
                                    None => {
                                        log::warn!("  Node NOT FOUND for track_id: {:?}", track_id);
                                    }
                                }
                            } else {
                                log::warn!("  No playlist storage initialized!");
                            }
                        }
                        // Handle drop on table (adds to current folder)
                        if let TrackTableMessage::DropReceived(_drop_track_id) = table_msg {
                            log::debug!("Left table: DropReceived");
                            if let Some(ref drag) = self.collection.dragging_track {
                                // Get the currently selected folder as drop target
                                if let Some(ref current_folder) = self.collection.browser_left.current_folder {
                                    if let Some(ref storage) = self.collection.playlist_storage {
                                        if let Some(folder_node) = storage.get_node(current_folder) {
                                            // Allow dropping onto playlists (not collection folders)
                                            if folder_node.kind == NodeKind::Playlist
                                                || folder_node.kind == NodeKind::PlaylistsRoot
                                            {
                                                log::info!(
                                                    "Drop on left table: {} -> {:?}",
                                                    drag.display_text(),
                                                    current_folder
                                                );
                                                return self.update(Message::DropTracksOnPlaylist {
                                                    track_ids: drag.track_ids.clone(),
                                                    target_playlist: current_folder.clone(),
                                                });
                                            } else {
                                                log::debug!("  Current folder is not a playlist, ignoring drop");
                                            }
                                        }
                                    }
                                }
                            }
                            // End drag on mouse release
                            return self.update(Message::DragTrackEnd);
                        }
                        // Handle right-click on track
                        if let TrackTableMessage::RightClick(track_id, _widget_position) = &table_msg {
                            // Use global mouse position for accurate menu placement
                            let position = self.global_mouse_position;
                            log::info!("[LEFT TABLE] RightClick received: track_id={:?}, global_position={:?}", track_id, position);
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(track_id) {
                                    log::info!("[LEFT TABLE] Track found: name={}", node.name);
                                    // Determine if we're in collection or playlist view
                                    let is_playlist_view = self.collection.browser_left.current_folder.as_ref()
                                        .and_then(|f| storage.get_node(f))
                                        .map(|n| n.kind == NodeKind::Playlist || n.kind == NodeKind::PlaylistsRoot)
                                        .unwrap_or(false);

                                    // Get currently selected tracks for batch operations
                                    let selected_tracks: Vec<NodeId> = self.collection.browser_left.table_state
                                        .selected
                                        .iter()
                                        .filter(|id| *id != track_id)
                                        .cloned()
                                        .collect();

                                    let menu_kind = if is_playlist_view {
                                        super::context_menu::ContextMenuKind::PlaylistTrack {
                                            track_id: track_id.clone(),
                                            track_name: node.name.clone(),
                                            selected_tracks,
                                        }
                                    } else {
                                        super::context_menu::ContextMenuKind::CollectionTrack {
                                            track_id: track_id.clone(),
                                            track_name: node.name.clone(),
                                            selected_tracks,
                                        }
                                    };
                                    log::info!("[LEFT TABLE] Showing context menu: is_playlist={}", is_playlist_view);
                                    return self.update(Message::ShowContextMenu(menu_kind, position));
                                } else {
                                    log::warn!("[LEFT TABLE] Track not found in storage: {:?}", track_id);
                                }
                            } else {
                                log::warn!("[LEFT TABLE] No playlist storage");
                            }
                        }
                        // Sort tracks when sort column/direction changes
                        if let TrackTableMessage::SortBy(_) = &table_msg {
                            let state = &self.collection.browser_left.table_state;
                            sort_tracks(
                                &mut self.collection.left_tracks,
                                state.sort_column,
                                state.sort_ascending,
                            );
                        }
                    }
                }
            }
            Message::BrowserRight(browser_msg) => {
                match browser_msg {
                    PlaylistBrowserMessage::Tree(ref tree_msg) => {
                        use mesh_widgets::TreeMessage;

                        // Handle special messages that need storage operations
                        match tree_msg {
                            TreeMessage::CreateChild(parent_id) => {
                                if let Some(ref mut storage) = self.collection.playlist_storage {
                                    match storage.create_playlist(parent_id, "New Playlist") {
                                        Ok(new_id) => {
                                            log::info!("Created playlist: {:?}", new_id);
                                            self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                            // Start editing the new playlist name
                                            self.collection.browser_right.tree_state.start_edit(
                                                new_id,
                                                "New Playlist".to_string(),
                                            );
                                        }
                                        Err(e) => log::error!("Failed to create playlist: {:?}", e),
                                    }
                                }
                            }
                            TreeMessage::StartEdit(id) => {
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    if let Some(node) = storage.get_node(id) {
                                        self.collection.browser_right.tree_state.start_edit(
                                            id.clone(),
                                            node.name.clone(),
                                        );
                                    }
                                }
                            }
                            TreeMessage::CommitEdit => {
                                if let Some((id, new_name)) = self.collection.browser_right.tree_state.commit_edit() {
                                    if let Some(ref mut storage) = self.collection.playlist_storage {
                                        if let Err(e) = storage.rename_playlist(&id, &new_name) {
                                            log::error!("Failed to rename playlist: {:?}", e);
                                        }
                                        self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                    }
                                }
                            }
                            TreeMessage::CancelEdit => {
                                self.collection.browser_right.tree_state.cancel_edit();
                            }
                            TreeMessage::DropReceived(target_id) => {
                                log::debug!("Right tree: DropReceived on {:?}", target_id);
                                // Check if we're dragging track(s) and this is a valid drop target
                                if let Some(ref drag) = self.collection.dragging_track {
                                    log::debug!("  Currently dragging: {}", drag.display_text());
                                    if let Some(ref storage) = self.collection.playlist_storage {
                                        if let Some(target_node) = storage.get_node(target_id) {
                                            log::debug!("  Target node kind: {:?}", target_node.kind);
                                            // Only allow dropping onto playlists
                                            if target_node.kind == NodeKind::Playlist
                                                || target_node.kind == NodeKind::PlaylistsRoot
                                            {
                                                log::info!(
                                                    "Drop on right tree: {} -> {:?}",
                                                    drag.display_text(),
                                                    target_id
                                                );
                                                return self.update(Message::DropTracksOnPlaylist {
                                                    track_ids: drag.track_ids.clone(),
                                                    target_playlist: target_id.clone(),
                                                });
                                            } else {
                                                log::debug!("  Target is not a playlist, ignoring drop");
                                            }
                                        } else {
                                            log::debug!("  Target node not found in storage");
                                        }
                                    }
                                } else {
                                    log::debug!("  Not dragging anything, just mouse release");
                                }
                                // Always end drag on mouse release
                                return self.update(Message::DragTrackEnd);
                            }
                            TreeMessage::RightClick(id, _widget_position) => {
                                // Use global mouse position for accurate menu placement
                                let position = self.global_mouse_position;
                                // Show context menu for the tree node
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    if let Some(node) = storage.get_node(id) {
                                        let menu_kind = if node.kind == NodeKind::Collection {
                                            super::context_menu::ContextMenuKind::Collection
                                        } else {
                                            super::context_menu::ContextMenuKind::Playlist {
                                                playlist_id: id.clone(),
                                                playlist_name: node.name.clone(),
                                            }
                                        };
                                        return self.update(Message::ShowContextMenu(menu_kind, position));
                                    }
                                }
                            }
                            TreeMessage::MouseMoved(_) => {
                                // Widget-relative position, not used (we track global position via subscription)
                            }
                            _ => {
                                // Handle Toggle, Select, EditChanged via the standard handler
                                let folder_changed = self.collection.browser_right.handle_tree_message(tree_msg);
                                if folder_changed {
                                    log::debug!("Right browser folder changed to {:?}", self.collection.browser_right.current_folder);
                                    // Refresh cached tracks for this browser
                                    if let Some(ref folder) = self.collection.browser_right.current_folder {
                                        if let Some(ref storage) = self.collection.playlist_storage {
                                            self.collection.right_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                        }
                                    } else {
                                        self.collection.right_tracks.clear();
                                    }
                                }
                            }
                        }
                    }
                    PlaylistBrowserMessage::Table(table_msg) => {
                        // Handle selection with CURRENT modifier state (not render-time state)
                        // This fixes Shift+Click and Ctrl+Click multi-selection
                        if let TrackTableMessage::Select(track_id) = &table_msg {
                            let modifiers = mesh_widgets::SelectModifiers {
                                shift: self.shift_held,
                                ctrl: self.ctrl_held,
                            };
                            let already_selected = self
                                .collection
                                .browser_right
                                .table_state
                                .is_selected(track_id);

                            // If clicking on already-selected track without modifiers,
                            // preserve selection for multi-drag (don't reset to single)
                            if already_selected && !modifiers.shift && !modifiers.ctrl {
                                log::info!("[RIGHT SELECT] preserving multi-selection for drag");
                            } else {
                                // Get all track IDs for range selection
                                let all_ids: Vec<NodeId> = self
                                    .collection
                                    .right_tracks
                                    .iter()
                                    .map(|t| t.id.clone())
                                    .collect();
                                // Apply selection with current modifiers
                                self.collection
                                    .browser_right
                                    .table_state
                                    .handle_select(track_id.clone(), modifiers, &all_ids);
                            }
                        }

                        // Handle table message and check for edit commits
                        if let Some((track_id, column, new_value)) =
                            self.collection.browser_right.handle_table_message(&table_msg)
                        {
                            // Edit committed - save to file
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(&track_id) {
                                    if let Some(ref path) = node.track_path {
                                        let field = match column {
                                            TrackColumn::Artist => MetadataField::Artist,
                                            TrackColumn::Bpm => MetadataField::Bpm,
                                            TrackColumn::Key => MetadataField::Key,
                                            _ => return Task::none(), // Not editable
                                        };
                                        match update_metadata_in_file(path, field, &new_value) {
                                            Ok(_) => {
                                                log::info!("Saved {:?} = '{}' for {:?}", column, new_value, track_id);

                                                // Sync to database - convert TrackColumn to DB field name
                                                let db_field = match column {
                                                    TrackColumn::Artist => "artist",
                                                    TrackColumn::Bpm => "bpm",
                                                    TrackColumn::Key => "key",
                                                    _ => "",
                                                };
                                                if !db_field.is_empty() {
                                                    if let Some(ref db) = self.collection.db {
                                                        let path_str = path.to_string_lossy();
                                                        if let Err(e) = TrackQuery::update_field_by_path(db, &path_str, db_field, &new_value) {
                                                            log::warn!("Failed to sync metadata to database: {:?}", e);
                                                        } else {
                                                            log::info!("Synced {:?} to database", column);
                                                        }
                                                    }
                                                }

                                                // Refresh tracks to show updated value
                                                if let Some(ref folder) = self.collection.browser_right.current_folder {
                                                    self.collection.right_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                                }
                                            }
                                            Err(e) => log::error!("Failed to save metadata: {:?}", e),
                                        }
                                    }
                                }
                            }
                        }

                        // Handle drag initiation after selection is updated
                        // (drag is cancelled on mouse release if not dropped on valid target)
                        if let TrackTableMessage::Select(_track_id) = &table_msg {
                            // Collect all selected tracks for multi-drag
                            let selected_ids: Vec<NodeId> = self
                                .collection
                                .browser_right
                                .table_state
                                .selected
                                .iter()
                                .cloned()
                                .collect();
                            if !selected_ids.is_empty() {
                                if let Some(ref storage) = self.collection.playlist_storage {
                                    let track_names: Vec<String> = selected_ids
                                        .iter()
                                        .filter_map(|id| storage.get_node(id).map(|n| n.name.clone()))
                                        .collect();
                                    log::debug!(
                                        "Right table: initiating drag for {} track(s)",
                                        selected_ids.len()
                                    );
                                    return self.update(Message::DragTrackStart {
                                        track_ids: selected_ids,
                                        track_names,
                                        browser: BrowserSide::Right,
                                    });
                                }
                            }
                        }
                        // Handle double-click to load track
                        if let TrackTableMessage::Activate(ref track_id) = table_msg {
                            log::info!("Right browser: Track activated (double-click): {:?}", track_id);
                            if let Some(ref storage) = self.collection.playlist_storage {
                                match storage.get_node(track_id) {
                                    Some(node) => {
                                        log::info!("  Node found: kind={:?}, name={}", node.kind, node.name);
                                        match &node.track_path {
                                            Some(path) => {
                                                log::info!("  track_path: {:?}", path);
                                                return self.update(Message::LoadTrackByPath(path.clone()));
                                            }
                                            None => {
                                                log::warn!("  track_path is None! Cannot load track.");
                                            }
                                        }
                                    }
                                    None => {
                                        log::warn!("  Node NOT FOUND for track_id: {:?}", track_id);
                                    }
                                }
                            } else {
                                log::warn!("  No playlist storage initialized!");
                            }
                        }
                        // Handle drop on table (adds to current folder)
                        if let TrackTableMessage::DropReceived(_drop_track_id) = table_msg {
                            log::debug!("Right table: DropReceived");
                            if let Some(ref drag) = self.collection.dragging_track {
                                // Get the currently selected folder as drop target
                                if let Some(ref current_folder) = self.collection.browser_right.current_folder {
                                    if let Some(ref storage) = self.collection.playlist_storage {
                                        if let Some(folder_node) = storage.get_node(current_folder) {
                                            // Allow dropping onto playlists (not collection folders)
                                            if folder_node.kind == NodeKind::Playlist
                                                || folder_node.kind == NodeKind::PlaylistsRoot
                                            {
                                                log::info!(
                                                    "Drop on right table: {} -> {:?}",
                                                    drag.display_text(),
                                                    current_folder
                                                );
                                                return self.update(Message::DropTracksOnPlaylist {
                                                    track_ids: drag.track_ids.clone(),
                                                    target_playlist: current_folder.clone(),
                                                });
                                            } else {
                                                log::debug!("  Current folder is not a playlist, ignoring drop");
                                            }
                                        }
                                    }
                                }
                            }
                            // End drag on mouse release
                            return self.update(Message::DragTrackEnd);
                        }
                        // Handle right-click on track
                        if let TrackTableMessage::RightClick(track_id, _widget_position) = &table_msg {
                            // Use global mouse position for accurate menu placement
                            let position = self.global_mouse_position;
                            if let Some(ref storage) = self.collection.playlist_storage {
                                if let Some(node) = storage.get_node(track_id) {
                                    // Determine if we're in collection or playlist view
                                    let is_playlist_view = self.collection.browser_right.current_folder.as_ref()
                                        .and_then(|f| storage.get_node(f))
                                        .map(|n| n.kind == NodeKind::Playlist || n.kind == NodeKind::PlaylistsRoot)
                                        .unwrap_or(false);

                                    // Get currently selected tracks for batch operations
                                    let selected_tracks: Vec<NodeId> = self.collection.browser_right.table_state
                                        .selected
                                        .iter()
                                        .filter(|id| *id != track_id)
                                        .cloned()
                                        .collect();

                                    let menu_kind = if is_playlist_view {
                                        super::context_menu::ContextMenuKind::PlaylistTrack {
                                            track_id: track_id.clone(),
                                            track_name: node.name.clone(),
                                            selected_tracks,
                                        }
                                    } else {
                                        super::context_menu::ContextMenuKind::CollectionTrack {
                                            track_id: track_id.clone(),
                                            track_name: node.name.clone(),
                                            selected_tracks,
                                        }
                                    };
                                    return self.update(Message::ShowContextMenu(menu_kind, position));
                                }
                            }
                        }
                        // Sort tracks when sort column/direction changes
                        if let TrackTableMessage::SortBy(_) = &table_msg {
                            let state = &self.collection.browser_right.table_state;
                            sort_tracks(
                                &mut self.collection.right_tracks,
                                state.sort_column,
                                state.sort_ascending,
                            );
                        }
                    }
                }
            }
            Message::RefreshPlaylists => {
                if let Some(ref mut storage) = self.collection.playlist_storage {
                    if let Err(e) = storage.refresh() {
                        log::error!("Failed to refresh playlists: {:?}", e);
                    } else {
                        self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                    }
                }
            }
            Message::LoadTrackByPath(path) => {
                // Auto-save current track if modified before loading new one
                let save_task = self.save_current_track_if_modified();

                log::info!("LoadTrackByPath: Loading {:?}", path);
                let load_task = Task::perform(
                    async move {
                        LoadedTrack::load_metadata_only(&path)
                            .map(|metadata| (path, metadata))
                            .map_err(|e| e.to_string())
                    },
                    Message::TrackMetadataLoaded,
                );

                if let Some(save) = save_task {
                    return Task::batch([save, load_task]);
                }
                return load_task;
            }

            // Drag and Drop
            Message::DragTrackStart { track_ids, track_names, browser } => {
                let display = if track_names.len() == 1 {
                    track_names[0].clone()
                } else {
                    format!("{} tracks", track_names.len())
                };
                log::info!("Drag started: {} from {:?}", display, browser);
                self.collection.dragging_track =
                    Some(DragState::multiple(track_ids, track_names, browser));
            }
            Message::DragTrackEnd => {
                if let Some(ref drag) = self.collection.dragging_track {
                    log::info!("Drag cancelled/ended: {}", drag.display_text());
                }
                self.collection.dragging_track = None;
            }
            Message::DropTracksOnPlaylist { track_ids, target_playlist } => {
                log::info!(
                    "Drop {} track(s) onto playlist {:?}",
                    track_ids.len(),
                    target_playlist
                );
                if let Some(ref mut storage) = self.collection.playlist_storage {
                    let mut success_count = 0;
                    for track_id in &track_ids {
                        match storage.move_track(track_id, &target_playlist) {
                            Ok(new_id) => {
                                log::info!("Track {:?} moved successfully to {:?}", track_id, new_id);
                                success_count += 1;
                            }
                            Err(e) => {
                                log::error!("Failed to move track {:?}: {:?}", track_id, e);
                            }
                        }
                    }
                    if success_count > 0 {
                        log::info!("Moved {}/{} tracks successfully", success_count, track_ids.len());
                        // Refresh tree and both browser track lists
                        self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                        if let Some(ref folder) = self.collection.browser_left.current_folder {
                            self.collection.left_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                        }
                        if let Some(ref folder) = self.collection.browser_right.current_folder {
                            self.collection.right_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                        }
                    }
                }
                self.collection.dragging_track = None;
            }

            // Batch Import
            Message::OpenImport => {
                // If import is already running, just open the modal (don't rescan)
                if self.import_state.phase.is_some() {
                    self.import_state.is_open = true;
                    return Task::none();
                }

                // Not running - reset state and trigger folder scan
                self.import_state = ImportState::default();
                self.import_state.is_open = true;
                return self.update(Message::ScanImportFolder);
            }
            Message::CloseImport => {
                // Just close the modal - DON'T cancel the import!
                // Import continues in background, progress visible via status bar at bottom of screen
                // Only Message::CancelImport (explicit cancel button) should stop the import
                self.import_state.is_open = false;
            }
            Message::ScanImportFolder => {
                self.import_state.phase = Some(ImportPhase::Scanning);
                let import_folder = self.import_state.import_folder.clone();
                return Task::perform(
                    async move {
                        batch_import::scan_and_group_stems(&import_folder)
                            .unwrap_or_else(|e| {
                                log::error!("Failed to scan import folder: {}", e);
                                Vec::new()
                            })
                    },
                    Message::ImportFolderScanned,
                );
            }
            Message::ImportFolderScanned(groups) => {
                log::info!("Import folder scanned: {} groups found", groups.len());
                self.import_state.detected_groups = groups;
                self.import_state.phase = None;
            }
            Message::StartBatchImport => {
                let complete_groups: Vec<_> = self
                    .import_state
                    .detected_groups
                    .iter()
                    .filter(|g| g.is_complete())
                    .cloned()
                    .collect();

                if complete_groups.is_empty() {
                    log::warn!("No complete stem groups to import");
                    return Task::none();
                }

                log::info!("Starting batch import of {} tracks", complete_groups.len());

                // Create channel for progress and atomic flag for cancellation
                let (progress_tx, progress_rx) = std::sync::mpsc::channel();
                let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let cancel_flag_clone = cancel_flag.clone();

                self.import_state.progress_rx = Some(progress_rx);
                self.import_state.cancel_flag = Some(cancel_flag);
                self.import_state.results.clear();

                // Set initial phase
                self.import_state.phase = Some(ImportPhase::Processing {
                    current_track: String::new(),
                    completed: 0,
                    total: complete_groups.len(),
                    start_time: std::time::Instant::now(),
                });

                // Create import config
                let config = ImportConfig {
                    import_folder: self.import_state.import_folder.clone(),
                    collection_path: self.collection.collection.path().to_path_buf(),
                    bpm_config: self.config.analysis.bpm.clone(),
                    loudness_config: self.config.analysis.loudness.clone(),
                    parallel_processes: self.config.analysis.parallel_processes,
                };

                // Spawn delegation thread
                std::thread::spawn(move || {
                    batch_import::run_batch_import(complete_groups, config, progress_tx, cancel_flag_clone);
                });
            }
            Message::ImportProgressUpdate(progress) => {
                match progress {
                    ImportProgress::Started { total } => {
                        log::info!("Import started: {} tracks", total);
                        self.import_state.phase = Some(ImportPhase::Processing {
                            current_track: String::new(),
                            completed: 0,
                            total,
                            start_time: std::time::Instant::now(),
                        });
                    }
                    ImportProgress::TrackStarted { base_name, index, total } => {
                        log::info!("Processing track {}/{}: {}", index + 1, total, base_name);
                        if let Some(ImportPhase::Processing { ref mut current_track, .. }) =
                            self.import_state.phase
                        {
                            *current_track = base_name;
                        }
                    }
                    ImportProgress::TrackCompleted(result) => {
                        log::info!(
                            "Track completed: {} (success={})",
                            result.base_name,
                            result.success
                        );
                        let was_success = result.success;
                        if let Some(ImportPhase::Processing { ref mut completed, .. }) =
                            self.import_state.phase
                        {
                            *completed += 1;
                        }
                        self.import_state.results.push(result);

                        // Refresh collection immediately when track imports successfully
                        // so user sees new tracks appear in browser as they complete
                        if was_success {
                            return Task::perform(async {}, |_| Message::RefreshPlaylists);
                        }
                    }
                    ImportProgress::AllComplete { results } => {
                        log::info!("Import complete: {} tracks processed", results.len());
                        // Calculate duration from start_time if available
                        let duration = if let Some(ImportPhase::Processing { start_time, .. }) =
                            self.import_state.phase
                        {
                            start_time.elapsed()
                        } else {
                            std::time::Duration::ZERO
                        };

                        self.import_state.phase = Some(ImportPhase::Complete { duration });
                        self.import_state.results = results;
                        self.import_state.show_results = true;
                        self.import_state.progress_rx = None;
                        self.import_state.cancel_flag = None;

                        // Refresh collection to show newly imported tracks
                        // Need both: RefreshCollection scans for tracks, RefreshPlaylists updates tree
                        return Task::batch([
                            Task::perform(async {}, |_| Message::RefreshCollection),
                            Task::perform(async {}, |_| Message::RefreshPlaylists),
                        ]);
                    }
                }
            }
            Message::CancelImport => {
                log::info!("Cancelling import");
                if let Some(ref flag) = self.import_state.cancel_flag {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.import_state.phase = None;
                self.import_state.progress_rx = None;
                self.import_state.cancel_flag = None;
            }
            Message::DismissImportResults => {
                self.import_state.phase = None;
                self.import_state.show_results = false;
                self.import_state.is_open = false;
            }

            // USB Export
            Message::OpenExport => {
                log::info!("Opening USB export modal");
                self.export_state.is_open = true;
                self.export_state.reset();
                // Request fresh device list from UsbManager
                self.usb_manager.refresh_devices();
            }
            Message::CloseExport => {
                // Just close the modal - don't cancel export in progress
                self.export_state.is_open = false;
            }
            Message::SelectExportDevice(idx) => {
                self.export_state.selected_device = Some(idx);
                // Invalidate cached sync plan when device changes
                self.export_state.sync_plan = None;
                // If the device isn't mounted yet, request mount
                if let Some(device) = self.export_state.devices.get(idx) {
                    if device.mount_point.is_none() {
                        self.export_state.phase = ExportPhase::Mounting {
                            device_label: device.label.clone(),
                        };
                        self.usb_manager.mount(device.device_path.clone());
                    } else {
                        // Device already mounted, trigger sync plan computation
                        self.trigger_sync_plan_computation();
                    }
                }
            }
            Message::ToggleExportPlaylist(id) => {
                // Use recursive toggle to select/deselect all children
                self.export_state.toggle_playlist_recursive(id, &self.collection.tree_nodes);
                // Invalidate cached sync plan and trigger recomputation
                self.export_state.sync_plan = None;
                self.trigger_sync_plan_computation();
            }
            Message::ToggleExportPlaylistExpand(id) => {
                self.export_state.toggle_playlist_expanded(id);
            }
            Message::ToggleExportConfig => {
                self.export_state.export_config = !self.export_state.export_config;
            }
            Message::BuildSyncPlan => {
                // Legacy handler - sync plan is now computed automatically
                // This triggers a manual recomputation if needed
                self.trigger_sync_plan_computation();
            }
            Message::StartExport => {
                log::info!("Starting USB export");
                if let Some(idx) = self.export_state.selected_device {
                    if let Some(device) = self.export_state.devices.get(idx) {
                        // Use the cached sync plan
                        if let Some(ref plan) = self.export_state.sync_plan {
                            // Use pre-computed LUFS check from sync plan (already computed in background)
                            let tracks_missing_lufs = plan.tracks_missing_lufs.clone();

                            if !tracks_missing_lufs.is_empty() {
                                // Need to analyze LUFS first before export
                                log::info!(
                                    "[LUFS] {} tracks missing LUFS, starting analysis before export",
                                    tracks_missing_lufs.len()
                                );

                                // Use existing reanalysis infrastructure
                                use crate::analysis::AnalysisType;
                                use crate::reanalysis::run_batch_reanalysis;
                                use std::sync::atomic::AtomicBool;
                                use std::sync::mpsc;

                                // Don't start if reanalysis is already running
                                if self.reanalysis_state.is_running {
                                    log::warn!("Re-analysis already in progress, cannot analyze LUFS for export");
                                    return Task::none();
                                }

                                // Set up reanalysis state
                                self.reanalysis_state.is_running = true;
                                self.reanalysis_state.analysis_type = Some(AnalysisType::Loudness);
                                self.reanalysis_state.total_tracks = tracks_missing_lufs.len();
                                self.reanalysis_state.completed_tracks = 0;
                                self.reanalysis_state.succeeded = 0;
                                self.reanalysis_state.failed = 0;
                                self.reanalysis_state.current_track = None;

                                // Mark that export should start after analysis
                                self.export_state.pending_lufs_analysis = true;

                                // Create cancel flag and progress channel
                                let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
                                self.reanalysis_state.cancel_flag = Some(cancel_flag.clone());

                                let (progress_tx, progress_rx) = mpsc::channel();
                                let bpm_config = self.config.analysis.bpm.clone();
                                let loudness_config = self.config.analysis.loudness.clone();
                                let parallel_processes = self.config.analysis.parallel_processes;

                                self.reanalysis_state.progress_rx = Some(progress_rx);

                                // Spawn worker thread for LUFS analysis
                                std::thread::spawn(move || {
                                    run_batch_reanalysis(
                                        tracks_missing_lufs,
                                        AnalysisType::Loudness,
                                        bpm_config,
                                        loudness_config,
                                        parallel_processes,
                                        progress_tx,
                                        cancel_flag,
                                    );
                                });

                                return Task::none();
                            }

                            // No tracks missing LUFS, proceed with export directly
                            let config = if self.export_state.export_config {
                                // Build ExportableConfig from mesh-cue's Config
                                use mesh_core::usb::{
                                    ExportableConfig, ExportableAudioConfig,
                                    ExportableDisplayConfig, ExportableSlicerConfig,
                                };
                                Some(ExportableConfig {
                                    audio: ExportableAudioConfig {
                                        global_bpm: self.config.display.global_bpm,
                                        phase_sync: true, // Default to true for mesh-cue
                                        loudness: self.config.analysis.loudness.clone(),
                                    },
                                    display: ExportableDisplayConfig {
                                        default_loop_length_index: self.config.display.default_loop_length_index,
                                        default_zoom_bars: self.config.display.zoom_bars,
                                        grid_bars: self.config.display.grid_bars,
                                        stem_color_palette: "natural".to_string(),
                                    },
                                    slicer: ExportableSlicerConfig {
                                        buffer_bars: self.config.slicer.validated_buffer_bars(),
                                        presets: Vec::new(), // Presets handled separately
                                    },
                                })
                            } else {
                                None
                            };
                            let _ = self.usb_manager.send(mesh_core::usb::UsbCommand::StartExport {
                                device_path: device.device_path.clone(),
                                plan: plan.clone(),
                                include_config: self.export_state.export_config,
                                config,
                            });
                        }
                    }
                }
            }
            Message::CancelExport => {
                log::info!("Cancelling USB export");
                let _ = self.usb_manager.send(mesh_core::usb::UsbCommand::CancelExport);
                self.export_state.phase = ExportPhase::SelectDevice;
            }
            Message::UsbMessage(usb_msg) => {
                // Handle USB manager messages
                use mesh_core::usb::UsbMessage as UsbMsg;
                match usb_msg {
                    UsbMsg::DevicesRefreshed(devices) => {
                        self.export_state.devices = devices;
                        // Auto-select first device if none selected and devices available
                        if self.export_state.selected_device.is_none()
                            && !self.export_state.devices.is_empty()
                        {
                            self.export_state.selected_device = Some(0);
                        }
                    }
                    UsbMsg::DeviceConnected(device) => {
                        log::info!("USB device connected: {}", device.label);
                        self.export_state.devices.push(device);
                    }
                    UsbMsg::DeviceDisconnected { device_path } => {
                        log::info!("USB device disconnected: {:?}", device_path);
                        self.export_state.devices.retain(|d| d.device_path != device_path);
                        // Clear selection if the disconnected device was selected
                        if let Some(idx) = self.export_state.selected_device {
                            if self.export_state.devices.get(idx).map(|d| &d.device_path) == Some(&device_path) {
                                self.export_state.selected_device = None;
                            }
                        }
                    }
                    UsbMsg::MountComplete { result } => {
                        match result {
                            Ok(dev) => {
                                log::info!("Device mounted at {:?}", dev.mount_point);
                                // Update device in list
                                if let Some(existing) = self.export_state.devices.iter_mut()
                                    .find(|d| d.device_path == dev.device_path)
                                {
                                    *existing = dev;
                                }
                                // Stay in SelectDevice phase, trigger sync plan computation
                                self.export_state.phase = ExportPhase::SelectDevice;
                                self.trigger_sync_plan_computation();
                            }
                            Err(e) => {
                                log::error!("Mount failed: {}", e);
                                self.export_state.phase = ExportPhase::Error(e.to_string());
                            }
                        }
                    }
                    UsbMsg::SyncPlanProgress { files_scanned: _, total_files: _ } => {
                        // Sync plan computation in progress (background, don't change phase)
                        self.export_state.sync_plan_computing = true;
                    }
                    UsbMsg::SyncPlanReady(plan) => {
                        // Store the computed plan (don't change phase - stay in SelectDevice)
                        self.export_state.sync_plan = Some(plan);
                        self.export_state.sync_plan_computing = false;
                    }
                    UsbMsg::ExportStarted { total_files, total_bytes } => {
                        self.export_state.phase = ExportPhase::Exporting {
                            current_file: String::new(),
                            files_complete: 0,
                            bytes_complete: 0,
                            total_files,
                            total_bytes,
                            start_time: std::time::Instant::now(),
                        };
                    }
                    UsbMsg::ExportProgress {
                        current_file,
                        files_complete,
                        bytes_complete,
                        total_files,
                        total_bytes,
                    } => {
                        if let ExportPhase::Exporting { start_time, .. } = &self.export_state.phase {
                            let start = *start_time;
                            self.export_state.phase = ExportPhase::Exporting {
                                current_file,
                                files_complete,
                                bytes_complete,
                                total_files,
                                total_bytes,
                                start_time: start,
                            };
                        }
                    }
                    UsbMsg::ExportComplete { duration, files_exported, failed_files } => {
                        self.export_state.phase = ExportPhase::Complete {
                            duration,
                            files_exported,
                            failed_files,
                        };
                        self.export_state.show_results = true;
                        // Re-open modal to show completion results (even if user closed it during export)
                        self.export_state.is_open = true;
                    }
                    UsbMsg::ExportError(err) => {
                        self.export_state.phase = ExportPhase::Error(err.to_string());
                        // Re-open modal to show error (even if user closed it during export)
                        self.export_state.is_open = true;
                    }
                    UsbMsg::ExportCancelled => {
                        self.export_state.phase = ExportPhase::SelectDevice;
                    }
                    _ => {
                        // Handle other messages as needed
                    }
                }
            }
            Message::DismissExportResults => {
                self.export_state.phase = ExportPhase::SelectDevice;
                self.export_state.show_results = false;
                self.export_state.is_open = false;
            }

            // Delete confirmation
            Message::RequestDelete(browser_side) => {
                use super::delete_modal::DeleteTarget;

                // Get selected tracks from the appropriate browser
                let (selected_ids, current_folder) = match browser_side {
                    BrowserSide::Left => (
                        self.collection.browser_left.table_state.selected.iter().cloned().collect::<Vec<_>>(),
                        self.collection.browser_left.current_folder.clone(),
                    ),
                    BrowserSide::Right => (
                        self.collection.browser_right.table_state.selected.iter().cloned().collect::<Vec<_>>(),
                        self.collection.browser_right.current_folder.clone(),
                    ),
                };

                if selected_ids.is_empty() {
                    log::debug!("Delete requested but no tracks selected");
                    return Task::none();
                }

                // Get track names from storage
                let track_names: Vec<String> = if let Some(ref storage) = self.collection.playlist_storage {
                    selected_ids
                        .iter()
                        .filter_map(|id| storage.get_node(id).map(|n| n.name.clone()))
                        .collect()
                } else {
                    return Task::none();
                };

                // Determine delete target based on current folder
                // If in the collection root (tracks folder), it's a permanent delete
                // If in a playlist, it's just removing from playlist
                let target = if current_folder == Some(NodeId::tracks()) {
                    // In collection - permanent deletion!
                    DeleteTarget::CollectionTracks {
                        track_names,
                        track_ids: selected_ids,
                    }
                } else if let Some(folder_id) = current_folder {
                    // In a playlist - just remove from playlist
                    let playlist_name = self
                        .collection
                        .playlist_storage
                        .as_ref()
                        .and_then(|s| s.get_node(&folder_id))
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| folder_id.to_string());
                    DeleteTarget::PlaylistTracks {
                        playlist_name,
                        track_ids: selected_ids,
                        track_names,
                    }
                } else {
                    log::debug!("Delete requested but no folder selected");
                    return Task::none();
                };

                log::info!("Showing delete confirmation for {:?}", target);
                self.delete_state.show(target);
            }
            Message::CancelDelete => {
                self.delete_state.cancel();
            }
            Message::ConfirmDelete => {
                use super::delete_modal::DeleteTarget;

                if let Some(ref target) = self.delete_state.target {
                    log::info!("Executing delete: {:?}", target);

                    match target {
                        DeleteTarget::PlaylistTracks { track_ids, .. } => {
                            // Remove tracks from playlist (not from collection)
                            if let Some(ref mut storage) = self.collection.playlist_storage {
                                for track_id in track_ids {
                                    if let Err(e) = storage.remove_track_from_playlist(track_id) {
                                        log::error!("Failed to remove track from playlist: {:?}", e);
                                    }
                                }
                                // Refresh displays
                                self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                if let Some(ref folder) = self.collection.browser_left.current_folder {
                                    self.collection.left_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                }
                                if let Some(ref folder) = self.collection.browser_right.current_folder {
                                    self.collection.right_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                }
                            }
                        }
                        DeleteTarget::CollectionTracks { track_ids, .. } => {
                            // PERMANENT deletion - delete files from disk!
                            if let Some(ref mut storage) = self.collection.playlist_storage {
                                for track_id in track_ids {
                                    if let Err(e) = storage.delete_track_permanently(track_id) {
                                        log::error!("Failed to delete track permanently: {:?}", e);
                                    }
                                }
                                // Refresh displays
                                self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                                if let Some(ref folder) = self.collection.browser_left.current_folder {
                                    self.collection.left_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                }
                                if let Some(ref folder) = self.collection.browser_right.current_folder {
                                    self.collection.right_tracks = get_tracks_for_folder(storage.as_ref(), folder);
                                }
                            }
                        }
                        DeleteTarget::Playlist { playlist_id, .. } => {
                            // Delete playlist (tracks stay in collection)
                            if let Some(ref mut storage) = self.collection.playlist_storage {
                                if let Err(e) = storage.delete_playlist(playlist_id) {
                                    log::error!("Failed to delete playlist: {:?}", e);
                                }
                                self.collection.tree_nodes = build_tree_nodes(storage.as_ref());
                            }
                        }
                    }

                    // Clear selection after delete
                    self.collection.browser_left.table_state.clear_selection();
                    self.collection.browser_right.table_state.clear_selection();
                }

                self.delete_state.complete();
            }

            // Context menu and track operations
            Message::RequestDeleteById(track_id) => {
                use super::delete_modal::DeleteTarget;

                self.context_menu_state.close();

                // Determine if track is in collection or playlist
                if track_id.is_in_tracks() {
                    // Collection track - permanent deletion
                    let track_name = self
                        .collection
                        .playlist_storage
                        .as_ref()
                        .and_then(|s| s.get_node(&track_id))
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| track_id.name().to_string());
                    self.delete_state.show(DeleteTarget::CollectionTracks {
                        track_names: vec![track_name],
                        track_ids: vec![track_id],
                    });
                } else {
                    // Playlist track - just remove from playlist
                    let track_name = self
                        .collection
                        .playlist_storage
                        .as_ref()
                        .and_then(|s| s.get_node(&track_id))
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| track_id.name().to_string());
                    let playlist_name = track_id
                        .parent()
                        .and_then(|p| {
                            self.collection
                                .playlist_storage
                                .as_ref()
                                .and_then(|s| s.get_node(&p))
                                .map(|n| n.name.clone())
                        })
                        .unwrap_or_default();
                    self.delete_state.show(DeleteTarget::PlaylistTracks {
                        playlist_name,
                        track_names: vec![track_name],
                        track_ids: vec![track_id],
                    });
                }
            }
            Message::RequestDeletePlaylist(playlist_id) => {
                use super::delete_modal::DeleteTarget;

                self.context_menu_state.close();

                let playlist_name = self
                    .collection
                    .playlist_storage
                    .as_ref()
                    .and_then(|s| s.get_node(&playlist_id))
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| playlist_id.name().to_string());

                self.delete_state.show(DeleteTarget::Playlist {
                    playlist_name,
                    playlist_id,
                });
            }
            Message::ShowContextMenu(kind, position) => {
                log::info!("[CONTEXT MENU] ShowContextMenu called: position={:?}, is_open will be: true", position);
                self.context_menu_state.show(kind, position);
                log::info!("[CONTEXT MENU] After show: is_open={}, position={:?}", self.context_menu_state.is_open, self.context_menu_state.position);
            }
            Message::CloseContextMenu => {
                self.context_menu_state.close();
            }
            Message::StartReanalysis { analysis_type, scope } => {
                use crate::analysis::ReanalysisScope;
                use crate::reanalysis::run_batch_reanalysis;
                use std::sync::atomic::AtomicBool;
                use std::sync::mpsc;

                self.context_menu_state.close();

                // Don't start if already running
                if self.reanalysis_state.is_running {
                    log::warn!("Re-analysis already in progress, ignoring request");
                    return Task::none();
                }

                // Resolve scope to list of file paths
                let tracks: Vec<PathBuf> = match &scope {
                    ReanalysisScope::SingleTrack(track_id) => {
                        self.collection
                            .playlist_storage
                            .as_ref()
                            .and_then(|s| s.get_node(track_id))
                            .and_then(|n| n.track_path.clone())
                            .map(|p| vec![p])
                            .unwrap_or_default()
                    }
                    ReanalysisScope::SelectedTracks(track_ids) => {
                        track_ids
                            .iter()
                            .filter_map(|id| {
                                self.collection
                                    .playlist_storage
                                    .as_ref()
                                    .and_then(|s| s.get_node(id))
                                    .and_then(|n| n.track_path.clone())
                            })
                            .collect()
                    }
                    ReanalysisScope::PlaylistFolder(playlist_id) => {
                        // Get all tracks in the playlist
                        self.collection
                            .playlist_storage
                            .as_ref()
                            .map(|s| {
                                s.get_children(playlist_id)
                                    .into_iter()
                                    .filter_map(|node| node.track_path)
                                    .collect()
                            })
                            .unwrap_or_default()
                    }
                    ReanalysisScope::EntireCollection => {
                        // Get all tracks from collection
                        self.collection
                            .collection
                            .tracks()
                            .iter()
                            .map(|t| t.path.clone())
                            .collect()
                    }
                };

                if tracks.is_empty() {
                    log::warn!("No tracks to re-analyze");
                    return Task::none();
                }

                log::info!(
                    "Starting {} re-analysis for {} tracks",
                    analysis_type.display_name(),
                    tracks.len()
                );

                // Set up state
                self.reanalysis_state.is_running = true;
                self.reanalysis_state.analysis_type = Some(analysis_type);
                self.reanalysis_state.total_tracks = tracks.len();
                self.reanalysis_state.completed_tracks = 0;
                self.reanalysis_state.succeeded = 0;
                self.reanalysis_state.failed = 0;
                self.reanalysis_state.current_track = None;

                // Create cancel flag and progress channel
                let cancel_flag = Arc::new(AtomicBool::new(false));
                self.reanalysis_state.cancel_flag = Some(cancel_flag.clone());

                let (progress_tx, progress_rx) = mpsc::channel();
                let bpm_config = self.config.analysis.bpm.clone();
                let loudness_config = self.config.analysis.loudness.clone();
                let parallel_processes = self.config.analysis.parallel_processes;

                // Store receiver for polling in Tick handler (same pattern as import)
                self.reanalysis_state.progress_rx = Some(progress_rx);

                // Spawn worker thread
                std::thread::spawn(move || {
                    run_batch_reanalysis(
                        tracks,
                        analysis_type,
                        bpm_config,
                        loudness_config,
                        parallel_processes,
                        progress_tx,
                        cancel_flag,
                    );
                });
            }
            Message::ReanalysisProgress(progress) => {
                use crate::analysis::ReanalysisProgress;

                match progress {
                    ReanalysisProgress::Started { total_tracks, analysis_type } => {
                        self.reanalysis_state.total_tracks = total_tracks;
                        self.reanalysis_state.analysis_type = Some(analysis_type);
                    }
                    ReanalysisProgress::TrackStarted { track_name, .. } => {
                        // Only update the display name, not the counter
                        // (counter is updated by TrackCompleted)
                        self.reanalysis_state.current_track = Some(track_name);
                    }
                    ReanalysisProgress::TrackCompleted { success, .. } => {
                        if success {
                            self.reanalysis_state.succeeded += 1;
                        } else {
                            self.reanalysis_state.failed += 1;
                        }
                        self.reanalysis_state.completed_tracks += 1;
                    }
                    ReanalysisProgress::AllComplete { succeeded, failed, .. } => {
                        self.reanalysis_state.is_running = false;
                        self.reanalysis_state.succeeded = succeeded;
                        self.reanalysis_state.failed = failed;
                        self.reanalysis_state.current_track = None;
                        self.reanalysis_state.cancel_flag = None;
                        self.reanalysis_state.progress_rx = None;

                        log::info!(
                            "Re-analysis complete: {} succeeded, {} failed",
                            succeeded,
                            failed
                        );

                        // Check if export was pending LUFS analysis
                        if self.export_state.pending_lufs_analysis {
                            self.export_state.pending_lufs_analysis = false;
                            log::info!("[LUFS] LUFS analysis complete, now starting USB export");

                            // Directly trigger export (can't return Task from here when called via Tick)
                            self.trigger_usb_export_after_lufs();
                        }

                        // Refresh collection to show updated metadata
                        return Task::perform(async {}, |_| Message::RefreshCollection);
                    }
                }
            }
            Message::CancelReanalysis => {
                if let Some(ref flag) = self.reanalysis_state.cancel_flag {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    log::info!("Re-analysis cancellation requested");
                }
            }
            Message::StartRenamePlaylist(playlist_id) => {
                self.context_menu_state.close();

                // Start inline rename in the appropriate tree
                if let Some(ref storage) = self.collection.playlist_storage {
                    if let Some(node) = storage.get_node(&playlist_id) {
                        // Try to find which browser has this playlist and start edit
                        if self
                            .collection
                            .browser_left
                            .tree_state
                            .is_expanded(&playlist_id.parent().unwrap_or_else(NodeId::playlists))
                        {
                            self.collection
                                .browser_left
                                .tree_state
                                .start_edit(playlist_id, node.name.clone());
                        } else {
                            self.collection
                                .browser_right
                                .tree_state
                                .start_edit(playlist_id, node.name.clone());
                        }
                    }
                }
            }
        }

        Task::none()
    }

    /// Render the UI
    pub fn view(&self) -> Element<'_, Message> {
        let header = self.view_header();

        let content: Element<Message> = match self.current_view {
            View::Collection => self.view_collection(),
        };

        // Global status bars at bottom (visible when import, export, or re-analysis is active)
        let import_bar = super::import_modal::view_progress_bar(&self.import_state);
        let export_bar = super::export_modal::view_progress_bar(&self.export_state);
        let reanalysis_bar = self.view_reanalysis_progress_bar();

        let mut main = column![header, content].spacing(10);
        if let Some(bar) = import_bar {
            main = main.push(bar);
        }
        if let Some(bar) = export_bar {
            main = main.push(bar);
        }
        if let Some(bar) = reanalysis_bar {
            main = main.push(bar);
        }

        let base: Element<Message> = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .into();

        // Overlay modals if open (export > import > delete > settings)
        if self.export_state.is_open {
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::CloseExport);

            // Extract the playlists subtree for the export modal
            fn find_playlists_tree(nodes: &[mesh_widgets::TreeNode<mesh_core::playlist::NodeId>]) -> Vec<mesh_widgets::TreeNode<mesh_core::playlist::NodeId>> {
                for node in nodes {
                    if node.id.0 == "playlists" {
                        // Return the children of the playlists root
                        return node.children.clone();
                    }
                    // Recurse into children
                    let result = find_playlists_tree(&node.children);
                    if !result.is_empty() {
                        return result;
                    }
                }
                Vec::new()
            }
            let playlist_tree = find_playlists_tree(&self.collection.tree_nodes);

            let modal = center(opaque(super::export_modal::view(&self.export_state, playlist_tree)))
                .width(Length::Fill)
                .height(Length::Fill);

            stack![base, backdrop, modal].into()
        } else if self.import_state.is_open {
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::CloseImport);

            let modal = center(opaque(super::import_modal::view(&self.import_state)))
                .width(Length::Fill)
                .height(Length::Fill);

            stack![base, backdrop, modal].into()
        } else if self.delete_state.is_open {
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::CancelDelete);

            let modal = center(opaque(super::delete_modal::view(&self.delete_state)))
                .width(Length::Fill)
                .height(Length::Fill);

            stack![base, backdrop, modal].into()
        } else if self.settings.is_open {
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.6).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::CloseSettings);

            let modal = center(opaque(super::settings::view(&self.settings)))
                .width(Length::Fill)
                .height(Length::Fill);

            stack![base, backdrop, modal].into()
        } else if self.context_menu_state.is_open {
            // Context menu overlay - transparent backdrop + positioned menu
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .on_press(Message::CloseContextMenu);

            if let Some(menu) = super::context_menu::view(&self.context_menu_state) {
                // Position the menu at the click location using spacers
                let pos = self.context_menu_state.position;
                let positioned_menu = column![
                    Space::new().height(Length::Fixed(pos.y)),
                    row![
                        Space::new().width(Length::Fixed(pos.x)),
                        menu,
                    ]
                ];

                stack![base, backdrop, positioned_menu].into()
            } else {
                base
            }
        } else {
            base
        }
    }

    /// Application theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    /// Subscription for periodic UI updates and keyboard/mouse events
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::{event, keyboard, mouse, time, Event};
        use std::time::Duration;

        // Keyboard events for keybindings and modifier tracking
        let keyboard_sub = keyboard::listen().map(|event| {
            match event {
                keyboard::Event::KeyPressed { key, modifiers, repeat, .. } => {
                    Message::KeyPressed(key, modifiers, repeat)
                }
                keyboard::Event::KeyReleased { key, modifiers, .. } => {
                    Message::KeyReleased(key, modifiers)
                }
                keyboard::Event::ModifiersChanged(modifiers) => {
                    Message::ModifiersChanged(modifiers)
                }
            }
        });

        // Window events for tracking global mouse position (used for context menu placement)
        let mouse_sub = event::listen().map(|event| {
            match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Message::GlobalMouseMoved(position)
                }
                _ => Message::Tick, // Ignore other events
            }
        });

        // Linked stem result subscription (engine owns the loader, we receive results)
        let linked_stem_sub = if let Some(receiver) = self.audio.linked_stem_receiver() {
            mpsc_subscription(receiver)
                .map(|result| Message::LinkedStemLoaded(LinkedStemLoadedMsg(Arc::new(result))))
        } else {
            iced::Subscription::none()
        };

        // USB manager subscription (event-driven device detection and export progress)
        let usb_sub = mpsc_subscription(self.usb_manager.message_receiver())
            .map(Message::UsbMessage);

        // Always run tick at 60fps for smooth waveform animation
        // This matches mesh-player's approach and ensures cueing/preview states work correctly
        iced::Subscription::batch([
            keyboard_sub,
            mouse_sub,
            time::every(Duration::from_millis(16)).map(|_| Message::Tick),
            linked_stem_sub,
            usb_sub,
        ])
    }

    /// Trigger USB export after LUFS analysis completes
    ///
    /// This is called from AllComplete handler when pending_lufs_analysis was set.
    /// It directly sends the export command without going through the message system.
    fn trigger_usb_export_after_lufs(&mut self) {
        if let Some(idx) = self.export_state.selected_device {
            if let Some(device) = self.export_state.devices.get(idx) {
                if let Some(ref plan) = self.export_state.sync_plan {
                    let config = if self.export_state.export_config {
                        use mesh_core::usb::{
                            ExportableConfig, ExportableAudioConfig,
                            ExportableDisplayConfig, ExportableSlicerConfig,
                        };
                        Some(ExportableConfig {
                            audio: ExportableAudioConfig {
                                global_bpm: self.config.display.global_bpm,
                                phase_sync: true,
                                loudness: self.config.analysis.loudness.clone(),
                            },
                            display: ExportableDisplayConfig {
                                default_loop_length_index: self.config.display.default_loop_length_index,
                                default_zoom_bars: self.config.display.zoom_bars,
                                grid_bars: self.config.display.grid_bars,
                                stem_color_palette: "natural".to_string(),
                            },
                            slicer: ExportableSlicerConfig {
                                buffer_bars: self.config.slicer.validated_buffer_bars(),
                                presets: Vec::new(),
                            },
                        })
                    } else {
                        None
                    };
                    let _ = self.usb_manager.send(mesh_core::usb::UsbCommand::StartExport {
                        device_path: device.device_path.clone(),
                        plan: plan.clone(),
                        include_config: self.export_state.export_config,
                        config,
                    });
                }
            }
        }
    }

    /// Trigger background sync plan computation for USB export
    ///
    /// This is called automatically when device or playlist selection changes.
    /// The sync plan is computed in the background and stored in export_state.sync_plan.
    fn trigger_sync_plan_computation(&mut self) {
        // Only compute if we have a device selected and playlists selected
        if self.export_state.selected_playlists.is_empty() {
            self.export_state.sync_plan = None;
            self.export_state.sync_plan_computing = false;
            return;
        }

        if let Some(idx) = self.export_state.selected_device {
            if let Some(device) = self.export_state.devices.get(idx) {
                // Only compute if device is mounted
                if device.mount_point.is_some() {
                    let playlists: Vec<_> = self.export_state.selected_playlists.iter().cloned().collect();
                    let collection_root = self.collection.collection.path().to_path_buf();
                    self.export_state.sync_plan_computing = true;
                    let _ = self.usb_manager.send(mesh_core::usb::UsbCommand::BuildSyncPlan {
                        device_path: device.device_path.clone(),
                        playlists,
                        local_collection_root: collection_root,
                    });
                }
            }
        }
    }

    /// Render a progress bar for re-analysis operations
    fn view_reanalysis_progress_bar(&self) -> Option<Element<'_, Message>> {
        if !self.reanalysis_state.is_running {
            return None;
        }

        let progress = if self.reanalysis_state.total_tracks > 0 {
            self.reanalysis_state.completed_tracks as f32 / self.reanalysis_state.total_tracks as f32
        } else {
            0.0
        };

        let analysis_name = self.reanalysis_state.analysis_type
            .map(|t| t.display_name())
            .unwrap_or("Analysis");

        let track_info = self.reanalysis_state.current_track
            .as_ref()
            .map(|name| {
                if name.len() > 40 {
                    format!("{}...", &name[..37])
                } else {
                    name.clone()
                }
            })
            .unwrap_or_default();

        let label = format!("Re-analysing {}: {}", analysis_name, track_info);
        let progress_text = format!(
            "{}/{}",
            self.reanalysis_state.completed_tracks,
            self.reanalysis_state.total_tracks
        );

        Some(super::import_modal::build_status_bar(
            label,
            progress_text,
            progress,
            Message::CancelReanalysis,
        ))
    }

    /// View header with app title and settings
    fn view_header(&self) -> Element<'_, Message> {
        // Settings gear icon (⚙ U+2699)
        let settings_btn = button(text("⚙").size(20))
            .on_press(Message::OpenSettings)
            .style(button::secondary);

        row![
            text("mesh-cue").size(24),
            Space::new().width(Length::Fill),
            settings_btn,
        ]
        .spacing(10)
        .into()
    }

    /// View for the collection browser and editor
    fn view_collection(&self) -> Element<'_, Message> {
        // Modifier key handling is done in update() where current keyboard state is available
        super::collection_browser::view(&self.collection, &self.import_state, self.stem_link_selection)
    }
}

// Helper functions (nudge_beat_grid, regenerate_beat_grid, etc.) moved to utils/ module
