//! Deck view component
//!
//! Displays a single DJ deck's controls:
//! - Track info display
//! - Transport controls (play/pause, cue, sync)
//! - Hot cue buttons
//! - Pitch/tempo fader
//! - Stem controls (mute/solo/volume per stem)
//!
//! Note: Waveform display is handled separately in the unified PlayerCanvas

use iced::widget::{button, column, container, mouse_area, row, scrollable, slider, text, Row, Space};
use iced::{Background, Center, Color, Element, Fill, Length};

use mesh_core::engine::Deck;
use mesh_core::types::PlayState;
use mesh_widgets::{CUE_COLORS, DeckPresetState, DeckPresetMessage, DECK_PRESET_NUM_MACROS};

use super::midi_learn::HighlightTarget;

/// Stem names for display
pub const STEM_NAMES: [&str; 4] = ["Vocals", "Drums", "Bass", "Other"];
/// Short stem names for compact display
pub const STEM_NAMES_SHORT: [&str; 4] = ["VOC", "DRM", "BAS", "OTH"];

/// Action button mode - determines behavior of the 8 performance pads
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionButtonMode {
    /// Hot cue mode - buttons trigger/set hot cues (default behavior)
    #[default]
    HotCue,
    /// Slicer mode - buttons queue slices for playback
    Slicer,
}

/// State for a deck view
pub struct DeckView {
    /// Deck index (0-3)
    deck_idx: usize,
    /// Current playback state
    state: PlayState,
    /// Current position (samples)
    position: u64,
    /// Total track duration (samples)
    duration_samples: u64,
    /// Track BPM
    track_bpm: f64,
    /// Track filename
    track_name: String,
    /// Last loaded track name (to detect changes)
    last_loaded_track: String,
    /// Hot cue positions (samples) for display - None if slot is empty
    hot_cue_positions: [Option<u64>; 8],
    /// Stem mute states
    stem_muted: [bool; 4],
    /// Stem solo states
    stem_soloed: [bool; 4],
    /// Loop active
    loop_active: bool,
    /// Current loop length in beats
    loop_length_beats: f32,
    /// Slip mode enabled
    slip_enabled: bool,
    /// Key matching enabled
    key_match_enabled: bool,
    /// Currently selected stem for effect chain view (0-3)
    selected_stem: usize,
    /// Deck preset state (shared preset + macros across all stems)
    deck_preset: DeckPresetState,
    /// Current action button mode (HotCue or Slicer)
    action_mode: ActionButtonMode,
    /// Whether slicer is active (synced from atomics)
    slicer_active: bool,
    /// Current slicer queue (16 slice indices)
    slicer_queue: [u8; 16],
    /// Current slice being played (0-15)
    slicer_current_slice: u8,
    /// Whether shift is currently held
    shift_held: bool,
    /// Current highlight target for MIDI learn mode (if any)
    highlight_target: Option<HighlightTarget>,
    /// Whether this deck is currently targeted by a physical MIDI deck
    midi_active: bool,
    /// Whether this deck is on Layer B (secondary layer)
    is_secondary_layer: bool,
}

/// Messages for deck interaction
#[derive(Debug, Clone)]
pub enum DeckMessage {
    /// Toggle play/pause
    TogglePlayPause,
    /// Cue button pressed
    CuePressed,
    /// Cue button released
    CueReleased,
    /// Set cue point
    SetCue,
    /// Hot cue button pressed (0-7) - CDJ-style: jump and play, or preview when stopped
    HotCuePressed(usize),
    /// Hot cue button released - returns to original position if previewing
    HotCueReleased(usize),
    /// Set hot cue at current position
    SetHotCue(usize),
    /// Clear hot cue
    ClearHotCue(usize),
    /// Sync to master
    Sync,
    /// Toggle loop
    ToggleLoop,
    /// Toggle slip mode
    ToggleSlip,
    /// Toggle key matching
    ToggleKeyMatch,
    /// Set loop length (beats)
    SetLoopLength(u32),
    /// Halve loop length
    LoopHalve,
    /// Double loop length
    LoopDouble,
    /// Beat jump backward (uses loop length)
    BeatJumpBack,
    /// Beat jump forward (uses loop length)
    BeatJumpForward,
    /// Toggle stem mute
    ToggleStemMute(usize),
    /// Toggle stem solo
    ToggleStemSolo(usize),
    /// Select stem tab for multiband view
    SelectStem(usize),
    /// Deck preset message (shared macros + preset selector)
    DeckPreset(DeckPresetMessage),
    /// Open multiband editor for a stem (stem_idx) - only used in mapping mode
    OpenMultibandEditor(usize),
    /// Set action button mode (HotCue or Slicer)
    SetActionMode(ActionButtonMode),
    /// Select slicer preset (0-7) - normal click on slicer pad
    SlicerPresetSelect(usize),
    /// Slicer trigger (0-7) - shift+click queues slice for live adjustment
    SlicerTrigger(usize),
    /// Reset slicer pattern to default [0,1,2,3,4,5,6,7]
    ResetSlicerPattern,
    /// Shift button pressed
    ShiftPressed,
    /// Shift button released
    ShiftReleased,
}

/// Loop length labels for display (1 beat to 64 bars = 256 beats)
const LOOP_LENGTHS: [f32; 9] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];

impl DeckView {
    /// Create a new deck view
    pub fn new(deck_idx: usize) -> Self {
        Self {
            deck_idx,
            state: PlayState::Stopped,
            position: 0,
            duration_samples: 0,
            track_bpm: 0.0,
            track_name: String::new(),
            last_loaded_track: String::new(),
            hot_cue_positions: [None; 8],
            stem_muted: [false; 4],
            stem_soloed: [false; 4],
            loop_active: false,
            loop_length_beats: 4.0, // Default 4 beats
            slip_enabled: false,
            key_match_enabled: false,
            selected_stem: 0,       // Start with Vocals selected
            deck_preset: DeckPresetState::new(),
            action_mode: ActionButtonMode::default(),
            slicer_active: false,
            slicer_queue: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            slicer_current_slice: 0,
            shift_held: false,
            highlight_target: None,
            midi_active: false,
            is_secondary_layer: false,
        }
    }

    /// Set the highlight target for MIDI learn mode
    pub fn set_highlight(&mut self, target: Option<HighlightTarget>) {
        self.highlight_target = target;
    }

    /// Check if a specific target should be highlighted
    fn is_highlighted(&self, target: HighlightTarget) -> bool {
        self.highlight_target == Some(target)
    }

    /// Get highlight border style for MIDI learn mode
    fn highlight_border() -> iced::Border {
        iced::Border {
            color: Color::from_rgb(1.0, 0.0, 0.0),
            width: 3.0,
            radius: 4.0.into(),
        }
    }

