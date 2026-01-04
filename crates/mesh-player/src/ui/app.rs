//! Main iced application for Mesh DJ Player
//!
//! This is the entry point for the GUI. It manages:
//! - Application state mirrored from the audio engine
//! - User input handling and message dispatch
//! - Layout of deck views and mixer

use std::sync::{Arc, Mutex};

use iced::widget::{column, container, row, slider, text, Space};
use iced::{Center, Element, Fill, Length, Subscription, Task, Theme};
use iced::time;

use crate::audio::SharedState;
use mesh_core::audio_file::StemBuffers;
use super::deck_view::{DeckView, DeckMessage};
use super::file_browser::{FileBrowserView, FileBrowserMessage};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};

/// Application state
pub struct MeshApp {
    /// Shared state with audio thread
    audio_state: Option<Arc<Mutex<SharedState>>>,
    /// Unified waveform state for all 4 decks
    player_canvas_state: PlayerCanvasState,
    /// Stem buffers for waveform recomputation (one per deck)
    deck_stems: [Option<Arc<StemBuffers>>; 4],
    /// Local deck view states (controls only, waveform moved to player_canvas_state)
    deck_views: [DeckView; 4],
    /// Mixer view state
    mixer_view: MixerView,
    /// File browser view
    file_browser: FileBrowserView,
    /// Global BPM
    global_bpm: f64,
    /// Status message
    status: String,
    /// Whether audio is connected
    audio_connected: bool,
}

/// Messages that can be sent to the application
#[derive(Debug, Clone)]
pub enum Message {
    /// Tick for periodic UI updates
    Tick,
    /// Deck-specific message
    Deck(usize, DeckMessage),
    /// Mixer message
    Mixer(MixerMessage),
    /// File browser message
    FileBrowser(FileBrowserMessage),
    /// Set global BPM
    SetGlobalBpm(f64),
    /// Load track to deck
    LoadTrack(usize, String),
    /// Seek on a deck (deck_idx, normalized position 0.0-1.0)
    DeckSeek(usize, f64),
    /// Set zoom level on a deck (deck_idx, zoom in bars)
    DeckSetZoom(usize, u32),
}

impl MeshApp {
    /// Create a new application instance
    pub fn new(audio_state: Option<Arc<Mutex<SharedState>>>) -> Self {
        let audio_connected = audio_state.is_some();
        Self {
            audio_state,
            player_canvas_state: PlayerCanvasState::new(),
            deck_stems: [None, None, None, None],
            deck_views: [
                DeckView::new(0),
                DeckView::new(1),
                DeckView::new(2),
                DeckView::new(3),
            ],
            mixer_view: MixerView::new(),
            file_browser: FileBrowserView::new(),
            global_bpm: 128.0,
            status: if audio_connected { "Audio connected".to_string() } else { "No audio".to_string() },
            audio_connected,
        }
    }

    /// Update application state
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                // Collect deck positions while holding lock briefly
                // This avoids blocking the audio thread during expensive compute_peaks()
                let mut deck_positions: [Option<u64>; 4] = [None; 4];

                if let Some(ref state) = self.audio_state {
                    if let Ok(s) = state.try_lock() {
                        self.global_bpm = s.engine.global_bpm();

                        // Update deck views and waveform state (fast operations only)
                        for i in 0..4 {
                            if let Some(deck) = s.engine.deck(i) {
                                // Sync deck view (controls)
                                self.deck_views[i].sync_from_deck(deck);

                                // Store position for later peak computation
                                let position = deck.position();
                                deck_positions[i] = Some(position);
                                self.player_canvas_state.set_playhead(i, position);

                                if let Some(track) = deck.track() {
                                    let duration = track.duration_samples as u64;
                                    if duration > 0 {
                                        let pos_normalized = position as f64 / duration as f64;
                                        self.player_canvas_state.decks[i]
                                            .overview
                                            .set_position(pos_normalized);
                                    }

                                    let loop_state = deck.loop_state();
                                    if loop_state.active && duration > 0 {
                                        let start = loop_state.start as f64 / duration as f64;
                                        let end = loop_state.end as f64 / duration as f64;
                                        self.player_canvas_state.decks[i]
                                            .overview
                                            .set_loop_region(Some((start, end)));
                                    } else {
                                        self.player_canvas_state.decks[i]
                                            .overview
                                            .set_loop_region(None);
                                    }
                                }
                            }
                        }

                        // Update mixer view
                        self.mixer_view.sync_from_mixer(s.engine.mixer());
                    }
                    // Lock released here - audio thread can proceed
                }

