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
use std::sync::atomic::AtomicBool;

use mesh_core::db::DatabaseService;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, slider, stack, text, Space};
use iced::{Center as CenterAlign, Color, Element, Fill, Length, Subscription, Task, Theme};
use iced::{event, mouse, Event};
use iced::time;

use crate::audio::CommandSender;
use crate::config::{self, PlayerConfig};
use crate::domain::MeshDomain;
use crate::plugin_gui::PluginGuiManager;

use mesh_midi::{ControllerManager, MidiMessage as MidiMsg, MidiInputEvent, DeckAction as MidiDeckAction, MixerAction as MidiMixerAction, BrowserAction as MidiBrowserAction};
use mesh_core::engine::{DeckAtomics, LinkedStemAtomics, SlicerAtomics};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{mpsc_subscription, multiband_editor, MultibandEditorState, SliceEditorState};

use super::collection_browser::{CollectionBrowserState, CollectionBrowserMessage};
use super::deck_view::{DeckView, DeckMessage};
use super::midi_learn::{MidiLearnState, HighlightTarget};
use super::mixer_view::{MixerView, MixerMessage};
use super::player_canvas::{view_player_canvas, PlayerCanvasState};
use super::settings::SettingsState;

// Re-export extracted modules for use by other UI modules
pub use super::message::{Message, SettingsMessage};
pub use super::state::{AppMode, LinkedStemLoadedMsg, PresetLoadedMsg, StemLinkState, TrackLoadedMsg};

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
    /// Master clipper clip indicator (true = clipping occurred this buffer)
    pub(crate) clip_indicator: Option<Arc<AtomicBool>>,
    /// Hold timer for clip indicator UI (decremented each tick, show red dot when > 0)
    pub(crate) clip_hold_frames: u8,
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
    /// Controller manager (MIDI + HID, optional - works without controllers)
    pub(crate) controller: Option<ControllerManager>,
    /// MIDI learn mode state
    pub(crate) midi_learn: MidiLearnState,
    /// UI display mode (performance vs mapping)
    pub(crate) app_mode: AppMode,
    /// Linked stem selection state machine
    pub(crate) stem_link_state: StemLinkState,
    /// Slice editor state (shared presets and per-stem patterns)
    pub(crate) slice_editor: SliceEditorState,
    /// Multiband editor modal state
    pub(crate) multiband_editor: MultibandEditorState,
    /// Plugin GUI manager for CLAP plugin windows and parameter learning
    pub(crate) plugin_gui_manager: PluginGuiManager,
    /// Currently selected global FX preset name (applied to all decks)
    pub(crate) global_fx_preset: Option<String>,
    /// Whether the global FX preset picker is open
    pub(crate) global_fx_picker_open: bool,
    /// Available deck presets for the global FX dropdown
    pub(crate) available_deck_presets: Vec<String>,
    /// Currently hovered preset index for MIDI scroll highlighting
    pub(crate) global_fx_hover_index: Option<usize>,
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
    /// - `clip_indicator`: Atomic flag set by master clipper when clipping occurs (None for offline mode)
    /// - `sample_rate`: Audio system's sample rate for track loading (e.g., 48000 or 44100)
    pub fn new(
        db_service: Arc<DatabaseService>,
        command_sender: Option<CommandSender>,
        deck_atomics: Option<[Arc<DeckAtomics>; NUM_DECKS]>,
        slicer_atomics: Option<[Arc<SlicerAtomics>; NUM_DECKS]>,
        linked_stem_atomics: Option<[Arc<LinkedStemAtomics>; NUM_DECKS]>,
        linked_stem_receiver: Option<mesh_core::loader::LinkedStemResultReceiver>,
        clip_indicator: Option<Arc<AtomicBool>>,
        sample_rate: u32,
        mapping_mode: bool,
    ) -> Self {
        // Load configuration
        let config_path = config::default_config_path();
        let config = Arc::new(config::load_config(&config_path));
        let settings = SettingsState::from_config(&config);

        let audio_connected = command_sender.is_some();

        // Load slicer presets (shared with both engine and UI)
        let slicer_config = mesh_widgets::load_slicer_presets(&config.collection_path);

        // Initialize MIDI controller
        // In mapping mode: connect to ALL available ports (for device discovery)
        // In normal mode: connect only to devices matching config (with raw capture for live learning)
        let controller = if mapping_mode {
            match ControllerManager::new_for_learn_mode() {
                Ok(controller) => {
                    if controller.is_connected() {
                        log::info!(
                            "MIDI Learn: Connected to {} device(s)",
                            controller.connected_count()
                        );
                    } else {
                        log::warn!("MIDI Learn: No MIDI devices found - connect a controller");
                    }
                    Some(controller)
                }
                Err(e) => {
                    log::warn!("MIDI Learn: Failed to initialize: {}", e);
                    None
                }
            }
        } else {
            match ControllerManager::new_with_options(None, true) {
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
            sample_rate,
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

        // Sync initial mixer state to engine (UI defaults to 0.0 volume,
        // engine defaults to 1.0 — must agree to avoid fader jump on first move)
        for ch in 0..4 {
            domain.set_volume(ch, 0.0);
        }
        domain.set_master_volume(0.8);
        domain.set_cue_volume(0.8);

        // Apply default loop length from config to all decks
        let default_loop_idx = config.display.default_loop_length_index;
        for deck in 0..4 {
            domain.set_loop_length_index(deck, default_loop_idx);
        }

        // Initialize deck views with configured loop length
        let mut deck_views = [
            DeckView::new(0),
            DeckView::new(1),
            DeckView::new(2),
            DeckView::new(3),
        ];
        for dv in &mut deck_views {
            dv.sync_loop_length_index(default_loop_idx as u8);
        }

        Self {
            domain,
            deck_atomics,
            slicer_atomics,
            linked_stem_atomics,
            clip_indicator,
            clip_hold_frames: 0,
            player_canvas_state: {
                let mut state = PlayerCanvasState::new();
                state.set_stem_colors(config.display.stem_color_palette.colors());
                state
            },
            deck_views,
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
            controller,
            midi_learn: MidiLearnState::new(),
            app_mode: if mapping_mode { AppMode::Mapping } else { AppMode::Performance },
            stem_link_state: StemLinkState::Idle,
            multiband_editor: MultibandEditorState::new(),
            plugin_gui_manager: PluginGuiManager::new(),
            global_fx_preset: None,
            global_fx_picker_open: false,
            available_deck_presets: Vec::new(),
            global_fx_hover_index: None,
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

            Message::PresetLoaded(msg) => super::handlers::deck_controls::handle_preset_loaded(self, msg),

            Message::Multiband(multiband_msg) => {
                super::handlers::multiband::handle(self, multiband_msg)
            }

            Message::PluginGuiTick => {
                // Poll GUI handles for parameter changes when in learning mode
                super::handlers::multiband::handle_plugin_gui_tick(self)
            }

            Message::SelectGlobalFxPreset(preset_name) => {
                super::handlers::deck_controls::handle_global_fx_preset_selection(self, preset_name)
            }

            Message::ToggleGlobalFxPicker => {
                super::handlers::deck_controls::handle_toggle_global_fx_picker(self);
                Task::none()
            }

            Message::ScrollGlobalFx(delta) => {
                super::handlers::deck_controls::handle_global_fx_scroll(self, delta);
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
    /// Returns a Task that should be processed by the iced runtime (e.g., for scroll operations)
    pub(crate) fn handle_midi_message(&mut self, msg: MidiMsg) -> Task<Message> {
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
                        let pad_mode_source = self.controller
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
                                super::deck_view::ActionButtonMode::Slicer => Some(DeckMessage::SlicerPresetSelect(slot)),
                            }
                        }
                    }
                    MidiDeckAction::HotCueRelease { slot } => {
                        // Check pad mode source for release handling
                        let pad_mode_source = self.controller
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
                    MidiDeckAction::LoopSize(delta) => {
                        if delta < 0 {
                            Some(DeckMessage::LoopHalve)
                        } else {
                            Some(DeckMessage::LoopDouble)
                        }
                    }
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
                    MidiDeckAction::SetFxMacro { macro_index, value } => {
                        Some(DeckMessage::DeckPreset(mesh_widgets::DeckPresetMessage::SetMacro { index: macro_index, value }))
                    }
                    MidiDeckAction::ToggleSlip => Some(DeckMessage::ToggleSlip),
                    MidiDeckAction::ToggleKeyMatch => Some(DeckMessage::ToggleKeyMatch),
                    MidiDeckAction::LoadSelected => {
                        // If a track is selected, load it to this deck
                        if let Some(track_path) = self.collection_browser.get_selected_track_path() {
                            let _ = self.update(Message::LoadTrack(deck, track_path.to_string_lossy().to_string()));
                        } else {
                            // No track selected — enter the selected folder/playlist
                            let _ = self.update(Message::CollectionBrowser(
                                CollectionBrowserMessage::SelectCurrent,
                            ));
                        }
                        None
                    }
                    MidiDeckAction::BrowseBack => {
                        let _ = self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::Back,
                        ));
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
                return match action {
                    MidiBrowserAction::Scroll { delta } => {
                        // Scroll browser and return Task for auto-scroll
                        self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::ScrollBy(delta),
                        ))
                    }
                    MidiBrowserAction::Select => {
                        // Check if we're in stem link selection mode
                        if matches!(self.stem_link_state, StemLinkState::Selecting { .. }) {
                            // Confirm linked stem selection instead of loading track
                            self.confirm_stem_link_selection();
                            Task::none()
                        } else {
                            // Normal: select current item (activates track -> loads to deck 0)
                            self.update(Message::CollectionBrowser(
                                CollectionBrowserMessage::SelectCurrent,
                            ))
                        }
                    }
                    MidiBrowserAction::Back => {
                        self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::Back,
                        ))
                    }
                };
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
                    MidiGlobalAction::FxScroll(delta) => {
                        return self.update(Message::ScrollGlobalFx(delta));
                    }
                    MidiGlobalAction::FxSelect => {
                        // Select the currently hovered preset
                        // Hover index: 0 = "No FX", 1..=N = preset at index (i-1)
                        let preset_name = self
                            .global_fx_hover_index
                            .and_then(|i| {
                                if i == 0 { None }
                                else { self.available_deck_presets.get(i - 1).cloned() }
                            });
                        return self.update(Message::SelectGlobalFxPreset(preset_name));
                    }
                }
            }

            MidiMsg::LayerToggle { physical_deck } => {
                // State already toggled in shared state by input callback
                // Update UI indicators for all deck views
                log::debug!("MIDI: Layer toggle for physical deck {}", physical_deck);
                self.update_layer_indicators();
            }

            MidiMsg::ShiftChanged { held, physical_deck } => {
                // Per-deck shift: update only the active virtual deck for this physical deck
                if let Some(ref midi) = self.controller {
                    let vd = midi.resolve_deck(physical_deck);
                    if vd < self.deck_views.len() {
                        self.deck_views[vd].set_shift_held(held);
                    }
                }
            }
        }

        Task::none()
    }

    /// Update deck view layer indicators based on MIDI controller state
    ///
    /// Sets `midi_active` and `is_secondary_layer` on each deck view
    /// so the UI can color-code deck labels by active layer.
    pub(crate) fn update_layer_indicators(&mut self) {
        if let Some(ref midi) = self.controller {
            if midi.is_layer_mode() {
                // For each physical deck (0=left, 1=right), find which virtual deck it targets
                for physical in 0..2 {
                    let virtual_deck = midi.resolve_deck(physical);
                    let layer = midi.get_layer(physical);
                    let is_secondary = layer == mesh_midi::LayerSelection::B;

                    // Mark the targeted virtual deck as active
                    if virtual_deck < self.deck_views.len() {
                        self.deck_views[virtual_deck].set_midi_active(true);
                        self.deck_views[virtual_deck].set_secondary_layer(is_secondary);
                    }
                }

                // Mark non-targeted decks as inactive
                for d in 0..self.deck_views.len() {
                    let is_targeted = (0..2).any(|p| midi.resolve_deck(p) == d);
                    if !is_targeted {
                        self.deck_views[d].set_midi_active(false);
                        self.deck_views[d].set_secondary_layer(false);
                    }
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

        // Global mouse event subscription for knob drag capture
        // Only active when a knob is being dragged (to avoid overhead otherwise)
        let mouse_capture_sub = if self.multiband_editor.is_any_knob_dragging() {
            event::listen_with(|event, _status, _id| {
                match event {
                    Event::Mouse(mouse::Event::CursorMoved { position }) => {
                        Some(Message::Multiband(
                            mesh_widgets::MultibandEditorMessage::GlobalMouseMoved(position)
                        ))
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        Some(Message::Multiband(
                            mesh_widgets::MultibandEditorMessage::GlobalMouseReleased
                        ))
                    }
                    _ => None,
                }
            })
        } else {
            Subscription::none()
        };

        // Plugin GUI polling subscription for parameter learning
        // Only active when in learning mode (to avoid overhead otherwise)
        let plugin_gui_sub = if self.multiband_editor.is_learning() {
            // Poll at 30fps for responsive learning detection
            time::every(std::time::Duration::from_millis(33)).map(|_| Message::PluginGuiTick)
        } else {
            Subscription::none()
        };

        Subscription::batch([
            // Update UI at ~60fps for smooth waveform animation
            time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick),
            // Background track load results (delivered as messages, no polling needed)
            mpsc_subscription(self.domain.track_loader_result_receiver())
                .map(|result| Message::TrackLoaded(TrackLoadedMsg(Arc::new(result)))),
            // Background peak computation results
            mpsc_subscription(self.domain.peaks_result_receiver())
                .map(Message::PeaksComputed),
            // Background preset load results (MultibandHost built on loader thread)
            mpsc_subscription(self.domain.preset_loader_result_receiver())
                .map(|result| Message::PresetLoaded(PresetLoadedMsg(
                    Arc::new(std::sync::Mutex::new(Some(result)))
                ))),
            // Background linked stem load results (from engine's loader)
            linked_stem_sub,
            // USB device events (connect, disconnect, mount complete)
            usb_sub,
            // Global mouse capture for smooth knob dragging
            mouse_capture_sub,
            // Plugin GUI parameter learning polling
            plugin_gui_sub,
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

        // Overlay FX dropdown list when open (above content, below modals)
        let with_fx_dropdown: Element<'a, Message> = if self.global_fx_picker_open {
            // Transparent backdrop to close dropdown on click-away
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .on_press(Message::ToggleGlobalFxPicker);

            // The dropdown list, positioned near the top
            // Use a column with a spacer to push the list below the header
            let dropdown_list = column![
                Space::new().height(50),
                container(self.view_global_fx_list())
                    .width(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center),
            ]
            .width(Length::Fill)
            .height(Length::Fill);

            stack![with_drawer, backdrop, dropdown_list].into()
        } else {
            with_drawer
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

            stack![with_fx_dropdown, backdrop, modal].into()
        } else if self.multiband_editor.is_open {
            // Overlay multiband editor modal
            if let Some(editor_view) = multiband_editor(&self.multiband_editor) {
                let multiband_modal = editor_view.map(Message::Multiband);
                stack![with_fx_dropdown, multiband_modal].into()
            } else {
                with_fx_dropdown
            }
        } else {
            with_fx_dropdown
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

        // Global FX preset selector
        let fx_element = self.view_global_fx_dropdown();

        let clipping = self.clip_hold_frames > 0;
        let dot_color = if clipping {
            Color::from_rgb(1.0, 0.15, 0.15)
        } else {
            Color::from_rgb(0.3, 0.8, 0.3)
        };
        let connection_status: Element<'_, Message> = if self.domain.is_audio_connected() {
            row![
                text("●").size(12).color(dot_color),
                text(" Audio Connected").size(12),
            ].into()
        } else {
            text("○ Audio Disconnected").size(12).into()
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
            Space::new().width(20),
            fx_element,
            Space::new().width(Fill),
            connection_status,
            settings_btn,
        ]
        .spacing(20)
        .align_y(CenterAlign)
        .padding(10)
        .into()
    }

    /// View for the global FX preset button in the header (list is overlaid separately)
    fn view_global_fx_dropdown(&self) -> Element<'_, Message> {
        // When open and hovering via MIDI, show the hovered preset name
        // Hover index: 0 = "No FX", 1..=N = preset at index (i-1)
        let label = if self.global_fx_picker_open {
            if let Some(idx) = self.global_fx_hover_index {
                if idx == 0 {
                    "No FX"
                } else {
                    self.available_deck_presets
                        .get(idx - 1)
                        .map(|s| s.as_str())
                        .unwrap_or("No FX")
                }
            } else {
                self.global_fx_preset.as_deref().unwrap_or("No FX")
            }
        } else {
            self.global_fx_preset.as_deref().unwrap_or("No FX")
        };

        let arrow = if self.global_fx_picker_open { "▴" } else { "▾" };

        button(
            row![text(label).size(11), Space::new().width(Fill), text(arrow).size(11)]
                .spacing(4)
                .align_y(CenterAlign),
        )
        .on_press(Message::ToggleGlobalFxPicker)
        .padding([4, 8])
        .width(Length::Fixed(140.0))
        .into()
    }

    /// Render the FX preset dropdown list (called from apply_overlays when open)
    fn view_global_fx_list(&self) -> Element<'_, Message> {
        use iced::widget::scrollable;

        let mut items: Vec<Element<'_, Message>> = Vec::new();

        // "No FX" option (hover index 0)
        let no_fx_selected = self.global_fx_preset.is_none();
        let no_fx_hovered = self.global_fx_hover_index == Some(0);
        items.push(
            button(text("(No FX)").size(10))
                .on_press(Message::SelectGlobalFxPreset(None))
                .padding([3, 8])
                .width(Fill)
                .style(if no_fx_selected || no_fx_hovered { button::primary } else { button::secondary })
                .into(),
        );

        // Available presets (hover index 1..=N maps to preset index i-1)
        for (i, preset_name) in self.available_deck_presets.iter().enumerate() {
            let is_selected = self.global_fx_preset.as_ref() == Some(preset_name);
            let is_hovered = self.global_fx_hover_index == Some(i + 1);
            let name = preset_name.clone();
            items.push(
                button(text(preset_name).size(10))
                    .on_press(Message::SelectGlobalFxPreset(Some(name)))
                    .padding([3, 8])
                    .width(Fill)
                    .style(if is_selected || is_hovered { button::primary } else { button::secondary })
                    .into(),
            );
        }

        let list = scrollable(column(items).spacing(1).width(Fill))
            .height(Length::Fixed(200.0));

        container(list)
            .width(Length::Fixed(160.0))
            .style(|_theme| container::Style {
                background: Some(Color::from_rgba(0.12, 0.12, 0.16, 0.98).into()),
                border: iced::Border {
                    color: Color::from_rgb(0.3, 0.3, 0.5),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .padding(4)
            .into()
    }

    /// Get the theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
    }
}

// Note: MeshApp no longer implements Default as it requires a DatabaseService

/// Convert a raw MidiInputEvent to CapturedEvent for learn mode
pub(crate) fn convert_midi_event_to_captured(event: &MidiInputEvent) -> super::midi_learn::CapturedEvent {
    use super::midi_learn::CapturedEvent;
    use mesh_midi::{ControlAddress, MidiAddress};

    let (address, value) = match event {
        MidiInputEvent::NoteOn { channel, note, velocity } => (
            ControlAddress::Midi(MidiAddress::Note { channel: *channel, note: *note }),
            *velocity,
        ),
        MidiInputEvent::NoteOff { channel, note, velocity } => (
            ControlAddress::Midi(MidiAddress::Note { channel: *channel, note: *note }),
            *velocity,
        ),
        MidiInputEvent::ControlChange { channel, cc, value } => (
            ControlAddress::Midi(MidiAddress::CC { channel: *channel, cc: *cc }),
            *value,
        ),
    };

    CapturedEvent {
        address,
        value,
        hardware_type: None, // MIDI: needs detection via MidiSampleBuffer
        source_device: None, // Source captured at port level in tick.rs
    }
}

/// Convert a HID ControlEvent to CapturedEvent for learn mode
pub(crate) fn convert_hid_event_to_captured(
    event: &mesh_midi::ControlEvent,
    descriptor: Option<&mesh_midi::ControlDescriptor>,
    device_name: &str,
) -> super::midi_learn::CapturedEvent {
    use super::midi_learn::CapturedEvent;

    CapturedEvent {
        address: event.address.clone(),
        value: event.value.as_midi_value(),
        hardware_type: descriptor.map(|d| d.control_type),
        source_device: Some(device_name.to_string()),
    }
}
