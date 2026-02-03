//! State structures for the multiband editor widget

use mesh_core::effect::{BandEffectInfo, BandState};

/// Effect source type for display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectSourceType {
    /// Pure Data effect
    Pd,
    /// CLAP plugin
    Clap,
    /// Native Rust effect
    Native,
}

impl std::fmt::Display for EffectSourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pd => write!(f, "PD"),
            Self::Clap => write!(f, "CLAP"),
            Self::Native => write!(f, "Native"),
        }
    }
}

/// A mapping from a macro knob to an effect parameter
#[derive(Debug, Clone, PartialEq)]
pub struct ParamMacroMapping {
    /// Which macro (0-7) controls this param, None if unmapped
    pub macro_index: Option<usize>,
    /// Min value when macro is at 0
    pub min_value: f32,
    /// Max value when macro is at 1
    pub max_value: f32,
}

impl Default for ParamMacroMapping {
    fn default() -> Self {
        Self {
            macro_index: None,
            min_value: 0.0,
            max_value: 1.0,
        }
    }
}

/// UI state for a single effect in a band
#[derive(Debug, Clone)]
pub struct EffectUiState {
    /// Effect identifier (for recreation from preset)
    pub id: String,
    /// Effect display name
    pub name: String,
    /// Effect category
    pub category: String,
    /// Effect source type
    pub source: EffectSourceType,
    /// Whether the effect is bypassed
    pub bypassed: bool,
    /// Parameter names (up to 8)
    pub param_names: Vec<String>,
    /// Current parameter values (normalized 0.0-1.0)
    pub param_values: Vec<f32>,
    /// Macro mappings for each parameter (which macro controls it)
    pub param_mappings: Vec<ParamMacroMapping>,
}

impl EffectUiState {
    /// Create from backend BandEffectInfo
    pub fn from_backend(id: String, source: EffectSourceType, info: &BandEffectInfo) -> Self {
        let param_count = info.param_values.len();
        Self {
            id,
            name: info.name.clone(),
            category: info.category.clone(),
            source,
            bypassed: info.bypassed,
            param_names: info.param_names.clone(),
            param_values: info.param_values.clone(),
            param_mappings: vec![ParamMacroMapping::default(); param_count],
        }
    }

    /// Get a short name for compact display (max 10 chars)
    pub fn short_name(&self) -> &str {
        if self.name.len() <= 10 {
            &self.name
        } else {
            &self.name[..10]
        }
    }
}

/// UI state for a single frequency band
#[derive(Debug, Clone)]
pub struct BandUiState {
    /// Band index (0-7)
    pub index: usize,
    /// Low frequency bound (Hz)
    pub freq_low: f32,
    /// High frequency bound (Hz)
    pub freq_high: f32,
    /// Band gain (linear, 0.0-2.0)
    pub gain: f32,
    /// Whether this band is muted
    pub muted: bool,
    /// Whether this band is soloed
    pub soloed: bool,
    /// Effects in this band's chain
    pub effects: Vec<EffectUiState>,
}

impl BandUiState {
    /// Create a new band UI state
    pub fn new(index: usize, freq_low: f32, freq_high: f32) -> Self {
        Self {
            index,
            freq_low,
            freq_high,
            gain: 1.0,
            muted: false,
            soloed: false,
            effects: Vec::new(),
        }
    }

    /// Update from backend BandState
    pub fn update_from_backend(&mut self, state: &BandState) {
        self.gain = state.gain;
        self.muted = state.muted;
        self.soloed = state.soloed;
    }

    /// Get the band name based on frequency range
    pub fn name(&self) -> &'static str {
        super::default_band_name(self.freq_low, self.freq_high)
    }

    /// Get frequency range as formatted string
    pub fn freq_range_str(&self) -> String {
        format!(
            "{} - {}",
            super::format_freq(self.freq_low),
            super::format_freq(self.freq_high)
        )
    }
}

/// State for a macro knob
#[derive(Debug, Clone)]
pub struct MacroUiState {
    /// Macro index (0-7)
    pub index: usize,
    /// Display name
    pub name: String,
    /// Current value (normalized 0.0-1.0)
    pub value: f32,
    /// Number of mappings to effect parameters
    pub mapping_count: usize,
}

impl MacroUiState {
    /// Create a new macro UI state with default name
    pub fn new(index: usize) -> Self {
        Self {
            index,
            name: format!("Macro {}", index + 1),
            value: 0.5,
            mapping_count: 0,
        }
    }
}

/// Complete state for the multiband editor widget
#[derive(Debug, Clone)]
pub struct MultibandEditorState {
    /// Whether the editor modal is open
    pub is_open: bool,

    /// Target deck index (0-3)
    pub deck: usize,

    /// Target stem index (0-3)
    pub stem: usize,

    /// Stem name for display
    pub stem_name: String,

    /// Crossover frequencies (N-1 for N bands)
    pub crossover_freqs: Vec<f32>,

    /// Which crossover divider is being dragged (index)
    pub dragging_crossover: Option<usize>,

    /// Which macro is being dragged for mapping (index)
    pub dragging_macro: Option<usize>,

    /// Band states
    pub bands: Vec<BandUiState>,

