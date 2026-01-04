//! Main application state and iced implementation

use crate::analysis::AnalysisResult;
use crate::audio::{AudioState, JackHandle, start_jack_client};
use crate::collection::Collection;
use crate::config::{self, Config};
use crate::export;
use crate::import::StemImporter;
use super::waveform::WaveformView;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, stack, text, Space};
use iced::{Color, Element, Length, Task, Theme};
use mesh_core::audio_file::{BeatGrid, CuePoint, LoadedTrack, StemBuffers, TrackMetadata};
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
#[derive(Debug)]
pub struct LoadedTrackState {
    /// Path to the track file
    pub path: PathBuf,
    /// Loaded audio data (wrapped in Arc for efficient cloning in messages)
    /// None while audio is loading asynchronously
    pub track: Option<Arc<LoadedTrack>>,
    /// Loaded stems (available after async load completes)
    pub stems: Option<Arc<StemBuffers>>,
    /// Current cue points (may be modified)
    pub cue_points: Vec<CuePoint>,
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
    /// Waveform display state (cached peak data)
    pub waveform: WaveformView,
    /// Whether audio is currently loading in the background
    pub loading_audio: bool,
    /// Whether audio is currently playing (synced from AudioState)
    pub is_playing: bool,
    /// Beat jump size in beats (1, 4, 8, 16, 32)
    pub beat_jump_size: i32,
    /// Current playhead position in samples (synced from AudioState)
    pub playhead_position: u64,
    /// CDJ-style cue point position (snap to grid)
    pub cue_point: Option<u64>,
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
            status: String::new(),
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
    /// Phase 2: Audio stems loaded (slow), now enable playback
    TrackStemsLoaded(Result<Arc<StemBuffers>, String>),
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
    /// CDJ-style cue button (snap to nearest beat grid)
    Cue,
    /// Beat jump by N beats (positive = forward, negative = backward)
    BeatJump(i32),
    /// Set beat jump size (1, 4, 8, 16, 32)
    SetBeatJumpSize(i32),

    // Hot Cues (8 action buttons)
    /// Jump to hot cue at index (0-7)
    JumpToCue(usize),
    /// Set hot cue at index to current playhead position
    SetCuePoint(usize),
    /// Clear hot cue at index (Shift+click)
    ClearCuePoint(usize),

    // Misc
    Tick,

