//! Main application state and iced implementation
//!
//! The application follows a domain-driven architecture:
//! - `MeshCueDomain` handles all business logic, services, and state
//! - `MeshCueApp` handles display and user input only

use crate::audio::{AudioHandle, AudioState};
use crate::config;
use crate::domain::MeshCueDomain;
use crate::keybindings::{self, KeybindingsConfig};
use iced::widget::{button, column, container, mouse_area, row, stack, text, Space};
use iced::{Color, Element, Length, Task, Theme};
use super::modals::with_modal_overlay;
use mesh_core::playlist::NodeId;
use mesh_widgets::mpsc_subscription;
use std::sync::Arc;

// Re-export extracted modules for use by other UI modules
pub use super::message::Message;
pub use super::state::{
    BrowserSide, CollectionState, DragState, ExportPhase, ExportState,
    ImportPhase, ImportState, LinkedStemLoadedMsg, LoadedTrackState,
    PendingDragState, ReanalysisState, SettingsState, StemsLoadResult, View,
    DRAG_THRESHOLD,
};
pub use super::utils::{
    build_tree_nodes, get_tracks_for_folder, nudge_beat_grid, regenerate_beat_grid,
    snap_to_nearest_beat, tracks_to_rows, update_waveform_beat_grid, BEAT_GRID_NUDGE_SAMPLES,
};

// State types imported from super::state

// State types (LoadedTrackState, SettingsState, ImportState, etc.) are now in state/ module
// Message enum is now in message.rs

/// Main application
///
/// The UI layer holds only display and input state. Business logic and
/// service management is delegated to `MeshCueDomain`.
pub struct MeshCueApp {
    // ═══════════════════════════════════════════════════════════════════════
    // Domain Layer (owns all business logic and services)
    // ═══════════════════════════════════════════════════════════════════════

    /// Domain layer managing all business logic
    pub(crate) domain: MeshCueDomain,

    // ═══════════════════════════════════════════════════════════════════════
    // UI State
    // ═══════════════════════════════════════════════════════════════════════

    /// Current view
    pub(crate) current_view: View,
    /// Collection UI state (browser panels, selections, drag state)
    pub(crate) collection: CollectionState,
    /// Audio playback state
    pub(crate) audio: AudioState,
    /// Audio handle (keeps audio running)
    #[allow(dead_code)]
    audio_handle: Option<AudioHandle>,
    /// Settings modal state
    pub(crate) settings: SettingsState,
    /// Whether shift key is currently held (for shift+click actions)
    pub(crate) shift_held: bool,
    /// Whether ctrl key is currently held (for ctrl+click toggle selection)
    pub(crate) ctrl_held: bool,
    /// Keybindings configuration
    pub(crate) keybindings: KeybindingsConfig,
    /// Hot cue keys currently pressed (for filtering key repeat)
    pub(crate) pressed_hot_cue_keys: std::collections::HashSet<usize>,
    /// Main cue key currently pressed
    pub(crate) pressed_cue_key: bool,
    /// Batch import UI state
    pub(crate) import_state: ImportState,
    /// Delete confirmation state
    pub(crate) delete_state: super::delete_modal::DeleteState,
    /// Context menu state
    pub(crate) context_menu_state: super::context_menu::ContextMenuState,
    /// Global mouse position (window coordinates) for context menu placement
    pub(crate) global_mouse_position: iced::Point,
    /// Stem link selection mode - Some(stem_index) when selecting a source track for linking
    pub(crate) stem_link_selection: Option<usize>,
    /// Re-analysis UI state
    pub(crate) reanalysis_state: ReanalysisState,
    /// USB export UI state
    pub(crate) export_state: ExportState,
    /// Effects editor modal state
    pub(crate) effects_editor: super::effects_editor::EffectsEditorState,
    /// Effect picker modal state
    pub(crate) effect_picker: super::effect_picker::EffectPickerState,
    /// Plugin GUI manager for CLAP parameter learning
    pub(crate) plugin_gui_manager: super::plugin_gui::PluginGuiManager,
}

