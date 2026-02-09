//! Effect picker message handlers
//!
//! Handles effect selection for adding effects to multiband chains.

use iced::Task;
use mesh_widgets::multiband::{AvailableParam, EffectSourceType, EffectUiState};

use crate::ui::app::MeshCueApp;
use crate::ui::effect_picker::{EffectPickerMessage, EffectPickerTarget};
use crate::ui::message::Message;

impl MeshCueApp {
    /// Handle an effect picker message
    pub fn handle_effect_picker(&mut self, msg: EffectPickerMessage) -> Task<Message> {
        use EffectPickerMessage::*;

        match msg {
            Open { target } => {
                match target {
                    EffectPickerTarget::PreFx => self.effect_picker.open_pre_fx(),
                    EffectPickerTarget::Band(idx) => self.effect_picker.open_for_band(idx),
                    EffectPickerTarget::PostFx => self.effect_picker.open_post_fx(),
                }
            }
            Close => {
                self.effect_picker.close();
            }
            SelectPdEffect(effect_id) => {
                let target = self.effect_picker.target;

                // Look up the effect to get its metadata, extract what we need
                let effect_data = self.domain.get_effect(&effect_id).map(|effect| {
                    let available_params: Vec<AvailableParam> = effect
                        .metadata
                        .params
                        .iter()
                        .map(|p| AvailableParam {
                            name: p.name.clone(),
                            min: p.min.unwrap_or(0.0),
                            max: p.max.unwrap_or(1.0),
                            default: p.default,
                            unit: p.unit.clone().unwrap_or_default(),
                        })
                        .collect();
                    (
                        effect.metadata.name.clone(),
                        effect.metadata.category.clone(),
                        available_params,
                    )
                });

                if let Some((name, category, available_params)) = effect_data {
                    // Create the effect UI state
                    let effect_state = EffectUiState::new_with_params(
                        effect_id.clone(),
                        name.clone(),
                        category,
                        EffectSourceType::Pd,
                        available_params,
                    );

                    // Add to the appropriate location
                    self.add_effect_to_target(target, effect_state);

                    // Ensure knobs exist for the new effect
                    mesh_widgets::multiband::ensure_effect_knobs_exist(&mut self.effects_editor.editor);

                    log::info!("Added PD effect '{}' to {:?}", effect_id, target);
                    self.effects_editor.set_status(format!("Added '{}'", name));
                } else {
                    log::warn!("PD effect '{}' not found", effect_id);
                    self.effects_editor.set_status(format!("Effect '{}' not found", effect_id));
                }

                self.effect_picker.close();
            }
            SelectClapEffect(plugin_id) => {
                let target = self.effect_picker.target;

                // Calculate effect index for instance ID
                let effect_idx = match target {
                    EffectPickerTarget::PreFx => self.effects_editor.editor.pre_fx.len(),
                    EffectPickerTarget::Band(idx) => {
                        self.effects_editor.editor.bands
                            .get(idx)
                            .map(|b| b.effects.len())
                            .unwrap_or(0)
                    }
                    EffectPickerTarget::PostFx => self.effects_editor.editor.post_fx.len(),
                };

                // Generate effect instance ID
                let effect_instance_id = match target {
                    EffectPickerTarget::PreFx => format!("{}_cue_prefx_{}", plugin_id, effect_idx),
                    EffectPickerTarget::Band(band_idx) => format!("{}_cue_b{}_{}", plugin_id, band_idx, effect_idx),
                    EffectPickerTarget::PostFx => format!("{}_cue_postfx_{}", plugin_id, effect_idx),
                };

                // Create the CLAP effect with GUI support to get actual params
                match self.domain.create_clap_effect_with_gui(&plugin_id, effect_instance_id) {
                    Ok(effect) => {
                        // Extract info before potentially moving the effect
                        let effect_info = effect.info();
                        let effect_name = effect_info.name.clone();
                        let effect_category = effect_info.category.clone();
                        let available_params: Vec<AvailableParam> = effect_info
                            .params
                            .iter()
                            .map(|p| AvailableParam {
                                name: p.name.clone(),
                                min: p.min,
                                max: p.max,
                                default: p.default,
                                unit: p.unit.clone(),
                            })
                            .collect();

                        log::info!(
                            "Created CLAP plugin '{}' with {} params",
                            plugin_id,
                            available_params.len()
                        );

                        // Create UI state with actual params
                        let effect_state = EffectUiState::new_with_params(
                            plugin_id.clone(),
                            effect_name.clone(),
                            effect_category,
                            EffectSourceType::Clap,
                            available_params,
                        );

                        // If audio preview is enabled, add to audio engine
                        if self.effects_editor.audio_preview_enabled {
                            let stem = self.effects_editor.active_stem_type();
                            match target {
                                EffectPickerTarget::PreFx => {
                                    self.audio.add_multiband_pre_fx(stem, effect);
                                }
                                EffectPickerTarget::Band(idx) => {
                                    self.audio.add_multiband_band_effect(stem, idx, effect);
                                }
                                EffectPickerTarget::PostFx => {
                                    self.audio.add_multiband_post_fx(stem, effect);
                                }
                            }
                        }

                        // Add to UI state
                        match target {
                            EffectPickerTarget::PreFx => {
                                self.effects_editor.editor.pre_fx.push(effect_state);
                            }
                            EffectPickerTarget::Band(idx) => {
                                if let Some(band) = self.effects_editor.editor.bands.get_mut(idx) {
                                    band.effects.push(effect_state);
                                }
                            }
                            EffectPickerTarget::PostFx => {
                                self.effects_editor.editor.post_fx.push(effect_state);
                            }
                        }

                        // Ensure knobs exist for the new effect
                        mesh_widgets::multiband::ensure_effect_knobs_exist(&mut self.effects_editor.editor);

                        self.effects_editor.set_status(format!("Added '{}'", effect_name));
                    }
                    Err(e) => {
                        log::error!("Failed to create CLAP plugin '{}': {}", plugin_id, e);
                        self.effects_editor.set_status(format!("Failed to create plugin: {}", e));
                    }
                }

                self.effect_picker.close();
            }
            ToggleSourceFilter(filter) => {
                self.effect_picker.source_filter = filter;
            }
        }

        Task::none()
    }

