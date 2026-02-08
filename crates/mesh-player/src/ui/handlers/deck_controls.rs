//! Deck controls message handler
//!
//! Handles all per-deck control messages: playback, hot cues, loops, stems, and slicer.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::deck_view::{DeckMessage, ActionButtonMode};
use crate::ui::message::Message;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{load_preset, EffectPresetConfig, MultibandPresetConfig, NUM_MACROS};

/// Handle deck control messages
pub fn handle(app: &mut MeshApp, deck_idx: usize, deck_msg: DeckMessage) -> Task<Message> {
    if deck_idx >= 4 {
        return Task::none();
    }

    use DeckMessage::*;
    match deck_msg {
        // ─────────────────────────────────────────────────
        // Playback Control
        // ─────────────────────────────────────────────────
        TogglePlayPause => {
            app.domain.toggle_play(deck_idx);
        }
        CuePressed => {
            app.domain.cue_press(deck_idx);
        }
        CueReleased => {
            app.domain.cue_release(deck_idx);
        }
        SetCue => {
            app.domain.set_cue_point(deck_idx);
        }

        // ─────────────────────────────────────────────────
        // Hot Cues
        // ─────────────────────────────────────────────────
        HotCuePressed(slot) => {
            // If slot is empty, engine will set a new hot cue at current position
            // Update UI optimistically by reading current position from atomics
            let slot_was_empty = app.deck_views[deck_idx].hot_cue_position(slot).is_none();
            if slot_was_empty {
                if let Some(ref atomics) = app.deck_atomics {
                    let position = atomics[deck_idx].position();
                    app.deck_views[deck_idx].set_hot_cue_position(slot, Some(position));
                }
            }
            app.domain.hot_cue_press(deck_idx, slot);
        }
        HotCueReleased(_slot) => {
            app.domain.hot_cue_release(deck_idx);
        }
        SetHotCue(_slot) => {
            // Hot cue is set automatically on press if empty
        }
        ClearHotCue(slot) => {
            // Clear the UI state for this hot cue slot
            app.deck_views[deck_idx].set_hot_cue_position(slot, None);
            app.domain.clear_hot_cue(deck_idx, slot);
        }
        Sync => {
            // TODO: Implement sync command
        }

        // ─────────────────────────────────────────────────
        // Loop Control
        // ─────────────────────────────────────────────────
        ToggleLoop => {
            app.domain.toggle_loop(deck_idx);
        }
        ToggleSlip => {
            app.domain.toggle_slip(deck_idx);
        }
        ToggleKeyMatch => {
            // Toggle key matching for this deck
            let current = app.deck_views[deck_idx].key_match_enabled();
            app.domain.set_key_match_enabled(deck_idx, !current);
        }
        SetLoopLength(_beats) => {
            // Loop length is handled via adjust commands
        }
        LoopHalve => {
            app.domain.adjust_loop_length(deck_idx, -1);
        }
        LoopDouble => {
            app.domain.adjust_loop_length(deck_idx, 1);
        }

        // ─────────────────────────────────────────────────
        // Beat Jump
        // ─────────────────────────────────────────────────
        BeatJumpBack => {
            app.domain.beat_jump_backward(deck_idx);
        }
        BeatJumpForward => {
            app.domain.beat_jump_forward(deck_idx);
        }

        // ─────────────────────────────────────────────────
        // Stem Control
        // ─────────────────────────────────────────────────
        ToggleStemMute(stem_idx) => {
            let shift_held = app.deck_views[deck_idx].shift_held();
            log::info!(
                "[STEM_TOGGLE] Stem button pressed: deck={}, stem={}, shift_held={}",
                deck_idx, stem_idx, shift_held
            );

            if shift_held {
                // Shift+Stem: Linked stem operation
                app.handle_shift_stem(deck_idx, stem_idx);
            } else {
                // Normal: Toggle mute
                if let Some(stem) = Stem::from_index(stem_idx) {
                    app.domain.toggle_stem_mute(deck_idx, stem);
                }
                // Toggle mute state in DeckView for UI
                let was_muted = app.deck_views[deck_idx].is_stem_muted(stem_idx);
                let new_muted = !was_muted;
                app.deck_views[deck_idx].set_stem_muted(stem_idx, new_muted);

                // stem_active = NOT muted (when muted, stem is inactive)
                app.player_canvas_state.set_stem_active(deck_idx, stem_idx, !new_muted);
            }
        }
        ToggleStemSolo(stem_idx) => {
            if let Some(stem) = Stem::from_index(stem_idx) {
                app.domain.toggle_stem_solo(deck_idx, stem);
            }
            // Toggle solo state
            let was_soloed = app.deck_views[deck_idx].is_stem_soloed(stem_idx);
            let new_soloed = !was_soloed;

            if new_soloed {
                // Solo: this stem becomes active, all others become inactive
                for i in 0..4 {
                    app.deck_views[deck_idx].set_stem_soloed(i, i == stem_idx);
                    // When soloing, set active state based on solo selection
                    // (ignore mute state - solo overrides)
                    app.player_canvas_state.set_stem_active(deck_idx, i, i == stem_idx);
                }
            } else {
                // Un-solo: all stems become active (unless muted)
                app.deck_views[deck_idx].set_stem_soloed(stem_idx, false);
                for i in 0..4 {
                    let is_muted = app.deck_views[deck_idx].is_stem_muted(i);
                    app.player_canvas_state.set_stem_active(deck_idx, i, !is_muted);
                }
            }
        }
        SelectStem(stem_idx) => {
            // UI-only state, no command needed
            app.deck_views[deck_idx].set_selected_stem(stem_idx);
        }
        StemPreset(stem_idx, ref preset_msg) => {
            use mesh_widgets::StemPresetMessage;

            // Handle stem preset messages
            match preset_msg {
                StemPresetMessage::SetMacro { index, value } => {
                    log::debug!(
                        "[MACRO] SetMacro deck={} stem={} macro={} value={:.3}",
                        deck_idx, stem_idx, index, value
                    );

                    // Update UI state immediately for responsive feedback
                    app.deck_views[deck_idx].set_stem_macro(stem_idx, *index, *value);

                    // Send macro value to the engine - the engine's MultibandHost
                    // will apply mappings during audio processing via apply_macros()
                    if let Some(stem) = Stem::from_index(stem_idx) {
                        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                            deck: deck_idx,
                            stem,
                            macro_index: *index,
                            value: *value,
                        });
                    }
                }
                StemPresetMessage::SelectPreset(preset_name) => {
                    // Load the selected preset to this stem
                    return handle_preset_selection(app, deck_idx, stem_idx, preset_name.clone());
                }
                StemPresetMessage::TogglePicker => {
                    // Toggle the preset picker dropdown
                    if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                        let was_open = preset.picker_open;
                        preset.picker_open = !was_open;

                        // Refresh presets list when opening
                        if !was_open {
                            preset.available_presets =
                                mesh_widgets::multiband::list_presets(&app.config.collection_path);
                        }
                    }
                }
                StemPresetMessage::ClosePicker => {
                    if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                        preset.picker_open = false;
                    }
                }
                StemPresetMessage::RefreshPresets => {
                    // Refresh the available presets list
                    let presets = mesh_widgets::multiband::list_presets(&app.config.collection_path);
                    if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                        preset.available_presets = presets;
                    }
                }
                StemPresetMessage::SetAvailablePresets(presets) => {
                    if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                        preset.available_presets = presets.clone();
                    }
                }
                StemPresetMessage::SetMacroNames(names) => {
                    if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                        preset.macro_names = names.clone();
                    }
                }
            }
        }

        // ─────────────────────────────────────────────────
        // Slicer Mode Controls
        // ─────────────────────────────────────────────────
        SetActionMode(mode) => {
            // Update UI state
            app.deck_views[deck_idx].set_action_mode(mode);

            // Enable/disable slicer based on mode for stems with patterns
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            match mode {
                ActionButtonMode::Slicer => {
                    // Entering slicer mode - enable slicer for stems with patterns
                    for (idx, &stem) in stems.iter().enumerate() {
                        if preset.stems[idx].is_some() {
                            app.domain.set_slicer_enabled(deck_idx, stem, true);
                        }
                    }
                }
                ActionButtonMode::HotCue => {
                    // Leaving slicer mode - disable processing but keep queue arrangement
                    for &stem in &stems {
                        app.domain.set_slicer_enabled(deck_idx, stem, false);
                    }
                }
            }
        }
        SlicerPresetSelect(preset_idx) => {
            // Select a new slicer preset via engine's button action
            // The engine handles enabling slicers and loading patterns
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

            // Update selected preset in slice editor state
            app.slice_editor.selected_preset = preset_idx;

            // Send button action to engine for each stem (shift_held=false = load preset)
            let preset = &app.slice_editor.presets[preset_idx];
            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_button_action(deck_idx, stem, preset_idx, false);
                }
            }
        }
        SlicerTrigger(button_idx) => {
            // Shift+click triggers slice for live queue adjustment
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let shift_held = app.deck_views[deck_idx].shift_held();
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_button_action(deck_idx, stem, button_idx, shift_held);
                }
            }
        }
        ResetSlicerPattern => {
            // Reset slicer queue to default [0..15]
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];
            let preset = &app.slice_editor.presets[app.slice_editor.selected_preset];

            for (idx, &stem) in stems.iter().enumerate() {
                if preset.stems[idx].is_some() {
                    app.domain.slicer_reset_queue(deck_idx, stem);
                }
            }
        }

        // ─────────────────────────────────────────────────
        // Shift State (UI only)
        // ─────────────────────────────────────────────────
        ShiftPressed => {
            app.deck_views[deck_idx].set_shift_held(true);
        }
        ShiftReleased => {
            app.deck_views[deck_idx].set_shift_held(false);
        }

        // ─────────────────────────────────────────────────
        // Multiband Editor
        // ─────────────────────────────────────────────────
        OpenMultibandEditor(stem_idx) => {
            // Open multiband editor modal for this deck/stem
            let stem_name = match stem_idx {
                0 => "Vocals",
                1 => "Drums",
                2 => "Bass",
                _ => "Other",
            };
            return Task::done(Message::Multiband(
                mesh_widgets::MultibandEditorMessage::Open {
                    deck: deck_idx,
                    stem: stem_idx,
                    stem_name: stem_name.to_string(),
                }
            ));
        }
    }
    Task::none()
}