/// Extract the playlists subtree from the tree nodes for the export modal
fn find_playlists_tree(nodes: &[mesh_widgets::TreeNode<NodeId>]) -> Vec<mesh_widgets::TreeNode<NodeId>> {
    for node in nodes {
        if node.id.0 == "playlists" {
            // Return the children of the playlists root
            return node.children.clone();
        }
        // Recurse into children
        let result = find_playlists_tree(&node.children);
        if !result.is_empty() {
            return result;
        }
    }
    Vec::new()
}

impl MeshCueApp {
    /// Create a new application instance
    pub fn new() -> (Self, Task<Message>) {
        // Load configuration
        let config_path = config::default_config_path();
        let config = config::load_config(&config_path);
        log::info!(
            "Loaded config: BPM range {}-{}",
            config.analysis.bpm.min_tempo,
            config.analysis.bpm.max_tempo
        );

        // Load keybindings
        let keybindings_path = keybindings::default_keybindings_path();
        let keybindings = keybindings::load_keybindings(&keybindings_path);
        log::info!("Loaded keybindings from {:?}", keybindings_path);

        let settings = SettingsState::from_config(&config);

        // Initialize collection state first (needed for collection_root)
        let mut collection_state = CollectionState::default();
        let collection_root = collection_state.collection_path.clone();

        // ═══════════════════════════════════════════════════════════════════════
        // Initialize Domain Layer
        // ═══════════════════════════════════════════════════════════════════════
        // Domain layer owns: database service, playlist storage, USB manager, config
        let domain = MeshCueDomain::new(
            collection_root.clone(),
            Arc::new(config),
            config_path,
        ).expect("Failed to initialize domain layer");

        log::info!("Domain layer initialized at {:?}", collection_root);

        // Build initial tree from domain
        collection_state.tree_nodes = domain.tree_nodes().to_vec();

        // Expand root nodes by default
        collection_state.browser_left.tree_state.expand(NodeId::tracks());
        collection_state.browser_left.tree_state.expand(NodeId::playlists());
        collection_state.browser_right.tree_state.expand(NodeId::tracks());
        collection_state.browser_right.tree_state.expand(NodeId::playlists());

        // Set left browser to show tracks (collection) by default
        collection_state.browser_left.set_current_folder(NodeId::tracks());
        // Convert domain tracks to UI format
        if let Ok(tracks) = domain.get_tracks_for_node(&NodeId::tracks()) {
            collection_state.left_tracks = tracks_to_rows(&tracks);
        }

        // Start audio system for preview (lock-free architecture)
        // Domain owns the db_service internally
        let (mut audio, audio_handle) = match domain.init_audio_preview() {
            Ok((audio_state, handle)) => {
                log::info!("Audio preview enabled (lock-free)");
                (audio_state, Some(handle))
            }
            Err(e) => {
                log::warn!("Audio not available: {} - audio preview disabled", e);
                (AudioState::disconnected(), None)
            }
        };

        // Apply initial config settings to audio engine
        audio.set_scratch_interpolation(settings.draft_scratch_interpolation);

        let app = Self {
            domain,
            current_view: View::Collection,
            collection: collection_state,
            audio,
            audio_handle,
            settings,
            shift_held: false,
            ctrl_held: false,
            keybindings,
            pressed_hot_cue_keys: std::collections::HashSet::new(),
            pressed_cue_key: false,
            import_state: ImportState::default(),
            delete_state: Default::default(),
            context_menu_state: Default::default(),
            global_mouse_position: iced::Point::ORIGIN,
            stem_link_selection: None,
            reanalysis_state: ReanalysisState::default(),
            export_state: ExportState::default(),
            effects_editor: super::effects_editor::EffectsEditorState::new(),
            effect_picker: super::effect_picker::EffectPickerState::new(),
            plugin_gui_manager: super::plugin_gui::PluginGuiManager::new(),
        };

        // Initial collection scan and playlist refresh
        let cmd = Task::batch([
            Task::perform(async {}, |_| Message::RefreshCollection),
            Task::perform(async {}, |_| Message::RefreshPlaylists),
        ]);

        (app, cmd)
    }

    /// Application title
    pub fn title(&self) -> String {
        String::from("mesh-cue - Track Preparation")
    }

