//! Settings message handler
//!
//! Handles the settings modal: opening, closing, updating draft values, and saving.

use std::sync::Arc;
use iced::Task;

use crate::config;
use crate::ui::app::MeshApp;
use crate::ui::handlers::browser::trigger_suggestion_query;
use crate::ui::message::{Message, SettingsMessage};
use crate::ui::settings::SettingsState;

/// Handle settings messages
pub fn handle(app: &mut MeshApp, msg: SettingsMessage) -> Task<Message> {
    use SettingsMessage::*;

    match msg {
        Open => {
            let midi_nav = app.settings.settings_midi_nav.take();
            let network = app.settings.network.take();
            let update = app.settings.update.take();
            let recording_active = app.settings.recording_active;
            app.settings = SettingsState::from_config(&app.config);
            app.settings.available_theme_names = app.themes.iter().map(|t| t.name.clone()).collect();
            // Preserve stateful sections across reopen (avoid re-detecting nmcli/NixOS)
            app.settings.network = network;
            app.settings.update = update;
            app.settings.recording_active = recording_active;
            app.settings.is_open = true;
            app.settings.settings_midi_nav = midi_nav;
            app.settings.take_snapshot();
            // Refresh network status synchronously on open. D-Bus round-trips
            // are fast (~10-30ms total), imperceptible when opening a modal.
            // We avoid Task::perform here because iced silently drops tasks
            // returned from handlers called within the Settings(Open) path.
            if app.settings.network.is_some() {
                use crate::ui::network::backend;
                let has_wifi = backend::detect_wifi_adapter();
                let wifi = backend::get_wifi_status();
                let lan = backend::get_lan_status();
                if let Some(ref mut state) = app.settings.network {
                    state.has_wifi_adapter = has_wifi;
                    state.wifi_status = wifi;
                    state.lan_status = lan;
                }
            }
            Task::none()
        }
        Close => {
            // Auto-save if any settings were changed
            let save_task = if app.settings.has_changes() {
                handle(app, Save)
            } else {
                Task::none()
            };
            app.settings.is_open = false;
            app.settings.status.clear();
            app.settings.settings_midi_nav = None;
            save_task
        }
        UpdateLoopLength(index) => {
            app.settings.draft_loop_length_index = index;
            Task::none()
        }
        UpdateZoomBars(bars) => {
            app.settings.draft_zoom_bars = bars;
            Task::none()
        }
        UpdateGridBars(bars) => {
            app.settings.draft_grid_bars = bars;
            Task::none()
        }
        UpdateTheme(name) => {
            app.settings.draft_theme = name;
            Task::none()
        }
        UpdateThemeIndex(idx) => {
            if let Some(name) = app.settings.available_theme_names.get(idx) {
                app.settings.draft_theme = name.clone();
            }
            Task::none()
        }
        UpdatePhaseSync(enabled) => {
            app.settings.draft_phase_sync = enabled;
            Task::none()
        }
        UpdateAutoCue(enabled) => {
            app.settings.draft_auto_cue = enabled;
            Task::none()
        }
        UpdateSlicerBufferBars(bars) => {
            app.settings.draft_slicer_buffer_bars = bars;
            Task::none()
        }
        UpdateAutoGainEnabled(enabled) => {
            app.settings.draft_auto_gain_enabled = enabled;
            Task::none()
        }
        UpdateTargetLufs(index) => {
            app.settings.draft_target_lufs_index = index;
            Task::none()
        }
        UpdateShowLocalCollection(enabled) => {
            app.settings.draft_show_local_collection = enabled;
            Task::none()
        }
        UpdatePersistentBrowse(enabled) => {
            app.settings.draft_persistent_browse = enabled;
            Task::none()
        }
        UpdateSuggestionPlaylistSplit(enabled) => {
            app.settings.draft_suggestion_playlist_split = enabled;
            Task::none()
        }
        UpdateShowBrowserAnalytics(enabled) => {
            app.settings.draft_show_browser_analytics = enabled;
            Task::none()
        }
        UpdateSuggestionBlendMode(mode) => {
            app.settings.draft_suggestion_blend_mode = mode;
            Task::none()
        }
        UpdateSuggestionTransitionReach(reach) => {
            app.settings.draft_suggestion_transition_reach = reach;
            Task::none()
        }
        UpdateSuggestionKeyFilter(filter) => {
            app.settings.draft_suggestion_key_filter = filter;
            Task::none()
        }
        UpdateSuggestionStemComplement(enabled) => {
            app.settings.draft_suggestion_stem_complement = enabled;
            Task::none()
        }
        UpdateKeyScoringModel(model) => {
            app.settings.draft_key_scoring_model = model;
            Task::none()
        }
        UpdateWaveformLayout(layout) => {
            app.settings.draft_waveform_layout = layout;
            Task::none()
        }
        UpdateWaveformAbstraction(level) => {
            app.settings.draft_waveform_abstraction = level;
            // Takes effect immediately (uniform change, no reload needed)
            app.player_canvas_state.abstraction_level = level.as_level();
            Task::none()
        }
        UpdateFont(font) => {
            app.settings.draft_font = font;
            Task::none()
        }
        UpdateFontSize(size) => {
            app.settings.draft_font_size = size;
            Task::none()
        }
        UpdateMasterPair(index) => {
            app.settings.draft_master_device = index;
            Task::none()
        }
        UpdateCuePair(index) => {
            app.settings.draft_cue_device = index;
            Task::none()
        }
        RefreshAudioDevices => {
            app.settings.refresh_audio_devices();
            Task::none()
        }
        UpdatePrereleaseChannel(enabled) => {
            app.settings.draft_prerelease_channel = enabled;
            Task::none()
        }
        PowerOffConfirm => {
            app.settings.power_off_confirm = true;
            Task::none()
        }
        PowerOffCancel => {
            app.settings.power_off_confirm = false;
            if let Some(ref mut nav) = app.settings.settings_midi_nav {
                nav.sub_panel = None;
            }
            Task::none()
        }
        PowerOffExecute => {
            app.settings.power_off_confirm = false;
            #[cfg(feature = "embedded-rt")]
            {
                if let Err(e) = crate::ui::system_update::power_off() {
                    app.settings.status = format!("Power off failed: {}", e);
                }
            }
            Task::none()
        }
        RecordingConfirm => {
            app.settings.recording_confirm = true;
            Task::none()
        }
        RecordingCancel => {
            app.settings.recording_confirm = false;
            if let Some(ref mut nav) = app.settings.settings_midi_nav {
                nav.sub_panel = None;
            }
            Task::none()
        }
        RecordingExecute => {
            app.settings.recording_confirm = false;
            if let Some(ref mut nav) = app.settings.settings_midi_nav {
                nav.sub_panel = None;
            }
            // Toggle recording state
            if app.recording_state.is_some() {
                // Stop recording
                app.domain.send_command(mesh_core::engine::EngineCommand::StopRecording);
                // Dropping handles triggers graceful stop + WAV finalization
                app.recording_state = None;
                app.settings.recording_active = false;
                app.status = "Recording stopped".to_string();
            } else {
                // Start recording on all connected USB sticks
                let (event_tx, event_rx) = std::sync::mpsc::channel();
                let event_rx = std::sync::Arc::new(std::sync::Mutex::new(event_rx));
                let sample_rate = app.audio_sample_rate;
                let mut handles = Vec::new();

                // Get USB devices with mesh collections as recording targets
                let usb_mounts: Vec<(std::path::PathBuf, u64)> = app.collection_browser.usb_devices
                    .iter()
                    .filter(|d| d.has_mesh_collection)
                    .filter_map(|d| d.mount_point.clone().map(|mp| (mp, d.available_bytes)))
                    .collect();

                // Fallback: if no USB sticks with mesh DBs, record to local collection
                let is_local = usb_mounts.is_empty();
                let recording_targets = if is_local {
                    let local_path = app.domain.local_collection_path().to_path_buf();
                    vec![(local_path, u64::MAX)]
                } else {
                    usb_mounts
                };

                for (mount, available_bytes) in &recording_targets {
                    match mesh_core::recording::start_recording(mount, sample_rate, *available_bytes, event_tx.clone()) {
                        Ok((producer, handle)) => {
                            // Send producer to audio thread (boxed for EngineCommand size)
                            app.domain.send_command(
                                mesh_core::engine::EngineCommand::StartRecording {
                                    producer: Box::new(producer),
                                }
                            );
                            handles.push(handle);
                        }
                        Err(e) => {
                            log::error!("[RECORDING] Failed to start on {}: {e}", mount.display());
                        }
                    }
                }

                if handles.is_empty() {
                    app.status = "Failed to start recording".to_string();
                    app.settings.recording_active = false;
                    return Task::none();
                }

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;

                let count = handles.len();
                app.recording_state = Some(crate::ui::app::RecordingState {
                    started_at: std::time::Instant::now(),
                    started_at_ms: now_ms,
                    handles,
                    event_rx,
                    event_tx,
                    error_count: 0,
                });
                app.settings.recording_active = true;
                app.status = if is_local {
                    "Recording to local collection".to_string()
                } else {
                    format!("Recording to {} USB stick(s)", count)
                };
            }
            Task::none()
        }
        Save => {
            // Apply draft settings to config
            let mut new_config = (*app.config).clone();
            new_config.display.default_loop_length_index = app.settings.draft_loop_length_index;
            new_config.display.default_zoom_bars = app.settings.draft_zoom_bars;
            new_config.display.grid_bars = app.settings.draft_grid_bars;
            new_config.display.theme = app.settings.draft_theme.clone();
            new_config.display.show_local_collection = app.settings.draft_show_local_collection;
            new_config.display.persistent_browse = app.settings.draft_persistent_browse;
            new_config.display.suggestion_playlist_split = app.settings.draft_suggestion_playlist_split;
            new_config.display.suggestion_blend_mode = app.settings.draft_suggestion_blend_mode;
            new_config.display.suggestion_transition_reach = app.settings.draft_suggestion_transition_reach;
            new_config.display.suggestion_key_filter = app.settings.draft_suggestion_key_filter;
            new_config.display.suggestion_stem_complement = app.settings.draft_suggestion_stem_complement;
            new_config.display.show_browser_analytics = app.settings.draft_show_browser_analytics;
            new_config.display.key_scoring_model = app.settings.draft_key_scoring_model;
            new_config.display.waveform_layout = app.settings.draft_waveform_layout;
            new_config.display.waveform_abstraction = app.settings.draft_waveform_abstraction;
            new_config.display.font = app.settings.draft_font;
            new_config.display.font_size = app.settings.draft_font_size;
            // Save global BPM from current state
            new_config.audio.global_bpm = app.domain.global_bpm();
            // Save phase sync setting
            new_config.audio.phase_sync = app.settings.draft_phase_sync;
            // Save auto-cue intent (effective value accounts for same-device constraint)
            new_config.audio.auto_cue = app.settings.draft_auto_cue;
            // Save only buffer_bars (presets are read-only from shared file)
            new_config.slicer.buffer_bars = app.settings.draft_slicer_buffer_bars;
            // Save loudness settings
            new_config.audio.loudness.auto_gain_enabled = app.settings.draft_auto_gain_enabled;
            new_config.audio.loudness.target_lufs = crate::ui::settings::TARGET_LUFS_OPTIONS[app.settings.draft_target_lufs_index];
            // Save update channel preference
            new_config.updates.prerelease_channel = app.settings.draft_prerelease_channel;
            // Check if audio output devices changed
            let master_changed = app.config.audio.outputs.master_device != Some(app.settings.draft_master_device);
            let cue_changed = app.config.audio.outputs.cue_device != Some(app.settings.draft_cue_device);
            let audio_changed = master_changed || cue_changed;

            // Save audio output device configuration
            new_config.audio.outputs.master_device = Some(app.settings.draft_master_device);
            new_config.audio.outputs.cue_device = Some(app.settings.draft_cue_device);

            app.config = Arc::new(new_config.clone());

            // Hot-swap audio outputs if device selection changed
            if audio_changed {
                let success = mesh_core::audio::reconnect_ports(
                    &app.audio_client_name,
                    Some(app.settings.draft_master_device),
                    Some(app.settings.draft_cue_device),
                );
                if !success {
                    // CPAL backend requires restart - show message to user
                    app.status = "Audio device changed. Restart app to apply.".to_string();
                }
            }

            // Apply theme colors to waveform display and iced theme immediately
            let active_theme = mesh_widgets::theme::find_theme(&app.themes, &app.settings.draft_theme);
            app.player_canvas_state.set_stem_colors(active_theme.stem_colors());
            app.iced_theme = active_theme.iced_theme();
            app.collection_browser.browser.table_state.pill_color = Some(active_theme.stems[1]);
            app.collection_browser.browser.table_state.tag_category_colors = Some([active_theme.stems[1], active_theme.stems[0], active_theme.stems[3], active_theme.stems[2]]);

            // Apply waveform layout immediately
            app.player_canvas_state.set_vertical_layout(
                app.settings.draft_waveform_layout.is_vertical()
            );
            app.player_canvas_state.set_vertical_inverted(
                app.settings.draft_waveform_layout.is_inverted()
            );

            // Apply grid density and zoom level to all loaded decks
            for deck in &mut app.player_canvas_state.decks {
                deck.overview.set_grid_bars(app.settings.draft_grid_bars);
                deck.zoomed.set_zoom(app.settings.draft_zoom_bars);
            }

            // Apply waveform abstraction level
            app.player_canvas_state.abstraction_level = app.settings.draft_waveform_abstraction.as_level();

            // Apply local collection visibility change immediately
            app.collection_browser.set_show_local_collection(
                app.settings.draft_show_local_collection
            );

            // Apply browser analytics toggle immediately
            app.collection_browser.show_analytics = app.settings.draft_show_browser_analytics;

            // Send settings to audio engine via domain
            app.domain.set_phase_sync(app.settings.draft_phase_sync);
            // Auto-cue is only effective when master and cue outputs are different devices
            let effective_auto_cue = app.settings.draft_auto_cue
                && app.settings.draft_master_device != app.settings.draft_cue_device;
            app.domain.set_auto_cue(effective_auto_cue);
            // Send loudness config to engine (triggers recalculation for all loaded decks)
            app.domain.set_loudness_config(app.config.audio.loudness.clone());
            // Send slicer buffer bars to audio engine for all decks and stems
            let buffer_bars = new_config.slicer.validated_buffer_bars();
            app.domain.apply_slicer_buffer_bars_all(buffer_bars);

            // Refresh suggestions if active (picks up any config changes)
            let suggest_task = if app.collection_browser.is_suggestions_enabled() {
                app.collection_browser.set_suggestion_loading(true);
                trigger_suggestion_query(app)
            } else {
                Task::none()
            };

            // Save to disk in background
            let config_clone = new_config;
            let config_path = app.config_path.clone();
            let save_task = Task::perform(
                async move {
                    config::save_config(&config_clone, &config_path)
                        .map_err(|e| e.to_string())
                },
                |result| Message::Settings(SettingsMessage::SaveComplete(result)),
            );
            Task::batch([save_task, suggest_task])
        }
        SaveComplete(result) => {
            match result {
                Ok(()) => {
                    app.settings.status = "Settings saved".to_string();
                    app.status = "Settings saved".to_string();
                    app.settings.settings_midi_nav = None;
                }
                Err(e) => {
                    app.settings.status = format!("Save failed: {}", e);
                    app.status = format!("Settings save failed: {}", e);
                }
            }
            Task::none()
        }
    }
}
