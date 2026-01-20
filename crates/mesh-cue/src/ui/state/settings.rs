//! Settings modal state

use crate::audio::StereoPair;
use crate::config::{BpmSource, Config};

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
    /// Available JACK stereo output pairs
    pub available_stereo_pairs: Vec<StereoPair>,
    /// Selected output pair index
    pub selected_output_pair: usize,
    /// Status message for save feedback
    pub status: String,
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
            selected_output_pair: 0,
            status: String::new(),
        }
    }

    /// Refresh available JACK ports
    pub fn refresh_jack_ports(&mut self) {
        self.available_stereo_pairs = crate::audio::get_available_stereo_pairs();
        // Keep selection in bounds
        if self.selected_output_pair >= self.available_stereo_pairs.len() {
            self.selected_output_pair = 0;
        }
    }
}
