//! Main iced application for Mesh DJ Player
//!
//! This is the entry point for the GUI. It manages:
//! - Application state mirrored from the audio engine
//! - User input handling and message dispatch
//! - Layout of deck views and mixer
//!
//! ## Lock-Free Architecture
//!
//! This app uses a lock-free command queue to communicate with the audio engine.
//! Instead of acquiring a mutex, UI actions send commands via an SPSC ringbuffer.
//! This guarantees zero audio dropouts during track loading or any UI interaction.

use std::sync::Arc;

use basedrop::Shared;
use iced::widget::{column, container, row, slider, text, Space};
use iced::{Center, Element, Fill, Length, Subscription, Task, Theme};
use iced::time;

use crate::audio::CommandSender;
use crate::loader::TrackLoader;
use mesh_core::audio_file::StemBuffers;
use mesh_core::engine::{DeckAtomics, EngineCommand};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{PeaksComputer, PeaksComputeRequest};
use super::deck_view::{DeckView, DeckMessage};
use super::file_browser::{FileBrowserView, FileBrowserMessage};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};

/// Application state
pub struct MeshApp {
    /// Command sender for lock-free communication with audio engine
    /// Uses an SPSC ringbuffer - no mutex, no dropouts, guaranteed delivery
    command_sender: Option<CommandSender>,
    /// Lock-free deck state for UI reads (position, play state, loop)
    /// These atomics are updated by the audio thread; UI reads are wait-free
    deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    /// Background track loader (avoids blocking UI/audio during loads)
    track_loader: TrackLoader,
    /// Background peak computer (offloads expensive waveform peak computation)
    peaks_computer: PeaksComputer,
    /// Unified waveform state for all 4 decks
    player_canvas_state: PlayerCanvasState,
    /// Stem buffers for waveform recomputation (Shared for RT-safe deallocation)
    deck_stems: [Option<Shared<StemBuffers>>; 4],
    /// Local deck view states (controls only, waveform moved to player_canvas_state)
    deck_views: [DeckView; 4],
    /// Mixer view state
    mixer_view: MixerView,
    /// File browser view
    file_browser: FileBrowserView,
    /// Global BPM (cached for UI display; authoritative value is in audio engine)
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
    ///
    /// ## Parameters
    ///
    /// - `command_sender`: Lock-free command channel for engine control (None for offline mode)
    /// - `deck_atomics`: Lock-free position/state for UI reads (None for offline mode)
    pub fn new(
        command_sender: Option<CommandSender>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    ) -> Self {
        let audio_connected = command_sender.is_some();
        Self {
            command_sender,
            deck_atomics,
            track_loader: TrackLoader::spawn(),
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
            status: if audio_connected { "Audio connected (lock-free)".to_string() } else { "No audio".to_string() },
            audio_connected,
        }
    }

