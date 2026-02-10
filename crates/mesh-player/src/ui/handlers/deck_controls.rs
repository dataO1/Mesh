//! Deck controls message handler
//!
//! Handles all per-deck control messages: playback, hot cues, loops, stems, and slicer.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::deck_view::{DeckMessage, ActionButtonMode};
use crate::ui::message::Message;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{
    list_deck_presets, list_stem_presets,
    EffectPresetConfig, StemPresetConfig, NUM_MACROS,
};

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
        DeckPreset(ref preset_msg) => {
            use mesh_widgets::DeckPresetMessage;

            // Handle deck preset messages (shared macros + preset selector)
            match preset_msg {
                DeckPresetMessage::SetMacro { index, value } => {
                    // Update UI state immediately for responsive feedback
                    app.deck_views[deck_idx].set_deck_macro(*index, *value);

                    // Read stem preset names and UI-side mappings (releases borrow before mut access)
                    let stem_has_preset: [bool; 4] = {
                        let dp = app.deck_views[deck_idx].deck_preset();
                        std::array::from_fn(|i| dp.stem_preset_names[i].is_some())
                    };
                    let mappings: Vec<_> = app.deck_views[deck_idx]
                        .deck_preset()
                        .mappings_for_macro(*index)
                        .to_vec();

                    log::info!(
                        "[MACRO] SetMacro deck={} macro={} value={:.3} ui_mappings={} stems_with_preset={}",
                        deck_idx, index, value, mappings.len(),
                        stem_has_preset.iter().filter(|&&b| b).count()
                    );

                    // Send macro value to engine for ALL stems with loaded presets
                    // This ensures engine-side apply_macros() works even if UI-side mappings are empty
                    for stem_idx in 0..4 {
                        if stem_has_preset[stem_idx] {
                            if let Some(stem) = Stem::from_index(stem_idx) {
                                app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                                    deck: deck_idx,
                                    stem,
                                    macro_index: *index,
                                    value: *value,
                                });
                            }
                        }
                    }

                    // Apply UI-side direct modulation (handles dry/wet and other non-engine-mapped targets)
                    for mapping in &mappings {
                        if let Some(stem) = Stem::from_index(mapping.stem_index) {
                            apply_macro_modulation_direct_single(
                                &mut app.domain,
                                deck_idx,
                                stem,
                                *value,
                                mapping,
                            );
                        }
                    }
                }
                DeckPresetMessage::SelectDeckPreset(preset_name) => {
                    // Load the selected deck preset
                    return handle_deck_preset_selection(app, deck_idx, preset_name.clone());
                }
                DeckPresetMessage::TogglePicker => {
                    // Toggle the preset picker dropdown
                    let deck_preset = app.deck_views[deck_idx].deck_preset_mut();
                    let was_open = deck_preset.picker_open;
                    deck_preset.picker_open = !was_open;

                    // Refresh presets list when opening
                    if !was_open {
                        deck_preset.available_deck_presets =
                            list_deck_presets(&app.config.collection_path);
                        deck_preset.available_stem_presets =
                            list_stem_presets(&app.config.collection_path);
                    }
                }
                DeckPresetMessage::ClosePicker => {
                    app.deck_views[deck_idx].deck_preset_mut().picker_open = false;
                }
                DeckPresetMessage::RefreshPresets => {
                    let deck_presets = list_deck_presets(&app.config.collection_path);
                    let stem_presets = list_stem_presets(&app.config.collection_path);
                    let dp = app.deck_views[deck_idx].deck_preset_mut();
                    dp.available_deck_presets = deck_presets;
                    dp.available_stem_presets = stem_presets;
                }
                DeckPresetMessage::SetAvailableDeckPresets(presets) => {
                    app.deck_views[deck_idx].deck_preset_mut().available_deck_presets = presets.clone();
                }
                DeckPresetMessage::SetAvailableStemPresets(presets) => {
                    app.deck_views[deck_idx].deck_preset_mut().available_stem_presets = presets.clone();
                }
                DeckPresetMessage::SetMacroNames(names) => {
                    app.deck_views[deck_idx].deck_preset_mut().macro_names = names.clone();
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

/// Handle deck preset selection
///
/// Loads a deck preset (wrapper referencing stem presets) and applies all
/// stem presets to their respective multiband containers.
fn handle_deck_preset_selection(
    app: &mut MeshApp,
    deck_idx: usize,
    preset_name: Option<String>,
) -> Task<Message> {
    use mesh_widgets::multiband::DeckPresetConfig;

    if let Some(name) = preset_name {
        // Load the fully resolved deck preset (wrapper + all referenced stem presets)
        match DeckPresetConfig::load_resolved(&app.config.collection_path, &name) {
            Ok(resolved) => {
                let dp = app.deck_views[deck_idx].deck_preset_mut();
                dp.loaded_deck_preset = Some(name.clone());
                dp.picker_open = false;

                // Update macro names from the deck preset
                let mut macro_names: [String; NUM_MACROS] = Default::default();
                for (i, macro_config) in resolved.macros.iter().enumerate().take(NUM_MACROS) {
                    macro_names[i] = macro_config.name.clone();
                }
                dp.macro_names = macro_names;

                // Always start macros at neutral (center) position
                dp.macro_values = [0.5; NUM_MACROS];

                // Store stem preset references
                dp.stem_preset_names = resolved.stem_names.clone();

                // Apply each stem preset to its multiband container
                // and collect macro mappings across all stems
                let mut all_mappings: [Vec<mesh_widgets::MacroParamMapping>; NUM_MACROS] = Default::default();

                let stem_names = ["Vocals", "Drums", "Bass", "Other"];
                for stem_idx in 0..4 {
                    if let Some(ref stem_config) = resolved.stems[stem_idx] {
                        if let Some(stem) = Stem::from_index(stem_idx) {
                            log::info!(
                                "[PRESET_LOAD] Loading stem {} ({}) preset '{}' for deck {} (background)",
                                stem_idx, stem_names[stem_idx], stem_config.name, deck_idx
                            );
                            // Build MultibandHost on background thread instead of blocking UI
                            let spec = stem_config.to_build_spec();
                            app.domain.load_preset(deck_idx, stem, spec);

                            // Extract macro mappings for this stem
                            extract_deck_macro_mappings(
                                stem_idx,
                                stem_config,
                                &mut all_mappings,
                            );
                        }
                    } else {
                        log::info!(
                            "[PRESET_LOAD] No stem preset for {} ({}) on deck {}",
                            stem_idx, stem_names[stem_idx], deck_idx
                        );
                    }
                }

                // Log mapping summary
                for (i, mappings) in all_mappings.iter().enumerate() {
                    if !mappings.is_empty() {
                        log::info!(
                            "[PRESET_LOAD] Macro {} has {} UI-side mappings",
                            i, mappings.len()
                        );
                    }
                }

                // Store all macro mappings on the deck preset
                app.deck_views[deck_idx].deck_preset_mut().macro_mappings = all_mappings;

                // Send initial neutral macro values (0.5) to all stems
                // so engine-side apply_macros() starts from center position
                for macro_idx in 0..NUM_MACROS {
                    for stem_idx in 0..4 {
                        if resolved.stems[stem_idx].is_some() {
                            if let Some(stem) = Stem::from_index(stem_idx) {
                                app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                                    deck: deck_idx,
                                    stem,
                                    macro_index: macro_idx,
                                    value: 0.5,
                                });
                            }
                        }
                    }
                }

                // If multiband editor is open for this deck, update it
                if app.multiband_editor.is_open && app.multiband_editor.deck == deck_idx {
                    let stem_idx = app.multiband_editor.stem;
                    if let Some(ref stem_config) = resolved.stems[stem_idx] {
                        stem_config.apply_to_editor_state(&mut app.multiband_editor);
                        app.multiband_editor.rebuild_macro_mappings_index();
                        mesh_widgets::multiband::ensure_effect_knobs_exist(&mut app.multiband_editor);
                    }
                }

                app.status = format!("Loaded deck preset '{}' on deck {}", name, deck_idx + 1);
            }
            Err(e) => {
                log::error!("Failed to load deck preset '{}': {}", name, e);
                app.status = format!("Failed to load deck preset: {}", e);
            }
        }
    } else {
        // Clear the deck preset (passthrough mode for all stems)
        app.deck_views[deck_idx].deck_preset_mut().clear_preset();

        // Clear effects from all multiband containers
        for stem_idx in 0..4 {
            if let Some(stem) = Stem::from_index(stem_idx) {
                clear_multiband_effects(app, deck_idx, stem);
            }
        }

        app.status = format!("Cleared deck preset on deck {}", deck_idx + 1);
    }

    Task::none()
}

