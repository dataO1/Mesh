//! Main application state and iced implementation

use crate::analysis::AnalysisResult;
use crate::audio::{AudioState, JackHandle, start_jack_client};
use crate::collection::Collection;
use crate::config::{self, Config};
use crate::export;
use crate::import::StemImporter;
use crate::keybindings::{self, KeybindingsConfig};
use super::waveform::{CombinedWaveformView, WaveformView, ZoomedWaveformView};
use iced::widget::{button, center, column, container, mouse_area, opaque, row, stack, text, Space};
use iced::{Color, Element, Length, Task, Theme};
use basedrop::Shared;
use mesh_core::audio_file::{BeatGrid, CuePoint, LoadedTrack, StemBuffers, TrackMetadata};
use mesh_core::engine::{Deck, PreparedTrack};
use mesh_core::types::{DeckId, PlayState};
use std::path::PathBuf;
use std::sync::Arc;

/// Current view in the application
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Staging area for importing and analyzing stems
    #[default]
    Staging,
    /// Collection browser and track editor
    Collection,
}

/// State for the staging (import) view
#[derive(Debug, Default)]
pub struct StagingState {
    /// Stem importer with loaded file paths
    pub importer: StemImporter,
    /// Loaded stem buffers (after import)
    pub stem_buffers: Option<StemBuffers>,
    /// Analysis result
    pub analysis_result: Option<AnalysisResult>,
    /// Analysis progress (0.0 - 1.0)
    pub analysis_progress: Option<f32>,
    /// Track name for export
    pub track_name: String,
    /// Status message
    pub status: String,
}

/// State for the collection view
#[derive(Debug)]
pub struct CollectionState {
    /// Collection manager
    pub collection: Collection,
    /// Currently selected track index
    pub selected_track: Option<usize>,
    /// Currently loaded track for editing
    pub loaded_track: Option<LoadedTrackState>,
}

impl Default for CollectionState {
    fn default() -> Self {
        Self {
            collection: Collection::default(),
            selected_track: None,
            loaded_track: None,
        }
    }
}

/// State for a loaded track being edited
///
/// Note: Manual Debug impl because Deck doesn't implement Debug
pub struct LoadedTrackState {
    /// Path to the track file
    pub path: PathBuf,
    /// Loaded audio data (wrapped in Arc for efficient cloning in messages)
    /// None while audio is loading asynchronously
    pub track: Option<Arc<LoadedTrack>>,
    /// Loaded stems (Shared for RT-safe deallocation)
    pub stems: Option<Shared<StemBuffers>>,
    /// Current cue points (may be modified)
    pub cue_points: Vec<CuePoint>,
    /// Saved loops (up to 8 loop slots)
    pub saved_loops: Vec<mesh_core::audio_file::SavedLoop>,
    /// Modified BPM (user override)
    pub bpm: f64,
    /// Modified key (user override)
    pub key: String,
    /// Beat grid from metadata
    pub beat_grid: Vec<u64>,
    /// Duration in samples (from metadata or computed)
    pub duration_samples: u64,
    /// Whether there are unsaved changes
    pub modified: bool,
    /// Combined waveform display (both zoomed detail and full overview in one canvas)
    /// This works around iced bug #3040 where multiple Canvas widgets don't render properly
    pub combined_waveform: CombinedWaveformView,
    /// Whether audio is currently loading in the background
    pub loading_audio: bool,
    /// Player deck for transport and hot cue state (created when stems load)
    /// Uses Deck's state for: is_playing, position, beat_jump_size, cue_point, hot_cue_preview
    pub deck: Option<Deck>,
    /// Last time the playhead position was updated (for smooth interpolation)
    pub last_playhead_update: std::time::Instant,
}

impl LoadedTrackState {
    /// Get current playhead position (from deck if loaded, otherwise 0)
    pub fn playhead_position(&self) -> u64 {
        self.deck.as_ref().map(|d| d.position()).unwrap_or(0)
    }

    /// Get interpolated playhead position for smooth waveform rendering
    ///
    /// When playing, this estimates the current position based on elapsed time
    /// since the last update. This eliminates visible "chunking" in waveform
    /// movement caused by the UI polling rate (16ms) being different from
    /// the audio buffer rate (5.8ms).
    pub fn interpolated_playhead_position(&self) -> u64 {
        let base_position = self.playhead_position();

        // Only interpolate when playing
        if !self.is_playing() {
            return base_position;
        }

        // Calculate samples elapsed since last update
        let elapsed = self.last_playhead_update.elapsed();
        let samples_elapsed = (elapsed.as_secs_f64() * mesh_core::types::SAMPLE_RATE as f64) as u64;

        // Return interpolated position (clamped to duration)
        base_position.saturating_add(samples_elapsed).min(self.duration_samples)
    }

    /// Update the playhead timestamp (call this when position is updated from audio thread)
    pub fn touch_playhead(&mut self) {
        self.last_playhead_update = std::time::Instant::now();
    }

    /// Check if audio is currently playing
    pub fn is_playing(&self) -> bool {
        self.deck
            .as_ref()
            .map(|d| d.state() == PlayState::Playing)
            .unwrap_or(false)
    }

