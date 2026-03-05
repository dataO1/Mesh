//! Static mapping catalog — single source of truth for all mappable actions.
//!
//! This module defines every section and mapping that can appear in the
//! MIDI learn tree. Adding a new mapping means adding one `MappingDef`
//! entry to the appropriate section. The runtime tree builder, config
//! generator, and verification window all derive from this catalog.

use crate::config::ControlBehavior;
use crate::learn_defs::*;

// ---------------------------------------------------------------------------
// Helper: shorthand constructors for common MappingDef patterns
// ---------------------------------------------------------------------------

const fn button(
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    action: &'static str,
    feedback: Option<&'static str>,
) -> MappingDef {
    MappingDef {
        id,
        label,
        description: desc,
        action,
        control_type: ControlType::Button,
        behavior: ControlBehavior::Momentary,
        feedback_state: feedback,
        param_key: None,
        param_value: None,
        uses_physical_deck: true,
        visibility: Visibility::Always,
        mode_condition: None,
    }
}

const fn button_toggle(
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    action: &'static str,
    feedback: Option<&'static str>,
) -> MappingDef {
    MappingDef {
        id,
        label,
        description: desc,
        action,
        control_type: ControlType::Button,
        behavior: ControlBehavior::Toggle,
        feedback_state: feedback,
        param_key: None,
        param_value: None,
        uses_physical_deck: true,
        visibility: Visibility::Always,
        mode_condition: None,
    }
}

const fn encoder(
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    action: &'static str,
) -> MappingDef {
    MappingDef {
        id,
        label,
        description: desc,
        action,
        control_type: ControlType::Encoder,
        behavior: ControlBehavior::Continuous,
        feedback_state: None,
        param_key: None,
        param_value: None,
        uses_physical_deck: false,
        visibility: Visibility::Always,
        mode_condition: None,
    }
}

const fn knob(
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    action: &'static str,
) -> MappingDef {
    MappingDef {
        id,
        label,
        description: desc,
        action,
        control_type: ControlType::Knob,
        behavior: ControlBehavior::Continuous,
        feedback_state: None,
        param_key: None,
        param_value: None,
        uses_physical_deck: false,
        visibility: Visibility::Always,
        mode_condition: None,
    }
}

const fn fader(
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    action: &'static str,
) -> MappingDef {
    MappingDef {
        id,
        label,
        description: desc,
        action,
        control_type: ControlType::Fader,
        behavior: ControlBehavior::Continuous,
        feedback_state: None,
        param_key: None,
        param_value: None,
        uses_physical_deck: false,
        visibility: Visibility::Always,
        mode_condition: None,
    }
}

// ---------------------------------------------------------------------------
// Section: Navigation (mapped first, enables tree navigation)
// ---------------------------------------------------------------------------

static NAVIGATION_MAPPINGS: &[MappingDef] = &[
    encoder("nav.browse_encoder", "Browse Encoder",
        "Turn to scroll through the track list.",
        "browser.scroll"),
    MappingDef {
        uses_physical_deck: false,
        ..button("nav.browse_select", "Browse Press",
            "Press to load the selected track or enter a folder.",
            "browser.select", None)
    },
];

