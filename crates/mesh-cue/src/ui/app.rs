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
    pub track: Arc<LoadedTrack>,
    /// Current cue points (may be modified)
    pub cue_points: Vec<CuePoint>,
    /// Modified BPM (user override)
    pub bpm: f64,
    /// Modified key (user override)
    pub key: String,
    /// Whether there are unsaved changes
    pub modified: bool,
    /// Waveform display state (cached peak data)
    pub waveform: WaveformView,
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

    // Misc
    Tick,

    // Settings
    OpenSettings,
    CloseSettings,
    UpdateSettingsMinTempo(String),
    UpdateSettingsMaxTempo(String),
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
                            beat_grid: BeatGrid { beats: analysis.beat_grid.clone() },
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
                if let Some(track) = self.collection.collection.tracks().get(index) {
                    let path = track.path.clone();
                    return Task::perform(
                        async move {
                            LoadedTrack::load(&path)
                                .map(Arc::new)
                                .map_err(|e| e.to_string())
                        },
                        Message::TrackLoaded,
                    );
                }
            }
            Message::TrackLoaded(result) => {
                match result {
                    Ok(track) => {
                        let path = track.path.clone();
                        let bpm = track.bpm();
                        let key = track.key().to_string();
                        let cue_points = track.metadata.cue_points.clone();
                        let duration_samples = track.duration_samples as u64;

                        // Initialize waveform with peak data from loaded track
                        let waveform = WaveformView::from_track(&track, &cue_points);

                        // Set up audio playback with the track stems
                        let stems = Arc::new(track.stems.clone());
                        self.audio.set_track(stems, duration_samples);

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path,
                            track,
                            cue_points,
                            bpm,
                            key,
                            modified: false,
                            waveform,
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
                    let path = state.path.clone();
                    let stems = state.track.stems.clone();
                    let cue_points = state.cue_points.clone();

                    // Build updated metadata from edited fields
                    let metadata = TrackMetadata {
                        bpm: Some(state.bpm),
                        original_bpm: state.track.metadata.original_bpm,
                        key: Some(state.key.clone()),
                        beat_grid: state.track.metadata.beat_grid.clone(),
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
            }
            Message::Pause => {
                self.audio.pause();
            }
            Message::Stop => {
                self.audio.pause();
                self.audio.seek(0);
            }
            Message::Seek(position) => {
                self.audio.seek((position * self.audio.length as f64) as u64);

                // Update waveform playhead position
                if let Some(ref mut state) = self.collection.loaded_track {
                    state.waveform.set_position(position);
                }
            }

            // Misc
            Message::Tick => {
                // Sync waveform playhead with audio position
                if let Some(ref mut state) = self.collection.loaded_track {
                    let pos = self.audio.position();
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
            Message::SaveSettings => {
                // Parse and validate values
                let min = self.settings.draft_min_tempo.parse::<i32>().unwrap_or(40);
                let max = self.settings.draft_max_tempo.parse::<i32>().unwrap_or(208);

                let mut new_config = (*self.config).clone();
                new_config.analysis.bpm.min_tempo = min;
                new_config.analysis.bpm.max_tempo = max;
                new_config.analysis.bpm.validate();

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