/// Apply a preset configuration to the multiband container
pub(super) fn apply_preset_to_multiband(
    app: &mut MeshApp,
    deck_idx: usize,
    stem: Stem,
    config: &StemPresetConfig,
) {
    // Reset multiband to clean single-band state
    clear_multiband_effects(app, deck_idx, stem);

    // Add extra bands if preset has more than 1
    for _ in 1..config.bands.len() {
        app.domain.send_command(mesh_core::engine::EngineCommand::AddMultibandBand {
            deck: deck_idx,
            stem,
        });
    }

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

        match &result {
            Ok(info) => {
                log::info!(
                    "[PRESET_LOAD] Added pre-fx '{}' ({}) to deck {} stem {:?} ({} params)",
                    effect.name, effect.source, deck_idx, stem, info.params.len()
                );
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
            Err(e) => {
                log::error!(
                    "[PRESET_LOAD] FAILED to create pre-fx '{}' ({}) for deck {} stem {:?}: {}",
                    effect.name, effect.source, deck_idx, stem, e
                );
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

            match &result {
                Ok(info) => {
                    log::info!(
                        "[PRESET_LOAD] Added band {} effect '{}' ({}) to deck {} stem {:?} ({} params)",
                        band_idx, effect.name, effect.source, deck_idx, stem, info.params.len()
                    );
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
                Err(e) => {
                    log::error!(
                        "[PRESET_LOAD] FAILED to create band {} effect '{}' ({}) for deck {} stem {:?}: {}",
                        band_idx, effect.name, effect.source, deck_idx, stem, e
                    );
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

        match &result {
            Ok(info) => {
                log::info!(
                    "[PRESET_LOAD] Added post-fx '{}' ({}) to deck {} stem {:?} ({} params)",
                    effect.name, effect.source, deck_idx, stem, info.params.len()
                );
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
            Err(e) => {
                log::error!(
                    "[PRESET_LOAD] FAILED to create post-fx '{}' ({}) for deck {} stem {:?}: {}",
                    effect.name, effect.source, deck_idx, stem, e
                );
            }
        }
    }

    // Apply dry/wet values from preset
    // Pre-fx effect dry/wet
    for (effect_idx, effect) in config.pre_fx.iter().enumerate() {
        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxEffectDryWet {
            deck: deck_idx,
            stem,
            effect_index: effect_idx,
            mix: effect.dry_wet,
        });
    }
    // Band effect dry/wet + band chain dry/wet
    for (band_idx, band) in config.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandEffectDryWet {
                deck: deck_idx,
                stem,
                band_index: band_idx,
                effect_index: effect_idx,
                mix: effect.dry_wet,
            });
        }
        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandChainDryWet {
            deck: deck_idx,
            stem,
            band_index: band_idx,
            mix: band.chain_dry_wet,
        });
    }
    // Post-fx effect dry/wet
    for (effect_idx, effect) in config.post_fx.iter().enumerate() {
        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxEffectDryWet {
            deck: deck_idx,
            stem,
            effect_index: effect_idx,
            mix: effect.dry_wet,
        });
    }
    // Chain-level and global dry/wet
    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxChainDryWet {
        deck: deck_idx,
        stem,
        mix: config.pre_fx_chain_dry_wet,
    });
    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxChainDryWet {
        deck: deck_idx,
        stem,
        mix: config.post_fx_chain_dry_wet,
    });
    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandGlobalDryWet {
        deck: deck_idx,
        stem,
        mix: config.global_dry_wet,
    });

    // Clear existing macro mappings
    for macro_idx in 0..NUM_MACROS {
        app.domain.send_command(mesh_core::engine::EngineCommand::ClearMultibandMacroMappings {
            deck: deck_idx,
            stem,
            macro_index: macro_idx,
        });
    }

    use mesh_core::effect::multiband::EffectLocation;

    // Apply macro mappings from the preset (engine-side for knob assignment params)
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

    log::info!(
        "[PRESET_LOAD] Applied '{}' to deck {} stem {:?}: {} pre-fx, {} bands ({} band effects total), {} post-fx, global_dry_wet={:.2}",
        config.name, deck_idx, stem,
        config.pre_fx.len(),
        config.bands.len(),
        config.bands.iter().map(|b| b.effects.len()).sum::<usize>(),
        config.post_fx.len(),
        config.global_dry_wet,
    );
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

