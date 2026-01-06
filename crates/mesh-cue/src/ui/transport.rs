//! Player controls component (vertical layout)
//!
//! CDJ-style player controls positioned to the left of waveforms:
//! - Beat jump label (shows current loop/jump length)
//! - Beat jump buttons (◄◄ / ►►) side by side
//! - Cue button (CDJ-style: set + preview while held)
//! - Large Play/Pause toggle

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Alignment, Element, Length};

/// Render vertical player controls (left of waveform)
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    let beat_jump_size = state.beat_jump_size();
    let is_playing = state.is_playing();

    // Disable controls while loading
    let controls_enabled = !state.loading_audio && state.stems.is_some();

    // Beat jump size label (controlled by Up/Down keys to adjust loop length)
    let loop_length_beats = state.loop_length_beats();
    let beat_jump_label = container(
        text(format!("{} beat{}", loop_length_beats, if loop_length_beats == 1.0 { "" } else { "s" }))
            .size(12)
    )
    .width(Length::Fixed(104.0))
    .center_x(Length::Fill);

    // Beat jump buttons (side by side)
    let jump_back = button(text("◄◄").size(14))
        .on_press_maybe(controls_enabled.then_some(Message::BeatJump(-beat_jump_size)))
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(36.0));

    let jump_forward = button(text("►►").size(14))
        .on_press_maybe(controls_enabled.then_some(Message::BeatJump(beat_jump_size)))
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(36.0));

    let jump_buttons = row![jump_back, jump_forward]
        .spacing(4)
        .align_y(Alignment::Center);

    // CDJ-style cue button
    // Press only works when stopped, but release always works to stop preview
    let cue_btn = button(text("[Cue]").size(18))
        .width(Length::Fixed(104.0))
        .height(Length::Fixed(60.0));  // Match Play button height

    let cue: Element<Message> = if controls_enabled {
        let mut area = mouse_area(cue_btn).on_release(Message::CueReleased);
        // Only allow press when stopped (not playing)
        if !is_playing {
            area = area.on_press(Message::Cue);
        }
        area.into()
    } else {
        cue_btn.into()
    };

    // Large Play/Pause toggle button
    let play_pause = if controls_enabled {
        if is_playing {
            button(text("▮▮").size(24))
                .on_press(Message::Pause)
                .width(Length::Fixed(104.0))
                .height(Length::Fixed(60.0))
        } else {
            button(text("▶").size(28))
                .on_press(Message::Play)
                .width(Length::Fixed(104.0))
                .height(Length::Fixed(60.0))
        }
    } else {
        button(text("▶").size(28))
            .width(Length::Fixed(104.0))
            .height(Length::Fixed(60.0))
    };

    // Vertical layout: beat jump label → jump buttons → cue → play/pause
    // No center_y - align to top with parent row's align_y(Start)
    container(
        column![
            beat_jump_label,
            jump_buttons,
            cue,
            play_pause,
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fixed(120.0))
    .into()
}
