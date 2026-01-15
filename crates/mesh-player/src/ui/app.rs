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
use mesh_core::engine::{DeckAtomics, EngineCommand, LinkedStemAtomics, SlicerAtomics};
use mesh_core::types::{StereoBuffer, NUM_DECKS};
use mesh_widgets::{
    mpsc_subscription, PeaksComputer, PeaksComputeRequest, SliceEditorState, ZoomedViewMode,
    TRACK_ROW_HEIGHT, TRACK_TABLE_SCROLLABLE_ID,
};
use super::collection_browser::{CollectionBrowserState, CollectionBrowserMessage};
use super::deck_view::{DeckView, DeckMessage};
use super::midi_learn::{MidiLearnState, MidiLearnMessage, HighlightTarget};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};
use super::settings::SettingsState;

// Re-export extracted modules for use by other UI modules
pub use super::message::Message;
pub use super::state::{AppMode, LinkedStemLoadedMsg, StemLinkState, TrackLoadedMsg};

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
    /// Lock-free linked stem state for UI reads (which stems have links)
    linked_stem_atomics: Option<[Arc<LinkedStemAtomics>; NUM_DECKS]>,
    /// Background track loader (avoids blocking UI/audio during loads)
    track_loader: TrackLoader,
    /// Background peak computer (offloads expensive waveform peak computation)
    peaks_computer: PeaksComputer,
    /// Unified waveform state for all 4 decks
    player_canvas_state: PlayerCanvasState,
    /// Stem buffers for waveform recomputation (Shared for RT-safe deallocation)
    deck_stems: [Option<Shared<StemBuffers>>; 4],
    /// Linked stem buffers per deck per stem [deck_idx][stem_idx]
    /// Used for zoomed waveform visualization of active linked stems
    deck_linked_stems: [[Option<Shared<StereoBuffer>>; 4]; 4],
    /// Track LUFS per deck (cached from TrackLoaded for LinkedStemLoaded)
    /// Used to avoid race conditions when passing host_lufs to LinkStem command
    track_lufs_per_deck: [Option<f32>; 4],
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
    /// Linked stem selection state machine
    stem_link_state: StemLinkState,
    /// Slice editor state (shared presets and per-stem patterns)
    slice_editor: SliceEditorState,
    /// Linked stem result receiver (engine owns the loader, we just receive results)
    linked_stem_receiver: Option<mesh_core::loader::LinkedStemResultReceiver>,
    /// USB manager for device detection and browsing
    usb_manager: mesh_core::usb::UsbManager,
}

// Message enum moved to message.rs