/// Clear all effects from a stem's multiband container
///
/// Uses ResetMultiband to atomically replace the MultibandHost with a fresh one,
/// clearing all bands, effect chains, crossovers, and macro mappings.
fn clear_multiband_effects(app: &mut MeshApp, deck_idx: usize, stem: Stem) {
    app.domain.send_command(mesh_core::engine::EngineCommand::ResetMultiband {
        deck: deck_idx,
        stem,
    });
    log::debug!("Reset multiband for deck {} stem {:?}", deck_idx, stem);
}

/// Apply macro modulation for a single mapping directly to the engine
///
/// Sends the modulated value directly. Handles both effect params and dry/wet targets.
fn apply_macro_modulation_direct_single(
    domain: &mut crate::domain::MeshDomain,
    deck_idx: usize,
    stem: Stem,
    macro_value: f32,
    mapping: &mesh_widgets::MacroParamMapping,
) {
    use mesh_widgets::multiband::EffectChainLocation;
    use mesh_widgets::MacroTargetType;

    let modulated_value = mapping.modulate(macro_value);

    log::info!(
        "[MACRO_DIRECT] stem={} {:?} target={:?} loc={:?} effect={} param={} base={:.3} offset={:.3} macro={:.3} -> modulated={:.3}",
        mapping.stem_index, stem, mapping.target, mapping.location,
        mapping.effect_index, mapping.param_index,
        mapping.base_value, mapping.offset_range,
        macro_value, modulated_value,
    );

    match mapping.target {
        MacroTargetType::EffectParam => {
            match mapping.location {
                EffectChainLocation::PreFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxParam {
                        deck: deck_idx,
                        stem,
                        effect_index: mapping.effect_index,
                        param_index: mapping.param_index,
                        value: modulated_value,
                    });
                }
                EffectChainLocation::Band(band_idx) => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandEffectParam {
                        deck: deck_idx,
                        stem,
                        band_index: band_idx,
                        effect_index: mapping.effect_index,
                        param_index: mapping.param_index,
                        value: modulated_value,
                    });
                }
                EffectChainLocation::PostFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxParam {
                        deck: deck_idx,
                        stem,
                        effect_index: mapping.effect_index,
                        param_index: mapping.param_index,
                        value: modulated_value,
                    });
                }
            }
        }
        MacroTargetType::EffectDryWet => {
            match mapping.location {
                EffectChainLocation::PreFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxEffectDryWet {
                        deck: deck_idx,
                        stem,
                        effect_index: mapping.effect_index,
                        mix: modulated_value,
                    });
                }
                EffectChainLocation::Band(band_idx) => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandEffectDryWet {
                        deck: deck_idx,
                        stem,
                        band_index: band_idx,
                        effect_index: mapping.effect_index,
                        mix: modulated_value,
                    });
                }
                EffectChainLocation::PostFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxEffectDryWet {
                        deck: deck_idx,
                        stem,
                        effect_index: mapping.effect_index,
                        mix: modulated_value,
                    });
                }
            }
        }
        MacroTargetType::ChainDryWet => {
            match mapping.location {
                EffectChainLocation::PreFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPreFxChainDryWet {
                        deck: deck_idx,
                        stem,
                        mix: modulated_value,
                    });
                }
                EffectChainLocation::Band(band_idx) => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandBandChainDryWet {
                        deck: deck_idx,
                        stem,
                        band_index: band_idx,
                        mix: modulated_value,
                    });
                }
                EffectChainLocation::PostFx => {
                    domain.send_command(mesh_core::engine::EngineCommand::SetMultibandPostFxChainDryWet {
                        deck: deck_idx,
                        stem,
                        mix: modulated_value,
                    });
                }
            }
        }
        MacroTargetType::GlobalDryWet => {
            domain.send_command(mesh_core::engine::EngineCommand::SetMultibandGlobalDryWet {
                deck: deck_idx,
                stem,
                mix: modulated_value,
            });
        }
    }
}

