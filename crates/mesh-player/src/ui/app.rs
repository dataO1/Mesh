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
use mesh_midi::{MidiController, MidiMessage as MidiMsg, MidiInputEvent, DeckAction as MidiDeckAction, MixerAction as MidiMixerAction, BrowserAction as MidiBrowserAction};
use mesh_core::audio_file::StemBuffers;
use mesh_core::engine::{DeckAtomics, EngineCommand, SlicerAtomics};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{PeaksComputer, PeaksComputeRequest, ZoomedViewMode, TRACK_TABLE_SCROLLABLE_ID, TRACK_ROW_HEIGHT};
use super::collection_browser::{CollectionBrowserState, CollectionBrowserMessage};
use super::deck_view::{DeckView, DeckMessage};
use super::midi_learn::{MidiLearnState, MidiLearnMessage, HighlightTarget};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};
use super::settings::SettingsState;

/// UI display mode - affects layout only, not engine behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Simplified layout: waveform canvas + browser only (for live performance)
    #[default]
    Performance,
    /// Full layout with deck controls and mixer (for MIDI mapping/configuration)
    Mapping,
}

/// Application state
pub struct MeshApp {
    /// Command sender for lock-free communication with audio engine
    /// Uses an SPSC ringbuffer - no mutex, no dropouts, guaranteed delivery
    command_sender: Option<CommandSender>,
    /// Lock-free deck state for UI reads (position, play state, loop)
    /// These atomics are updated by the audio thread; UI reads are wait-free
    deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    /// Lock-free slicer state for UI reads (drums stem slicer on all decks)
    slicer_atomics: Option<[Arc<SlicerAtomics>; NUM_DECKS]>,
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
    /// MIDI controller (optional - works without MIDI)
    midi_controller: Option<MidiController>,
    /// MIDI learn mode state
    midi_learn: MidiLearnState,
    /// UI display mode (performance vs mapping)
    app_mode: AppMode,
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
    /// Update settings: slicer buffer bars (4, 8, or 16)
    UpdateSettingsSlicerBufferBars(u32),
    /// Update settings: toggle slicer affected stem (stem_index, enabled)
    UpdateSettingsSlicerAffectedStem(usize, bool),
    /// Save settings to disk
    SaveSettings,
    /// Settings save complete
    SaveSettingsComplete(Result<(), String>),

    // MIDI Learn
    /// MIDI learn mode message
    MidiLearn(MidiLearnMessage),
}

impl MeshApp {
    /// Create a new application instance
    ///
    /// ## Parameters
    ///
    /// - `command_sender`: Lock-free command channel for engine control (None for offline mode)
    /// - `deck_atomics`: Lock-free position/state for UI reads (None for offline mode)
    /// - `slicer_atomics`: Lock-free slicer state for UI reads (None for offline mode)
    /// - `jack_sample_rate`: JACK's sample rate for track loading (e.g., 48000 or 44100)
    pub fn new(
        mut command_sender: Option<CommandSender>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
        slicer_atomics: Option<[Arc<SlicerAtomics>; NUM_DECKS]>,
        jack_sample_rate: u32,
        mapping_mode: bool,
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
            // Initialize slicer presets from config
            let _ = sender.send(EngineCommand::SetSlicerPresets {
                presets: Box::new(config.slicer.presets),
            });
            // Initialize slicer buffer size for all decks and stems
            let buffer_bars = config.slicer.buffer_bars();
            let stems = [
                mesh_core::types::Stem::Vocals,
                mesh_core::types::Stem::Drums,
                mesh_core::types::Stem::Bass,
                mesh_core::types::Stem::Other,
            ];
            for deck in 0..4 {
                for &stem in &stems {
                    let _ = sender.send(EngineCommand::SetSlicerBufferBars {
                        deck,
                        stem,
                        bars: buffer_bars,
                    });
                }
            }
        }

        let audio_connected = command_sender.is_some();

