//! Multiband Effect Editor Widget
//!
//! A reusable widget for editing multiband effect containers.
//! Provides UI for:
//! - Crossover frequency adjustment (draggable dividers)
//! - Per-band effect chains with add/remove
//! - 8 macro knobs with routing
//! - Preset save/load
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │  [Load Preset ▾]  [Save Preset]           Deck 1 - Drums         [×]   │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Crossover Bar: 20Hz ═══════╪═══════════╪════════════════════ 20kHz    │
//! │                           200Hz        2kHz        (draggable)         │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Band 1: 20-200Hz (Sub)    [S] [M]                                     │
//! │   ┌──────────┐ ┌──────────┐                                            │
//! │   │ Effect 1 │ │ Effect 2 │  [+]                                       │
//! │   │ ○○○○○○○○ │ │ ○○○○○○○○ │                                            │
//! │   └──────────┘ └──────────┘                                            │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Band 2: 200Hz-2kHz (Mid)  [S] [M]                                     │
//! │   ... more bands ...                                                   │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │  Macros: [1:____] [2:____] [3:____] [4:____] ...                       │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

pub mod config;
mod crossover_bar;
mod message;
mod state;
mod view;

pub use config::{
    delete_preset, list_presets, load_preset, multiband_presets_folder, save_preset,
    MultibandPresetConfig,
};
pub use crossover_bar::{crossover_bar, crossover_controls, CROSSOVER_BAR_HEIGHT};
pub use message::MultibandEditorMessage;
pub use state::{
    AvailableParam, BandUiState, EffectChainLocation, EffectSourceType, EffectUiState,
    KnobAssignment, MacroUiState, MultibandEditorState, ParamMacroMapping, MAX_UI_KNOBS,
};
pub use view::{multiband_editor, multiband_editor_content, ensure_effect_knobs_exist};

/// Frequency range for crossover display (Hz)
pub const FREQ_MIN: f32 = 20.0;
pub const FREQ_MAX: f32 = 20000.0;

/// Number of macro knobs
pub const NUM_MACROS: usize = 8;

/// Default band names based on frequency ranges
pub fn default_band_name(_freq_low: f32, freq_high: f32) -> &'static str {
    if freq_high <= 80.0 {
        "Sub"
    } else if freq_high <= 250.0 {
        "Bass"
    } else if freq_high <= 500.0 {
        "Low-Mid"
    } else if freq_high <= 2000.0 {
        "Mid"
    } else if freq_high <= 6000.0 {
        "High-Mid"
    } else if freq_high <= 12000.0 {
        "Presence"
    } else {
        "Air"
    }
}

/// Convert frequency to position (0.0-1.0) on log scale
pub fn freq_to_position(freq: f32) -> f32 {
    let log_min = FREQ_MIN.log10();
    let log_max = FREQ_MAX.log10();
    let log_freq = freq.clamp(FREQ_MIN, FREQ_MAX).log10();
    (log_freq - log_min) / (log_max - log_min)
}

/// Convert position (0.0-1.0) to frequency on log scale
pub fn position_to_freq(pos: f32) -> f32 {
    let log_min = FREQ_MIN.log10();
    let log_max = FREQ_MAX.log10();
    let log_freq = log_min + pos.clamp(0.0, 1.0) * (log_max - log_min);
    10.0_f32.powf(log_freq)
}

/// Format frequency for display
pub fn format_freq(freq: f32) -> String {
    if freq >= 1000.0 {
        format!("{:.1}kHz", freq / 1000.0)
    } else {
        format!("{:.0}Hz", freq)
    }
}
