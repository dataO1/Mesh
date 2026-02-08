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
        gui_open: false,
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
            // Effect editing moved to mesh-cue - use preset selector instead
            log::debug!("Effect picker not available in mesh-player - use mesh-cue for preset editing");
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
        OpenEffectPicker(_band_idx) => {
            // Effect editing moved to mesh-cue - use preset selector instead
            log::debug!("Effect picker not available in mesh-player - use mesh-cue for preset editing");
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

            // Look up the actual parameter index from knob_assignments
            // (param is the knob slot 0-7, but the actual param could be different after learning)
            let actual_param_index = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                };
                effect_state
                    .and_then(|e| e.knob_assignments.get(param))
                    .and_then(|a| a.param_index)
                    .unwrap_or(param) // Fallback to knob slot if no assignment
            };

            // Get the knob and handle the event
            let knob = app.multiband_editor.get_effect_knob(location, effect, param);
            if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
                // Update local state (base value)
                app.multiband_editor.set_effect_param_value(location, effect, param, new_value);

                // Check if this knob has a macro mapping
                let macro_mapping = {
                    let effect_state = match location {
                        EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                        EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect)),
                        EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                    };
                    effect_state
                        .and_then(|e| e.knob_assignments.get(param))
                        .and_then(|a| a.macro_mapping.clone())
                };

                // Calculate the value to send (apply modulation if mapped)
                let value_to_send = if let Some(ref mapping) = macro_mapping {
                    if let Some(macro_idx) = mapping.macro_index {
                        // Get current macro value
                        let macro_value = app.multiband_editor.macro_value(macro_idx);
                        // Apply modulation
                        mapping.modulate(new_value, macro_value)
                    } else {
                        new_value
                    }
                } else {
                    new_value
                };

                // Update modulation bounds visualization if mapped
                if let Some(mapping) = macro_mapping {
                    use mesh_widgets::knob::ModulationRange;
                    let key = (location, effect, param);
                    if let Some(knob) = app.multiband_editor.effect_knobs.get_mut(&key) {
                        let (min, max) = mapping.modulation_bounds(new_value);
                        knob.set_modulations(vec![ModulationRange::new(
                            min,
                            max,
                            iced::Color::from_rgb(0.9, 0.6, 0.2),
                        )]);
                    }
                }

                // Send to backend using the actual parameter index (not the knob slot)
                match location {
                    EffectChainLocation::PreFx => {
                        app.domain.set_pre_fx_param(deck, stem, effect, actual_param_index, value_to_send);
                    }
                    EffectChainLocation::Band(band_idx) => {
                        app.domain.set_band_effect_param(deck, stem, band_idx, effect, actual_param_index, value_to_send);
                    }
                    EffectChainLocation::PostFx => {
                        app.domain.set_post_fx_param(deck, stem, effect, actual_param_index, value_to_send);
                    }
                }
            }
            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // Post-FX chain management
        // ─────────────────────────────────────────────────────────────────────
        OpenPostFxEffectPicker => {
            // Effect editing moved to mesh-cue - use preset selector instead
            log::debug!("Effect picker not available in mesh-player - use mesh-cue for preset editing");
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
                        app.deck_views[deck].set_stem_macro(stem_idx, index, new_value);
                    }

                    // Send macro value to engine (for any engine-side processing)
                    app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                        deck,
                        stem,
                        macro_index: index,
                        value: new_value,
                    });

                    // Apply modulation to all parameters mapped to this macro
                    apply_macro_modulation(app, index, new_value);
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

        DropMacroOnParam { macro_index, location, effect, param } => {
            use mesh_widgets::knob::ModulationRange;

            // Get base value first (immutable borrow)
            let base_value = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                };
                effect_state
                    .and_then(|e| e.knob_assignments.get(param))
                    .map(|a| a.value)
                    .unwrap_or(0.5)
            };

            // Get effect state based on location (mutable)
            let effect_state = match location {
                EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                    .get_mut(band_idx)
                    .and_then(|b| b.effects.get_mut(effect)),
                EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
            };

            let offset_range = 0.25; // ±25% default range

            // Update UI state - set the mapping on both structures
            if let Some(effect_state) = effect_state {
                // Set on knob_assignments (primary storage for new mappings)
                if let Some(assignment) = effect_state.knob_assignments.get_mut(param) {
                    assignment.macro_mapping = Some(ParamMacroMapping::new(macro_index, offset_range));
                }
                // Also set on param_mappings for legacy compatibility
                if let Some(mapping) = effect_state.param_mappings.get_mut(param) {
                    mapping.macro_index = Some(macro_index);
                    mapping.offset_range = offset_range;
                }
            }

            // Update knob visualization with modulation range
            let key = (location, effect, param);
            if let Some(knob) = app.multiband_editor.effect_knobs.get_mut(&key) {
                let (min, max) = ParamMacroMapping::new(macro_index, offset_range).modulation_bounds(base_value);
                knob.set_modulations(vec![ModulationRange::new(
                    min,
                    max,
                    iced::Color::from_rgb(0.9, 0.6, 0.2), // Orange for modulation
                )]);
            }

            // Update macro's mapping count
            if let Some(macro_state) = app.multiband_editor.macros.get_mut(macro_index) {
                macro_state.mapping_count += 1;
            }

            // Clear drag state
            app.multiband_editor.dragging_macro = None;

            log::info!("Mapped macro {} to {:?} effect {} param {} with ±{:.0}% range", macro_index, location, effect, param, offset_range * 100.0);
            Task::none()
        }

        RemoveParamMapping { location, effect, param } => {
            // Get effect state based on location
            let effect_state = match location {
                EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                    .get_mut(band_idx)
                    .and_then(|b| b.effects.get_mut(effect)),
                EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
            };

            // Get the macro that was mapped and decrement its count
            if let Some(effect_state) = effect_state {
                // Clear from knob_assignments
                if let Some(assignment) = effect_state.knob_assignments.get_mut(param) {
                    if let Some(ref mapping) = assignment.macro_mapping {
                        if let Some(old_macro) = mapping.macro_index {
                            if let Some(macro_state) = app.multiband_editor.macros.get_mut(old_macro) {
                                macro_state.mapping_count = macro_state.mapping_count.saturating_sub(1);
                            }
                        }
                    }
                    assignment.macro_mapping = None;
                }
                // Clear from param_mappings (legacy)
                if let Some(mapping) = effect_state.param_mappings.get_mut(param) {
                    mapping.macro_index = None;
                }
            }

            // Clear knob modulation visualization
            let key = (location, effect, param);
            if let Some(knob) = app.multiband_editor.effect_knobs.get_mut(&key) {
                knob.clear_modulations();
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
                // Look up the actual parameter index from knob_assignments
                // (param is the knob slot 0-7, but the actual param could be different after learning)
                let actual_param_index = {
                    let effect_state = match location {
                        EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                        EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect)),
                        EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                    };
                    effect_state
                        .and_then(|e| e.knob_assignments.get(param))
                        .and_then(|a| a.param_index)
                        .unwrap_or(param) // Fallback to knob slot if no assignment
                };

                // Get macro mapping info before mutable borrow
                let macro_mapping = {
                    let effect_state = match location {
                        EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                        EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect)),
                        EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                    };
                    effect_state
                        .and_then(|e| e.knob_assignments.get(param))
                        .and_then(|a| a.macro_mapping.clone())
                };

                let knob = app.multiband_editor.get_effect_knob(location, effect, param);
                if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                    app.multiband_editor.set_effect_param_value(location, effect, param, new_value);

                    // Calculate the value to send (apply modulation if mapped)
                    let value_to_send = if let Some(ref mapping) = macro_mapping {
                        if let Some(macro_idx) = mapping.macro_index {
                            let macro_value = app.multiband_editor.macro_value(macro_idx);
                            mapping.modulate(new_value, macro_value)
                        } else {
                            new_value
                        }
                    } else {
                        new_value
                    };

                    // Update modulation bounds visualization if mapped
                    if let Some(mapping) = macro_mapping {
                        use mesh_widgets::knob::ModulationRange;
                        let key = (location, effect, param);
                        if let Some(knob) = app.multiband_editor.effect_knobs.get_mut(&key) {
                            let (min, max) = mapping.modulation_bounds(new_value);
                            knob.set_modulations(vec![ModulationRange::new(
                                min,
                                max,
                                iced::Color::from_rgb(0.9, 0.6, 0.2),
                            )]);
                        }
                    }

                    // Send to backend using the actual parameter index (not the knob slot)
                    match location {
                        EffectChainLocation::PreFx => {
                            app.domain.set_pre_fx_param(deck, stem, effect, actual_param_index, value_to_send);
                        }
                        EffectChainLocation::Band(band_idx) => {
                            app.domain.set_band_effect_param(deck, stem, band_idx, effect, actual_param_index, value_to_send);
                        }
                        EffectChainLocation::PostFx => {
                            app.domain.set_post_fx_param(deck, stem, effect, actual_param_index, value_to_send);
                        }
                    }
                }
            }

            // Route to dragging macro knob
            if let Some(index) = app.multiband_editor.dragging_macro_knob {
                if let Some(knob) = app.multiband_editor.macro_knobs.get_mut(index) {
                    if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                        if deck < 4 && stem_idx < 4 && index < 8 {
                            app.deck_views[deck].set_stem_macro(stem_idx, index, new_value);
                        }

                        app.domain.send_command(mesh_core::engine::EngineCommand::SetMultibandMacro {
                            deck,
                            stem,
                            macro_index: index,
                            value: new_value,
                        });

                        // Apply modulation to all mapped parameters
                        apply_macro_modulation(app, index, new_value);
                    }
                }
            }

            Task::none()
        }

        // ─────────────────────────────────────────────────────────────────────
        // CLAP Plugin GUI Learning Mode
        // ─────────────────────────────────────────────────────────────────────
        OpenPluginGui { location, effect } => {
            log::info!("OpenPluginGui: location={:?}, effect={}", location, effect);

            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Get effect info (immutably first for safety checks)
            let (plugin_id, source, effect_instance_id) = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                };
                match effect_state {
                    Some(e) => {
                        let effect_instance_id = match location {
                            EffectChainLocation::PreFx => {
                                crate::domain::MeshDomain::make_pre_fx_effect_id(&e.id, deck, stem)
                            }
                            EffectChainLocation::Band(band_idx) => {
                                crate::domain::MeshDomain::make_band_effect_id(&e.id, deck, stem, band_idx)
                            }
                            EffectChainLocation::PostFx => {
                                crate::domain::MeshDomain::make_post_fx_effect_id(&e.id, deck, stem)
                            }
                        };
                        (e.id.clone(), e.source, effect_instance_id)
                    }
                    None => return Task::none(),
                }
            };

            if source != EffectSourceType::Clap {
                log::warn!("Cannot open plugin GUI for non-CLAP effect: {}", source);
                return Task::none();
            }

            // Get GUI handle from domain and create/show
            if let Some(gui_handle) = app.domain.get_clap_gui_handle(&effect_instance_id) {
                if !gui_handle.supports_gui() {
                    log::warn!("Plugin '{}' does not support GUI", plugin_id);
                    return Task::none();
                }

                // Create and show floating GUI window
                match gui_handle.create_gui(true) {
                    Ok(()) => {
                        if let Err(e) = gui_handle.show_gui() {
                            log::error!("Failed to show plugin GUI: {}", e);
                        } else {
                            log::info!("Opened plugin GUI for '{}'", plugin_id);
                            // Update UI state to track that GUI is open
                            let effect_state = match location {
                                EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                                EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                                    .get_mut(band_idx)
                                    .and_then(|b| b.effects.get_mut(effect)),
                                EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
                            };
                            if let Some(e) = effect_state {
                                e.gui_open = true;
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to create plugin GUI: {}", e);
                    }
                }
            } else {
                log::warn!("No GUI handle found for effect instance '{}'", effect_instance_id);
            }

            Task::none()
        }

        ClosePluginGui { location, effect } => {
            log::info!("ClosePluginGui: location={:?}, effect={}", location, effect);

            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Get effect info and instance ID
            let effect_instance_id = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                };
                effect_state.map(|e| match location {
                    EffectChainLocation::PreFx => {
                        crate::domain::MeshDomain::make_pre_fx_effect_id(&e.id, deck, stem)
                    }
                    EffectChainLocation::Band(band_idx) => {
                        crate::domain::MeshDomain::make_band_effect_id(&e.id, deck, stem, band_idx)
                    }
                    EffectChainLocation::PostFx => {
                        crate::domain::MeshDomain::make_post_fx_effect_id(&e.id, deck, stem)
                    }
                })
            };

            if let Some(effect_instance_id) = effect_instance_id {
                // Get GUI handle and destroy the GUI
                if let Some(gui_handle) = app.domain.get_clap_gui_handle(&effect_instance_id) {
                    gui_handle.destroy_gui();
                    log::info!("Closed plugin GUI");
                }

                // Update UI state
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
                };
                if let Some(e) = effect_state {
                    e.gui_open = false;
                }
            }

            Task::none()
        }

        StartLearning { location, effect, knob } => {
            log::info!(
                "[CLAP_LEARN] StartLearning handler: location={:?}, effect={}, knob={}",
                location, effect, knob
            );

            // Check that this is a CLAP effect and extract plugin ID (avoiding borrow issues)
            let clap_plugin_id = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => {
                        log::info!("[CLAP_LEARN] Looking up PreFx effect at index {}", effect);
                        app.multiband_editor.pre_fx.get(effect)
                    }
                    EffectChainLocation::Band(band_idx) => {
                        log::info!("[CLAP_LEARN] Looking up Band {} effect at index {}", band_idx, effect);
                        app.multiband_editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect))
                    }
                    EffectChainLocation::PostFx => {
                        log::info!("[CLAP_LEARN] Looking up PostFx effect at index {}", effect);
                        app.multiband_editor.post_fx.get(effect)
                    }
                };

                if let Some(e) = effect_state {
                    log::info!("[CLAP_LEARN] Found effect: id='{}', source={:?}", e.id, e.source);
                    if e.source == EffectSourceType::Clap {
                        Some(e.id.clone())
                    } else {
                        log::warn!("[CLAP_LEARN] Learning mode only available for CLAP effects, got {:?}", e.source);
                        None
                    }
                } else {
                    log::warn!("[CLAP_LEARN] No effect found at location={:?}, index={}", location, effect);
                    None
                }
            };

            if let Some(plugin_id) = clap_plugin_id {
                // Construct the full effect instance ID (must match how domain stores GUI handles)
                let deck = app.multiband_editor.deck;
                let stem = Stem::ALL[app.multiband_editor.stem];

                log::info!("[CLAP_LEARN] Got plugin_id='{}', deck={}, stem={:?}", plugin_id, deck, stem);

                use crate::domain::MeshDomain;
                let effect_instance_id = match location {
                    EffectChainLocation::PreFx => MeshDomain::make_pre_fx_effect_id(&plugin_id, deck, stem),
                    EffectChainLocation::Band(band_idx) => MeshDomain::make_band_effect_id(&plugin_id, deck, stem, band_idx),
                    EffectChainLocation::PostFx => MeshDomain::make_post_fx_effect_id(&plugin_id, deck, stem),
                };

                log::info!("[CLAP_LEARN] Constructed effect_instance_id='{}'", effect_instance_id);
                log::info!("[CLAP_LEARN] Available GUI handles: {:?}", app.domain.list_clap_gui_handles());

                // Get the GUI handle and start learning mode
                if let Some(gui_handle) = app.domain.get_clap_gui_handle(&effect_instance_id) {
                    log::info!("[CLAP_LEARN] GUI handle FOUND for effect_instance_id='{}'", effect_instance_id);
                    log::info!("[CLAP_LEARN] Calling gui_handle.start_learning_mode()...");

                    // Start learning mode on the plugin wrapper - this snapshots all param values
                    // so we can detect changes by comparing current values to the snapshot
                    gui_handle.start_learning_mode();

                    log::info!("[CLAP_LEARN] gui_handle.start_learning_mode() returned");
                } else {
                    log::warn!("[CLAP_LEARN] NO GUI handle for effect_instance_id='{}'", effect_instance_id);
                }

                // Start learning mode in the UI state
                app.multiband_editor.start_learning(location, effect, knob);

                // Start learning in PluginGuiManager (tracks effect_id -> knob mapping)
                app.plugin_gui_manager.start_learning(effect_instance_id, knob);
            }

            Task::none()
        }

        CancelLearning => {
            log::info!("CancelLearning");

            // Stop learning mode on the plugin wrapper (if we have a learning target)
            if let Some(target) = app.plugin_gui_manager.learning_target() {
                if let Some(gui_handle) = app.domain.get_clap_gui_handle(&target.effect_id) {
                    gui_handle.stop_learning_mode();
                }
            }

            app.multiband_editor.cancel_learning();
            app.plugin_gui_manager.cancel_learning();
            Task::none()
        }

        ParamLearned { location, effect, knob, param_id, param_name } => {
            log::info!(
                "ParamLearned: location={:?}, effect={}, knob={}, param_id={}, param_name={}",
                location, effect, knob, param_id, param_name
            );

            // Clear learning mode in UI
            app.multiband_editor.cancel_learning();

            let deck = app.multiband_editor.deck;
            let stem = Stem::ALL[app.multiband_editor.stem];

            // Get the effect state (immutable first to get plugin ID)
            let plugin_id = {
                let effect_state = match location {
                    EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => app.multiband_editor.post_fx.get(effect),
                };
                effect_state.map(|e| e.id.clone())
            };

            // Look up the correct param_index and get current value using the GUI handle
            let (param_index, current_normalized_value) = if let Some(ref plugin_id) = plugin_id {
                let effect_instance_id = match location {
                    EffectChainLocation::PreFx => {
                        crate::domain::MeshDomain::make_pre_fx_effect_id(plugin_id, deck, stem)
                    }
                    EffectChainLocation::Band(band_idx) => {
                        crate::domain::MeshDomain::make_band_effect_id(plugin_id, deck, stem, band_idx)
                    }
                    EffectChainLocation::PostFx => {
                        crate::domain::MeshDomain::make_post_fx_effect_id(plugin_id, deck, stem)
                    }
                };

                // Find index and current value via GUI handle
                if let Some(gui_handle) = app.domain.get_clap_gui_handle(&effect_instance_id) {
                    let idx = gui_handle.param_ids.iter().position(|&id| id == param_id);

                    // Get the current parameter value and normalize it
                    let normalized = if let (Some(value), Some((min, max, _default))) = (
                        gui_handle.get_param_value(param_id),
                        gui_handle.get_param_info(param_id),
                    ) {
                        // Normalize value from [min, max] to [0, 1]
                        let range = max - min;
                        if range > 0.0 {
                            ((value - min) / range) as f32
                        } else {
                            0.5
                        }
                    } else {
                        0.5 // Default if we can't get the value
                    };

                    (idx, normalized)
                } else {
                    (None, 0.5)
                }
            } else {
                (None, 0.5)
            };

            // Get effect state mutably for update
            let effect_state = match location {
                EffectChainLocation::PreFx => app.multiband_editor.pre_fx.get_mut(effect),
                EffectChainLocation::Band(band_idx) => app.multiband_editor.bands
                    .get_mut(band_idx)
                    .and_then(|b| b.effects.get_mut(effect)),
                EffectChainLocation::PostFx => app.multiband_editor.post_fx.get_mut(effect),
            };

            if let Some(effect_state) = effect_state {
                // Use the param_index from GUI handle, or fall back to name search
                let param_index = param_index.unwrap_or_else(|| {
                    log::warn!("Could not find param_id {} in GUI handle, falling back to name search", param_id);
                    effect_state.available_params
                        .iter()
                        .position(|p| p.name == param_name)
                        .unwrap_or(knob) // Last resort: use knob slot
                });

                // Assign to the knob with the current value from the plugin
                if let Some(assignment) = effect_state.knob_assignments.get_mut(knob) {
                    assignment.param_index = Some(param_index);
                    assignment.value = current_normalized_value;
                    log::info!(
                        "Assigned param '{}' (index {}) to knob {} with value {:.2}",
                        param_name, param_index, knob, current_normalized_value
                    );
                }

                // Update param_names for UI label display
                if knob < effect_state.param_names.len() {
                    effect_state.param_names[knob] = param_name.clone();
                }

                // Also update param_values for the knob widget
                if knob < effect_state.param_values.len() {
                    effect_state.param_values[knob] = current_normalized_value;
                }
            }

            // Sync the knob widget value as well
            app.multiband_editor.set_effect_param_value(location, effect, knob, current_normalized_value);

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
            let value = app.deck_views[deck].stem_macro_value(stem_idx, macro_idx);
            app.multiband_editor.set_macro_value(macro_idx, value);
        }
    }
}