/// Handle preset selection for a stem
///
/// Loads the preset configuration and applies it to the multiband container.
fn handle_preset_selection(
    app: &mut MeshApp,
    deck_idx: usize,
    stem_idx: usize,
    preset_name: Option<String>,
) -> Task<Message> {
    if let Some(name) = preset_name {
        // Load the preset from disk
        match load_preset(&app.config.collection_path, &name) {
            Ok(config) => {
                // Update UI state
                if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
                    preset.loaded_preset = Some(name.clone());
                    preset.picker_open = false;

                    // Update macro names from the preset
                    let mut names: [String; NUM_MACROS] = Default::default();
                    for (i, macro_config) in config.macros.iter().enumerate().take(NUM_MACROS) {
                        names[i] = macro_config.name.clone();
                    }
                    preset.macro_names = names;

                    // Reset macro values to center
                    preset.macro_values = [0.5; NUM_MACROS];
                }

                // Apply the preset to the multiband container
                if let Some(stem) = Stem::from_index(stem_idx) {
                    apply_preset_to_multiband(app, deck_idx, stem, &config);
                }

                // If multiband editor is open for this deck/stem, also populate it
                // so macro modulation will work from deck view sliders
                if app.multiband_editor.is_open
                    && app.multiband_editor.deck == deck_idx
                    && app.multiband_editor.stem == stem_idx
                {
                    config.apply_to_editor_state(&mut app.multiband_editor);
                    app.multiband_editor.rebuild_macro_mappings_index();
                    mesh_widgets::multiband::ensure_effect_knobs_exist(&mut app.multiband_editor);
                }

                app.status = format!("Loaded preset '{}' on deck {} {}", name, deck_idx + 1,
                    Stem::from_index(stem_idx).map(|s| s.name()).unwrap_or("?"));
            }
            Err(e) => {
                log::error!("Failed to load preset '{}': {}", name, e);
                app.status = format!("Failed to load preset: {}", e);
            }
        }
    } else {
        // Clear the preset (passthrough mode)
        if let Some(preset) = app.deck_views[deck_idx].stem_preset_mut(stem_idx) {
            preset.clear_preset();
        }

        // Clear effects from the multiband container
        if let Some(stem) = Stem::from_index(stem_idx) {
            clear_multiband_effects(app, deck_idx, stem);
        }

        app.status = format!("Cleared preset on deck {} {}", deck_idx + 1,
            Stem::from_index(stem_idx).map(|s| s.name()).unwrap_or("?"));
    }

    Task::none()
}