    /// Add an effect to the target location in the effects editor
    ///
    /// Also adds the effect to the audio engine if preview is enabled.
    fn add_effect_to_target(&mut self, target: EffectPickerTarget, effect: EffectUiState) {
        // Calculate effect index (position in the target chain)
        let effect_idx = match target {
            EffectPickerTarget::PreFx => self.effects_editor.editor.pre_fx.len(),
            EffectPickerTarget::Band(idx) => {
                self.effects_editor.editor.bands
                    .get(idx)
                    .map(|b| b.effects.len())
                    .unwrap_or(0)
            }
            EffectPickerTarget::PostFx => self.effects_editor.editor.post_fx.len(),
        };

        // If audio preview is enabled, also add to the audio engine
        if self.effects_editor.audio_preview_enabled {
            let stem = self.effects_editor.active_stem_type();

            // Create an audio effect instance (with GUI support for CLAP)
            if let Some(audio_effect) = self.create_audio_effect(&effect, target, effect_idx) {
                match target {
                    EffectPickerTarget::PreFx => {
                        self.audio.add_multiband_pre_fx(stem, audio_effect);
                    }
                    EffectPickerTarget::Band(idx) => {
                        self.audio.add_multiband_band_effect(stem, idx, audio_effect);
                    }
                    EffectPickerTarget::PostFx => {
                        self.audio.add_multiband_post_fx(stem, audio_effect);
                    }
                }
            }
        }

        // Add to editor UI state
        match target {
            EffectPickerTarget::PreFx => {
                self.effects_editor.editor.pre_fx.push(effect);
            }
            EffectPickerTarget::Band(idx) => {
                if let Some(band) = self.effects_editor.editor.bands.get_mut(idx) {
                    band.effects.push(effect);
                }
            }
            EffectPickerTarget::PostFx => {
                self.effects_editor.editor.post_fx.push(effect);
            }
        }
    }

    /// Create an audio effect instance from effect UI state
    ///
    /// Uses the domain's effect managers (PD or CLAP) to instantiate effects.
    /// For CLAP effects, stores the GUI handle for plugin window support.
    fn create_audio_effect(
        &mut self,
        effect_ui: &EffectUiState,
        target: EffectPickerTarget,
        effect_idx: usize,
    ) -> Option<Box<dyn mesh_core::effect::Effect>> {
        match &effect_ui.source {
            EffectSourceType::Pd => {
                self.domain.create_pd_effect(&effect_ui.id).ok()
            }
            EffectSourceType::Clap => {
                // Generate effect instance ID for mesh-cue (matches handler's ID format)
                let effect_instance_id = match target {
                    EffectPickerTarget::PreFx => format!("{}_cue_prefx_{}", effect_ui.id, effect_idx),
                    EffectPickerTarget::Band(band_idx) => format!("{}_cue_b{}_{}", effect_ui.id, band_idx, effect_idx),
                    EffectPickerTarget::PostFx => format!("{}_cue_postfx_{}", effect_ui.id, effect_idx),
                };

                // Create with GUI support so we can open plugin windows
                self.domain.create_clap_effect_with_gui(&effect_ui.id, effect_instance_id).ok()
            }
            EffectSourceType::Native => {
                // Native effects not supported in presets yet
                None
            }
        }
    }
}