impl MeshApp {
    /// Create a new application instance
    ///
    /// ## Parameters
    ///
    /// - `command_sender`: Lock-free command channel for engine control (None for offline mode)
    /// - `deck_atomics`: Lock-free position/state for UI reads (None for offline mode)
    /// - `slicer_atomics`: Lock-free slicer state for UI reads (None for offline mode)
    /// - `linked_stem_atomics`: Lock-free linked stem state for UI reads (None for offline mode)
    /// - `linked_stem_receiver`: Receiver for linked stem load results (engine owns the loader)
    /// - `jack_sample_rate`: JACK's sample rate for track loading (e.g., 48000 or 44100)
    pub fn new(
        mut command_sender: Option<CommandSender>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
        slicer_atomics: Option<[Arc<SlicerAtomics>; NUM_DECKS]>,
        linked_stem_atomics: Option<[Arc<LinkedStemAtomics>; NUM_DECKS]>,
        linked_stem_receiver: Option<mesh_core::loader::LinkedStemResultReceiver>,
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
            // Initialize slicer presets from shared presets file (same source as UI)
            let slicer_config = mesh_widgets::load_slicer_presets(&config.collection_path);
            let _ = sender.send(EngineCommand::SetSlicerPresets {
                presets: Box::new(config::slicer_config_to_engine_presets(&slicer_config)),
            });
            // Initialize slicer buffer size for all decks and stems
            let buffer_bars = slicer_config.validated_buffer_bars();
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
            // Initialize loudness config (engine calculates LUFS gain automatically)
            let _ = sender.send(EngineCommand::SetLoudnessConfig(config.audio.loudness.clone()));
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
            linked_stem_atomics,
            track_loader: TrackLoader::spawn(jack_sample_rate),
            peaks_computer: PeaksComputer::spawn(),
            player_canvas_state: {
                let mut state = PlayerCanvasState::new();
                state.set_stem_colors(config.display.stem_color_palette.colors());
                state
            },
            deck_stems: [None, None, None, None],
            deck_linked_stems: std::array::from_fn(|_| [None, None, None, None]),
            track_lufs_per_deck: [None, None, None, None],
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
            slice_editor: {
                // Load presets from dedicated file (shared with mesh-cue, read-only)
                let slicer_config = mesh_widgets::load_slicer_presets(&config.collection_path);
                let mut state = SliceEditorState::default();
                slicer_config.apply_to_editor_state(&mut state);
                state
            },
            config,
            config_path,
            settings,
            midi_controller,
            midi_learn: MidiLearnState::new(),
            app_mode: if mapping_mode { AppMode::Mapping } else { AppMode::Performance },
            stem_link_state: StemLinkState::Idle,
            linked_stem_receiver,
            usb_manager: mesh_core::usb::UsbManager::spawn(),
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
                            // Check if we're in hardware detection mode (sampling in progress)
                            let sampling_active = self.midi_learn.detection_buffer.is_some();
                            // Check if we're waiting for encoder press
                            let awaiting_encoder_press = self.midi_learn.awaiting_encoder_press;

                            // Drain raw events
                            for raw_event in controller.drain_raw_events() {
                                let captured = convert_midi_event_to_captured(&raw_event);

                                // Always update display so user sees what's happening
                                self.midi_learn.last_captured = Some(captured.clone());

                                if sampling_active {
                                    // Add sample to detection buffer
                                    if self.midi_learn.add_detection_sample(&captured) {
                                        // Buffer is complete - finalize mapping
                                        self.midi_learn.finalize_mapping();
                                        break;
                                    }
                                } else if awaiting_encoder_press {
                                    // Waiting for encoder press - capture button event
                                    // Check if this event should be captured (debounce + Note Off filter)
                                    if !self.midi_learn.should_capture(&captured) {
                                        continue; // Skip this event, check next
                                    }

                                    // Record the encoder press and advance
                                    self.midi_learn.record_encoder_press(captured);
                                    break;
                                } else {
                                    // Not sampling yet - check if we should start

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
                                        // Mapping phase - start hardware detection
                                        // record_mapping creates buffer, adds first sample
                                        // For buttons (Note events), it completes immediately
                                        self.midi_learn.record_mapping(captured);
                                    }

                                    // Only start one capture per tick
                                    break;
                                }
                            }

