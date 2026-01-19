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

use mesh_core::db::DatabaseService;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, slider, stack, text, Space};
use iced::{Center as CenterAlign, Color, Element, Fill, Length, Subscription, Task, Theme};
use iced::time;

use crate::audio::CommandSender;
use crate::config::{self, PlayerConfig};
use crate::domain::MeshDomain;

use mesh_midi::{MidiController, MidiMessage as MidiMsg, MidiInputEvent, DeckAction as MidiDeckAction, MixerAction as MidiMixerAction, BrowserAction as MidiBrowserAction};
use mesh_core::engine::{DeckAtomics, LinkedStemAtomics, SlicerAtomics};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{mpsc_subscription, SliceEditorState};
use super::collection_browser::{CollectionBrowserState, CollectionBrowserMessage};
use super::deck_view::{DeckView, DeckMessage};
use super::midi_learn::{MidiLearnState, HighlightTarget};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};
use super::settings::SettingsState;

// Re-export extracted modules for use by other UI modules
pub use super::message::{Message, SettingsMessage};
pub use super::state::{AppMode, LinkedStemLoadedMsg, StemLinkState, TrackLoadedMsg};

/// Application state
pub struct MeshApp {
    /// Domain layer for service orchestration
    /// Manages databases, services, and domain state (stems, LUFS, BPM)
    pub(crate) domain: MeshDomain,
    /// Lock-free deck state for UI reads (position, play state, loop)
    /// These atomics are updated by the audio thread; UI reads are wait-free
    pub(crate) deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
    /// Lock-free slicer state for UI reads (drums stem slicer on all decks)
    pub(crate) slicer_atomics: Option<[Arc<SlicerAtomics>; NUM_DECKS]>,
    /// Lock-free linked stem state for UI reads (which stems have links)
    pub(crate) linked_stem_atomics: Option<[Arc<LinkedStemAtomics>; NUM_DECKS]>,
    /// Unified waveform state for all 4 decks
    pub(crate) player_canvas_state: PlayerCanvasState,
    /// Local deck view states (controls only, waveform moved to player_canvas_state)
    pub(crate) deck_views: [DeckView; 4],
    /// Mixer view state
    pub(crate) mixer_view: MixerView,
    /// Collection browser (read-only, shared with mesh-cue)
    pub(crate) collection_browser: CollectionBrowserState,
    /// Status message
    pub(crate) status: String,
    /// Configuration
    pub(crate) config: Arc<PlayerConfig>,
    /// Path to config file
    pub(crate) config_path: PathBuf,
    /// Settings modal state
    pub(crate) settings: SettingsState,
    /// MIDI controller (optional - works without MIDI)
    pub(crate) midi_controller: Option<MidiController>,
    /// MIDI learn mode state
    pub(crate) midi_learn: MidiLearnState,
    /// UI display mode (performance vs mapping)
    pub(crate) app_mode: AppMode,
    /// Linked stem selection state machine
    pub(crate) stem_link_state: StemLinkState,
    /// Slice editor state (shared presets and per-stem patterns)
    pub(crate) slice_editor: SliceEditorState,
}

// Message enum moved to message.rs

