//! Application messages
//!
//! All message types that can be dispatched in the mesh-cue application.

use std::path::PathBuf;
use std::sync::Arc;

use iced::keyboard::{Key, Modifiers};
use iced::Point;
use mesh_core::audio_file::{LoadedTrack, TrackMetadata};
use mesh_core::playlist::NodeId;
use mesh_widgets::PlaylistBrowserMessage;

use crate::analysis::{AnalysisType, ReanalysisProgress, ReanalysisScope};
use crate::batch_import::{ImportProgress, StemGroup};
use crate::config::BpmSource;
use mesh_core::usb::UsbMessage;
use super::context_menu::ContextMenuKind;
use super::state::{BrowserSide, LinkedStemLoadedMsg, StemsLoadResult, View};

/// Application messages
#[derive(Debug, Clone)]
pub enum Message {
    // Navigation
    SwitchView(View),

    // Collection: Browser
    RefreshCollection,
    SelectTrack(usize),
    LoadTrack(usize),
    /// Phase 1: Metadata loaded (fast), now show UI
    TrackMetadataLoaded(Result<(PathBuf, TrackMetadata), String>),
    /// Phase 2: Audio stems loaded (slow), now enable playback (Shared for RT-safe drop)
    TrackStemsLoaded(StemsLoadResult),
    /// Background linked stem load completed
    LinkedStemLoaded(LinkedStemLoadedMsg),
    /// Legacy: full track loaded (kept for compatibility)
    TrackLoaded(Result<Arc<LoadedTrack>, String>),

    // Collection: Editor
    SetBpm(f64),
    /// Increase BPM by 1
    IncreaseBpm,
    /// Decrease BPM by 1
    DecreaseBpm,
    SetKey(String),
    AddCuePoint(u64),
    DeleteCuePoint(usize),
    SetCueLabel(usize, String),
    SaveTrack,
    SaveComplete(Result<(), String>),

    // Transport
    Play,
    Pause,
    Stop,
    Seek(f64),
    /// Enter scratch mode (vinyl-style scrubbing)
    ScratchStart,
    /// Update scratch position (0.0-1.0 ratio)
    ScratchMove(f64),
    /// Exit scratch mode
    ScratchEnd,
    /// CDJ-style cue button pressed (set cue point, start preview)
    Cue,
    /// CDJ-style cue button released (stop preview, return to cue point)
    CueReleased,
    /// Beat jump by N beats (positive = forward, negative = backward)
    BeatJump(i32),
    /// Set overview waveform grid density (4, 8, 16, 32 bars)
    SetOverviewGridBars(u32),
    /// Toggle loop on/off
    ToggleLoop,
    /// Adjust loop length (+1 = double, -1 = halve)
    AdjustLoopLength(i32),

    // Hot Cues (8 action buttons)
    /// Jump to hot cue at index (0-7)
    JumpToCue(usize),
    /// Set hot cue at index to current playhead position
    SetCuePoint(usize),
    /// Clear hot cue at index (Shift+click)
    ClearCuePoint(usize),
    /// Hot cue button pressed - start preview from this cue point (CDJ-style)
    HotCuePressed(usize),
    /// Hot cue button released - stop preview and return to cue point
    HotCueReleased(usize),

    // Saved Loops (8 loop buttons)
    /// Save current loop to slot index (0-7)
    SaveLoop(usize),
    /// Jump to and activate saved loop at index
    JumpToSavedLoop(usize),
    /// Clear saved loop at index (Shift+click)
    ClearSavedLoop(usize),

    // Drop Marker (for linked stem alignment)
    /// Set drop marker at current playhead position
    SetDropMarker,
    /// Clear drop marker (Shift+click)
    ClearDropMarker,

    // Stem Links (for prepared mode - stored in mslk chunk)
    /// Start stem link selection for a stem slot (0=Vocals, 1=Drums, 2=Bass, 3=Other)
    /// This focuses the browser for track selection
    StartStemLinkSelection(usize),
    /// Confirm stem link selection - link the stem to the currently selected track
    ConfirmStemLink(usize),
    /// Clear a stem link (Shift+click)
    ClearStemLink(usize),
    /// Toggle between original and linked stem for playback (when linked stem is loaded)
    ToggleStemLinkActive(usize),

    // Slice Editor
    /// Toggle a cell in the slice editor grid (step 0-15, slice 0-15)
    SliceEditorCellToggle { step: usize, slice: u8 },
    /// Toggle mute for a step
    SliceEditorMuteToggle(usize),
    /// Click stem button (toggles enabled + selects for editing)
    SliceEditorStemClick(usize),
    /// Select a preset tab (0-7)
    SliceEditorPresetSelect(usize),
    /// Save slicer presets to config file
    SaveSlicerPresets,

