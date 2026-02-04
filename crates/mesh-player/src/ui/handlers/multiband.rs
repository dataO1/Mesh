//! Multiband editor message handler
//!
//! Handles multiband container editing: crossover control, band management,
//! effect chains per band, and macro knob routing.

use iced::Task;
use mesh_core::effect::EffectInfo;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{
    self, ensure_effect_knobs_exist, AvailableParam, EffectChainLocation, EffectSourceType,
    EffectUiState, KnobAssignment, MultibandPresetConfig, ParamMacroMapping, MAX_UI_KNOBS,
};
use mesh_widgets::{MultibandEditorMessage, DEFAULT_SENSITIVITY};

use crate::ui::app::MeshApp;
use crate::ui::message::Message;

/// Create an EffectUiState from actual effect info returned by the backend
fn create_effect_state_from_info(
    id: String,
    effect_info: &EffectInfo,
    source: EffectSourceType,
) -> EffectUiState {
    // Convert all params from the effect info to available params
    let available_params: Vec<AvailableParam> = effect_info.params.iter()
        .map(|p| AvailableParam {
            name: p.name.clone(),
            min: p.min,
            max: p.max,
            default: p.default,
            unit: p.unit.clone(),
        })
        .collect();

    // Create knob assignments - assign first 8 params by default
    let mut knob_assignments: [KnobAssignment; MAX_UI_KNOBS] = Default::default();
    for (i, assignment) in knob_assignments.iter_mut().enumerate() {
        if i < effect_info.params.len() {
            assignment.param_index = Some(i);
            assignment.value = effect_info.params[i].default;
        } else {
            assignment.param_index = None;
            assignment.value = 0.5;
        }
    }

    // Create param_names and param_values for the first 8 or fewer params
    let param_count = effect_info.params.len().min(8);
    let param_names: Vec<String> = effect_info.params.iter()
        .take(8)
        .map(|p| p.name.clone())
        .collect();
    let param_values: Vec<f32> = effect_info.params.iter()
        .take(8)
        .map(|p| p.default)
        .collect();

    EffectUiState {
        id,
        name: effect_info.name.clone(),
        category: effect_info.category.clone(),
        source,
        bypassed: false,
        available_params,
        knob_assignments,
        param_names,
        param_values,
        param_mappings: vec![ParamMacroMapping::default(); param_count],
    }
}

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
            // Ensure all effect knobs exist before view is rendered
            ensure_effect_knobs_exist(&mut app.multiband_editor);
            Task::none()
        }

        Close => {
            app.multiband_editor.close();
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Pre-FX chain management
        // ─────────────────────────────────────────────────────────────────────
        OpenPreFxEffectPicker => {
            let deck = app.multiband_editor.deck;
            let stem_idx = app.multiband_editor.stem;
            // Open effect picker for pre-fx (use band 255 as marker for pre-fx)
            app.effect_picker.open_for_band(deck, stem_idx, 255);
            log::info!("Opening effect picker for pre-fx (deck {} stem {})", deck, stem_idx);
            Task::none()
        }

        PreFxEffectSelected { effect_id, source } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Check for existing PD effects - warn about libpd limitation
            let existing_pd_count = app.multiband_editor.pre_fx.iter()
                .filter(|e| e.source == EffectSourceType::Pd).count();
            let is_pd = source == "pd";

            let (result, source_type) = match source.as_str() {
                "pd" => (app.domain.add_pd_effect_pre_fx(deck, stem, &effect_id), EffectSourceType::Pd),
                "clap" => (app.domain.add_clap_effect_pre_fx(deck, stem, &effect_id), EffectSourceType::Clap),
                _ => (Err(format!("Unknown effect source: {}", source)), EffectSourceType::Native),
            };

            match result {
                Err(e) => {
                    log::error!("Failed to add pre-fx effect: {}", e);
                    app.status = format!("Failed to add pre-fx: {}", e);
                }
                Ok(effect_info) => {
                    // Warn if adding multiple PD effects
                    if is_pd && existing_pd_count > 0 {
                        log::warn!("Multiple PD effects in pre-fx - libpd processes all patches in parallel!");
                        app.status = format!("Added PD effect - ⚠ {} PD effects (parallel processing)", existing_pd_count + 1);
                    } else {
                        log::info!("Added {} pre-fx effect '{}' ({} params)",
                            source, effect_id, effect_info.params.len());
                    }

                    app.multiband_editor.pre_fx.push(create_effect_state_from_info(
                        effect_id.clone(),
                        &effect_info,
                        source_type,
                    ));
                    // Create knobs for the new effect's parameters
                    ensure_effect_knobs_exist(&mut app.multiband_editor);
                }
            }
            Task::none()
        }

        RemovePreFxEffect(index) => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            app.domain.remove_pre_fx_effect(deck, stem, index);
            if index < app.multiband_editor.pre_fx.len() {
                app.multiband_editor.pre_fx.remove(index);
            }
            Task::none()
        }

        TogglePreFxBypass(index) => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            if let Some(effect) = app.multiband_editor.pre_fx.get_mut(index) {
                effect.bypassed = !effect.bypassed;
                app.domain.set_pre_fx_bypass(deck, stem, index, effect.bypassed);
            }
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

            // Check for existing PD effects in this band - warn about libpd limitation
            let existing_pd_count = app.multiband_editor.bands.get(band)
                .map(|b| b.effects.iter().filter(|e| e.source == EffectSourceType::Pd).count())
                .unwrap_or(0);
            let is_pd = source == "pd";

            // Add effect based on source type to the specified band
            let (result, source_type) = match source.as_str() {
                "pd" => (app.domain.add_pd_effect(deck, stem, &effect_id, band), EffectSourceType::Pd),
                "clap" => (app.domain.add_clap_effect(deck, stem, &effect_id, band), EffectSourceType::Clap),
                _ => (Err(format!("Unknown effect source: {}", source)), EffectSourceType::Native),
            };

            match result {
                Err(e) => {
                    log::error!("Failed to add effect to band {}: {}", band, e);
                    app.status = format!("Failed to add effect: {}", e);
                }
                Ok(effect_info) => {
                    // Warn if adding multiple PD effects
                    if is_pd && existing_pd_count > 0 {
                        log::warn!("Multiple PD effects in band {} - libpd processes all patches in parallel!", band);
                        app.status = format!("Added PD effect - ⚠ {} PD effects (parallel processing)", existing_pd_count + 1);
                    } else {
                        log::info!("Added {} effect '{}' to band {} ({} params)",
                            source, effect_id, band, effect_info.params.len());
                    }

                    // Add effect to UI state using the actual effect info from backend
                    if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                        band_state.effects.push(create_effect_state_from_info(
                            effect_id.clone(),
                            &effect_info,
                            source_type,
                        ));
                    }

                    // Create knobs for the new effect's parameters
                    ensure_effect_knobs_exist(&mut app.multiband_editor);
                    // Sync macro values from deck view
                    sync_from_backend(app);
                }
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

        SelectEffect { location, effect } => {
            app.multiband_editor.selected_effect = Some((location, effect));
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Unified effect knob handling (stateful knobs)
        // ─────────────────────────────────────────────────────────────────────
        EffectKnob { location, effect, param, event } => {
            use mesh_widgets::knob::KnobEvent;
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Track drag state for global mouse capture
            match &event {
                KnobEvent::Pressed => {
                    app.multiband_editor.dragging_effect_knob = Some((location, effect, param));
                }
                KnobEvent::Released => {
                    app.multiband_editor.dragging_effect_knob = None;
                }
                KnobEvent::Moved(_) => {}
            }

            // Get the knob and handle the event
            let knob = app.multiband_editor.get_effect_knob(location, effect, param);
            if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
                // Update local state
                app.multiband_editor.set_effect_param_value(location, effect, param, new_value);

                // Send to backend based on location
                match location {
                    EffectChainLocation::PreFx => {
                        app.domain.set_pre_fx_param(deck, stem, effect, param, new_value);
                    }
                    EffectChainLocation::Band(band_idx) => {
                        app.domain.set_band_effect_param(deck, stem, band_idx, effect, param, new_value);
                    }
                    EffectChainLocation::PostFx => {
                        app.domain.set_post_fx_param(deck, stem, effect, param, new_value);
                    }
                }
            }
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Post-FX chain management
        // ─────────────────────────────────────────────────────────────────────
        OpenPostFxEffectPicker => {
            let deck = app.multiband_editor.deck;
            let stem_idx = app.multiband_editor.stem;
            // Open effect picker for post-fx (use band 254 as marker)
            app.effect_picker.open_for_band(deck, stem_idx, 254);
            log::info!("Opening effect picker for post-fx (deck {} stem {})", deck, stem_idx);
            Task::none()
        }

        PostFxEffectSelected { effect_id, source } => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Check for existing PD effects - warn about libpd limitation
            let existing_pd_count = app.multiband_editor.post_fx.iter()
                .filter(|e| e.source == EffectSourceType::Pd).count();
            let is_pd = source == "pd";

            // Add effect to post-fx chain in backend
            let (result, source_type) = match source.as_str() {
                "pd" => (app.domain.add_pd_effect_post_fx(deck, stem, &effect_id), EffectSourceType::Pd),
                "clap" => (app.domain.add_clap_effect_post_fx(deck, stem, &effect_id), EffectSourceType::Clap),
                _ => (Err(format!("Unknown effect source: {}", source)), EffectSourceType::Native),
            };

            match result {
                Err(e) => {
                    log::error!("Failed to add post-fx effect: {}", e);
                    app.status = format!("Failed to add effect: {}", e);
                }
                Ok(effect_info) => {
                    // Warn if adding multiple PD effects
                    if is_pd && existing_pd_count > 0 {
                        log::warn!("Multiple PD effects in post-fx - libpd processes all patches in parallel!");
                        app.status = format!("Added PD effect - ⚠ {} PD effects (parallel processing)", existing_pd_count + 1);
                    } else {
                        log::info!("Added {} post-fx effect '{}' ({} params)",
                            source, effect_id, effect_info.params.len());
                    }

                    // Add effect to UI state using actual effect info from backend
                    app.multiband_editor.post_fx.push(create_effect_state_from_info(
                        effect_id.clone(),
                        &effect_info,
                        source_type,
                    ));
                    // Create knobs for the new effect's parameters
                    ensure_effect_knobs_exist(&mut app.multiband_editor);
                }
            }
            Task::none()
        }

        RemovePostFxEffect(index) => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            app.domain.remove_post_fx_effect(deck, stem, index);

            if index < app.multiband_editor.post_fx.len() {
                app.multiband_editor.post_fx.remove(index);
            }
            Task::none()
        }

        TogglePostFxBypass(index) => {
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            if let Some(effect) = app.multiband_editor.post_fx.get_mut(index) {
                effect.bypassed = !effect.bypassed;
                app.domain.set_post_fx_bypass(deck, stem, index, effect.bypassed);
            }
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Macro control (unified stateful knob handling)
        // ─────────────────────────────────────────────────────────────────────
        MacroKnob { index, event } => {
            use mesh_widgets::knob::KnobEvent;
            let deck = app.multiband_editor.deck;
            let stem_idx = app.multiband_editor.stem;
            let stem = Stem::ALL[stem_idx];

            // Track drag state for global mouse capture
            match &event {
                KnobEvent::Pressed => {
                    app.multiband_editor.dragging_macro_knob = Some(index);
                }
                KnobEvent::Released => {
                    app.multiband_editor.dragging_macro_knob = None;
                }
                KnobEvent::Moved(_) => {}
            }

            // Get the knob and handle the event
            if let Some(knob) = app.multiband_editor.macro_knobs.get_mut(index) {
                if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
                    // Sync to deck view (bidirectional sync for consistency)
                    if deck < 4 && stem_idx < 4 && index < 8 {
                        app.deck_views[deck].set_stem_knob(stem_idx, index, new_value);
                    }

                    // Send to engine
                    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                        deck,
                        stem,
                        macro_index: index,
                        value: new_value,
                    });
                }
            }
            Task::none()
        }

        RenameMacro { index, name } => {
            app.multiband_editor.set_macro_name(index, name);
            Task::none()
        }

        StartDragMacro(index) => {
            app.multiband_editor.dragging_macro = Some(index);
            Task::none()
        }

        EndDragMacro => {
            app.multiband_editor.dragging_macro = None;
            Task::none()
        }

        DropMacroOnParam { macro_index, band, effect, param } => {
            // Update UI state - set the mapping on the effect's param
            if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                if let Some(effect_state) = band_state.effects.get_mut(effect) {
                    if let Some(mapping) = effect_state.param_mappings.get_mut(param) {
                        mapping.macro_index = Some(macro_index);
                    }
                }
            }

            // Update macro's mapping count
            if let Some(macro_state) = app.multiband_editor.macros.get_mut(macro_index) {
                macro_state.mapping_count += 1;
            }

            // Clear drag state
            app.multiband_editor.dragging_macro = None;

            // TODO: Send mapping to backend MultibandHost
            log::info!("Mapped macro {} to band {} effect {} param {}", macro_index, band, effect, param);
            Task::none()
        }

        RemoveParamMapping { band, effect, param } => {
            // Get the macro that was mapped and decrement its count
            if let Some(band_state) = app.multiband_editor.bands.get_mut(band) {
                if let Some(effect_state) = band_state.effects.get_mut(effect) {
                    if let Some(mapping) = effect_state.param_mappings.get_mut(param) {
                        if let Some(old_macro) = mapping.macro_index {
                            if let Some(macro_state) = app.multiband_editor.macros.get_mut(old_macro) {
                                macro_state.mapping_count = macro_state.mapping_count.saturating_sub(1);
                            }
                        }
                        mapping.macro_index = None;
                    }
                }
            }
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
            // Refresh preset list when opening browser
            let presets = multiband::list_presets(&app.config.collection_path);
            app.multiband_editor.available_presets = presets;
            app.multiband_editor.preset_browser_open = true;
            Task::none()
        }

        ClosePresetBrowser => {
            app.multiband_editor.preset_browser_open = false;
            Task::none()
        }

        OpenSaveDialog => {
            // Pre-fill with stem name as default preset name
            app.multiband_editor.preset_name_input = format!(
                "{}-{}",
                app.multiband_editor.stem_name,
                app.multiband_editor.bands.len()
            );
            app.multiband_editor.save_dialog_open = true;
            Task::none()
        }

        CloseSaveDialog => {
            app.multiband_editor.save_dialog_open = false;
            Task::none()
        }

        SetPresetNameInput(name) => {
            app.multiband_editor.preset_name_input = name;
            Task::none()
        }

        LoadPreset(name) => {
            match multiband::load_preset(&app.config.collection_path, &name) {
                Ok(preset_config) => {
                    // Apply preset to editor state
                    preset_config.apply_to_editor_state(&mut app.multiband_editor);
                    app.multiband_editor.preset_browser_open = false;

                    // TODO: Recreate effects in backend based on preset
                    // For now, just update UI state - effects need to be re-added manually
                    log::info!("Loaded preset '{}' - UI state updated", name);
                    app.status = format!("Loaded preset: {}", name);
                }
                Err(e) => {
                    log::error!("Failed to load preset: {}", e);
                    app.status = format!("Failed to load preset: {}", e);
                }
            }
            Task::none()
        }

        SavePreset => {
            let name = app.multiband_editor.preset_name_input.trim().to_string();
            if name.is_empty() {
                app.status = "Preset name cannot be empty".to_string();
                return Task::none();
            }

            // Create preset config from current editor state
            let preset_config = MultibandPresetConfig::from_editor_state(
                &app.multiband_editor,
                &name,
            );

            // Save to disk
            match multiband::save_preset(&preset_config, &app.config.collection_path) {
                Ok(()) => {
                    log::info!("Saved preset '{}'", name);
                    app.status = format!("Saved preset: {}", name);
                    app.multiband_editor.save_dialog_open = false;
                    // Refresh preset list
                    app.multiband_editor.available_presets =
                        multiband::list_presets(&app.config.collection_path);
                }
                Err(e) => {
                    log::error!("Failed to save preset: {}", e);
                    app.status = format!("Failed to save preset: {}", e);
                }
            }
            Task::none()
        }

        DeletePreset(name) => {
            match multiband::delete_preset(&app.config.collection_path, &name) {
                Ok(()) => {
                    log::info!("Deleted preset '{}'", name);
                    app.status = format!("Deleted preset: {}", name);
                    // Refresh preset list
                    app.multiband_editor.available_presets =
                        multiband::list_presets(&app.config.collection_path);
                }
                Err(e) => {
                    log::error!("Failed to delete preset: {}", e);
                    app.status = format!("Failed to delete: {}", e);
                }
            }
            Task::none()
        }

        RefreshPresets => {
            app.multiband_editor.available_presets =
                multiband::list_presets(&app.config.collection_path);
            Task::none()
        }

        SetAvailablePresets(presets) => {
            app.multiband_editor.available_presets = presets;
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Parameter picker
        // ─────────────────────────────────────────────────────────────────────
        OpenParamPicker { location, effect, knob } => {
            app.multiband_editor.param_picker_open = Some((location, effect, knob));
            app.multiband_editor.param_picker_search = String::new();
            Task::none()
        }

        CloseParamPicker => {
            app.multiband_editor.param_picker_open = None;
            app.multiband_editor.param_picker_search = String::new();
            Task::none()
        }

        AssignParam { location, effect, knob, param_index } => {
            // Get the effect to update
            let effect_state = match location {
                EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                EffectChainLocation::Band(band_idx) => app
                    .multiband_editor
                    .bands
                    .get_mut(band_idx)
                    .and_then(|b| b.effects.get_mut(effect)),
                EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
            };

            if let Some(effect_state) = effect_state {
                if let Some(assignment) = effect_state.knob_assignments.get_mut(knob) {
                    // Update the knob assignment
                    assignment.param_index = param_index;

                    // If assigning to a parameter, set the value from the param's default
                    if let Some(idx) = param_index {
                        if let Some(param) = effect_state.available_params.get(idx) {
                            assignment.value = param.default;
                        }
                    }

                    // Update legacy param_names for backwards compatibility
                    if knob < effect_state.param_names.len() {
                        if let Some(idx) = param_index {
                            if let Some(param) = effect_state.available_params.get(idx) {
                                effect_state.param_names[knob] = param.name.clone();
                            }
                        } else {
                            effect_state.param_names[knob] = "[assign]".to_string();
                        }
                    }
                }
            }

            // Close the picker after assignment
            app.multiband_editor.param_picker_open = None;
            app.multiband_editor.param_picker_search = String::new();
            Task::none()
        }

        SetParamPickerFilter(filter) => {
            app.multiband_editor.param_picker_search = filter;
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Global mouse events for knob drag capture
        // ─────────────────────────────────────────────────────────────────────
        GlobalMouseMoved(position) => {
            use mesh_widgets::knob::KnobEvent;
            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];
            let stem_idx = app.multiband_editor.stem;

            // Route to dragging effect knob
            if let Some((location, effect, param)) = app.multiband_editor.dragging_effect_knob {
                let knob = app.multiband_editor.get_effect_knob(location, effect, param);
                if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                    app.multiband_editor.set_effect_param_value(location, effect, param, new_value);

                    match location {
                        EffectChainLocation::PreFx => {
                            app.domain.set_pre_fx_param(deck, stem, effect, param, new_value);
                        }
                        EffectChainLocation::Band(band_idx) => {
                            app.domain.set_band_effect_param(deck, stem, band_idx, effect, param, new_value);
                        }
                        EffectChainLocation::PostFx => {
                            app.domain.set_post_fx_param(deck, stem, effect, param, new_value);
                        }
                    }
                }
            }

            // Route to dragging macro knob
            if let Some(index) = app.multiband_editor.dragging_macro_knob {
                if let Some(knob) = app.multiband_editor.macro_knobs.get_mut(index) {
                    if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                        if deck < 4 && stem_idx < 4 && index < 8 {
                            app.deck_views[deck].set_stem_knob(stem_idx, index, new_value);
                        }

                        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                            deck,
                            stem,
                            macro_index: index,
                            value: new_value,
                        });
                    }
                }
            }

            Task::none()
        }

        GlobalMouseReleased => {
            use mesh_widgets::knob::KnobEvent;

            // Release dragging effect knob
            if let Some((location, effect, param)) = app.multiband_editor.dragging_effect_knob.take() {
                let knob = app.multiband_editor.get_effect_knob(location, effect, param);
                knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
            }

            // Release dragging macro knob
            if let Some(index) = app.multiband_editor.dragging_macro_knob.take() {
                if let Some(knob) = app.multiband_editor.macro_knobs.get_mut(index) {
                    knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                }
            }

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
