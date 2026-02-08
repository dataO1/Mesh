//! Effects editor message handlers
//!
//! Handles multiband effects editing, preset save/load, and audio preview routing.

use iced::Task;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{
    EffectChainLocation, EffectSourceType, MultibandPresetConfig, load_preset, save_preset,
};
use mesh_widgets::{MultibandEditorMessage, DEFAULT_SENSITIVITY};

use crate::ui::app::MeshCueApp;
use crate::ui::message::Message;

impl MeshCueApp {
    /// Handle OpenEffectsEditor message
    pub fn handle_open_effects_editor(&mut self) -> Task<Message> {
        log::info!("[FX] Opening effects editor");
        self.effects_editor.open();
        log::info!("[FX] effects_editor.is_open = {}", self.effects_editor.is_open);
        Task::none()
    }

    /// Handle CloseEffectsEditor message
    pub fn handle_close_effects_editor(&mut self) -> Task<Message> {
        log::info!("[FX] Closing effects editor");
        self.effects_editor.close();
        Task::none()
    }

    /// Handle a multiband editor message
    pub fn handle_effects_editor(&mut self, msg: MultibandEditorMessage) -> Task<Message> {
        use MultibandEditorMessage::*;

        match msg {
            // Modal control
            Open { .. } => {
                // Already handled by OpenEffectsEditor
            }
            Close => {
                self.effects_editor.close();
            }

            // Preset management
            OpenPresetBrowser => {
                // Refresh presets list
                let presets = mesh_widgets::multiband::list_presets(&self.domain.collection_root());
                self.effects_editor.editor.available_presets = presets;
                self.effects_editor.editor.preset_browser_open = true;
            }
            ClosePresetBrowser => {
                self.effects_editor.editor.preset_browser_open = false;
            }
            LoadPreset(name) => {
                return self.handle_load_preset(name);
            }
            SavePreset => {
                // Use current name or prompt
                let name = if self.effects_editor.preset_name_input.is_empty() {
                    "Untitled".to_string()
                } else {
                    self.effects_editor.preset_name_input.clone()
                };
                return self.handle_effects_editor_save(name);
            }
            DeletePreset(name) => {
                return self.handle_delete_preset(name);
            }
            RefreshPresets => {
                let presets = mesh_widgets::multiband::list_presets(&self.domain.collection_root());
                self.effects_editor.editor.available_presets = presets;
            }
            SetAvailablePresets(presets) => {
                self.effects_editor.editor.available_presets = presets;
            }

            // Save dialog
            OpenSaveDialog => {
                self.effects_editor.open_save_dialog();
            }
            CloseSaveDialog => {
                self.effects_editor.close_save_dialog();
            }
            SetPresetNameInput(name) => {
                self.effects_editor.preset_name_input = name;
            }

            // Crossover control - forward to editor state and audio
            StartDragCrossover(idx) => {
                self.effects_editor.editor.dragging_crossover = Some(idx);
            }
            DragCrossover(freq) => {
                if let Some(idx) = self.effects_editor.editor.dragging_crossover {
                    self.effects_editor.editor.set_crossover_freq(idx, freq);
                    // Apply to audio preview if enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_crossover(stem, idx, freq);
                    }
                }
            }
            EndDragCrossover => {
                self.effects_editor.editor.dragging_crossover = None;
            }

            // Band management
            AddBand => {
                self.effects_editor.editor.add_band();
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.add_multiband_band(stem);
                }
            }
            RemoveBand(idx) => {
                self.effects_editor.editor.remove_band(idx);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.remove_multiband_band(stem, idx);
                }
            }
            SetBandMute { band, muted } => {
                if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    b.muted = muted;
                }
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_band_mute(stem, band, muted);
                }
            }
            SetBandSolo { band, soloed } => {
                if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    b.soloed = soloed;
                }
                self.effects_editor.editor.any_soloed = self.effects_editor.editor.bands.iter().any(|b| b.soloed);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_band_solo(stem, band, soloed);
                }
            }
            SetBandGain { band, gain } => {
                if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    b.gain = gain;
                }
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_band_gain(stem, band, gain);
                }
            }

            // Effect picker - open the picker for the appropriate target
            OpenEffectPicker(band_idx) => {
                self.effect_picker.open_for_band(band_idx);
                log::info!("Opening effect picker for band {}", band_idx);
            }
            OpenPreFxEffectPicker => {
                self.effect_picker.open_pre_fx();
                log::info!("Opening effect picker for pre-fx chain");
            }
            OpenPostFxEffectPicker => {
                self.effect_picker.open_post_fx();
                log::info!("Opening effect picker for post-fx chain");
            }

            // ─────────────────────────────────────────────────────────────────────
            // Pre-FX effect management
            // ─────────────────────────────────────────────────────────────────────
            RemovePreFxEffect(idx) => {
                if idx < self.effects_editor.editor.pre_fx.len() {
                    self.effects_editor.editor.pre_fx.remove(idx);
                    log::info!("Removed pre-fx effect at index {}", idx);
                }
            }
            TogglePreFxBypass(idx) => {
                if let Some(effect) = self.effects_editor.editor.pre_fx.get_mut(idx) {
                    effect.bypassed = !effect.bypassed;
                    log::debug!("Toggled pre-fx bypass: {} = {}", idx, effect.bypassed);

                    // Send to audio engine if preview enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_pre_fx_bypass(stem, idx, effect.bypassed);
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Band effect management
            // ─────────────────────────────────────────────────────────────────────
            RemoveEffect { band, effect } => {
                if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    if effect < b.effects.len() {
                        b.effects.remove(effect);
                        log::info!("Removed effect {} from band {}", effect, band);
                    }
                }
            }
            ToggleEffectBypass { band, effect } => {
                let bypassed = if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    if let Some(e) = b.effects.get_mut(effect) {
                        e.bypassed = !e.bypassed;
                        log::debug!("Toggled band {} effect {} bypass: {}", band, effect, e.bypassed);
                        Some(e.bypassed)
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Send to audio engine if preview enabled
                if let Some(bypassed) = bypassed {
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_effect_bypass(stem, band, effect, bypassed);
                    }
                }
            }
            SelectEffect { location, effect } => {
                self.effects_editor.editor.selected_effect = Some((location, effect));
            }

            // ─────────────────────────────────────────────────────────────────────
            // Post-FX effect management
            // ─────────────────────────────────────────────────────────────────────
            RemovePostFxEffect(idx) => {
                if idx < self.effects_editor.editor.post_fx.len() {
                    self.effects_editor.editor.post_fx.remove(idx);
                    log::info!("Removed post-fx effect at index {}", idx);
                }
            }
            TogglePostFxBypass(idx) => {
                if let Some(effect) = self.effects_editor.editor.post_fx.get_mut(idx) {
                    effect.bypassed = !effect.bypassed;
                    log::debug!("Toggled post-fx bypass: {} = {}", idx, effect.bypassed);

                    // Send to audio engine if preview enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_post_fx_bypass(stem, idx, effect.bypassed);
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Knob events
            // ─────────────────────────────────────────────────────────────────────
            MacroKnob { index, event } => {
                use mesh_widgets::knob::KnobEvent;

                // Track drag state for global mouse capture
                match &event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_macro_knob = Some(index);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_macro_knob = None;
                        // Important: Must also clear the knob's internal drag state here,
                        // not just in GlobalMouseReleased, because both events may fire
                        // when releasing over a knob and this handler might run first.
                        if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                            knob.handle_event(event.clone(), DEFAULT_SENSITIVITY);
                        }
                        return Task::none(); // Already handled, don't process again below
                    }
                    KnobEvent::Moved(_) => {}
                }

                if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                    if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
                        self.effects_editor.editor.set_macro_value(index, new_value);

                        // Apply modulation to all parameters mapped to this macro
                        self.apply_macro_modulation(index, new_value);
                    }
                }
            }
            EffectKnob { location, effect, param, event } => {
                use mesh_widgets::knob::KnobEvent;

                // Track drag state for global mouse capture
                match &event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_effect_knob = Some((location, effect, param));
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_effect_knob = None;
                        // Important: Must also clear the knob's internal drag state here,
                        // not just in GlobalMouseReleased, because both events may fire
                        // when releasing over a knob and this handler might run first.
                        let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                        knob.handle_event(event.clone(), DEFAULT_SENSITIVITY);
                        return Task::none(); // Already handled, don't process again below
                    }
                    KnobEvent::Moved(_) => {}
                }

                // Look up the actual parameter index from knob_assignments
                // (param is the knob slot 0-7, but actual param could be different after learning)
                let (actual_param_index, macro_mapping) = {
                    let effect_state = match location {
                        EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                        EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect)),
                        EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                    };
                    let assignment = effect_state.and_then(|e| e.knob_assignments.get(param));
                    (
                        assignment.and_then(|a| a.param_index).unwrap_or(param),
                        assignment.and_then(|a| a.macro_mapping.clone()),
                    )
                };

                let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                if let Some(new_value) = knob.handle_event(event, DEFAULT_SENSITIVITY) {
                    // Update UI state (base value)
                    self.effects_editor.editor.set_effect_param_value(location, effect, param, new_value);

                    // Send to audio engine if preview enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;

                        // Calculate value to send (apply modulation if mapped)
                        let value_to_send = if let Some(ref mapping) = macro_mapping {
                            if let Some(macro_idx) = mapping.macro_index {
                                let macro_value = self.effects_editor.editor.macro_value(macro_idx);
                                mapping.modulate(new_value, macro_value)
                            } else {
                                new_value
                            }
                        } else {
                            new_value
                        };

                        // Send to audio engine using actual param index
                        match location {
                            EffectChainLocation::PreFx => {
                                self.audio.set_multiband_pre_fx_param(stem, effect, actual_param_index, value_to_send);
                            }
                            EffectChainLocation::Band(band_idx) => {
                                self.audio.set_multiband_effect_param(stem, band_idx, effect, actual_param_index, value_to_send);
                            }
                            EffectChainLocation::PostFx => {
                                self.audio.set_multiband_post_fx_param(stem, effect, actual_param_index, value_to_send);
                            }
                        }
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Macro mapping
            // ─────────────────────────────────────────────────────────────────────
            RenameMacro { index, name } => {
                self.effects_editor.editor.set_macro_name(index, name);
            }
            StartDragMacro(index) => {
                self.effects_editor.editor.dragging_macro = Some(index);
            }
            EndDragMacro => {
                self.effects_editor.editor.dragging_macro = None;
            }
            DropMacroOnParam { macro_index, location, effect, param } => {
                use mesh_widgets::multiband::ParamMacroMapping;

                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    if let Some(assignment) = effect_state.knob_assignments.get_mut(param) {
                        assignment.macro_mapping = Some(ParamMacroMapping::new(macro_index, 0.25));
                    }
                    if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(macro_index) {
                        macro_state.mapping_count += 1;
                    }
                }
                self.effects_editor.editor.dragging_macro = None;
                log::info!("Mapped macro {} to {:?} effect {} param {}", macro_index, location, effect, param);
            }
            RemoveParamMapping { location, effect, param } => {
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    if let Some(assignment) = effect_state.knob_assignments.get_mut(param) {
                        if let Some(ref mapping) = assignment.macro_mapping {
                            if let Some(old_macro) = mapping.macro_index {
                                if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(old_macro) {
                                    macro_state.mapping_count = macro_state.mapping_count.saturating_sub(1);
                                }
                            }
                        }
                        assignment.macro_mapping = None;
                    }
                }
            }
            OpenMacroMapper(_) | AddMacroMapping { .. } | ClearMacroMappings(_) => {
                // Not implemented in mesh-cue (direct drag-drop is used instead)
            }

            // ─────────────────────────────────────────────────────────────────────
            // Parameter picker
            // ─────────────────────────────────────────────────────────────────────
            OpenParamPicker { location, effect, knob } => {
                self.effects_editor.editor.param_picker_open = Some((location, effect, knob));
                self.effects_editor.editor.param_picker_search = String::new();
            }
            CloseParamPicker => {
                self.effects_editor.editor.param_picker_open = None;
            }
            AssignParam { location, effect, knob, param_index } => {
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    if let Some(assignment) = effect_state.knob_assignments.get_mut(knob) {
                        assignment.param_index = param_index;
                        if let Some(idx) = param_index {
                            if let Some(p) = effect_state.available_params.get(idx) {
                                assignment.value = p.default;
                            }
                        }
                    }
                }
                self.effects_editor.editor.param_picker_open = None;
            }
            SetParamPickerFilter(filter) => {
                self.effects_editor.editor.param_picker_search = filter;
            }

            // ─────────────────────────────────────────────────────────────────────
            // CLAP Plugin GUI
            // ─────────────────────────────────────────────────────────────────────
            OpenPluginGui { location, effect } => {
                log::info!("OpenPluginGui: location={:?}, effect={}", location, effect);

                // Get effect info
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                };

                let (plugin_id, source, effect_instance_id) = match effect_state {
                    Some(e) => {
                        // Generate effect instance ID for mesh-cue (simplified, no deck/stem)
                        let instance_id = match location {
                            EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", e.id, effect),
                            EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", e.id, band_idx, effect),
                            EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", e.id, effect),
                        };
                        (e.id.clone(), e.source, instance_id)
                    }
                    None => return Task::none(),
                };

                if source != EffectSourceType::Clap {
                    log::warn!("Cannot open plugin GUI for non-CLAP effect: {:?}", source);
                    return Task::none();
                }

                // Get GUI handle from domain and create/show
                if let Some(gui_handle) = self.domain.get_clap_gui_handle(&effect_instance_id) {
                    if !gui_handle.supports_gui() {
                        log::warn!("Plugin '{}' does not support GUI", plugin_id);
                        self.effects_editor.set_status(format!("Plugin '{}' has no GUI", plugin_id));
                        return Task::none();
                    }

                    // Create and show floating GUI window
                    match gui_handle.create_gui(true) {
                        Ok(()) => {
                            if let Err(e) = gui_handle.show_gui() {
                                log::error!("Failed to show plugin GUI: {}", e);
                                self.effects_editor.set_status(format!("Failed to show GUI: {}", e));
                            } else {
                                log::info!("Opened plugin GUI for '{}'", plugin_id);
                                // Update UI state to track that GUI is open
                                let effect_state = match location {
                                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                        .get_mut(band_idx)
                                        .and_then(|b| b.effects.get_mut(effect)),
                                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                                };
                                if let Some(e) = effect_state {
                                    e.gui_open = true;
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to create plugin GUI: {}", e);
                            self.effects_editor.set_status(format!("Failed to create GUI: {}", e));
                        }
                    }
                } else {
                    log::warn!("No GUI handle found for effect instance '{}' (was effect created with GUI support?)", effect_instance_id);
                    self.effects_editor.set_status("Plugin GUI not available".to_string());
                }
            }

            ClosePluginGui { location, effect } => {
                log::info!("ClosePluginGui: location={:?}, effect={}", location, effect);

                // Get effect info to build instance ID
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                };

                if let Some(e) = effect_state {
                    let effect_instance_id = match location {
                        EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", e.id, effect),
                        EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", e.id, band_idx, effect),
                        EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", e.id, effect),
                    };

                    // Get GUI handle and destroy the GUI
                    if let Some(gui_handle) = self.domain.get_clap_gui_handle(&effect_instance_id) {
                        gui_handle.destroy_gui();
                        log::info!("Closed plugin GUI");
                    }

                    // Update UI state
                    let effect_state = match location {
                        EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                        EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                            .get_mut(band_idx)
                            .and_then(|b| b.effects.get_mut(effect)),
                        EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                    };
                    if let Some(e) = effect_state {
                        e.gui_open = false;
                    }
                }
            }

            StartLearning { location, effect, knob } => {
                log::info!(
                    "[CLAP_LEARN] StartLearning: location={:?}, effect={}, knob={}",
                    location, effect, knob
                );

                // Check that this is a CLAP effect and get its instance ID
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                };

                let effect_instance_id = match effect_state {
                    Some(e) if e.source == EffectSourceType::Clap => {
                        // Generate mesh-cue effect instance ID
                        match location {
                            EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", e.id, effect),
                            EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", e.id, band_idx, effect),
                            EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", e.id, effect),
                        }
                    }
                    Some(e) => {
                        log::warn!("[CLAP_LEARN] Learning only available for CLAP effects, got {:?}", e.source);
                        return Task::none();
                    }
                    None => {
                        log::warn!("[CLAP_LEARN] Effect not found at location={:?}, index={}", location, effect);
                        return Task::none();
                    }
                };

                // Get GUI handle and start learning mode
                if let Some(gui_handle) = self.domain.get_clap_gui_handle(&effect_instance_id) {
                    log::info!("[CLAP_LEARN] Starting learning mode on GUI handle");
                    gui_handle.start_learning_mode();
                } else {
                    log::warn!("[CLAP_LEARN] No GUI handle for '{}'", effect_instance_id);
                    self.effects_editor.set_status("Open plugin GUI first to learn parameters".to_string());
                    return Task::none();
                }

                // Start learning mode in both UI state and plugin GUI manager
                self.effects_editor.editor.start_learning(location, effect, knob);
                self.plugin_gui_manager.start_learning(effect_instance_id, knob);
            }

            CancelLearning => {
                log::info!("[CLAP_LEARN] CancelLearning");

                // Stop learning mode on the plugin if we have a learning target
                if let Some(target) = self.plugin_gui_manager.learning_target() {
                    if let Some(gui_handle) = self.domain.get_clap_gui_handle(&target.effect_instance_id) {
                        gui_handle.stop_learning_mode();
                    }
                }

                self.effects_editor.editor.cancel_learning();
                self.plugin_gui_manager.cancel_learning();
            }

            ParamLearned { location, effect, knob, param_id, param_name } => {
                log::info!(
                    "[CLAP_LEARN] ParamLearned: location={:?}, effect={}, knob={}, param_id={}, param_name={}",
                    location, effect, knob, param_id, param_name
                );

                // Clear learning mode in UI
                self.effects_editor.editor.cancel_learning();
                self.plugin_gui_manager.cancel_learning();

                // Get effect instance ID and lookup param info
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get(band_idx)
                        .and_then(|b| b.effects.get(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                };

                let effect_instance_id = effect_state.map(|e| match location {
                    EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", e.id, effect),
                    EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", e.id, band_idx, effect),
                    EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", e.id, effect),
                });

                // Look up param index and current value from GUI handle
                let (param_index, current_value) = if let Some(ref id) = effect_instance_id {
                    if let Some(gui_handle) = self.domain.get_clap_gui_handle(id) {
                        let idx = gui_handle.param_ids.iter().position(|&pid| pid == param_id);

                        // Get current normalized value
                        let normalized = if let (Some(value), Some((min, max, _default))) = (
                            gui_handle.get_param_value(param_id),
                            gui_handle.get_param_info(param_id),
                        ) {
                            let range = max - min;
                            if range > 0.0 {
                                ((value - min) / range) as f32
                            } else {
                                0.5
                            }
                        } else {
                            0.5
                        };

                        // Stop learning mode on the GUI handle
                        gui_handle.stop_learning_mode();

                        (idx, normalized)
                    } else {
                        (None, 0.5)
                    }
                } else {
                    (None, 0.5)
                };

                // Update effect state with learned param
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    // Use param_index from GUI or fall back to name search
                    let param_index = param_index.unwrap_or_else(|| {
                        effect_state.available_params
                            .iter()
                            .position(|p| p.name == param_name)
                            .unwrap_or(knob)
                    });

                    // Assign to the knob
                    if let Some(assignment) = effect_state.knob_assignments.get_mut(knob) {
                        assignment.param_index = Some(param_index);
                        assignment.value = current_value;
                        log::info!(
                            "[CLAP_LEARN] Assigned '{}' (index {}) to knob {} with value {:.2}",
                            param_name, param_index, knob, current_value
                        );
                    }

                    // Update param_names for UI display
                    if knob < effect_state.param_names.len() {
                        effect_state.param_names[knob] = param_name.clone();
                    }
                    if knob < effect_state.param_values.len() {
                        effect_state.param_values[knob] = current_value;
                    }
                }

                // Sync the knob widget value
                self.effects_editor.editor.set_effect_param_value(location, effect, knob, current_value);
                self.effects_editor.set_status(format!("Learned: {}", param_name));
            }

            // ─────────────────────────────────────────────────────────────────────
            // Global mouse events
            // ─────────────────────────────────────────────────────────────────────
            GlobalMouseMoved(position) => {
                use mesh_widgets::knob::KnobEvent;
                // Route to dragging macro knob
                if let Some(index) = self.effects_editor.editor.dragging_macro_knob {
                    if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                        if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                            self.effects_editor.editor.set_macro_value(index, new_value);
                            // Apply modulation to all parameters mapped to this macro
                            self.apply_macro_modulation(index, new_value);
                        }
                    }
                }
                // Route to dragging effect knob
                if let Some((location, effect, param)) = self.effects_editor.editor.dragging_effect_knob {
                    // Look up actual param index and macro mapping
                    let (actual_param_index, macro_mapping) = {
                        let effect_state = match location {
                            EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                            EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                .get(band_idx)
                                .and_then(|b| b.effects.get(effect)),
                            EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                        };
                        let assignment = effect_state.and_then(|e| e.knob_assignments.get(param));
                        (
                            assignment.and_then(|a| a.param_index).unwrap_or(param),
                            assignment.and_then(|a| a.macro_mapping.clone()),
                        )
                    };

                    let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                    if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                        self.effects_editor.editor.set_effect_param_value(location, effect, param, new_value);

                        // Send to audio engine if preview enabled
                        if self.effects_editor.audio_preview_enabled {
                            let stem = self.effects_editor.preview_stem;

                            // Calculate value to send (apply modulation if mapped)
                            let value_to_send = if let Some(ref mapping) = macro_mapping {
                                if let Some(macro_idx) = mapping.macro_index {
                                    let macro_value = self.effects_editor.editor.macro_value(macro_idx);
                                    mapping.modulate(new_value, macro_value)
                                } else {
                                    new_value
                                }
                            } else {
                                new_value
                            };

                            // Send to audio engine using actual param index
                            match location {
                                EffectChainLocation::PreFx => {
                                    self.audio.set_multiband_pre_fx_param(stem, effect, actual_param_index, value_to_send);
                                }
                                EffectChainLocation::Band(band_idx) => {
                                    self.audio.set_multiband_effect_param(stem, band_idx, effect, actual_param_index, value_to_send);
                                }
                                EffectChainLocation::PostFx => {
                                    self.audio.set_multiband_post_fx_param(stem, effect, actual_param_index, value_to_send);
                                }
                            }
                        }
                    }
                }
            }
            GlobalMouseReleased => {
                use mesh_widgets::knob::KnobEvent;
                if let Some((location, effect, param)) = self.effects_editor.editor.dragging_effect_knob.take() {
                    let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                    knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                }
                if let Some(index) = self.effects_editor.editor.dragging_macro_knob.take() {
                    if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                        knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Effect selected (from legacy picker - now handled by EffectPicker)
            // ─────────────────────────────────────────────────────────────────────
            PreFxEffectSelected { .. } | EffectSelected { .. } | PostFxEffectSelected { .. } => {
                // Handled by effect_picker handler instead
            }
        }

        Task::none()
    }

    /// Create a new empty preset
    ///
    /// Resets the editor to a clean state and syncs to audio if preview is enabled.
    pub fn handle_effects_editor_new_preset(&mut self) -> Task<Message> {
        log::info!("Creating new preset");

        // Reset to clean state
        self.effects_editor.new_preset();

        // If preview is enabled, reset the audio state too
        if self.effects_editor.audio_preview_enabled {
            let stem = self.effects_editor.preview_stem;
            self.audio.reset_multiband(stem);
            log::info!("Reset audio for new preset on stem {:?}", stem);
        }

        self.effects_editor.set_status("New preset created");
        Task::none()
    }

    /// Load a preset into the editor
    ///
    /// This does the full load including:
    /// 1. Load preset config from YAML
    /// 2. Apply to editor UI state
    /// 3. Instantiate CLAP plugins with GUI handles
    /// 4. Ensure knob state exists for all effects
    /// 5. Sync to audio if preview is enabled
    fn handle_load_preset(&mut self, name: String) -> Task<Message> {
        match load_preset(&self.domain.collection_root(), &name) {
            Ok(config) => {
                log::info!("Loading preset '{}' with {} pre-fx, {} bands, {} post-fx",
                    name, config.pre_fx.len(), config.bands.len(), config.post_fx.len());

                // Apply to UI state (sets up EffectUiState objects)
                config.apply_to_editor_state(&mut self.effects_editor.editor);

                // Instantiate CLAP plugins with GUI handles
                self.instantiate_preset_effects();

                // Ensure knob state exists for all effects
                mesh_widgets::multiband::ensure_effect_knobs_exist(&mut self.effects_editor.editor);

                // If preview is enabled, sync the loaded state to audio
                if self.effects_editor.audio_preview_enabled {
                    self.sync_editor_to_audio();
                }

                self.effects_editor.load_preset(name.clone());
                self.effects_editor.set_status(format!("Loaded preset '{}'", name));
                self.effects_editor.editor.preset_browser_open = false;

                log::info!("Preset '{}' loaded successfully", name);
            }
            Err(e) => {
                log::error!("Failed to load preset '{}': {}", name, e);
                self.effects_editor.set_status(format!("Failed to load: {}", e));
            }
        }
        Task::none()
    }

    /// Instantiate CLAP plugins for all effects in the editor
    ///
    /// Creates GUI handles for CLAP effects so they can be opened.
    /// Called after loading a preset to ensure plugins are ready.
    fn instantiate_preset_effects(&mut self) {
        // Collect effect info to avoid borrow conflicts
        let pre_fx_effects: Vec<(usize, String, EffectSourceType)> = self.effects_editor.editor.pre_fx
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.id.clone(), e.source.clone()))
            .collect();

        let band_effects: Vec<(usize, Vec<(usize, String, EffectSourceType)>)> = self.effects_editor.editor.bands
            .iter()
            .enumerate()
            .map(|(band_idx, band)| {
                let effects: Vec<_> = band.effects
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i, e.id.clone(), e.source.clone()))
                    .collect();
                (band_idx, effects)
            })
            .collect();

        let post_fx_effects: Vec<(usize, String, EffectSourceType)> = self.effects_editor.editor.post_fx
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.id.clone(), e.source.clone()))
            .collect();

        // Instantiate pre-fx CLAP effects
        for (effect_idx, plugin_id, source) in &pre_fx_effects {
            if *source == EffectSourceType::Clap {
                let effect_instance_id = format!("{}_cue_prefx_{}", plugin_id, effect_idx);
                self.instantiate_clap_effect(plugin_id, effect_instance_id);
            }
        }

        // Instantiate band CLAP effects
        for (band_idx, effects) in &band_effects {
            for (effect_idx, plugin_id, source) in effects {
                if *source == EffectSourceType::Clap {
                    let effect_instance_id = format!("{}_cue_b{}_{}", plugin_id, band_idx, effect_idx);
                    self.instantiate_clap_effect(plugin_id, effect_instance_id);
                }
            }
        }

        // Instantiate post-fx CLAP effects
        for (effect_idx, plugin_id, source) in &post_fx_effects {
            if *source == EffectSourceType::Clap {
                let effect_instance_id = format!("{}_cue_postfx_{}", plugin_id, effect_idx);
                self.instantiate_clap_effect(plugin_id, effect_instance_id);
            }
        }
    }

    /// Instantiate a CLAP plugin and store its GUI handle
    fn instantiate_clap_effect(&mut self, plugin_id: &str, effect_instance_id: String) {
        // Check if already instantiated
        if self.domain.get_clap_gui_handle(&effect_instance_id).is_some() {
            log::debug!("CLAP effect '{}' already instantiated", effect_instance_id);
            return;
        }

        match self.domain.create_clap_effect_with_gui(plugin_id, effect_instance_id.clone()) {
            Ok(_effect) => {
                log::info!("Instantiated CLAP effect '{}' -> '{}'", plugin_id, effect_instance_id);
                // Note: The effect is created but not added to audio yet.
                // The GUI handle is stored in domain for later use.
                // Audio effects are added via sync_editor_to_audio when preview is enabled.
            }
            Err(e) => {
                log::error!("Failed to instantiate CLAP effect '{}': {}", plugin_id, e);
            }
        }
    }

    /// Save the current editor state as a preset
    pub fn handle_effects_editor_save(&mut self, name: String) -> Task<Message> {
        let config = MultibandPresetConfig::from_editor_state(&self.effects_editor.editor, &name);

        match save_preset(&config, &self.domain.collection_root()) {
            Ok(()) => {
                self.effects_editor.editing_preset = Some(name.clone());
                self.effects_editor.set_status(format!("Saved preset '{}'", name));
                self.effects_editor.close_save_dialog();

                // Refresh presets list
                let presets = mesh_widgets::multiband::list_presets(&self.domain.collection_root());
                self.effects_editor.editor.available_presets = presets;
            }
            Err(e) => {
                log::error!("Failed to save preset '{}': {}", name, e);
                self.effects_editor.set_status(format!("Failed to save: {}", e));
            }
        }
        Task::none()
    }

    /// Delete a preset
    fn handle_delete_preset(&mut self, name: String) -> Task<Message> {
        match mesh_widgets::multiband::delete_preset(&self.domain.collection_root(), &name) {
            Ok(()) => {
                self.effects_editor.set_status(format!("Deleted preset '{}'", name));

                // If we deleted the currently editing preset, clear it
                if self.effects_editor.editing_preset.as_ref() == Some(&name) {
                    self.effects_editor.editing_preset = None;
                }

                // Refresh presets list
                let presets = mesh_widgets::multiband::list_presets(&self.domain.collection_root());
                self.effects_editor.editor.available_presets = presets;
            }
            Err(e) => {
                log::error!("Failed to delete preset '{}': {}", name, e);
                self.effects_editor.set_status(format!("Failed to delete: {}", e));
            }
        }
        Task::none()
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Audio Preview Controls
    // ═══════════════════════════════════════════════════════════════════════════

    /// Toggle audio preview on/off
    ///
    /// When enabled, syncs the current editor state to the audio engine.
    /// When disabled, resets the stem's multiband to default (clean) state.
    pub fn handle_effects_editor_toggle_preview(&mut self) -> Task<Message> {
        self.effects_editor.toggle_audio_preview();

        if self.effects_editor.audio_preview_enabled {
            // Sync current editor state to audio
            self.sync_editor_to_audio();
            self.effects_editor.set_status("Audio preview enabled");
            log::info!("Effects editor: audio preview enabled for stem {:?}", self.effects_editor.preview_stem);
        } else {
            // Reset to clean state
            let stem = self.effects_editor.preview_stem;
            self.audio.reset_multiband(stem);
            self.effects_editor.set_status("Audio preview disabled");
            log::info!("Effects editor: audio preview disabled, reset stem {:?}", stem);
        }

        Task::none()
    }

    /// Set which stem to use for audio preview
    ///
    /// If preview is currently enabled, this resets the old stem and syncs to the new one.
    pub fn handle_effects_editor_set_preview_stem(&mut self, stem: Stem) -> Task<Message> {
        let old_stem = self.effects_editor.preview_stem;
        let was_enabled = self.effects_editor.audio_preview_enabled;

        // Update the preview stem
        self.effects_editor.set_preview_stem(stem);

        // If preview was enabled, switch stems
        if was_enabled && old_stem != stem {
            // Reset old stem
            self.audio.reset_multiband(old_stem);
            // Sync to new stem
            self.sync_editor_to_audio();
            log::info!("Effects editor: switched preview from {:?} to {:?}", old_stem, stem);
        }

        Task::none()
    }

    /// Sync the entire editor state to the audio engine
    ///
    /// This rebuilds the audio state from scratch to match the editor:
    /// 1. Reset the multiband host to clean slate
    /// 2. Add bands as needed
    /// 3. Set crossover frequencies
    /// 4. Configure band mute/solo/gain
    /// 5. Add effects to each band
    /// 6. Add pre-fx and post-fx effects
    /// 7. Sync macro values
    /// 8. Sync effect parameters and bypass states
    fn sync_editor_to_audio(&mut self) {
        let stem = self.effects_editor.preview_stem;

        // 1. Reset the multiband host to clean slate
        self.audio.reset_multiband(stem);

        // Extract data from editor to avoid borrow conflicts with self.domain
        let num_bands = self.effects_editor.editor.bands.len();
        let crossover_freqs: Vec<f32> = self.effects_editor.editor.crossover_freqs.clone();
        let band_configs: Vec<(bool, bool, f32)> = self.effects_editor.editor.bands.iter()
            .map(|b| (b.muted, b.soloed, b.gain))
            .collect();

        // Collect effect IDs, sources, and parameters for each location
        // Each effect includes: (id, source, bypassed, [(param_index, value)])
        let pre_fx_effects: Vec<EffectSyncData> = self.effects_editor.editor.pre_fx.iter()
            .map(EffectSyncData::from_effect)
            .collect();
        let band_effects: Vec<Vec<EffectSyncData>> = self.effects_editor.editor.bands.iter()
            .map(|b| b.effects.iter().map(EffectSyncData::from_effect).collect())
            .collect();
        let post_fx_effects: Vec<EffectSyncData> = self.effects_editor.editor.post_fx.iter()
            .map(EffectSyncData::from_effect)
            .collect();

        // Collect macro values
        let macro_values: Vec<f32> = (0..self.effects_editor.editor.macro_knobs.len())
            .map(|i| self.effects_editor.editor.macro_value(i))
            .collect();

        // 2. Add bands (editor starts with 1 band, add more if needed)
        for _ in 1..num_bands {
            self.audio.add_multiband_band(stem);
        }

        // 3. Set crossover frequencies
        for (i, freq) in crossover_freqs.iter().enumerate() {
            self.audio.set_multiband_crossover(stem, i, *freq);
        }

        // 4. Configure bands and add effects
        for (band_idx, (muted, soloed, gain)) in band_configs.iter().enumerate() {
            self.audio.set_multiband_band_mute(stem, band_idx, *muted);
            self.audio.set_multiband_band_solo(stem, band_idx, *soloed);
            self.audio.set_multiband_band_gain(stem, band_idx, *gain);

            // Add effects to band
            if let Some(effects) = band_effects.get(band_idx) {
                for (effect_idx, data) in effects.iter().enumerate() {
                    let location = EffectChainLocation::Band(band_idx);
                    if let Some(effect) = self.create_effect_for_audio(&data.id, &data.source, location, effect_idx) {
                        self.audio.add_multiband_band_effect(stem, band_idx, effect);
                        // Sync bypass
                        if data.bypassed {
                            self.audio.set_multiband_effect_bypass(stem, band_idx, effect_idx, true);
                        }
                        // Sync parameters
                        for (param_idx, value) in &data.params {
                            self.audio.set_multiband_effect_param(stem, band_idx, effect_idx, *param_idx, *value);
                        }
                    }
                }
            }
        }

        // 5. Add pre-fx effects
        for (effect_idx, data) in pre_fx_effects.iter().enumerate() {
            if let Some(effect) = self.create_effect_for_audio(&data.id, &data.source, EffectChainLocation::PreFx, effect_idx) {
                self.audio.add_multiband_pre_fx(stem, effect);
                // Sync bypass
                if data.bypassed {
                    self.audio.set_multiband_pre_fx_bypass(stem, effect_idx, true);
                }
                // Sync parameters
                for (param_idx, value) in &data.params {
                    self.audio.set_multiband_pre_fx_param(stem, effect_idx, *param_idx, *value);
                }
            }
        }

        // 6. Add post-fx effects
        for (effect_idx, data) in post_fx_effects.iter().enumerate() {
            if let Some(effect) = self.create_effect_for_audio(&data.id, &data.source, EffectChainLocation::PostFx, effect_idx) {
                self.audio.add_multiband_post_fx(stem, effect);
                // Sync bypass
                if data.bypassed {
                    self.audio.set_multiband_post_fx_bypass(stem, effect_idx, true);
                }
                // Sync parameters
                for (param_idx, value) in &data.params {
                    self.audio.set_multiband_post_fx_param(stem, effect_idx, *param_idx, *value);
                }
            }
        }

        // 7. Sync macro values
        for (idx, value) in macro_values.iter().enumerate() {
            self.audio.set_multiband_macro(stem, idx, *value);
        }

        log::debug!("Synced editor state to audio: {} bands, {} pre-fx, {} post-fx",
            num_bands, pre_fx_effects.len(), post_fx_effects.len());
    }

    /// Create an audio effect instance by ID, source type, and location
    ///
    /// For CLAP effects, creates the effect WITH a GUI handle so that the same
    /// plugin wrapper is used for both audio processing and GUI/learning.
    /// This is essential for parameter learning to work correctly.
    fn create_effect_for_audio(
        &mut self,
        id: &str,
        source: &EffectSourceType,
        location: EffectChainLocation,
        effect_idx: usize,
    ) -> Option<Box<dyn mesh_core::effect::Effect>> {
        match source {
            EffectSourceType::Pd => {
                self.domain.create_pd_effect(id).ok()
            }
            EffectSourceType::Clap => {
                // Generate effect instance ID matching the format used in handlers
                let effect_instance_id = match location {
                    EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", id, effect_idx),
                    EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", id, band_idx, effect_idx),
                    EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", id, effect_idx),
                };

                // Create effect WITH GUI handle so audio and GUI share the same wrapper
                // This ensures parameter learning works correctly
                self.domain.create_clap_effect_with_gui(id, effect_instance_id).ok()
            }
            EffectSourceType::Native => {
                // Native effects not supported in presets yet
                None
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Parameter Learning
    // ═══════════════════════════════════════════════════════════════════════════

    /// Poll for parameter learning changes
    ///
    /// Called periodically when learning mode is active. Polls the plugin's
    /// GUI handle for parameter changes and emits ParamLearned when detected.
    pub fn poll_learning_mode(&mut self) -> Task<Message> {
        // Only poll if we're in learning mode
        if !self.plugin_gui_manager.is_learning() {
            return Task::none();
        }

        // Get the effect instance ID we're learning from
        let effect_instance_id = match self.plugin_gui_manager.effect_to_poll() {
            Some(id) => id.to_string(),
            None => return Task::none(),
        };

        // Get the GUI handle and poll for changes
        if let Some(gui_handle) = self.domain.get_clap_gui_handle(&effect_instance_id) {
            if let Some((param_id, param_name, knob_idx)) =
                self.plugin_gui_manager.poll_learning_changes(&effect_instance_id, gui_handle)
            {
                // Get the learning target location from the editor
                if let Some((location, effect_idx, _knob)) = self.effects_editor.editor.learning_target() {
                    // Emit the ParamLearned message
                    return Task::done(Message::EffectsEditor(
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

        Task::none()
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Macro Modulation
    // ═══════════════════════════════════════════════════════════════════════════

    /// Apply macro modulation to all parameters mapped to this macro
    ///
    /// When a macro value changes, finds all parameters mapped to it and sends
    /// modulated values to the audio engine.
    fn apply_macro_modulation(&mut self, macro_index: usize, macro_value: f32) {
        if !self.effects_editor.audio_preview_enabled {
            return; // No audio preview, no need to modulate
        }

        let stem = self.effects_editor.preview_stem;

        // Collect all modulation commands first (to avoid borrow conflicts)
        // Each command is: (location, effect_idx, param_index, modulated_value)
        let mut commands: Vec<(EffectChainLocation, usize, usize, f32)> = Vec::new();

        // Process Pre-FX effects
        for (effect_idx, effect) in self.effects_editor.editor.pre_fx.iter().enumerate() {
            collect_modulation_commands(effect, effect_idx, EffectChainLocation::PreFx, macro_index, macro_value, &mut commands);
        }

        // Process Band effects
        for (band_idx, band) in self.effects_editor.editor.bands.iter().enumerate() {
            for (effect_idx, effect) in band.effects.iter().enumerate() {
                collect_modulation_commands(effect, effect_idx, EffectChainLocation::Band(band_idx), macro_index, macro_value, &mut commands);
            }
        }

        // Process Post-FX effects
        for (effect_idx, effect) in self.effects_editor.editor.post_fx.iter().enumerate() {
            collect_modulation_commands(effect, effect_idx, EffectChainLocation::PostFx, macro_index, macro_value, &mut commands);
        }

        // Now send all commands to audio engine
        for (location, effect_idx, param_index, modulated_value) in commands {
            match location {
                EffectChainLocation::PreFx => {
                    self.audio.set_multiband_pre_fx_param(stem, effect_idx, param_index, modulated_value);
                }
                EffectChainLocation::Band(band_idx) => {
                    self.audio.set_multiband_effect_param(stem, band_idx, effect_idx, param_index, modulated_value);
                }
                EffectChainLocation::PostFx => {
                    self.audio.set_multiband_post_fx_param(stem, effect_idx, param_index, modulated_value);
                }
            }
        }
    }
}

/// Collect modulation commands for a single effect's parameters
///
/// Helper function that collects modulation commands without borrowing self.
fn collect_modulation_commands(
    effect: &mesh_widgets::multiband::EffectUiState,
    effect_idx: usize,
    location: EffectChainLocation,
    macro_index: usize,
    macro_value: f32,
    commands: &mut Vec<(EffectChainLocation, usize, usize, f32)>,
) {
    for (_knob_idx, assignment) in effect.knob_assignments.iter().enumerate() {
        // Check if this knob is mapped to the macro we're modulating
        if let Some(ref mapping) = assignment.macro_mapping {
            if mapping.macro_index == Some(macro_index) {
                if let Some(param_index) = assignment.param_index {
                    // Calculate modulated value
                    let base_value = assignment.value;
                    let modulated_value = mapping.modulate(base_value, macro_value);
                    commands.push((location, effect_idx, param_index, modulated_value));
                }
            }
        }
    }
}

/// Data extracted from EffectUiState for syncing to audio
///
/// Used to avoid borrow conflicts when iterating over effects
/// while also calling audio methods.
struct EffectSyncData {
    id: String,
    source: EffectSourceType,
    bypassed: bool,
    /// Parameter index and value pairs to sync
    params: Vec<(usize, f32)>,
}

impl EffectSyncData {
    fn from_effect(effect: &mesh_widgets::multiband::EffectUiState) -> Self {
        // Extract parameters from knob assignments
        let params: Vec<(usize, f32)> = effect.knob_assignments
            .iter()
            .filter_map(|assignment| {
                assignment.param_index.map(|idx| (idx, assignment.value))
            })
            .collect();

        Self {
            id: effect.id.clone(),
            source: effect.source.clone(),
            bypassed: effect.bypassed,
            params,
        }
    }
}