/// Apply a preset configuration to the multiband container
pub(super) fn apply_preset_to_multiband(
    app: &mut MeshApp,
    deck_idx: usize,
    stem: Stem,
    config: &MultibandPresetConfig,
) {
    // Clear existing effects by removing them one by one
    // This is a workaround since we don't have a bulk clear command
    clear_multiband_effects(app, deck_idx, stem);

    // Apply crossover frequencies
    for (i, &freq) in config.crossover_freqs.iter().enumerate() {
        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandCrossover {
            deck: deck_idx,
            stem,
            crossover_index: i,
            freq,
        });
    }

    // Add pre-fx effects
    for (effect_idx, effect) in config.pre_fx.iter().enumerate() {
        let result = match effect.source.as_str() {
            "pd" => app.domain.add_pd_effect_pre_fx(deck_idx, stem, &effect.id),
            "clap" => app.domain.add_clap_effect_pre_fx(deck_idx, stem, &effect.id),
            _ => continue,
        };

        if let Ok(_info) = result {
            // Apply ALL parameter values (not just knob-mapped ones)
            // This preserves settings made via the plugin GUI (e.g., reverb mode)
            let params = get_effect_params(effect);
            for (param_idx, value) in params {
                app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxParam {
                    deck: deck_idx,
                    stem,
                    effect_index: effect_idx,
                    param_index: param_idx,
                    value,
                });
            }

            // Apply bypass state
            if effect.bypassed {
                app.domain.set_pre_fx_bypass(deck_idx, stem, effect_idx, true);
            }
        }
    }

    // Add per-band effects
    for (band_idx, band) in config.bands.iter().enumerate() {
        // Set band gain
        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandGain {
            deck: deck_idx,
            stem,
            band_index: band_idx,
            gain: band.gain,
        });

        for (effect_idx, effect) in band.effects.iter().enumerate() {
            let result = match effect.source.as_str() {
                "pd" => app.domain.add_pd_effect(deck_idx, stem, &effect.id, band_idx),
                "clap" => app.domain.add_clap_effect(deck_idx, stem, &effect.id, band_idx),
                _ => continue,
            };

            if let Ok(_info) = result {
                // Apply ALL parameter values (not just knob-mapped ones)
                let params = get_effect_params(effect);
                for (param_idx, value) in params {
                    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandEffectParam {
                        deck: deck_idx,
                        stem,
                        band_index: band_idx,
                        effect_index: effect_idx,
                        param_index: param_idx,
                        value,
                    });
                }

                // Apply bypass state
                if effect.bypassed {
                    app.domain.set_band_effect_bypass(deck_idx, stem, band_idx, effect_idx, effect.bypassed);
                }
            }
        }
    }

    // Add post-fx effects
    for (effect_idx, effect) in config.post_fx.iter().enumerate() {
        let result = match effect.source.as_str() {
            "pd" => app.domain.add_pd_effect_post_fx(deck_idx, stem, &effect.id),
            "clap" => app.domain.add_clap_effect_post_fx(deck_idx, stem, &effect.id),
            _ => continue,
        };

        if let Ok(_info) = result {
            // Apply ALL parameter values (not just knob-mapped ones)
            let params = get_effect_params(effect);
            for (param_idx, value) in params {
                app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxParam {
                    deck: deck_idx,
                    stem,
                    effect_index: effect_idx,
                    param_index: param_idx,
                    value,
                });
            }

            // Apply bypass state
            if effect.bypassed {
                app.domain.set_post_fx_bypass(deck_idx, stem, effect_idx, true);
            }
        }
    }

    // Clear existing macro mappings
    for macro_idx in 0..NUM_MACROS {
        app.domain.send_command(mesh_core::engine::EngineCommand::ClearMultibandMacroMappings {
            deck: deck_idx,
            stem,
            macro_index: macro_idx,
        });
    }

    use mesh_core::effect::multiband::EffectLocation;

    // Apply macro mappings from the preset
    // Pre-FX effects mappings
    for (effect_idx, effect) in config.pre_fx.iter().enumerate() {
        apply_effect_macro_mappings(app, deck_idx, stem, EffectLocation::PreFx, effect_idx, effect);
    }

    // Band effects mappings
    for (band_idx, band) in config.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            apply_effect_macro_mappings(app, deck_idx, stem, EffectLocation::Band(band_idx), effect_idx, effect);
        }
    }

    // Post-FX effects mappings
    for (effect_idx, effect) in config.post_fx.iter().enumerate() {
        apply_effect_macro_mappings(app, deck_idx, stem, EffectLocation::PostFx, effect_idx, effect);
    }

    log::info!("Applied preset '{}' to deck {} stem {:?}", config.name, deck_idx, stem);
}

