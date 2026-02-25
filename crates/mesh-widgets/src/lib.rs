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
//! - `waveform_player_shader`: 4-deck GPU shader waveforms (mesh-player)
//! - `waveform_shader_combined`: Single-deck GPU shader zoomed + overview (mesh-cue)

pub mod button_styles;
pub mod deck_preset;
pub mod font;
pub mod keyboard;
pub mod knob;
pub mod multiband;
pub mod playlist_browser;
pub mod slice_editor;
pub mod stem_preset;
pub mod subscription;
pub mod theme;
pub mod track_table;
pub mod tree;
pub mod waveform;

// Re-export commonly used items
pub use font::{AppFont, LOGO_HANDLE};
pub use theme::{WaveformConfig, CUE_COLORS, STEM_COLORS, STEM_NAMES, STEM_NAMES_SHORT};

// Button styling functions
pub use button_styles::{
    colored_style, colored_toggle_style, press_release_style, toggle_style,
    ACTIVE_BG, DEFAULT_BG,
};

// Shader-based knob widget with modulation indicators
pub use knob::{
    Knob, KnobEvent, ModulationRange,
    colors as knob_colors, DEFAULT_SENSITIVITY,
};

// Peak generation utilities
pub use waveform::{
    allocate_empty_peaks, compute_highres_width, generate_peaks, generate_peaks_for_range,
    smooth_peaks_gaussian, update_peaks_for_region, DEFAULT_WIDTH,
    PEAK_REFERENCE_ZOOM_BARS, PEAK_SMOOTHING_WINDOW,
};

// Waveform state structures
pub use waveform::{
    CombinedState, CueMarker, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    // Constants
    COMBINED_WAVEFORM_GAP, DEFAULT_ZOOM_BARS, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};

// GPU shader waveform view functions
pub use waveform::{
    SingleDeckAction, WaveformAction, view_deck_header,
    waveform_player_shader, waveform_shader_combined,
    waveform_shader_single_overview, waveform_shader_single_zoomed,
};

// Tree widget for hierarchical navigation
pub use tree::{tree_view, TreeIcon, TreeMessage, TreeNode, TreeState};

// Track table widget for displaying tracks
pub use track_table::{
    sort_tracks, track_table, SelectModifiers, TrackColumn, TrackRow, TrackTableMessage,
    TrackTableState, TrackTag, parse_hex_color, tag_sort_priority,
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

// GPU shader waveform rendering (additional re-exports)
pub use waveform::{PeakBuffer, waveform_shader_overview, waveform_shader_zoomed};

// Slicer overlay — now handled by GPU shader, canvas functions removed from compilation

// Subscription helpers for message-driven architecture
pub use subscription::{mpsc_subscription, mpsc_subscription_owned};

// Multiband effect editor widget
pub use multiband::{
    multiband_editor, multiband_editor_content, preset_browser_overlay, save_dialog_overlay,
    BandUiState, EffectUiState, MultibandEditorMessage, MultibandEditorState,
    FREQ_MAX, FREQ_MIN, NUM_MACROS,
};

// Stem preset selector widget (legacy, kept for compatibility)
pub use stem_preset::{
    stem_preset_view, StemPresetMessage, StemPresetState,
    NUM_MACROS as STEM_PRESET_NUM_MACROS, DEFAULT_MACRO_NAMES,
};

// Deck preset selector widget (replaces per-stem presets with deck-level)
pub use deck_preset::{
    deck_preset_view, DeckPresetMessage, DeckPresetState, MacroParamMapping,
    MacroTargetType, NUM_MACROS as DECK_PRESET_NUM_MACROS,
};

// On-screen keyboard widget for embedded touchscreen and MIDI encoder
pub use keyboard::{
    keyboard_view, keyboard_handle, KeyboardState, KeyboardMessage, KeyboardEvent,
};