    /// Sync view state from deck
    pub fn sync_from_deck(&mut self, deck: &Deck) {
        self.state = deck.state();
        self.position = deck.position();
        self.loop_active = deck.loop_state().active;
        self.slip_enabled = deck.slip_enabled();
        self.key_match_enabled = deck.key_match_enabled();

        if let Some(track) = deck.track() {
            self.track_bpm = track.bpm();
            self.track_name = track.filename().to_string();
            self.duration_samples = track.duration_samples as u64;
            self.last_loaded_track = self.track_name.clone();
        } else {
            self.track_bpm = 0.0;
            self.track_name = String::new();
            self.duration_samples = 0;
            self.last_loaded_track.clear();
        }

        // Sync hot cue positions for display
        for i in 0..8 {
            self.hot_cue_positions[i] = deck.hot_cue(i).map(|hc| hc.position as u64);
        }

        // Sync stem states and multiband containers
        for i in 0..4 {
            let stem = mesh_core::types::Stem::ALL[i];
            let stem_state = deck.stem(stem);

            // Mute/solo is on StemState itself
            self.stem_muted[i] = stem_state.muted;
            self.stem_soloed[i] = stem_state.soloed;

            // Sync macro values from the multiband to the deck preset state
            // (macros are shared at deck level, but we read them from the engine's per-stem state)
            if i == 0 {
                // Only sync from first stem to avoid overwriting - macros are deck-level
                for k in 0..DECK_PRESET_NUM_MACROS {
                    self.deck_preset.macro_values[k] = stem_state.multiband.macro_value(k);
                }
            }
        }
    }

    /// Sync only play state from atomics (lock-free UI update)
    ///
    /// This is called every frame to update the play/pause button state
    /// without acquiring the engine mutex.
    pub fn sync_play_state(&mut self, state: PlayState) {
        self.state = state;
    }

    /// Sync loop length from atomics (lock-free UI update)
    ///
    /// This is called every frame to update the loop length display.
    pub fn sync_loop_length_index(&mut self, index: u8) {
        if let Some(&beats) = LOOP_LENGTHS.get(index as usize) {
            self.loop_length_beats = beats;
        }
    }

    /// Set the selected stem for effect chain view (UI-only state)
    pub fn set_selected_stem(&mut self, stem_idx: usize) {
        if stem_idx < 4 {
            self.selected_stem = stem_idx;
        }
    }

    /// Check if a stem is muted
    pub fn is_stem_muted(&self, stem_idx: usize) -> bool {
        self.stem_muted.get(stem_idx).copied().unwrap_or(false)
    }

    /// Check if a stem is soloed
    pub fn is_stem_soloed(&self, stem_idx: usize) -> bool {
        self.stem_soloed.get(stem_idx).copied().unwrap_or(false)
    }

    /// Get hot cue position for a slot
    pub fn hot_cue_position(&self, slot: usize) -> Option<u64> {
        self.hot_cue_positions.get(slot).copied().flatten()
    }

    /// Set hot cue position for a slot (called after track load or when setting new cue)
    pub fn set_hot_cue_position(&mut self, slot: usize, position: Option<u64>) {
        if slot < 8 {
            self.hot_cue_positions[slot] = position;
        }
    }

    /// Update stem mute state (for optimistic UI updates)
    pub fn set_stem_muted(&mut self, stem_idx: usize, muted: bool) {
        if stem_idx < 4 {
            self.stem_muted[stem_idx] = muted;
        }
    }

    /// Update stem solo state (for optimistic UI updates)
    pub fn set_stem_soloed(&mut self, stem_idx: usize, soloed: bool) {
        if stem_idx < 4 {
            self.stem_soloed[stem_idx] = soloed;
        }
    }

    /// Set a shared macro knob value
    pub fn set_deck_macro(&mut self, knob_idx: usize, value: f32) {
        self.deck_preset.set_macro_value(knob_idx, value);
    }

    /// Get a shared macro knob value
    pub fn deck_macro_value(&self, knob_idx: usize) -> f32 {
        self.deck_preset.macro_value(knob_idx)
    }

    /// Get mutable reference to the deck preset state
    pub fn deck_preset_mut(&mut self) -> &mut DeckPresetState {
        &mut self.deck_preset
    }

    /// Get reference to the deck preset state
    pub fn deck_preset(&self) -> &DeckPresetState {
        &self.deck_preset
    }

    /// Check if key matching is enabled
    pub fn key_match_enabled(&self) -> bool {
        self.key_match_enabled
    }

    /// Set the action button mode
    pub fn set_action_mode(&mut self, mode: ActionButtonMode) {
        self.action_mode = mode;
    }

    /// Get the current action button mode
    pub fn action_mode(&self) -> ActionButtonMode {
        self.action_mode
    }

    /// Check if slicer is active
    pub fn slicer_active(&self) -> bool {
        self.slicer_active
    }

    /// Sync slicer state from atomics
    pub fn sync_slicer_state(&mut self, active: bool, current_slice: u8, queue: [u8; 16]) {
        self.slicer_active = active;
        self.slicer_current_slice = current_slice;
        self.slicer_queue = queue;
    }

    /// Set shift held state
    pub fn set_shift_held(&mut self, held: bool) {
        self.shift_held = held;
    }

    /// Check if shift is held
    pub fn shift_held(&self) -> bool {
        self.shift_held
    }

    /// Set whether this deck is currently targeted by a physical MIDI deck
    pub fn set_midi_active(&mut self, active: bool) {
        self.midi_active = active;
    }

    /// Set whether this deck is on Layer B (secondary layer)
    pub fn set_secondary_layer(&mut self, is_secondary: bool) {
        self.is_secondary_layer = is_secondary;
    }

    /// Get hot cues bitmap (bit N = hot cue N is set)
    ///
    /// Used for LED feedback to MIDI controllers.
    pub fn hot_cues_bitmap(&self) -> u8 {
        let mut bitmap = 0u8;
        for (i, pos) in self.hot_cue_positions.iter().enumerate() {
            if i < 8 && pos.is_some() {
                bitmap |= 1 << i;
            }
        }
        bitmap
    }

    /// Check if loop is active
    pub fn loop_active(&self) -> bool {
        self.loop_active
    }

    /// Get current loop length in beats
    pub fn loop_length_beats(&self) -> f32 {
        self.loop_length_beats
    }

    /// Check if slip mode is enabled
    pub fn slip_enabled(&self) -> bool {
        self.slip_enabled
    }

    /// Get stem mute states as bitmap (bit N = stem N is muted)
    pub fn stems_muted_bitmap(&self) -> u8 {
        let mut bitmap = 0u8;
        for (i, &muted) in self.stem_muted.iter().enumerate() {
            if i < 4 && muted {
                bitmap |= 1 << i;
            }
        }
        bitmap
    }

