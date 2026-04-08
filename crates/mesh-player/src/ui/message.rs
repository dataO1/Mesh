//! Application messages for mesh-player
//!
//! All message types that can be dispatched in the mesh-player application.

use std::sync::Arc;
use mesh_widgets::MultibandEditorMessage;

use crate::config::{AppFont, FontSize, KeyScoringModel, WaveformAbstraction, WaveformLayout};
use crate::history::SuggestionContext;
use crate::suggestions::SplitSuggestions;
use super::collection_browser::CollectionBrowserMessage;
use super::deck_view::DeckMessage;
use mesh_widgets::keyboard::KeyboardMessage;
use super::network::NetworkMessage;
use super::system_update::SystemUpdateMessage;
use super::midi_learn::MidiLearnMessage;
use super::mixer_view::MixerMessage;
use super::state::{LinkedStemLoadedMsg, PresetLoadedMsg, TrackLoadedMsg};
use mesh_core::usb::UsbMessage;

/// Messages for set recording
#[derive(Debug, Clone)]
pub enum RecordingMessage {
    /// Recording event received from recording thread
    Event(mesh_core::recording::RecordingEvent),
}

/// Settings-related messages
#[derive(Debug, Clone)]
pub enum SettingsMessage {
    /// Open settings modal
    Open,
    /// Close settings modal
    Close,
    /// Update draft loop length index
    UpdateLoopLength(usize),
    /// Update draft zoom bars
    UpdateZoomBars(u32),
    /// Update draft grid bars
    UpdateGridBars(u32),
    /// Update draft theme (by name from theme.yaml)
    UpdateTheme(String),
    /// Update draft theme by index (for MIDI navigation — index into available theme names)
    UpdateThemeIndex(usize),
    /// Update draft phase sync setting
    UpdatePhaseSync(bool),
    /// Update draft auto-cue setting
    UpdateAutoCue(bool),
    /// Update draft slicer buffer bars
    UpdateSlicerBufferBars(u32),
    /// Update draft auto-gain enabled
    UpdateAutoGainEnabled(bool),
    /// Update draft target LUFS index
    UpdateTargetLufs(usize),
    /// Update draft show local collection
    UpdateShowLocalCollection(bool),
    /// Update draft persistent browse (keep browser visible while browse mode active)
    UpdatePersistentBrowse(bool),
    /// Update draft key scoring model
    UpdateKeyScoringModel(KeyScoringModel),
    /// Update draft waveform layout orientation
    UpdateWaveformLayout(WaveformLayout),
    /// Update draft waveform abstraction level
    UpdateWaveformAbstraction(WaveformAbstraction),
    /// Update draft UI font (requires restart)
    UpdateFont(AppFont),
    /// Update draft font size preset
    UpdateFontSize(FontSize),
    /// Update master device index
    UpdateMasterPair(usize),
    /// Update cue device index
    UpdateCuePair(usize),
    /// Refresh available audio devices
    RefreshAudioDevices,
    /// Update prerelease channel toggle (include RC/beta in OTA checks)
    UpdatePrereleaseChannel(bool),
    /// Show power off confirmation dialog (embedded only)
    PowerOffConfirm,
    /// Cancel power off (dismiss dialog)
    PowerOffCancel,
    /// Execute power off (confirmed)
    PowerOffExecute,
    /// Show set recording confirmation dialog
    RecordingConfirm,
    /// Cancel set recording dialog
    RecordingCancel,
    /// Execute set recording start/stop (confirmed)
    RecordingExecute,
    /// Save settings to disk
    Save,
    /// Settings save completed
    SaveComplete(Result<(), String>),
}

/// Messages that can be sent to the application
#[derive(Debug, Clone)]
pub enum Message {
    /// Frame-synced tick for atomic state sync (playheads, loops, stems, volume)
    Tick,
    /// Timer-driven LED feedback evaluation (~30Hz)
    UpdateLeds,
    /// Background track load completed
    TrackLoaded(TrackLoadedMsg),
    /// Background linked stem load completed
    LinkedStemLoaded(LinkedStemLoadedMsg),
    /// Background preset load completed (MultibandHost built on loader thread)
    PresetLoaded(PresetLoadedMsg),
    /// Deck-specific message
    Deck(usize, DeckMessage),
    /// Mixer message
    Mixer(MixerMessage),
    /// Collection browser message
    CollectionBrowser(CollectionBrowserMessage),
    /// Set global BPM
    SetGlobalBpm(f64),
    /// Load track to deck (deck_idx, path, optional suggestion context)
    LoadTrack(usize, String, Option<SuggestionContext>),
    /// Seek on a deck (deck_idx, normalized position 0.0-1.0)
    DeckSeek(usize, f64),
    /// Set zoom level on a deck (deck_idx, zoom in bars)
    DeckSetZoom(usize, u32),

    /// Settings modal message
    Settings(SettingsMessage),

    /// MIDI learn mode message
    MidiLearn(MidiLearnMessage),

    /// Multiband editor message
    Multiband(MultibandEditorMessage),

    /// USB manager message received
    Usb(UsbMessage),

    /// Plugin GUI polling tick (for parameter learning)
    PluginGuiTick,

    /// Select a global FX preset (applied to all decks)
    SelectGlobalFxPreset(Option<String>),
    /// Toggle the global FX preset picker dropdown
    ToggleGlobalFxPicker,
    /// Scroll through global FX preset list (from MIDI encoder)
    ScrollGlobalFx(i32),

    /// Smart suggestions query completed
    SuggestionsReady(Arc<Result<SplitSuggestions, String>>),

    /// A relevant seed condition changed (play/pause, volume threshold, track load).
    /// Starts a debounced timer — only one pending at a time.
    ScheduleSuggestionRefresh,
    /// Debounce timer expired — compute active seeds and retrigger if changed.
    CheckSuggestionSeeds,
    /// Energy direction debounce timer expired — query if generation still matches.
    CheckEnergyDebounce(u64),

    /// Hide the browser overlay (click-away backdrop)
    HideBrowserOverlay,

    /// On-screen keyboard message
    Keyboard(KeyboardMessage),

    /// Network management message (WiFi/LAN)
    Network(NetworkMessage),

    /// System update message (OTA)
    SystemUpdate(SystemUpdateMessage),

    /// Set recording message
    Recording(RecordingMessage),

    /// Monitor size detected at startup (for auto-sizing)
    GotMonitorSize(Option<iced::Size>),

    /// Periodic resource stats refresh (CPU%, GPU%, RAM)
    RefreshResourceStats,
}
