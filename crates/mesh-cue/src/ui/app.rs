//! Main application state and iced implementation

use crate::analysis::AnalysisResult;
use crate::audio::AudioState;
use crate::collection::{Collection, CollectionTrack};
use crate::import::StemImporter;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Element, Length, Task, Theme};
use mesh_core::audio_file::{CuePoint, LoadedTrack, StemBuffers};
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
    CollectionRefreshed(Result<Vec<CollectionTrack>, String>),
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
                self.staging.status = String::from("Analyzing...");
                self.staging.analysis_progress = Some(0.0);
                // TODO: Run analysis in background
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
                self.staging.status = String::from("Adding to collection...");
                // TODO: Export to collection
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
                let mut collection = Collection::new(self.collection.collection.path());
                return Task::perform(
                    async move {
                        collection.scan().map_err(|e| e.to_string())?;
                        Ok(collection.tracks().to_vec())
                    },
                    Message::CollectionRefreshed,
                );
            }
            Message::CollectionRefreshed(result) => {
                match result {
                    Ok(tracks) => {
                        // Re-scan succeeded - collection will update internally
                        let _ = self.collection.collection.scan();
                    }
                    Err(e) => {
                        // Log error
                        eprintln!("Failed to refresh collection: {}", e);
                    }
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