    /// Handle a deck message
    pub fn handle_message(&mut self, msg: DeckMessage, deck: Option<&mut Deck>) {
        let Some(deck) = deck else { return };

        match msg {
            DeckMessage::TogglePlayPause => {
                if deck.state() == PlayState::Playing {
                    deck.pause();
                } else {
                    deck.play();
                }
            }
            DeckMessage::CuePressed => deck.cue_press(),
            DeckMessage::CueReleased => deck.cue_release(),
            DeckMessage::SetCue => deck.set_cue_point(),
            DeckMessage::HotCuePressed(idx) => deck.hot_cue_press(idx),
            DeckMessage::HotCueReleased(_idx) => deck.hot_cue_release(),
            DeckMessage::SetHotCue(idx) => deck.set_hot_cue(idx),
            DeckMessage::ClearHotCue(idx) => deck.clear_hot_cue(idx),
            DeckMessage::Sync => {
                // Sync is handled at engine level
            }
            DeckMessage::ToggleLoop => {
                deck.toggle_loop();
            }
            DeckMessage::ToggleSlip => {
                deck.toggle_slip();
            }
            DeckMessage::ToggleKeyMatch => {
                // Handled at engine level via command
            }
            DeckMessage::SetLoopLength(beats) => {
                // Find index for this beat length and set it
                if let Some(idx) = LOOP_LENGTHS.iter().position(|&b| b == beats as f32) {
                    for _ in 0..(idx as i32 - deck.loop_state().length_index as i32).abs() {
                        if idx > deck.loop_state().length_index {
                            deck.adjust_loop_length(1);
                        } else {
                            deck.adjust_loop_length(-1);
                        }
                    }
                }
                self.loop_length_beats = LOOP_LENGTHS[deck.loop_state().length_index];
            }
            DeckMessage::LoopHalve => {
                deck.adjust_loop_length(-1);
                self.loop_length_beats = LOOP_LENGTHS[deck.loop_state().length_index];
            }
            DeckMessage::LoopDouble => {
                deck.adjust_loop_length(1);
                self.loop_length_beats = LOOP_LENGTHS[deck.loop_state().length_index];
            }
            DeckMessage::BeatJumpBack => {
                deck.beat_jump_backward();
            }
            DeckMessage::BeatJumpForward => {
                deck.beat_jump_forward();
            }
            DeckMessage::ToggleStemMute(stem_idx) => {
                // Mute/solo is now on StemState directly
                if stem_idx < 4 {
                    let stem = mesh_core::types::Stem::ALL[stem_idx];
                    let stem_state = deck.stem_mut(stem);
                    stem_state.muted = !stem_state.muted;
                }
            }
            DeckMessage::ToggleStemSolo(stem_idx) => {
                if stem_idx < 4 {
                    let stem = mesh_core::types::Stem::ALL[stem_idx];
                    let stem_state = deck.stem_mut(stem);
                    stem_state.soloed = !stem_state.soloed;
                }
            }
            DeckMessage::SelectStem(stem_idx) => {
                if stem_idx < 4 {
                    self.selected_stem = stem_idx;
                }
            }
            DeckMessage::OpenMultibandEditor(_stem_idx) => {
                // Handled at app level - opens multiband editor modal
            }
            DeckMessage::DeckPreset(ref msg) => {
                // Update local UI state for immediate feedback
                self.deck_preset.handle_message(msg.clone());
                // Actual engine update handled at app level
            }
            DeckMessage::SetActionMode(mode) => {
                self.action_mode = mode;
            }
            DeckMessage::SlicerPresetSelect(_idx) => {
                // Handled at app level - selects preset and enables slicer
            }
            DeckMessage::SlicerTrigger(_idx) => {
                // Handled at app level via EngineCommand
            }
            DeckMessage::ResetSlicerPattern => {
                // Handled at app level via EngineCommand
            }
            DeckMessage::ShiftPressed => {
                self.shift_held = true;
            }
            DeckMessage::ShiftReleased => {
                self.shift_held = false;
            }
        }
    }

    /// Build the deck view
    ///
    /// Layout:
    /// ┌────────────────────────────────────────────────┐
    /// │ DECK 1 - Track name (BPM)                      │
    /// ├─────────────────┬──────────────────────────────┤
    /// │ STEM FX section │ 8 Hot Cue Buttons            │
    /// │ (tabs, knobs)   │ [1][2][3][4][5][6][7][8]     │
    /// ├─────────────────┴──────────────────────────────┤
    /// │ Transport: [◀◀][CUE][▶][▶▶] [÷2][4][×2][LOOP] │
    /// │ Pitch slider                                   │
    /// └────────────────────────────────────────────────┘
    pub fn view(&self) -> Element<'_, DeckMessage> {
        // Top: Deck label + track info (color indicates layer state)
        let deck_label_color = if self.midi_active {
            if self.is_secondary_layer {
                Color::from_rgb(0.2, 0.8, 0.2) // Green = Layer B
            } else {
                Color::from_rgb(0.9, 0.3, 0.3) // Red = Layer A
            }
        } else {
            Color::WHITE // Not targeted by MIDI
        };
        let deck_label = text(format!("DECK {}", self.deck_idx + 1))
            .size(16)
            .color(deck_label_color);

        let track_info = if self.track_name.is_empty() {
            text("No track loaded").size(12)
        } else {
            text(format!("{} ({:.1} BPM)", self.track_name, self.track_bpm)).size(12)
        };

        let header = row![deck_label, Space::new().width(10), track_info]
            .align_y(Center);

        // Left column: Stem FX section at top, transport controls stacked below
        let stems = self.view_stems();
        let transport = self.view_transport_vertical();

        let left_controls = column![
            stems,
            transport,
        ]
        .spacing(8);

        // Right column: Hot cues in 2x4 grid
        let hot_cues = self.view_hot_cues_grid();

        // Main content row: controls on left, hot cues on right
        let main_row = row![
            container(left_controls).width(Fill),
            container(hot_cues),
        ]
        .spacing(10);

        let content = column![
            header,
            main_row,
        ]
        .spacing(5)
        .padding(10);

