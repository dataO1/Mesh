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

use std::path::PathBuf;
use std::sync::Arc;

use basedrop::Shared;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, slider, stack, text, Space};
use iced::{Center as CenterAlign, Color, Element, Fill, Length, Subscription, Task, Theme};
use iced::time;

use crate::audio::CommandSender;
use crate::config::{self, PlayerConfig};
use crate::loader::TrackLoader;
use mesh_core::audio_file::StemBuffers;
use mesh_core::engine::{DeckAtomics, EngineCommand};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{PeaksComputer, PeaksComputeRequest};
use super::collection_browser::{CollectionBrowserState, CollectionBrowserMessage};
use super::deck_view::{DeckView, DeckMessage};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};
use super::settings::SettingsState;

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
    /// Collection browser (read-only, shared with mesh-cue)
    collection_browser: CollectionBrowserState,
    /// Global BPM (cached for UI display; authoritative value is in audio engine)
    global_bpm: f64,
    /// Status message
    status: String,
    /// Whether audio is connected
    audio_connected: bool,
    /// Configuration
    config: Arc<PlayerConfig>,
    /// Path to config file
    config_path: PathBuf,
    /// Settings modal state
    settings: SettingsState,
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
    /// Collection browser message
    CollectionBrowser(CollectionBrowserMessage),
    /// Set global BPM
    SetGlobalBpm(f64),
    /// Load track to deck
    LoadTrack(usize, String),
    /// Seek on a deck (deck_idx, normalized position 0.0-1.0)
    DeckSeek(usize, f64),
    /// Set zoom level on a deck (deck_idx, zoom in bars)
    DeckSetZoom(usize, u32),

    // Settings
    /// Open settings modal
    OpenSettings,
    /// Close settings modal
    CloseSettings,
    /// Update settings: loop length index
    UpdateSettingsLoopLength(usize),
    /// Update settings: zoom bars
    UpdateSettingsZoomBars(u32),
    /// Update settings: grid bars
    UpdateSettingsGridBars(u32),
    /// Update settings: phase sync enabled
    UpdateSettingsPhaseSync(bool),
    /// Save settings to disk
    SaveSettings,
    /// Settings save complete
    SaveSettingsComplete(Result<(), String>),
}