/// Extract macro-to-parameter mappings from a stem preset config for a specific stem
///
/// Collects all macro-mapped parameters from a single stem's preset config
/// and adds them to the deck-level mapping arrays (with stem_index set).
fn extract_deck_macro_mappings(
    stem_idx: usize,
    config: &StemPresetConfig,
    mappings: &mut [Vec<mesh_widgets::MacroParamMapping>; NUM_MACROS],
) {
    use mesh_widgets::multiband::EffectChainLocation;
    use mesh_widgets::{MacroParamMapping, MacroTargetType};

    /// Push a mapping if the macro index is valid
    fn push(mappings: &mut [Vec<MacroParamMapping>; NUM_MACROS], macro_index: usize, mapping: MacroParamMapping) {
        if macro_index < NUM_MACROS {
            mappings[macro_index].push(mapping);
        }
    }

    /// Extract macro mappings from a single effect (knob params + dry/wet)
    fn extract_effect(
        mappings: &mut [Vec<MacroParamMapping>; NUM_MACROS],
        stem_idx: usize,
        location: EffectChainLocation,
        effect_idx: usize,
        effect: &EffectPresetConfig,
    ) {
        // Knob assignment macro mappings (effect params)
        for assignment in &effect.knob_assignments {
            if let (Some(param_index), Some(ref macro_mapping)) = (assignment.param_index, &assignment.macro_mapping) {
                if let Some(macro_index) = macro_mapping.macro_index {
                    push(mappings, macro_index, MacroParamMapping {
                        stem_index: stem_idx,
                        location,
                        effect_index: effect_idx,
                        param_index,
                        target: MacroTargetType::EffectParam,
                        base_value: assignment.value,
                        offset_range: macro_mapping.offset_range,
                    });
                }
            }
        }

        // Per-effect dry/wet macro mapping
        if let Some(ref dw_mapping) = effect.dry_wet_macro_mapping {
            if let Some(macro_index) = dw_mapping.macro_index {
                push(mappings, macro_index, MacroParamMapping {
                    stem_index: stem_idx,
                    location,
                    effect_index: effect_idx,
                    param_index: 0,
                    target: MacroTargetType::EffectDryWet,
                    base_value: effect.dry_wet,
                    offset_range: dw_mapping.offset_range,
                });
            }
        }
    }

    // Process pre-fx effects
    for (effect_idx, effect) in config.pre_fx.iter().enumerate() {
        extract_effect(mappings, stem_idx, EffectChainLocation::PreFx, effect_idx, effect);
    }

    // Process band effects + band chain dry/wet
    for (band_idx, band) in config.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            extract_effect(mappings, stem_idx, EffectChainLocation::Band(band_idx), effect_idx, effect);
        }

        // Band chain dry/wet macro mapping
        if let Some(ref chain_dw) = band.chain_dry_wet_macro_mapping {
            if let Some(macro_index) = chain_dw.macro_index {
                push(mappings, macro_index, MacroParamMapping {
                    stem_index: stem_idx,
                    location: EffectChainLocation::Band(band_idx),
                    effect_index: 0,
                    param_index: 0,
                    target: MacroTargetType::ChainDryWet,
                    base_value: band.chain_dry_wet,
                    offset_range: chain_dw.offset_range,
                });
            }
        }
    }

    // Process post-fx effects
    for (effect_idx, effect) in config.post_fx.iter().enumerate() {
        extract_effect(mappings, stem_idx, EffectChainLocation::PostFx, effect_idx, effect);
    }

    // Pre-FX chain dry/wet macro mapping
    if let Some(ref m) = config.pre_fx_chain_dry_wet_macro_mapping {
        if let Some(macro_index) = m.macro_index {
            push(mappings, macro_index, MacroParamMapping {
                stem_index: stem_idx,
                location: EffectChainLocation::PreFx,
                effect_index: 0,
                param_index: 0,
                target: MacroTargetType::ChainDryWet,
                base_value: config.pre_fx_chain_dry_wet,
                offset_range: m.offset_range,
            });
        }
    }

    // Post-FX chain dry/wet macro mapping
    if let Some(ref m) = config.post_fx_chain_dry_wet_macro_mapping {
        if let Some(macro_index) = m.macro_index {
            push(mappings, macro_index, MacroParamMapping {
                stem_index: stem_idx,
                location: EffectChainLocation::PostFx,
                effect_index: 0,
                param_index: 0,
                target: MacroTargetType::ChainDryWet,
                base_value: config.post_fx_chain_dry_wet,
                offset_range: m.offset_range,
            });
        }
    }

    // Global dry/wet macro mapping
    if let Some(ref m) = config.global_dry_wet_macro_mapping {
        if let Some(macro_index) = m.macro_index {
            push(mappings, macro_index, MacroParamMapping {
                stem_index: stem_idx,
                location: EffectChainLocation::PreFx, // unused for global
                effect_index: 0,
                param_index: 0,
                target: MacroTargetType::GlobalDryWet,
                base_value: config.global_dry_wet,
                offset_range: m.offset_range,
            });
        }
    }

    log::debug!(
        "[MACRO_EXTRACT] Extracted mappings for stem {}: macro0={}, macro1={}, macro2={}, macro3={}",
        stem_idx, mappings[0].len(), mappings[1].len(), mappings[2].len(), mappings[3].len()
    );
}

