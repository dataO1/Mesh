//! Mixer message handler
//!
//! Handles volume, EQ, filter, and cue controls for all deck channels.

use iced::Task;

use crate::ui::app::MeshApp;
use crate::ui::message::Message;
use crate::ui::mixer_view::MixerMessage;

/// Handle mixer messages (volume, EQ, filter, cue)
pub fn handle(app: &mut MeshApp, mixer_msg: MixerMessage) -> Task<Message> {
    use MixerMessage::*;

    // Send mixer commands to audio engine via domain
    match &mixer_msg {
        SetChannelVolume(deck, volume) => {
            app.domain.set_volume(*deck, *volume);
        }
        ToggleChannelCue(deck) => {
            // Read current state and toggle
            let enabled = !app.mixer_view.cue_enabled(*deck);
            app.domain.set_cue_listen(*deck, enabled);
        }
        SetChannelEqHi(deck, value) => {
            app.domain.set_eq_hi(*deck, *value);
        }
        SetChannelEqMid(deck, value) => {
            app.domain.set_eq_mid(*deck, *value);
        }
        SetChannelEqLo(deck, value) => {
            app.domain.set_eq_lo(*deck, *value);
        }
        SetChannelFilter(deck, value) => {
            app.domain.set_filter(*deck, *value);
        }
        SetMasterVolume(volume) => {
            app.domain.set_master_volume(*volume);
        }
        SetCueMix(mix) => {
            app.domain.set_cue_mix(*mix);
        }
        SetCueVolume(volume) => {
            app.domain.set_cue_volume(*volume);
        }
    }

    // Always update local UI state
    app.mixer_view.handle_local_message(mixer_msg);
    Task::none()
}