    /// Save current track if it has been modified
    ///
    /// Saves all metadata to the database (single source of truth).
    /// WAV files are not modified - they contain only audio data.
    fn save_current_track_if_modified(&mut self) -> Option<Task<Message>> {
        if let Some(ref mut state) = self.collection.loaded_track {
            if state.modified {
                let result = self.domain.save_track_metadata(
                    &state.path,
                    state.bpm,
                    &state.key,
                    state.drop_marker,
                    state.beat_grid.first().copied().unwrap_or(0),
                    &state.cue_points,
                    &state.saved_loops,
                    &state.stem_links,
                );

                if let Err(e) = result {
                    log::error!("Auto-save failed: {:?}", e);
                    return None;
                }

                state.modified = false;
                log::info!("Auto-saved track to database: {:?}", state.path);
            }
        }
        None
    }

    /// Update state based on message
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // Navigation
            Message::SwitchView(view) => {
                // Auto-save when switching away from Collection view
                let save_task = if self.current_view == View::Collection && view != View::Collection {
                    self.save_current_track_if_modified()
                } else {
                    None
                };

                self.current_view = view;

                let refresh_task = if view == View::Collection {
                    Some(Task::perform(async {}, |_| Message::RefreshCollection))
                } else {
                    None
                };

                // Return combined tasks if any
                match (save_task, refresh_task) {
                    (Some(save), Some(refresh)) => return Task::batch([save, refresh]),
                    (Some(save), None) => return save,
                    (None, Some(refresh)) => return refresh,
                    (None, None) => {}
                }
            }

            // Collection: Browser
            Message::RefreshCollection => {
                // Rebuild tree from database
                self.domain.refresh_tree();
                self.collection.tree_nodes = self.domain.tree_nodes().to_vec();
                // Refresh track lists for both browsers
                if let Some(ref folder) = self.collection.browser_left.current_folder {
                    self.collection.left_tracks = self.domain.get_tracks_for_display(folder);
                }
                if let Some(ref folder) = self.collection.browser_right.current_folder {
                    self.collection.right_tracks = self.domain.get_tracks_for_display(folder);
                }
                log::info!("Refreshed collection tree and track lists from database");
            }
            // Track loading (delegated to handlers/track_loading.rs)
            Message::TrackMetadataLoaded(result) => return self.handle_track_metadata_loaded(result),
            Message::TrackStemsLoaded(result) => return self.handle_track_stems_loaded(result),
            Message::LinkedStemLoaded(msg) => return self.handle_linked_stem_loaded(msg),

            // Collection: Editor (delegated to handlers/editing.rs)
            Message::SetBpm(bpm) => return self.handle_set_bpm(bpm),
            Message::IncreaseBpm => return self.handle_adjust_bpm(1.0),
            Message::DecreaseBpm => return self.handle_adjust_bpm(-1.0),
            Message::SetKey(key) => return self.handle_set_key(key),
            Message::AddCuePoint(position) => return self.handle_add_cue_point(position),
            Message::DeleteCuePoint(index) => return self.handle_delete_cue_point(index),
            Message::SetCueLabel(index, label) => return self.handle_set_cue_label(index, label),
            Message::SaveTrack => return self.handle_save_track(),
            Message::SaveComplete(result) => return self.handle_save_complete(result),

            // Transport (delegated to handlers/playback.rs)
            Message::Play => return self.handle_play(),
            Message::Pause => return self.handle_pause(),
            Message::Stop => return self.handle_stop(),
            Message::Seek(position) => return self.handle_seek(position),
            Message::ScratchStart => return self.handle_scratch_start(),
            Message::ScratchMove(position) => return self.handle_scratch_move(position),
            Message::ScratchEnd => return self.handle_scratch_end(),
            Message::ToggleLoop => return self.handle_toggle_loop(),
            Message::AdjustLoopLength(delta) => return self.handle_adjust_loop_length(delta),
            Message::Cue => return self.handle_cue(),
            Message::CueReleased => return self.handle_cue_released(),
            Message::BeatJump(beats) => return self.handle_beat_jump(beats),
            Message::SetOverviewGridBars(bars) => return self.handle_set_overview_grid_bars(bars),
            // Cue points (delegated to handlers/editing.rs)
            Message::JumpToCue(index) => return self.handle_jump_to_cue(index),
            Message::SetCuePoint(index) => return self.handle_set_cue_point(index),
            Message::ClearCuePoint(index) => return self.handle_clear_cue_point(index),