        // Initialize MIDI controller with raw event capture enabled (for MIDI learn mode)
        // Raw capture has minimal overhead when not actively reading
        let midi_controller = match MidiController::new_with_options(None, true) {
            Ok(controller) => {
                if controller.is_connected() {
                    log::info!("MIDI controller connected (raw capture enabled)");
                }
                Some(controller)
            }
            Err(e) => {
                log::warn!("MIDI not available: {}", e);
                None
            }
        };

        Self {
            command_sender,
            deck_atomics,
            slicer_atomics,
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
            midi_controller,
            midi_learn: MidiLearnState::new(),
            app_mode: if mapping_mode { AppMode::Mapping } else { AppMode::Performance },
        }
    }

    /// Start MIDI learn mode
    pub fn start_midi_learn(&mut self) {
        self.midi_learn.start();
    }

    /// Get highlight target for views to check
    pub fn midi_learn_highlight(&self) -> Option<HighlightTarget> {
        if self.midi_learn.is_active {
            self.midi_learn.highlight_target
        } else {
            None
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
                            // Clone key for engine command (before moving to canvas state)
                            let key_for_engine = if track_key.is_empty() { None } else { Some(track_key.clone()) };
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

                                // Send track key to engine for key matching
                                let _ = sender.send(EngineCommand::SetTrackKey {
                                    deck: deck_idx,
                                    key: key_for_engine.clone(),
                                });

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
                        zoomed.apply_computed_peaks(result);
                    }
                }

                // Poll MIDI input (non-blocking)
                // MIDI messages are processed at 60fps, providing ~16ms latency
                // Collect first to release borrow before calling handle_midi_message
                let midi_messages: Vec<_> = self
                    .midi_controller
                    .as_ref()
                    .map(|m| m.drain().collect())
                    .unwrap_or_default();
                for midi_msg in midi_messages {
                    self.handle_midi_message(midi_msg);
                }

                // MIDI Learn mode: capture raw events when waiting for input
                // This happens before normal MIDI routing so we can intercept events
                if self.midi_learn.is_active {
                    let needs_capture = match self.midi_learn.phase {
                        super::midi_learn::LearnPhase::Setup => {
                            // Only capture during ShiftButton step
                            self.midi_learn.setup_step == super::midi_learn::SetupStep::ShiftButton
                        }
                        super::midi_learn::LearnPhase::Review => false,
                        // All other phases need MIDI capture
                        _ => true,
                    };

                    if needs_capture {
                        if let Some(ref controller) = self.midi_controller {
                            // Drain raw events and find first valid one
                            for raw_event in controller.drain_raw_events() {
                                let captured = convert_midi_event_to_captured(&raw_event);

                                // Always update display so user sees what's happening
                                self.midi_learn.last_captured = Some(captured.clone());

                                // Check if this event should be captured (debounce + Note Off filter)
                                if !self.midi_learn.should_capture(&captured) {
                                    continue; // Skip this event, check next
                                }

                                // Mark capture time for debouncing
                                self.midi_learn.mark_captured();

                                // Handle based on current phase
                                if self.midi_learn.phase == super::midi_learn::LearnPhase::Setup {
                                    // Shift button detection - auto-advance
                                    self.midi_learn.shift_mapping = Some(captured);
                                    self.midi_learn.advance();
                                } else {
                                    // Mapping phase - record and advance
                                    self.midi_learn.record_mapping(captured);
                                }

                                // Only process one valid event per tick
                                break;
                            }
                        }
                    }
                }

                // Update highlight targets for MIDI learn mode
                // Each deck/mixer view needs to know if one of its elements should be highlighted
                let highlight = self.midi_learn.highlight_target;
                for i in 0..4 {
                    self.deck_views[i].set_highlight(highlight);
                }
                self.mixer_view.set_highlight(highlight);

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

                        // Update key matching state for header display
                        let key_match_enabled = atomics[i].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
                        let current_transpose = atomics[i].current_transpose.load(std::sync::atomic::Ordering::Relaxed);
                        self.player_canvas_state.set_key_match_enabled(i, key_match_enabled);
                        self.player_canvas_state.set_transpose(i, current_transpose);

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

                // Sync slicer state from atomics (LOCK-FREE - never blocks audio thread)
                // Updates slicer active state, queue, and current slice for UI display
                if let Some(ref slicer_atomics) = self.slicer_atomics {
                    for i in 0..4 {
                        let sa = &slicer_atomics[i];
                        let active = sa.active.load(std::sync::atomic::Ordering::Relaxed);
                        let current_slice = sa.current_slice.load(std::sync::atomic::Ordering::Relaxed);
                        let queue = sa.queue();

                        // Sync to deck view for button display
                        self.deck_views[i].sync_slicer_state(active, current_slice, queue);

                        // Sync to canvas for waveform overlay
                        let duration = self.player_canvas_state.decks[i].overview.duration_samples;
                        if active && duration > 0 {
                            let buffer_start = sa.buffer_start.load(std::sync::atomic::Ordering::Relaxed);
                            let buffer_end = sa.buffer_end.load(std::sync::atomic::Ordering::Relaxed);

                            // Convert to normalized positions
                            let start_norm = buffer_start as f64 / duration as f64;
                            let end_norm = buffer_end as f64 / duration as f64;

                            self.player_canvas_state.decks[i]
                                .overview
                                .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_slicer_region(Some((start_norm, end_norm)), Some(current_slice));
                            // Set fixed buffer view mode for slicer
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_fixed_buffer_bounds(Some((buffer_start as u64, buffer_end as u64)));
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_view_mode(ZoomedViewMode::FixedBuffer);
                            // Set zoom level based on slicer buffer size for optimal resolution
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_fixed_buffer_zoom(self.config.slicer.buffer_bars());
                        } else {
                            self.player_canvas_state.decks[i]
                                .overview
                                .set_slicer_region(None, None);
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_slicer_region(None, None);
                            // Restore scrolling view mode
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_fixed_buffer_bounds(None);
                            self.player_canvas_state.decks[i]
                                .zoomed
                                .set_view_mode(ZoomedViewMode::Scrolling);
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
                                    view_mode: zoomed.view_mode,
                                    fixed_buffer_bounds: zoomed.fixed_buffer_bounds,
                                });
                            }
                        }
                    }
                }

                // Update MIDI LED feedback (send state to controller LEDs)
                // Only runs if controller has output connection and feedback mappings
                if let Some(ref mut controller) = self.midi_controller {
                    let mut feedback = mesh_midi::FeedbackState::default();

                    for deck_idx in 0..4 {
                        // Get play state and loop active from atomics
                        if let Some(ref atomics) = self.deck_atomics {
                            feedback.decks[deck_idx].is_playing = atomics[deck_idx].is_playing();
                            feedback.decks[deck_idx].loop_active = atomics[deck_idx].loop_active();
                            feedback.decks[deck_idx].key_match_enabled =
                                atomics[deck_idx].key_match_enabled.load(std::sync::atomic::Ordering::Relaxed);
                        }

                        // Get slicer state
                        if let Some(ref slicer_atomics) = self.slicer_atomics {
                            feedback.decks[deck_idx].slicer_active =
                                slicer_atomics[deck_idx].active.load(std::sync::atomic::Ordering::Relaxed);
                            feedback.decks[deck_idx].slicer_current_slice =
                                slicer_atomics[deck_idx].current_slice.load(std::sync::atomic::Ordering::Relaxed);
                        }

                        // Get deck view state (hot cues, slip, stem mutes, action mode)
                        feedback.decks[deck_idx].hot_cues_set = self.deck_views[deck_idx].hot_cues_bitmap();
                        feedback.decks[deck_idx].slip_active = self.deck_views[deck_idx].slip_enabled();
                        feedback.decks[deck_idx].stems_muted = self.deck_views[deck_idx].stems_muted_bitmap();

                        // Set action mode for LED feedback
                        feedback.decks[deck_idx].action_mode = match self.deck_views[deck_idx].action_mode() {
                            super::deck_view::ActionButtonMode::HotCue => mesh_midi::ActionMode::HotCue,
                            super::deck_view::ActionButtonMode::Slicer => mesh_midi::ActionMode::Slicer,
                        };

                        // Get mixer cue (PFL) state
                        feedback.mixer[deck_idx].cue_enabled = self.mixer_view.cue_enabled(deck_idx);
                    }

                    controller.update_feedback(&feedback);
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
                            ToggleKeyMatch => {
                                // Toggle key matching for this deck
                                let current = self.deck_views[deck_idx].key_match_enabled();
                                let _ = sender.send(EngineCommand::SetKeyMatchEnabled { deck: deck_idx, enabled: !current });
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
                            // ─────────────────────────────────────────────────
                            // Slicer Mode Controls
                            // ─────────────────────────────────────────────────
                            SetActionMode(mode) => {
                                // Update UI state
                                self.deck_views[deck_idx].set_action_mode(mode);

                                // Enable/disable slicer based on mode for all affected stems
                                use crate::ui::deck_view::ActionButtonMode;
                                use mesh_core::types::Stem;
                                let affected_stems = self.config.slicer.affected_stems;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

                                match mode {
                                    ActionButtonMode::Slicer => {
                                        // Entering slicer mode - enable slicer processing for affected stems
                                        for (idx, &stem) in stems.iter().enumerate() {
                                            if affected_stems[idx] {
                                                let _ = sender.send(EngineCommand::SetSlicerEnabled {
                                                    deck: deck_idx,
                                                    stem,
                                                    enabled: true,
                                                });
                                            }
                                        }
                                    }
                                    ActionButtonMode::HotCue => {
                                        // Leaving slicer mode - disable processing but keep queue arrangement
                                        for &stem in &stems {
                                            let _ = sender.send(EngineCommand::SetSlicerEnabled {
                                                deck: deck_idx,
                                                stem,
                                                enabled: false,
                                            });
                                        }
                                    }
                                }
                            }
                            SlicerTrigger(button_idx) => {
                                // UI just reports button press - engine handles all behavior
                                use mesh_core::types::Stem;
                                let affected_stems = self.config.slicer.affected_stems;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
                                let shift_held = self.deck_views[deck_idx].shift_held();

                                for (idx, &stem) in stems.iter().enumerate() {
                                    if affected_stems[idx] {
                                        let _ = sender.send(EngineCommand::SlicerButtonAction {
                                            deck: deck_idx,
                                            stem,
                                            button_idx,
                                            shift_held,
                                        });
                                    }
                                }
                            }
                            ResetSlicerPattern => {
                                // Reset slicer queue to default [0..15]
                                use mesh_core::types::Stem;
                                let affected_stems = self.config.slicer.affected_stems;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

                                for (idx, &stem) in stems.iter().enumerate() {
                                    if affected_stems[idx] {
                                        let _ = sender.send(EngineCommand::SlicerResetQueue {
                                            deck: deck_idx,
                                            stem,
                                        });
                                    }
                                }
                            }
                            ShiftPressed => {
                                // UI-only state change
                                self.deck_views[deck_idx].set_shift_held(true);
                            }
                            ShiftReleased => {
                                // UI-only state change
                                self.deck_views[deck_idx].set_shift_held(false);
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
                // Check if this is a scroll message (for auto-scroll after)
                let is_scroll = matches!(browser_msg, CollectionBrowserMessage::ScrollBy(_));

                // Handle collection browser message and check if we need to load a track
                if let Some((deck_idx, path)) = self.collection_browser.handle_message(browser_msg) {
                    // Convert to LoadTrack message
                    let path_str = path.to_string_lossy().to_string();
                    return self.update(Message::LoadTrack(deck_idx, path_str));
                }

                // If it was a scroll, create a Task to auto-scroll the track list
                if is_scroll {
                    if let Some(selected_idx) = self.collection_browser.get_selected_index() {
                        // Calculate scroll offset to keep selection centered in view
                        // Assume ~10 visible rows; center selection with some margin
                        let visible_rows = 10.0_f32;
                        let center_offset = (visible_rows / 2.0 - 1.0) * TRACK_ROW_HEIGHT;
                        let target_y = (selected_idx as f32 * TRACK_ROW_HEIGHT - center_offset)
                            .max(0.0);

                        // Create scroll operation
                        use iced::widget::scrollable;
                        let offset = scrollable::AbsoluteOffset { x: 0.0, y: target_y };
                        let scroll_id = TRACK_TABLE_SCROLLABLE_ID.clone();

                        // Use iced's widget operation system to scroll
                        return iced::advanced::widget::operate(
                            iced::advanced::widget::operation::scrollable::scroll_to(
                                scroll_id.into(),
                                offset.into(),
                            )
                        );
                    }
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
            Message::UpdateSettingsSlicerBufferBars(bars) => {
                self.settings.draft_slicer_buffer_bars = bars;
                Task::none()
            }
            Message::UpdateSettingsSlicerAffectedStem(stem_idx, enabled) => {
                if stem_idx < 4 {
                    self.settings.draft_slicer_affected_stems[stem_idx] = enabled;
                }
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
                // Save slicer settings
                new_config.slicer.default_buffer_bars = self.settings.draft_slicer_buffer_bars;
                new_config.slicer.affected_stems = self.settings.draft_slicer_affected_stems;

                self.config = Arc::new(new_config.clone());

                // Send settings to audio engine immediately
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::SetPhaseSync(self.settings.draft_phase_sync));
                    // Send slicer buffer bars to audio engine for all decks and stems
                    let buffer_bars = new_config.slicer.buffer_bars();
                    let stems = [
                        mesh_core::types::Stem::Vocals,
                        mesh_core::types::Stem::Drums,
                        mesh_core::types::Stem::Bass,
                        mesh_core::types::Stem::Other,
                    ];
                    for deck in 0..4 {
                        for &stem in &stems {
                            let _ = sender.send(EngineCommand::SetSlicerBufferBars {
                                deck,
                                stem,
                                bars: buffer_bars,
                            });
                        }
                    }
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

            // MIDI Learn mode
            Message::MidiLearn(learn_msg) => {
                use MidiLearnMessage::*;
                match learn_msg {
                    Start => {
                        self.midi_learn.start();
                        // Close settings modal if open
                        self.settings.is_open = false;
                        self.status = "MIDI Learn mode started".to_string();
                    }
                    Cancel => {
                        self.midi_learn.cancel();
                        self.status = "MIDI Learn cancelled".to_string();
                    }
                    Next => {
                        self.midi_learn.advance();
                    }
                    Back => {
                        self.midi_learn.go_back();
                    }
                    Skip => {
                        self.midi_learn.advance();
                    }
                    Save => {
                        self.status = format!(
                            "Saving {} mappings for {}...",
                            self.midi_learn.pending_mappings.len(),
                            self.midi_learn.controller_name
                        );

                        // Generate the config from learned mappings
                        let config = self.midi_learn.generate_config();
                        let config_path = mesh_midi::default_midi_config_path();

                        // Save to disk in background
                        return Task::perform(
                            async move {
                                mesh_midi::save_midi_config(&config, &config_path)
                                    .map_err(|e| e.to_string())
                            },
                            |result| Message::MidiLearn(MidiLearnMessage::SaveComplete(result)),
                        );
                    }
                    SaveComplete(result) => {
                        match result {
                            Ok(()) => {
                                self.midi_learn.cancel(); // Reset state
                                self.status = "MIDI config saved! Reloading...".to_string();

                                // Reload MIDI controller with new config
                                // Drop old controller first to release the port
                                self.midi_controller = None;

                                // Create new controller with fresh config
                                match MidiController::new_with_options(None, true) {
                                    Ok(controller) => {
                                        if controller.is_connected() {
                                            log::info!("MIDI: Reloaded controller with new config");
                                            self.status = "MIDI config saved and loaded!".to_string();
                                        } else {
                                            self.status = "MIDI config saved (no device connected)".to_string();
                                        }
                                        self.midi_controller = Some(controller);
                                    }
                                    Err(e) => {
                                        log::warn!("MIDI: Failed to reload controller: {}", e);
                                        self.status = format!("Config saved, but reload failed: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                self.midi_learn.status = format!("Save failed: {}", e);
                                self.status = format!("MIDI config save failed: {}", e);
                            }
                        }
                    }
                    SetControllerName(name) => {
                        self.midi_learn.controller_name = name;
                    }
                    SetDeckCount(count) => {
                        self.midi_learn.deck_count = count;
                    }
                    SetHasLayerToggle(has) => {
                        self.midi_learn.has_layer_toggle = has;
                    }
                    SetPadModeSource(source) => {
                        self.midi_learn.pad_mode_source = source;
                    }
                    ShiftDetected(event) => {
                        self.midi_learn.shift_mapping = event;
                        self.midi_learn.advance();
                    }
                    MidiCaptured(event) => {
                        self.midi_learn.record_mapping(event);
                    }
                }
                Task::none()
            }
        }
    }

    /// Handle a MIDI message by dispatching to existing message handlers
    fn handle_midi_message(&mut self, msg: MidiMsg) {
        match msg {
            MidiMsg::Deck { deck, action } => {
                // Map MIDI deck actions to existing DeckMessages
                let deck_msg = match action {
                    MidiDeckAction::TogglePlay => Some(DeckMessage::TogglePlayPause),
                    MidiDeckAction::CuePress => Some(DeckMessage::CuePressed),
                    MidiDeckAction::CueRelease => Some(DeckMessage::CueReleased),
                    MidiDeckAction::Sync => None, // TODO: Add sync support
                    MidiDeckAction::HotCuePress { slot } => {
                        // Check pad mode source to determine routing
                        let pad_mode_source = self.midi_controller
                            .as_ref()
                            .map(|c| c.pad_mode_source())
                            .unwrap_or_default();

                        if pad_mode_source == mesh_midi::PadModeSource::Controller {
                            // Controller-driven: MIDI note already determined action
                            Some(DeckMessage::HotCuePressed(slot))
                        } else {
                            // App-driven: check current action mode
                            match self.deck_views[deck].action_mode() {
                                super::deck_view::ActionButtonMode::HotCue => Some(DeckMessage::HotCuePressed(slot)),
                                super::deck_view::ActionButtonMode::Slicer => Some(DeckMessage::SlicerTrigger(slot)),
                            }
                        }
                    }
                    MidiDeckAction::HotCueRelease { slot } => {
                        // Check pad mode source for release handling
                        let pad_mode_source = self.midi_controller
                            .as_ref()
                            .map(|c| c.pad_mode_source())
                            .unwrap_or_default();

                        if pad_mode_source == mesh_midi::PadModeSource::Controller {
                            // Controller-driven: always hot cue release
                            Some(DeckMessage::HotCueReleased(slot))
                        } else {
                            // App-driven: only release for hot cue mode
                            match self.deck_views[deck].action_mode() {
                                super::deck_view::ActionButtonMode::HotCue => Some(DeckMessage::HotCueReleased(slot)),
                                super::deck_view::ActionButtonMode::Slicer => None,
                            }
                        }
                    }
                    MidiDeckAction::HotCueClear { slot } => Some(DeckMessage::ClearHotCue(slot)),
                    MidiDeckAction::ToggleLoop => Some(DeckMessage::ToggleLoop),
                    MidiDeckAction::LoopHalve => Some(DeckMessage::LoopHalve),
                    MidiDeckAction::LoopDouble => Some(DeckMessage::LoopDouble),
                    MidiDeckAction::LoopIn | MidiDeckAction::LoopOut => None, // TODO
                    MidiDeckAction::BeatJumpForward => Some(DeckMessage::BeatJumpForward),
                    MidiDeckAction::BeatJumpBackward => Some(DeckMessage::BeatJumpBack),
                    MidiDeckAction::SlicerTrigger { pad } => Some(DeckMessage::SlicerTrigger(pad)),
                    MidiDeckAction::SlicerAssign { .. } => None, // TODO
                    MidiDeckAction::SetSlicerMode { enabled } => {
                        if enabled {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::Slicer))
                        } else {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::HotCue))
                        }
                    }
                    MidiDeckAction::SetHotCueMode { enabled } => {
                        if enabled {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::HotCue))
                        } else {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::Slicer))
                        }
                    }
                    MidiDeckAction::SlicerReset => Some(DeckMessage::ResetSlicerPattern),
                    MidiDeckAction::ToggleStemMute { stem } => Some(DeckMessage::ToggleStemMute(stem)),
                    MidiDeckAction::ToggleStemSolo { stem } => Some(DeckMessage::ToggleStemSolo(stem)),
                    MidiDeckAction::SelectStem { stem } => Some(DeckMessage::SelectStem(stem)),
                    MidiDeckAction::SetEffectParam { .. } => None, // TODO: Not implemented yet
                    MidiDeckAction::ToggleSlip => Some(DeckMessage::ToggleSlip),
                    MidiDeckAction::ToggleKeyMatch => Some(DeckMessage::ToggleKeyMatch),
                    MidiDeckAction::LoadSelected => {
                        // Load currently selected track in browser to this deck
                        if let Some(track_path) = self.collection_browser.get_selected_track_path() {
                            let _ = self.update(Message::LoadTrack(deck, track_path.to_string_lossy().to_string()));
                        }
                        None
                    }
                    MidiDeckAction::Seek { .. } | MidiDeckAction::Nudge { .. } => None, // TODO
                };

                if let Some(dm) = deck_msg {
                    let _ = self.update(Message::Deck(deck, dm));
                }
            }

            MidiMsg::Mixer { channel, action } => {
                let mixer_msg = match action {
                    MidiMixerAction::SetVolume(v) => Some(MixerMessage::SetChannelVolume(channel, v)),
                    MidiMixerAction::SetFilter(v) => Some(MixerMessage::SetChannelFilter(channel, v)),
                    MidiMixerAction::SetEqHi(v) => Some(MixerMessage::SetChannelEqHi(channel, v)),
                    MidiMixerAction::SetEqMid(v) => Some(MixerMessage::SetChannelEqMid(channel, v)),
                    MidiMixerAction::SetEqLo(v) => Some(MixerMessage::SetChannelEqLo(channel, v)),
                    MidiMixerAction::ToggleCue => Some(MixerMessage::ToggleChannelCue(channel)),
                    MidiMixerAction::SetCrossfader(_) => None, // No crossfader in mesh-player
                };

                if let Some(mm) = mixer_msg {
                    let _ = self.update(Message::Mixer(mm));
                }
            }

            MidiMsg::Browser(action) => {
                match action {
                    MidiBrowserAction::Scroll { delta } => {
                        // Scroll the collection browser selection by delta
                        let _ = self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::ScrollBy(delta),
                        ));
                    }
                    MidiBrowserAction::Select => {
                        // Select current item (activates track -> loads to deck 0)
                        let _ = self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::SelectCurrent,
                        ));
                    }
                    MidiBrowserAction::Back => {
                        // Could implement folder navigation back, for now just log
                        log::debug!("MIDI: Browser back (not implemented)");
                    }
                }
            }

            MidiMsg::Global(action) => {
                use mesh_midi::GlobalAction as MidiGlobalAction;
                match action {
                    MidiGlobalAction::SetMasterVolume(v) => {
                        let _ = self.update(Message::Mixer(MixerMessage::SetMasterVolume(v)));
                    }
                    MidiGlobalAction::SetCueVolume(v) => {
                        let _ = self.update(Message::Mixer(MixerMessage::SetCueVolume(v)));
                    }
                    MidiGlobalAction::SetCueMix(v) => {
                        let _ = self.update(Message::Mixer(MixerMessage::SetCueMix(v)));
                    }
                    MidiGlobalAction::SetBpm(_) | MidiGlobalAction::AdjustBpm(_) => {
                        // BPM control not implemented yet
                    }
                }
            }

            MidiMsg::LayerToggle { physical_deck } => {
                // Handled internally by MidiController
                log::debug!("MIDI: Layer toggle for physical deck {}", physical_deck);
            }

            MidiMsg::ShiftChanged { held } => {
                // Update deck views' shift state
                for deck_view in &mut self.deck_views {
                    deck_view.set_shift_held(held);
                }
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
        // Build base content based on app mode
        let base = match self.app_mode {
            AppMode::Performance => self.view_performance_mode(),
            AppMode::Mapping => self.view_mapping_mode(),
        };

        // Apply overlays (MIDI learn drawer and settings modal) - same for both modes
        self.apply_overlays(base)
    }

    /// Performance mode: simplified layout with canvas (~60%) + browser (~40%)
    /// Waveform heights are fractional for clean display on Full HD and UHD screens
    /// For live performance with MIDI controller - no load buttons
    fn view_performance_mode(&self) -> Element<Message> {
        let header = self.view_header();
        let canvas = view_player_canvas(&self.player_canvas_state);
        // Use compact browser (no load buttons) for performance mode
        let browser = self.collection_browser.view_compact().map(Message::CollectionBrowser);
        let status_bar = container(text(&self.status).size(12)).padding(5);

        column![
            header,
            container(canvas)
                .width(Length::Fill)
                .height(Length::FillPortion(11)),  // ~55% (11/20)
            container(browser)
                .width(Length::Fill)
                .height(Length::FillPortion(9)),   // ~45% (9/20)
            status_bar,
        ]
        .spacing(8)
        .padding(10)
        .into()
    }

    /// Mapping mode: full 3-column layout with deck controls and mixer
    /// For MIDI mapping/configuration
    fn view_mapping_mode(&self) -> Element<Message> {
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

        column![
            header,
            main_row,
            bottom_row,
            status_bar,
        ]
        .spacing(10)
        .padding(10)
        .into()
    }

    /// Apply overlays (MIDI learn drawer and settings modal) to base content
    fn apply_overlays<'a>(&'a self, base: Element<'a, Message>) -> Element<'a, Message> {
        use iced::widget::Space;

        let base: Element<'a, Message> = container(base)
            .width(Fill)
            .height(Fill)
            .into();

        // MIDI Learn drawer at the bottom (if active)
        let with_drawer: Element<'a, Message> = if self.midi_learn.is_active {
            let drawer = super::midi_learn::view_drawer(&self.midi_learn)
                .map(Message::MidiLearn);

            // Use a column to put the drawer at the bottom
            let main_with_drawer = column![
                container(base).height(Length::FillPortion(1)),
                drawer,
            ]
            .width(Length::Fill)
            .height(Length::Fill);

            main_with_drawer.into()
        } else {
            base
        };

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

            stack![with_drawer, backdrop, modal].into()
        } else {
            with_drawer
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
        // Default to performance mode
        Self::new(None, None, None, 48000, false)
    }
}

/// Convert a raw MidiInputEvent to CapturedMidiEvent for learn mode
fn convert_midi_event_to_captured(event: &MidiInputEvent) -> super::midi_learn::CapturedMidiEvent {
    use super::midi_learn::CapturedMidiEvent;

    match event {
        MidiInputEvent::NoteOn { channel, note, velocity } => CapturedMidiEvent {
            channel: *channel,
            number: *note,
            value: *velocity,
            is_note: true,
        },
        MidiInputEvent::NoteOff { channel, note, velocity } => CapturedMidiEvent {
            channel: *channel,
            number: *note,
            value: *velocity,
            is_note: true,
        },
        MidiInputEvent::ControlChange { channel, cc, value } => CapturedMidiEvent {
            channel: *channel,
            number: *cc,
            value: *value,
            is_note: false,
        },
    }
}
