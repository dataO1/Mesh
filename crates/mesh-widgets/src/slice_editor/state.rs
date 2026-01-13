//! State structures for the slice editor widget
//!
//! These structures represent the editable slicer preset patterns.
//! The UI state is separate from mesh-core's engine types but can be
//! converted to engine format when applying presets.

use mesh_core::engine::{SliceStep, SlicerPreset, StepSequence, MUTED_SLICE, MAX_SLICE_LAYERS};

/// Number of steps in a slice sequence
pub const NUM_STEPS: usize = 16;
/// Number of possible slice indices (0-15)
pub const NUM_SLICES: usize = 16;
/// Number of presets
pub const NUM_PRESETS: usize = 8;
/// Number of stems
pub const NUM_STEMS: usize = 4;

/// Stem names for display
pub const STEM_NAMES: [&str; NUM_STEMS] = ["VOC", "DRM", "BAS", "OTH"];

/// State for the slice editor widget
#[derive(Debug, Clone)]
pub struct SliceEditorState {
    /// 8 preset patterns
    pub presets: [SliceEditPreset; NUM_PRESETS],
    /// Currently selected preset (0-7)
    pub selected_preset: usize,
    /// Currently selected stem for editing (0-3: VOC, DRM, BAS, OTH)
    /// None if no stem is selected (grid shows empty)
    pub selected_stem: Option<usize>,
    /// Which stems have slicer enabled [VOC, DRM, BAS, OTH]
    pub stem_enabled: [bool; NUM_STEMS],
}

impl Default for SliceEditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl SliceEditorState {
    /// Create a new slice editor state with default presets
    pub fn new() -> Self {
        Self {
            presets: std::array::from_fn(|_| SliceEditPreset::default()),
            selected_preset: 0,
            selected_stem: Some(1), // Default: drums selected
            stem_enabled: [false, true, false, false], // Default: only drums enabled
        }
    }

    /// Get the current preset being edited
    pub fn current_preset(&self) -> &SliceEditPreset {
        &self.presets[self.selected_preset]
    }

    /// Get the current preset mutably
    pub fn current_preset_mut(&mut self) -> &mut SliceEditPreset {
        &mut self.presets[self.selected_preset]
    }

    /// Get the current stem's sequence for editing (if a stem is selected and has a pattern)
    pub fn current_sequence(&self) -> Option<&SliceEditSequence> {
        self.selected_stem.and_then(|stem_idx| {
            self.current_preset().stems[stem_idx].as_ref()
        })
    }

    /// Toggle a cell in the grid for the selected stem
    ///
    /// Returns true if the state was changed.
    pub fn toggle_cell(&mut self, step: usize, slice: u8) -> bool {
        if step >= NUM_STEPS || slice as usize >= NUM_SLICES {
            return false;
        }

        let Some(stem_idx) = self.selected_stem else {
            return false;
        };

        // Ensure the stem has a sequence
        if self.current_preset().stems[stem_idx].is_none() {
            self.current_preset_mut().stems[stem_idx] = Some(SliceEditSequence::default());
        }

        if let Some(ref mut seq) = self.current_preset_mut().stems[stem_idx] {
            let step_data = &mut seq.steps[step];

            if step_data.active_slices.contains(&slice) {
                // Remove this slice
                step_data.active_slices.retain(|&s| s != slice);
                true
            } else if step_data.active_slices.len() < MAX_SLICE_LAYERS {
                // Add this slice (if room)
                step_data.active_slices.push(slice);
                true
            } else {
                // Already at max layers, could replace oldest
                false
            }
        } else {
            false
        }
    }

    /// Toggle mute for a column (step)
    pub fn toggle_mute(&mut self, step: usize) -> bool {
        if step >= NUM_STEPS {
            return false;
        }

        let Some(stem_idx) = self.selected_stem else {
            return false;
        };

        // Ensure the stem has a sequence
        if self.current_preset().stems[stem_idx].is_none() {
            self.current_preset_mut().stems[stem_idx] = Some(SliceEditSequence::default());
        }

        if let Some(ref mut seq) = self.current_preset_mut().stems[stem_idx] {
            seq.steps[step].muted = !seq.steps[step].muted;
            true
        } else {
            false
        }
    }

    /// Handle stem button click - toggles enabled AND selects for editing
    pub fn click_stem(&mut self, stem_idx: usize) {
        if stem_idx >= NUM_STEMS {
            return;
        }

        if self.stem_enabled[stem_idx] {
            // Currently enabled - disable and deselect
            self.stem_enabled[stem_idx] = false;
            if self.selected_stem == Some(stem_idx) {
                self.selected_stem = None;
            }
        } else {
            // Currently disabled - enable and select
            self.stem_enabled[stem_idx] = true;
            self.selected_stem = Some(stem_idx);
        }
    }