            // Saved Loops (delegated to handlers/editing.rs)
            Message::SaveLoop(index) => return self.handle_save_loop(index),
            Message::JumpToSavedLoop(index) => return self.handle_jump_to_saved_loop(index),
            Message::ClearSavedLoop(index) => return self.handle_clear_saved_loop(index),

            // Drop Marker (delegated to handlers/editing.rs)
            Message::SetDropMarker => return self.handle_set_drop_marker(),
            Message::ClearDropMarker => return self.handle_clear_drop_marker(),

            // Stem Link handling (delegated to handlers/stem_links.rs)
            Message::StartStemLinkSelection(stem_idx) => return self.handle_start_stem_link_selection(stem_idx),
            Message::ConfirmStemLink(stem_idx) => return self.handle_confirm_stem_link(stem_idx),
            Message::ClearStemLink(stem_idx) => return self.handle_clear_stem_link(stem_idx),
            Message::ToggleStemLinkActive(stem_idx) => return self.handle_toggle_stem_link_active(stem_idx),

            // Slice Editor (delegated to handlers/slicer.rs)
            Message::SliceEditorCellToggle { step, slice } => return self.handle_slice_editor_cell_toggle(step, slice),
            Message::SliceEditorMuteToggle(step) => return self.handle_slice_editor_mute_toggle(step),
            Message::SliceEditorStemClick(stem_idx) => return self.handle_slice_editor_stem_click(stem_idx),
            Message::SliceEditorPresetSelect(preset_idx) => return self.handle_slice_editor_preset_select(preset_idx),
            Message::SaveSlicerPresets => return self.handle_save_slicer_presets(),

            // Hot cues (delegated to handlers/slicer.rs)
            Message::HotCuePressed(index) => return self.handle_hot_cue_pressed(index),
            Message::HotCueReleased(index) => return self.handle_hot_cue_released(index),

            // Tick (delegated to handlers/tick.rs)
            Message::Tick => return self.handle_tick(),

            // Zoomed Waveform (delegated to handlers/editing.rs)
            Message::SetZoomBars(bars) => return self.handle_set_zoom_bars(bars),

            // Beat Grid (delegated to handlers/editing.rs)
            Message::NudgeBeatGridLeft => return self.handle_nudge_beat_grid_left(),
            Message::NudgeBeatGridRight => return self.handle_nudge_beat_grid_right(),
            Message::AlignBeatGridToPlayhead => return self.handle_align_beat_grid_to_playhead(),

            // Settings (delegated to handlers/settings.rs)
            Message::OpenSettings => return self.handle_open_settings(),
            Message::CloseSettings => return self.handle_close_settings(),
            Message::UpdateSettingsMinTempo(value) => return self.handle_update_settings_min_tempo(value),
            Message::UpdateSettingsMaxTempo(value) => return self.handle_update_settings_max_tempo(value),
            Message::UpdateSettingsParallelProcesses(value) => return self.handle_update_settings_parallel_processes(value),
            Message::UpdateSettingsTrackNameFormat(value) => return self.handle_update_settings_track_name_format(value),
            Message::UpdateSettingsGridBars(value) => return self.handle_update_settings_grid_bars(value),
            Message::UpdateSettingsBpmSource(source) => return self.handle_update_settings_bpm_source(source),
            Message::UpdateSettingsSlicerBufferBars(bars) => return self.handle_update_settings_slicer_buffer_bars(bars),
            Message::UpdateSettingsOutputPair(idx) => return self.handle_update_settings_output_pair(idx),
            Message::UpdateSettingsScratchInterpolation(method) => return self.handle_update_settings_scratch_interpolation(method),
            Message::UpdateSettingsSeparationBackend(backend) => return self.handle_update_settings_separation_backend(backend),
            Message::UpdateSettingsSeparationModel(model) => return self.handle_update_settings_separation_model(model),
            Message::UpdateSettingsSeparationShifts(shifts) => return self.handle_update_settings_separation_shifts(shifts),
            Message::RefreshAudioDevices => return self.handle_refresh_audio_devices(),
            Message::SaveSettings => return self.handle_save_settings(),
            Message::SaveSettingsComplete(result) => return self.handle_save_settings_complete(result),
            // Keyboard input (delegated to handlers/keyboard.rs)
            Message::KeyPressed(key, modifiers, repeat) => return self.handle_key_pressed(key, modifiers, repeat),
            Message::KeyReleased(key, modifiers) => return self.handle_key_released(key, modifiers),
            Message::ModifiersChanged(modifiers) => return self.handle_modifiers_changed(modifiers),
            Message::GlobalMouseMoved(position) => return self.handle_global_mouse_moved(position),

