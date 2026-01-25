//! Shared UI widgets for mesh audio applications
//!
//! This crate provides reusable iced widgets and utilities for waveform display,
//! audio visualization, and DJ-style controls.
//!
//! ## Architecture (iced 0.14 patterns)
//!
//! Following idiomatic iced patterns:
//!
//! - **State structs**: Pure data (`OverviewState`, `ZoomedState`, `CombinedState`)
//! - **View functions**: Take state + callbacks, return `Element<Message>`
//! - **Canvas Programs**: Handle custom rendering and event-to-callback translation
//!
//! ## Current Features
//!
//! - **Theme constants**: Shared color schemes for stems and cue points
//! - **Peak generation**: Utilities for downsampling audio to waveform peaks
//! - **Waveform state**: Data structures for overview and zoomed waveform views
//! - **CueMarker**: Data structure for cue point display
//! - **Button styles**: Material 3D button styling with raised/pressed effects
//!
//! ## View Functions
//!
//! - `waveform_overview`: Overview waveform with click-to-seek
//! - `waveform_zoomed`: Zoomed detail view with drag-to-zoom
//! - `waveform_combined`: Both views in a single canvas (iced bug #3040 workaround)

pub mod button_styles;
pub mod playlist_browser;
pub mod rotary_knob;
pub mod slice_editor;
pub mod subscription;
pub mod theme;
pub mod track_table;
pub mod tree;
pub mod waveform;

// Re-export commonly used items
pub use theme::{WaveformConfig, CUE_COLORS, STEM_COLORS, STEM_NAMES, STEM_NAMES_SHORT};

// Button styling functions
pub use button_styles::{
    colored_style, colored_toggle_style, press_release_style, toggle_style,
    ACTIVE_BG, DEFAULT_BG,
};

// Rotary knob widget
pub use rotary_knob::{rotary_knob, RotaryKnobState};

// Peak generation utilities
pub use waveform::{
    generate_peaks, generate_peaks_for_range, generate_waveform_preview,
    generate_waveform_preview_with_gain, smooth_peaks,
    DEFAULT_WIDTH, HIGHRES_WIDTH, PEAK_SMOOTHING_WINDOW,
};

// Background peak computation
pub use waveform::{PeaksComputer, PeaksComputeRequest, PeaksComputeResult, PeaksResultReceiver};

// Waveform state structures
pub use waveform::{
    CombinedState, CueMarker, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    // Constants
    COMBINED_WAVEFORM_GAP, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

// Waveform view functions (idiomatic iced 0.14 pattern)
pub use waveform::{waveform_combined, waveform_overview, waveform_player, waveform_zoomed};

// Canvas interaction types for advanced usage
pub use waveform::{
    CombinedInteraction, OverviewInteraction, PlayerInteraction, ZoomedInteraction,
    // Player canvas layout constants
    OVERVIEW_STACK_GAP, PLAYER_SECTION_GAP, ZOOMED_GRID_GAP,
};

// Tree widget for hierarchical navigation
pub use tree::{tree_view, TreeIcon, TreeMessage, TreeNode, TreeState};

// Track table widget for displaying tracks
pub use track_table::{
    sort_tracks, track_table, SelectModifiers, TrackColumn, TrackRow, TrackTableMessage,
    TrackTableState,
    // Constants and functions for programmatic scrolling
    TRACK_ROW_HEIGHT, TRACK_TABLE_SCROLLABLE_ID,
    calculate_scroll_offset_for_centered_selection, scroll_to_centered_selection,
};

// Combined playlist browser (tree + table)
pub use playlist_browser::{
    playlist_browser, playlist_browser_with_drop_highlight, table_browser, tree_browser,
    PlaylistBrowserMessage, PlaylistBrowserState, TREE_PANEL_WIDTH,
};

// Slice editor widget for slicer preset editing
pub use slice_editor::{
    slice_editor, SliceEditPreset, SliceEditSequence, SliceEditStep, SliceEditorState,
    SlicerConfig, SlicerPresetConfig, SlicerSequenceConfig, SlicerStepConfig,
    NUM_PRESETS, NUM_SLICES, NUM_STEMS, NUM_STEPS,
    // Shared presets file I/O
    load_slicer_presets, save_slicer_presets, slicer_presets_path, SLICER_PRESETS_FILENAME,
};

// Slicer overlay drawing utilities
pub use waveform::{draw_slicer_overlay, draw_slicer_overlay_zoomed};

// Subscription helpers for message-driven architecture
pub use subscription::{mpsc_subscription, mpsc_subscription_owned};
