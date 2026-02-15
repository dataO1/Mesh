//! Settings message handler
//!
//! Handles the settings modal: opening, closing, updating draft values, and saving.

use std::sync::Arc;
use iced::Task;

use crate::config;
use crate::ui::app::MeshApp;
use crate::ui::handlers::browser::trigger_suggestion_query;
use crate::config::WaveformLayout;
use crate::ui::message::{Message, SettingsMessage};
use crate::ui::settings::SettingsState;

/// Handle settings messages
pub fn handle(app: &mut MeshApp, msg: SettingsMessage) -> Task<Message> {
    use SettingsMessage::*;

    match msg {
        Open => {
            app.settings.is_open = true;
            app.settings = SettingsState::from_config(&app.config);
            app.settings.is_open = true;
            Task::none()
        }
        Close => {
            app.settings.is_open = false;
            app.settings.status.clear();
            Task::none()
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
        UpdateStemColorPalette(palette) => {
            app.settings.draft_stem_color_palette = palette;
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
            new_config.display.stem_color_palette = app.settings.draft_stem_color_palette;
            new_config.display.show_local_collection = app.settings.draft_show_local_collection;
            new_config.display.key_scoring_model = app.settings.draft_key_scoring_model;
            new_config.display.waveform_layout = app.settings.draft_waveform_layout;
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

            // Apply stem color palette to waveform display immediately
            app.player_canvas_state.set_stem_colors(
                app.settings.draft_stem_color_palette.colors()
            );

            // Apply waveform layout immediately
            app.player_canvas_state.set_vertical_layout(
                app.settings.draft_waveform_layout.is_vertical()
            );
            app.player_canvas_state.set_vertical_inverted(
                app.settings.draft_waveform_layout.is_inverted()
            );

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