    // Settings
    OpenSettings,
    CloseSettings,
    UpdateSettingsMinTempo(String),
    UpdateSettingsMaxTempo(String),
    UpdateSettingsTrackNameFormat(String),
    SaveSettings,
    SaveSettingsComplete(Result<(), String>),
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
        };

        // Initial collection scan
        let cmd = Task::perform(async {}, |_| Message::RefreshCollection);

        (app, cmd)
    }

    /// Application title
    pub fn title(&self) -> String {
        String::from("mesh-cue - Track Preparation")
    }

    /// Update state based on message
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Navigation
            Message::SwitchView(view) => {
                self.current_view = view;
                if view == View::Collection {
                    return Task::perform(async {}, |_| Message::RefreshCollection);
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

                        // Export to temp file
                        match crate::export::export_stem_file(&temp_path, &buffers, &metadata, &[]) {
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
                // Phase 1: Load metadata first (fast, ~50ms)
                if let Some(track) = self.collection.collection.tracks().get(index) {
                    let path = track.path.clone();
                    log::info!("LoadTrack: Starting two-phase load for {:?}", path);
                    return Task::perform(
                        async move {
                            LoadedTrack::load_metadata_only(&path)
                                .map(|metadata| (path, metadata))
                                .map_err(|e| e.to_string())
                        },
                        Message::TrackMetadataLoaded,
                    );
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

                        // Create placeholder waveform with beat markers from metadata
                        let waveform = WaveformView::from_metadata(&metadata);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path: path.clone(),
                            track: None,
                            stems: None,
                            cue_points,
                            bpm,
                            key,
                            beat_grid,
                            duration_samples: 0, // Will be set when audio loads
                            modified: false,
                            waveform,
                            loading_audio: true,
                            is_playing: false,
                            beat_jump_size: 4,
                            playhead_position: 0,
                            cue_point: None,
                        });

                        // Phase 2: Load audio stems in background (slow, ~3s)
                        return Task::perform(
                            async move {
                                LoadedTrack::load_stems(&path)
                                    .map(Arc::new)
                                    .map_err(|e| e.to_string())
                            },
                            Message::TrackStemsLoaded,
                        );
                    }
                    Err(e) => {
                        log::error!("Failed to load track metadata: {}", e);
                    }
                }
            }
            Message::TrackStemsLoaded(result) => {
                match result {
                    Ok(stems) => {
                        log::info!("TrackStemsLoaded: Audio ready, generating waveform");
                        if let Some(ref mut state) = self.collection.loaded_track {
                            let duration_samples = stems.len() as u64;
                            state.duration_samples = duration_samples;
                            state.loading_audio = false;

                            // Generate waveform from loaded stems
                            state.waveform.set_stems(&stems, &state.cue_points, &state.beat_grid);
                            state.stems = Some(stems.clone());

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

                        // Initialize waveform with peak data from loaded track
                        let waveform = WaveformView::from_track(&track, &cue_points);

                        // Set up audio playback with the track stems
                        let stems = Arc::new(track.stems.clone());
                        self.audio.set_track(stems.clone(), duration_samples);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path,
                            track: Some(track),
                            stems: Some(stems),
                            cue_points,
                            bpm,
                            key,
                            beat_grid,
                            duration_samples,
                            modified: false,
                            waveform,
                            loading_audio: false,
                            is_playing: false,
                            beat_jump_size: 4,
                            playhead_position: 0,
                            cue_point: None,
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
                    };

                    return Task::perform(
                        async move {
                            export::save_track_metadata(&path, &stems, &metadata, &cue_points)
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
                self.audio.play();
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.is_playing = true;
                }
            }
            Message::Pause => {
                self.audio.pause();
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.is_playing = false;
                }
            }
            Message::Stop => {
                self.audio.pause();
                self.audio.seek(0);
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.is_playing = false;
                    state.playhead_position = 0;
                }
            }
            Message::Seek(position) => {
                self.audio.seek((position * self.audio.length as f64) as u64);

                // Update waveform playhead position
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.waveform.set_position(position);
                }
            }
            Message::Cue => {
                // CDJ-style cue: snap to nearest beat in grid
                if let Some(ref mut state) = self.collection.loaded_track {
                    let current_pos = state.playhead_position;
                    let beat_grid = &state.beat_grid;

                    // Find nearest beat
                    if let Some(&nearest_beat) = beat_grid
                        .iter()
                        .min_by_key(|&&b| (b as i64 - current_pos as i64).unsigned_abs())
                    {
                        state.cue_point = Some(nearest_beat);
                        self.audio.seek(nearest_beat);
                        state.playhead_position = nearest_beat;

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = nearest_beat as f64 / self.audio.length as f64;
                            state.waveform.set_position(normalized);
                        }
                    }
                }
            }
            Message::BeatJump(beats) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    let current_pos = state.playhead_position;
                    let beat_grid = &state.beat_grid;

                    // Find current beat index
                    let current_beat_idx = beat_grid
                        .iter()
                        .position(|&b| b >= current_pos)
                        .unwrap_or(0) as i32;

                    // Calculate target beat index
                    let target_idx = (current_beat_idx + beats)
                        .max(0)
                        .min(beat_grid.len().saturating_sub(1) as i32)
                        as usize;

                    if let Some(&target_pos) = beat_grid.get(target_idx) {
                        self.audio.seek(target_pos);
                        state.playhead_position = target_pos;

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = target_pos as f64 / self.audio.length as f64;
                            state.waveform.set_position(normalized);
                        }
                    }
                }
            }
            Message::SetBeatJumpSize(size) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.beat_jump_size = size;
                }
            }
            Message::JumpToCue(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    if let Some(cue) = state.cue_points.iter().find(|c| c.index == index as u8) {
                        let pos = cue.sample_position;
                        self.audio.seek(pos);
                        state.playhead_position = pos;

                        // Update waveform
                        if self.audio.length > 0 {
                            let normalized = pos as f64 / self.audio.length as f64;
                            state.waveform.set_position(normalized);
                        }
                    }
                }
            }
            Message::SetCuePoint(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    let pos = state.playhead_position;

                    // Remove existing cue at this index (if any)
                    state.cue_points.retain(|c| c.index != index as u8);

                    // Add new cue point
                    state.cue_points.push(CuePoint {
                        index: index as u8,
                        sample_position: pos,
                        label: format!("Cue {}", index + 1),
                        color: None,
                    });

                    // Sort by index
                    state.cue_points.sort_by_key(|c| c.index);

                    // Update waveform markers
                    state.waveform.update_cue_markers(&state.cue_points);
                }
            }
            Message::ClearCuePoint(index) => {
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.cue_points.retain(|c| c.index != index as u8);
                    state.waveform.update_cue_markers(&state.cue_points);
                }
            }

            // Misc
            Message::Tick => {
                // Sync waveform playhead and state with audio position
                if let Some(ref mut state) = self.collection.loaded_track {
                    let pos = self.audio.position();
                    state.playhead_position = pos;
                    state.is_playing = self.audio.is_playing();
                    if self.audio.length > 0 {
                        let normalized = pos as f64 / self.audio.length as f64;
                        state.waveform.set_position(normalized);
                    }
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

    /// Subscription for periodic UI updates during playback
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::time;
        use std::time::Duration;

        // Update waveform playhead 30 times per second when playing
        if self.audio.is_playing() {
            time::every(Duration::from_millis(33)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
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