    /// Update application state
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                // Poll for completed background track loads (non-blocking)
                // With lock-free architecture, there's no contention - commands always succeed
                while let Some(load_result) = self.track_loader.try_recv() {
                    let deck_idx = load_result.deck_idx;

                    match load_result.result {
                        Ok(prepared) => {
                            // Update waveform state (UI-only)
                            self.player_canvas_state.decks[deck_idx].overview =
                                load_result.overview_state;
                            self.player_canvas_state.decks[deck_idx].zoomed =
                                load_result.zoomed_state;
                            self.deck_stems[deck_idx] = Some(load_result.stems);

                            // Send track to audio engine via lock-free queue (~50ns, never blocks!)
                            if let Some(ref mut sender) = self.command_sender {
                                log::debug!("[PERF] UI: Sending LoadTrack command for deck {}", deck_idx);
                                let send_start = std::time::Instant::now();
                                let result = sender.send(EngineCommand::LoadTrack {
                                    deck: deck_idx,
                                    track: Box::new(prepared),
                                });
                                log::debug!(
                                    "[PERF] UI: LoadTrack command sent in {:?} (success: {})",
                                    send_start.elapsed(),
                                    result.is_ok()
                                );
                                self.status = format!("Loaded track to deck {}", deck_idx + 1);
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
                    }
                }

                // Read deck positions from atomics (LOCK-FREE - never blocks audio thread)
                // Position/state reads happen ~60Hz with zero contention
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

                // Request zoomed waveform peak recomputation in background thread
                // This expensive operation (10-50ms) is fully async - UI never blocks
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
                    // Translate DeckMessage to EngineCommand and send via lock-free queue
                    // No mutex, no blocking, no dropouts!
                    if let Some(ref mut sender) = self.command_sender {
                        use DeckMessage::*;
                        match deck_msg {
                            TogglePlayPause => {
                                let _ = sender.send(EngineCommand::TogglePlay { deck: deck_idx });
                            }
                            CuePressed => {
                                let _ = sender.send(EngineCommand::CuePress { deck: deck_idx });
                            }
                            CueReleased => {
                                let _ = sender.send(EngineCommand::CueRelease { deck: deck_idx });
                            }
                            SetCue => {
                                let _ = sender.send(EngineCommand::SetCuePoint { deck: deck_idx });
                            }
                            HotCuePressed(slot) => {
                                let _ = sender.send(EngineCommand::HotCuePress { deck: deck_idx, slot });
                            }
                            HotCueReleased(_slot) => {
                                let _ = sender.send(EngineCommand::HotCueRelease { deck: deck_idx });
                            }
                            SetHotCue(_slot) => {
                                // Hot cue is set automatically on press if empty
                            }
                            ClearHotCue(slot) => {
                                let _ = sender.send(EngineCommand::ClearHotCue { deck: deck_idx, slot });
                            }
                            SetBeatJumpSize(beats) => {
                                let _ = sender.send(EngineCommand::SetBeatJumpSize { deck: deck_idx, beats });
                            }
                            Sync => {
                                // TODO: Implement sync command
                            }
                            ToggleLoop => {
                                let _ = sender.send(EngineCommand::ToggleLoop { deck: deck_idx });
                            }
                            SetLoopLength(_beats) => {
                                // Loop length is handled via adjust commands
                            }
                            LoopHalve => {
                                let _ = sender.send(EngineCommand::AdjustLoopLength { deck: deck_idx, direction: -1 });
                            }
                            LoopDouble => {
                                let _ = sender.send(EngineCommand::AdjustLoopLength { deck: deck_idx, direction: 1 });
                            }
                            BeatJumpBack => {
                                let _ = sender.send(EngineCommand::BeatJumpBackward { deck: deck_idx });
                            }
                            BeatJumpForward => {
                                let _ = sender.send(EngineCommand::BeatJumpForward { deck: deck_idx });
                            }
                            SetPitch(_pitch) => {
                                // TODO: Implement pitch control via command
                            }
                            ToggleStemMute(stem_idx) => {
                                if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                    let _ = sender.send(EngineCommand::ToggleStemMute { deck: deck_idx, stem });
                                }
                            }
                            ToggleStemSolo(stem_idx) => {
                                if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                    let _ = sender.send(EngineCommand::ToggleStemSolo { deck: deck_idx, stem });
                                }
                            }
                            SetStemVolume(_stem_idx, _volume) => {
                                // TODO: Add stem volume command
                            }
                            SelectStem(stem_idx) => {
                                // UI-only state, no command needed
                                self.deck_views[deck_idx].set_selected_stem(stem_idx);
                            }
                            SetStemKnob(_stem_idx, _knob_idx, _value) => {
                                // TODO: Effect parameter control
                            }
                            ToggleEffectBypass(_stem_idx, _effect_idx) => {
                                // TODO: Effect bypass control
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::Mixer(mixer_msg) => {
                // Translate MixerMessage to EngineCommand where applicable
                if let Some(ref mut sender) = self.command_sender {
                    use MixerMessage::*;
                    match &mixer_msg {
                        SetChannelVolume(deck, volume) => {
                            let _ = sender.send(EngineCommand::SetVolume { deck: *deck, volume: *volume });
                        }
                        ToggleChannelCue(deck) => {
                            // Read current state and send toggle
                            let enabled = !self.mixer_view.cue_enabled(*deck);
                            let _ = sender.send(EngineCommand::SetCueListen { deck: *deck, enabled });
                        }
                        _ => {
                            // EQ, filter, master volume, etc. - not yet in engine commands
                        }
                    }
                }
                // Always update local UI state
                self.mixer_view.handle_local_message(mixer_msg);
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
                // Send BPM change via lock-free command (~50ns)
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::SetGlobalBpm(bpm));
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
