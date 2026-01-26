//! Settings modal message handlers
//!
//! Handles: OpenSettings, CloseSettings, UpdateSettings*, SaveSettings, SaveSettingsComplete

use iced::Task;
use super::super::app::MeshCueApp;
use super::super::message::Message;
use super::super::state::SettingsState;
use crate::config;

impl MeshCueApp {
    /// Handle OpenSettings message
    pub fn handle_open_settings(&mut self) -> Task<Message> {
        // Reset draft values from current config
        self.settings = SettingsState::from_config(self.domain.config());
        self.settings.is_open = true;
        // Refresh available audio devices
        self.settings.refresh_audio_devices();
        Task::none()
    }

    /// Handle CloseSettings message
    pub fn handle_close_settings(&mut self) -> Task<Message> {
        self.settings.is_open = false;
        self.settings.status.clear();
        Task::none()
    }

    /// Handle UpdateSettingsMinTempo message
    pub fn handle_update_settings_min_tempo(&mut self, value: String) -> Task<Message> {
        self.settings.draft_min_tempo = value;
        Task::none()
    }

    /// Handle UpdateSettingsMaxTempo message
    pub fn handle_update_settings_max_tempo(&mut self, value: String) -> Task<Message> {
        self.settings.draft_max_tempo = value;
        Task::none()
    }

    /// Handle UpdateSettingsParallelProcesses message
    pub fn handle_update_settings_parallel_processes(&mut self, value: String) -> Task<Message> {
        self.settings.draft_parallel_processes = value;
        Task::none()
    }

    /// Handle UpdateSettingsTrackNameFormat message
    pub fn handle_update_settings_track_name_format(&mut self, value: String) -> Task<Message> {
        self.settings.draft_track_name_format = value;
        Task::none()
    }

    /// Handle UpdateSettingsGridBars message
    pub fn handle_update_settings_grid_bars(&mut self, value: u32) -> Task<Message> {
        self.settings.draft_grid_bars = value;
        Task::none()
    }

    /// Handle UpdateSettingsBpmSource message
    pub fn handle_update_settings_bpm_source(&mut self, source: crate::config::BpmSource) -> Task<Message> {
        self.settings.draft_bpm_source = source;
        Task::none()
    }

    /// Handle UpdateSettingsSlicerBufferBars message
    pub fn handle_update_settings_slicer_buffer_bars(&mut self, bars: u32) -> Task<Message> {
        self.settings.draft_slicer_buffer_bars = bars;
        Task::none()
    }

    /// Handle UpdateSettingsOutputPair message
    ///
    /// Updates the draft selection. The actual device change is applied on Save.
    pub fn handle_update_settings_output_pair(&mut self, idx: usize) -> Task<Message> {
        self.settings.selected_output_pair = idx;
        log::info!(
            "Audio device selected: {:?}",
            self.settings.available_stereo_pairs.get(idx).map(|p| &p.label)
        );
        Task::none()
    }

    /// Handle UpdateSettingsScratchInterpolation message
    ///
    /// Applies the interpolation change immediately for instant feedback.
    pub fn handle_update_settings_scratch_interpolation(
        &mut self,
        method: mesh_core::engine::InterpolationMethod,
    ) -> Task<Message> {
        self.settings.draft_scratch_interpolation = method;
        // Apply immediately for instant feedback (don't wait for save)
        self.audio.set_scratch_interpolation(method);
        log::info!("Scratch interpolation changed to {:?}", method);
        Task::none()
    }

    /// Handle RefreshAudioDevices message
    pub fn handle_refresh_audio_devices(&mut self) -> Task<Message> {
        self.settings.refresh_audio_devices();
        Task::none()
    }

    /// Handle SaveSettings message
    pub fn handle_save_settings(&mut self) -> Task<Message> {
        // Parse and validate values
        let min = self.settings.draft_min_tempo.parse::<i32>().unwrap_or(40);
        let max = self.settings.draft_max_tempo.parse::<i32>().unwrap_or(208);
        let parallel = self.settings.draft_parallel_processes.parse::<u8>().unwrap_or(4);

        // Check if audio output device changed
        let old_device = self.domain.config().audio.output_device;
        let new_device = Some(self.settings.selected_output_pair);
        let audio_changed = old_device != new_device;

        // Update config via domain
        {
            let config = self.domain.config_mut();
            config.analysis.bpm.min_tempo = min;
            config.analysis.bpm.max_tempo = max;
            config.analysis.bpm.source = self.settings.draft_bpm_source;
            config.analysis.parallel_processes = parallel;
            config.analysis.validate(); // validates both bpm and parallel_processes

            // Update track name format
            config.track_name_format = self.settings.draft_track_name_format.clone();

            // Update display settings (grid bars)
            config.display.grid_bars = self.settings.draft_grid_bars;

            // Update slicer buffer bars
            config.slicer.buffer_bars = self.settings.draft_slicer_buffer_bars;

            // Update audio output device
            config.audio.output_device = new_device;

            // Update scratch interpolation
            config.audio.scratch_interpolation = self.settings.draft_scratch_interpolation;

            // Update drafts to show validated values
            self.settings.draft_min_tempo = config.analysis.bpm.min_tempo.to_string();
            self.settings.draft_max_tempo = config.analysis.bpm.max_tempo.to_string();
            self.settings.draft_parallel_processes = config.analysis.parallel_processes.to_string();
        }

        // Apply scratch interpolation to audio engine immediately
        self.audio.set_scratch_interpolation(self.settings.draft_scratch_interpolation);

        // Hot-swap audio output if device changed
        if audio_changed {
            // mesh-cue uses master-only mode, so pass the same device for both master and cue
            let success = crate::audio::reconnect_ports(
                "mesh-player",  // JACK client name (shared with mesh-player in mesh-core)
                new_device,
                new_device,  // Same device for cue in master-only mode
            );
            if !success {
                // CPAL backend requires restart
                self.settings.status = "Audio device changed. Restart app to apply.".to_string();
            }
        }

        // Save to file
        let config_path = self.domain.config_path().to_path_buf();
        let config_clone = self.domain.config().clone();

        Task::perform(
            async move {
                config::save_config(&config_clone, &config_path)
                    .map_err(|e| e.to_string())
            },
            Message::SaveSettingsComplete,
        )
    }

    /// Handle SaveSettingsComplete message
    pub fn handle_save_settings_complete(&mut self, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => {
                log::info!("Settings saved successfully");
                self.settings.status = String::from("Settings saved!");
                // Close modal after brief delay would be nice, for now just close
                self.settings.is_open = false;
            }
            Err(e) => {
                log::error!("Failed to save settings: {}", e);
                self.settings.status = format!("Failed to save: {}", e);
            }
        }
        Task::none()
    }
}
