//! Slice editor message handlers
//!
//! Handles: SliceEditorCellToggle, SliceEditorMuteToggle, SliceEditorStemClick,
//! SliceEditorPresetSelect, SaveSlicerPresets

use iced::Task;
use mesh_core::types::Stem;
use super::super::app::MeshCueApp;
use super::super::message::Message;

impl MeshCueApp {
    /// Handle SliceEditorCellToggle message
    pub fn handle_slice_editor_cell_toggle(&mut self, step: usize, slice: u8) -> Task<Message> {
        // Toggle cell and get sync data if changed
        let sync_data = self.collection.loaded_track.as_mut().and_then(|state| {
            if state.slice_editor.toggle_cell(step, slice) {
                // Extract data for audio sync
                state.slice_editor.selected_stem.and_then(|stem_idx| {
                    state.slice_editor.current_sequence().map(|seq| {
                        (stem_idx, seq.to_engine_sequence())
                    })
                })
            } else {
                None
            }
        });
        // Sync to audio engine (after releasing borrow)
        if let Some((stem_idx, engine_sequence)) = sync_data {
            if let Some(stem) = Stem::from_index(stem_idx) {
                self.audio.slicer_load_sequence(stem, engine_sequence);
            }
        }
        Task::none()
    }

    /// Handle SliceEditorMuteToggle message
    pub fn handle_slice_editor_mute_toggle(&mut self, step: usize) -> Task<Message> {
        // Toggle mute and get sync data if changed
        let sync_data = self.collection.loaded_track.as_mut().and_then(|state| {
            if state.slice_editor.toggle_mute(step) {
                // Extract data for audio sync
                state.slice_editor.selected_stem.and_then(|stem_idx| {
                    state.slice_editor.current_sequence().map(|seq| {
                        (stem_idx, seq.to_engine_sequence())
                    })
                })
            } else {
                None
            }
        });
        // Sync to audio engine (after releasing borrow)
        if let Some((stem_idx, engine_sequence)) = sync_data {
            if let Some(stem) = Stem::from_index(stem_idx) {
                self.audio.slicer_load_sequence(stem, engine_sequence);
            }
        }
        Task::none()
    }

    /// Handle SliceEditorStemClick message
    pub fn handle_slice_editor_stem_click(&mut self, stem_idx: usize) -> Task<Message> {
        // Toggle stem and get the new enabled state
        let enabled_change = self.collection.loaded_track.as_mut().map(|state| {
            let was_enabled = state.slice_editor.stem_enabled[stem_idx];
            state.slice_editor.click_stem(stem_idx);
            let now_enabled = state.slice_editor.stem_enabled[stem_idx];
            (was_enabled, now_enabled)
        });
        // Sync to audio engine (after releasing borrow)
        if let Some((was_enabled, now_enabled)) = enabled_change {
            if now_enabled != was_enabled {
                if let Some(stem) = Stem::from_index(stem_idx) {
                    // Set buffer bars before enabling (use config value)
                    if now_enabled {
                        let buffer_bars = self.domain.config().slicer.validated_buffer_bars();
                        self.audio.set_slicer_buffer_bars(stem, buffer_bars);
                    }
                    self.audio.set_slicer_enabled(stem, now_enabled);
                }
            }
        }
        Task::none()
    }

    /// Handle SliceEditorPresetSelect message
    pub fn handle_slice_editor_preset_select(&mut self, preset_idx: usize) -> Task<Message> {
        // Select preset and get preset data for activation
        let preset_data = self.collection.loaded_track.as_mut().map(|state| {
            state.slice_editor.select_preset(preset_idx);
            let presets = state.slice_editor.to_engine_presets();
            // Clone the selected preset's stem configuration for activation
            let stem_has_pattern: [bool; 4] = std::array::from_fn(|i| {
                state.slice_editor.presets[preset_idx].stems[i].is_some()
            });
            (presets, stem_has_pattern)
        });

        // Sync presets and activate slicer for stems with patterns
        if let Some((presets, stem_has_pattern)) = preset_data {
            let stems = [Stem::Vocals, Stem::Drums, Stem::Bass, Stem::Other];

            // Send preset data to engine
            self.audio.set_slicer_presets(presets);

            // Activate slicer for each stem that has a pattern in this preset
            // shift_held=false means "select preset" mode (enables slicer + loads pattern)
            for (idx, &stem) in stems.iter().enumerate() {
                if stem_has_pattern[idx] {
                    self.audio.slicer_button_action(stem, preset_idx as u8, false);
                }
            }
        }
        Task::none()
    }

    /// Handle SaveSlicerPresets message
    pub fn handle_save_slicer_presets(&mut self) -> Task<Message> {
        // Save current slice editor presets to dedicated slicer-presets.yaml file
        if let Some(ref state) = self.collection.loaded_track {
            // Preserve existing buffer_bars when saving presets
            let buffer_bars = self.domain.config().slicer.validated_buffer_bars();
            let slicer_config = crate::config::SlicerConfig::from_editor_state_with_buffer(
                &state.slice_editor,
                buffer_bars,
            );

            // Update in-memory config via domain
            self.domain.config_mut().slicer = slicer_config.clone();

            // Save to dedicated presets file (shared with mesh-player)
            let collection_path = self.collection.collection_path.to_path_buf();
            return Task::perform(
                async move {
                    mesh_widgets::save_slicer_presets(&slicer_config, &collection_path).ok()
                },
                |_| Message::Tick,
            );
        }
        Task::none()
    }

    /// Handle HotCuePressed message
    pub fn handle_hot_cue_pressed(&mut self, index: usize) -> Task<Message> {
        // Shift+click = delete cue point
        if self.shift_held {
            return self.handle_clear_cue_point(index);
        }

        // CDJ-style hot cue press - use audio engine
        if let Some(ref mut state) = self.collection.loaded_track {
            self.audio.hot_cue_press(index);
            // Position and state will update via atomics on next tick
            let pos = state.playhead_position();
            let cue_pos = state.cue_point();

            // Update waveform positions and cue marker
            if state.duration_samples > 0 {
                let normalized = pos as f64 / state.duration_samples as f64;
                let cue_normalized = cue_pos as f64 / state.duration_samples as f64;
                state.combined_waveform.overview.set_position(normalized);
                state.combined_waveform.overview.set_cue_position(Some(cue_normalized));
            }
            state.update_zoomed_waveform_cache(pos);
        }
        Task::none()
    }

    /// Handle HotCueReleased message
    pub fn handle_hot_cue_released(&mut self, _index: usize) -> Task<Message> {
        use mesh_core::types::PlayState;
        // Use audio engine for CDJ-style hot cue release
        if let Some(ref mut state) = self.collection.loaded_track {
            // Check if we were in preview mode BEFORE releasing
            let was_previewing = state.play_state() == PlayState::Cueing;

            self.audio.hot_cue_release();

            // Always pause audio when releasing from preview mode
            if was_previewing {
                self.audio.pause();
            }

            let pos = state.playhead_position();
            // Update waveform positions
            if state.duration_samples > 0 {
                let normalized = pos as f64 / state.duration_samples as f64;
                state.combined_waveform.overview.set_position(normalized);
            }
            state.update_zoomed_waveform_cache(pos);
        }
        Task::none()
    }
}