    /// Get beat jump size (from deck if loaded, default 4)
    pub fn beat_jump_size(&self) -> i32 {
        self.deck.as_ref().map(|d| d.beat_jump_size()).unwrap_or(4)
    }

    /// Update zoomed waveform cache if needed for new playhead position
    ///
    /// Call this after any operation that changes the playhead position
    /// (Seek, Stop, BeatJump, JumpToCue, etc.) to ensure the zoomed
    /// waveform displays correctly.
    pub fn update_zoomed_waveform_cache(&mut self, playhead: u64) {
        if self.combined_waveform.zoomed.needs_recompute(playhead) {
            if let Some(ref stems) = self.stems {
                self.combined_waveform.zoomed.compute_peaks(stems, playhead, 1600);
            }
        }
    }
}

impl std::fmt::Debug for LoadedTrackState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTrackState")
            .field("path", &self.path)
            .field("cue_points", &self.cue_points)
            .field("bpm", &self.bpm)
            .field("key", &self.key)
            .field("duration_samples", &self.duration_samples)
            .field("modified", &self.modified)
            .field("loading_audio", &self.loading_audio)
            .field("has_deck", &self.deck.is_some())
            .finish_non_exhaustive()
    }
}

/// State for the settings modal
#[derive(Debug, Default)]
pub struct SettingsState {
    /// Whether the settings modal is open
    pub is_open: bool,
    /// Draft min tempo value (text input)
    pub draft_min_tempo: String,
    /// Draft max tempo value (text input)
    pub draft_max_tempo: String,
    /// Draft track name format template
    pub draft_track_name_format: String,
    /// Draft grid bars value (4, 8, 16, 32)
    pub draft_grid_bars: u32,
    /// Status message for save feedback
    pub status: String,
}

impl SettingsState {
    /// Initialize from current config
    pub fn from_config(config: &Config) -> Self {
        Self {
            is_open: false,
            draft_min_tempo: config.analysis.bpm.min_tempo.to_string(),
            draft_max_tempo: config.analysis.bpm.max_tempo.to_string(),
            draft_track_name_format: config.track_name_format.clone(),
            draft_grid_bars: config.display.grid_bars,
            status: String::new(),
        }
    }
}

/// Wrapper for stems load result - provides Debug impl for Shared<StemBuffers>
///
/// basedrop::Shared doesn't implement Debug, so we need this wrapper
/// for the Message enum to derive Debug.
#[derive(Clone)]
pub struct StemsLoadResult(pub Result<Shared<StemBuffers>, String>);

impl std::fmt::Debug for StemsLoadResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Ok(stems) => write!(f, "StemsLoadResult(Ok(<{} frames>))", stems.len()),
            Err(e) => write!(f, "StemsLoadResult(Err({}))", e),
        }
    }
}

/// Application messages
#[derive(Debug, Clone)]
pub enum Message {
    // Navigation
    SwitchView(View),

    // Staging: Import
    SelectStemFile(usize), // 0=vocals, 1=drums, 2=bass, 3=other
    StemFileSelected(usize, Option<PathBuf>),
    SetTrackName(String),

    // Staging: Analysis
    StartAnalysis,
    AnalysisProgress(f32),
    AnalysisComplete(AnalysisResult),
    AnalysisError(String),

    // Staging: Export
    AddToCollection,
    AddToCollectionComplete(Result<PathBuf, String>),

    // Collection: Browser
    RefreshCollection,
    SelectTrack(usize),
    LoadTrack(usize),
    /// Phase 1: Metadata loaded (fast), now show UI
    TrackMetadataLoaded(Result<(PathBuf, TrackMetadata), String>),
    /// Phase 2: Audio stems loaded (slow), now enable playback (Shared for RT-safe drop)
    TrackStemsLoaded(StemsLoadResult),
    /// Legacy: full track loaded (kept for compatibility)
    TrackLoaded(Result<Arc<LoadedTrack>, String>),

    // Collection: Editor
    SetBpm(f64),
    SetKey(String),
    AddCuePoint(u64),
    DeleteCuePoint(usize),
    SetCueLabel(usize, String),
    SaveTrack,
    SaveComplete(Result<(), String>),

    // Transport
    Play,
    Pause,
    Stop,
    Seek(f64),
    /// CDJ-style cue button pressed (set cue point, start preview)
    Cue,
    /// CDJ-style cue button released (stop preview, return to cue point)
    CueReleased,
    /// Beat jump by N beats (positive = forward, negative = backward)
    BeatJump(i32),
    /// Set beat jump size (1, 4, 8, 16, 32)
    SetBeatJumpSize(i32),
    /// Set overview waveform grid density (4, 8, 16, 32 bars)
    SetOverviewGridBars(u32),

    // Hot Cues (8 action buttons)
    /// Jump to hot cue at index (0-7)
    JumpToCue(usize),
    /// Set hot cue at index to current playhead position
    SetCuePoint(usize),
    /// Clear hot cue at index (Shift+click)
    ClearCuePoint(usize),
    /// Hot cue button pressed - start preview from this cue point (CDJ-style)
    HotCuePressed(usize),
    /// Hot cue button released - stop preview and return to cue point
    HotCueReleased(usize),

