//! Mixer view component
//!
//! Displays the 4-channel mixer with:
//! - Per-channel volume faders
//! - Per-channel EQ (hi/mid/lo)
//! - Per-channel filter
//! - Master/cue volume
//! - Cue select buttons
//!
//! Note: No crossfader - Mesh uses stem-based mixing with global BPM sync

use iced::widget::{button, column, container, row, slider, text, Row};
use iced::{Center, Element};

use mesh_core::engine::Mixer;

/// State for the mixer view
pub struct MixerView {
    /// Channel volumes (0-1)
    channel_volumes: [f32; 4],
    /// Channel filter positions (-1 to 1, 0 = flat)
    channel_filters: [f32; 4],
    /// Channel EQ hi (0-1)
    channel_eq_hi: [f32; 4],
    /// Channel EQ mid (0-1)
    channel_eq_mid: [f32; 4],
    /// Channel EQ lo (0-1)
    channel_eq_lo: [f32; 4],
    /// Cue enabled per channel
    channel_cue: [bool; 4],
    /// Master volume
    master_volume: f32,
    /// Cue/headphone volume
    cue_volume: f32,
    /// Cue/master mix for headphones
    cue_mix: f32,
}

/// Messages for mixer interaction
#[derive(Debug, Clone)]
pub enum MixerMessage {
    /// Set channel volume
    SetChannelVolume(usize, f32),
    /// Set channel filter
    SetChannelFilter(usize, f32),
    /// Set channel EQ hi
    SetChannelEqHi(usize, f32),
    /// Set channel EQ mid
    SetChannelEqMid(usize, f32),
    /// Set channel EQ lo
    SetChannelEqLo(usize, f32),
    /// Toggle channel cue
    ToggleChannelCue(usize),
    /// Set master volume
    SetMasterVolume(f32),
    /// Set cue volume
    SetCueVolume(f32),
    /// Set cue/master mix
    SetCueMix(f32),
}

impl MixerView {
    /// Create a new mixer view
    pub fn new() -> Self {
        Self {
            channel_volumes: [0.75; 4],
            channel_filters: [0.0; 4],
            channel_eq_hi: [0.5; 4],
            channel_eq_mid: [0.5; 4],
            channel_eq_lo: [0.5; 4],
            channel_cue: [false; 4],
            master_volume: 0.8,
            cue_volume: 0.8,
            cue_mix: 0.5,
        }
    }

    /// Sync view state from mixer
    pub fn sync_from_mixer(&mut self, mixer: &Mixer) {
        for i in 0..4 {
            if let Some(ch) = mixer.channel(i) {
                self.channel_volumes[i] = ch.volume;
                self.channel_filters[i] = ch.filter;
                self.channel_cue[i] = ch.cue_enabled;
            }
        }
        self.master_volume = mixer.master_volume();
        // cue_volume not separate in mixer yet - use cue_mix
        self.cue_mix = mixer.cue_mix();
    }

