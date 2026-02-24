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
            app.settings = SettingsState::from_config(&app.config);
            app.settings.available_theme_names = app.themes.iter().map(|t| t.name.clone()).collect();
            // Preserve stateful sections across reopen (avoid re-detecting nmcli/NixOS)
            app.settings.network = network;
            app.settings.update = update;
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
        Save => {
            // Apply draft settings to config
            let mut new_config = (*app.config).clone();
            new_config.display.default_loop_length_index = app.settings.draft_loop_length_index;
            new_config.display.default_zoom_bars = app.settings.draft_zoom_bars;
            new_config.display.grid_bars = app.settings.draft_grid_bars;
            new_config.display.theme = app.settings.draft_theme.clone();
            new_config.display.show_local_collection = app.settings.draft_show_local_collection;
            new_config.display.key_scoring_model = app.settings.draft_key_scoring_model;
            new_config.display.waveform_layout = app.settings.draft_waveform_layout;
            new_config.display.waveform_abstraction = app.settings.draft_waveform_abstraction;
            // Save global BPM from current state
            new_config.audio.global_bpm = app.domain.global_bpm();
            // Save phase sync setting
            new_config.audio.phase_sync = app.settings.draft_phase_sync;
            // Save only buffer_bars (presets are read-only from shared file)
            new_config.slicer.buffer_bars = app.settings.draft_slicer_buffer_bars;
            // Save loudness settings
            new_config.audio.loudness.auto_gain_enabled = app.settings.draft_auto_gain_enabled;
            new_config.audio.loudness.target_lufs = app.settings.target_lufs();
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

            // Send settings to audio engine via domain
            app.domain.set_phase_sync(app.settings.draft_phase_sync);
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