/// Handle plugin GUI tick - poll for parameter changes during learning mode
///
/// Called periodically from the subscription when learning mode is active.
/// Polls all GUI handles for parameter changes and emits ParamLearned when detected.
pub fn handle_plugin_gui_tick(app: &mut MeshApp) -> Task<Message> {
    // Only process if we're in learning mode
    if !app.plugin_gui_manager.is_learning() {
        return Task::none();
    }

    // Get the learning target info
    let target = match app.plugin_gui_manager.learning_target() {
        Some(t) => t.clone(),
        None => {
            log::warn!("[CLAP_LEARN] is_learning() true but no learning_target");
            return Task::none();
        }
    };

    log::trace!("[CLAP_LEARN] Tick: polling effect_id={}", target.effect_id);

    // Look up the GUI handle for this effect
    match app.domain.get_clap_gui_handle(&target.effect_id) {
        Some(gui_handle) => {
            log::trace!("[CLAP_LEARN] Found GUI handle, calling poll_param_changes()");

            // Check for parameter changes
            let changes = gui_handle.poll_param_changes();

            log::trace!("[CLAP_LEARN] poll_param_changes returned {} changes", changes.len());

            if !changes.is_empty() {
                // Use the first change
                let change = &changes[0];
                let param_id = change.param_id;
                let param_name = gui_handle
                    .param_name_for_id(param_id)
                    .unwrap_or_else(|| format!("Param {}", param_id));

                log::info!(
                    "[CLAP_LEARN] Learning detected param change: effect={}, param_id={}, param_name={}",
                    target.effect_id,
                    param_id,
                    param_name
                );

                // Get the location and effect index from multiband_editor's learning state
                if let Some((location, effect_idx, knob_idx)) = app.multiband_editor.learning_target() {
                    // Stop learning mode on the plugin wrapper (clears cached param values)
                    gui_handle.stop_learning_mode();

                    // Clear learning state in both places
                    app.plugin_gui_manager.cancel_learning();
                    app.multiband_editor.cancel_learning();

                    // Emit the ParamLearned message to complete the assignment
                    return Task::done(Message::Multiband(
                        MultibandEditorMessage::ParamLearned {
                            location,
                            effect: effect_idx,
                            knob: knob_idx,
                            param_id,
                            param_name,
                        }
                    ));
                }
            }
        }
        None => {
            log::warn!("[CLAP_LEARN] No GUI handle found for effect_id={}", target.effect_id);
        }
    }

    Task::none()
}

