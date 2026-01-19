//! MIDI learn message handler
//!
//! Handles the MIDI learn workflow: capturing mappings, saving config, reloading.

use iced::Task;

use mesh_midi::MidiController;
use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::midi_learn::MidiLearnMessage;

/// Handle MIDI learn messages
pub fn handle(app: &mut MeshApp, learn_msg: MidiLearnMessage) -> Task<Message> {
    use MidiLearnMessage::*;

    match learn_msg {
        Start => {
            app.midi_learn.start();
            // Close settings modal if open
            app.settings.is_open = false;
            app.status = "MIDI Learn mode started".to_string();
        }
        Cancel => {
            app.midi_learn.cancel();
            app.status = "MIDI Learn cancelled".to_string();
        }
        Next => {
            app.midi_learn.advance();
        }
        Back => {
            app.midi_learn.go_back();
        }
        Skip => {
            if app.midi_learn.awaiting_encoder_press {
                app.midi_learn.skip_encoder_press();
            } else {
                app.midi_learn.advance();
            }
        }
        Save => {
            app.status = format!(
                "Saving {} mappings for {}...",
                app.midi_learn.pending_mappings.len(),
                app.midi_learn.controller_name
            );

            // Generate the config from learned mappings
            let config = app.midi_learn.generate_config();
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
                    app.midi_learn.cancel(); // Reset state
                    app.status = "MIDI config saved! Reloading...".to_string();

                    // Reload MIDI controller with new config
                    // Drop old controller first to release the port
                    app.midi_controller = None;

                    // Create new controller with fresh config
                    match MidiController::new_with_options(None, true) {
                        Ok(controller) => {
                            if controller.is_connected() {
                                log::info!("MIDI: Reloaded controller with new config");
                                app.status = "MIDI config saved and loaded!".to_string();
                            } else {
                                app.status = "MIDI config saved (no device connected)".to_string();
                            }
                            app.midi_controller = Some(controller);
                        }
                        Err(e) => {
                            log::warn!("MIDI: Failed to reload controller: {}", e);
                            app.status = format!("Config saved, but reload failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    app.midi_learn.status = format!("Save failed: {}", e);
                    app.status = format!("MIDI config save failed: {}", e);
                }
            }
        }
        SetControllerName(name) => {
            app.midi_learn.controller_name = name;
        }
        SetDeckCount(count) => {
            app.midi_learn.deck_count = count;
        }
        SetHasLayerToggle(has) => {
            app.midi_learn.has_layer_toggle = has;
        }
        SetPadModeSource(source) => {
            app.midi_learn.pad_mode_source = source;
        }
        ShiftDetected(event) => {
            app.midi_learn.shift_mapping = event;
            app.midi_learn.advance();
        }
        MidiCaptured(event) => {
            app.midi_learn.record_mapping(event);
        }
    }
    Task::none()
}