    // Zoomed Waveform
    /// Set zoom level for zoomed waveform (1-64 bars)
    SetZoomBars(u32),

    // Misc
    Tick,

    // Beat Grid
    /// Nudge beat grid left (earlier) by small increment
    NudgeBeatGridLeft,
    /// Nudge beat grid right (later) by small increment
    NudgeBeatGridRight,
    /// Align beat grid so the nearest beat matches the current playhead
    AlignBeatGridToPlayhead,

    // Settings
    OpenSettings,
    CloseSettings,
    UpdateSettingsMinTempo(String),
    UpdateSettingsMaxTempo(String),
    UpdateSettingsParallelProcesses(String),
    UpdateSettingsTrackNameFormat(String),
    UpdateSettingsGridBars(u32),
    UpdateSettingsBpmSource(BpmSource),
    UpdateSettingsSlicerBufferBars(u32),
    /// Update selected audio output pair
    UpdateSettingsOutputPair(usize),
    /// Refresh available audio devices
    RefreshAudioDevices,
    SaveSettings,
    SaveSettingsComplete(Result<(), String>),

    // Keyboard
    /// Key pressed with modifiers (for keybindings and shift tracking)
    /// The bool indicates if this is a repeat event (key held down)
    KeyPressed(Key, Modifiers, bool),
    /// Key released (for hot cue preview release)
    KeyReleased(Key, Modifiers),
    /// Modifier keys changed (Shift/Ctrl pressed/released without another key)
    ModifiersChanged(Modifiers),
    /// Global mouse position updated (for context menu placement)
    GlobalMouseMoved(Point),

    // Playlist Browsers
    /// Message from left playlist browser
    BrowserLeft(PlaylistBrowserMessage<NodeId, NodeId>),
    /// Message from right playlist browser
    BrowserRight(PlaylistBrowserMessage<NodeId, NodeId>),
    /// Refresh playlist storage and tree
    RefreshPlaylists,
    /// Load track from playlist by path
    LoadTrackByPath(PathBuf),

    // Drag and Drop
    /// Start dragging track(s) from a browser (supports multi-selection)
    DragTrackStart {
        track_ids: Vec<NodeId>,
        track_names: Vec<String>,
        browser: BrowserSide,
    },
    /// Cancel/end drag operation (mouse released without valid drop)
    DragTrackEnd,
    /// Drop track(s) onto a playlist folder
    DropTracksOnPlaylist {
        track_ids: Vec<NodeId>,
        target_playlist: NodeId,
    },

    // Batch Import
    /// Open the import modal
    OpenImport,
    /// Close the import modal
    CloseImport,
    /// Scan the import folder for stem groups
    ScanImportFolder,
    /// Import folder scan complete
    ImportFolderScanned(Vec<StemGroup>),
    /// Start the batch import process
    StartBatchImport,
    /// Progress update from import thread
    ImportProgressUpdate(ImportProgress),
    /// Cancel the current import
    CancelImport,
    /// Dismiss the import results popup
    DismissImportResults,

    // Delete confirmation
    /// Request deletion (shows confirmation modal)
    RequestDelete(BrowserSide),
    /// Request deletion by track/playlist ID (from context menu)
    RequestDeleteById(NodeId),
    /// Request deletion of a playlist
    RequestDeletePlaylist(NodeId),
    /// Cancel the delete operation
    CancelDelete,
    /// Confirm and execute the delete
    ConfirmDelete,

    // Context menu
    /// Show context menu at position
    ShowContextMenu(ContextMenuKind, Point),
    /// Close context menu
    CloseContextMenu,

    // Track operations
    /// Start renaming a playlist
    StartRenamePlaylist(NodeId),

    // Re-analysis
    /// Start re-analysis of tracks with specified type and scope
    StartReanalysis {
        analysis_type: AnalysisType,
        scope: ReanalysisScope,
    },
    /// Progress update from re-analysis worker thread
    ReanalysisProgress(ReanalysisProgress),
    /// Cancel the current re-analysis
    CancelReanalysis,

    // USB Export
    /// Open the USB export modal
    OpenExport,
    /// Close the USB export modal
    CloseExport,
    /// Select a USB device by index
    SelectExportDevice(usize),
    /// Toggle playlist selection for export (recursive - includes children)
    ToggleExportPlaylist(NodeId),
    /// Toggle expand/collapse state of a playlist tree node
    ToggleExportPlaylistExpand(NodeId),
    /// Toggle whether to include config in export
    ToggleExportConfig,
    /// Start building sync plan
    BuildSyncPlan,
    /// Start the export process
    StartExport,
    /// Cancel the current export
    CancelExport,
    /// USB manager message received
    UsbMessage(UsbMessage),
    /// Dismiss export results
    DismissExportResults,
}
