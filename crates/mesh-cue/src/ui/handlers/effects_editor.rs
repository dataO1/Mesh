//! Effects editor message handlers
//!
//! Handles multiband effects editing, preset save/load, and audio preview routing.

use iced::Task;
use mesh_core::types::Stem;
use mesh_widgets::multiband::{
    ChainTarget, DryWetKnobId, EffectChainLocation, EffectSourceType, MultibandPresetConfig,
    ParamMacroMapping, load_preset, save_preset,
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
                let name = if self.effects_editor.editor.preset_name_input.is_empty() {
                    "Untitled".to_string()
                } else {
                    self.effects_editor.editor.preset_name_input.clone()
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
                self.effects_editor.editor.preset_name_input = name;
            }

            // Crossover control - forward to editor state and audio
            StartDragCrossover(idx) => {
                self.effects_editor.editor.dragging_crossover = Some(idx);
                // Store initial frequency for relative calculation
                let start_freq = self.effects_editor.editor.crossover_freqs.get(idx).copied();
                self.effects_editor.editor.crossover_drag_start_freq = start_freq;
                self.effects_editor.editor.crossover_drag_last_x = None;
            }
            DragCrossover(freq) => {
                // Legacy absolute positioning
                if let Some(idx) = self.effects_editor.editor.dragging_crossover {
                    self.effects_editor.editor.set_crossover_freq(idx, freq);
                    // Apply to audio preview if enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_crossover(stem, idx, freq);
                    }
                }
            }
            DragCrossoverRelative { new_freq, mouse_x } => {
                if let Some(idx) = self.effects_editor.editor.dragging_crossover {
                    self.effects_editor.editor.set_crossover_freq(idx, new_freq);
                    self.effects_editor.editor.crossover_drag_last_x = Some(mouse_x);
                    // Apply to audio preview if enabled
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        self.audio.set_multiband_crossover(stem, idx, new_freq);
                    }
                }
            }
            EndDragCrossover => {
                self.effects_editor.editor.dragging_crossover = None;
                self.effects_editor.editor.crossover_drag_start_freq = None;
                self.effects_editor.editor.crossover_drag_last_x = None;
            }

            // Band management
            AddBand => {
                self.effects_editor.editor.add_band();
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.add_multiband_band(stem);
                }
            }
            AddBandAtFrequency(freq) => {
                self.effects_editor.editor.add_band_at_frequency(freq);
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

            // Band drag and drop
            StartDragBand(band_idx) => {
                self.effects_editor.editor.dragging_band = Some(band_idx);
            }
            SetBandDropTarget(target) => {
                self.effects_editor.editor.band_drop_target = target;
            }
            DropBandAt(target_idx) => {
                if let Some(source_idx) = self.effects_editor.editor.dragging_band {
                    self.effects_editor.editor.swap_band_contents(source_idx, target_idx);
                }
                self.effects_editor.editor.dragging_band = None;
                self.effects_editor.editor.band_drop_target = None;
            }
            EndDragBand => {
                self.effects_editor.editor.dragging_band = None;
                self.effects_editor.editor.band_drop_target = None;
            }

            // Effect drag and drop
            StartDragEffect { location, effect } => {
                // Get the effect name for visual drag overlay
                let effect_name = match location {
                    EffectChainLocation::PreFx => {
                        self.effects_editor.editor.pre_fx.get(effect).map(|e| e.name.clone())
                    }
                    EffectChainLocation::Band(band_idx) => {
                        self.effects_editor.editor.bands.get(band_idx)
                            .and_then(|b| b.effects.get(effect))
                            .map(|e| e.name.clone())
                    }
                    EffectChainLocation::PostFx => {
                        self.effects_editor.editor.post_fx.get(effect).map(|e| e.name.clone())
                    }
                };
                self.effects_editor.editor.dragging_effect = Some((location, effect));
                self.effects_editor.editor.dragging_effect_name = effect_name;
                self.effects_editor.editor.effect_drag_mouse_pos = None;
            }
            SetEffectDropTarget(target) => {
                self.effects_editor.editor.effect_drop_target = target;
            }
            DropEffectAt { location, position } => {
                if let Some((from_location, from_idx)) = self.effects_editor.editor.dragging_effect {
                    self.effects_editor.editor.move_effect(from_location, from_idx, location, position);
                }
                self.effects_editor.editor.dragging_effect = None;
                self.effects_editor.editor.effect_drop_target = None;
                self.effects_editor.editor.dragging_effect_name = None;
                self.effects_editor.editor.effect_drag_mouse_pos = None;
            }
            EndDragEffect => {
                self.effects_editor.editor.dragging_effect = None;
                self.effects_editor.editor.effect_drop_target = None;
                self.effects_editor.editor.dragging_effect_name = None;
                self.effects_editor.editor.effect_drag_mouse_pos = None;
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

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                // to prevent flickering from dual event processing
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_macro_knob = Some(index);
                        if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                            knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                        }
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_macro_knob = None;
                        if let Some(knob) = self.effects_editor.editor.macro_knobs.get_mut(index) {
                            knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                        }
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }
            EffectKnob { location, effect, param, event } => {
                use mesh_widgets::knob::KnobEvent;

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                // to prevent flickering from dual event processing
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_effect_knob = Some((location, effect, param));
                        let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                        knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_effect_knob = None;
                        let knob = self.effects_editor.editor.get_effect_knob(location, effect, param);
                        knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Macro mapping
            // ─────────────────────────────────────────────────────────────────────
            RenameMacro { index, name } => {
                self.effects_editor.editor.set_macro_name(index, name.clone());
            }
            StartEditMacroName(index) => {
                self.effects_editor.editor.editing_macro_name = Some(index);
            }
            EndEditMacroName => {
                self.effects_editor.editor.editing_macro_name = None;
            }
            StartDragMacro(index) => {
                self.effects_editor.editor.dragging_macro = Some(index);
            }
            EndDragMacro => {
                self.effects_editor.editor.dragging_macro = None;
            }
            DropMacroOnParam { macro_index, location, effect, param } => {
                use mesh_widgets::multiband::ParamMacroMapping;

                let offset_range = 0.25; // ±25% default range

                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    if let Some(assignment) = effect_state.knob_assignments.get_mut(param) {
                        assignment.macro_mapping = Some(ParamMacroMapping::new(macro_index, offset_range));
                    }
                    if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(macro_index) {
                        macro_state.mapping_count += 1;
                    }
                }

                // Add to reverse mapping index
                self.effects_editor.editor.add_mapping_to_index(macro_index, location, effect, param, offset_range);

                self.effects_editor.editor.dragging_macro = None;
                log::info!("Mapped macro {} to {:?} effect {} param {}", macro_index, location, effect, param);
            }
            RemoveParamMapping { location, effect, param } => {
                // Get the macro that was mapped (before modifying)
                let old_macro_index = {
                    let effect_state = match location {
                        EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect),
                        EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                            .get(band_idx)
                            .and_then(|b| b.effects.get(effect)),
                        EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect),
                    };
                    effect_state
                        .and_then(|e| e.knob_assignments.get(param))
                        .and_then(|a| a.macro_mapping.as_ref())
                        .and_then(|m| m.macro_index)
                };

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

                // Remove from reverse mapping index
                if let Some(macro_index) = old_macro_index {
                    self.effects_editor.editor.remove_mapping_from_index(macro_index, location, effect, param);
                }
            }
            OpenMacroMapper(_) | AddMacroMapping { .. } | ClearMacroMappings(_) => {
                // Not implemented in mesh-cue (direct drag-drop is used instead)
            }

            // ─────────────────────────────────────────────────────────────────────
            // Macro Modulation Range Controls
            // ─────────────────────────────────────────────────────────────────────
            StartDragModRange { macro_index, mapping_idx } => {
                use mesh_widgets::multiband::ModRangeDrag;

                // Get the current offset_range as the starting value
                let start_offset = self.effects_editor.editor.macro_mappings_index[macro_index]
                    .get(mapping_idx)
                    .map(|m| m.offset_range)
                    .unwrap_or(0.0);

                self.effects_editor.editor.dragging_mod_range = Some(ModRangeDrag {
                    macro_index,
                    mapping_idx,
                    start_offset,
                    start_y: None, // Will be set on first mouse move
                });
            }

            DragModRange { macro_index, mapping_idx, new_offset_range } => {
                use mesh_widgets::multiband::ParamMacroMapping;

                // Clamp offset_range to valid range
                let new_offset_range = new_offset_range.clamp(-1.0, 1.0);

                // Look up the mapping reference to get the effect location
                if let Some(mapping_ref) = self.effects_editor.editor.macro_mappings_index[macro_index].get(mapping_idx).copied() {
                    let location = mapping_ref.location;
                    let effect_idx = mapping_ref.effect_idx;
                    let knob_idx = mapping_ref.knob_idx;

                    // Update actual offset_range in the effect's knob assignment
                    let effect_state = match location {
                        EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect_idx),
                        EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                            .get_mut(band_idx)
                            .and_then(|b| b.effects.get_mut(effect_idx)),
                        EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect_idx),
                    };

                    if let Some(effect) = effect_state {
                        if let Some(mapping) = effect.knob_assignments[knob_idx].macro_mapping.as_mut() {
                            mapping.offset_range = new_offset_range;
                        }
                    }

                    // Update the index cache
                    self.effects_editor.editor.update_mapping_offset_range(macro_index, mapping_idx, new_offset_range);

                    // If audio preview is enabled, send updated modulation to audio
                    if self.effects_editor.audio_preview_enabled {
                        let stem = self.effects_editor.preview_stem;
                        let macro_value = self.effects_editor.editor.macro_value(macro_index);

                        // Get base value and recalculate modulated value
                        let base_value = {
                            let effect_state = match location {
                                EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect_idx),
                                EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                    .get(band_idx)
                                    .and_then(|b| b.effects.get(effect_idx)),
                                EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect_idx),
                            };
                            effect_state
                                .and_then(|e| e.knob_assignments.get(knob_idx))
                                .map(|a| a.value)
                                .unwrap_or(0.5)
                        };

                        let param_index = {
                            let effect_state = match location {
                                EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect_idx),
                                EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                    .get(band_idx)
                                    .and_then(|b| b.effects.get(effect_idx)),
                                EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect_idx),
                            };
                            effect_state
                                .and_then(|e| e.knob_assignments.get(knob_idx))
                                .and_then(|a| a.param_index)
                        };

                        if let Some(param_idx) = param_index {
                            let modulated_value = ParamMacroMapping::new(macro_index, new_offset_range).modulate(base_value, macro_value);
                            match location {
                                EffectChainLocation::PreFx => {
                                    self.audio.set_multiband_pre_fx_param(stem, effect_idx, param_idx, modulated_value);
                                }
                                EffectChainLocation::Band(band_idx) => {
                                    self.audio.set_multiband_effect_param(stem, band_idx, effect_idx, param_idx, modulated_value);
                                }
                                EffectChainLocation::PostFx => {
                                    self.audio.set_multiband_post_fx_param(stem, effect_idx, param_idx, modulated_value);
                                }
                            }
                        }
                    }
                }
            }

            EndDragModRange => {
                self.effects_editor.editor.dragging_mod_range = None;
            }

            HoverModRange { macro_index, mapping_idx } => {
                self.effects_editor.editor.hovered_mapping = Some((macro_index, mapping_idx));
            }

            UnhoverModRange => {
                self.effects_editor.editor.hovered_mapping = None;
            }

            HoverParam { location, effect, param } => {
                self.effects_editor.editor.hovered_param = Some((location, effect, param));
            }

            UnhoverParam => {
                self.effects_editor.editor.hovered_param = None;
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

                // Route to dragging mod range indicator
                if let Some(ref mut drag) = self.effects_editor.editor.dragging_mod_range {
                    use mesh_widgets::knob::ModulationRange;
                    use mesh_widgets::multiband::ParamMacroMapping;

                    const MOD_DRAG_SENSITIVITY: f32 = 0.01; // offset per pixel

                    if drag.start_y.is_none() {
                        // First mouse move - capture starting position
                        drag.start_y = Some(position.y);
                    } else {
                        // Calculate new offset based on drag delta
                        // Moving UP (negative y delta) increases offset, moving DOWN decreases
                        let start_y = drag.start_y.unwrap();
                        let delta_y = start_y - position.y; // Inverted: up is positive
                        let new_offset = (drag.start_offset + delta_y * MOD_DRAG_SENSITIVITY).clamp(-1.0, 1.0);
                        log::trace!("Mod range drag: start={:.2}, delta_y={:.1}, new_offset={:.3}", drag.start_offset, delta_y, new_offset);

                        let macro_index = drag.macro_index;
                        let mapping_idx = drag.mapping_idx;

                        // Look up the mapping reference to get the effect location
                        if let Some(mapping_ref) = self.effects_editor.editor.macro_mappings_index[macro_index].get(mapping_idx).copied() {
                            let location = mapping_ref.location;
                            let effect_idx = mapping_ref.effect_idx;
                            let knob_idx = mapping_ref.knob_idx;

                            // Get base value and actual param index for audio update
                            let (base_value, actual_param_index) = {
                                let effect_state = match location {
                                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get(effect_idx),
                                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                        .get(band_idx)
                                        .and_then(|b| b.effects.get(effect_idx)),
                                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get(effect_idx),
                                };
                                let assignment = effect_state.and_then(|e| e.knob_assignments.get(knob_idx));
                                (
                                    assignment.map(|a| a.value).unwrap_or(0.5),
                                    assignment.and_then(|a| a.param_index).unwrap_or(knob_idx),
                                )
                            };

                            // Update actual offset_range in the effect's knob assignment
                            let effect_state = match location {
                                EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect_idx),
                                EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                                    .get_mut(band_idx)
                                    .and_then(|b| b.effects.get_mut(effect_idx)),
                                EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect_idx),
                            };

                            if let Some(effect) = effect_state {
                                if let Some(mapping) = effect.knob_assignments[knob_idx].macro_mapping.as_mut() {
                                    mapping.offset_range = new_offset;
                                }
                            }

                            // Update the index cache
                            self.effects_editor.editor.update_mapping_offset_range(macro_index, mapping_idx, new_offset);

                            // Update parameter knob modulation visualization
                            let mapping = ParamMacroMapping::new(macro_index, new_offset);
                            let key = (location, effect_idx, knob_idx);
                            if let Some(knob) = self.effects_editor.editor.effect_knobs.get_mut(&key) {
                                let (min, max) = mapping.modulation_bounds(base_value);
                                knob.set_modulations(vec![ModulationRange::new(
                                    min,
                                    max,
                                    iced::Color::from_rgb(0.9, 0.5, 0.2), // Orange for modulation
                                )]);
                            }

                            // If audio preview is enabled, recompute and send the modulated value
                            if self.effects_editor.audio_preview_enabled {
                                let macro_value = self.effects_editor.editor.macro_value(macro_index);
                                let modulated_value = mapping.modulate(base_value, macro_value);
                                let stem = self.effects_editor.preview_stem;

                                match location {
                                    EffectChainLocation::PreFx => {
                                        self.audio.set_multiband_pre_fx_param(stem, effect_idx, actual_param_index, modulated_value);
                                    }
                                    EffectChainLocation::Band(band_idx) => {
                                        self.audio.set_multiband_effect_param(stem, band_idx, effect_idx, actual_param_index, modulated_value);
                                    }
                                    EffectChainLocation::PostFx => {
                                        self.audio.set_multiband_post_fx_param(stem, effect_idx, actual_param_index, modulated_value);
                                    }
                                }
                            }
                        }
                    }
                }

                // Route to dragging dry/wet knob
                if let Some(dry_wet_id) = self.effects_editor.editor.dragging_dry_wet_knob.clone() {
                    match dry_wet_id {
                        DryWetKnobId::Effect(location, effect) => {
                            let key = (location.clone(), effect);
                            if let Some(knob) = self.effects_editor.editor.effect_dry_wet_knobs.get_mut(&key) {
                                if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                                    return self.handle_effects_editor(SetEffectDryWet { location, effect, mix: new_value });
                                }
                            }
                        }
                        DryWetKnobId::PreFxChain => {
                            if let Some(new_value) = self.effects_editor.editor.pre_fx_chain_dry_wet_knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                                return self.handle_effects_editor(SetPreFxChainDryWet(new_value));
                            }
                        }
                        DryWetKnobId::BandChain(band) => {
                            if let Some(knob) = self.effects_editor.editor.band_chain_dry_wet_knobs.get_mut(band) {
                                if let Some(new_value) = knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                                    return self.handle_effects_editor(SetBandChainDryWet { band, mix: new_value });
                                }
                            }
                        }
                        DryWetKnobId::PostFxChain => {
                            if let Some(new_value) = self.effects_editor.editor.post_fx_chain_dry_wet_knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                                return self.handle_effects_editor(SetPostFxChainDryWet(new_value));
                            }
                        }
                        DryWetKnobId::Global => {
                            if let Some(new_value) = self.effects_editor.editor.global_dry_wet_knob.handle_event(KnobEvent::Moved(position), DEFAULT_SENSITIVITY) {
                                return self.handle_effects_editor(SetGlobalDryWet(new_value));
                            }
                        }
                    }
                }

                // Track mouse position during effect drag for visual overlay
                if self.effects_editor.editor.dragging_effect.is_some() {
                    self.effects_editor.editor.effect_drag_mouse_pos = Some((position.x, position.y));
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
                // Release dragging mod range indicator
                self.effects_editor.editor.dragging_mod_range = None;

                // Release dragging dry/wet knob
                if let Some(dry_wet_id) = self.effects_editor.editor.dragging_dry_wet_knob.take() {
                    match dry_wet_id {
                        DryWetKnobId::Effect(location, effect) => {
                            let key = (location, effect);
                            if let Some(knob) = self.effects_editor.editor.effect_dry_wet_knobs.get_mut(&key) {
                                knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                            }
                        }
                        DryWetKnobId::PreFxChain => {
                            self.effects_editor.editor.pre_fx_chain_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                        }
                        DryWetKnobId::BandChain(band) => {
                            if let Some(knob) = self.effects_editor.editor.band_chain_dry_wet_knobs.get_mut(band) {
                                knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                            }
                        }
                        DryWetKnobId::PostFxChain => {
                            self.effects_editor.editor.post_fx_chain_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                        }
                        DryWetKnobId::Global => {
                            self.effects_editor.editor.global_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                        }
                    }
                }
            }

            // ─────────────────────────────────────────────────────────────────────
            // Effect selected (from legacy picker - now handled by EffectPicker)
            // ─────────────────────────────────────────────────────────────────────
            PreFxEffectSelected { .. } | EffectSelected { .. } | PostFxEffectSelected { .. } => {
                // Handled by effect_picker handler instead
            }

            // ─────────────────────────────────────────────────────────────────────
            // Dry/Wet Mix Controls
            // ─────────────────────────────────────────────────────────────────────
            SetEffectDryWet { location, effect, mix } => {
                // Update UI state
                match &location {
                    EffectChainLocation::PreFx => {
                        if let Some(fx) = self.effects_editor.editor.pre_fx.get_mut(effect) {
                            fx.dry_wet = mix;
                        }
                        if self.effects_editor.audio_preview_enabled {
                            let stem = self.effects_editor.preview_stem;
                            self.audio.set_multiband_pre_fx_effect_dry_wet(stem, effect, mix);
                        }
                    }
                    EffectChainLocation::Band(band) => {
                        if let Some(b) = self.effects_editor.editor.bands.get_mut(*band) {
                            if let Some(fx) = b.effects.get_mut(effect) {
                                fx.dry_wet = mix;
                            }
                        }
                        if self.effects_editor.audio_preview_enabled {
                            let stem = self.effects_editor.preview_stem;
                            self.audio.set_multiband_band_effect_dry_wet(stem, *band, effect, mix);
                        }
                    }
                    EffectChainLocation::PostFx => {
                        if let Some(fx) = self.effects_editor.editor.post_fx.get_mut(effect) {
                            fx.dry_wet = mix;
                        }
                        if self.effects_editor.audio_preview_enabled {
                            let stem = self.effects_editor.preview_stem;
                            self.audio.set_multiband_post_fx_effect_dry_wet(stem, effect, mix);
                        }
                    }
                }
                // Sync knob value
                let knob = self.effects_editor.editor.effect_dry_wet_knobs
                    .entry((location, effect))
                    .or_insert_with(|| mesh_widgets::knob::Knob::new(24.0));
                knob.set_value(mix);
            }

            EffectDryWetKnob { location, effect, event } => {
                use mesh_widgets::knob::KnobEvent;

                // Ensure knob exists with correct initial value
                let key = (location.clone(), effect);
                if !self.effects_editor.editor.effect_dry_wet_knobs.contains_key(&key) {
                    let initial_value = match &location {
                        EffectChainLocation::PreFx => {
                            self.effects_editor.editor.pre_fx.get(effect).map(|e| e.dry_wet).unwrap_or(1.0)
                        }
                        EffectChainLocation::Band(band) => {
                            self.effects_editor.editor.bands.get(*band)
                                .and_then(|b| b.effects.get(effect))
                                .map(|e| e.dry_wet)
                                .unwrap_or(1.0)
                        }
                        EffectChainLocation::PostFx => {
                            self.effects_editor.editor.post_fx.get(effect).map(|e| e.dry_wet).unwrap_or(1.0)
                        }
                    };
                    let mut knob = mesh_widgets::knob::Knob::new(24.0);
                    knob.set_value(initial_value);
                    self.effects_editor.editor.effect_dry_wet_knobs.insert(key.clone(), knob);
                }

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_dry_wet_knob =
                            Some(DryWetKnobId::Effect(location, effect));
                        if let Some(knob) = self.effects_editor.editor.effect_dry_wet_knobs.get_mut(&key) {
                            knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                        }
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_dry_wet_knob = None;
                        if let Some(knob) = self.effects_editor.editor.effect_dry_wet_knobs.get_mut(&key) {
                            knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                        }
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            SetPreFxChainDryWet(mix) => {
                self.effects_editor.editor.pre_fx_chain_dry_wet = mix;
                self.effects_editor.editor.pre_fx_chain_dry_wet_knob.set_value(mix);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_pre_fx_chain_dry_wet(stem, mix);
                }
            }

            PreFxChainDryWetKnob(event) => {
                use mesh_widgets::knob::KnobEvent;

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_dry_wet_knob = Some(DryWetKnobId::PreFxChain);
                        self.effects_editor.editor.pre_fx_chain_dry_wet_knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_dry_wet_knob = None;
                        self.effects_editor.editor.pre_fx_chain_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            SetBandChainDryWet { band, mix } => {
                if let Some(b) = self.effects_editor.editor.bands.get_mut(band) {
                    b.chain_dry_wet = mix;
                }
                // Sync knob value
                while self.effects_editor.editor.band_chain_dry_wet_knobs.len() <= band {
                    self.effects_editor.editor.band_chain_dry_wet_knobs.push(mesh_widgets::knob::Knob::new(36.0));
                }
                self.effects_editor.editor.band_chain_dry_wet_knobs[band].set_value(mix);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_band_chain_dry_wet(stem, band, mix);
                }
            }

            BandChainDryWetKnob { band, event } => {
                use mesh_widgets::knob::KnobEvent;

                // Ensure we have enough knobs for this band with correct initial value
                while self.effects_editor.editor.band_chain_dry_wet_knobs.len() <= band {
                    let initial_value = self.effects_editor.editor.bands.get(band)
                        .map(|b| b.chain_dry_wet)
                        .unwrap_or(1.0);
                    let mut knob = mesh_widgets::knob::Knob::new(36.0);
                    knob.set_value(initial_value);
                    self.effects_editor.editor.band_chain_dry_wet_knobs.push(knob);
                }

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_dry_wet_knob = Some(DryWetKnobId::BandChain(band));
                        self.effects_editor.editor.band_chain_dry_wet_knobs[band].handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_dry_wet_knob = None;
                        self.effects_editor.editor.band_chain_dry_wet_knobs[band].handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            SetPostFxChainDryWet(mix) => {
                self.effects_editor.editor.post_fx_chain_dry_wet = mix;
                self.effects_editor.editor.post_fx_chain_dry_wet_knob.set_value(mix);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_post_fx_chain_dry_wet(stem, mix);
                }
            }

            PostFxChainDryWetKnob(event) => {
                use mesh_widgets::knob::KnobEvent;

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_dry_wet_knob = Some(DryWetKnobId::PostFxChain);
                        self.effects_editor.editor.post_fx_chain_dry_wet_knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_dry_wet_knob = None;
                        self.effects_editor.editor.post_fx_chain_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            SetGlobalDryWet(mix) => {
                self.effects_editor.editor.global_dry_wet = mix;
                self.effects_editor.editor.global_dry_wet_knob.set_value(mix);
                if self.effects_editor.audio_preview_enabled {
                    let stem = self.effects_editor.preview_stem;
                    self.audio.set_multiband_global_dry_wet(stem, mix);
                }
            }

            GlobalDryWetKnob(event) => {
                use mesh_widgets::knob::KnobEvent;

                // Only handle Pressed/Released locally - Moved is handled by GlobalMouseMoved
                match event {
                    KnobEvent::Pressed => {
                        self.effects_editor.editor.dragging_dry_wet_knob = Some(DryWetKnobId::Global);
                        self.effects_editor.editor.global_dry_wet_knob.handle_event(KnobEvent::Pressed, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Released => {
                        self.effects_editor.editor.dragging_dry_wet_knob = None;
                        self.effects_editor.editor.global_dry_wet_knob.handle_event(KnobEvent::Released, DEFAULT_SENSITIVITY);
                    }
                    KnobEvent::Moved(_) => {
                        // Ignore local Moved events - GlobalMouseMoved handles all movement
                    }
                }
            }

            DropMacroOnEffectDryWet { macro_index, location, effect } => {
                let offset_range = 0.5; // ±50% for dry/wet

                // Get effect state and set the dry/wet macro mapping
                let effect_state = match location {
                    EffectChainLocation::PreFx => self.effects_editor.editor.pre_fx.get_mut(effect),
                    EffectChainLocation::Band(band_idx) => self.effects_editor.editor.bands
                        .get_mut(band_idx)
                        .and_then(|b| b.effects.get_mut(effect)),
                    EffectChainLocation::PostFx => self.effects_editor.editor.post_fx.get_mut(effect),
                };

                if let Some(effect_state) = effect_state {
                    effect_state.dry_wet_macro_mapping = Some(ParamMacroMapping::new(macro_index, offset_range));
                }

                // Update macro's mapping count
                if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(macro_index) {
                    macro_state.mapping_count += 1;
                }

                // Clear drag state
                self.effects_editor.editor.dragging_macro = None;

                log::info!("Mapped macro {} to {:?} effect {} dry/wet with ±{:.0}% range",
                    macro_index, location, effect, offset_range * 100.0);
            }

            DropMacroOnChainDryWet { macro_index, chain } => {
                let offset_range = 0.5; // ±50% for dry/wet

                match chain {
                    ChainTarget::PreFx => {
                        self.effects_editor.editor.pre_fx_chain_dry_wet_macro_mapping =
                            Some(ParamMacroMapping::new(macro_index, offset_range));
                    }
                    ChainTarget::Band(band_idx) => {
                        if let Some(band) = self.effects_editor.editor.bands.get_mut(band_idx) {
                            band.chain_dry_wet_macro_mapping =
                                Some(ParamMacroMapping::new(macro_index, offset_range));
                        }
                    }
                    ChainTarget::PostFx => {
                        self.effects_editor.editor.post_fx_chain_dry_wet_macro_mapping =
                            Some(ParamMacroMapping::new(macro_index, offset_range));
                    }
                }

                // Update macro's mapping count
                if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(macro_index) {
                    macro_state.mapping_count += 1;
                }

                // Clear drag state
                self.effects_editor.editor.dragging_macro = None;

                log::info!("Mapped macro {} to {:?} chain dry/wet with ±{:.0}% range",
                    macro_index, chain, offset_range * 100.0);
            }

            DropMacroOnGlobalDryWet { macro_index } => {
                let offset_range = 0.5; // ±50% for dry/wet

                self.effects_editor.editor.global_dry_wet_macro_mapping =
                    Some(ParamMacroMapping::new(macro_index, offset_range));

                // Update macro's mapping count
                if let Some(macro_state) = self.effects_editor.editor.macros.get_mut(macro_index) {
                    macro_state.mapping_count += 1;
                }

                // Clear drag state
                self.effects_editor.editor.dragging_macro = None;

                log::info!("Mapped macro {} to global dry/wet with ±{:.0}% range",
                    macro_index, offset_range * 100.0);
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
    /// 4. Apply ALL saved parameter values to plugins (not just knob-mapped ones)
    /// 5. Ensure knob state exists for all effects
    /// 6. Sync to audio if preview is enabled
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

                // Rebuild the macro mappings reverse index
                self.effects_editor.editor.rebuild_macro_mappings_index();

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
    ///
    /// Captures ALL parameter values from CLAP plugins (not just knob-mapped ones)
    /// before saving. This preserves settings made via the plugin GUI.
    pub fn handle_effects_editor_save(&mut self, name: String) -> Task<Message> {
        // Capture all param values from CLAP plugins before creating config
        self.capture_all_effect_param_values();

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

    /// Capture current parameter values from all CLAP plugins
    ///
    /// Reads ALL param values from each CLAP plugin's GUI handle and stores them
    /// in the corresponding EffectUiState's saved_param_values field. This ensures
    /// that settings made via the plugin GUI are preserved in the preset.
    fn capture_all_effect_param_values(&mut self) {
        // Helper to generate effect instance ID
        fn effect_instance_id(id: &str, location: EffectChainLocation, effect_idx: usize) -> String {
            match location {
                EffectChainLocation::PreFx => format!("{}_cue_prefx_{}", id, effect_idx),
                EffectChainLocation::Band(band_idx) => format!("{}_cue_b{}_{}", id, band_idx, effect_idx),
                EffectChainLocation::PostFx => format!("{}_cue_postfx_{}", id, effect_idx),
            }
        }

        // Collect effect info first to avoid borrow conflicts
        let pre_fx_info: Vec<(usize, String, EffectSourceType)> = self.effects_editor.editor.pre_fx
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.id.clone(), e.source))
            .collect();

        let band_effects_info: Vec<(usize, Vec<(usize, String, EffectSourceType)>)> = self.effects_editor.editor.bands
            .iter()
            .enumerate()
            .map(|(band_idx, band)| {
                let effects: Vec<_> = band.effects
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i, e.id.clone(), e.source))
                    .collect();
                (band_idx, effects)
            })
            .collect();

        let post_fx_info: Vec<(usize, String, EffectSourceType)> = self.effects_editor.editor.post_fx
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.id.clone(), e.source))
            .collect();

        // Now capture params from CLAP plugins and update editor state
        // Pre-FX
        for (effect_idx, plugin_id, source) in &pre_fx_info {
            if *source == EffectSourceType::Clap {
                let instance_id = effect_instance_id(plugin_id, EffectChainLocation::PreFx, *effect_idx);
                if let Some(params) = self.capture_plugin_params(&instance_id) {
                    if let Some(effect) = self.effects_editor.editor.pre_fx.get_mut(*effect_idx) {
                        log::debug!("Captured {} params for pre-fx[{}]", params.len(), effect_idx);
                        effect.saved_param_values = params;
                    }
                }
            }
        }

        // Band effects
        for (band_idx, effects) in &band_effects_info {
            for (effect_idx, plugin_id, source) in effects {
                if *source == EffectSourceType::Clap {
                    let instance_id = effect_instance_id(plugin_id, EffectChainLocation::Band(*band_idx), *effect_idx);
                    if let Some(params) = self.capture_plugin_params(&instance_id) {
                        if let Some(band) = self.effects_editor.editor.bands.get_mut(*band_idx) {
                            if let Some(effect) = band.effects.get_mut(*effect_idx) {
                                log::debug!("Captured {} params for band[{}].effect[{}]", params.len(), band_idx, effect_idx);
                                effect.saved_param_values = params;
                            }
                        }
                    }
                }
            }
        }

        // Post-FX
        for (effect_idx, plugin_id, source) in &post_fx_info {
            if *source == EffectSourceType::Clap {
                let instance_id = effect_instance_id(plugin_id, EffectChainLocation::PostFx, *effect_idx);
                if let Some(params) = self.capture_plugin_params(&instance_id) {
                    if let Some(effect) = self.effects_editor.editor.post_fx.get_mut(*effect_idx) {
                        log::debug!("Captured {} params for post-fx[{}]", params.len(), effect_idx);
                        effect.saved_param_values = params;
                    }
                }
            }
        }
    }

    /// Capture all parameter values from a CLAP plugin instance
    ///
    /// Returns normalized (0.0-1.0) param values, or None if plugin not found.
    fn capture_plugin_params(&self, effect_instance_id: &str) -> Option<Vec<f32>> {
        let gui_handle = self.domain.get_clap_gui_handle(effect_instance_id)?;

        let mut param_values = Vec::with_capacity(gui_handle.param_ids.len());

        for &param_id in &gui_handle.param_ids {
            // Get current value and normalize it
            let value = if let (Some(current), Some((min, max, _default))) = (
                gui_handle.get_param_value(param_id),
                gui_handle.get_param_info(param_id),
            ) {
                let range = max - min;
                if range > 0.0 {
                    ((current - min) / range) as f32
                } else {
                    0.5
                }
            } else {
                0.5 // Default if can't read
            };

            param_values.push(value);
        }

        Some(param_values)
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

        // ─────────────────────────────────────────────────────────────────────
        // Dry/Wet Modulation
        // ─────────────────────────────────────────────────────────────────────

        // Helper to check if a mapping applies to this macro
        let apply_dry_wet = |mapping: &Option<mesh_widgets::multiband::ParamMacroMapping>,
                             base_value: f32| -> Option<f32> {
            if let Some(ref m) = mapping {
                if m.macro_index == Some(macro_index) {
                    return Some(m.modulate(base_value, macro_value));
                }
            }
            None
        };

        // Per-effect dry/wet: Pre-FX effects
        for (effect_idx, effect) in self.effects_editor.editor.pre_fx.iter().enumerate() {
            if let Some(modulated) = apply_dry_wet(&effect.dry_wet_macro_mapping, effect.dry_wet) {
                self.audio.set_multiband_pre_fx_effect_dry_wet(stem, effect_idx, modulated);
            }
        }

        // Per-effect dry/wet: Band effects
        for (band_idx, band) in self.effects_editor.editor.bands.iter().enumerate() {
            for (effect_idx, effect) in band.effects.iter().enumerate() {
                if let Some(modulated) = apply_dry_wet(&effect.dry_wet_macro_mapping, effect.dry_wet) {
                    self.audio.set_multiband_band_effect_dry_wet(stem, band_idx, effect_idx, modulated);
                }
            }
        }

        // Per-effect dry/wet: Post-FX effects
        for (effect_idx, effect) in self.effects_editor.editor.post_fx.iter().enumerate() {
            if let Some(modulated) = apply_dry_wet(&effect.dry_wet_macro_mapping, effect.dry_wet) {
                self.audio.set_multiband_post_fx_effect_dry_wet(stem, effect_idx, modulated);
            }
        }

        // Chain dry/wet: Pre-FX
        if let Some(modulated) = apply_dry_wet(
            &self.effects_editor.editor.pre_fx_chain_dry_wet_macro_mapping,
            self.effects_editor.editor.pre_fx_chain_dry_wet,
        ) {
            self.audio.set_multiband_pre_fx_chain_dry_wet(stem, modulated);
        }

        // Chain dry/wet: Bands
        for (band_idx, band) in self.effects_editor.editor.bands.iter().enumerate() {
            if let Some(modulated) = apply_dry_wet(&band.chain_dry_wet_macro_mapping, band.chain_dry_wet) {
                self.audio.set_multiband_band_chain_dry_wet(stem, band_idx, modulated);
            }
        }

        // Chain dry/wet: Post-FX
        if let Some(modulated) = apply_dry_wet(
            &self.effects_editor.editor.post_fx_chain_dry_wet_macro_mapping,
            self.effects_editor.editor.post_fx_chain_dry_wet,
        ) {
            self.audio.set_multiband_post_fx_chain_dry_wet(stem, modulated);
        }

        // Global dry/wet
        if let Some(modulated) = apply_dry_wet(
            &self.effects_editor.editor.global_dry_wet_macro_mapping,
            self.effects_editor.editor.global_dry_wet,
        ) {
            self.audio.set_multiband_global_dry_wet(stem, modulated);
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
        // If we have saved param values (from preset load), use ALL of them.
        // Otherwise, extract parameters from knob assignments only.
        let params: Vec<(usize, f32)> = if !effect.saved_param_values.is_empty() {
            // Use all saved param values - includes params set via plugin GUI
            effect.saved_param_values
                .iter()
                .enumerate()
                .map(|(idx, &value)| (idx, value))
                .collect()
        } else {
            // Fresh effect: only sync knob-assigned params
            effect.knob_assignments
                .iter()
                .filter_map(|assignment| {
                    assignment.param_index.map(|idx| (idx, assignment.value))
                })
                .collect()
        };

        Self {
            id: effect.id.clone(),
            source: effect.source.clone(),
            bypassed: effect.bypassed,
            params,
        }
    }
}
