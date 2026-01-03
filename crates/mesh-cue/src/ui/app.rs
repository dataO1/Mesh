//! Main application state and iced implementation

use crate::analysis::AnalysisResult;
use crate::audio::AudioState;
use crate::collection::Collection;
use crate::import::StemImporter;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Element, Length, Task, Theme};
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
}

impl MeshCueApp {
    /// Create a new application instance
    pub fn new() -> (Self, Task<Message>) {
        let app = Self {
            current_view: View::Staging,
            staging: StagingState::default(),
            collection: CollectionState::default(),
            audio: AudioState::default(),
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

                // Clone importer for the async task
                let importer = self.staging.importer.clone();

                // Spawn background task for analysis
                return Task::perform(
                    async move {
                        // Load stems and compute mono sum for analysis
                        let mono_samples = importer.get_mono_sum()?;

                        // Run analysis (BPM detection, key detection, beat grid)
                        let result = crate::analysis::analyze_audio(&mono_samples)?;

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
                // Validate we have analysis result and track name
                let analysis = match self.staging.analysis_result.clone() {
                    Some(a) => a,
                    None => {
                        self.staging.status = String::from("Please analyze first");
                        return Task::none();
                    }
                };

                if self.staging.track_name.trim().is_empty() {
                    self.staging.status = String::from("Please enter a track name");
                    return Task::none();
                }

                self.staging.status = String::from("Exporting to collection...");

                // Clone data for async task
                let importer = self.staging.importer.clone();
                let track_name = self.staging.track_name.clone();
                let collection_path = self.collection.collection.path().to_path_buf();

                // Spawn background task
                return Task::perform(
                    async move {
                        // Import stems
                        let buffers = importer.import()?;

                        // Create metadata from analysis result
                        let metadata = TrackMetadata {
                            bpm: Some(analysis.bpm),
                            original_bpm: Some(analysis.original_bpm),
                            key: Some(analysis.key.clone()),
                            beat_grid: BeatGrid { beats: analysis.beat_grid.clone() },
                            cue_points: Vec::new(), // No cue points initially
                        };

                        // Create temp file for export
                        let temp_dir = std::env::temp_dir();
                        let temp_path = temp_dir.join(format!("{}.wav", &track_name));

                        // Export to temp file
                        crate::export::export_stem_file(&temp_path, &buffers, &metadata, &[])?;

                        // Add to collection (copies file to collection folder)
                        let mut collection = Collection::new(&collection_path);
                        let dest_path = collection.add_track(&temp_path, &track_name)?;

                        // Clean up temp file
                        let _ = std::fs::remove_file(&temp_path);

                        Ok::<PathBuf, anyhow::Error>(dest_path)
                    },
                    |result| Message::AddToCollectionComplete(result.map_err(|e| e.to_string())),
                );
            }
            Message::AddToCollectionComplete(result) => {
                match result {
                    Ok(path) => {
                        self.staging.status = format!("Added: {}", path.display());
                        // Clear staging state for next import
                        self.staging.importer.clear();
                        self.staging.analysis_result = None;
                        self.staging.track_name.clear();
                    }
                    Err(e) => {
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

                        self.collection.loaded_track = Some(LoadedTrackState {
                            path,
                            track,
                            cue_points,
                            bpm,
                            key,
                            modified: false,
                        });
                    }
                    Err(e) => {
                        eprintln!("Failed to load track: {}", e);
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
                // TODO: Save track with updated metadata
            }
            Message::SaveComplete(result) => {
                match result {
                    Ok(()) => {
                        if let Some(ref mut state) = self.collection.loaded_track {
                            state.modified = false;
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to save track: {}", e);
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
            }

            // Misc
            Message::Tick => {
                // Update UI from audio state
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

        container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .into()
    }

    /// Application theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
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

        row![
            text("mesh-cue").size(24),
            Space::new().width(Length::Fill),
            staging_btn,
            collection_btn,
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