static NAVIGATION: SectionDef = SectionDef {
    id: "navigation",
    label: "Navigation",
    description: "Scroll the track browser and select tracks for loading.",
    repeat_mode: RepeatMode::Once,
    visibility: Visibility::Always,
    mappings: NAVIGATION_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Modifiers (shift buttons, layer toggles)
// ---------------------------------------------------------------------------

static MODIFIER_MAPPINGS: &[MappingDef] = &[
    MappingDef {
        uses_physical_deck: false,
        ..button("mod.shift_left", "Shift Button — Left",
            "Hold to access the shift-layer of other mapped controls.",
            "_shift", None)
    },
    MappingDef {
        uses_physical_deck: false,
        ..button("mod.shift_right", "Shift Button — Right",
            "Hold to access the shift-layer of other mapped controls.",
            "_shift", None)
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::LayerToggleOnly,
        ..button("mod.layer_toggle_left", "Layer Toggle — Left",
            "Press to switch this side between Layer A (Decks 1-2) and Layer B (Decks 3-4).",
            "_layer_toggle", Some("deck.layer_active"))
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::LayerToggleOnly,
        ..button("mod.layer_toggle_right", "Layer Toggle — Right",
            "Press to switch this side between Layer A (Decks 1-2) and Layer B (Decks 3-4).",
            "_layer_toggle", Some("deck.layer_active"))
    },
];

static MODIFIERS: SectionDef = SectionDef {
    id: "modifiers",
    label: "Modifiers",
    description: "Shift buttons add a second function to any mapped control. Layer toggles switch between deck layers.",
    repeat_mode: RepeatMode::Once,
    visibility: Visibility::Always,
    mappings: MODIFIER_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Transport (per physical deck)
// ---------------------------------------------------------------------------

static TRANSPORT_MAPPINGS: &[MappingDef] = &[
    button("transport.play", "Play",
        "Start or pause playback.",
        "deck.play", Some("deck.is_playing")),
    button("transport.cue", "Cue",
        "Hold to preview from the cue point. Release to snap back.",
        "deck.cue_press", Some("deck.is_cueing")),
    MappingDef {
        uses_physical_deck: false,
        ..button_toggle("transport.loop_toggle", "Loop Toggle",
            "Turn the active loop on or off.",
            "deck.toggle_loop", Some("deck.loop_encoder"))
    },
    MappingDef {
        uses_physical_deck: false,
        ..encoder("transport.loop_encoder", "Loop Size Encoder",
            "Turn to halve or double the loop length.",
            "deck.loop_size")
    },
    MappingDef {
        uses_physical_deck: false,
        ..button("transport.loop_in", "Loop In",
            "Set the loop start point at the current position.",
            "deck.loop_in", None)
    },
    MappingDef {
        uses_physical_deck: false,
        ..button("transport.loop_out", "Loop Out",
            "Set the loop end point and activate the loop.",
            "deck.loop_out", None)
    },
    button("transport.beat_jump_back", "Beat Jump Back",
        "Jump backward by the current beat jump size.",
        "deck.beat_jump_backward", None),
    button("transport.beat_jump_fwd", "Beat Jump Forward",
        "Jump forward by the current beat jump size.",
        "deck.beat_jump_forward", None),
    button("transport.slip", "Slip Mode",
        "Enable slip: playback continues underneath loops and scratches.",
        "deck.slip", Some("deck.slip_active")),
    button("transport.key_match", "Key Match",
        "Transpose this deck's key to match the master deck.",
        "deck.key_match", Some("deck.key_match_enabled")),
];

static TRANSPORT: SectionDef = SectionDef {
    id: "transport",
    label: "Transport",
    description: "Playback controls: play, cue, loops, and beat jumps.",
    repeat_mode: RepeatMode::PerPhysicalDeck,
    visibility: Visibility::Always,
    mappings: TRANSPORT_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Performance Pads (per physical deck)
// ---------------------------------------------------------------------------

static PADS_MAPPINGS: &[MappingDef] = &[
    button("pads.hot_cue_mode", "Hot Cue Mode",
        "Switch pads to hot cue mode (if your controller has mode buttons).",
        "deck.hot_cue_mode", Some("deck.hot_cue_mode")),
    MappingDef { param_key: Some("slot"), param_value: Some(0),
        ..button("pads.hot_cue.0", "Hot Cue 1", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(1),
        ..button("pads.hot_cue.1", "Hot Cue 2", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(2),
        ..button("pads.hot_cue.2", "Hot Cue 3", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(3),
        ..button("pads.hot_cue.3", "Hot Cue 4", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(4),
        ..button("pads.hot_cue.4", "Hot Cue 5", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(5),
        ..button("pads.hot_cue.5", "Hot Cue 6", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(6),
        ..button("pads.hot_cue.6", "Hot Cue 7", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef { param_key: Some("slot"), param_value: Some(7),
        ..button("pads.hot_cue.7", "Hot Cue 8", "Set or trigger a cue point at this position.", "deck.hot_cue_press", Some("deck.hot_cue_set")) },
    MappingDef {
        visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer_mode", "Slicer Mode",
            "Switch pads to slicer mode.",
            "deck.slicer_mode", Some("deck.slicer_mode"))
    },
    MappingDef { param_key: Some("pad"), param_value: Some(0), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.0", "Slicer 1", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(1), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.1", "Slicer 2", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(2), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.2", "Slicer 3", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(3), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.3", "Slicer 4", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(4), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.4", "Slicer 5", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(5), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.5", "Slicer 6", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(6), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.6", "Slicer 7", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef { param_key: Some("pad"), param_value: Some(7), visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer.7", "Slicer 8", "Trigger a slice from the current slicer buffer.", "deck.slicer_trigger", Some("deck.slicer_slice_active")) },
    MappingDef {
        visibility: Visibility::ControllerPadModeOnly,
        ..button("pads.slicer_reset", "Slicer Reset",
            "Clear the slicer buffer and return to normal playback.",
            "deck.slicer_reset", None)
    },
];

static PADS: SectionDef = SectionDef {
    id: "pads",
    label: "Performance Pads",
    description: "Hot cue points and slicer pads for live performance.",
    repeat_mode: RepeatMode::PerPhysicalDeck,
    visibility: Visibility::Always,
    mappings: PADS_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Stems (per virtual deck)
// ---------------------------------------------------------------------------

const STEM_NAMES: [&str; 4] = ["Vocals", "Drums", "Bass", "Other"];

static STEMS_MAPPINGS: &[MappingDef] = &[
    // Mutes
    MappingDef { param_key: Some("stem"), param_value: Some(0), uses_physical_deck: false,
        ..button("stems.mute.0", "Vocals Mute", "Silence the vocals stem.", "deck.stem_mute", Some("deck.stem_muted")) },
    MappingDef { param_key: Some("stem"), param_value: Some(1), uses_physical_deck: false,
        ..button("stems.mute.1", "Drums Mute", "Silence the drums stem.", "deck.stem_mute", Some("deck.stem_muted")) },
    MappingDef { param_key: Some("stem"), param_value: Some(2), uses_physical_deck: false,
        ..button("stems.mute.2", "Bass Mute", "Silence the bass stem.", "deck.stem_mute", Some("deck.stem_muted")) },
    MappingDef { param_key: Some("stem"), param_value: Some(3), uses_physical_deck: false,
        ..button("stems.mute.3", "Other Mute", "Silence the other/melody stem.", "deck.stem_mute", Some("deck.stem_muted")) },
    // Solos
    MappingDef { param_key: Some("stem"), param_value: Some(0), uses_physical_deck: false,
        ..button("stems.solo.0", "Vocals Solo", "Play only vocals, muting all other stems.", "deck.stem_solo", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(1), uses_physical_deck: false,
        ..button("stems.solo.1", "Drums Solo", "Play only drums, muting all other stems.", "deck.stem_solo", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(2), uses_physical_deck: false,
        ..button("stems.solo.2", "Bass Solo", "Play only bass, muting all other stems.", "deck.stem_solo", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(3), uses_physical_deck: false,
        ..button("stems.solo.3", "Other Solo", "Play only other/melody, muting all other stems.", "deck.stem_solo", None) },
    // Links
    MappingDef { param_key: Some("stem"), param_value: Some(0), uses_physical_deck: false,
        ..button("stems.link.0", "Vocals Link", "Link vocals to the same stem on the other deck for smooth transitions.", "deck.stem_link", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(1), uses_physical_deck: false,
        ..button("stems.link.1", "Drums Link", "Link drums to the same stem on the other deck for smooth transitions.", "deck.stem_link", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(2), uses_physical_deck: false,
        ..button("stems.link.2", "Bass Link", "Link bass to the same stem on the other deck for smooth transitions.", "deck.stem_link", None) },
    MappingDef { param_key: Some("stem"), param_value: Some(3), uses_physical_deck: false,
        ..button("stems.link.3", "Other Link", "Link other/melody to the same stem on the other deck for smooth transitions.", "deck.stem_link", None) },
];

static STEMS: SectionDef = SectionDef {
    id: "stems",
    label: "Stems",
    description: "Mute, solo, or link individual stems (Vocals, Drums, Bass, Other).",
    repeat_mode: RepeatMode::PerVirtualDeck,
    visibility: Visibility::Always,
    mappings: STEMS_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Mixer (per virtual deck + global crossfader)
// ---------------------------------------------------------------------------

static MIXER_MAPPINGS: &[MappingDef] = &[
    fader("mixer.volume", "Volume", "Channel volume fader.", "mixer.volume"),
    knob("mixer.filter", "Filter", "Bipolar filter: left = low-pass, center = off, right = high-pass.", "mixer.filter"),
    knob("mixer.eq_hi", "EQ High", "3-band equalizer high frequency.", "mixer.eq_hi"),
    knob("mixer.eq_mid", "EQ Mid", "3-band equalizer mid frequency.", "mixer.eq_mid"),
    knob("mixer.eq_lo", "EQ Low", "3-band equalizer low frequency.", "mixer.eq_lo"),
    MappingDef {
        uses_physical_deck: false,
        ..button("mixer.cue", "Cue / PFL",
            "Send this channel to the headphone cue bus for previewing.",
            "mixer.cue", Some("mixer.cue_enabled"))
    },
];

static MIXER_GLOBAL_MAPPINGS: &[MappingDef] = &[
    fader("mixer.crossfader", "Crossfader",
        "Blend between left and right channels.",
        "mixer.crossfader"),
];

static MIXER_CHANNELS: SectionDef = SectionDef {
    id: "mixer",
    label: "Mixer",
    description: "Volume faders, EQ knobs, filter, and headphone cue per channel.",
    repeat_mode: RepeatMode::PerVirtualDeck,
    visibility: Visibility::Always,
    mappings: MIXER_MAPPINGS,
};

static MIXER_GLOBAL: SectionDef = SectionDef {
    id: "mixer_global",
    label: "Mixer Global",
    description: "Crossfader (shared across all channels).",
    repeat_mode: RepeatMode::Once,
    visibility: Visibility::Always,
    mappings: MIXER_GLOBAL_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Effects (per physical deck)
// ---------------------------------------------------------------------------

static EFFECTS_MAPPINGS: &[MappingDef] = &[
    MappingDef { param_key: Some("macro"), param_value: Some(0), uses_physical_deck: true,
        ..knob("fx.macro.0", "FX Macro 1", "Control parameters of the active FX preset.", "deck.fx_macro") },
    MappingDef { param_key: Some("macro"), param_value: Some(1), uses_physical_deck: true,
        ..knob("fx.macro.1", "FX Macro 2", "Control parameters of the active FX preset.", "deck.fx_macro") },
    MappingDef { param_key: Some("macro"), param_value: Some(2), uses_physical_deck: true,
        ..knob("fx.macro.2", "FX Macro 3", "Control parameters of the active FX preset.", "deck.fx_macro") },
    MappingDef { param_key: Some("macro"), param_value: Some(3), uses_physical_deck: true,
        ..knob("fx.macro.3", "FX Macro 4", "Control parameters of the active FX preset.", "deck.fx_macro") },
];

static EFFECTS: SectionDef = SectionDef {
    id: "effects",
    label: "Effects",
    description: "FX macro knobs that control the active FX preset's parameters.",
    repeat_mode: RepeatMode::PerPhysicalDeck,
    visibility: Visibility::Always,
    mappings: EFFECTS_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Section: Global Controls (once)
// ---------------------------------------------------------------------------

static GLOBAL_MAPPINGS: &[MappingDef] = &[
    encoder("global.fx_encoder", "FX Preset Encoder",
        "Scroll through available FX presets for all decks.",
        "global.fx_scroll"),
    MappingDef {
        uses_physical_deck: false,
        ..button("global.fx_select", "FX Preset Select",
            "Apply the currently highlighted FX preset.",
            "global.fx_select", None)
    },
    fader("global.master_volume", "Master Volume",
        "Main output level.",
        "global.master_volume"),
    knob("global.cue_volume", "Cue Volume",
        "Headphone output level.",
        "global.cue_volume"),
    knob("global.cue_mix", "Cue Mix",
        "Balance between cue (preview) and master in headphones.",
        "mixer.cue_mix"),
    knob("global.bpm", "BPM",
        "Adjust the global tempo.",
        "global.bpm"),
    MappingDef {
        uses_physical_deck: false,
        ..button("global.settings", "Settings Button",
            "Open or close the settings panel.",
            "global.settings_toggle", None)
    },
    knob("global.suggestion_energy_left", "Suggestion Energy — Left",
        "Bias track suggestions toward higher or lower energy.",
        "deck.suggestion_energy"),
    knob("global.suggestion_energy_right", "Suggestion Energy — Right",
        "Bias track suggestions toward higher or lower energy.",
        "deck.suggestion_energy"),
    MappingDef {
        uses_physical_deck: false,
        ..button("global.deck_load.0", "Deck Load 1",
            "Load the selected browser track into this deck.",
            "deck.load_selected", None)
    },
    MappingDef {
        uses_physical_deck: false,
        ..button("global.deck_load.1", "Deck Load 2",
            "Load the selected browser track into this deck.",
            "deck.load_selected", None)
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::FourDeckOnly,
        ..button("global.deck_load.2", "Deck Load 3",
            "Load the selected browser track into this deck.",
            "deck.load_selected", None)
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::FourDeckOnly,
        ..button("global.deck_load.3", "Deck Load 4",
            "Load the selected browser track into this deck.",
            "deck.load_selected", None)
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::CompactModeOnly,
        ..button("global.side_browse_left", "Side Browse Mode — Left",
            "Hold to temporarily enter browse mode on this side.",
            "side.browse_mode", Some("side.browse_mode"))
    },
    MappingDef {
        uses_physical_deck: false,
        visibility: Visibility::CompactModeOnly,
        ..button("global.side_browse_right", "Side Browse Mode — Right",
            "Hold to temporarily enter browse mode on this side.",
            "side.browse_mode", Some("side.browse_mode"))
    },
];

static GLOBAL: SectionDef = SectionDef {
    id: "global",
    label: "Global Controls",
    description: "Master volume, cue mix, BPM, FX preset scrolling, deck load buttons, and settings.",
    repeat_mode: RepeatMode::Once,
    visibility: Visibility::Always,
    mappings: GLOBAL_MAPPINGS,
};

// ---------------------------------------------------------------------------
// Public API: section catalog
// ---------------------------------------------------------------------------

/// Returns the complete section catalog in display order.
///
/// The tree builder walks this list, checks visibility, and expands
/// repeated sections into concrete deck-labeled instances.
pub fn section_catalog() -> &'static [SectionDef] {
    static CATALOG: &[SectionDef] = &[
        NAVIGATION,
        MODIFIERS,
        TRANSPORT,
        PADS,
        STEMS,
        MIXER_CHANNELS,
        MIXER_GLOBAL,
        EFFECTS,
        GLOBAL,
    ];
    CATALOG
}

/// Stem display names (index 0-3: Vocals, Drums, Bass, Other).
pub fn stem_name(index: usize) -> &'static str {
    STEM_NAMES.get(index).copied().unwrap_or("Unknown")
}