/// Handle a completed preset load from the background loader thread.
///
/// Extracts the built MultibandHost and sends a single `SwapMultiband` command
/// to the audio engine, replacing 300-1000+ individual commands with one atomic swap.
pub(crate) fn handle_preset_loaded(
    app: &mut MeshApp,
    msg: crate::ui::app::PresetLoadedMsg,
) -> Task<Message> {
    // Extract the result from the Arc<Mutex<Option<>>> wrapper
    let result = match msg.0.lock() {
        Ok(mut guard) => match guard.take() {
            Some(r) => r,
            None => {
                log::warn!("[PRESET_LOADER] PresetLoadResult already consumed");
                return Task::none();
            }
        },
        Err(e) => {
            log::error!("[PRESET_LOADER] Failed to lock PresetLoadResult: {}", e);
            return Task::none();
        }
    };

    let deck = result.deck;
    let stem = result.stem;

    match result.result {
        Ok(multiband) => {
            log::info!(
                "[PRESET_LOADER] Swapping multiband for deck {} stem {:?} (id={})",
                deck, stem, result.id
            );
            // Send a single SwapMultiband command — replaces the entire MultibandHost atomically
            app.domain.send_command(mesh_core::engine::EngineCommand::SwapMultiband {
                deck,
                stem,
                multiband: Box::new(multiband),
            });
        }
        Err(e) => {
            log::error!(
                "[PRESET_LOADER] Failed to build multiband for deck {} stem {:?}: {}",
                deck, stem, e
            );
            app.status = format!("Preset load failed: {}", e);
        }
    }

    Task::none()
}
