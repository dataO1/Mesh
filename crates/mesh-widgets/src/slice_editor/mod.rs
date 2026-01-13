//! Slice preset editor widget
//!
//! A visual editor for slicer patterns with a 16x16 MIDI-style grid.
//! Supports per-stem patterns, 8 presets, and layered slices.
//!
//! ## Architecture
//!
//! Uses native iced buttons instead of Canvas to avoid iced bug #3040
//! where multiple Canvas widgets don't render properly together.
//!
//! ## Layout
//!
//! ```text
//! [1] [2] [3] [4] [5] [6] [7] [8]   <- Preset tabs
//! [VOC] [M] [M] [M] ... [M]        <- Mute row (16 buttons)
//! [DRM] [■] [□] [□] ... [□]        <- Grid row 15 (top)
//! [BAS] [□] [■] [□] ... [□]        <- Grid row 14
//! [OTH] [□] [□] [■] ... [□]        <- ...
//!       ...                        <- Grid rows 13-1
//!       [□] [□] [□] ... [■]        <- Grid row 0 (bottom)
//! ```
//!
//! - X axis (columns): queue slot 0-15
//! - Y axis (rows): possible slice indices 0-15, origin at bottom-left
//! - Black cells: slice is ON at that position
//! - White cells: default diagonal position (x=y)
//! - Gray cells: muted column

pub mod config;
pub mod state;
pub mod view;

pub use config::{
    SlicerConfig, SlicerPresetConfig, SlicerSequenceConfig, SlicerStepConfig,
    // Shared presets file I/O
    load_slicer_presets, save_slicer_presets, slicer_presets_path, SLICER_PRESETS_FILENAME,
};
pub use state::{
    SliceEditPreset, SliceEditSequence, SliceEditStep, SliceEditorState,
    NUM_PRESETS, NUM_SLICES, NUM_STEMS, NUM_STEPS, STEM_NAMES,
};
pub use view::slice_editor;