impl MeshApp {
    /// Create a new application instance
    ///
    /// ## Parameters
    ///
    /// - `command_sender`: Lock-free command channel for engine control (None for offline mode)
    /// - `deck_atomics`: Lock-free position/state for UI reads (None for offline mode)
    /// - `jack_sample_rate`: JACK's sample rate for track loading (e.g., 48000 or 44100)
    pub fn new(
        mut command_sender: Option<CommandSender>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
        jack_sample_rate: u32,
    ) -> Self {
        // Load configuration
        let config_path = config::default_config_path();
        let config = Arc::new(config::load_config(&config_path));
        let settings = SettingsState::from_config(&config);

        // Send initial config to audio engine
        if let Some(ref mut sender) = command_sender {
            // Initialize global BPM
            let _ = sender.send(EngineCommand::SetGlobalBpm(config.audio.global_bpm));
            // Initialize phase sync setting
            let _ = sender.send(EngineCommand::SetPhaseSync(config.audio.phase_sync));
        }

        let audio_connected = command_sender.is_some();
        Self {
            command_sender,
            deck_atomics,
            track_loader: TrackLoader::spawn(jack_sample_rate),
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
            collection_browser: CollectionBrowserState::new(config.collection_path.clone()),
            global_bpm: config.audio.global_bpm,
            status: if audio_connected { "Audio connected (lock-free)".to_string() } else { "No audio".to_string() },
            audio_connected,
            config,
            config_path,
            settings,
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

                            // Set track name and key for header display (before moving prepared)
                            let track_name = prepared.track.filename().to_string();
                            let track_key = prepared.track.key().to_string();
                            self.player_canvas_state.set_track_name(deck_idx, track_name);
                            self.player_canvas_state.set_track_key(deck_idx, track_key);

                            // Sync hot cue positions from track metadata for button colors
                            // First clear all slots
                            for slot in 0..8 {
                                self.deck_views[deck_idx].set_hot_cue_position(slot, None);
                            }
                            // Then set positions from cue points (indexed by their slot/index field)
                            for i in 0..prepared.track.cue_count() {
                                if let Some(cue) = prepared.track.get_cue(i) {
                                    let slot = cue.index as usize;
                                    if slot < 8 {
                                        self.deck_views[deck_idx].set_hot_cue_position(slot, Some(cue.sample_position));
                                    }
                                }
                            }

                            // Reset stem states for new track (all stems active, none muted/soloed)
                            // Update UI state
                            for stem_idx in 0..4 {
                                self.deck_views[deck_idx].set_stem_muted(stem_idx, false);
                                self.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                                self.player_canvas_state.set_stem_active(deck_idx, stem_idx, true);
                            }

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

                                // Set default loop length from config
                                let _ = sender.send(EngineCommand::SetLoopLengthIndex {
                                    deck: deck_idx,
                                    index: self.config.display.default_loop_length_index,
                                });

                                // Reset all stems to unmuted/un-soloed in the engine
                                for stem_idx in 0..4 {
                                    if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                        let _ = sender.send(EngineCommand::SetStemMute {
                                            deck: deck_idx,
                                            stem,
                                            muted: false,
                                        });
                                        let _ = sender.send(EngineCommand::SetStemSolo {
                                            deck: deck_idx,
                                            stem,
                                            soloed: false,
                                        });
                                    }
                                }

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
                        let is_master = atomics[i].is_master();

                        deck_positions[i] = Some(position);

                        // Update playhead state for smooth interpolation
                        self.player_canvas_state.set_playhead(i, position, is_playing);

                        // Update master status for UI indicator
                        self.player_canvas_state.set_master(i, is_master);

                        // Update deck view state from atomics
                        self.deck_views[i].sync_play_state(atomics[i].play_state());
                        self.deck_views[i].sync_loop_length_index(atomics[i].loop_length_index());

                        // Sync stem active states to canvas
                        // Check if any stem is soloed
                        let any_soloed = (0..4).any(|s| self.deck_views[i].is_stem_soloed(s));
                        for stem_idx in 0..4 {
                            let is_muted = self.deck_views[i].is_stem_muted(stem_idx);
                            let is_soloed = self.deck_views[i].is_stem_soloed(stem_idx);
                            // If any stem is soloed, only soloed stems are active
                            // Otherwise, non-muted stems are active
                            let is_active = if any_soloed {
                                is_soloed && !is_muted
                            } else {
                                !is_muted
                            };
                            self.player_canvas_state.set_stem_active(i, stem_idx, is_active);
                        }

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
                                self.player_canvas_state.decks[i]
                                    .zoomed
                                    .set_loop_region(Some((start, end)));
                            } else {
                                self.player_canvas_state.decks[i]
                                    .overview
                                    .set_loop_region(None);
                                self.player_canvas_state.decks[i]
                                    .zoomed
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
                                // If slot is empty, engine will set a new hot cue at current position
                                // Update UI optimistically by reading current position from atomics
                                let slot_was_empty = self.deck_views[deck_idx].hot_cue_position(slot).is_none();
                                if slot_was_empty {
                                    if let Some(ref atomics) = self.deck_atomics {
                                        let position = atomics[deck_idx].position();
                                        self.deck_views[deck_idx].set_hot_cue_position(slot, Some(position));
                                    }
                                }
                                let _ = sender.send(EngineCommand::HotCuePress { deck: deck_idx, slot });
                            }
                            HotCueReleased(_slot) => {
                                let _ = sender.send(EngineCommand::HotCueRelease { deck: deck_idx });
                            }
                            SetHotCue(_slot) => {
                                // Hot cue is set automatically on press if empty
                            }
                            ClearHotCue(slot) => {
                                // Clear the UI state for this hot cue slot
                                self.deck_views[deck_idx].set_hot_cue_position(slot, None);
                                let _ = sender.send(EngineCommand::ClearHotCue { deck: deck_idx, slot });
                            }
                            Sync => {
                                // TODO: Implement sync command
                            }
                            ToggleLoop => {
                                let _ = sender.send(EngineCommand::ToggleLoop { deck: deck_idx });
                            }
                            ToggleSlip => {
                                let _ = sender.send(EngineCommand::ToggleSlip { deck: deck_idx });
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
                            ToggleStemMute(stem_idx) => {
                                if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                    let _ = sender.send(EngineCommand::ToggleStemMute { deck: deck_idx, stem });
                                }
                                // Toggle mute state in DeckView for UI
                                let was_muted = self.deck_views[deck_idx].is_stem_muted(stem_idx);
                                let new_muted = !was_muted;
                                self.deck_views[deck_idx].set_stem_muted(stem_idx, new_muted);

                                // stem_active = NOT muted (when muted, stem is inactive)
                                self.player_canvas_state.set_stem_active(deck_idx, stem_idx, !new_muted);
                            }
                            ToggleStemSolo(stem_idx) => {
                                if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                    let _ = sender.send(EngineCommand::ToggleStemSolo { deck: deck_idx, stem });
                                }
                                // Toggle solo state
                                let was_soloed = self.deck_views[deck_idx].is_stem_soloed(stem_idx);
                                let new_soloed = !was_soloed;

