//! Main iced application for Mesh DJ Player
//!
//! This is the entry point for the GUI. It manages:
//! - Application state mirrored from the audio engine
//! - User input handling and message dispatch
//! - Layout of deck views and mixer

use std::sync::{Arc, Mutex};

use iced::widget::{column, container, row, slider, text, Space};
use iced::{Center, Element, Fill, Subscription, Task, Theme};
use iced::time;

use crate::audio::SharedState;
use super::deck_view::{DeckView, DeckMessage};
use super::mixer_view::{MixerView, MixerMessage};

/// Application state
pub struct MeshApp {
    /// Shared state with audio thread
    audio_state: Option<Arc<Mutex<SharedState>>>,
    /// Local deck view states
    deck_views: [DeckView; 4],
    /// Mixer view state
    mixer_view: MixerView,
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
    /// Set global BPM
    SetGlobalBpm(f64),
    /// Load track to deck
    LoadTrack(usize, String),
}

impl MeshApp {
    /// Create a new application instance
    pub fn new(audio_state: Option<Arc<Mutex<SharedState>>>) -> Self {
        let audio_connected = audio_state.is_some();
        Self {
            audio_state,
            deck_views: [
                DeckView::new(0),
                DeckView::new(1),
                DeckView::new(2),
                DeckView::new(3),
            ],
            mixer_view: MixerView::new(),
            global_bpm: 128.0,
            status: if audio_connected { "Audio connected".to_string() } else { "No audio".to_string() },
            audio_connected,
        }
    }

    /// Update application state
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                // Sync UI state from audio engine
                if let Some(ref state) = self.audio_state {
                    if let Ok(s) = state.try_lock() {
                        self.global_bpm = s.engine.global_bpm();

                        // Update deck views with engine state
                        for i in 0..4 {
                            if let Some(deck) = s.engine.deck(i) {
                                self.deck_views[i].sync_from_deck(deck);
                            }
                        }

                        // Update mixer view
                        self.mixer_view.sync_from_mixer(s.engine.mixer());
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
                                        deck.load_track(track);
                                        self.status = format!("Loaded track to deck {}", deck_idx + 1);
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

        // Main content: 4 decks in 2x2 grid
        let top_decks = row![
            self.deck_views[0].view().map(|m| Message::Deck(0, m)),
            self.deck_views[1].view().map(|m| Message::Deck(1, m)),
        ]
        .spacing(10);

        let bottom_decks = row![
            self.deck_views[2].view().map(|m| Message::Deck(2, m)),
            self.deck_views[3].view().map(|m| Message::Deck(3, m)),
        ]
        .spacing(10);

        // Mixer at the bottom
        let mixer = self.mixer_view.view().map(Message::Mixer);

        // Status bar
        let status_bar = container(
            text(&self.status).size(12)
        )
        .padding(5);

        let content = column![
            header,
            top_decks,
            bottom_decks,
            mixer,
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