    /// Handle a mixer message (legacy method for direct mixer access)
    #[allow(dead_code)]
    pub fn handle_message(&mut self, msg: MixerMessage, mixer: &mut Mixer) {
        match msg {
            MixerMessage::SetChannelVolume(ch, vol) => {
                self.channel_volumes[ch] = vol;
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.volume = vol;
                }
            }
            MixerMessage::SetChannelFilter(ch, pos) => {
                self.channel_filters[ch] = pos;
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.filter = pos;
                }
            }
            MixerMessage::SetChannelEqHi(ch, val) => {
                self.channel_eq_hi[ch] = val;
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.set_eq_hi(val);
                }
            }
            MixerMessage::SetChannelEqMid(ch, val) => {
                self.channel_eq_mid[ch] = val;
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.set_eq_mid(val);
                }
            }
            MixerMessage::SetChannelEqLo(ch, val) => {
                self.channel_eq_lo[ch] = val;
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.set_eq_lo(val);
                }
            }
            MixerMessage::ToggleChannelCue(ch) => {
                self.channel_cue[ch] = !self.channel_cue[ch];
                if let Some(channel) = mixer.channel_mut(ch) {
                    channel.cue_enabled = self.channel_cue[ch];
                }
            }
            MixerMessage::SetMasterVolume(vol) => {
                self.master_volume = vol;
                mixer.set_master_volume(vol);
            }
            MixerMessage::SetCueVolume(vol) => {
                self.cue_volume = vol;
                // Separate cue volume not in mixer - it uses cue_mix
            }
            MixerMessage::SetCueMix(mix) => {
                self.cue_mix = mix;
                mixer.set_cue_mix(mix);
            }
        }
    }

    /// Handle messages that only affect local UI state (no engine commands needed)
    pub fn handle_local_message(&mut self, msg: MixerMessage) {
        match msg {
            MixerMessage::SetChannelVolume(ch, vol) => {
                self.channel_volumes[ch] = vol;
            }
            MixerMessage::SetChannelFilter(ch, pos) => {
                self.channel_filters[ch] = pos;
            }
            MixerMessage::SetChannelEqHi(ch, val) => {
                self.channel_eq_hi[ch] = val;
            }
            MixerMessage::SetChannelEqMid(ch, val) => {
                self.channel_eq_mid[ch] = val;
            }
            MixerMessage::SetChannelEqLo(ch, val) => {
                self.channel_eq_lo[ch] = val;
            }
            MixerMessage::ToggleChannelCue(ch) => {
                self.channel_cue[ch] = !self.channel_cue[ch];
            }
            MixerMessage::SetMasterVolume(vol) => {
                self.master_volume = vol;
            }
            MixerMessage::SetCueVolume(vol) => {
                self.cue_volume = vol;
            }
            MixerMessage::SetCueMix(mix) => {
                self.cue_mix = mix;
            }
        }
    }

    /// Get cue enabled state for a channel
    pub fn cue_enabled(&self, ch: usize) -> bool {
        self.channel_cue.get(ch).copied().unwrap_or(false)
    }

    /// Set cue enabled state for a channel (local UI state only)
    pub fn set_cue_enabled(&mut self, ch: usize, enabled: bool) {
        if ch < 4 {
            self.channel_cue[ch] = enabled;
        }
    }

    /// Build the mixer view
    ///
    /// Compact 2-section layout (for side panel next to collection browser):
    /// ```text
    /// ┌────────────────────────────────┬─────────────────┐
    /// │ CH1   CH2   CH3   CH4          │ MASTER    CUE   │
    /// │ (4 channel strips)             │ MIX             │
    /// │ ~75%                           │ ~25%            │
    /// └────────────────────────────────┴─────────────────┘
    /// ```
    pub fn view(&self) -> Element<MixerMessage> {
        use iced::Length;

        // Channel strips column (~75%)
        let channels: Vec<Element<MixerMessage>> = (0..4)
            .map(|i| self.view_channel(i))
            .collect();

        let channels_section = container(
            Row::with_children(channels)
                .spacing(12)
                .align_y(Center)
        )
        .width(Length::FillPortion(75));

        // Master/Cue section (~25%)
        let master = column![
            text("MASTER").size(10),
            slider(0.0..=1.0, self.master_volume, MixerMessage::SetMasterVolume)
                .step(0.01)
                .width(70),
        ]
        .spacing(4)
        .align_x(Center);

        let cue = column![
            text("CUE").size(10),
            slider(0.0..=1.0, self.cue_volume, MixerMessage::SetCueVolume)
                .step(0.01)
                .width(70),
            text("MIX").size(10),
            slider(0.0..=1.0, self.cue_mix, MixerMessage::SetCueMix)
                .step(0.01)
                .width(70),
        ]
        .spacing(4)
        .align_x(Center);

        let master_cue_section = container(
            row![master, cue]
                .spacing(12)
                .align_y(Center)
        )
        .width(Length::FillPortion(25));

        let content = row![
            channels_section,
            master_cue_section,
        ]
        .align_y(Center);

        container(content)
            .padding(8)
            .width(Length::Fill)
            .into()
    }

    /// View for a single channel strip
    fn view_channel(&self, ch: usize) -> Element<MixerMessage> {
        use iced::Length;

        let ch_label = text(format!("CH {}", ch + 1)).size(11);

        // EQ sliders (fill available width)
        let eq_hi = column![
            text("HI").size(9),
            slider(0.0..=1.0, self.channel_eq_hi[ch], move |v| MixerMessage::SetChannelEqHi(ch, v))
                .step(0.01)
                .width(Length::Fill),
        ]
        .spacing(2)
        .align_x(Center)
        .width(Length::Fill);

        let eq_mid = column![
            text("MID").size(9),
            slider(0.0..=1.0, self.channel_eq_mid[ch], move |v| MixerMessage::SetChannelEqMid(ch, v))
                .step(0.01)
                .width(Length::Fill),
        ]
        .spacing(2)
        .align_x(Center)
        .width(Length::Fill);

        let eq_lo = column![
            text("LO").size(9),
            slider(0.0..=1.0, self.channel_eq_lo[ch], move |v| MixerMessage::SetChannelEqLo(ch, v))
                .step(0.01)
                .width(Length::Fill),
        ]
        .spacing(2)
        .align_x(Center)
        .width(Length::Fill);

        // Filter
        let filter = column![
            text("FILTER").size(9),
            slider(-1.0..=1.0, self.channel_filters[ch], move |v| MixerMessage::SetChannelFilter(ch, v))
                .step(0.01)
                .width(Length::Fill),
        ]
        .spacing(2)
        .align_x(Center)
        .width(Length::Fill);

        // Volume fader
        let volume = column![
            text("VOL").size(9),
            slider(0.0..=1.0, self.channel_volumes[ch], move |v| MixerMessage::SetChannelVolume(ch, v))
                .step(0.01)
                .width(Length::Fill),
        ]
        .spacing(2)
        .align_x(Center)
        .width(Length::Fill);

        // Cue button
        let cue_label = if self.channel_cue[ch] { "CUE ●" } else { "CUE" };
        let cue_btn = button(text(cue_label).size(10))
            .on_press(MixerMessage::ToggleChannelCue(ch))
            .padding([4, 8])
            .width(Length::Fill);

        column![
            ch_label,
            eq_hi,
            eq_mid,
            eq_lo,
            filter,
            volume,
            cue_btn,
        ]
        .spacing(4)
        .align_x(Center)
        .width(Length::Fill)
        .into()
    }
}

impl Default for MixerView {
    fn default() -> Self {
        Self::new()
    }
}
