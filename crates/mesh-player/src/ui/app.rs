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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

use mesh_core::db::DatabaseService;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, slider, stack, text, Space};
use iced::{Center as CenterAlign, Color, Element, Fill, Length, Subscription, Task, Theme};
use iced::{event, mouse, Event};
use iced::time;

use crate::audio::CommandSender;
use crate::config::{self, PlayerConfig};
use crate::domain::MeshDomain;
use crate::plugin_gui::PluginGuiManager;

use mesh_midi::{ControllerManager, MidiMessage as MidiMsg, MidiEvent, MidiInputEvent, DeckAction as MidiDeckAction, MixerAction as MidiMixerAction, BrowserAction as MidiBrowserAction};
use mesh_core::engine::{DeckAtomics, LinkedStemAtomics, SlicerAtomics};
use mesh_core::types::NUM_DECKS;
use mesh_widgets::{mpsc_subscription, multiband_editor, MultibandEditorState, SliceEditorState};
use mesh_widgets::keyboard::{KeyboardState, KeyboardEvent, keyboard_view, keyboard_handle};

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
    /// Per-side browse mode active state (0=left, 1=right), synced from mapping engine
    pub(crate) browse_mode_active: [bool; 2],
    /// Whether browser overlay is visible (performance mode only)
    pub(crate) browser_visible: bool,
    /// Ticks until browser auto-hide (0 = no timer; 300 = 5s at 60Hz)
    pub(crate) browser_hide_countdown: u16,
    /// Whether a suggestion seed refresh is already scheduled (debounce guard)
    pub(crate) suggestion_refresh_pending: bool,
    /// Generation counter for energy direction debounce (trailing-edge: only the last timer fires)
    pub(crate) energy_debounce_gen: u64,
    /// Actual JACK client name (for port reconnection)
    pub(crate) audio_client_name: String,
    /// Real output pipeline latency in samples (from CPAL/JACK timestamps)
    pub(crate) output_latency_samples: Option<Arc<AtomicU64>>,
    /// Internal effect chain latency in samples (global max)
    pub(crate) internal_latency_samples: Option<Arc<AtomicU32>>,
    /// Audio sample rate for latency calculations
    pub(crate) audio_sample_rate: u32,
    /// On-screen keyboard state (shared widget from mesh-widgets)
    pub(crate) keyboard: KeyboardState,
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
        audio_client_name: String,
        mapping_mode: bool,
        output_latency_samples: Option<Arc<AtomicU64>>,
        internal_latency_samples: Option<Arc<AtomicU32>>,
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
        // Domain owns: command_sender, track_loader, usb_manager, linked_stem_receiver
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
                state.set_vertical_layout(config.display.waveform_layout.is_vertical());
                state.set_vertical_inverted(config.display.waveform_layout.is_inverted());
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
            browse_mode_active: [false; 2],
            browser_visible: false,
            browser_hide_countdown: 0,
            suggestion_refresh_pending: false,
            energy_debounce_gen: 0,
            audio_client_name,
            output_latency_samples,
            internal_latency_samples,
            audio_sample_rate: sample_rate,
            keyboard: KeyboardState::new(),
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

    /// Show the browser overlay (performance mode only)
    pub(crate) fn show_browser_overlay(&mut self) {
        if self.app_mode == AppMode::Performance {
            self.browser_visible = true;
            self.browser_hide_countdown = 300; // 5 seconds at 60Hz
        }
    }

    /// Hide the browser overlay and clear the countdown
    pub(crate) fn hide_browser_overlay(&mut self) {
        self.browser_visible = false;
        self.browser_hide_countdown = 0;
    }

    /// Update application state
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => super::handlers::tick::handle(self),
            Message::UpdateLeds => super::handlers::led_feedback::handle(self),

            Message::TrackLoaded(msg) => super::handlers::track_loading::handle_track_loaded(self, msg),

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
                // Streaming track loading: create skeleton immediately, load audio in background
                if deck_idx < 4 {
                    match self.domain.create_skeleton_and_load(deck_idx, path.into()) {
                        Ok(skeleton) => {
                            // Apply skeleton waveform (loading state with beat/cue markers)
                            self.player_canvas_state.decks[deck_idx].overview = skeleton.overview_state;
                            self.player_canvas_state.decks[deck_idx].zoomed = skeleton.zoomed_state;

                            // Apply user display config
                            self.player_canvas_state.decks[deck_idx]
                                .overview.set_grid_bars(self.config.display.grid_bars);
                            self.player_canvas_state.decks[deck_idx]
                                .zoomed.set_zoom(self.config.display.default_zoom_bars);

                            // Set track info from skeleton metadata
                            let track = &skeleton.prepared.track;
                            let filename = track.path.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Unknown")
                                .to_string();

                            self.player_canvas_state.set_track_name(deck_idx, filename.clone());
                            self.player_canvas_state.set_track_key(
                                deck_idx,
                                track.metadata.key.clone().unwrap_or_default(),
                            );
                            self.player_canvas_state.set_track_bpm(deck_idx, track.metadata.bpm);

                            // Sync hot cues to deck view
                            for (slot, hot_cue) in skeleton.prepared.hot_cues.iter().enumerate() {
                                self.deck_views[deck_idx].set_hot_cue_position(
                                    slot,
                                    hot_cue.as_ref().map(|hc| hc.position as u64),
                                );
                            }

                            // Store loaded track path for stale detection
                            self.deck_views[deck_idx].set_loaded_track_path(
                                Some(track.path.to_string_lossy().to_string()),
                            );

                            // Reset stem mute/solo state
                            for stem_idx in 0..4 {
                                self.deck_views[deck_idx].set_stem_muted(stem_idx, false);
                                self.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                                self.player_canvas_state.set_stem_active(deck_idx, stem_idx, true);
                                self.player_canvas_state.set_linked_stem(deck_idx, stem_idx, false, false);
                            }

                            // Send skeleton to engine (empty stems, correct duration for navigation)
                            self.domain.apply_loaded_track(
                                deck_idx,
                                skeleton.stems,
                                skeleton.lufs,
                                skeleton.prepared,
                            );

                            self.deck_views[deck_idx].set_audio_loading(true);
                            self.status = format!("Loading audio for deck {}...", deck_idx + 1);
                        }
                        Err(e) => {
                            self.status = format!("Failed to start load: {}", e);
                        }
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

            Message::SuggestionsReady(result) => {
                super::handlers::browser::handle_suggestions_ready(self, result)
            }

            Message::ScheduleSuggestionRefresh => {
                super::handlers::browser::schedule_suggestion_refresh(self)
            }

            Message::CheckSuggestionSeeds => {
                super::handlers::browser::check_suggestion_seeds(self)
            }

            Message::CheckEnergyDebounce(gen) => {
                super::handlers::browser::check_energy_debounce(self, gen)
            }

            Message::HideBrowserOverlay => {
                self.hide_browser_overlay();
                Task::none()
            }

            Message::Keyboard(kb_msg) => {
                if let Some(event) = keyboard_handle(&mut self.keyboard, kb_msg) {
                    match event {
                        KeyboardEvent::Submit(password) => {
                            // Route to WiFi connect if a network was selected
                            if let Some(ref net_state) = self.settings.network {
                                if let Some(idx) = net_state.selected_network {
                                    if let Some(network) = net_state.networks.get(idx) {
                                        let ssid = network.ssid.clone();
                                        return self.update(Message::Network(
                                            super::network::NetworkMessage::ConnectSecured {
                                                ssid,
                                                password,
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                        KeyboardEvent::Cancel => {
                            log::debug!("Keyboard cancelled");
                        }
                    }
                }
                Task::none()
            }

            Message::Network(net_msg) => {
                super::handlers::network::handle(self, net_msg)
            }

            Message::SystemUpdate(update_msg) => {
                super::handlers::system_update::handle(self, update_msg)
            }

            Message::GotMonitorSize(Some(size)) => {
                log::info!("Monitor size detected: {}x{}", size.width, size.height);
                iced::window::oldest().then(move |opt_id| {
                    if let Some(id) = opt_id {
                        iced::window::resize(id, size)
                    } else {
                        Task::none()
                    }
                })
            }
            Message::GotMonitorSize(None) => {
                log::warn!("Could not detect monitor size, using default window size");
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

    /// Handle a MIDI event by dispatching to existing message handlers
    /// Returns a Task that should be processed by the iced runtime (e.g., for scroll operations)
    ///
    /// If `engine_dispatched` is true, the timing-critical engine command was already
    /// sent directly from the MIDI callback — skip the UI→engine path to avoid
    /// double-execution (e.g. double play-toggle would cancel out).
    pub(crate) fn handle_midi_message(&mut self, event: MidiEvent) -> Task<Message> {
        let engine_dispatched = event.engine_dispatched;
        let msg = event.message;

        // ── Keyboard MIDI interception ──
        // When on-screen keyboard is open, intercept browser scroll/select
        // for key navigation before anything else.
        if self.keyboard.is_open {
            match &msg {
                MidiMsg::Browser(MidiBrowserAction::Scroll { delta }) => {
                    let delta = *delta;
                    return self.update(Message::Keyboard(
                        mesh_widgets::keyboard::KeyboardMessage::MidiScroll(delta),
                    ));
                }
                MidiMsg::Browser(MidiBrowserAction::Select) => {
                    return self.update(Message::Keyboard(
                        mesh_widgets::keyboard::KeyboardMessage::MidiSelect,
                    ));
                }
                _ => {} // Other messages fall through
            }
        }

        // ── Settings MIDI nav interception ──
        // When settings is open via MIDI, intercept browser scroll/select
        // to navigate settings instead of the collection browser.
        if self.settings.is_open && self.settings.settings_midi_nav.is_some() {
            match &msg {
                MidiMsg::Browser(MidiBrowserAction::Scroll { delta }) => {
                    let delta = *delta;
                    return self.handle_settings_midi_scroll(delta);
                }
                MidiMsg::Browser(MidiBrowserAction::Select) => {
                    return self.handle_settings_midi_select();
                }
                _ => {} // Other messages fall through
            }
        }

        match msg {
            MidiMsg::Deck { deck, action } if engine_dispatched => {
                // Engine already processed this — skip UI dispatch to avoid double-execution.
                // UI state (playhead, play indicator) will update from atomics on next tick.
                log::trace!("MIDI: Deck {} action {:?} already dispatched to engine", deck, action);
                return Task::none();
            }
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
                                super::deck_view::ActionButtonMode::Performance
                                | super::deck_view::ActionButtonMode::HotCue => Some(DeckMessage::HotCuePressed(slot)),
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
                                super::deck_view::ActionButtonMode::Performance
                                | super::deck_view::ActionButtonMode::HotCue => Some(DeckMessage::HotCueReleased(slot)),
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
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::Performance))
                        }
                    }
                    MidiDeckAction::SetHotCueMode { enabled } => {
                        if enabled {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::HotCue))
                        } else {
                            Some(DeckMessage::SetActionMode(super::deck_view::ActionButtonMode::Performance))
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
                    MidiDeckAction::SetSuggestionEnergy(value) => {
                        return self.update(Message::CollectionBrowser(
                            CollectionBrowserMessage::SetEnergyDirection(value),
                        ));
                    }
                    MidiDeckAction::ToggleSlip => Some(DeckMessage::ToggleSlip),
                    MidiDeckAction::ToggleKeyMatch => Some(DeckMessage::ToggleKeyMatch),
                    MidiDeckAction::LoadSelected => {
                        // If a track is selected, load it to this deck
                        if let Some(track_path) = self.collection_browser.get_selected_track_path() {
                            let _ = self.update(Message::LoadTrack(deck, track_path.to_string_lossy().to_string()));
                            self.hide_browser_overlay();
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
                self.show_browser_overlay();
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
                            self.confirm_stem_link_selection();
                            Task::none()
                        } else {
                            // Navigate into folders/playlists only — never load tracks.
                            // Track loading is handled by deck.load_selected (dedicated load buttons).
                            self.update(Message::CollectionBrowser(
                                CollectionBrowserMessage::NavigateInto,
                            ))
                        }
                    }
                    MidiBrowserAction::Back => {
                        self.show_browser_overlay();
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
                    MidiGlobalAction::SetBpm(bpm) => {
                        let bpm_rounded = bpm.round();
                        self.domain.set_global_bpm_with_engine(bpm_rounded);
                    }
                    MidiGlobalAction::AdjustBpm(_delta) => {
                        // Relative BPM adjustment — not needed yet
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
                    MidiGlobalAction::SettingsToggle => {
                        if self.settings.is_open {
                            // Restore browse mode state from before settings opened
                            if let Some(ref nav) = self.settings.settings_midi_nav {
                                let saved = nav.saved_browse_state;
                                if let Some(ref ctrl) = self.controller {
                                    ctrl.set_browse_mode(0, saved[0]);
                                    ctrl.set_browse_mode(1, saved[1]);
                                }
                            }
                            self.settings.settings_midi_nav = None;
                            // Close handles auto-save internally
                            return self.update(Message::Settings(SettingsMessage::Close));
                        } else {
                            // Save current browse mode state, then force both sides active
                            // so encoders produce Browser::Scroll for settings navigation
                            let saved = if let Some(ref ctrl) = self.controller {
                                let s = [ctrl.get_browse_mode(0), ctrl.get_browse_mode(1)];
                                ctrl.set_browse_mode(0, true);
                                ctrl.set_browse_mode(1, true);
                                s
                            } else {
                                [false, false]
                            };
                            // Open: snapshot + activate MIDI nav
                            let _ = self.update(Message::Settings(SettingsMessage::Open));
                            self.settings.settings_midi_nav = Some(
                                super::settings::SettingsMidiNav::new(saved)
                            );
                        }
                    }
                    MidiGlobalAction::BrowseModeChanged { side, active } => {
                        if side < 2 {
                            self.browse_mode_active[side] = active;
                        }
                        if active {
                            self.show_browser_overlay();
                        } else if !self.browse_mode_active[0] && !self.browse_mode_active[1] {
                            // Both sides off — hide overlay
                            self.hide_browser_overlay();
                        }
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

    /// Handle encoder scroll while in settings MIDI navigation mode.
    /// Priority: sub-panel → editing → browsing settings list.
    fn handle_settings_midi_scroll(&mut self, delta: i32) -> Task<Message> {
        use super::settings::{build_settings_entries, settings_entry_count, SubPanelFocus, SETTINGS_SCROLLABLE_ID};

        if self.settings.settings_midi_nav.is_none() {
            return Task::none();
        }

        // Sub-panel has highest priority — handle before taking other borrows
        if let Some(ref mut nav) = self.settings.settings_midi_nav {
            if let Some(ref mut panel) = nav.sub_panel {
                match panel {
                    SubPanelFocus::WifiNetworkList { selected } => {
                        let count = self.settings.network.as_ref()
                            .map(|n| n.networks.len()).unwrap_or(0);
                        if count > 0 {
                            *selected = if delta > 0 {
                                (*selected + 1) % count
                            } else {
                                (*selected + count - 1) % count
                            };
                            // Sync visual selection
                            let sel = *selected;
                            if let Some(ref mut net) = self.settings.network {
                                net.selected_network = Some(sel);
                            }
                        }
                        return Task::none();
                    }
                    SubPanelFocus::UpdateActions { selected } => {
                        let action_count = 2;
                        *selected = if delta > 0 {
                            (*selected + 1) % action_count
                        } else {
                            (*selected + action_count - 1) % action_count
                        };
                        return Task::none();
                    }
                }
            }
        }

        // Pre-compute values that need &self.settings before mutably borrowing nav
        let entries = build_settings_entries(&self.settings);
        let entry_count = settings_entry_count(&self.settings);

        let nav = self.settings.settings_midi_nav.as_mut().unwrap();

        if nav.editing {
            // Cycle through options for the focused setting
            if let Some(entry) = entries.get(nav.focused_index) {
                let count = entry.options.len();
                if count > 0 {
                    let new_idx = if delta > 0 {
                        (entry.selected + 1) % count
                    } else {
                        (entry.selected + count - 1) % count
                    };
                    let msg = (entry.on_select)(new_idx);
                    return self.update(Message::Settings(msg));
                }
            }
        } else {
            // Cycle focused_index through all settings (wrapping)
            if entry_count > 0 {
                let new_idx = if delta > 0 {
                    (nav.focused_index + 1) % entry_count
                } else {
                    (nav.focused_index + entry_count - 1) % entry_count
                };
                nav.focused_index = new_idx;
            }
        }

        // Scroll the settings container to keep the focused item visible
        let focused = nav.focused_index;
        let max_idx = entry_count.saturating_sub(1);
        let relative_y = if max_idx > 0 {
            (focused as f32 / max_idx as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let offset = iced::widget::operation::RelativeOffset { x: 0.0, y: relative_y };
        iced::widget::operation::snap_to(SETTINGS_SCROLLABLE_ID.clone(), offset)
    }

    /// Handle encoder press while in settings MIDI navigation mode.
    /// Shift+press always exits current mode (sub-panel → editing → scroll).
    /// Normal press: sub-panel activates action, otherwise toggles edit mode.
    /// For Network/Update entries: press in edit mode enters sub-panel.
    fn handle_settings_midi_select(&mut self) -> Task<Message> {
        use super::settings::SubPanelFocus;
        use super::network::NetworkMessage;
        use super::system_update::SystemUpdateMessage;

        if self.settings.settings_midi_nav.is_none() {
            return Task::none();
        }

        // Shift+press: step out of current mode
        let shift_held = self.controller.as_ref()
            .is_some_and(|c| c.is_shift_held());
        if shift_held {
            let nav = self.settings.settings_midi_nav.as_mut().unwrap();
            if nav.sub_panel.is_some() {
                nav.sub_panel = None;
            } else if nav.editing {
                nav.editing = false;
            }
            return Task::none();
        }

        // If in sub-panel, take it and activate the selected item
        let sub_panel = self.settings.settings_midi_nav.as_mut()
            .and_then(|n| n.sub_panel.take());
        if let Some(panel) = sub_panel {
            match panel {
                SubPanelFocus::WifiNetworkList { selected } => {
                    return self.update(Message::Network(NetworkMessage::SelectNetwork(selected)));
                }
                SubPanelFocus::UpdateActions { selected } => {
                    match selected {
                        0 => return self.update(Message::SystemUpdate(SystemUpdateMessage::CheckForUpdate)),
                        1 => {
                            if let Some(ref us) = self.settings.update {
                                if us.is_install_complete() {
                                    return self.update(Message::SystemUpdate(SystemUpdateMessage::RestartCage));
                                } else if us.has_available_update() {
                                    return self.update(Message::SystemUpdate(SystemUpdateMessage::InstallUpdate));
                                }
                            }
                        }
                        _ => {}
                    }
                    return Task::none();
                }
            }
        }

        // Pre-compute indices before mutably borrowing nav
        let base_entries = 13usize;
        let has_network = self.settings.network.is_some();
        let has_update = self.settings.update.is_some();
        let network_entry_idx = if has_network { Some(base_entries) } else { None };
        let update_entry_idx = if has_update {
            Some(base_entries + if has_network { 1 } else { 0 })
        } else { None };
        let initial_net_selection = self.settings.network.as_ref()
            .and_then(|n| n.selected_network)
            .unwrap_or(0);
        let networks_empty = self.settings.network.as_ref()
            .is_some_and(|n| n.networks.is_empty());

        // Compute MIDI Learn entry index (always last)
        let midi_learn_idx = base_entries
            + if has_network { 1 } else { 0 }
            + if has_update { 1 } else { 0 };

        let nav = self.settings.settings_midi_nav.as_mut().unwrap();

        // Network/Update: go directly to sub-panel (no editing step needed)
        if Some(nav.focused_index) == network_entry_idx {
            nav.sub_panel = Some(SubPanelFocus::WifiNetworkList { selected: initial_net_selection });
            if networks_empty {
                return self.update(Message::Network(NetworkMessage::Scan));
            }
            return Task::none();
        } else if Some(nav.focused_index) == update_entry_idx {
            nav.sub_panel = Some(SubPanelFocus::UpdateActions { selected: 0 });
            return Task::none();
        }

        // MIDI Learn: press triggers Start directly
        if nav.focused_index == midi_learn_idx {
            return self.update(Message::MidiLearn(
                super::midi_learn::MidiLearnMessage::Start,
            ));
        }

        // Regular settings: toggle editing mode
        if nav.editing {
            nav.editing = false;
        } else {
            nav.editing = true;
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

        // LED feedback subscription — 30Hz timer, only when controller is connected
        let led_sub = if self.controller.is_some() {
            time::every(std::time::Duration::from_millis(33)).map(|_| Message::UpdateLeds)
        } else {
            Subscription::none()
        };

        // Journal polling subscription for OTA update progress
        let journal_poll_sub = if self.settings.is_open && self.settings.update.as_ref().is_some_and(|u| u.is_installing()) {
            time::every(std::time::Duration::from_secs(2))
                .map(|_| Message::SystemUpdate(super::system_update::SystemUpdateMessage::PollJournal))
        } else {
            Subscription::none()
        };

        Subscription::batch([
            // Update UI synced to display refresh rate (60Hz, 120Hz, etc.)
            iced::window::frames().map(|_| Message::Tick),
            // Background track load results (delivered as messages, no polling needed)
            mpsc_subscription(self.domain.track_loader_result_receiver())
                .map(|result| Message::TrackLoaded(TrackLoadedMsg(Arc::new(result)))),
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
            // OTA update journal polling (2s interval, only during install)
            journal_poll_sub,
            // LED feedback evaluation (30Hz timer, only when controller connected)
            led_sub,
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

    /// Performance mode: full-screen waveforms, browser shown as overlay on demand
    /// Browser appears via MIDI browse mode or encoder interaction, auto-hides after 5s
    fn view_performance_mode(&self) -> Element<'_, Message> {
        let header = self.view_header();
        let canvas = view_player_canvas(&self.player_canvas_state);
        let status_bar = container(text(&self.status).size(12))
            .padding(5)
            .height(Length::Shrink);

        column![
            header,
            container(canvas)
                .width(Length::Fill)
                .height(Length::Fill),
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
        // Gets the majority of vertical space so waveforms aren't cramped
        let main_row = container(
            row![
                left_controls,
                center_column,
                right_controls,
            ]
            .spacing(10)
        )
        .height(Length::FillPortion(7));

        // Bottom row: Collection browser (left) | Mixer (right)
        // Compact area — browser scrolls within its allocated space
        let collection_browser = self.collection_browser.view().map(Message::CollectionBrowser);
        let mixer = self.mixer_view.view().map(Message::Mixer);
        let bottom_row = container(
            row![
                container(collection_browser).width(Length::FillPortion(3)),
                container(mixer).width(Length::FillPortion(2)),
            ]
            .spacing(10)
        )
        .height(Length::FillPortion(3));

        // Status bar
        let status_bar = container(
            text(&self.status).size(12)
        )
        .padding(5)
        .height(Length::Shrink);

        column![
            header,
            main_row,
            bottom_row,
            status_bar,
        ]
        .spacing(10)
        .padding(10)
        .height(Length::Fill)
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

        // Browser overlay in performance mode (above content, below FX dropdown and modals)
        let with_browser: Element<'a, Message> = if self.browser_visible && self.app_mode == AppMode::Performance {
            let browser = self.collection_browser.view_compact().map(Message::CollectionBrowser);

            // Semi-transparent backdrop — click to dismiss
            let backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.4).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::HideBrowserOverlay);

            // Browser panel pinned to bottom ~45% of screen
            let browser_panel = column![
                Space::new().height(Length::FillPortion(11)),  // top spacer
                container(browser)
                    .width(Length::Fill)
                    .height(Length::FillPortion(9))            // ~45%
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.05, 0.05, 0.08, 0.95).into()),
                        ..Default::default()
                    }),
            ]
            .width(Length::Fill)
            .height(Length::Fill);

            stack![with_drawer, backdrop, browser_panel].into()
        } else {
            with_drawer
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

            stack![with_browser, backdrop, dropdown_list].into()
        } else {
            with_browser
        };

        // Overlay settings modal if open
        let with_modal: Element<'a, Message> = if self.settings.is_open {
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
        };

        // On-screen keyboard overlay (topmost — can appear above settings for WiFi password)
        if self.keyboard.is_open {
            let kb_backdrop = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.5).into()),
                        ..Default::default()
                    }),
            )
            .on_press(Message::Keyboard(mesh_widgets::keyboard::KeyboardMessage::Cancel));

            let kb_modal = center(opaque(keyboard_view(&self.keyboard).map(Message::Keyboard)))
                .width(Length::Fill)
                .height(Length::Fill);

            stack![with_modal, kb_backdrop, kb_modal].into()
        } else {
            with_modal
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

        // Latency readout: output + internal pipeline latency
        let latency_label: Element<'_, Message> = if let Some(ref output_lat) = self.output_latency_samples {
            let output_samples = output_lat.load(std::sync::atomic::Ordering::Relaxed);
            let internal_samples = self.internal_latency_samples
                .as_ref()
                .map(|a| a.load(std::sync::atomic::Ordering::Relaxed))
                .unwrap_or(0);
            let sr = self.audio_sample_rate as f64;
            let total_ms = if sr > 0.0 {
                ((output_samples as f64 + internal_samples as f64) / sr) * 1000.0
            } else {
                0.0
            };
            text(format!("{:.1}ms", total_ms))
                .size(11)
                .color(Color::from_rgb(0.5, 0.5, 0.5))
                .into()
        } else {
            text("").into()
        };

        row![
            title,
            Space::new().width(Fill),
            bpm_label,
            bpm_slider,
            Space::new().width(20),
            fx_element,
            Space::new().width(Fill),
            connection_status,
            latency_label,
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