impl MeshApp {
    /// Create a new application instance
    ///
    /// ## Parameters
    ///
    /// - `db_service`: Database service for track metadata (required)
    /// - `command_sender`: Lock-free command channel for engine control (None for offline mode)
    /// - `deck_atomics`: Lock-free position/state for UI reads (None for offline mode)
    /// - `slicer_atomics`: Lock-free slicer state for UI reads (None for offline mode)
    /// - `linked_stem_atomics`: Lock-free linked stem state for UI reads (None for offline mode)
    /// - `linked_stem_receiver`: Receiver for linked stem load results (engine owns the loader)
    /// - `jack_sample_rate`: JACK's sample rate for track loading (e.g., 48000 or 44100)
    pub fn new(
        db_service: Arc<DatabaseService>,
        command_sender: Option<CommandSender>,
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

        let audio_connected = command_sender.is_some();

        // Load slicer presets (shared with both engine and UI)
        let slicer_config = mesh_widgets::load_slicer_presets(&config.collection_path);

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

        // Create domain layer with all services
        // Domain owns: command_sender, track_loader, peaks_computer, usb_manager, linked_stem_receiver
        // Domain also owns: deck_stems, deck_linked_stems, track_lufs_per_deck, global_bpm
        let mut domain = MeshDomain::new(
            db_service.clone(),
            config.collection_path.clone(),
            command_sender,
            linked_stem_receiver,
            jack_sample_rate,
            config.audio.global_bpm,
        );

        // Initialize audio engine with config (sends initial settings)
        domain.initialize_engine(
            config.audio.global_bpm,
            config.audio.phase_sync,
            config::slicer_config_to_engine_presets(&slicer_config),
            slicer_config.validated_buffer_bars(),
            config.audio.loudness.clone(),
        );

        Self {
            domain,
            deck_atomics,
            slicer_atomics,
            linked_stem_atomics,
            player_canvas_state: {
                let mut state = PlayerCanvasState::new();
                state.set_stem_colors(config.display.stem_color_palette.colors());
                state
            },
            deck_views: [
                DeckView::new(0),
                DeckView::new(1),
                DeckView::new(2),
                DeckView::new(3),
            ],
            mixer_view: MixerView::new(),
            collection_browser: CollectionBrowserState::new(
                config.collection_path.clone(),
                db_service.clone(),
                config.display.show_local_collection,
            ),
            status: if audio_connected { "Audio connected (lock-free)".to_string() } else { "No audio".to_string() },
            slice_editor: {
                // Apply presets to slice editor (reusing already-loaded config)
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
            Message::Tick => super::handlers::tick::handle(self),

            Message::TrackLoaded(msg) => super::handlers::track_loading::handle_track_loaded(self, msg),

            Message::PeaksComputed(result) => super::handlers::track_loading::handle_peaks_computed(self, result),

            Message::LinkedStemLoaded(msg) => super::handlers::track_loading::handle_linked_stem_loaded(self, msg),

            Message::Deck(deck_idx, deck_msg) => super::handlers::deck_controls::handle(self, deck_idx, deck_msg),

            Message::Mixer(mixer_msg) => super::handlers::mixer::handle(self, mixer_msg),

            Message::CollectionBrowser(browser_msg) => super::handlers::browser::handle_browser(self, browser_msg),

            Message::SetGlobalBpm(bpm) => {
                // Round to integer BPM for clean display and computation
                let bpm_rounded = bpm.round();
                // Update domain state and send to audio engine
                self.domain.set_global_bpm_with_engine(bpm_rounded);
                Task::none()
            }

            Message::LoadTrack(deck_idx, path) => {
                // Send load request to background thread (non-blocking)
                // Result will be picked up via subscription from domain's track_loader
                if deck_idx < 4 {
                    self.status = format!("Loading track to deck {}...", deck_idx + 1);

                    // Domain handles metadata lookup and track loading
                    if let Err(e) = self.domain.request_track_load(deck_idx, path.into()) {
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
                                self.domain.seek(deck_idx, seek_samples);
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

            Message::Settings(settings_msg) => super::handlers::settings::handle(self, settings_msg),

            Message::MidiLearn(learn_msg) => super::handlers::midi_learn::handle(self, learn_msg),

            Message::Usb(usb_msg) => super::handlers::browser::handle_usb(self, usb_msg),
        }
    }

    /// Handle shift+stem gesture for linked stem operations
    ///
    /// When shift is held and a stem button is pressed:
    /// - If already in Selecting mode for same deck/stem: cancel selection
    /// - If a linked stem exists: toggle between original/linked
    /// - Otherwise: enter Selecting mode for browser track selection
    pub(crate) fn handle_shift_stem(&mut self, deck_idx: usize, stem_idx: usize) {
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
                    if let Some(stem) = mesh_core::types::Stem::from_index(stem_idx) {
                        log::info!(
                            "[STEM_TOGGLE] Sending ToggleLinkedStem: deck={}, stem={:?}",
                            deck_idx, stem
                        );
                        self.domain.toggle_linked_stem(deck_idx, stem);
                        self.status = format!(
                            "Toggled {} linked stem on deck {}",
                            stem.name(),
                            deck_idx + 1
                        );
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
                let host_bpm = self.domain.global_bpm();
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

                // Send command to engine via domain (single source of truth for stem loading)
                if self.domain.load_linked_stem(deck, stem, path.clone(), host_bpm, host_drop_marker, host_duration) {
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
    pub(crate) fn handle_midi_message(&mut self, msg: MidiMsg) {
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
        // Linked stem subscription (domain owns the receiver from engine)
        let linked_stem_sub = if let Some(receiver) = self.domain.linked_stem_result_receiver() {
            mpsc_subscription(receiver.clone())
                .map(|result| Message::LinkedStemLoaded(LinkedStemLoadedMsg(Arc::new(result))))
        } else {
            Subscription::none()
        };

        // USB manager subscription (event-driven device detection)
        let usb_sub = mpsc_subscription(self.domain.usb_message_receiver())
            .map(Message::Usb);

        Subscription::batch([
            // Update UI at ~60fps for smooth waveform animation
            time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick),
            // Background track load results (delivered as messages, no polling needed)
            mpsc_subscription(self.domain.track_loader_result_receiver())
                .map(|result| Message::TrackLoaded(TrackLoadedMsg(Arc::new(result)))),
            // Background peak computation results
            mpsc_subscription(self.domain.peaks_result_receiver())
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
            .on_press(Message::Settings(SettingsMessage::Close));

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

        let global_bpm = self.domain.global_bpm();
        let bpm_label = text(format!("BPM: {}", global_bpm as i32)).size(16);

        let bpm_slider = slider(30.0..=200.0, global_bpm, Message::SetGlobalBpm)
            .step(1.0)
            .width(200);

        let connection_status = if self.domain.is_audio_connected() {
            text("● JACK Connected").size(12)
        } else {
            text("○ JACK Disconnected").size(12)
        };

        // Settings gear icon (⚙ U+2699)
        let settings_btn = button(text("⚙").size(20))
            .on_press(Message::Settings(SettingsMessage::Open))
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

// Note: MeshApp no longer implements Default as it requires a DatabaseService

/// Convert a raw MidiInputEvent to CapturedMidiEvent for learn mode
pub(crate) fn convert_midi_event_to_captured(event: &MidiInputEvent) -> super::midi_learn::CapturedMidiEvent {
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
