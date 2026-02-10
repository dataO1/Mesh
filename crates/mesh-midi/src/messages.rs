//! MIDI message types for iced integration
//!
//! These types are sent from the MIDI input handler to the iced app
//! via the flume channel and subscription.

/// Top-level MIDI message for iced app
#[derive(Debug, Clone)]
pub enum MidiMessage {
    /// Deck-specific action (transport, hot cues, slicer, etc.)
    Deck {
        /// Target deck index (0-3), already resolved from layer
        deck: usize,
        /// The action to perform
        action: DeckAction,
    },

    /// Mixer action (volume, EQ, filter)
    Mixer {
        /// Target channel index (0-3)
        channel: usize,
        /// The action to perform
        action: MixerAction,
    },

    /// Browser/navigation action
    Browser(BrowserAction),

    /// Global action (BPM, etc.)
    Global(GlobalAction),

    /// Layer toggle (handled by MidiController, also forwarded to app for UI update)
    LayerToggle {
        /// Physical deck that toggled (0 = left, 1 = right)
        physical_deck: usize,
    },

    /// Shift state changed
    ShiftChanged {
        /// Whether shift is now held
        held: bool,
    },
}

/// Deck-specific actions
#[derive(Debug, Clone)]
pub enum DeckAction {
    // Transport
    /// Toggle play/pause
    TogglePlay,
    /// Cue button pressed
    CuePress,
    /// Cue button released
    CueRelease,
    /// Sync button pressed
    Sync,

    // Hot Cues
    /// Hot cue pad pressed
    HotCuePress {
        /// Slot index (0-7)
        slot: usize,
    },
    /// Hot cue pad released
    HotCueRelease {
        /// Slot index (0-7)
        slot: usize,
    },
    /// Clear hot cue (shift + pad)
    HotCueClear {
        /// Slot index (0-7)
        slot: usize,
    },

    // Loop
    /// Toggle loop on/off
    ToggleLoop,
    /// Halve loop length
    LoopHalve,
    /// Double loop length
    LoopDouble,
    /// Set loop in point
    LoopIn,
    /// Set loop out point
    LoopOut,

    // Beat Jump
    /// Beat jump forward
    BeatJumpForward,
    /// Beat jump backward
    BeatJumpBackward,

    // Slicer
    /// Slicer pad trigger
    SlicerTrigger {
        /// Pad index (0-7)
        pad: usize,
    },
    /// Slicer assign (shift + pad)
    SlicerAssign {
        /// Pad index (0-7)
        pad: usize,
    },
    /// Set slicer mode on/off
    SetSlicerMode {
        /// Enable slicer mode
        enabled: bool,
    },
    /// Set hot cue mode on/off
    SetHotCueMode {
        /// Enable hot cue mode
        enabled: bool,
    },
    /// Reset slicer pattern
    SlicerReset,

    // Stem control
    /// Toggle stem mute
    ToggleStemMute {
        /// Stem index (0=vocals, 1=drums, 2=bass, 3=other)
        stem: usize,
    },
    /// Toggle stem solo
    ToggleStemSolo {
        /// Stem index
        stem: usize,
    },
    /// Select stem for effects
    SelectStem {
        /// Stem index
        stem: usize,
    },

    // Effects
    /// Set effect parameter value
    SetEffectParam {
        /// Parameter index (0-7 for the 8 knobs)
        param: usize,
        /// Normalized value (0.0-1.0)
        value: f32,
    },

    // Misc
    /// Toggle slip mode
    ToggleSlip,
    /// Toggle key match
    ToggleKeyMatch,
    /// Load selected track from browser
    LoadSelected,
    /// Seek to position (from jog wheel or waveform touch)
    Seek {
        /// Normalized position (0.0-1.0)
        position: f32,
    },
    /// Nudge tempo (from jog wheel)
    Nudge {
        /// Nudge direction and amount (-1.0 to 1.0)
        amount: f32,
    },

    /// Set FX macro value (macro_index 0-3, value 0.0-1.0)
    SetFxMacro {
        /// Macro index (0-3)
        macro_index: usize,
        /// Normalized value (0.0-1.0)
        value: f32,
    },
}

/// Mixer actions
#[derive(Debug, Clone)]
pub enum MixerAction {
    /// Set channel volume
    SetVolume(f32),
    /// Set channel filter (-1.0 = LP, 0.0 = flat, 1.0 = HP)
    SetFilter(f32),
    /// Set EQ high
    SetEqHi(f32),
    /// Set EQ mid
    SetEqMid(f32),
    /// Set EQ low
    SetEqLo(f32),
    /// Toggle headphone cue (PFL)
    ToggleCue,
    /// Set crossfader position
    SetCrossfader(f32),
}

/// Browser/navigation actions
#[derive(Debug, Clone)]
pub enum BrowserAction {
    /// Scroll browser list
    Scroll {
        /// Scroll direction: positive = down, negative = up
        delta: i32,
    },
    /// Select/enter current item
    Select,
    /// Go back/up in hierarchy
    Back,
}

/// Global actions
#[derive(Debug, Clone)]
pub enum GlobalAction {
    /// Set global BPM
    SetBpm(f64),
    /// Adjust BPM by delta
    AdjustBpm(f64),
    /// Set master volume
    SetMasterVolume(f32),
    /// Set cue/headphone volume
    SetCueVolume(f32),
    /// Set cue/master mix for headphones (0.0 = all cue, 1.0 = all master)
    SetCueMix(f32),
    /// Scroll FX preset list (delta from encoder)
    FxScroll(i32),
    /// Select/confirm current FX preset
    FxSelect,
}

impl MidiMessage {
    /// Create a deck play toggle message
    pub fn deck_play(deck: usize) -> Self {
        Self::Deck {
            deck,
            action: DeckAction::TogglePlay,
        }
    }

    /// Create a deck cue press message
    pub fn deck_cue_press(deck: usize) -> Self {
        Self::Deck {
            deck,
            action: DeckAction::CuePress,
        }
    }

    /// Create a deck cue release message
    pub fn deck_cue_release(deck: usize) -> Self {
        Self::Deck {
            deck,
            action: DeckAction::CueRelease,
        }
    }

    /// Create a hot cue press message
    pub fn hot_cue_press(deck: usize, slot: usize) -> Self {
        Self::Deck {
            deck,
            action: DeckAction::HotCuePress { slot },
        }
    }

    /// Create a hot cue release message
    pub fn hot_cue_release(deck: usize, slot: usize) -> Self {
        Self::Deck {
            deck,
            action: DeckAction::HotCueRelease { slot },
        }
    }

    /// Create a mixer volume message
    pub fn mixer_volume(channel: usize, value: f32) -> Self {
        Self::Mixer {
            channel,
            action: MixerAction::SetVolume(value),
        }
    }

    /// Create a mixer filter message
    pub fn mixer_filter(channel: usize, value: f32) -> Self {
        Self::Mixer {
            channel,
            action: MixerAction::SetFilter(value),
        }
    }

    /// Create a browser scroll message
    pub fn browser_scroll(delta: i32) -> Self {
        Self::Browser(BrowserAction::Scroll { delta })
    }
}