/// Apply macro modulation to all parameters mapped to this macro
///
/// When a macro value changes, this function finds all parameters that are
/// mapped to that macro and sends updated (modulated) values to the backend.
///
/// The modulation formula is:
/// actual_value = base_value + (macro_value * 2 - 1) * offset_range
///
/// This creates bipolar modulation where macro=50% is neutral.
fn apply_macro_modulation(app: &mut MeshApp, macro_index: usize, macro_value: f32) {
    use mesh_core::types::Stem;

    let deck = app.multiband_editor.deck;
    let stem = Stem::ALL[app.multiband_editor.stem];

    // Helper to process an effect's mappings
    let process_effect = |effect: &multiband::EffectUiState,
                          effect_idx: usize,
                          location: EffectChainLocation,
                          domain: &mut crate::domain::MeshDomain| {
        for (knob_idx, assignment) in effect.knob_assignments.iter().enumerate() {
            // Check if this knob is mapped to our macro
            if let Some(ref mapping) = assignment.macro_mapping {
                if mapping.macro_index == Some(macro_index) {
                    // Get the actual parameter index for this knob
                    if let Some(param_index) = assignment.param_index {
                        // Calculate modulated value
                        let base_value = assignment.value;
                        let modulated_value = mapping.modulate(base_value, macro_value);

                        // Send to backend
                        match location {
                            EffectChainLocation::PreFx => {
                                domain.set_pre_fx_param(deck, stem, effect_idx, param_index, modulated_value);
                            }
                            EffectChainLocation::Band(band_idx) => {
                                domain.set_band_effect_param(deck, stem, band_idx, effect_idx, param_index, modulated_value);
                            }
                            EffectChainLocation::PostFx => {
                                domain.set_post_fx_param(deck, stem, effect_idx, param_index, modulated_value);
                            }
                        }

                        log::trace!(
                            "Macro {} modulation: {:?} effect {} knob {} -> param {}: base={:.2} + offset={:.2} = {:.2}",
                            macro_index,
                            location,
                            effect_idx,
                            knob_idx,
                            param_index,
                            base_value,
                            (macro_value * 2.0 - 1.0) * mapping.offset_range,
                            modulated_value
                        );
                    }
                }
            }

            // Also check legacy param_mappings for backwards compatibility
            if let Some(mapping) = effect.param_mappings.get(knob_idx) {
                if mapping.macro_index == Some(macro_index) {
                    if let Some(param_index) = assignment.param_index {
                        let base_value = assignment.value;
                        let modulated_value = mapping.modulate(base_value, macro_value);

                        match location {
                            EffectChainLocation::PreFx => {
                                domain.set_pre_fx_param(deck, stem, effect_idx, param_index, modulated_value);
                            }
                            EffectChainLocation::Band(band_idx) => {
                                domain.set_band_effect_param(deck, stem, band_idx, effect_idx, param_index, modulated_value);
                            }
                            EffectChainLocation::PostFx => {
                                domain.set_post_fx_param(deck, stem, effect_idx, param_index, modulated_value);
                            }
                        }
                    }
                }
            }
        }
    };

    // Process Pre-FX effects
    for (effect_idx, effect) in app.multiband_editor.pre_fx.iter().enumerate() {
        process_effect(effect, effect_idx, EffectChainLocation::PreFx, &mut app.domain);
    }

    // Process Band effects
    for (band_idx, band) in app.multiband_editor.bands.iter().enumerate() {
        for (effect_idx, effect) in band.effects.iter().enumerate() {
            process_effect(effect, effect_idx, EffectChainLocation::Band(band_idx), &mut app.domain);
        }
    }

    // Process Post-FX effects
    for (effect_idx, effect) in app.multiband_editor.post_fx.iter().enumerate() {
        process_effect(effect, effect_idx, EffectChainLocation::PostFx, &mut app.domain);
    }
}