                // Recompute zoomed waveform peaks OUTSIDE the lock
                // This expensive operation (10-50ms) no longer blocks the audio thread
                for i in 0..4 {
                    if let Some(position) = deck_positions[i] {
                        if self.player_canvas_state.decks[i].zoomed.needs_recompute(position) {
                            if let Some(ref stems) = self.deck_stems[i] {
                                self.player_canvas_state.decks[i]
                                    .zoomed
                                    .compute_peaks(stems, position, 800);
                            }
                        }
                    }
                }

                Task::none()
            }

            Message::Deck(deck_idx, deck_msg) => {
                if deck_idx < 4 {
                    if let Some(ref state) = self.audio_state {
                        if let Ok(mut s) = state.lock() {
                            self.deck_views[deck_idx].handle_message(
                                deck_msg,
                                s.engine.deck_mut(deck_idx),
                            );
                        }
                    }
                }
                Task::none()
            }

            Message::Mixer(mixer_msg) => {
                if let Some(ref state) = self.audio_state {
                    if let Ok(mut s) = state.lock() {
                        self.mixer_view.handle_message(mixer_msg, s.engine.mixer_mut());
                    }
                }
                Task::none()
            }

            Message::FileBrowser(browser_msg) => {
                // Handle file browser message and check if we need to load a track
                if let Some((deck_idx, path)) = self.file_browser.handle_message(browser_msg) {
                    // Convert to LoadTrack message
                    let path_str = path.to_string_lossy().to_string();
                    return self.update(Message::LoadTrack(deck_idx, path_str));
                }
                Task::none()
            }

            Message::SetGlobalBpm(bpm) => {
                self.global_bpm = bpm;
                if let Some(ref state) = self.audio_state {
                    if let Ok(mut s) = state.lock() {
                        s.engine.set_global_bpm(bpm);
                    }
                }
                Task::none()
            }

            Message::LoadTrack(deck_idx, path) => {
                if deck_idx < 4 {
                    if let Some(ref state) = self.audio_state {
                        if let Ok(mut s) = state.lock() {
                            if let Some(deck) = s.engine.deck_mut(deck_idx) {
                                match mesh_core::audio_file::LoadedTrack::load(&path) {
                                    Ok(track) => {
                                        // Populate waveform state BEFORE moving track to deck
                                        let duration = track.duration_samples as u64;
                                        let bpm = track.metadata.bpm.unwrap_or(120.0);

                                        // Create cue markers for display
                                        let cue_markers: Vec<mesh_widgets::CueMarker> = track
                                            .metadata
                                            .cue_points
                                            .iter()
                                            .map(|cue| {
                                                let position = if duration > 0 {
                                                    cue.sample_position as f64 / duration as f64
                                                } else {
                                                    0.0
                                                };
                                                mesh_widgets::CueMarker {
                                                    position,
                                                    label: cue.label.clone(),
                                                    color: mesh_widgets::CUE_COLORS
                                                        [(cue.index as usize) % 8],
                                                    index: cue.index,
                                                }
                                            })
                                            .collect();

                                        // Populate overview from waveform preview (if available)
                                        if let Some(ref preview) = track.metadata.waveform_preview {
                                            self.player_canvas_state.decks[deck_idx].overview =
                                                mesh_widgets::OverviewState::from_preview(
                                                    preview,
                                                    &track.metadata.beat_grid.beats,
                                                    &track.metadata.cue_points,
                                                    duration,
                                                );
                                        } else {
                                            self.player_canvas_state.decks[deck_idx].overview =
                                                mesh_widgets::OverviewState::empty_with_message(
                                                    "No waveform preview",
                                                    &track.metadata.cue_points,
                                                    duration,
                                                );
                                        }

                                        // Populate zoomed waveform state
                                        self.player_canvas_state.decks[deck_idx].zoomed =
                                            mesh_widgets::ZoomedState::from_metadata(
                                                bpm,
                                                track.metadata.beat_grid.beats.clone(),
                                                cue_markers,
                                            );
                                        self.player_canvas_state.decks[deck_idx]
                                            .zoomed
                                            .set_duration(duration);

                                        // Store stems for waveform recomputation
                                        self.deck_stems[deck_idx] =
                                            Some(Arc::new(track.stems.clone()));

                                        // Compute initial zoomed peaks from stem data
                                        self.player_canvas_state.decks[deck_idx]
                                            .zoomed
                                            .compute_peaks(&track.stems, 0, 800);

                                        // Load track into engine (moves ownership)
                                        deck.load_track(track);
                                        self.status =
                                            format!("Loaded track to deck {}", deck_idx + 1);
                                    }
                                    Err(e) => {
                                        self.status = format!("Error loading track: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::DeckSeek(deck_idx, position) => {
                if deck_idx < 4 {
                    // TODO: Implement actual seeking via engine
                    let _ = position;
                }
                Task::none()
            }

            Message::DeckSetZoom(deck_idx, bars) => {
                if deck_idx < 4 {
                    self.player_canvas_state.decks[deck_idx].zoomed.set_zoom(bars);
                }
                Task::none()
            }
        }
    }

    /// Subscribe to periodic updates
    pub fn subscription(&self) -> Subscription<Message> {
        // Update UI at ~30fps
        time::every(std::time::Duration::from_millis(33)).map(|_| Message::Tick)
    }

    /// Build the view
    pub fn view(&self) -> Element<Message> {
        // Header with global controls
        let header = self.view_header();

        // File browser (top, full width)
        let file_browser = self.file_browser.view().map(Message::FileBrowser);

        // 3-column layout for controls + canvas + mixer:
        // Left: Decks 1 & 3 controls | Center: Waveform canvas + Mixer | Right: Decks 2 & 4 controls

        // Left column: Deck 1 (top) and Deck 3 (bottom) controls
        let left_controls = column![
            self.deck_views[0].view().map(|m| Message::Deck(0, m)),
            self.deck_views[2].view().map(|m| Message::Deck(2, m)),
        ]
        .spacing(10)
        .width(Length::FillPortion(1));

        // Center column: Waveform canvas (top) + Mixer (bottom)
        let center_canvas = view_player_canvas(&self.player_canvas_state);
        let mixer = self.mixer_view.view().map(Message::Mixer);
        let center_column = column![
            center_canvas,
            mixer,
        ]
        .spacing(10)
        .width(Length::FillPortion(2));

        // Right column: Deck 2 (top) and Deck 4 (bottom) controls
        let right_controls = column![
            self.deck_views[1].view().map(|m| Message::Deck(1, m)),
            self.deck_views[3].view().map(|m| Message::Deck(3, m)),
        ]
        .spacing(10)
        .width(Length::FillPortion(1));

        // Main content area: controls | canvas+mixer | controls
        let main_row = row![
            left_controls,
            center_column,
            right_controls,
        ]
        .spacing(10);

        // Status bar
        let status_bar = container(
            text(&self.status).size(12)
        )
        .padding(5);

        let content = column![
            header,
            file_browser,
            main_row,
            status_bar,
        ]
        .spacing(10)
        .padding(10);

        container(content)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// View for the header/global controls
    fn view_header(&self) -> Element<Message> {
        let title = text("MESH DJ PLAYER")
            .size(24);

        let bpm_label = text(format!("BPM: {:.1}", self.global_bpm)).size(16);

        let bpm_slider = slider(30.0..=200.0, self.global_bpm, Message::SetGlobalBpm)
            .step(0.1)
            .width(200);

        let connection_status = if self.audio_connected {
            text("● JACK Connected").size(12)
        } else {
            text("○ JACK Disconnected").size(12)
        };

        row![
            title,
            Space::new().width(Fill),
            bpm_label,
            bpm_slider,
            Space::new().width(Fill),
            connection_status,
        ]
        .spacing(20)
        .align_y(Center)
        .padding(10)
        .into()
    }

    /// Get the theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
    }
}

impl Default for MeshApp {
    fn default() -> Self {
        Self::new(None)
    }
}
