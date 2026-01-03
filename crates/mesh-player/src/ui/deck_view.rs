//! Deck view component
//!
//! Displays a single DJ deck with:
//! - Track info and waveform
//! - Transport controls (play/pause, cue, sync)
//! - Hot cue buttons
//! - Pitch/tempo fader
//! - Stem controls (mute/solo/volume per stem)

use iced::widget::{button, column, container, row, slider, text, Row, Space};
use iced::{Center, Element, Fill};

use mesh_core::engine::Deck;
use mesh_core::types::PlayState;

use super::waveform::WaveformView;

/// State for a deck view
pub struct DeckView {
    /// Deck index (0-3)
    deck_idx: usize,
    /// Waveform view
    #[allow(dead_code)]
    waveform: WaveformView,
    /// Current playback state
    state: PlayState,
    /// Current position (samples)
    position: u64,
    /// Track BPM
    track_bpm: f64,
    /// Track filename
    track_name: String,
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
}

/// Messages for deck interaction
#[derive(Debug, Clone)]
pub enum DeckMessage {
    /// Play button pressed
    Play,
    /// Pause button pressed
    Pause,
    /// Cue button pressed
    CuePressed,
    /// Cue button released
    CueReleased,
    /// Set cue point
    SetCue,
    /// Trigger hot cue (0-7)
    HotCue(usize),
    /// Set hot cue at current position
    SetHotCue(usize),
    /// Clear hot cue
    ClearHotCue(usize),
    /// Sync to master
    Sync,
    /// Toggle loop
    ToggleLoop,
    /// Set loop length (beats)
    SetLoopLength(u32),
    /// Adjust pitch
    SetPitch(f64),
    /// Toggle stem mute
    ToggleStemMute(usize),
    /// Toggle stem solo
    ToggleStemSolo(usize),
    /// Set stem volume
    SetStemVolume(usize, f32),
    /// Seek to position
    Seek(f64),
}

impl DeckView {
    /// Create a new deck view
    pub fn new(deck_idx: usize) -> Self {
        Self {
            deck_idx,
            waveform: WaveformView::new(),
            state: PlayState::Stopped,
            position: 0,
            track_bpm: 0.0,
            track_name: String::new(),
            stem_muted: [false; 4],
            stem_soloed: [false; 4],
            stem_volumes: [1.0; 4],
            pitch: 0.0,
            loop_active: false,
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
        } else {
            self.track_bpm = 0.0;
            self.track_name = String::new();
        }

        // Sync stem states
        for i in 0..4 {
            if let Some(chain) = deck.stem_chain(i) {
                self.stem_muted[i] = chain.is_muted();
                self.stem_soloed[i] = chain.is_soloed();
            }
        }
    }

    /// Handle a deck message
    pub fn handle_message(&mut self, msg: DeckMessage, deck: Option<&mut Deck>) {
        let Some(deck) = deck else { return };

        match msg {
            DeckMessage::Play => deck.play(),
            DeckMessage::Pause => deck.pause(),
            DeckMessage::CuePressed => deck.cue_press(),
            DeckMessage::CueReleased => deck.cue_release(),
            DeckMessage::SetCue => deck.set_cue_point(),
            DeckMessage::HotCue(idx) => deck.trigger_hot_cue(idx),
            DeckMessage::SetHotCue(idx) => deck.set_hot_cue(idx),
            DeckMessage::ClearHotCue(idx) => deck.clear_hot_cue(idx),
            DeckMessage::Sync => {
                // Sync is handled at engine level
            }
            DeckMessage::ToggleLoop => {
                if deck.loop_state().active {
                    deck.loop_off();
                } else {
                    deck.loop_in();
                    // In a real implementation, we'd set loop out after N beats
                }
            }
            DeckMessage::SetLoopLength(beats) => {
                // Set loop length in beats
                let _ = beats; // TODO: implement beat-based loop length
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
            DeckMessage::Seek(position) => {
                let _ = position; // TODO: implement seeking
            }
        }
    }

    /// Build the deck view
    pub fn view(&self) -> Element<DeckMessage> {
        let deck_label = text(format!("DECK {}", self.deck_idx + 1))
            .size(16);

        // Track info
        let track_info = if self.track_name.is_empty() {
            text("No track loaded").size(12)
        } else {
            text(format!("{} ({:.1} BPM)", self.track_name, self.track_bpm)).size(12)
        };

        // Playback state
        let state_text = match self.state {
            PlayState::Playing => "▶ Playing",
            PlayState::Stopped => "⏹ Stopped",
            PlayState::Cueing => "● Cueing",
        };
        let state_display = text(state_text).size(14);

        // Transport controls
        let transport = self.view_transport();

        // Hot cues
        let hot_cues = self.view_hot_cues();

        // Stem controls
        let stems = self.view_stems();

        // Pitch slider
        let pitch_section = self.view_pitch();

        let content = column![
            row![deck_label, Space::new().width(Fill), state_display].align_y(Center),
            track_info,
            transport,
            hot_cues,
            stems,
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
        let play_btn = button(text("▶").size(20))
            .on_press(DeckMessage::Play)
            .padding(10);

        let pause_btn = button(text("⏸").size(20))
            .on_press(DeckMessage::Pause)
            .padding(10);

        let cue_btn = button(text("CUE").size(14))
            .on_press(DeckMessage::CuePressed)
            .padding(10);

        let sync_btn = button(text("SYNC").size(14))
            .on_press(DeckMessage::Sync)
            .padding(10);

        let loop_text = if self.loop_active { "LOOP ●" } else { "LOOP" };
        let loop_btn = button(text(loop_text).size(14))
            .on_press(DeckMessage::ToggleLoop)
            .padding(10);

        row![play_btn, pause_btn, cue_btn, sync_btn, loop_btn]
            .spacing(5)
            .align_y(Center)
            .into()
    }

    /// Hot cue buttons view
    fn view_hot_cues(&self) -> Element<DeckMessage> {
        let buttons: Vec<Element<DeckMessage>> = (0..8)
            .map(|i| {
                button(text(format!("{}", i + 1)).size(12))
                    .on_press(DeckMessage::HotCue(i))
                    .padding(8)
                    .into()
            })
            .collect();

        Row::with_children(buttons)
            .spacing(3)
            .into()
    }

    /// Stem controls view
    fn view_stems(&self) -> Element<DeckMessage> {
        let stem_names = ["VOC", "DRM", "BAS", "OTH"];

        let stems: Vec<Element<DeckMessage>> = (0..4)
            .map(|i| {
                let mute_label = if self.stem_muted[i] { "M●" } else { "M" };
                let solo_label = if self.stem_soloed[i] { "S●" } else { "S" };

                column![
                    text(stem_names[i]).size(10),
                    row![
                        button(text(mute_label).size(10))
                            .on_press(DeckMessage::ToggleStemMute(i))
                            .padding(4),
                        button(text(solo_label).size(10))
                            .on_press(DeckMessage::ToggleStemSolo(i))
                            .padding(4),
                    ]
                    .spacing(2),
                    slider(0.0..=1.0, self.stem_volumes[i], move |v| DeckMessage::SetStemVolume(i, v))
                        .width(40),
                ]
                .spacing(2)
                .align_x(Center)
                .into()
            })
            .collect();

        row![
            text("STEMS").size(10),
            Row::with_children(stems).spacing(10),
        ]
        .spacing(10)
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