            // Playlist Browsers (delegated to handlers/browser.rs)
            Message::BrowserLeft(msg) => return self.handle_browser_left(msg),
            Message::BrowserRight(msg) => return self.handle_browser_right(msg),
            Message::RefreshPlaylists => return self.handle_refresh_playlists(),
            Message::LoadTrackByPath(path) => {
                // Auto-save current track if modified before loading new one
                let save_task = self.save_current_track_if_modified();

                log::info!("LoadTrackByPath: Loading {:?}", path);

                // Load metadata from database (includes stem link conversion)
                let result = match self.domain.get_track_metadata(&path) {
                    Ok(Some(metadata)) => Ok((path, metadata)),
                    Ok(None) => Err(format!("Track not found in database: {:?}", path)),
                    Err(e) => Err(format!("Database error: {}", e)),
                };

                // Process the result through the message handler
                let load_task = Task::done(Message::TrackMetadataLoaded(result));

                if let Some(save) = save_task {
                    return Task::batch([save, load_task]);
                }
                return load_task;
            }

            // Drag and Drop (delegated to handlers/browser.rs)
            Message::DragTrackStart { track_ids, track_names, browser } => {
                return self.handle_drag_track_start(track_ids, track_names, browser)
            }
            Message::DragTrackEnd => return self.handle_drag_track_end(),
            Message::DropTracksOnPlaylist { track_ids, target_playlist } => {
                return self.handle_drop_tracks_on_playlist(track_ids, target_playlist)
            }

            // Batch Import (delegated to handlers/import.rs)
            Message::OpenImport => return self.handle_open_import(),
            Message::CloseImport => return self.handle_close_import(),
            Message::SetImportMode(mode) => return self.handle_set_import_mode(mode),
            Message::ScanImportFolder => return self.handle_scan_import_folder(),
            Message::ImportFolderScanned(groups) => return self.handle_import_folder_scanned(groups),
            Message::MixedAudioFolderScanned(files) => return self.handle_mixed_audio_folder_scanned(files),
            Message::StartBatchImport => return self.handle_start_batch_import(),
            Message::StartMixedAudioImport => return self.handle_start_mixed_audio_import(),
            Message::ImportProgressUpdate(progress) => return self.handle_import_progress_update(progress),
            Message::CancelImport => return self.handle_cancel_import(),
            Message::DismissImportResults => return self.handle_dismiss_import_results(),

            // USB Export (delegated to handlers/export.rs)
            Message::OpenExport => return self.handle_open_export(),
            Message::CloseExport => return self.handle_close_export(),
            Message::SelectExportDevice(idx) => return self.handle_select_export_device(idx),
            Message::ToggleExportPlaylist(id) => return self.handle_toggle_export_playlist(id),
            Message::ToggleExportPlaylistExpand(id) => return self.handle_toggle_export_playlist_expand(id),
            Message::ToggleExportConfig => return self.handle_toggle_export_config(),
            Message::BuildSyncPlan => return self.handle_build_sync_plan(),
            Message::StartExport => return self.handle_start_export(),
            Message::CancelExport => return self.handle_cancel_export(),
            Message::UsbMessage(usb_msg) => return self.handle_usb_message(usb_msg),
            Message::DismissExportResults => return self.handle_dismiss_export_results(),

