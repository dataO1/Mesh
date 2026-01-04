//! Player controls component (vertical layout)
//!
//! CDJ-style player controls positioned to the left of waveforms:
//! - Beat jump size selector (1, 4, 8, 16, 32)
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

    // Beat jump size selector (row of buttons at top)
    let jump_sizes = [1, 4, 8, 16, 32];
    let jump_size_buttons: Vec<Element<Message>> = jump_sizes
        .iter()
        .map(|&size| {
            let is_selected = beat_jump_size == size;
            let btn = button(text(format!("{}", size)).size(11))
                .on_press_maybe(controls_enabled.then_some(Message::SetBeatJumpSize(size)))
                .style(if is_selected {
                    iced::widget::button::primary
                } else {
                    iced::widget::button::secondary
                })
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(24.0));
            btn.into()
        })
        .collect();

    let beat_jump_selector = row(jump_size_buttons)
        .spacing(2)
        .align_y(Alignment::Center);

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
    let cue_btn = button(text("[Cue]").size(14))
        .width(Length::Fixed(104.0))
        .height(Length::Fixed(36.0));

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

    // Vertical layout: beat jump selector → jump buttons → cue → play/pause
    container(
        column![
            beat_jump_selector,
            jump_buttons,
            cue,
            play_pause,
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding(8)
    .width(Length::Fixed(120.0))
    .height(Length::Fill)
    .center_y(Length::Fill)
    .into()
}
