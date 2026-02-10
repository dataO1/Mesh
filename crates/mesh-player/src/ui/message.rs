//! Application messages for mesh-player
//!
//! All message types that can be dispatched in the mesh-player application.

use mesh_widgets::{MultibandEditorMessage, PeaksComputeResult};

use crate::config::StemColorPalette;
use super::collection_browser::CollectionBrowserMessage;
use super::deck_view::DeckMessage;
use super::midi_learn::MidiLearnMessage;
use super::mixer_view::MixerMessage;
use super::state::{LinkedStemLoadedMsg, PresetLoadedMsg, TrackLoadedMsg};
use mesh_core::usb::UsbMessage;

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
    /// Update draft stem color palette
    UpdateStemColorPalette(StemColorPalette),
    /// Update draft phase sync setting
    UpdatePhaseSync(bool),
    /// Update draft slicer buffer bars
    UpdateSlicerBufferBars(u32),
    /// Update draft auto-gain enabled
    UpdateAutoGainEnabled(bool),
    /// Update draft target LUFS index
    UpdateTargetLufs(usize),
    /// Update draft show local collection
    UpdateShowLocalCollection(bool),
    /// Update master device index
    UpdateMasterPair(usize),
    /// Update cue device index
    UpdateCuePair(usize),
    /// Refresh available audio devices
    RefreshAudioDevices,
    /// Save settings to disk
    Save,
    /// Settings save completed
    SaveComplete(Result<(), String>),
}

/// Messages that can be sent to the application
#[derive(Debug, Clone)]
pub enum Message {
    /// Tick for periodic UI updates (waveform animation, atomics reading)
    Tick,
    /// Background track load completed
    TrackLoaded(TrackLoadedMsg),
    /// Background peak computation completed
    PeaksComputed(PeaksComputeResult),
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
    /// Load track to deck
    LoadTrack(usize, String),
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
}