/// Apply macro mappings from an effect's knob assignments to the engine
fn apply_effect_macro_mappings(
    app: &mut MeshApp,
    deck_idx: usize,
    stem: Stem,
    location: mesh_core::effect::multiband::EffectLocation,
    effect_index: usize,
    effect: &EffectPresetConfig,
) {
    for assignment in &effect.knob_assignments {
        // Only process if knob has a param assigned and a macro mapping
        if let (Some(param_index), Some(ref mapping)) = (assignment.param_index, &assignment.macro_mapping) {
            if let Some(macro_index) = mapping.macro_index {
                // Convert bipolar offset_range to min/max values
                // UI formula: actual = base + (macro * 2 - 1) * offset_range
                // At macro=0: min = base - offset_range
                // At macro=1: max = base + offset_range
                let base_value = assignment.value;
                let min_value = (base_value - mapping.offset_range).max(0.0);
                let max_value = (base_value + mapping.offset_range).min(1.0);

                log::debug!(
                    "[MACRO_MAPPING] Adding mapping: macro={} -> {:?} effect={} param={} range=[{:.2}, {:.2}]",
                    macro_index, location, effect_index, param_index, min_value, max_value
                );

                app.domain.send_command(mesh_core::engine::EngineCommand::AddMultibandMacroMapping {
                    deck: deck_idx,
                    stem,
                    macro_index,
                    location,
                    effect_index,
                    param_index,
                    min_value,
                    max_value,
                });
            }
        }
    }
}