                                if new_soloed {
                                    // Solo: this stem becomes active, all others become inactive
                                    for i in 0..4 {
                                        self.deck_views[deck_idx].set_stem_soloed(i, i == stem_idx);
                                        // When soloing, set active state based on solo selection
                                        // (ignore mute state - solo overrides)
                                        self.player_canvas_state.set_stem_active(deck_idx, i, i == stem_idx);
                                    }
                                } else {
                                    // Un-solo: all stems become active (unless muted)
                                    self.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                                    for i in 0..4 {
                                        let is_muted = self.deck_views[deck_idx].is_stem_muted(i);
                                        self.player_canvas_state.set_stem_active(deck_idx, i, !is_muted);
                                    }
                                }
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
                        SetChannelEqHi(deck, value) => {
                            let _ = sender.send(EngineCommand::SetEqHi { deck: *deck, value: *value });
                        }
                        SetChannelEqMid(deck, value) => {
                            let _ = sender.send(EngineCommand::SetEqMid { deck: *deck, value: *value });
                        }
                        SetChannelEqLo(deck, value) => {
                            let _ = sender.send(EngineCommand::SetEqLo { deck: *deck, value: *value });
                        }
                        SetChannelFilter(deck, value) => {
                            let _ = sender.send(EngineCommand::SetFilter { deck: *deck, value: *value });
                        }
                        _ => {
                            // Master volume, cue volume, cue mix - not yet in engine
                        }
                    }
                }
                // Always update local UI state
                self.mixer_view.handle_local_message(mixer_msg);
                Task::none()
            }

            Message::CollectionBrowser(browser_msg) => {
                // Handle collection browser message and check if we need to load a track
                if let Some((deck_idx, path)) = self.collection_browser.handle_message(browser_msg) {
                    // Convert to LoadTrack message
                    let path_str = path.to_string_lossy().to_string();
                    return self.update(Message::LoadTrack(deck_idx, path_str));
                }
                Task::none()
            }

            Message::SetGlobalBpm(bpm) => {
                // Round to integer BPM for clean display and computation
                let bpm_rounded = bpm.round();
                self.global_bpm = bpm_rounded;
                // Send BPM change via lock-free command (~50ns)
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::SetGlobalBpm(bpm_rounded));
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
                    // Only allow seeking when deck is stopped (not playing or cueing)
                    if let Some(ref atomics) = self.deck_atomics {
                        let is_playing = atomics[deck_idx].is_playing();
                        let is_cueing = atomics[deck_idx].is_cueing();

                        if !is_playing && !is_cueing {
                            let duration = self.player_canvas_state.decks[deck_idx].overview.duration_samples;
                            if duration > 0 {
                                let seek_samples = (position * duration as f64) as usize;

                                // Send seek command to audio engine (lock-free)
                                if let Some(ref mut sender) = self.command_sender {
                                    let _ = sender.send(EngineCommand::Seek {
                                        deck: deck_idx,
                                        position: seek_samples,
                                    });
                                }
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::DeckSetZoom(deck_idx, bars) => {
                if deck_idx < 4 {
                    self.player_canvas_state.decks[deck_idx].zoomed.set_zoom(bars);
                }
                Task::none()
            }

            // Settings handlers
            Message::OpenSettings => {
                self.settings.is_open = true;
                self.settings = SettingsState::from_config(&self.config);
                self.settings.is_open = true;
                Task::none()
            }
            Message::CloseSettings => {
                self.settings.is_open = false;
                self.settings.status.clear();
                Task::none()
            }
            Message::UpdateSettingsLoopLength(index) => {
                self.settings.draft_loop_length_index = index;
                Task::none()
            }
            Message::UpdateSettingsZoomBars(bars) => {
                self.settings.draft_zoom_bars = bars;
                Task::none()
            }
            Message::UpdateSettingsGridBars(bars) => {
                self.settings.draft_grid_bars = bars;
                Task::none()
            }
            Message::UpdateSettingsPhaseSync(enabled) => {
                self.settings.draft_phase_sync = enabled;
                Task::none()
            }
            Message::SaveSettings => {
                // Apply draft settings to config
                let mut new_config = (*self.config).clone();
                new_config.display.default_loop_length_index = self.settings.draft_loop_length_index;
                new_config.display.default_zoom_bars = self.settings.draft_zoom_bars;
                new_config.display.grid_bars = self.settings.draft_grid_bars;
                // Save global BPM from current state
                new_config.audio.global_bpm = self.global_bpm;
                // Save phase sync setting
                new_config.audio.phase_sync = self.settings.draft_phase_sync;

                self.config = Arc::new(new_config.clone());

                // Send phase sync setting to audio engine immediately
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::SetPhaseSync(self.settings.draft_phase_sync));
                }

                // Save to disk in background
                let config_clone = new_config;
                let config_path = self.config_path.clone();
                Task::perform(
                    async move {
                        config::save_config(&config_clone, &config_path)
                            .map_err(|e| e.to_string())
                    },
                    Message::SaveSettingsComplete,
                )
            }
            Message::SaveSettingsComplete(result) => {
                match result {
                    Ok(()) => {
                        self.settings.status = "Settings saved".to_string();
                        self.status = "Settings saved".to_string();
                    }
                    Err(e) => {
                        self.settings.status = format!("Save failed: {}", e);
                        self.status = format!("Settings save failed: {}", e);
                    }
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

        // 3-column layout for controls + canvas:
        // Left: Decks 1 & 3 controls | Center: Waveform canvas | Right: Decks 2 & 4 controls

        // Left column: Deck 1 (top) and Deck 3 (bottom) controls with spacer
        use iced::widget::Space;
        let left_controls = column![
            self.deck_views[0].view_compact().map(|m| Message::Deck(0, m)),
            self.deck_views[2].view_compact().map(|m| Message::Deck(2, m)),
            Space::new().height(Length::Fixed(10.0)),
        ]
        .spacing(10)
        .width(Length::FillPortion(1));

        // Center column: Waveform canvas only
        let center_canvas = view_player_canvas(&self.player_canvas_state);
        let center_column = container(center_canvas)
            .width(Length::FillPortion(2));

        // Right column: Deck 2 (top) and Deck 4 (bottom) controls with spacer
        let right_controls = column![
            self.deck_views[1].view_compact().map(|m| Message::Deck(1, m)),
            self.deck_views[3].view_compact().map(|m| Message::Deck(3, m)),
            Space::new().height(Length::Fixed(10.0)),
        ]
        .spacing(10)
        .width(Length::FillPortion(1));

        // Main content area: controls | canvas | controls
        let main_row = row![
            left_controls,
            center_column,
            right_controls,
        ]
        .spacing(10);

        // Bottom row: Collection browser (left) | Mixer (right)
        let collection_browser = self.collection_browser.view().map(Message::CollectionBrowser);
        let mixer = self.mixer_view.view().map(Message::Mixer);
        let bottom_row = row![
            container(collection_browser).width(Length::FillPortion(3)),
            container(mixer).width(Length::FillPortion(2)),
        ]
        .spacing(10);

        // Status bar
        let status_bar = container(
            text(&self.status).size(12)
        )
        .padding(5);

        let content = column![
            header,
            main_row,
            bottom_row,
            status_bar,
        ]
        .spacing(10)
        .padding(10);

        let base: Element<Message> = container(content)
            .width(Fill)
            .height(Fill)
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

    /// View for the header/global controls
    fn view_header(&self) -> Element<Message> {
        let title = text("MESH DJ PLAYER")
            .size(24);

        let bpm_label = text(format!("BPM: {}", self.global_bpm as i32)).size(16);

        let bpm_slider = slider(30.0..=200.0, self.global_bpm, Message::SetGlobalBpm)
            .step(1.0)
            .width(200);

        let connection_status = if self.audio_connected {
            text("● JACK Connected").size(12)
        } else {
            text("○ JACK Disconnected").size(12)
        };

        // Settings gear icon (⚙ U+2699)
        let settings_btn = button(text("⚙").size(20))
            .on_press(Message::OpenSettings)
            .style(button::secondary);

        row![
            title,
            Space::new().width(Fill),
            bpm_label,
            bpm_slider,
            Space::new().width(Fill),
            connection_status,
            settings_btn,
        ]
        .spacing(20)
        .align_y(CenterAlign)
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
        // Default to 48kHz when no JACK rate is available (matches SAMPLE_RATE constant)
        Self::new(None, None, 48000)
    }
}
