//! Player controls component (vertical layout)
//!
//! CDJ-style player controls positioned to the left of waveforms:
//! - Loop toggle button
//! - Beat jump label (shows current loop/jump length)
//! - Beat jump buttons (◄◄ / ►►) side by side
//! - Cue button (CDJ-style: set + preview while held)
//! - Large Play/Pause toggle

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length};

/// Render vertical player controls (left of waveform)
pub fn view(state: &LoadedTrackState) -> Element<'_, Message> {
    let beat_jump_size = state.beat_jump_size();
    let is_playing = state.is_playing();
    let loop_active = state.is_loop_active();

    // Disable controls while loading
    let controls_enabled = !state.loading_audio && state.stems.is_some();

    // Loop toggle button (green when active)
    let loop_btn = button(text("LOOP").size(14))
        .on_press_maybe(controls_enabled.then_some(Message::ToggleLoop))
        .width(Length::Fixed(104.0))
        .height(Length::Fixed(32.0))
        .style(move |theme, status| {
            if loop_active {
                // Green style when loop is active
                button::Style {
                    background: Some(Background::Color(Color::from_rgb(0.2, 0.7, 0.3))),
                    text_color: Color::WHITE,
                    border: Border {
                        color: Color::from_rgb(0.1, 0.5, 0.2),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..button::primary(theme, status)
                }
            } else {
                button::secondary(theme, status)
            }
        });

    // Loop length controls: [-] [N beats] [+]
    let loop_length_beats = state.loop_length_beats();

    // Halve loop length button
    let halve_btn = button(text("−").size(14))
        .on_press_maybe(controls_enabled.then_some(Message::AdjustLoopLength(-1)))
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(24.0))
        .padding(0);

    // Loop length label
    let beat_label = text(format!("{}", loop_length_beats)).size(12);

    // Double loop length button
    let double_btn = button(text("+").size(14))
        .on_press_maybe(controls_enabled.then_some(Message::AdjustLoopLength(1)))
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(24.0))
        .padding(0);

    let loop_length_row = row![halve_btn, beat_label, double_btn]
        .spacing(4)
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

    // Vertical layout: loop → loop length controls → jump buttons → cue → play/pause
    // No center_y - align to top with parent row's align_y(Start)
    container(
        column![
            loop_btn,
            loop_length_row,
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
