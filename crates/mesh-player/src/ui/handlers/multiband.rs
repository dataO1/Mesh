//! Multiband editor message handler
//!
//! Handles multiband container editing: crossover control, band management,
//! effect chains per band, and macro knob routing.

use iced::Task;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{EffectSourceType, EffectUiState};
use mesh_widgets::MultibandEditorMessage;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;

/// Handle multiband editor messages
pub fn handle(app: &mut MeshApp, msg: MultibandEditorMessage) -> Task<Message> {
    use MultibandEditorMessage::*;

    match msg {
        // ─────────────────────────────────────────────────────────────────────
        // Modal control
        // ─────────────────────────────────────────────────────────────────────
        Open { deck, stem, stem_name } => {
            app.multiband_editor.open(deck, stem, &stem_name);
            // Sync state from backend MultibandHost
            sync_from_backend(app);
            Task::none()
        }

        Close => {
            app.multiband_editor.close();
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Crossover control
        // ─────────────────────────────────────────────────────────────────────
        StartDragCrossover(index) => {
            app.multiband_editor.dragging_crossover = Some(index);
            Task::none()
        }

        DragCrossover(freq) => {
            if let Some(index) = app.multiband_editor.dragging_crossover {
                let deck = app.multiband_editor.deck;
                let stem = Stem::ALL[app.multiband_editor.stem];

                // Update UI state
                app.multiband_editor.set_crossover_freq(index, freq);

                // Send to backend crossover
                app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandCrossover {
                    deck,
                    stem,
                    crossover_index: index,
                    freq,
                });
            }
            Task::none()
        }

        EndDragCrossover => {
            app.multiband_editor.dragging_crossover = None;
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Band management
        // ─────────────────────────────────────────────────────────────────────
        AddBand => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Update UI state
            app.multiband_editor.add_band();

            // Send to backend (will add band and enable crossover splitting)
            app.domain.send_command(mesh_core::engine::EngineCommand::AddMultibandBand {
                deck,
                stem,
            });
            Task::none()
        }

        RemoveBand(band_idx) => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Update UI state
            app.multiband_editor.remove_band(band_idx);

            // Send to backend
            app.domain.send_command(mesh_core::engine::EngineCommand::RemoveMultibandBand {
                deck,
                stem,
                band_index: band_idx,
            });
            Task::none()
        }

        SetBandMute { band, muted } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            app.multiband_editor.set_band_mute(band, muted);
            app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandMute {
                deck,
                stem,
                band_index: band,
                muted,
            });
            Task::none()
        }

        SetBandSolo { band, soloed } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            app.multiband_editor.set_band_solo(band, soloed);
            app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandSolo {
                deck,
                stem,
                band_index: band,
                soloed,
            });
            Task::none()
        }

        SetBandGain { band, gain } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                band_state.gain = gain;
            }
            app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandGain {
                deck,
                stem,
                band_index: band,
                gain,
            });
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Effect management
        // ─────────────────────────────────────────────────────────────────────
        OpenEffectPicker(band_idx) => {
            // Store which band we're adding to, then open the picker
            let deck = app.multiband_editor.deck;
            let stem_idx = app.multiband_editor.stem;
            // Open effect picker for this stem and band
            app.effect_picker.open_for_band(deck, stem_idx, band_idx);
            log::info!("Opening effect picker for band {} (deck {} stem {})", band_idx, deck, stem_idx);
            Task::none()
        }

        EffectSelected { band, effect_id, source } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Add effect based on source type to the specified band
            let (result, source_type) = match source.as_str() {
                "pd" => (app.domain.add_pd_effect(deck, stem, &effect_id, band), EffectSourceType::Pd),
                "clap" => (app.domain.add_clap_effect(deck, stem, &effect_id, band), EffectSourceType::Clap),
                _ => (Err(format!("Unknown effect source: {}", source)), EffectSourceType::Native),
            };

            if let Err(e) = result {
                log::error!("Failed to add effect to band {}: {}", band, e);
                app.status = format!("Failed to add effect: {}", e);
            } else {
                log::info!("Added {} effect '{}' to band {}", source, effect_id, band);

                // Add effect to UI state
                if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                    // Extract effect name from ID (last path component or the ID itself)
                    let effect_name = effect_id
                        .rsplit('/')
                        .next()
                        .unwrap_or(&effect_id)
                        .trim_end_matches(".pd")
                        .to_string();

                    band_state.effects.push(EffectUiState {
                        id: effect_id.clone(),
                        name: effect_name,
                        category: source.to_uppercase(),
                        source: source_type,
                        bypassed: false,
                        param_names: vec!["P1".into(), "P2".into(), "P3".into(), "P4".into(),
                                         "P5".into(), "P6".into(), "P7".into(), "P8".into()],
                        param_values: vec![0.5; 8],
                    });
                }

                // Sync macro values from deck view
                sync_from_backend(app);
            }
            Task::none()
        }

        RemoveEffect { band, effect } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            app.domain.remove_effect_from_band(deck, stem, band, effect);

            // Update UI state
            if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                if effect < band_state.effects.len() {
                    band_state.effects.remove(effect);
                }
            }
            Task::none()
        }

        ToggleEffectBypass { band, effect } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Toggle local state
            let new_bypass = if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                if let Some(effect_state) = band_state.effects.get_mut(effect) {
                    effect_state.bypassed = !effect_state.bypassed;
                    effect_state.bypassed
                } else {
                    return Task::none();
                }
            } else {
                return Task::none();
            };

            app.domain.set_band_effect_bypass(deck, stem, band, effect, new_bypass);
            Task::none()
        }

        SelectEffect { band, effect } => {
            app.multiband_editor.selected_effect = Some((band, effect));
            Task::none()
        }

        SetEffectParam { band, effect, param, value } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Update local state
            if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                if let Some(effect_state) = band_state.effects.get_mut(effect) {
                    if param < effect_state.param_values.len() {
                        effect_state.param_values[param] = value;
                    }
                }
            }

            app.domain.set_band_effect_param(deck, stem, band, effect, param, value);
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Macro control
        // ─────────────────────────────────────────────────────────────────────
        SetMacro { index, value } => {
            let deck = app.multiband_editor.deck;
            let stem_idx = app.multiband_editor.stem;
            let stem = Stem::ALL[stem_idx];

            // Update multiband editor UI state
            app.multiband_editor.set_macro_value(index, value);

            // Sync to deck view (bidirectional sync for consistency)
            if deck < 4 && stem_idx < 4 && index < 8 {
                app.deck_views[deck].set_stem_knob(stem_idx, index, value);
            }

            // Send to engine
            app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                deck,
                stem,
                macro_index: index,
                value,
            });
            Task::none()
        }

        RenameMacro { index, name } => {
            app.multiband_editor.set_macro_name(index, name);
            Task::none()
        }

        OpenMacroMapper(_index) => {
            // TODO: Open macro mapping dialog
            Task::none()
        }

        AddMacroMapping { macro_index: _, band: _, effect: _, param: _ } => {
            // TODO: Add mapping to MultibandHost
            Task::none()
        }

        ClearMacroMappings(_index) => {
            // TODO: Clear all mappings for this macro
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Preset management
        // ─────────────────────────────────────────────────────────────────────
        OpenPresetBrowser => {
            app.multiband_editor.preset_browser_open = true;
            Task::none()
        }

        ClosePresetBrowser => {
            app.multiband_editor.preset_browser_open = false;
            Task::none()
        }

        LoadPreset(_name) => {
            // TODO: Load preset from disk
            Task::none()
        }

        SavePreset(_name) => {
            // TODO: Save current state as preset
            Task::none()
        }

        DeletePreset(_name) => {
            // TODO: Delete preset
            Task::none()
        }

        RefreshPresets => {
            // TODO: Refresh preset list
            Task::none()
        }
    }
}

/// Sync multiband editor UI state from deck view (single source of truth for now)
///
/// This syncs macro values from deck_views to the multiband editor.
/// The deck_views are synced from the engine when tracks are loaded.
///
/// TODO: For true single source of truth, add atomic storage for macros
/// and read directly from atomics like we do for play_state.
fn sync_from_backend(app: &mut MeshApp) {
    let deck = app.multiband_editor.deck;
    let stem_idx = app.multiband_editor.stem;

    // Sync macro values from deck view (which holds the current state)
    if deck < 4 && stem_idx < 4 {
        for macro_idx in 0..8 {
            let value = app.deck_views[deck].stem_knob_value(stem_idx, macro_idx);
            app.multiband_editor.set_macro_value(macro_idx, value);
        }
    }
}