    /// Currently selected effect for parameter focus
    /// (band_index, effect_index)
    pub selected_effect: Option<(usize, usize)>,

    /// Macro knob states
    pub macros: Vec<MacroUiState>,

    /// Whether the preset browser is open
    pub preset_browser_open: bool,

    /// Available preset names
    pub available_presets: Vec<String>,

    /// Whether any band is soloed (for solo logic display)
    pub any_soloed: bool,
}

impl Default for MultibandEditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl MultibandEditorState {
    /// Create a new multiband editor state (closed, single band)
    pub fn new() -> Self {
        Self {
            is_open: false,
            deck: 0,
            stem: 0,
            stem_name: "Vocals".to_string(),
            crossover_freqs: Vec::new(),
            dragging_crossover: None,
            dragging_macro: None,
            bands: vec![BandUiState::new(0, super::FREQ_MIN, super::FREQ_MAX)],
            selected_effect: None,
            macros: (0..super::NUM_MACROS).map(MacroUiState::new).collect(),
            preset_browser_open: false,
            available_presets: Vec::new(),
            any_soloed: false,
        }
    }

    /// Open the editor for a specific deck and stem
    pub fn open(&mut self, deck: usize, stem: usize, stem_name: &str) {
        self.is_open = true;
        self.deck = deck;
        self.stem = stem;
        self.stem_name = stem_name.to_string();
        self.selected_effect = None;
        self.preset_browser_open = false;
    }

    /// Close the editor
    pub fn close(&mut self) {
        self.is_open = false;
        self.dragging_crossover = None;
    }

    /// Get the number of bands
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// Update band frequency ranges from crossover frequencies
    pub fn update_band_frequencies(&mut self) {
        let num_bands = self.bands.len();

        for (i, band) in self.bands.iter_mut().enumerate() {
            band.freq_low = if i == 0 {
                super::FREQ_MIN
            } else {
                self.crossover_freqs[i - 1]
            };

            band.freq_high = if i == num_bands - 1 {
                super::FREQ_MAX
            } else {
                self.crossover_freqs[i]
            };
        }
    }

    /// Add a new band (splits the last band)
    pub fn add_band(&mut self) {
        if self.bands.len() >= 8 {
            return;
        }

        let new_index = self.bands.len();

        // Calculate new crossover frequency (logarithmic midpoint of last band)
        let last_band = self.bands.last().unwrap();
        let log_mid = (last_band.freq_low.log10() + last_band.freq_high.log10()) / 2.0;
        let new_crossover = 10.0_f32.powf(log_mid);

        self.crossover_freqs.push(new_crossover);
        self.bands.push(BandUiState::new(new_index, new_crossover, last_band.freq_high));

        self.update_band_frequencies();
    }

    /// Remove a band by index
    pub fn remove_band(&mut self, index: usize) {
        if self.bands.len() <= 1 || index >= self.bands.len() {
            return;
        }

        self.bands.remove(index);

        // Remove the corresponding crossover frequency
        if !self.crossover_freqs.is_empty() {
            let freq_index = index.min(self.crossover_freqs.len() - 1);
            self.crossover_freqs.remove(freq_index);
        }

        // Update band indices
        for (i, band) in self.bands.iter_mut().enumerate() {
            band.index = i;
        }

        self.update_band_frequencies();
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
    }

    /// Set a crossover frequency
    pub fn set_crossover_freq(&mut self, index: usize, freq: f32) {
        if index >= self.crossover_freqs.len() {
            return;
        }

        // Clamp to valid range (must be between adjacent crossovers)
        let min_freq = if index == 0 {
            super::FREQ_MIN + 10.0
        } else {
            self.crossover_freqs[index - 1] + 10.0
        };

        let max_freq = if index == self.crossover_freqs.len() - 1 {
            super::FREQ_MAX - 10.0
        } else {
            self.crossover_freqs[index + 1] - 10.0
        };

        self.crossover_freqs[index] = freq.clamp(min_freq, max_freq);
        self.update_band_frequencies();
    }

    /// Set band mute state
    pub fn set_band_mute(&mut self, index: usize, muted: bool) {
        if let Some(band) = self.bands.get_mut(index) {
            band.muted = muted;
        }
    }

    /// Set band solo state
    pub fn set_band_solo(&mut self, index: usize, soloed: bool) {
        if let Some(band) = self.bands.get_mut(index) {
            band.soloed = soloed;
        }
        self.any_soloed = self.bands.iter().any(|b| b.soloed);
    }

    /// Set effect bypass state
    pub fn set_effect_bypass(&mut self, band_index: usize, effect_index: usize, bypassed: bool) {
        if let Some(band) = self.bands.get_mut(band_index) {
            if let Some(effect) = band.effects.get_mut(effect_index) {
                effect.bypassed = bypassed;
            }
        }
    }

    /// Set macro value
    pub fn set_macro_value(&mut self, index: usize, value: f32) {
        if let Some(macro_state) = self.macros.get_mut(index) {
            macro_state.value = value.clamp(0.0, 1.0);
        }
    }

    /// Set macro name
    pub fn set_macro_name(&mut self, index: usize, name: String) {
        if let Some(macro_state) = self.macros.get_mut(index) {
            macro_state.name = name;
        }
    }
}