                            // Check if detection timed out (1 second elapsed)
                            if self.midi_learn.is_detection_complete() {
                                self.midi_learn.finalize_mapping();
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

                        // Sync LUFS gain from engine for waveform scaling (single source of truth)
                        let lufs_gain = atomics[i].lufs_gain();
                        self.player_canvas_state.decks[i].zoomed.set_lufs_gain(lufs_gain);

                        // Update gain display in deck header (dB)
                        let lufs_gain_db = if (lufs_gain - 1.0).abs() < 0.001 {
                            None // No gain compensation (unity or disabled)
                        } else {
                            Some(20.0 * lufs_gain.log10())
                        };
                        self.player_canvas_state.set_lufs_gain_db(i, lufs_gain_db);

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
                                .set_fixed_buffer_zoom(self.config.slicer.validated_buffer_bars());
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

                // Sync linked stem state from atomics (LOCK-FREE - never blocks audio thread)
                // Updates which stems have links and whether links are active for UI display
                if let Some(ref linked_atomics) = self.linked_stem_atomics {
                    for i in 0..4 {
                        let la = &linked_atomics[i];
                        for stem_idx in 0..4 {
                            let has_linked = la.has_linked[stem_idx].load(std::sync::atomic::Ordering::Relaxed);
                            let is_active = la.use_linked[stem_idx].load(std::sync::atomic::Ordering::Relaxed);
                            self.player_canvas_state.set_linked_stem(i, stem_idx, has_linked, is_active);
                        }
                    }
                }

                // Request zoomed waveform peak recomputation in background thread
                // This expensive operation (10-50ms) is fully async - UI never blocks
                for i in 0..4 {
                    if let Some(position) = deck_positions[i] {
                        // Get linked stem active state from atomics (needed for cache invalidation check)
                        let linked_active = if let Some(ref linked_atomics) = self.linked_stem_atomics {
                            let la = &linked_atomics[i];
                            [
                                la.has_linked[0].load(std::sync::atomic::Ordering::Relaxed)
                                    && la.use_linked[0].load(std::sync::atomic::Ordering::Relaxed),
                                la.has_linked[1].load(std::sync::atomic::Ordering::Relaxed)
                                    && la.use_linked[1].load(std::sync::atomic::Ordering::Relaxed),
                                la.has_linked[2].load(std::sync::atomic::Ordering::Relaxed)
                                    && la.use_linked[2].load(std::sync::atomic::Ordering::Relaxed),
                                la.has_linked[3].load(std::sync::atomic::Ordering::Relaxed)
                                    && la.use_linked[3].load(std::sync::atomic::Ordering::Relaxed),
                            ]
                        } else {
                            [false, false, false, false]
                        };

                        let zoomed = &self.player_canvas_state.decks[i].zoomed;
                        if zoomed.needs_recompute(position, &linked_active) && zoomed.has_track {
                            if let Some(ref stems) = self.deck_stems[i] {
                                // Clone linked stem buffer references (cheap Shared clone)
                                let linked_stems = [
                                    self.deck_linked_stems[i][0].clone(),
                                    self.deck_linked_stems[i][1].clone(),
                                    self.deck_linked_stems[i][2].clone(),
                                    self.deck_linked_stems[i][3].clone(),
                                ];

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
                                    linked_stems,
                                    linked_active,
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

            Message::TrackLoaded(msg) => {
                // Extract the result from Arc wrapper
                // We use Arc::try_unwrap to get ownership if we're the sole owner,
                // otherwise we need to handle the shared case
                let result = match Arc::try_unwrap(msg.0) {
                    Ok(r) => r,
                    Err(_arc) => {
                        // Still shared - this shouldn't happen in practice since
                        // subscriptions deliver to one handler, but handle gracefully
                        log::warn!("TrackLoadResult Arc still shared, skipping");
                        return Task::none();
                    }
                };

                let deck_idx = result.deck_idx;

                match result.result {
                    Ok(prepared) => {
                        let track = &prepared.track;
                        let filename = track.path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                            .to_string();

                        log::info!(
                            "[TRACK] Loaded {} to deck {} ({} samples)",
                            filename,
                            deck_idx + 1,
                            track.duration_samples
                        );

                        // Apply pre-computed waveform states (expensive work already done in loader)
                        self.player_canvas_state.decks[deck_idx].overview = result.overview_state;
                        self.player_canvas_state.decks[deck_idx].zoomed = result.zoomed_state;

                        // Store stem buffers for potential waveform recomputation
                        self.deck_stems[deck_idx] = Some(result.stems);

                        // Set track info on player canvas state (for header display)
                        self.player_canvas_state.set_track_name(deck_idx, filename.clone());
                        self.player_canvas_state.set_track_key(
                            deck_idx,
                            track.metadata.key.clone().unwrap_or_default()
                        );
                        self.player_canvas_state.set_track_bpm(deck_idx, track.metadata.bpm);
                        // Store track LUFS for use in LinkedStemLoaded (avoids race conditions)
                        self.track_lufs_per_deck[deck_idx] = track.metadata.lufs;
                        // LUFS gain is calculated by engine and synced via atomics in Tick

                        // Sync hot cues to deck view for display
                        for (slot, hot_cue) in prepared.hot_cues.iter().enumerate() {
                            self.deck_views[deck_idx].set_hot_cue_position(
                                slot,
                                hot_cue.as_ref().map(|hc| hc.position as u64)
                            );
                        }

                        // Reset stem mute/solo state for the deck (all stems active)
                        for stem_idx in 0..4 {
                            self.deck_views[deck_idx].set_stem_muted(stem_idx, false);
                            self.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                            self.player_canvas_state.set_stem_active(deck_idx, stem_idx, true);
                            // Clear any linked stems from previous track
                            self.player_canvas_state.set_linked_stem(deck_idx, stem_idx, false, false);
                            self.deck_linked_stems[deck_idx][stem_idx] = None;
                        }

                        // Send prepared track to audio engine via lock-free command
                        if let Some(ref mut sender) = self.command_sender {
                            let _ = sender.send(EngineCommand::LoadTrack {
                                deck: deck_idx,
                                track: Box::new(prepared),
                            });
                        }

                        self.status = format!("Loaded {} to deck {}", filename, deck_idx + 1);
                    }
                    Err(e) => {
                        log::error!("Failed to load track to deck {}: {}", deck_idx + 1, e);
                        self.status = format!("Error loading track: {}", e);
                    }
                }

                Task::none()
            }

            Message::PeaksComputed(result) => {
                // Apply computed peaks to the appropriate deck's zoomed waveform state
                if result.id < 4 {
                    let zoomed = &mut self.player_canvas_state.decks[result.id].zoomed;
                    zoomed.apply_computed_peaks(result);
                }
                Task::none()
            }

            Message::LinkedStemLoaded(msg) => {
                // Extract the result from Arc wrapper
                let linked_result = match Arc::try_unwrap(msg.0) {
                    Ok(r) => r,
                    Err(_) => {
                        log::warn!("LinkedStemLoadResult Arc still shared, skipping");
                        return Task::none();
                    }
                };

                let deck_idx = linked_result.host_deck_idx;
                let stem_idx = linked_result.stem_idx;

                match linked_result.result {
                    Ok(linked_data) => {
                        // Store shared buffer reference for zoomed waveform visualization
                        if let Some(shared_buffer) = linked_result.shared_buffer {
                            log::info!(
                                "[LINKED] Storing shared buffer for deck {} stem {} ({} samples)",
                                deck_idx,
                                stem_idx,
                                shared_buffer.len()
                            );
                            self.deck_linked_stems[deck_idx][stem_idx] = Some(shared_buffer);
                        }

                        // Store linked stem overview peaks in waveform state for visualization
                        if let Some(peaks) = linked_result.overview_peaks {
                            log::info!(
                                "[LINKED] Storing {} overview peaks for deck {} stem {}",
                                peaks.len(),
                                deck_idx,
                                stem_idx
                            );
                            self.player_canvas_state
                                .deck_mut(deck_idx)
                                .overview
                                .set_linked_stem_peaks(stem_idx, peaks);
                        }

                        // Store linked stem highres peaks for stable zoomed view rendering
                        if let Some(peaks) = linked_result.highres_peaks {
                            log::info!(
                                "[LINKED] Storing {} highres peaks for deck {} stem {}",
                                peaks.len(),
                                deck_idx,
                                stem_idx
                            );
                            self.player_canvas_state
                                .deck_mut(deck_idx)
                                .overview
                                .set_linked_highres_peaks(stem_idx, peaks);
                        }

                        // Store linked stem metadata for split-view alignment
                        // Use STRETCHED values to match audio engine alignment
                        if let Some(stretched_duration) = linked_result.linked_duration {
                            let host_duration = self.player_canvas_state.decks[deck_idx].overview.duration_samples;
                            let host_drop = self.player_canvas_state.decks[deck_idx].overview.drop_marker;
                            log::info!(
                                "[LINKED] Visual alignment for deck {} stem {}: stretched_drop={}, stretched_dur={}, host_drop={:?}, host_dur={}, ratio={:.3}",
                                deck_idx,
                                stem_idx,
                                linked_data.drop_marker,
                                stretched_duration,
                                host_drop,
                                host_duration,
                                stretched_duration as f64 / host_duration as f64
                            );
                            self.player_canvas_state
                                .deck_mut(deck_idx)
                                .overview
                                .set_linked_stem_metadata(
                                    stem_idx,
                                    linked_data.drop_marker,
                                    stretched_duration,
                                );
                        }

                        // Calculate LUFS gain for linked stem waveform
                        let linked_gain = self.config.audio.loudness.calculate_gain_linear(linked_data.lufs);
                        self.player_canvas_state
                            .deck_mut(deck_idx)
                            .overview
                            .set_linked_lufs_gain(stem_idx, linked_gain);
                        log::info!(
                            "[LINKED] Set LUFS gain for deck {} stem {}: linked_lufs={:?}, gain={:.3} ({:+.1}dB)",
                            deck_idx,
                            stem_idx,
                            linked_data.lufs,
                            linked_gain,
                            20.0 * linked_gain.log10()
                        );

                        // Mark stem as having a linked stem (enables split-view)
                        self.player_canvas_state.set_linked_stem(deck_idx, stem_idx, true, false);

                        // Send linked stem to audio engine
                        if let Some(ref mut sender) = self.command_sender {
                            if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                                let track_name = linked_data.track_name.clone();
                                // Pass host_lufs explicitly to avoid race conditions
                                // (deck.host_lufs in engine might be stale)
                                let host_lufs = self.track_lufs_per_deck[deck_idx];
                                let _ = sender.send(EngineCommand::LinkStem {
                                    deck: deck_idx,
                                    stem,
                                    linked_stem: Box::new(linked_data),
                                    host_lufs,
                                });
                                self.status = format!(
                                    "Linked {} stem on deck {} from {}",
                                    stem.name(),
                                    deck_idx + 1,
                                    track_name
                                );
                            }
                        }

                        // Transition from Loading to Idle - linked stem is ready
                        if matches!(
                            self.stem_link_state,
                            StemLinkState::Loading { deck, stem, .. }
                            if deck == deck_idx && stem == stem_idx
                        ) {
                            self.stem_link_state = StemLinkState::Idle;
                            log::info!(
                                "Linked stem ready: deck={}, stem={} - shift+stem to toggle",
                                deck_idx,
                                stem_idx
                            );
                        }
                    }
                    Err(e) => {
                        self.status = format!("Error loading linked stem: {}", e);
                        self.stem_link_state = StemLinkState::Idle;
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
                                let shift_held = self.deck_views[deck_idx].shift_held();
                                log::info!(
                                    "[STEM_TOGGLE] Stem button pressed: deck={}, stem={}, shift_held={}",
                                    deck_idx, stem_idx, shift_held
                                );

                                if shift_held {
                                    // Shift+Stem: Linked stem operation
                                    self.handle_shift_stem(deck_idx, stem_idx);
                                } else {
                                    // Normal: Toggle mute
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

                                // Enable/disable slicer based on mode for stems with patterns
                                use crate::ui::deck_view::ActionButtonMode;
                                use mesh_core::types::Stem;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
                                let preset = &self.slice_editor.presets[self.slice_editor.selected_preset];

                                match mode {
                                    ActionButtonMode::Slicer => {
                                        // Entering slicer mode - enable slicer for stems with patterns
                                        for (idx, &stem) in stems.iter().enumerate() {
                                            if preset.stems[idx].is_some() {
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
                            SlicerPresetSelect(preset_idx) => {
                                // Select a new slicer preset via engine's button action
                                // The engine handles enabling slicers and loading patterns
                                use mesh_core::types::Stem;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

                                // Update selected preset in slice editor state
                                self.slice_editor.selected_preset = preset_idx;

                                // Send button action to engine for each stem (shift_held=false = load preset)
                                let preset = &self.slice_editor.presets[preset_idx];
                                for (idx, &stem) in stems.iter().enumerate() {
                                    if preset.stems[idx].is_some() {
                                        let _ = sender.send(EngineCommand::SlicerButtonAction {
                                            deck: deck_idx,
                                            stem,
                                            button_idx: preset_idx,
                                            shift_held: false, // Load preset mode
                                        });
                                    }
                                }
                            }
                            SlicerTrigger(button_idx) => {
                                // Shift+click triggers slice for live queue adjustment
                                use mesh_core::types::Stem;
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
                                let shift_held = self.deck_views[deck_idx].shift_held();
                                let preset = &self.slice_editor.presets[self.slice_editor.selected_preset];

                                for (idx, &stem) in stems.iter().enumerate() {
                                    if preset.stems[idx].is_some() {
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
                                let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
                                let preset = &self.slice_editor.presets[self.slice_editor.selected_preset];

                                for (idx, &stem) in stems.iter().enumerate() {
                                    if preset.stems[idx].is_some() {
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
            Message::UpdateSettingsStemColorPalette(palette) => {
                self.settings.draft_stem_color_palette = palette;
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
            Message::UpdateSettingsAutoGainEnabled(enabled) => {
                self.settings.draft_auto_gain_enabled = enabled;
                Task::none()
            }
            Message::UpdateSettingsTargetLufs(index) => {
                self.settings.draft_target_lufs_index = index;
                Task::none()
            }
            Message::SaveSettings => {
                // Apply draft settings to config
                let mut new_config = (*self.config).clone();
                new_config.display.default_loop_length_index = self.settings.draft_loop_length_index;
                new_config.display.default_zoom_bars = self.settings.draft_zoom_bars;
                new_config.display.grid_bars = self.settings.draft_grid_bars;
                new_config.display.stem_color_palette = self.settings.draft_stem_color_palette;
                // Save global BPM from current state
                new_config.audio.global_bpm = self.global_bpm;
                // Save phase sync setting
                new_config.audio.phase_sync = self.settings.draft_phase_sync;
                // Save only buffer_bars (presets are read-only from shared file)
                new_config.slicer.buffer_bars = self.settings.draft_slicer_buffer_bars;
                // Save loudness settings
                new_config.audio.loudness.auto_gain_enabled = self.settings.draft_auto_gain_enabled;
                new_config.audio.loudness.target_lufs = self.settings.target_lufs();

                self.config = Arc::new(new_config.clone());

                // Apply stem color palette to waveform display immediately
                self.player_canvas_state.set_stem_colors(
                    self.settings.draft_stem_color_palette.colors()
                );

                // Send settings to audio engine immediately
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::SetPhaseSync(self.settings.draft_phase_sync));
                    // Send loudness config to engine (triggers recalculation for all loaded decks)
                    let _ = sender.send(EngineCommand::SetLoudnessConfig(
                        self.config.audio.loudness.clone()
                    ));
                    // Send slicer buffer bars to audio engine for all decks and stems
                    let buffer_bars = new_config.slicer.validated_buffer_bars();
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
                        if self.midi_learn.awaiting_encoder_press {
                            self.midi_learn.skip_encoder_press();
                        } else {
                            self.midi_learn.advance();
                        }
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

            // USB device messages
            Message::Usb(usb_msg) => {
                use mesh_core::usb::UsbMessage as UsbMsg;
                match usb_msg {
                    UsbMsg::DevicesRefreshed(devices) => {
                        log::info!("USB: {} devices detected", devices.len());
                        self.collection_browser.update_usb_devices(devices.clone());
                        // Initialize storages for mounted devices
                        for device in &devices {
                            if device.mount_point.is_some() && device.has_mesh_collection {
                                self.collection_browser.init_usb_storage(device);
                            }
                        }
                    }
                    UsbMsg::DeviceConnected(device) => {
                        self.collection_browser.add_usb_device(device.clone());
                        if device.mount_point.is_some() && device.has_mesh_collection {
                            self.collection_browser.init_usb_storage(&device);
                        }
                        self.status = format!("USB: {} connected", device.label);
                    }
                    UsbMsg::DeviceDisconnected { device_path } => {
                        self.collection_browser.remove_usb_device(&device_path);
                        self.status = "USB: Device disconnected".to_string();
                    }
                    UsbMsg::MountComplete { result } => {
                        match result {
                            Ok(device) => {
                                // Update device in browser and init storage if it has mesh collection
                                self.collection_browser.add_usb_device(device.clone());
                                if device.has_mesh_collection {
                                    self.collection_browser.init_usb_storage(&device);
                                }
                            }
                            Err(e) => {
                                log::warn!("USB mount failed: {}", e);
                            }
                        }
                    }
                    _ => {
                        // Ignore export-related messages in mesh-player (read-only)
                    }
                }
                Task::none()
            }
        }
    }

    /// Handle shift+stem gesture for linked stem operations
    ///
    /// When shift is held and a stem button is pressed:
    /// - If already in Selecting mode for same deck/stem: cancel selection
    /// - If a linked stem exists: toggle between original/linked
    /// - Otherwise: enter Selecting mode for browser track selection
    fn handle_shift_stem(&mut self, deck_idx: usize, stem_idx: usize) {
        // Check if we have atomics to query linked stem state
        // Read from LinkedStemAtomics (lock-free, ~5ns)
        let has_linked = self.linked_stem_atomics.as_ref().map_or(false, |atomics| {
            atomics[deck_idx].has_linked[stem_idx].load(std::sync::atomic::Ordering::Relaxed)
        });

        log::info!(
            "[STEM_TOGGLE] handle_shift_stem: deck={}, stem={}, has_linked={}, state={:?}",
            deck_idx, stem_idx, has_linked, self.stem_link_state
        );

        match &self.stem_link_state {
            StemLinkState::Idle => {
                if has_linked {
                    // Toggle existing linked stem
                    if let Some(ref mut sender) = self.command_sender {
                        if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                            log::info!(
                                "[STEM_TOGGLE] Sending ToggleLinkedStem: deck={}, stem={:?}",
                                deck_idx, stem
                            );
                            let _ = sender.send(EngineCommand::ToggleLinkedStem {
                                deck: deck_idx,
                                stem,
                            });
                            self.status = format!(
                                "Toggled {} linked stem on deck {}",
                                stem.name(),
                                deck_idx + 1
                            );
                        }
                    } else {
                        log::warn!("[STEM_TOGGLE] command_sender is None!");
                    }
                } else {
                    // Enter Selecting mode - user will pick a track from browser
                    self.stem_link_state = StemLinkState::Selecting {
                        deck: deck_idx,
                        stem: stem_idx,
                    };
                    self.status = format!(
                        "Select track for {} stem link (deck {})",
                        mesh_core::types::Stem::from_index(stem_idx)
                            .map(|s| s.name())
                            .unwrap_or("?"),
                        deck_idx + 1
                    );
                    log::info!(
                        "Entered stem link selection mode: deck={}, stem={}",
                        deck_idx,
                        stem_idx
                    );
                }
            }
            StemLinkState::Selecting { deck, stem } => {
                if *deck == deck_idx && *stem == stem_idx {
                    // Same deck/stem pressed again - cancel selection
                    self.stem_link_state = StemLinkState::Idle;
                    self.status = "Cancelled stem link selection".to_string();
                    log::info!("Cancelled stem link selection");
                } else {
                    // Different deck/stem - switch to new selection
                    self.stem_link_state = StemLinkState::Selecting {
                        deck: deck_idx,
                        stem: stem_idx,
                    };
                    self.status = format!(
                        "Select track for {} stem link (deck {})",
                        mesh_core::types::Stem::from_index(stem_idx)
                            .map(|s| s.name())
                            .unwrap_or("?"),
                        deck_idx + 1
                    );
                }
            }
            StemLinkState::Loading { .. } => {
                // Already loading - ignore
                self.status = "Linked stem loading in progress...".to_string();
            }
        }
    }

    /// Confirm linked stem selection from browser
    ///
    /// Called when user presses encoder/enter while in Selecting mode
    fn confirm_stem_link_selection(&mut self) {
        if let StemLinkState::Selecting { deck, stem } = self.stem_link_state.clone() {
            // Get selected track path from browser
            if let Some(path) = self.collection_browser.get_selected_track_path().cloned() {
                // Get host deck's BPM, drop marker, and duration
                let host_bpm = self.global_bpm;
                // Get host track's drop marker from LinkedStemAtomics (set when track loads)
                let host_drop_marker = self
                    .linked_stem_atomics
                    .as_ref()
                    .map(|atomics| {
                        atomics[deck]
                            .host_drop_marker
                            .load(std::sync::atomic::Ordering::Relaxed)
                    })
                    .unwrap_or(0);
                // Get host track's duration from waveform state
                let host_duration = self.player_canvas_state.decks[deck].overview.duration_samples;

                // Send command to engine (single source of truth for stem loading)
                if let Some(ref mut sender) = self.command_sender {
                    let _ = sender.send(EngineCommand::LoadLinkedStem {
                        deck,
                        stem_idx: stem,
                        path: path.clone(),
                        host_bpm,
                        host_drop_marker,
                        host_duration,
                    });
                    self.stem_link_state = StemLinkState::Loading {
                        deck,
                        stem,
                        path: path.clone(),
                    };
                    self.status = format!(
                        "Loading {} stem from {}...",
                        mesh_core::types::Stem::from_index(stem)
                            .map(|s| s.name())
                            .unwrap_or("?"),
                        path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("track")
                    );
                } else {
                    self.status = "Audio not connected".to_string();
                    self.stem_link_state = StemLinkState::Idle;
                }
            } else {
                self.status = "No track selected in browser".to_string();
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
                        // Check if we're in stem link selection mode
                        if matches!(self.stem_link_state, StemLinkState::Selecting { .. }) {
                            // Confirm linked stem selection instead of loading track
                            self.confirm_stem_link_selection();
                        } else {
                            // Normal: select current item (activates track -> loads to deck 0)
                            let _ = self.update(Message::CollectionBrowser(
                                CollectionBrowserMessage::SelectCurrent,
                            ));
                        }
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

    /// Subscribe to periodic updates and async results
    pub fn subscription(&self) -> Subscription<Message> {
        // Linked stem subscription (engine owns the loader, we receive results)
        let linked_stem_sub = if let Some(ref receiver) = self.linked_stem_receiver {
            mpsc_subscription(receiver.clone())
                .map(|result| Message::LinkedStemLoaded(LinkedStemLoadedMsg(Arc::new(result))))
        } else {
            Subscription::none()
        };

        // USB manager subscription (event-driven device detection)
        let usb_sub = mpsc_subscription(self.usb_manager.message_receiver())
            .map(Message::Usb);

        Subscription::batch([
            // Update UI at ~60fps for smooth waveform animation
            time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick),
            // Background track load results (delivered as messages, no polling needed)
            mpsc_subscription(self.track_loader.result_receiver())
                .map(|result| Message::TrackLoaded(TrackLoadedMsg(Arc::new(result)))),
            // Background peak computation results
            mpsc_subscription(self.peaks_computer.result_receiver())
                .map(Message::PeaksComputed),
            // Background linked stem load results (from engine's loader)
            linked_stem_sub,
            // USB device events (connect, disconnect, mount complete)
            usb_sub,
        ])
    }

    /// Build the view
    pub fn view(&self) -> Element<'_, Message> {
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
    fn view_performance_mode(&self) -> Element<'_, Message> {
        let header = self.view_header();
        let canvas = view_player_canvas(&self.player_canvas_state);
        // Use compact browser (no load buttons) for performance mode
        let browser = self.collection_browser.view_compact().map(Message::CollectionBrowser);
        let status_bar = container(text(&self.status).size(12))
            .padding(5)
            .height(Length::Shrink);

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
        .height(Length::Fill)
        .into()
    }

    /// Mapping mode: full 3-column layout with deck controls and mixer
    /// For MIDI mapping/configuration
    fn view_mapping_mode(&self) -> Element<'_, Message> {
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

        // Center column: Waveform canvas
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
    fn view_header(&self) -> Element<'_, Message> {
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
        // Default to performance mode (no linked_stem_receiver in offline mode)
        // Note: USB manager is spawned by new(), not created separately
        Self::new(None, None, None, None, None, 48000, false)
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