            // Delete confirmation (delegated to handlers/delete.rs)
            Message::RequestDelete(browser_side) => return self.handle_request_delete(browser_side),
            Message::CancelDelete => return self.handle_cancel_delete(),
            Message::ConfirmDelete => return self.handle_confirm_delete(),

            // Context menu (delegated to handlers/delete.rs)
            Message::RequestDeleteById(track_id) => return self.handle_request_delete_by_id(track_id),
            Message::RequestDeletePlaylist(playlist_id) => return self.handle_request_delete_playlist(playlist_id),
            Message::ShowContextMenu(kind, position) => return self.handle_show_context_menu(kind, position),
            Message::CloseContextMenu => return self.handle_close_context_menu(),

            // Reanalysis (delegated to handlers/reanalysis.rs)
            Message::StartReanalysis { analysis_type, scope } => return self.handle_start_reanalysis(analysis_type, scope),
            Message::ReanalysisProgress(progress) => return self.handle_reanalysis_progress(progress),
            Message::CancelReanalysis => return self.handle_cancel_reanalysis(),
            Message::StartRenamePlaylist(playlist_id) => return self.handle_start_rename_playlist(playlist_id),

            // Effects Editor
            Message::OpenEffectsEditor => {
                return self.handle_open_effects_editor();
            }
            Message::CloseEffectsEditor => {
                return self.handle_close_effects_editor();
            }
            Message::EffectsEditor(editor_msg) => {
                // Delegate to effects editor handler
                return self.handle_effects_editor(editor_msg);
            }
            Message::EffectsEditorNewPreset => {
                return self.handle_effects_editor_new_preset();
            }
            Message::EffectsEditorOpenSaveDialog => {
                self.effects_editor.open_save_dialog();
            }
            Message::EffectsEditorSavePreset(name) => {
                return self.handle_effects_editor_save(name);
            }
            Message::EffectsEditorCloseSaveDialog => {
                self.effects_editor.close_save_dialog();
            }
            Message::EffectsEditorSetPresetName(name) => {
                self.effects_editor.editor.preset_name_input = name;
            }

            // Effects Editor Audio Preview
            Message::EffectsEditorTogglePreview => {
                return self.handle_effects_editor_toggle_preview();
            }
            Message::EffectsEditorSetPreviewStem(stem) => {
                return self.handle_effects_editor_set_preview_stem(stem);
            }

            // Effect Picker
            Message::EffectPicker(picker_msg) => {
                return self.handle_effect_picker(picker_msg);
            }