    /// Select a preset tab
    pub fn select_preset(&mut self, preset_idx: usize) {
        if preset_idx < NUM_PRESETS {
            self.selected_preset = preset_idx;
        }
    }

    /// Check if a cell is active (slice is on at this step)
    pub fn is_cell_active(&self, step: usize, slice: u8) -> bool {
        self.current_sequence()
            .map(|seq| seq.steps[step].active_slices.contains(&slice))
            .unwrap_or(false)
    }

    /// Check if a step is muted
    pub fn is_step_muted(&self, step: usize) -> bool {
        self.current_sequence()
            .map(|seq| seq.steps[step].muted)
            .unwrap_or(false)
    }

    /// Check if a cell is the default position (x == y diagonal)
    pub fn is_default_position(step: usize, slice: u8) -> bool {
        step == slice as usize
    }

    /// Convert the current preset to engine format
    pub fn to_engine_preset(&self, preset_idx: usize) -> SlicerPreset {
        self.presets[preset_idx].to_engine_preset()
    }

    /// Convert all presets to engine format
    pub fn to_engine_presets(&self) -> [SlicerPreset; NUM_PRESETS] {
        std::array::from_fn(|i| self.to_engine_preset(i))
    }
}

/// A single preset pattern containing per-stem sequences
#[derive(Debug, Clone)]
pub struct SliceEditPreset {
    /// Per-stem patterns (None = bypass for this stem)
    /// Index: [VOC=0, DRM=1, BAS=2, OTH=3]
    pub stems: [Option<SliceEditSequence>; NUM_STEMS],
}

impl Default for SliceEditPreset {
    fn default() -> Self {
        Self {
            // Default: all stems have default sequential pattern
            stems: std::array::from_fn(|_| Some(SliceEditSequence::default())),
        }
    }
}

impl SliceEditPreset {
    /// Create an empty preset (all stems bypass)
    pub fn empty() -> Self {
        Self {
            stems: std::array::from_fn(|_| None),
        }
    }

    /// Convert to engine format
    pub fn to_engine_preset(&self) -> SlicerPreset {
        SlicerPreset {
            stems: std::array::from_fn(|i| {
                self.stems[i].as_ref().map(|seq| seq.to_engine_sequence())
            }),
        }
    }
}

/// Editable step sequence for one stem
#[derive(Debug, Clone)]
pub struct SliceEditSequence {
    /// 16 steps, each with muted flag + active slices
    pub steps: [SliceEditStep; NUM_STEPS],
}

impl Default for SliceEditSequence {
    fn default() -> Self {
        // Default: diagonal pattern (step N plays slice N)
        Self {
            steps: std::array::from_fn(|i| SliceEditStep {
                muted: false,
                active_slices: vec![i as u8],
            }),
        }
    }
}

impl SliceEditSequence {
    /// Create an empty sequence (all steps have no active slices)
    pub fn empty() -> Self {
        Self {
            steps: std::array::from_fn(|_| SliceEditStep::default()),
        }
    }

    /// Convert to engine format
    pub fn to_engine_sequence(&self) -> StepSequence {
        StepSequence {
            steps: std::array::from_fn(|i| {
                let step = &self.steps[i];
                if step.muted {
                    // Muted step
                    SliceStep {
                        slices: [MUTED_SLICE, MUTED_SLICE],
                        velocities: [0.0, 0.0],
                    }
                } else if step.active_slices.is_empty() {
                    // No slices = muted
                    SliceStep {
                        slices: [MUTED_SLICE, MUTED_SLICE],
                        velocities: [0.0, 0.0],
                    }
                } else {
                    // Active slices
                    let slice0 = step.active_slices.get(0).copied().unwrap_or(MUTED_SLICE);
                    let slice1 = step.active_slices.get(1).copied().unwrap_or(MUTED_SLICE);
                    let vel0 = if slice0 != MUTED_SLICE { 1.0 } else { 0.0 };
                    let vel1 = if slice1 != MUTED_SLICE { 1.0 } else { 0.0 };
                    SliceStep {
                        slices: [slice0, slice1],
                        velocities: [vel0, vel1],
                    }
                }
            }),
        }
    }
}

/// Single step in the sequence (UI representation)
#[derive(Debug, Clone, Default)]
pub struct SliceEditStep {
    /// Whether this step is muted
    pub muted: bool,
    /// Active slices at this step (0-15)
    /// Multiple values = layered slices (up to MAX_SLICE_LAYERS)
    pub active_slices: Vec<u8>,
}