    // Zoomed Waveform
    /// Set zoom level for zoomed waveform (1-64 bars)
    SetZoomBars(u32),

    // Misc
    Tick,

    // Beat Grid
    /// Nudge beat grid left (earlier) by small increment
    NudgeBeatGridLeft,
    /// Nudge beat grid right (later) by small increment
    NudgeBeatGridRight,

    // Settings
    OpenSettings,
    CloseSettings,
    UpdateSettingsMinTempo(String),
    UpdateSettingsMaxTempo(String),
    UpdateSettingsTrackNameFormat(String),
    UpdateSettingsGridBars(u32),
    SaveSettings,
    SaveSettingsComplete(Result<(), String>),

    // Keyboard
    /// Key pressed with modifiers (for keybindings and shift tracking)
    /// The bool indicates if this is a repeat event (key held down)
    KeyPressed(iced::keyboard::Key, iced::keyboard::Modifiers, bool),
    /// Key released (for hot cue preview release)
    KeyReleased(iced::keyboard::Key, iced::keyboard::Modifiers),
}

/// Main application
pub struct MeshCueApp {
    /// Current view
    current_view: View,
    /// Staging state
    staging: StagingState,
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
    /// Keybindings configuration
    keybindings: KeybindingsConfig,
    /// Hot cue keys currently pressed (for filtering key repeat)
    pressed_hot_cue_keys: std::collections::HashSet<usize>,
    /// Main cue key currently pressed
    pressed_cue_key: bool,
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
        let audio = AudioState::default();

        // Start JACK client for audio preview
        let jack_client = match start_jack_client(&audio) {
            Ok(client) => {
                log::info!("JACK audio preview enabled");
                Some(client)
            }
            Err(e) => {
                log::warn!("JACK not available: {} - audio preview disabled", e);
                None
            }
        };

        let app = Self {
            current_view: View::Staging,
            staging: StagingState::default(),
            collection: CollectionState::default(),
            audio,
            jack_client,
            config: Arc::new(config),
            config_path,
            settings,
            shift_held: false,
            keybindings,
            pressed_hot_cue_keys: std::collections::HashSet::new(),
            pressed_cue_key: false,
        };