            // Plugin GUI Learning Mode
            Message::PluginGuiTick => {
                return self.poll_learning_mode();
            }
        }

        Task::none()
    }

    /// Render the UI
    pub fn view(&self) -> Element<'_, Message> {
        let header = self.view_header();

        let content: Element<Message> = match self.current_view {
            View::Collection => self.view_collection(),
        };

        // Global status bars at bottom (visible when import, export, or re-analysis is active)
        let import_bar = super::import_modal::view_progress_bar(&self.import_state);
        let export_bar = super::export_modal::view_progress_bar(&self.export_state);
        let reanalysis_bar = self.view_reanalysis_progress_bar();

        let mut main = column![header, content].spacing(10);
        if let Some(bar) = import_bar {
            main = main.push(bar);
        }
        if let Some(bar) = export_bar {
            main = main.push(bar);
        }
        if let Some(bar) = reanalysis_bar {
            main = main.push(bar);
        }

        let base: Element<Message> = container(main)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .into();

        // Overlay modals if open (export > import > delete > settings > context menu)
        if self.export_state.is_open {
            // Export modal needs special handling for playlist tree extraction
            let playlist_tree = find_playlists_tree(&self.collection.tree_nodes);
            with_modal_overlay(
                base,
                super::export_modal::view(&self.export_state, playlist_tree),
                Message::CloseExport,
            )
        } else if self.import_state.is_open {
            with_modal_overlay(
                base,
                super::import_modal::view(&self.import_state),
                Message::CloseImport,
            )
        } else if self.delete_state.is_open {
            with_modal_overlay(
                base,
                super::delete_modal::view(&self.delete_state),
                Message::CancelDelete,
            )
        } else if self.settings.is_open {
            with_modal_overlay(
                base,
                super::settings::view(&self.settings),
                Message::CloseSettings,
            )
        } else if self.effects_editor.is_open {
            // Effects editor modal
            if let Some(editor_view) = super::effects_editor::effects_editor_view(&self.effects_editor) {
                let editor_modal = with_modal_overlay(base, editor_view, Message::CloseEffectsEditor);

                // If effect picker is also open, layer it on top
                if self.effect_picker.is_open {
                    use iced::widget::{center, opaque};

                    let picker_backdrop: Element<Message> = mouse_area(
                        container(Space::new())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(|_theme| container::Style {
                                background: Some(iced::Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                                ..Default::default()
                            }),
                    )
                    .on_press(Message::EffectPicker(super::effect_picker::EffectPickerMessage::Close))
                    .into();

                    // Get available effects from domain
                    let pd_effects = self.domain.available_effects();
                    let clap_plugins = self.domain.available_clap_plugins();
                    let picker_view = self.effect_picker.view(&pd_effects, &clap_plugins)
                        .map(Message::EffectPicker);

                    let picker_modal = center(opaque(picker_view))
                        .width(Length::Fill)
                        .height(Length::Fill);

                    stack![editor_modal, picker_backdrop, picker_modal].into()
                } else {
                    editor_modal
                }
            } else {
                base
            }
        } else if self.context_menu_state.is_open {
            // Context menu uses transparent backdrop and positioned content
            let backdrop: Element<Message> = mouse_area(
                container(Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .on_press(Message::CloseContextMenu)
            .into();

            if let Some(menu) = super::context_menu::view(&self.context_menu_state) {
                // Position the menu at the click location using spacers
                let pos = self.context_menu_state.position;
                let positioned_menu = column![
                    Space::new().height(Length::Fixed(pos.y)),
                    row![
                        Space::new().width(Length::Fixed(pos.x)),
                        menu,
                    ]
                ];

                stack![base, backdrop, positioned_menu].into()
            } else {
                base
            }
        } else if let Some(ref drag) = self.collection.dragging_track {
            // Show drag indicator near the cursor when dragging tracks
            let drag_text = drag.display_text();
            let indicator = container(text(drag_text).size(12))
                .padding([4, 8])
                .style(|_theme| container::Style {
                    background: Some(iced::Color::from_rgba(0.2, 0.2, 0.2, 0.85).into()),
                    border: iced::Border {
                        color: iced::Color::from_rgba(0.4, 0.4, 0.4, 0.8),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                });

            // Position indicator slightly below and right of the cursor
            let pos = self.global_mouse_position;
            let positioned_indicator = column![
                Space::new().height(Length::Fixed(pos.y + 16.0)),
                row![
                    Space::new().width(Length::Fixed(pos.x + 12.0)),
                    indicator,
                ]
            ];

            stack![base, positioned_indicator].into()
        } else {
            base
        }
    }

    /// Application theme
    pub fn theme(&self) -> Theme {
        Theme::Dark
    }

    /// Subscription for periodic UI updates and keyboard/mouse events
    pub fn subscription(&self) -> iced::Subscription<Message> {
        use iced::{event, keyboard, mouse, time, Event};
        use std::time::Duration;

        // Keyboard events for keybindings and modifier tracking
        let keyboard_sub = keyboard::listen().map(|event| {
            match event {
                keyboard::Event::KeyPressed { key, modifiers, repeat, .. } => {
                    Message::KeyPressed(key, modifiers, repeat)
                }
                keyboard::Event::KeyReleased { key, modifiers, .. } => {
                    Message::KeyReleased(key, modifiers)
                }
                keyboard::Event::ModifiersChanged(modifiers) => {
                    Message::ModifiersChanged(modifiers)
                }
            }
        });

        // Window events for tracking global mouse position (used for context menu placement)
        let mouse_sub = event::listen().map(|event| {
            match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Message::GlobalMouseMoved(position)
                }
                _ => Message::Tick, // Ignore other events
            }
        });

        // Mouse capture subscription for smooth knob dragging in effects editor
        // Only active when a knob is being dragged (to avoid overhead otherwise)
        let effects_editor_mouse_sub = if self.effects_editor.is_open
            && self.effects_editor.editor.is_any_knob_dragging()
        {
            event::listen_with(|event, _status, _id| {
                match event {
                    Event::Mouse(mouse::Event::CursorMoved { position }) => {
                        Some(Message::EffectsEditor(
                            mesh_widgets::MultibandEditorMessage::GlobalMouseMoved(position)
                        ))
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        Some(Message::EffectsEditor(
                            mesh_widgets::MultibandEditorMessage::GlobalMouseReleased
                        ))
                    }
                    _ => None,
                }
            })
        } else {
            iced::Subscription::none()
        };

        // Linked stem result subscription (engine owns the loader, we receive results)
        let linked_stem_sub = if let Some(receiver) = self.audio.linked_stem_receiver() {
            mpsc_subscription(receiver)
                .map(|result| Message::LinkedStemLoaded(LinkedStemLoadedMsg(Arc::new(result))))
        } else {
            iced::Subscription::none()
        };

        // USB manager subscription (event-driven device detection and export progress)
        let usb_sub = mpsc_subscription(self.domain.usb_message_receiver())
            .map(Message::UsbMessage);

        // Plugin GUI learning mode polling subscription
        // Only active when in learning mode (polls at ~20Hz for responsive learning)
        let learning_sub = if self.plugin_gui_manager.is_learning() {
            time::every(Duration::from_millis(50)).map(|_| Message::PluginGuiTick)
        } else {
            iced::Subscription::none()
        };

        // Always run tick at 60fps for smooth waveform animation
        // This matches mesh-player's approach and ensures cueing/preview states work correctly
        iced::Subscription::batch([
            keyboard_sub,
            mouse_sub,
            effects_editor_mouse_sub,
            time::every(Duration::from_millis(16)).map(|_| Message::Tick),
            linked_stem_sub,
            usb_sub,
            learning_sub,
        ])
    }

    /// Render a progress bar for re-analysis operations
    fn view_reanalysis_progress_bar(&self) -> Option<Element<'_, Message>> {
        if !self.reanalysis_state.is_running {
            return None;
        }

        let progress = if self.reanalysis_state.total_tracks > 0 {
            self.reanalysis_state.completed_tracks as f32 / self.reanalysis_state.total_tracks as f32
        } else {
            0.0
        };

        let analysis_name = self.reanalysis_state.analysis_type
            .map(|t| t.display_name())
            .unwrap_or("Analysis");

        let track_info = self.reanalysis_state.current_track
            .as_ref()
            .map(|name| {
                if name.len() > 40 {
                    format!("{}...", &name[..37])
                } else {
                    name.clone()
                }
            })
            .unwrap_or_default();

        let label = format!("Re-analysing {}: {}", analysis_name, track_info);
        let progress_text = format!(
            "{}/{}",
            self.reanalysis_state.completed_tracks,
            self.reanalysis_state.total_tracks
        );

        Some(super::import_modal::build_status_bar(
            label,
            progress_text,
            progress,
            Message::CancelReanalysis,
        ))
    }

    /// View header with app title and settings
    fn view_header(&self) -> Element<'_, Message> {
        // FX Presets button - simple primary style
        let fx_btn = button(text("FX Presets").size(14))
            .on_press(Message::OpenEffectsEditor)
            .padding([8, 16])
            .style(button::primary);

        // Settings gear icon (⚙ U+2699)
        let settings_btn = button(text("⚙").size(20))
            .on_press(Message::OpenSettings)
            .style(button::secondary);

        row![
            text("mesh-cue").size(24),
            Space::new().width(Length::Fill),
            fx_btn,
            settings_btn,
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// View for the collection browser and editor
    fn view_collection(&self) -> Element<'_, Message> {
        // Modifier key handling is done in update() where current keyboard state is available
        super::collection_browser::view(&self.collection, &self.import_state, self.stem_link_selection)
    }
}

// Helper functions (nudge_beat_grid, regenerate_beat_grid, etc.) moved to utils/ module