/// Extract all parameter values from an effect preset config
///
/// Returns (param_index, value) pairs for all parameters.
/// Prefers `all_param_values` (which contains the complete plugin state)
/// over `knob_assignments` (which only has 8 mapped params).
fn get_effect_params(effect: &EffectPresetConfig) -> Vec<(usize, f32)> {
    if !effect.all_param_values.is_empty() {
        // Use all_param_values - contains the complete plugin state
        // including settings made via the plugin GUI
        effect.all_param_values
            .iter()
            .enumerate()
            .map(|(idx, &value)| (idx, value))
            .collect()
    } else {
        // Fallback to knob_assignments for older presets
        effect.knob_assignments
            .iter()
            .filter_map(|assignment| {
                assignment.param_index.map(|idx| (idx, assignment.value))
            })
            .collect()
    }
}

/// Clear all effects from a multiband container
///
/// Note: This is a placeholder - we need to query the current effect count
/// and remove them one by one. For now, we'll just log a warning.
fn clear_multiband_effects(_app: &mut MeshApp, deck_idx: usize, stem: Stem) {
    // TODO: Implement proper clearing by querying effect counts and removing
    // For now, we just log a warning - effects will be added on top
    log::warn!("clear_multiband_effects not fully implemented - effects may stack on preset changes. deck={}, stem={:?}", deck_idx, stem);
}
