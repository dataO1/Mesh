//! Application messages for mesh-player
//!
//! All message types that can be dispatched in the mesh-player application.

use mesh_widgets::PeaksComputeResult;

use crate::config::StemColorPalette;
use super::collection_browser::CollectionBrowserMessage;
use super::deck_view::DeckMessage;
use super::midi_learn::MidiLearnMessage;
use super::mixer_view::MixerMessage;
use super::state::{LinkedStemLoadedMsg, TrackLoadedMsg};
use mesh_core::usb::UsbMessage;

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

    // Settings
    /// Open settings modal
    OpenSettings,
    /// Close settings modal
    CloseSettings,
    /// Update settings: loop length index
    UpdateSettingsLoopLength(usize),
    /// Update settings: zoom bars
    UpdateSettingsZoomBars(u32),
    /// Update settings: grid bars
    UpdateSettingsGridBars(u32),
    /// Update settings: stem color palette
    UpdateSettingsStemColorPalette(StemColorPalette),
    /// Update settings: phase sync enabled
    UpdateSettingsPhaseSync(bool),
    /// Update settings: slicer buffer bars (1, 4, 8, or 16)
    UpdateSettingsSlicerBufferBars(u32),
    /// Update settings: auto-gain enabled
    UpdateSettingsAutoGainEnabled(bool),
    /// Update settings: target LUFS index (0-3)
    UpdateSettingsTargetLufs(usize),
    /// Save settings to disk
    SaveSettings,
    /// Settings save complete
    SaveSettingsComplete(Result<(), String>),

    // MIDI Learn
    /// MIDI learn mode message
    MidiLearn(MidiLearnMessage),

    // USB
    /// USB manager message received
    Usb(UsbMessage),
}
