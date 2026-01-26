//! Settings modal state

use crate::audio::StereoPair;
use crate::config::{BackendType, BpmSource, Config, ModelType};
use mesh_core::engine::InterpolationMethod;

/// State for the settings modal
#[derive(Debug, Default)]
pub struct SettingsState {
    /// Whether the settings modal is open
    pub is_open: bool,
    /// Draft min tempo value (text input)
    pub draft_min_tempo: String,
    /// Draft max tempo value (text input)
    pub draft_max_tempo: String,
    /// Draft parallel processes value (text input, 1-16)
    pub draft_parallel_processes: String,
    /// Draft track name format template
    pub draft_track_name_format: String,
    /// Draft grid bars value (4, 8, 16, 32)
    pub draft_grid_bars: u32,
    /// Draft BPM source for analysis (drums-only or full mix)
    pub draft_bpm_source: BpmSource,
    /// Draft slicer buffer bars (1, 4, 8, or 16)
    pub draft_slicer_buffer_bars: u32,
    /// Available audio output devices (for display only in CPAL mode)
    pub available_stereo_pairs: Vec<StereoPair>,
    /// Selected output pair index (for future device selection)
    pub selected_output_pair: usize,
    /// Draft scratch interpolation method
    pub draft_scratch_interpolation: InterpolationMethod,
    /// Status message for save feedback
    pub status: String,
    // ── Separation Settings ──────────────────────────────────────────────────
    /// Draft separation backend type
    pub draft_separation_backend: BackendType,
    /// Draft separation model type
    pub draft_separation_model: ModelType,
    /// Draft GPU acceleration flag
    pub draft_separation_use_gpu: bool,
}

impl SettingsState {
    /// Initialize from current config
    pub fn from_config(config: &Config) -> Self {
        Self {
            is_open: false,
            draft_min_tempo: config.analysis.bpm.min_tempo.to_string(),
            draft_max_tempo: config.analysis.bpm.max_tempo.to_string(),
            draft_parallel_processes: config.analysis.parallel_processes.to_string(),
            draft_track_name_format: config.track_name_format.clone(),
            draft_grid_bars: config.display.grid_bars,
            draft_bpm_source: config.analysis.bpm.source,
            draft_slicer_buffer_bars: config.slicer.validated_buffer_bars(),
            available_stereo_pairs: Vec::new(),
            selected_output_pair: config.audio.output_device.unwrap_or(0),
            draft_scratch_interpolation: config.audio.scratch_interpolation,
            status: String::new(),
            // Separation settings
            draft_separation_backend: config.analysis.separation.backend,
            draft_separation_model: config.analysis.separation.model,
            draft_separation_use_gpu: config.analysis.separation.use_gpu,
        }
    }

    /// Refresh available audio devices
    ///
    /// Note: In CPAL mode, device selection requires restarting the audio system.
    /// This is informational only - actual device selection is not yet implemented.
    pub fn refresh_audio_devices(&mut self) {
        self.available_stereo_pairs = crate::audio::get_available_stereo_pairs();
        // Keep selection in bounds
        if self.selected_output_pair >= self.available_stereo_pairs.len() {
            self.selected_output_pair = 0;
        }
    }
}