        // Initial collection scan
        let cmd = Task::perform(async {}, |_| Message::RefreshCollection);

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
                        bpm: Some(state.bpm),
                        original_bpm: Some(state.bpm),
                        key: Some(state.key.clone()),
                        beat_grid: BeatGrid {
                            beats: state.beat_grid.clone(),
                            first_beat_sample: state.beat_grid.first().copied(),
                        },
                        cue_points: cue_points.clone(),
                        saved_loops: saved_loops.clone(),
                        waveform_preview: None,
                    };

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

            // Staging: Import
            Message::SelectStemFile(index) => {
                // Open file dialog
                return Task::perform(
                    async move {
                        let file = rfd::AsyncFileDialog::new()
                            .add_filter("WAV files", &["wav", "WAV"])
                            .pick_file()
                            .await;
                        (index, file.map(|f| f.path().to_path_buf()))
                    },
                    |(index, path)| Message::StemFileSelected(index, path),
                );
            }
            Message::StemFileSelected(index, path) => {
                if let Some(path) = path {
                    match index {
                        0 => self.staging.importer.set_vocals(&path),
                        1 => self.staging.importer.set_drums(&path),
                        2 => self.staging.importer.set_bass(&path),
                        3 => self.staging.importer.set_other(&path),
                        _ => {}
                    }
                    self.staging.status = format!("Loaded stem {}", index + 1);

                    // Auto-fill track name from first stem filename if empty
                    if self.staging.track_name.is_empty() {
                        if let Some(parsed_name) = parse_track_name_from_filename(&path) {
                            self.staging.track_name = parsed_name;
                        }
                    }
                }
            }
            Message::SetTrackName(name) => {
                self.staging.track_name = name;
            }

            // Staging: Analysis
            Message::StartAnalysis => {
                // Check that all stems are loaded before starting
                if !self.staging.importer.is_complete() {
                    self.staging.status = String::from("Please load all 4 stem files first");
                    return Task::none();
                }

                self.staging.status = String::from("Analyzing... (this may take a moment)");

                // Clone data for the async task
                let importer = self.staging.importer.clone();
                let bpm_config = self.config.analysis.bpm.clone();

                // Spawn background task for analysis
                return Task::perform(
                    async move {
                        // Load stems and compute mono sum for analysis
                        let mono_samples = importer.get_mono_sum()?;

                        // Run analysis with configured BPM range
                        let result = crate::analysis::analyze_audio(&mono_samples, &bpm_config)?;

                        Ok::<_, anyhow::Error>(result)
                    },
                    |result| match result {
                        Ok(analysis) => Message::AnalysisComplete(analysis),
                        Err(e) => Message::AnalysisError(e.to_string()),
                    },
                );
            }
            Message::AnalysisProgress(progress) => {
                self.staging.analysis_progress = Some(progress);
            }
            Message::AnalysisComplete(result) => {
                self.staging.analysis_result = Some(result);
                self.staging.analysis_progress = None;
                self.staging.status = String::from("Analysis complete");
            }
            Message::AnalysisError(error) => {
                self.staging.status = format!("Analysis failed: {}", error);
                self.staging.analysis_progress = None;
            }

            // Staging: Export
            Message::AddToCollection => {
                log::info!("=== AddToCollection triggered ===");

                // Validate we have analysis result and track name
                let analysis = match self.staging.analysis_result.clone() {
                    Some(a) => {
                        log::info!("Analysis result found: BPM={:.1}, Key={}", a.bpm, a.key);
                        a
                    }
                    None => {
                        log::warn!("AddToCollection failed: no analysis result");
                        self.staging.status = String::from("Please analyze first");
                        return Task::none();
                    }
                };

                if self.staging.track_name.trim().is_empty() {
                    log::warn!("AddToCollection failed: empty track name");
                    self.staging.status = String::from("Please enter a track name");
                    return Task::none();
                }

                log::info!("Track name: '{}'", self.staging.track_name);
                self.staging.status = String::from("Exporting to collection...");

                // Clone data for async task
                let importer = self.staging.importer.clone();
                let track_name = self.staging.track_name.clone();
                let collection_path = self.collection.collection.path().to_path_buf();

                log::info!("Collection path: {:?}", collection_path);
                log::info!(
                    "Stem paths - Vocals: {:?}, Drums: {:?}, Bass: {:?}, Other: {:?}",
                    importer.vocals_path,
                    importer.drums_path,
                    importer.bass_path,
                    importer.other_path
                );

                // Spawn background task
                return Task::perform(
                    async move {
                        log::info!("=== Async export task started ===");

                        // Import stems
                        log::info!("Step 1: Importing stems...");
                        let buffers = match importer.import() {
                            Ok(b) => {
                                log::info!("Stems imported successfully: {} samples", b.len());
                                b
                            }
                            Err(e) => {
                                log::error!("Failed to import stems: {}", e);
                                return Err(e);
                            }
                        };

                        // Create metadata from analysis result
                        log::info!("Step 2: Creating metadata...");
                        let metadata = TrackMetadata {
                            bpm: Some(analysis.bpm),
                            original_bpm: Some(analysis.original_bpm),
                            key: Some(analysis.key.clone()),
                            beat_grid: BeatGrid {
                                beats: analysis.beat_grid.clone(),
                                first_beat_sample: analysis.beat_grid.first().copied(),
                            },
                            cue_points: Vec::new(), // No cue points initially
                            saved_loops: Vec::new(), // No saved loops initially
                            waveform_preview: None, // Generated during export
                        };
                        log::info!(
                            "Metadata: BPM={:.1}, Key={}, {} beats in grid",
                            analysis.bpm,
                            analysis.key,
                            analysis.beat_grid.len()
                        );

                        // Create temp file for export
                        let temp_dir = std::env::temp_dir();
                        let temp_path = temp_dir.join(format!("{}.wav", &track_name));
                        log::info!("Step 3: Exporting to temp file: {:?}", temp_path);

                        // Export to temp file (no saved loops for newly analyzed tracks)
                        match crate::export::export_stem_file(&temp_path, &buffers, &metadata, &[], &[]) {
                            Ok(()) => log::info!("Export to temp file succeeded"),
                            Err(e) => {
                                log::error!("Failed to export to temp file: {}", e);
                                return Err(e);
                            }
                        }

                        // Verify temp file exists and get size
                        match std::fs::metadata(&temp_path) {
                            Ok(meta) => log::info!("Temp file size: {} bytes", meta.len()),
                            Err(e) => log::error!("Temp file doesn't exist: {}", e),
                        }

                        // Add to collection (copies file to collection folder)
                        log::info!("Step 4: Adding to collection at {:?}", collection_path);
                        let mut collection = Collection::new(&collection_path);
                        let dest_path = match collection.add_track(&temp_path, &track_name) {
                            Ok(p) => {
                                log::info!("Added to collection: {:?}", p);
                                p
                            }
                            Err(e) => {
                                log::error!("Failed to add to collection: {}", e);
                                return Err(e);
                            }
                        };

                        // Clean up temp file
                        log::info!("Step 5: Cleaning up temp file");
                        if let Err(e) = std::fs::remove_file(&temp_path) {
                            log::warn!("Failed to remove temp file: {}", e);
                        }

                        log::info!("=== Export complete: {:?} ===", dest_path);
                        Ok::<PathBuf, anyhow::Error>(dest_path)
                    },
                    |result| Message::AddToCollectionComplete(result.map_err(|e| e.to_string())),
                );
            }
            Message::AddToCollectionComplete(result) => {
                log::info!("AddToCollectionComplete received");
                match result {
                    Ok(path) => {
                        log::info!("AddToCollectionComplete: SUCCESS - {:?}", path);
                        self.staging.status = format!("Added: {}", path.display());
                        // Clear staging state for next import
                        self.staging.importer.clear();
                        self.staging.analysis_result = None;
                        self.staging.track_name.clear();
                    }
                    Err(e) => {
                        log::error!("AddToCollectionComplete: FAILED - {}", e);
                        self.staging.status = format!("Failed to add: {}", e);
                    }
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

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path: path.clone(),
                            track: None,
                            stems: None,
                            cue_points,
                            saved_loops: metadata.saved_loops.clone(),
                            bpm,
                            key,
                            beat_grid,
                            duration_samples: 0, // Will be set when audio loads
                            modified: false,
                            combined_waveform,
                            loading_audio: true,
                            deck: None, // Created when stems load
                            last_playhead_update: std::time::Instant::now(),
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

                            // Initialize zoomed waveform with stem data
                            state.combined_waveform.zoomed.set_duration(duration_samples);
                            state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                            // Apply zoom level from config
                            state.combined_waveform.zoomed.set_zoom(self.config.display.zoom_bars);
                            state.combined_waveform.zoomed.compute_peaks(&stems, 0, 1600);

                            state.stems = Some(stems.clone());

                            // Create LoadedTrack from metadata + stems for Deck (Shared for RT-safe drop)
                            let duration_seconds = duration_samples as f64 / mesh_core::types::SAMPLE_RATE as f64;
                            let loaded_track = LoadedTrack {
                                path: state.path.clone(),
                                stems: stems.clone(),
                                metadata: TrackMetadata {
                                    bpm: Some(state.bpm),
                                    original_bpm: Some(state.bpm),
                                    key: Some(state.key.clone()),
                                    beat_grid: BeatGrid {
                                        beats: state.beat_grid.clone(),
                                        first_beat_sample: state.beat_grid.first().copied(),
                                    },
                                    cue_points: state.cue_points.clone(),
                                    saved_loops: state.saved_loops.clone(),
                                    waveform_preview: None, // Using live-generated waveform
                                },
                                duration_samples: duration_samples as usize,
                                duration_seconds,
                            };

                            // Create Deck and load track into it using fast path
                            let mut deck = Deck::new(DeckId::new(0));
                            let prepared = PreparedTrack::prepare(loaded_track);
                            deck.apply_prepared_track(prepared);
                            state.deck = Some(deck);

                            // Set up audio playback
                            self.audio.set_track(stems, duration_samples);
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

                        // Set up audio playback with the track stems (zero-copy via Shared)
                        let stems = track.stems.clone();
                        self.audio.set_track(stems.clone(), duration_samples);

                        // Create combined waveform with full track data
                        let mut combined_waveform = CombinedWaveformView::new();
                        combined_waveform.overview = WaveformView::from_track(&track, &cue_points);
                        // Apply grid density from config
                        combined_waveform.overview.set_grid_bars(self.config.display.grid_bars);
                        combined_waveform.zoomed = ZoomedWaveformView::from_metadata(
                            bpm,
                            beat_grid.clone(),
                            Vec::new(),
                        );
                        combined_waveform.zoomed.set_duration(duration_samples);
                        combined_waveform.zoomed.compute_peaks(&stems, 0, 1600);

                        // Create Deck and load track using fast path (Shared for RT-safe dealloc)
                        let mut deck = Deck::new(DeckId::new(0));
                        let track_for_deck = LoadedTrack {
                            path: track.path.clone(),
                            stems: track.stems.clone(),
                            metadata: track.metadata.clone(),
                            duration_samples: track.duration_samples,
                            duration_seconds: track.duration_seconds,
                        };
                        let prepared = PreparedTrack::prepare(track_for_deck);
                        deck.apply_prepared_track(prepared);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path,
                            track: Some(track.clone()),
                            stems: Some(stems),
                            cue_points,
                            saved_loops: track.metadata.saved_loops.clone(),
                            bpm,
                            key,
                            beat_grid,
                            duration_samples,
                            modified: false,
                            combined_waveform,
                            loading_audio: false,
                            deck: Some(deck),
                            last_playhead_update: std::time::Instant::now(),
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

                        // Sync beat grid to deck so beat jump uses updated grid
                        if let Some(ref mut deck) = state.deck {
                            deck.set_beat_grid(state.beat_grid.clone());
                        }
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
                        bpm: Some(state.bpm),
                        original_bpm: Some(state.bpm), // Use current BPM if no original
                        key: Some(state.key.clone()),
                        beat_grid: BeatGrid {
                            beats: state.beat_grid.clone(),
                            first_beat_sample: state.beat_grid.first().copied(),
                        },
                        cue_points: cue_points.clone(),
                        saved_loops: saved_loops.clone(),
                        waveform_preview: None, // Will be regenerated during save
                    };

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
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        // Clear preview return so release doesn't jump back
                        deck.clear_preview_return();
                        deck.play();
                        self.audio.play();
                    }
                }
                // Clear pressed hot cue keys to prevent spurious release events
                self.pressed_hot_cue_keys.clear();
            }
            Message::Pause => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        deck.pause();
                        self.audio.pause();
                    }
                }
            }
            Message::Stop => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        deck.pause();
                        deck.seek(0);
                        self.audio.pause();
                        self.audio.seek(0);
                    }
                    state.update_zoomed_waveform_cache(0);
                }
            }
            Message::Seek(position) => {
                let seek_pos = (position * self.audio.length as f64) as u64;
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        deck.seek(seek_pos as usize);
                    }
                    self.audio.seek(seek_pos);
                    state.combined_waveform.overview.set_position(position);
                    state.update_zoomed_waveform_cache(seek_pos);
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

                    let mut update_pos = None;
                    if let Some(ref mut deck) = state.deck {
                        // Snap to nearest beat using UI's current beat grid (not Deck's stale copy)
                        let current_pos = deck.position();
                        let snapped_pos = snap_to_nearest_beat(current_pos as u64, &state.beat_grid) as usize;

                        // Seek to snapped position and set as cue point
                        deck.seek(snapped_pos);
                        deck.set_cue_point_position(snapped_pos);
                        deck.play(); // Start preview
                        self.audio.seek(snapped_pos as u64);
                        self.audio.play();
                        update_pos = Some(snapped_pos as u64);

                        // Update waveform and cue marker
                        if self.audio.length > 0 {
                            let normalized = snapped_pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                            state.combined_waveform.overview.set_cue_position(Some(normalized));
                        }
                    }
                    if let Some(pos) = update_pos {
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::CueReleased => {
                // CDJ-style cue release: stop preview, return to cue point
                if let Some(ref mut state) = self.collection.loaded_track {
                    let mut update_pos = None;
                    if let Some(ref mut deck) = state.deck {
                        let cue_pos = deck.cue_point();
                        deck.pause();
                        deck.seek(cue_pos);
                        self.audio.pause();
                        self.audio.seek(cue_pos as u64);
                        update_pos = Some(cue_pos as u64);

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = cue_pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                    }
                    if let Some(pos) = update_pos {
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::BeatJump(beats) => {
                // Use Deck's beat jump methods
                if let Some(ref mut state) = self.collection.loaded_track {
                    let mut update_pos = None;
                    if let Some(ref mut deck) = state.deck {
                        if beats > 0 {
                            deck.beat_jump_forward();
                        } else {
                            deck.beat_jump_backward();
                        }
                        let pos = deck.position();
                        self.audio.seek(pos);
                        update_pos = Some(pos);

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                    }
                    if let Some(pos) = update_pos {
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::SetBeatJumpSize(size) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        deck.set_beat_jump_size(size);
                    }
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
                        if let Some(ref mut deck) = state.deck {
                            deck.seek(pos as usize);
                        }
                        self.audio.seek(pos);

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::SetCuePoint(index) => {
                // Snap to beat using UI's current beat grid (not Deck's stale copy)
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

                    // Update deck's hot cue (without re-snapping)
                    if let Some(ref mut deck) = state.deck {
                        deck.set_hot_cue_position(index, snapped_pos as usize);
                    }

                    // Update waveform markers (both overview and zoomed)
                    state.combined_waveform.overview.update_cue_markers(&state.cue_points);
                    state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                    state.modified = true;
                }
            }
            Message::ClearCuePoint(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(ref mut deck) = state.deck {
                        deck.clear_hot_cue(index);
                    }
                    state.cue_points.retain(|c| c.index != index as u8);
                    state.combined_waveform.overview.update_cue_markers(&state.cue_points);
                    state.combined_waveform.zoomed.update_cue_markers(&state.cue_points);
                    state.modified = true;
                }
            }
            Message::HotCuePressed(index) => {
                // Shift+click = delete cue point
                if self.shift_held {
                    return self.update(Message::ClearCuePoint(index));
                }

                // CDJ-style hot cue press (cue point handling done by Deck internally)
                if let Some(ref mut state) = self.collection.loaded_track {
                    let mut update_pos = None;

                    if let Some(ref mut deck) = state.deck {
                        deck.hot_cue_press(index);
                        let pos = deck.position();
                        let cue_pos = deck.cue_point(); // Deck sets this internally

                        self.audio.seek(pos);
                        // Start audio if playing OR if we just entered Cueing (preview) mode
                        if deck.state() == PlayState::Playing || deck.state() == PlayState::Cueing {
                            self.audio.play();
                        }
                        update_pos = Some(pos);

                        // Update waveform positions and cue marker
                        if self.audio.length > 0 {
                            let normalized = pos as f64 / self.audio.length as f64;
                            let cue_normalized = cue_pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                            state.combined_waveform.overview.set_cue_position(Some(cue_normalized));
                        }
                    }
                    if let Some(pos) = update_pos {
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }
            Message::HotCueReleased(_index) => {
                // Use Deck's hot_cue_release for CDJ-style return
                if let Some(ref mut state) = self.collection.loaded_track {
                    let mut update_pos = None;
                    if let Some(ref mut deck) = state.deck {
                        // Check if we were in preview mode BEFORE releasing
                        let was_previewing = deck.state() == PlayState::Cueing;

                        deck.hot_cue_release();
                        let pos = deck.position();
                        self.audio.seek(pos);

                        // Always pause audio when releasing from preview mode
                        if was_previewing {
                            self.audio.pause();
                        }
                        update_pos = Some(pos);

                        // Update waveform positions
                        if self.audio.length > 0 {
                            let normalized = pos as f64 / self.audio.length as f64;
                            state.combined_waveform.overview.set_position(normalized);
                        }
                    }
                    if let Some(pos) = update_pos {
                        state.update_zoomed_waveform_cache(pos);
                    }
                }
            }

            // Misc
            Message::Tick => {
                // Sync deck position from audio (JACK drives the position)
                if let Some(ref mut state) = self.collection.loaded_track {
                    let pos = self.audio.position();
                    if let Some(ref mut deck) = state.deck {
                        deck.seek(pos as usize);
                    }
                    // Update playhead timestamp for smooth interpolation
                    state.touch_playhead();

                    if self.audio.length > 0 {
                        let normalized = pos as f64 / self.audio.length as f64;
                        state.combined_waveform.overview.set_position(normalized);
                    }

                    // Update zoomed waveform peaks if playhead moved outside cache
                    if state.combined_waveform.zoomed.needs_recompute(pos) {
                        if let Some(ref stems) = state.stems {
                            state.combined_waveform.zoomed.compute_peaks(stems, pos, 1600);
                        }
                    }
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
                }
            }
            Message::NudgeBeatGridRight => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    nudge_beat_grid(state, BEAT_GRID_NUDGE_SAMPLES);
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
            Message::UpdateSettingsTrackNameFormat(value) => {
                self.settings.draft_track_name_format = value;
            }
            Message::UpdateSettingsGridBars(value) => {
                self.settings.draft_grid_bars = value;
            }
            Message::SaveSettings => {
                // Parse and validate values
                let min = self.settings.draft_min_tempo.parse::<i32>().unwrap_or(40);
                let max = self.settings.draft_max_tempo.parse::<i32>().unwrap_or(208);

                let mut new_config = (*self.config).clone();
                new_config.analysis.bpm.min_tempo = min;
                new_config.analysis.bpm.max_tempo = max;
                new_config.analysis.bpm.validate();

                // Update track name format
                new_config.track_name_format = self.settings.draft_track_name_format.clone();

                // Update display settings (grid bars)
                new_config.display.grid_bars = self.settings.draft_grid_bars;

                // Update drafts to show validated values
                self.settings.draft_min_tempo = new_config.analysis.bpm.min_tempo.to_string();
                self.settings.draft_max_tempo = new_config.analysis.bpm.max_tempo.to_string();

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
                // Track shift key state for shift+click actions
                self.shift_held = modifiers.shift();

                // Only handle keybindings in Collection view with a loaded track
                if self.current_view != View::Collection {
                    return Task::none();
                }
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

                // Increase/decrease beat jump size (ignore repeat)
                if !repeat && bindings.increase_jump_size.iter().any(|b| b == &key_str) {
                    let current = self.collection.loaded_track.as_ref()
                        .map(|s| s.beat_jump_size()).unwrap_or(4);
                    let new_size = match current {
                        1 => 4,
                        4 => 8,
                        8 => 16,
                        16 => 32,
                        _ => 32,
                    };
                    return self.update(Message::SetBeatJumpSize(new_size));
                }
                if !repeat && bindings.decrease_jump_size.iter().any(|b| b == &key_str) {
                    let current = self.collection.loaded_track.as_ref()
                        .map(|s| s.beat_jump_size()).unwrap_or(4);
                    let new_size = match current {
                        32 => 16,
                        16 => 8,
                        8 => 4,
                        4 => 1,
                        _ => 1,
                    };
                    return self.update(Message::SetBeatJumpSize(new_size));
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
                // Only handle in Collection view with a loaded track
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
        }

        Task::none()
    }

    /// Render the UI
    pub fn view(&self) -> Element<Message> {
        let header = self.view_header();

        let content: Element<Message> = match self.current_view {
            View::Staging => self.view_staging(),
            View::Collection => self.view_collection(),
        };

        let main = column![header, content].spacing(10);

        let base: Element<Message> = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .into();

        // Overlay settings modal if open
        if self.settings.is_open {
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
        } else {
            base
        }
    }

    /// Application theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    /// Subscription for periodic UI updates during playback and keyboard events
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::{keyboard, time};
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
                _ => Message::Tick, // Ignore ModifiersChanged
            }
        });

        // Update waveform playhead 60 times per second when playing
        let is_playing = self
            .collection
            .loaded_track
            .as_ref()
            .map(|t| t.is_playing())
            .unwrap_or(false);

        if is_playing || self.audio.is_playing() {
            iced::Subscription::batch([
                keyboard_sub,
                time::every(Duration::from_millis(16)).map(|_| Message::Tick),
            ])
        } else {
            keyboard_sub
        }
    }

    /// View header with navigation tabs
    fn view_header(&self) -> Element<Message> {
        let staging_btn = button(text("Staging"))
            .on_press(Message::SwitchView(View::Staging))
            .style(if self.current_view == View::Staging {
                button::primary
            } else {
                button::secondary
            });

        let collection_btn = button(text("Collection"))
            .on_press(Message::SwitchView(View::Collection))
            .style(if self.current_view == View::Collection {
                button::primary
            } else {
                button::secondary
            });

        // Settings gear icon (⚙ U+2699)
        let settings_btn = button(text("⚙").size(20))
            .on_press(Message::OpenSettings)
            .style(button::secondary);

        row![
            text("mesh-cue").size(24),
            Space::new().width(Length::Fill),
            staging_btn,
            collection_btn,
            settings_btn,
        ]
        .spacing(10)
        .into()
    }

    /// View for the staging (import) area
    fn view_staging(&self) -> Element<Message> {
        super::staging::view(&self.staging)
    }

    /// View for the collection browser and editor
    fn view_collection(&self) -> Element<Message> {
        super::collection_browser::view(&self.collection)
    }
}

/// Parse track name from stem filename
///
/// Handles common patterns:
/// - "Artist - Track Name (Vocals).wav" → "Artist - Track Name"
/// - "Artist - Track Name_Vocals.wav" → "Artist - Track Name"
/// - "Track Name - Vocals.wav" → "Track Name"
fn parse_track_name_from_filename(path: &std::path::Path) -> Option<String> {
    let filename = path.file_stem()?.to_string_lossy();

    // Remove common stem suffixes (case insensitive)
    let stem_patterns = [
        "(Vocals)", "(vocals)", "(VOCALS)",
        "(Drums)", "(drums)", "(DRUMS)",
        "(Bass)", "(bass)", "(BASS)",
        "(Other)", "(other)", "(OTHER)",
        "_Vocals", "_vocals", "_VOCALS",
        "_Drums", "_drums", "_DRUMS",
        "_Bass", "_bass", "_BASS",
        "_Other", "_other", "_OTHER",
        " - Vocals", " - vocals",
        " - Drums", " - drums",
        " - Bass", " - bass",
        " - Other", " - other",
    ];

    let mut name = filename.to_string();
    for pattern in stem_patterns {
        if let Some(idx) = name.find(pattern) {
            name = name[..idx].to_string();
            break;
        }
    }

    // Clean up trailing whitespace, dashes, and underscores
    let name = name.trim_end_matches(|c| c == ' ' || c == '-' || c == '_');

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Nudge amount in samples (~10ms at 44.1kHz for fine-grained control)
const BEAT_GRID_NUDGE_SAMPLES: i64 = 441;

/// Sample rate constant (matches mesh_core::types::SAMPLE_RATE)
const SAMPLE_RATE_F64: f64 = 44100.0;

/// Nudge the beat grid by a delta amount of samples
///
/// The grid is shifted by moving the first beat position, then regenerating
/// all subsequent beats. If the first beat would go negative or beyond one bar,
/// it wraps around to stay within a single bar range.
fn nudge_beat_grid(state: &mut LoadedTrackState, delta_samples: i64) {
    if state.beat_grid.is_empty() || state.bpm <= 0.0 {
        return;
    }

    // Calculate samples per bar (4 beats)
    let samples_per_beat = (SAMPLE_RATE_F64 * 60.0 / state.bpm) as i64;
    let samples_per_bar = samples_per_beat * 4;

    // Get current first beat
    let first_beat = state.beat_grid[0] as i64;

    // Apply delta
    let mut new_first_beat = first_beat + delta_samples;

    // Wrap around one bar if out of bounds
    if new_first_beat < 0 {
        new_first_beat += samples_per_bar;
    } else if new_first_beat >= samples_per_bar {
        new_first_beat -= samples_per_bar;
    }

    // Regenerate beat grid from new first beat
    let new_first_beat = new_first_beat as u64;
    state.beat_grid = regenerate_beat_grid(new_first_beat, state.bpm, state.duration_samples);

    // Update waveform displays
    update_waveform_beat_grid(state);

    // Sync beat grid to deck so beat jump uses updated grid
    if let Some(ref mut deck) = state.deck {
        deck.set_beat_grid(state.beat_grid.clone());
    }

    // Mark as modified for save
    state.modified = true;
}

/// Regenerate beat grid from a first beat position, BPM, and track duration
fn regenerate_beat_grid(first_beat: u64, bpm: f64, duration_samples: u64) -> Vec<u64> {
    if bpm <= 0.0 || duration_samples == 0 {
        return Vec::new();
    }

    let samples_per_beat = (SAMPLE_RATE_F64 * 60.0 / bpm) as u64;
    let mut beats = Vec::new();
    let mut pos = first_beat;

    while pos < duration_samples {
        beats.push(pos);
        pos += samples_per_beat;
    }

    beats
}

/// Update waveform beat grid markers after grid modification
fn update_waveform_beat_grid(state: &mut LoadedTrackState) {
    // Update zoomed view (uses sample positions directly)
    state.combined_waveform.zoomed.set_beat_grid(state.beat_grid.clone());

    // Update overview (uses normalized positions 0.0-1.0)
    if state.duration_samples > 0 {
        state.combined_waveform.overview.beat_markers = state.beat_grid
            .iter()
            .map(|&pos| pos as f64 / state.duration_samples as f64)
            .collect();
    }
}

/// Snap a position to the nearest beat in the beat grid
fn snap_to_nearest_beat(position: u64, beat_grid: &[u64]) -> u64 {
    if beat_grid.is_empty() {
        return position;
    }
    beat_grid
        .iter()
        .min_by_key(|&&b| (b as i64 - position as i64).unsigned_abs())
        .copied()
        .unwrap_or(position)
}
