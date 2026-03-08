//! MIDI learn message handler
//!
//! Handles the tree-based MIDI learn workflow: setup, tree navigation,
//! capture routing, saving config, reloading controller.

use iced::Task;

use mesh_midi::ControllerManager;
use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::midi_learn::{LearnMode, MidiLearnMessage, scroll_tree_to_cursor};

/// Clear MIDI learn highlight from all views (called when learn mode exits)
fn clear_highlights(app: &mut MeshApp) {
    for i in 0..4 {
        app.deck_views[i].set_highlight(None);
    }
    app.mixer_view.set_highlight(None);
}

/// Handle MIDI learn messages
pub fn handle(app: &mut MeshApp, learn_msg: MidiLearnMessage) -> Task<Message> {
    use MidiLearnMessage::*;

    match learn_msg {
        Start => {
            app.midi_learn.start();
            // Close settings modal if open
            app.settings.is_open = false;

            // If existing config exists, load it into the tree
            let config_path = mesh_midi::default_midi_config_path();
            if config_path.exists() {
                let config = mesh_midi::load_midi_config(&config_path);
                if !config.devices.is_empty() {
                    log::info!("MIDI Learn: Loading existing config with {} devices", config.devices.len());
                    app.midi_learn.load_existing_config(&config);
                    app.status = "MIDI Learn: existing config loaded. Edit and save.".to_string();
                } else {
                    app.status = "MIDI Learn mode started".to_string();
                }
            } else {
                app.status = "MIDI Learn mode started".to_string();
            }
        }
        Cancel => {
            app.midi_learn.cancel();
            clear_highlights(app);
            app.status = "MIDI Learn cancelled".to_string();
        }

        // --- Setup phase ---
        SetTopology(choice) => {
            app.midi_learn.topology_choice = choice;
        }
        SetOverlayMode(enabled) => {
            app.midi_learn.overlay_mode = enabled;
        }
        SetPadMode(source) => {
            app.midi_learn.pad_mode_source = source;
        }
        ConfirmSetup => {
            app.midi_learn.confirm_setup();
            if app.midi_learn.mode == LearnMode::TreeNavigation {
                app.status = "Tree built. Map your controls!".to_string();
            }
        }

        // --- Tree navigation (keyboard/touch) ---
        ScrollTree(delta) => {
            // During verification, ScrollTree(0) means "go back to tree"
            if app.midi_learn.mode == LearnMode::Verification && delta == 0 {
                app.midi_learn.mode = LearnMode::TreeNavigation;
                app.midi_learn.update_highlight();
                return Task::none();
            }

            if app.midi_learn.mode == LearnMode::Setup {
                app.midi_learn.setup_scroll(delta);
            } else if let Some(ref mut tree) = app.midi_learn.tree {
                tree.scroll(delta);
            }
            app.midi_learn.update_highlight();

            // Auto-scroll tree view to keep cursor visible
            if let Some((cursor, total)) = app.midi_learn.tree_scroll_info() {
                return scroll_tree_to_cursor(cursor, total);
            }
        }
        SelectRow(idx) => {
            if app.midi_learn.mode == LearnMode::Setup {
                // Setup mode: set cursor and select the item
                app.midi_learn.setup_cursor = idx;
                let should_confirm = app.midi_learn.setup_select();
                if should_confirm {
                    app.midi_learn.confirm_setup();
                    if app.midi_learn.mode == LearnMode::TreeNavigation {
                        app.status = "Tree built. Map your controls!".to_string();
                    }
                }
            } else if let Some(ref mut tree) = app.midi_learn.tree {
                tree.cursor = idx;
                let is_done = tree.select();
                if is_done {
                    app.midi_learn.mode = LearnMode::Verification;
                }
            }
            app.midi_learn.update_highlight();
            // Auto-scroll to selected row
            if let Some((cursor, total)) = app.midi_learn.tree_scroll_info() {
                return scroll_tree_to_cursor(cursor, total);
            }
        }
        ToggleSection => {
            if let Some(ref mut tree) = app.midi_learn.tree {
                let is_done = tree.select();
                if is_done {
                    app.midi_learn.mode = LearnMode::Verification;
                }
            }
            app.midi_learn.update_highlight();

            // Auto-scroll after section toggle
            if let Some((cursor, total)) = app.midi_learn.tree_scroll_info() {
                return scroll_tree_to_cursor(cursor, total);
            }
        }
        ClearMapping => {
            if let Some(ref mut tree) = app.midi_learn.tree {
                tree.clear_current_mapping();
            }
            app.midi_learn.update_highlight();
            app.midi_learn.rebuild_active_mappings();
        }

        // --- Reset confirmation ---
        ResetMappings => {
            app.midi_learn.mode = LearnMode::ResetConfirm;
            app.midi_learn.reset_confirm_cursor = 0;
        }
        ConfirmReset => {
            app.midi_learn.start();
            clear_highlights(app);
            app.status = "MIDI Learn restarted. Turn your main BROWSE encoder.".to_string();
        }
        CancelReset => {
            app.midi_learn.mode = LearnMode::TreeNavigation;
            app.midi_learn.update_highlight();
        }

        // --- Capture (from tick.rs) ---
        MidiCaptured(event) => {
            app.midi_learn.start_capture(event);
        }

        // --- Deferred scroll (after fold/unfold layout recalculation) ---
        RefreshScroll => {
            if let Some((cursor, total)) = app.midi_learn.tree_scroll_info() {
                return scroll_tree_to_cursor(cursor, total);
            }
        }

        // --- Save ---
        Save => {
            app.status = "Saving MIDI config...".to_string();

            let config = app.midi_learn.generate_config();
            let config_path = mesh_midi::default_midi_config_path();

            let mapping_count = config.devices.first()
                .map(|d| d.mappings.len())
                .unwrap_or(0);
            log::info!("MIDI Learn: Saving config with {} mappings", mapping_count);

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
                    app.midi_learn.cancel();
                    clear_highlights(app);
                    app.status = "MIDI config saved! Reloading...".to_string();

                    // Reload MIDI controller with new config
                    app.controller = None;
                    match ControllerManager::new_with_options(None, true) {
                        Ok(controller) => {
                            if controller.is_connected() {
                                log::info!("MIDI: Reloaded controller with new config");
                                app.status = "MIDI config saved and loaded!".to_string();
                            } else {
                                app.status =
                                    "MIDI config saved (no device connected)".to_string();
                            }
                            app.controller = Some(controller);
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
    }
    Task::none()
}