        container(content)
            .width(Fill)
            .into()
    }

    /// Transport controls view
    fn view_transport(&self) -> Element<'_, DeckMessage> {
        // Beat jump buttons
        let jump_back = button(text("◀◀").size(14))
            .on_press(DeckMessage::BeatJumpBack)
            .padding(6);

        let jump_fwd = button(text("▶▶").size(14))
            .on_press(DeckMessage::BeatJumpForward)
            .padding(6);

        // Styled cue button - orange when cueing
        let is_cueing = matches!(self.state, PlayState::Cueing);
        let cue_style = if is_cueing {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(1.0, 0.6, 0.0))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::WHITE,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.5),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        let cue_btn = mouse_area(
            button(text("CUE").size(14))
                .padding(8)
                .style(move |_, _| cue_style)
        )
        .on_press(DeckMessage::CuePressed)
        .on_release(DeckMessage::CueReleased);

        // Styled play/pause toggle button - green when playing, shows pause icon
        let is_playing = matches!(self.state, PlayState::Playing);
        let play_icon = if is_playing { "⏸" } else { "▶" };
        let play_style = if is_playing {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.2, 0.8, 0.2))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::WHITE,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.5),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        let play_btn = button(text(play_icon).size(18))
            .on_press(DeckMessage::TogglePlayPause)
            .padding(8)
            .style(move |_, _| play_style);

        // Loop controls with length display
        let loop_text = if self.loop_active { "●" } else { "○" };
        let loop_btn = button(text(format!("LOOP {}", loop_text)).size(12))
            .on_press(DeckMessage::ToggleLoop)
            .padding(6);

        // Slip button (shows state)
        let slip_text = if self.slip_enabled { "SLIP ●" } else { "SLIP" };
        let slip_btn = button(text(slip_text).size(12))
            .on_press(DeckMessage::ToggleSlip)
            .padding(6);

        // Key match button (shows state)
        let key_text = if self.key_match_enabled { "KEY ●" } else { "KEY" };
        let key_btn = button(text(key_text).size(12))
            .on_press(DeckMessage::ToggleKeyMatch)
            .padding(6);

        let loop_halve = button(text("÷2").size(10))
            .on_press(DeckMessage::LoopHalve)
            .padding(4);

        let loop_length_text = format_loop_length(self.loop_length_beats);
        let loop_length = text(loop_length_text).size(10);

        let loop_double = button(text("×2").size(10))
            .on_press(DeckMessage::LoopDouble)
            .padding(4);

        row![
            jump_back,
            cue_btn,
            play_btn,
            jump_fwd,
            Space::new().width(10),
            loop_halve,
            loop_length,
            loop_double,
            loop_btn,
            slip_btn,
            key_btn,
        ]
        .spacing(3)
        .align_y(Center)
        .into()
    }

    /// Hot cue buttons view (CDJ-style with press/release for preview)
    fn view_hot_cues(&self) -> Element<'_, DeckMessage> {
        let buttons: Vec<Element<DeckMessage>> = (0..8)
            .map(|i| {
                let is_set = self.hot_cue_positions[i].is_some();
                let color = CUE_COLORS[i];

                // Create styled button based on whether hot cue is set
                let btn_style = if is_set {
                    // Colored button for set cues
                    button::Style {
                        background: Some(Background::Color(color)),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            color: Color::WHITE,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }
                } else {
                    // Gray button for empty slots
                    button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                        text_color: Color::from_rgb(0.5, 0.5, 0.5),
                        border: iced::Border {
                            color: Color::from_rgb(0.4, 0.4, 0.4),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    }
                };

                let label = format!("{}", i + 1);
                let btn = button(text(label).size(12))
                    .padding(8)
                    .style(move |_, _| btn_style);

                // Wrap in mouse_area for press/release detection (CDJ-style preview)
                mouse_area(btn)
                    .on_press(DeckMessage::HotCuePressed(i))
                    .on_release(DeckMessage::HotCueReleased(i))
                    .into()
            })
            .collect();

        Row::with_children(buttons)
            .spacing(3)
            .into()
    }

    /// Stem effect chain view with tabs
    fn view_stems(&self) -> Element<'_, DeckMessage> {
        // Tab buttons for selecting stem
        let tabs: Vec<Element<DeckMessage>> = (0..4)
            .map(|i| {
                let is_selected = i == self.selected_stem;
                let label = STEM_NAMES_SHORT[i];
                let style = if is_selected { "●" } else { "" };

                button(text(format!("{}{}", label, style)).size(11))
                    .on_press(DeckMessage::SelectStem(i))
                    .padding(5)
                    .into()
            })
            .collect();

        let tab_row = Row::with_children(tabs).spacing(2);

        // Selected stem's mute/solo and volume
        let stem_idx = self.selected_stem;
        let mute_label = if self.stem_muted[stem_idx] { "M●" } else { "M" };
        let solo_label = if self.stem_soloed[stem_idx] { "S●" } else { "S" };

        let stem_controls = row![
            text(STEM_NAMES[stem_idx]).size(12),
            button(text(mute_label).size(10))
                .on_press(DeckMessage::ToggleStemMute(stem_idx))
                .padding(4),
            button(text(solo_label).size(10))
                .on_press(DeckMessage::ToggleStemSolo(stem_idx))
                .padding(4),
        ]
        .spacing(5)
        .align_y(Center);

        // Macro knobs (shared across all stems)
        let knobs = self.view_chain_knobs(stem_idx);

        column![
            row![text("STEM FX").size(10), Space::new().width(Fill), tab_row].align_y(Center),
            stem_controls,
            knobs,
        ]
        .spacing(4)
        .into()
    }

    /// View the deck preset selector
    ///
    /// Shows a dropdown to select deck preset.
    fn view_effect_chain(&self, _stem_idx: usize) -> Element<'_, DeckMessage> {
        let preset = &self.deck_preset;

        // Preset dropdown button
        let label = preset
            .loaded_deck_preset
            .as_deref()
            .unwrap_or("No Preset");

        let dropdown_btn = button(
            row![text(label).size(9), Space::new().width(Fill), text("▾").size(9)]
                .spacing(4)
                .align_y(Center)
        )
        .on_press(DeckMessage::DeckPreset(DeckPresetMessage::TogglePicker))
        .padding([3, 6])
        .width(Fill);

        if preset.picker_open {
            // Show dropdown with preset list
            let picker_list = self.view_preset_picker_list();
            column![dropdown_btn, picker_list]
                .spacing(2)
                .width(Fill)
                .into()
        } else {
            dropdown_btn.into()
        }
    }

    /// View the preset picker dropdown list
    fn view_preset_picker_list(&self) -> Element<'_, DeckMessage> {
        let preset = &self.deck_preset;
        let mut items: Vec<Element<'_, DeckMessage>> = Vec::new();

        // "No Preset" option (passthrough)
        let no_preset_selected = preset.loaded_deck_preset.is_none();
        items.push(
            button(text("(No Preset)").size(9))
                .on_press(DeckMessage::DeckPreset(DeckPresetMessage::SelectDeckPreset(None)))
                .padding([3, 8])
                .width(Fill)
                .style(if no_preset_selected { preset_item_selected_style } else { preset_item_style })
                .into(),
        );

        // Available deck presets
        for preset_name in &preset.available_deck_presets {
            let is_selected = preset.loaded_deck_preset.as_ref() == Some(preset_name);
            let name = preset_name.clone();
            items.push(
                button(text(preset_name).size(9))
                    .on_press(DeckMessage::DeckPreset(DeckPresetMessage::SelectDeckPreset(Some(name))))
                    .padding([3, 8])
                    .width(Fill)
                    .style(if is_selected { preset_item_selected_style } else { preset_item_style })
                    .into(),
            );
        }

        let list = scrollable(column(items).spacing(1).width(Fill))
            .height(Length::Fixed(120.0));

        container(list)
            .padding(4)
            .width(Fill)
            .style(picker_container_style)
            .into()
    }

    /// View the shared macro knobs for the deck preset
    ///
    /// These are interactive sliders that control the shared macros in real-time
    /// across all stems.
    fn view_chain_knobs(&self, _stem_idx: usize) -> Element<'_, DeckMessage> {
        let preset = &self.deck_preset;

        let knobs: Vec<Element<DeckMessage>> = (0..DECK_PRESET_NUM_MACROS)
            .map(|k| {
                let value = preset.macro_values[k];
                let name = preset.macro_name(k);
                // Truncate name if too long
                let display_name = if name.len() > 4 {
                    format!("{}…", &name[..3])
                } else {
                    name.to_string()
                };

                column![
                    text(display_name).size(7),
                    slider(0.0..=1.0, value, move |v| DeckMessage::DeckPreset(
                        DeckPresetMessage::SetMacro { index: k, value: v }
                    ))
                    .step(0.01)  // Ensure continuous control with 100 steps
                    .width(Fill),
                ]
                .spacing(1)
                .width(Fill)
                .align_x(Center)
                .into()
            })
            .collect();

        row![
            text("MACROS").size(9),
            Row::with_children(knobs).spacing(4).width(Fill),
        ]
        .spacing(5)
        .width(Fill)
        .align_y(Center)
        .into()
    }

    /// Vertical transport controls (new layout: stacked from bottom up)
    ///
    /// Layout (top to bottom):
    /// - Loop controls row: [÷2] [length] [×2] [LOOP] [SLIP]
    /// - Beat jump row: [◀◀] [▶▶]
    /// - Cue button
    /// - Play button
    fn view_transport_vertical(&self) -> Element<'_, DeckMessage> {
        // Play button (large, at bottom of stack but rendered last)
        let is_playing = matches!(self.state, PlayState::Playing);
        let play_icon = if is_playing { "⏸" } else { "▶" };
        let play_style = if is_playing {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.2, 0.8, 0.2))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::WHITE,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.5),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        let play_btn = button(text(play_icon).size(24))
            .on_press(DeckMessage::TogglePlayPause)
            .padding([12, 40])
            .style(move |_, _| play_style);

        // Cue button
        let is_cueing = matches!(self.state, PlayState::Cueing);
        let cue_style = if is_cueing {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(1.0, 0.6, 0.0))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::WHITE,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::from_rgb(0.5, 0.5, 0.5),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        let cue_btn = mouse_area(
            button(text("CUE").size(16))
                .padding([10, 36])
                .style(move |_, _| cue_style)
        )
        .on_press(DeckMessage::CuePressed)
        .on_release(DeckMessage::CueReleased);

        // Beat jump buttons (side by side)
        let jump_back = button(text("◀◀").size(16))
            .on_press(DeckMessage::BeatJumpBack)
            .padding([8, 20]);

        let jump_fwd = button(text("▶▶").size(16))
            .on_press(DeckMessage::BeatJumpForward)
            .padding([8, 20]);

        let beat_jump_row = row![jump_back, jump_fwd]
            .spacing(8)
            .align_y(Center);

        // Loop controls row
        let loop_halve = button(text("÷2").size(11))
            .on_press(DeckMessage::LoopHalve)
            .padding(5);

        let loop_length_text = format_loop_length(self.loop_length_beats);
        let loop_length = container(text(loop_length_text).size(11))
            .padding([5, 8]);

        let loop_double = button(text("×2").size(11))
            .on_press(DeckMessage::LoopDouble)
            .padding(5);

        let loop_text = if self.loop_active { "LOOP ●" } else { "LOOP" };
        let loop_btn = button(text(loop_text).size(11))
            .on_press(DeckMessage::ToggleLoop)
            .padding(5);

        let slip_text = if self.slip_enabled { "SLIP ●" } else { "SLIP" };
        let slip_btn = button(text(slip_text).size(11))
            .on_press(DeckMessage::ToggleSlip)
            .padding(5);

        let key_text = if self.key_match_enabled { "KEY ●" } else { "KEY" };
        let key_btn = button(text(key_text).size(11))
            .on_press(DeckMessage::ToggleKeyMatch)
            .padding(5);

        let loop_row = row![loop_halve, loop_length, loop_double, loop_btn, slip_btn, key_btn]
            .spacing(4)
            .align_y(Center);

        // Stack vertically: loop controls at top, then beat jump, cue, play at bottom
        column![
            loop_row,
            beat_jump_row,
            cue_btn,
            play_btn,
        ]
        .spacing(6)
        .align_x(Center)
        .into()
    }

    /// Hot cue buttons in 2x4 grid layout (fills available width)
    fn view_hot_cues_grid(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        let deck_idx = self.deck_idx;

        // Create 2 rows of 4 buttons each
        let make_button = |i: usize| -> Element<DeckMessage> {
            let is_set = self.hot_cue_positions[i].is_some();
            let is_highlighted = self.is_highlighted(HighlightTarget::DeckHotCue(deck_idx, i));
            let color = CUE_COLORS[i];

            // Determine border based on highlight state
            let border = if is_highlighted {
                Self::highlight_border()
            } else if is_set {
                iced::Border {
                    color,
                    width: 1.5,
                    radius: 4.0.into(),
                }
            } else {
                iced::Border {
                    color: Color::from_rgb(0.3, 0.3, 0.3),
                    width: 1.0,
                    radius: 4.0.into(),
                }
            };

            // When set: dimmed version of cue color (30% blend with dark background)
            // When not set: plain dark gray
            let btn_style = if is_set {
                let dimmed_color = Color::from_rgb(
                    0.15 + color.r * 0.35,
                    0.15 + color.g * 0.35,
                    0.15 + color.b * 0.35,
                );
                button::Style {
                    background: Some(Background::Color(dimmed_color)),
                    text_color: color, // Text shows the actual cue color
                    border,
                    ..Default::default()
                }
            } else {
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.18, 0.18, 0.18))),
                    text_color: Color::from_rgb(0.45, 0.45, 0.45),
                    border,
                    ..Default::default()
                }
            };

            let label = format!("{}", i + 1);
            let btn = button(text(label).size(14))
                .padding([12, 0])
                .width(Length::Fill)
                .style(move |_, _| btn_style);

            mouse_area(btn)
                .on_press(DeckMessage::HotCuePressed(i))
                .on_release(DeckMessage::HotCueReleased(i))
                .into()
        };

        // Row 1: buttons 1-4
        let row1 = row![
            make_button(0),
            make_button(1),
            make_button(2),
            make_button(3),
        ]
        .spacing(4)
        .width(Length::Fill);

        // Row 2: buttons 5-8
        let row2 = row![
            make_button(4),
            make_button(5),
            make_button(6),
            make_button(7),
        ]
        .spacing(4)
        .width(Length::Fill);

        column![row1, row2]
            .spacing(4)
            .width(Length::Fill)
            .into()
    }

    /// Action buttons grid - displays either hot cues or slicer based on mode
    fn view_action_buttons_grid(&self) -> Element<'_, DeckMessage> {
        match self.action_mode {
            ActionButtonMode::HotCue => self.view_hot_cues_grid(),
            ActionButtonMode::Slicer => self.view_slicer_grid(),
        }
    }

    /// Slicer buttons in 2x4 grid layout (fills available width)
    ///
    /// Each button represents a slice (0-7) and queues it for playback when pressed.
    /// Visual feedback shows which slice is currently playing and the queue state.
    fn view_slicer_grid(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        // Slicer color scheme - orange gradient for slices
        let slice_colors: [Color; 8] = [
            Color::from_rgb(1.0, 0.4, 0.1), // Slice 1 - bright orange
            Color::from_rgb(1.0, 0.5, 0.2), // Slice 2
            Color::from_rgb(1.0, 0.6, 0.2), // Slice 3
            Color::from_rgb(0.9, 0.6, 0.3), // Slice 4
            Color::from_rgb(0.9, 0.5, 0.3), // Slice 5
            Color::from_rgb(0.8, 0.5, 0.3), // Slice 6
            Color::from_rgb(0.8, 0.4, 0.2), // Slice 7
            Color::from_rgb(0.7, 0.4, 0.2), // Slice 8 - darker orange
        ];

        // Create 2 rows of 4 buttons each
        let make_button = |i: usize| -> Element<DeckMessage> {
            let is_current = self.slicer_active && self.slicer_current_slice == i as u8;
            let is_in_queue = self.slicer_queue.contains(&(i as u8));
            let color = slice_colors[i];

            let btn_style = if is_current {
                // Currently playing slice - bright highlight
                button::Style {
                    background: Some(Background::Color(color)),
                    text_color: Color::WHITE,
                    border: iced::Border {
                        color: Color::WHITE,
                        width: 2.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            } else if is_in_queue && self.slicer_active {
                // In queue - dimmed version of color
                let dimmed_color = Color::from_rgb(
                    0.15 + color.r * 0.35,
                    0.15 + color.g * 0.35,
                    0.15 + color.b * 0.35,
                );
                button::Style {
                    background: Some(Background::Color(dimmed_color)),
                    text_color: color,
                    border: iced::Border {
                        color,
                        width: 1.5,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            } else {
                // Inactive - dark gray
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.18, 0.18, 0.18))),
                    text_color: Color::from_rgb(0.5, 0.35, 0.2),
                    border: iced::Border {
                        color: Color::from_rgb(0.35, 0.25, 0.15),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            };

            let label = format!("{}", i + 1);
            // Normal click = select preset, Shift+click = trigger slice for live adjustment
            let msg = if self.shift_held {
                DeckMessage::SlicerTrigger(i)
            } else {
                DeckMessage::SlicerPresetSelect(i)
            };
            let btn = button(text(label).size(14))
                .on_press(msg)
                .padding([12, 0])
                .width(Length::Fill)
                .style(move |_, _| btn_style);

            btn.into()
        };

        // Row 1: slices 1-4
        let row1 = row![
            make_button(0),
            make_button(1),
            make_button(2),
            make_button(3),
        ]
        .spacing(4)
        .width(Length::Fill);

        // Row 2: slices 5-8
        let row2 = row![
            make_button(4),
            make_button(5),
            make_button(6),
            make_button(7),
        ]
        .spacing(4)
        .width(Length::Fill);

        column![row1, row2]
            .spacing(4)
            .width(Length::Fill)
            .into()
    }

    // =========================================================================
    // Compact Layout (for side-by-side with waveforms)
    // =========================================================================

    /// Build compact deck controls for placement next to waveforms
    ///
    /// Layout:
    /// ```text
    /// ┌──────────────────────────────────────────────────────┐
    /// │ STEM CONTROLS (full width)                           │
    /// │ ┌─────────┬─────────────────────────────────────────┐│
    /// │ │ VOC     │ Effect Chain: [Reverb]->[Delay]->...    ││
    /// │ │ DRM     ├─────────────────────────────────────────┤│
    /// │ │ BAS     │ [K1][K2][K3][K4][K5][K6][K7][K8]       ││
    /// │ │ OTH     │                                         ││
    /// │ └─────────┴─────────────────────────────────────────┘│
    /// ├──────────────────────────────────────────────────────┤
    /// │ [LOOP][SLIP] [÷][4][×]  [◀◀][▶▶]                    │
    /// ├────────────┬─────────────────────────────────────────┤
    /// │ [  CUE   ] │  ┌─────┬─────┬─────┬─────┐            │
    /// │ [ PLAY   ] │  │  1  │  2  │  3  │  4  │            │
    /// │            │  ├─────┼─────┼─────┼─────┤            │
    /// │            │  │  5  │  6  │  7  │  8  │            │
    /// │            │  └─────┴─────┴─────┴─────┘            │
    /// └────────────┴─────────────────────────────────────────┘
    /// ```
    pub fn view_compact(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        // Top: Stem section (full width)
        let stem_section = self.view_stem_section_compact();

        // Middle: Loop/Slip + Loop size + Beat jump (horizontal, full width)
        let control_row = self.view_control_row_compact();

        // Mode row: [SHIFT] [HOTCUE] [SLICER] - switches action button behavior
        let mode_row = self.view_mode_row();

        // Bottom: CUE/PLAY (left) | Action Buttons (right) - aligned vertically
        // Action buttons show hotcues or slicer grid depending on mode
        let cue_play_col = self.view_cue_play_compact();

        let action_buttons_col = container(self.view_action_buttons_grid())
            .width(Length::Fill);

        let bottom_section = row![cue_play_col, action_buttons_col]
            .spacing(12)
            .align_y(Center);

        column![stem_section, control_row, mode_row, bottom_section]
            .spacing(8)
            .padding(8)
            .into()
    }

    /// Compact stem section with horizontal tabs above rotary knobs
    ///
    /// Layout:
    /// ```text
    /// ┌─────────────────────────────────────────────────────────────┐
    /// │ [VOC][DRM][BAS][OTH]  [M][S]  Chain: [Reverb●]─[Delay●]─[+] │
    /// ├─────────────────────────────────────────────────────────────┤
    /// │   (1)   (2)   (3)   (4)   (5)   (6)   (7)   (8)             │
    /// │  rotary knobs for effect chain parameters                   │
    /// └─────────────────────────────────────────────────────────────┘
    /// ```
    fn view_stem_section_compact(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        let stem_idx = self.selected_stem;

        // Horizontal stem tabs (fixed width buttons)
        let stem_tabs: Vec<Element<DeckMessage>> = (0..4)
            .map(|i| {
                let is_selected = i == self.selected_stem;
                let label = STEM_NAMES_SHORT[i];

                let btn_style = if is_selected {
                    button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.35, 0.35, 0.4))),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            color: Color::from_rgb(0.5, 0.5, 0.6),
                            width: 1.0,
                            radius: 3.0.into(),
                        },
                        ..Default::default()
                    }
                } else {
                    button::Style {
                        background: Some(Background::Color(Color::from_rgb(0.2, 0.2, 0.2))),
                        text_color: Color::from_rgb(0.7, 0.7, 0.7),
                        border: iced::Border {
                            color: Color::from_rgb(0.3, 0.3, 0.3),
                            width: 1.0,
                            radius: 3.0.into(),
                        },
                        ..Default::default()
                    }
                };

                button(text(label).size(10))
                    .on_press(DeckMessage::SelectStem(i))
                    .padding([4, 8])
                    .width(Length::Fixed(40.0))
                    .style(move |_, _| btn_style)
                    .into()
            })
            .collect();

        let tabs_row = Row::with_children(stem_tabs).spacing(2);

        // Mute/Solo buttons for selected stem
        let mute_label = if self.stem_muted[stem_idx] { "M●" } else { "M" };
        let solo_label = if self.stem_soloed[stem_idx] { "S●" } else { "S" };

        let mute_btn = button(text(mute_label).size(10))
            .on_press(DeckMessage::ToggleStemMute(stem_idx))
            .padding([4, 6])
            .width(Length::Fixed(28.0));

        // Wrap mute button with optional MIDI learn highlight
        let mute_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckStemMute(self.deck_idx, stem_idx)) {
            container(mute_btn)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            mute_btn.into()
        };

        let solo_btn = button(text(solo_label).size(10))
            .on_press(DeckMessage::ToggleStemSolo(stem_idx))
            .padding([4, 6])
            .width(Length::Fixed(28.0));

        // Top row: tabs + M/S
        let top_row = row![
            tabs_row,
            Space::new().width(8),
            mute_elem,
            solo_btn,
        ]
        .spacing(2)
        .align_y(Center);

        // Rotary knobs row
        let knobs = self.view_chain_knobs_compact(stem_idx);

        column![top_row, knobs]
            .spacing(6)
            .width(Length::Fill)
            .into()
    }

    /// Compact deck preset selector view
    fn view_effect_chain_compact(&self, _stem_idx: usize) -> Element<'_, DeckMessage> {
        let preset = &self.deck_preset;

        // Preset dropdown button
        let label = preset
            .loaded_deck_preset
            .as_deref()
            .unwrap_or("No Preset");

        let dropdown_btn = button(
            row![text(label).size(9), Space::new().width(Fill), text("▾").size(9)]
                .spacing(2)
                .align_y(Center)
        )
        .on_press(DeckMessage::DeckPreset(DeckPresetMessage::TogglePicker))
        .padding(2)
        .width(Fill);

        if preset.picker_open {
            // Show dropdown with preset list
            let picker_list = self.view_preset_picker_list();
            column![dropdown_btn, picker_list]
                .spacing(2)
                .width(Fill)
                .into()
        } else {
            dropdown_btn.into()
        }
    }

    /// Compact macro sliders for real-time control (shared across all stems)
    fn view_chain_knobs_compact(&self, _stem_idx: usize) -> Element<'_, DeckMessage> {
        let preset = &self.deck_preset;

        let knobs: Vec<Element<DeckMessage>> = (0..DECK_PRESET_NUM_MACROS)
            .map(|k| {
                let value = preset.macro_values[k];
                let name = preset.macro_name(k);
                // Truncate name if too long
                let display_name = if name.len() > 3 {
                    format!("{}…", &name[..2])
                } else {
                    name.to_string()
                };

                column![
                    text(display_name).size(6),
                    slider(0.0..=1.0, value, move |v| DeckMessage::DeckPreset(
                        DeckPresetMessage::SetMacro { index: k, value: v }
                    ))
                    .step(0.01)  // Ensure continuous control with 100 steps
                    .width(Fill),
                ]
                .spacing(1)
                .width(Fill)
                .align_x(Center)
                .into()
            })
            .collect();

        Row::with_children(knobs)
            .spacing(4)
            .width(Fill)
            .align_y(Center)
            .into()
    }

    /// Horizontal control row: Loop/Slip, Loop size, Beat jump
    fn view_control_row_compact(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        // Loop button with optional MIDI learn highlight
        let loop_text = if self.loop_active { "LOOP ●" } else { "LOOP" };
        let loop_btn = button(text(loop_text).size(10))
            .on_press(DeckMessage::ToggleLoop)
            .padding([4, 8])
            .width(Length::Fixed(60.0));
        let loop_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckLoop(self.deck_idx)) {
            container(loop_btn)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            loop_btn.into()
        };

        // Slip button
        let slip_text = if self.slip_enabled { "SLIP ●" } else { "SLIP" };
        let slip_btn = button(text(slip_text).size(10))
            .on_press(DeckMessage::ToggleSlip)
            .padding([4, 8])
            .width(Length::Fixed(60.0));

        // Key match button
        let key_text = if self.key_match_enabled { "KEY ●" } else { "KEY" };
        let key_btn = button(text(key_text).size(10))
            .on_press(DeckMessage::ToggleKeyMatch)
            .padding([4, 8])
            .width(Length::Fixed(60.0));

        // Loop length controls with optional MIDI learn highlights
        let loop_halve = button(text("÷2").size(10))
            .on_press(DeckMessage::LoopHalve)
            .padding([4, 6]);

        let loop_length_text = format_loop_length(self.loop_length_beats);
        let loop_length = container(text(loop_length_text).size(11))
            .padding([4, 8])
            .width(Length::Fixed(32.0))
            .center_x(Length::Fill);

        let loop_double = button(text("×2").size(10))
            .on_press(DeckMessage::LoopDouble)
            .padding([4, 6]);

        let loop_size_row = row![loop_halve, loop_length, loop_double].spacing(2);
        let loop_size_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckLoopEncoder(self.deck_idx)) {
            container(loop_size_row)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            loop_size_row.into()
        };

        // Beat jump buttons with optional MIDI learn highlights
        let jump_back = button(text("◀◀").size(12))
            .on_press(DeckMessage::BeatJumpBack)
            .padding([4, 8])
            .width(Length::Fixed(60.0));
        let jump_back_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckBeatJumpBack(self.deck_idx)) {
            container(jump_back)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            jump_back.into()
        };

        let jump_fwd = button(text("▶▶").size(12))
            .on_press(DeckMessage::BeatJumpForward)
            .padding([4, 8])
            .width(Length::Fixed(60.0));
        let jump_fwd_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckBeatJumpForward(self.deck_idx)) {
            container(jump_fwd)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            jump_fwd.into()
        };

        row![
            jump_back_elem,
            jump_fwd_elem,
            Space::new().width(8),
            loop_size_elem,
            Space::new().width(8),
            loop_elem,
            slip_btn,
            key_btn,
        ]
        .spacing(4)
        .align_y(Center)
        .into()
    }

    /// Mode selection row: [SHIFT] [HOTCUE] [SLICER]
    ///
    /// Determines the behavior of the 8 action buttons below
    fn view_mode_row(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        // Shift button - same width as CUE button for alignment
        let shift_text = if self.shift_held { "SHIFT ●" } else { "SHIFT" };
        let shift_style = if self.shift_held {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.5, 0.4, 0.2))),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: Color::from_rgb(0.8, 0.6, 0.2),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                text_color: Color::from_rgb(0.8, 0.8, 0.8),
                border: iced::Border {
                    color: Color::from_rgb(0.4, 0.4, 0.4),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        let shift_btn = mouse_area(
            button(text(shift_text).size(10))
                .padding([6, 12])
                .width(Length::Fixed(70.0))
                .style(move |_, _| shift_style)
        )
        .on_press(DeckMessage::ShiftPressed)
        .on_release(DeckMessage::ShiftReleased);

        // Helper to build styled mode button
        let build_mode_style = |is_active: bool| -> button::Style {
            if is_active {
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.3, 0.5, 0.3))),
                    text_color: Color::WHITE,
                    border: iced::Border {
                        color: Color::from_rgb(0.4, 0.7, 0.4),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            } else {
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                    text_color: Color::from_rgb(0.7, 0.7, 0.7),
                    border: iced::Border {
                        color: Color::from_rgb(0.4, 0.4, 0.4),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            }
        };

        // HOTCUE mode button
        let hotcue_is_active = self.action_mode == ActionButtonMode::HotCue;
        let hotcue_style = build_mode_style(hotcue_is_active);
        let hotcue_btn = button(text("HOTCUE").size(10))
            .on_press(DeckMessage::SetActionMode(ActionButtonMode::HotCue))
            .padding([6, 12])
            .width(Length::Fixed(60.0))
            .style(move |_, _| hotcue_style);

        // SLICER mode button
        let slicer_is_active = self.action_mode == ActionButtonMode::Slicer;
        let slicer_style = if slicer_is_active {
            button::Style {
                background: Some(Background::Color(if self.slicer_active {
                    Color::from_rgb(0.6, 0.4, 0.2) // Orange when slicer is running
                } else {
                    Color::from_rgb(0.3, 0.5, 0.3) // Green when selected but not running
                })),
                text_color: Color::WHITE,
                border: iced::Border {
                    color: if self.slicer_active {
                        Color::from_rgb(0.9, 0.6, 0.2)
                    } else {
                        Color::from_rgb(0.4, 0.7, 0.4)
                    },
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
                text_color: Color::from_rgb(0.7, 0.7, 0.7),
                border: iced::Border {
                    color: Color::from_rgb(0.4, 0.4, 0.4),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            }
        };

        // If shift is held and slicer mode is active, clicking resets the pattern
        let slicer_btn_msg = if self.shift_held && self.action_mode == ActionButtonMode::Slicer {
            DeckMessage::ResetSlicerPattern
        } else {
            DeckMessage::SetActionMode(ActionButtonMode::Slicer)
        };

        let slicer_btn = button(text("SLICER").size(10))
            .on_press(slicer_btn_msg)
            .padding([6, 12])
            .width(Length::Fixed(70.0))
            .style(move |_, _| slicer_style);

        // Wrap mode buttons with optional MIDI learn highlights
        let hotcue_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckHotCueMode(self.deck_idx)) {
            container(hotcue_btn)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            hotcue_btn.into()
        };

        let slicer_elem: Element<_> = if self.is_highlighted(HighlightTarget::DeckSlicerMode(self.deck_idx)) {
            container(slicer_btn)
                .style(|_| container::Style {
                    border: Self::highlight_border(),
                    ..Default::default()
                })
                .into()
        } else {
            slicer_btn.into()
        };

        row![
            shift_btn,
            Space::new().width(12),
            hotcue_elem,
            slicer_elem,
        ]
        .spacing(4)
        .align_y(Center)
        .into()
    }

    /// CUE and PLAY buttons column (fixed width, left-aligned)
    fn view_cue_play_compact(&self) -> Element<'_, DeckMessage> {
        use iced::Length;

        const BUTTON_WIDTH: f32 = 70.0;

        // Check for highlight targets
        let highlight_cue = self.is_highlighted(HighlightTarget::DeckCue(self.deck_idx));
        let highlight_play = self.is_highlighted(HighlightTarget::DeckPlay(self.deck_idx));

        // Cue button
        let is_cueing = matches!(self.state, PlayState::Cueing);
        let cue_border = if highlight_cue {
            Self::highlight_border()
        } else if is_cueing {
            iced::Border {
                color: Color::WHITE,
                width: 1.0,
                radius: 4.0.into(),
            }
        } else {
            iced::Border {
                color: Color::from_rgb(0.5, 0.5, 0.5),
                width: 1.0,
                radius: 4.0.into(),
            }
        };

        let cue_style = if is_cueing {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(1.0, 0.6, 0.0))),
                text_color: Color::WHITE,
                border: cue_border,
                ..Default::default()
            }
        } else {
            button::Style {
                background: Some(Background::Color(Color::from_rgb(0.3, 0.3, 0.3))),
                text_color: Color::WHITE,
                border: cue_border,
                ..Default::default()
            }
        };

        let cue_btn = mouse_area(
            button(text("CUE").size(14))
                .padding([12, 16])
                .width(Length::Fixed(BUTTON_WIDTH))
                .style(move |_, _| cue_style)
        )
        .on_press(DeckMessage::CuePressed)
        .on_release(DeckMessage::CueReleased);

        // Play button - sleek minimal style with unicode icons
        let is_playing = matches!(self.state, PlayState::Playing);
        let play_label = if is_playing { "❚❚" } else { "▶" }; // Unicode pause (two bars) and play

        let play_border = if highlight_play {
            Self::highlight_border()
        } else {
            iced::Border {
                color: Color::from_rgb(0.4, 0.4, 0.4),
                width: 1.0,
                radius: 4.0.into(),
            }
        };

        let play_style = button::Style {
            background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
            text_color: Color::WHITE,
            border: play_border,
            ..Default::default()
        };

        let play_btn = button(text(play_label).size(14))
            .on_press(DeckMessage::TogglePlayPause)
            .padding([12, 16])
            .width(Length::Fixed(BUTTON_WIDTH))
            .style(move |_, _| play_style);

        column![cue_btn, play_btn]
            .spacing(6)
            .align_x(iced::Alignment::Start)
            .width(Length::Shrink)
            .into()
    }
}

/// Format loop length for display
fn format_loop_length(beats: f32) -> String {
    if beats < 1.0 {
        format!("1/{:.0}", 1.0 / beats)
    } else {
        format!("{:.0}", beats)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Preset picker styles
// ─────────────────────────────────────────────────────────────────────────────

const PICKER_BG_DARK: Color = Color::from_rgb(0.12, 0.12, 0.14);
const PICKER_BORDER: Color = Color::from_rgb(0.35, 0.35, 0.40);
const PICKER_ACCENT: Color = Color::from_rgb(0.3, 0.7, 0.9);

fn preset_item_style(_theme: &iced::Theme, _status: iced::widget::button::Status) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: Some(Background::Color(PICKER_BG_DARK)),
        text_color: Color::from_rgb(0.9, 0.9, 0.9),
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn preset_item_selected_style(_theme: &iced::Theme, _status: iced::widget::button::Status) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: Some(Background::Color(PICKER_ACCENT)),
        text_color: Color::WHITE,
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    }
}

fn picker_container_style(_theme: &iced::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(Background::Color(PICKER_BG_DARK)),
        border: iced::Border {
            color: PICKER_BORDER,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    }
}
