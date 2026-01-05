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
use crate::loader::{TrackLoader, TrackLoadResult};
use mesh_core::audio_file::StemBuffers;
use mesh_core::engine::DeckAtomics;
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{PeaksComputer, PeaksComputeRequest};
use super::deck_view::{DeckView, DeckMessage};
use super::file_browser::{FileBrowserView, FileBrowserMessage};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};

/// Application state
pub struct MeshApp {
    /// Shared state with audio thread (for control commands)
    audio_state: Option<Arc<Mutex<SharedState>>>,
    /// Lock-free deck state for UI reads (position, play state, loop)
    /// These can be read without acquiring the engine mutex
    deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    /// Background track loader (avoids blocking UI/audio during loads)
    track_loader: TrackLoader,
    /// Pending track loads that couldn't be handed off due to lock contention
    /// These will be retried on the next tick (avoids blocking UI thread)
    pending_track_loads: Vec<TrackLoadResult>,
    /// Background peak computer (offloads expensive waveform peak computation)
    peaks_computer: PeaksComputer,
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
    pub fn new(
        audio_state: Option<Arc<Mutex<SharedState>>>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    ) -> Self {
        let audio_connected = audio_state.is_some();
        Self {
            audio_state,
            deck_atomics,
            track_loader: TrackLoader::spawn(),
            pending_track_loads: Vec::new(),
            peaks_computer: PeaksComputer::spawn(),
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
                // First, retry any pending track loads (from previous lock contention)
                // This ensures tracks are handed off even if audio thread was busy
                if !self.pending_track_loads.is_empty() {
                    if let Some(ref state) = self.audio_state {
                        if let Ok(mut s) = state.try_lock() {
                            // Drain pending loads while we have the lock
                            for load_result in self.pending_track_loads.drain(..) {
                                if let Ok(track) = load_result.result {
                                    if let Some(deck) = s.engine.deck_mut(load_result.deck_idx) {
                                        deck.load_track(track);
                                    }
                                    self.status = format!("Loaded track to deck {}", load_result.deck_idx + 1);
                                }
                            }
                        }
                        // If try_lock fails, pending loads stay queued for next tick
                    }
                }

                // Poll for completed background track loads (non-blocking)
                while let Some(load_result) = self.track_loader.try_recv() {
                    let deck_idx = load_result.deck_idx;

                    match load_result.result {
                        Ok(track) => {
                            // Update waveform state (UI-only, no lock needed)
                            self.player_canvas_state.decks[deck_idx].overview =
                                load_result.overview_state;
                            self.player_canvas_state.decks[deck_idx].zoomed =
                                load_result.zoomed_state;
                            self.deck_stems[deck_idx] = Some(load_result.stems);

                            // Try non-blocking lock to hand off track to engine
                            if let Some(ref state) = self.audio_state {
                                match state.try_lock() {
                                    Ok(mut s) => {
                                        if let Some(deck) = s.engine.deck_mut(deck_idx) {
                                            deck.load_track(track);
                                        }
                                        self.status = format!("Loaded track to deck {}", deck_idx + 1);
                                    }
                                    Err(_) => {
                                        // Lock contention - queue for retry on next tick
                                        // Rebuild the load result with the track for retry
                                        self.pending_track_loads.push(TrackLoadResult {
                                            deck_idx,
                                            result: Ok(track),
                                            overview_state: self.player_canvas_state.decks[deck_idx].overview.clone(),
                                            zoomed_state: self.player_canvas_state.decks[deck_idx].zoomed.clone(),
                                            stems: self.deck_stems[deck_idx].clone().unwrap_or_else(|| {
                                                Arc::new(mesh_core::audio_file::StemBuffers::with_length(0))
                                            }),
                                        });
                                        self.status = "Track loaded, syncing...".to_string();
                                    }
                                }
                            } else {
                                self.status = format!("Loaded track to deck {} (no audio)", deck_idx + 1);
                            }
                        }
                        Err(e) => {
                            self.status = format!("Error loading track: {}", e);
                        }
                    }
                }

                // Poll for completed background peak computations (non-blocking)
                // Results from peaks_computer are applied to ZoomedState
                while let Some(result) = self.peaks_computer.try_recv() {
                    if result.id < 4 {
                        let zoomed = &mut self.player_canvas_state.decks[result.id].zoomed;
                        zoomed.cached_peaks = result.cached_peaks;
                        zoomed.cache_start = result.cache_start;
                        zoomed.cache_end = result.cache_end;
                        // zoom_bars is already set on the zoomed state before request
                    }
                }

                // Read deck positions from atomics (LOCK-FREE - never blocks audio thread)
                // This is the key optimization: position/state reads happen ~60Hz and
                // no longer compete with the audio callback for the mutex
                let mut deck_positions: [Option<u64>; 4] = [None; 4];

                if let Some(ref atomics) = self.deck_atomics {
                    for i in 0..4 {
                        let position = atomics[i].position();
                        let is_playing = atomics[i].is_playing();
                        let loop_active = atomics[i].loop_active();
                        let loop_start = atomics[i].loop_start();
                        let loop_end = atomics[i].loop_end();

                        deck_positions[i] = Some(position);

                        // Update playhead state for smooth interpolation
                        self.player_canvas_state.set_playhead(i, position, is_playing);

                        // Update deck view play state from atomics
                        self.deck_views[i].sync_play_state(atomics[i].play_state());

                        // Update position and loop display in waveform
                        let duration = self.player_canvas_state.decks[i].overview.duration_samples;
                        if duration > 0 {
                            let pos_normalized = position as f64 / duration as f64;
                            self.player_canvas_state.decks[i]
                                .overview
                                .set_position(pos_normalized);

                            if loop_active {
                                let start = loop_start as f64 / duration as f64;
                                let end = loop_end as f64 / duration as f64;
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

                // Brief lock only for mixer sync and global BPM (less critical, can skip frame)
                if let Some(ref state) = self.audio_state {
                    if let Ok(s) = state.try_lock() {
                        self.global_bpm = s.engine.global_bpm();
                        self.mixer_view.sync_from_mixer(s.engine.mixer());
                    }
                    // Lock released here - audio thread can proceed
                }

                // Request zoomed waveform peak recomputation in background thread
                // This expensive operation (10-50ms) is now fully async - UI never blocks
                for i in 0..4 {
                    if let Some(position) = deck_positions[i] {
                        let zoomed = &self.player_canvas_state.decks[i].zoomed;
                        if zoomed.needs_recompute(position) && zoomed.has_track {
                            if let Some(ref stems) = self.deck_stems[i] {
                                let _ = self.peaks_computer.compute(PeaksComputeRequest {
                                    id: i,
                                    playhead: position,
                                    stems: stems.clone(),
                                    width: 1600,
                                    zoom_bars: zoomed.zoom_bars,
                                    duration_samples: zoomed.duration_samples,
                                    bpm: zoomed.bpm,
                                });
                            }
                        }
                    }
                }

                Task::none()
            }

            Message::Deck(deck_idx, deck_msg) => {
                if deck_idx < 4 {
                    if let Some(ref state) = self.audio_state {
                        // Use try_lock to avoid blocking audio thread
                        // If lock fails, show feedback to user (they can retry)
                        match state.try_lock() {
                            Ok(mut s) => {
                                self.deck_views[deck_idx].handle_message(
                                    deck_msg,
                                    s.engine.deck_mut(deck_idx),
                                );
                            }
                            Err(_) => {
                                // Lock contention - audio thread is busy
                                self.status = "Audio busy - try again".to_string();
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::Mixer(mixer_msg) => {
                if let Some(ref state) = self.audio_state {
                    // Use try_lock to avoid blocking audio thread
                    match state.try_lock() {
                        Ok(mut s) => {
                            self.mixer_view.handle_message(mixer_msg, s.engine.mixer_mut());
                        }
                        Err(_) => {
                            // Lock contention - audio thread is busy
                            self.status = "Audio busy - try again".to_string();
                        }
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
                    // Use try_lock to avoid blocking audio thread
                    match state.try_lock() {
                        Ok(mut s) => {
                            s.engine.set_global_bpm(bpm);
                        }
                        Err(_) => {
                            // Lock contention - will sync on next tick
                        }
                    }
                }
                Task::none()
            }

            Message::LoadTrack(deck_idx, path) => {
                // Send load request to background thread (non-blocking)
                // Result will be picked up in Tick handler via track_loader.try_recv()
                if deck_idx < 4 {
                    self.status = format!("Loading track to deck {}...", deck_idx + 1);
                    if let Err(e) = self.track_loader.load(deck_idx, path.into()) {
                        self.status = format!("Failed to start load: {}", e);
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
        // Update UI at ~60fps for smooth waveform animation
        time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick)
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
        Self::new(None, None)
    }
}
