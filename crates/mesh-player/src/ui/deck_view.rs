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

use iced::widget::{button, column, container, mouse_area, row, slider, text, Row, Space};
use iced::{Background, Center, Color, Element, Fill};

use mesh_core::engine::Deck;
use mesh_core::types::PlayState;
use mesh_widgets::{rotary_knob, RotaryKnobState, CUE_COLORS};

/// Stem names for display
pub const STEM_NAMES: [&str; 4] = ["Vocals", "Drums", "Bass", "Other"];
/// Short stem names for compact display
pub const STEM_NAMES_SHORT: [&str; 4] = ["VOC", "DRM", "BAS", "OTH"];

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
    /// Effect chain knob values per stem (8 knobs x 4 stems)
    stem_knobs: [[f32; 8]; 4],
    /// Effect names in each stem's chain (for display)
    stem_effect_names: [Vec<String>; 4],
    /// Effect bypass states per stem
    stem_effect_bypassed: [Vec<bool>; 4],
    /// Rotary knob states for rendering (8 knobs, shared across stems)
    knob_states: [RotaryKnobState; 8],
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
    /// Select stem tab for effect chain view
    SelectStem(usize),
    /// Set effect chain knob value (stem_idx, knob_idx, value)
    SetStemKnob(usize, usize, f32),
    /// Toggle effect bypass in chain (stem_idx, effect_idx)
    ToggleEffectBypass(usize, usize),
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
            stem_knobs: [[0.0; 8]; 4],
            stem_effect_names: Default::default(),
            stem_effect_bypassed: Default::default(),
            knob_states: Default::default(),
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

        // Sync stem states and effect chains
        for i in 0..4 {
            if let Some(chain) = deck.stem_chain(i) {
                self.stem_muted[i] = chain.is_muted();
                self.stem_soloed[i] = chain.is_soloed();

                // Sync effect chain info
                let effect_count = chain.effect_count();
                self.stem_effect_names[i].clear();
                self.stem_effect_bypassed[i].clear();

                for j in 0..effect_count {
                    if let Some(effect) = chain.get_effect(j) {
                        self.stem_effect_names[i].push(effect.info().name.clone());
                        self.stem_effect_bypassed[i].push(effect.is_bypassed());
                    }
                }

                // Sync knob values
                for k in 0..8 {
                    self.stem_knobs[i][k] = chain.get_knob(k);
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

    /// Check if key matching is enabled
    pub fn key_match_enabled(&self) -> bool {
        self.key_match_enabled
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
                if let Some(chain) = deck.stem_chain_mut(stem_idx) {
                    chain.set_muted(!chain.is_muted());
                }
            }
            DeckMessage::ToggleStemSolo(stem_idx) => {
                if let Some(chain) = deck.stem_chain_mut(stem_idx) {
                    chain.set_soloed(!chain.is_soloed());
                }
            }
            DeckMessage::SelectStem(stem_idx) => {
                if stem_idx < 4 {
                    self.selected_stem = stem_idx;
                }
            }
            DeckMessage::SetStemKnob(stem_idx, knob_idx, value) => {
                if stem_idx < 4 && knob_idx < 8 {
                    self.stem_knobs[stem_idx][knob_idx] = value;
                    if let Some(chain) = deck.stem_chain_mut(stem_idx) {
                        chain.set_knob(knob_idx, value);
                    }
                }
            }
            DeckMessage::ToggleEffectBypass(stem_idx, effect_idx) => {
                if let Some(chain) = deck.stem_chain_mut(stem_idx) {
                    if let Some(effect) = chain.get_effect_mut(effect_idx) {
                        effect.set_bypass(!effect.is_bypassed());
                    }
                }
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
    pub fn view(&self) -> Element<DeckMessage> {
        // Top: Deck label + track info
        let deck_label = text(format!("DECK {}", self.deck_idx + 1))
            .size(16);

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
    fn view_transport(&self) -> Element<DeckMessage> {
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
    fn view_hot_cues(&self) -> Element<DeckMessage> {
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
    fn view_stems(&self) -> Element<DeckMessage> {
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

        // Effect chain visualization
        let effect_chain = self.view_effect_chain(stem_idx);

        // 8 mappable knobs
        let knobs = self.view_chain_knobs(stem_idx);

        column![
            row![text("STEM FX").size(10), Space::new().width(Fill), tab_row].align_y(Center),
            stem_controls,
            effect_chain,
            knobs,
        ]
        .spacing(4)
        .into()
    }

    /// View the effect chain for a stem
    fn view_effect_chain(&self, stem_idx: usize) -> Element<DeckMessage> {
        let effects = &self.stem_effect_names[stem_idx];
        let bypassed = &self.stem_effect_bypassed[stem_idx];

        if effects.is_empty() {
            return row![
                text("Chain: ").size(10),
                text("(empty)").size(10),
                button(text("+").size(10)).padding(3),
            ]
            .spacing(3)
            .align_y(Center)
            .into();
        }

        // Build effect chain display: [Effect1 ●]──[Effect2 ◯]──[+]
        let mut chain_elements: Vec<Element<DeckMessage>> = Vec::new();
        chain_elements.push(text("Chain: ").size(10).into());

        for (i, name) in effects.iter().enumerate() {
            let is_bypassed = bypassed.get(i).copied().unwrap_or(false);
            let bypass_indicator = if is_bypassed { "◯" } else { "●" };

            // Shorten effect name if needed
            let short_name: String = name.chars().take(8).collect();
            let label = format!("{} {}", short_name, bypass_indicator);

            let effect_btn = button(text(label).size(9))
                .on_press(DeckMessage::ToggleEffectBypass(stem_idx, i))
                .padding(3);

            chain_elements.push(effect_btn.into());

            // Add connector if not last
            if i < effects.len() - 1 {
                chain_elements.push(text("─").size(10).into());
            }
        }

        // Add button for adding new effects
        chain_elements.push(text("─").size(10).into());
        chain_elements.push(button(text("+").size(10)).padding(3).into());

        Row::with_children(chain_elements)
            .spacing(2)
            .align_y(Center)
            .into()
    }

    /// View the 8 mappable knobs for the stem's effect chain
    fn view_chain_knobs(&self, stem_idx: usize) -> Element<DeckMessage> {
        let knobs: Vec<Element<DeckMessage>> = (0..8)
            .map(|k| {
                let value = self.stem_knobs[stem_idx][k];
                column![
                    text(format!("{}", k + 1)).size(8),
                    slider(0.0..=1.0, value, move |v| DeckMessage::SetStemKnob(stem_idx, k, v))
                        .width(30),
                ]
                .spacing(1)
                .align_x(Center)
                .into()
            })
            .collect();

        row![
            text("KNOBS").size(8),
            Row::with_children(knobs).spacing(4),
        ]
        .spacing(5)
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
    fn view_transport_vertical(&self) -> Element<DeckMessage> {
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
    fn view_hot_cues_grid(&self) -> Element<DeckMessage> {
        use iced::Length;

        // Create 2 rows of 4 buttons each
        let make_button = |i: usize| -> Element<DeckMessage> {
            let is_set = self.hot_cue_positions[i].is_some();
            let color = CUE_COLORS[i];

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
                    border: iced::Border {
                        color,
                        width: 1.5,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            } else {
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.18, 0.18, 0.18))),
                    text_color: Color::from_rgb(0.45, 0.45, 0.45),
                    border: iced::Border {
                        color: Color::from_rgb(0.3, 0.3, 0.3),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
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
    pub fn view_compact(&self) -> Element<DeckMessage> {
        use iced::Length;

        // Top: Stem section (full width)
        let stem_section = self.view_stem_section_compact();

        // Middle: Loop/Slip + Loop size + Beat jump (horizontal, full width)
        let control_row = self.view_control_row_compact();

        // Bottom: CUE/PLAY (left) | Hot Cues (right) - aligned vertically
        let cue_play_col = self.view_cue_play_compact();

        let hot_cues_col = container(self.view_hot_cues_grid())
            .width(Length::Fill);

        let bottom_section = row![cue_play_col, hot_cues_col]
            .spacing(12)
            .align_y(Center);

        column![stem_section, control_row, bottom_section]
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
    fn view_stem_section_compact(&self) -> Element<DeckMessage> {
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

        let solo_btn = button(text(solo_label).size(10))
            .on_press(DeckMessage::ToggleStemSolo(stem_idx))
            .padding([4, 6])
            .width(Length::Fixed(28.0));

        // Effect chain visualization
        let effect_chain = self.view_effect_chain_compact(stem_idx);

        // Top row: tabs + M/S + effect chain
        let top_row = row![
            tabs_row,
            Space::new().width(8),
            mute_btn,
            solo_btn,
            Space::new().width(8),
            effect_chain,
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

    /// Compact effect chain view
    fn view_effect_chain_compact(&self, stem_idx: usize) -> Element<DeckMessage> {
        let effects = &self.stem_effect_names[stem_idx];
        let bypassed = &self.stem_effect_bypassed[stem_idx];

        let mut chain_elements: Vec<Element<DeckMessage>> = Vec::new();

        if effects.is_empty() {
            chain_elements.push(text("Chain: (empty)").size(9).into());
            chain_elements.push(button(text("+").size(9)).padding(2).into());
        } else {
            for (i, name) in effects.iter().enumerate() {
                let is_bypassed = bypassed.get(i).copied().unwrap_or(false);
                let indicator = if is_bypassed { "◯" } else { "●" };
                let short_name: String = name.chars().take(6).collect();

                let effect_btn = button(text(format!("{}{}", short_name, indicator)).size(9))
                    .on_press(DeckMessage::ToggleEffectBypass(stem_idx, i))
                    .padding(2);

                chain_elements.push(effect_btn.into());

                if i < effects.len() - 1 {
                    chain_elements.push(text("→").size(9).into());
                }
            }
            chain_elements.push(text("→").size(9).into());
            chain_elements.push(button(text("+").size(9)).padding(2).into());
        }

        Row::with_children(chain_elements)
            .spacing(2)
            .align_y(Center)
            .into()
    }

    /// Compact knob row using rotary knobs
    fn view_chain_knobs_compact(&self, stem_idx: usize) -> Element<DeckMessage> {
        const KNOB_SIZE: f32 = 32.0;
        const KNOB_LABELS: [&str; 8] = ["1", "2", "3", "4", "5", "6", "7", "8"];

        let knobs: Vec<Element<DeckMessage>> = (0..8)
            .map(|k| {
                let value = self.stem_knobs[stem_idx][k];
                rotary_knob(
                    &self.knob_states[k],
                    value,
                    KNOB_SIZE,
                    Some(KNOB_LABELS[k]),
                    move |v| DeckMessage::SetStemKnob(stem_idx, k, v),
                )
            })
            .collect();

        Row::with_children(knobs)
            .spacing(4)
            .align_y(Center)
            .into()
    }

    /// Horizontal control row: Loop/Slip, Loop size, Beat jump
    fn view_control_row_compact(&self) -> Element<DeckMessage> {
        use iced::Length;

        // Loop button
        let loop_text = if self.loop_active { "LOOP ●" } else { "LOOP" };
        let loop_btn = button(text(loop_text).size(10))
            .on_press(DeckMessage::ToggleLoop)
            .padding([4, 8])
            .width(Length::Fixed(60.0));

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

        // Loop length controls
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

        // Beat jump buttons (same width as LOOP/SLIP)
        let jump_back = button(text("◀◀").size(12))
            .on_press(DeckMessage::BeatJumpBack)
            .padding([4, 8])
            .width(Length::Fixed(60.0));

        let jump_fwd = button(text("▶▶").size(12))
            .on_press(DeckMessage::BeatJumpForward)
            .padding([4, 8])
            .width(Length::Fixed(60.0));

        row![
            jump_back,
            jump_fwd,
            Space::new().width(8),
            loop_halve,
            loop_length,
            loop_double,
            Space::new().width(8),
            loop_btn,
            slip_btn,
            key_btn,
        ]
        .spacing(4)
        .align_y(Center)
        .into()
    }

    /// CUE and PLAY buttons column (fixed width, left-aligned)
    fn view_cue_play_compact(&self) -> Element<DeckMessage> {
        use iced::Length;

        const BUTTON_WIDTH: f32 = 70.0;

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
        let play_style = button::Style {
            background: Some(Background::Color(Color::from_rgb(0.25, 0.25, 0.25))),
            text_color: Color::WHITE,
            border: iced::Border {
                color: Color::from_rgb(0.4, 0.4, 0.4),
                width: 1.0,
                radius: 4.0.into(),
            },
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
