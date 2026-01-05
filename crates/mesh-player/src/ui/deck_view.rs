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
use mesh_widgets::CUE_COLORS;
use mesh_core::types::PlayState;

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
    /// Per-stem volumes
    stem_volumes: [f32; 4],
    /// Pitch adjustment (-8% to +8%)
    pitch: f64,
    /// Loop active
    loop_active: bool,
    /// Current loop length in beats
    loop_length_beats: f32,
    /// Currently selected stem for effect chain view (0-3)
    selected_stem: usize,
    /// Effect chain knob values per stem (8 knobs x 4 stems)
    stem_knobs: [[f32; 8]; 4],
    /// Effect names in each stem's chain (for display)
    stem_effect_names: [Vec<String>; 4],
    /// Effect bypass states per stem
    stem_effect_bypassed: [Vec<bool>; 4],
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
    /// Set beat jump size in beats (1, 4, 8, 16, 32)
    SetBeatJumpSize(i32),
    /// Sync to master
    Sync,
    /// Toggle loop
    ToggleLoop,
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
    /// Adjust pitch
    SetPitch(f64),
    /// Toggle stem mute
    ToggleStemMute(usize),
    /// Toggle stem solo
    ToggleStemSolo(usize),
    /// Set stem volume
    SetStemVolume(usize, f32),
    /// Select stem tab for effect chain view
    SelectStem(usize),
    /// Set effect chain knob value (stem_idx, knob_idx, value)
    SetStemKnob(usize, usize, f32),
    /// Toggle effect bypass in chain (stem_idx, effect_idx)
    ToggleEffectBypass(usize, usize),
}

/// Loop length labels for display
const LOOP_LENGTHS: [f32; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

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
            stem_volumes: [1.0; 4],
            pitch: 0.0,
            loop_active: false,
            loop_length_beats: 4.0, // Default 4 beats
            selected_stem: 0,       // Start with Vocals selected
            stem_knobs: [[0.0; 8]; 4],
            stem_effect_names: Default::default(),
            stem_effect_bypassed: Default::default(),
        }
    }

    /// Sync view state from deck
    pub fn sync_from_deck(&mut self, deck: &Deck) {
        self.state = deck.state();
        self.position = deck.position();
        self.loop_active = deck.loop_state().active;

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
            DeckMessage::SetBeatJumpSize(beats) => deck.set_beat_jump_size(beats),
            DeckMessage::Sync => {
                // Sync is handled at engine level
            }
            DeckMessage::ToggleLoop => {
                deck.toggle_loop();
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
            DeckMessage::SetPitch(pitch) => {
                self.pitch = pitch;
                // TODO: implement pitch control
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
            DeckMessage::SetStemVolume(stem_idx, vol) => {
                self.stem_volumes[stem_idx] = vol;
                // TODO: implement stem volume control (need to add to effect chain)
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

        // Middle row: FX section (left) | Hot cues (right)
        let stems = self.view_stems();
        let hot_cues = self.view_hot_cues();

        let middle_row = row![
            container(stems).width(Fill),
            container(hot_cues).width(Fill),
        ]
        .spacing(10);

        // Bottom: Transport + pitch
        let transport = self.view_transport();
        let pitch_section = self.view_pitch();

        let content = column![
            header,
            middle_row,
            transport,
            pitch_section,
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
            slider(0.0..=1.0, self.stem_volumes[stem_idx], move |v| DeckMessage::SetStemVolume(stem_idx, v))
                .width(60),
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

    /// Pitch slider view
    fn view_pitch(&self) -> Element<DeckMessage> {
        row![
            text("PITCH").size(10),
            slider(-8.0..=8.0, self.pitch, DeckMessage::SetPitch)
                .step(0.01)
                .width(150),
            text(format!("{:+.1}%", self.pitch)).size(10),
        ]
        .spacing(10)
        .align_y(Center)
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
