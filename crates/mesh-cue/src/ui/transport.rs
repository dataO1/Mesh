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
use mesh_widgets::{sz, COMBINED_WAVEFORM_GAP, WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT};

/// Render vertical player controls (left of waveform)
pub fn view(state: &LoadedTrackState, vocal_color: Color) -> Element<'_, Message> {
    let beat_jump_size = state.beat_jump_size();
    let is_playing = state.is_playing();
    let loop_active = state.is_loop_active();

    // Disable controls while loading
    let controls_enabled = !state.loading_audio && state.stems.is_some();

    // Loop toggle button (vocal stem color when active)
    // Uses Fill height to absorb remaining space so transport matches waveform height
    let loop_btn = button(text("LOOP").size(sz(14.0)))
        .on_press_maybe(controls_enabled.then_some(Message::ToggleLoop))
        .width(Length::Fixed(104.0))
        .height(Length::Fill)
        .style(move |theme, status| {
            if loop_active {
                let base = button::primary(theme, status);
                button::Style {
                    background: Some(Background::Color(vocal_color)),
                    text_color: Color::BLACK,
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: base.border.radius,
                    },
                    ..base
                }
            } else {
                button::secondary(theme, status)
            }
        });

    // Loop length controls: [-] [N beats] [+]
    let loop_length_beats = state.loop_length_beats();

    // Halve loop length button — flush left
    let halve_btn = button(text("−").size(sz(14.0)).center())
        .on_press_maybe(controls_enabled.then_some(Message::AdjustLoopLength(-1)))
        .width(Length::Fill)
        .height(Length::Fixed(24.0))
        .padding(0);

    // Loop length label (use fraction notation for sub-beat values)
    let beat_text: String = if (loop_length_beats - 0.125).abs() < 0.001 { "1/8".into() }
        else if (loop_length_beats - 0.25).abs() < 0.001 { "1/4".into() }
        else if (loop_length_beats - 0.5).abs() < 0.001 { "1/2".into() }
        else { format!("{:.0}", loop_length_beats) };
    let beat_label = container(text(beat_text).size(sz(12.0)).center())
        .width(Length::Fixed(32.0))
        .center_x(Length::Fixed(32.0));

    // Double loop length button — flush right
    let double_btn = button(text("+").size(sz(14.0)).center())
        .on_press_maybe(controls_enabled.then_some(Message::AdjustLoopLength(1)))
        .width(Length::Fill)
        .height(Length::Fixed(24.0))
        .padding(0);

    // -/+ buttons fill to the edges, number has fixed width in center
    let loop_length_row = row![halve_btn, beat_label, double_btn]
        .width(Length::Fixed(104.0))
        .align_y(Alignment::Center);

    // Beat jump buttons (side by side)
    let jump_back = button(text("◄◄").size(sz(14.0)))
        .on_press_maybe(controls_enabled.then_some(Message::BeatJump(-beat_jump_size)))
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(36.0));

    let jump_forward = button(text("►►").size(sz(14.0)))
        .on_press_maybe(controls_enabled.then_some(Message::BeatJump(beat_jump_size)))
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(36.0));

    let jump_buttons = row![jump_back, jump_forward]
        .spacing(4)
        .align_y(Alignment::Center);

    // CDJ-style cue button
    // Press only works when stopped, but release always works to stop preview
    let cue_btn = button(text("[Cue]").size(sz(18.0)))
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
            button(text("▮▮").size(sz(24.0)))
                .on_press(Message::Pause)
                .width(Length::Fixed(104.0))
                .height(Length::Fixed(60.0))
        } else {
            button(text("▶").size(sz(28.0)))
                .on_press(Message::Play)
                .width(Length::Fixed(104.0))
                .height(Length::Fixed(60.0))
        }
    } else {
        button(text("▶").size(sz(28.0)))
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
    .padding(iced::Padding::from([0, 8]))  // No vertical padding — flush with waveform
    .width(Length::Fixed(120.0))
    .height(Length::Fixed(ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP + WAVEFORM_HEIGHT))
    .into()
}
