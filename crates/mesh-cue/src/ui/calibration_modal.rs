//! Aggression calibration modal
//!
//! Two-phase modal: explanation screen → pairwise comparison cards.
//! Users compare pairs of tracks to teach the suggestion engine
//! what "more intense" means for their library.

use super::app::Message;
use super::state::calibration::{CalibrationSide, CalibrationState};
use iced::widget::{button, column, container, progress_bar, row, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length};
use mesh_widgets::sz;

/// Render the calibration modal content (either explanation or comparison).
pub fn view(state: &CalibrationState) -> Element<'_, Message> {
    if state.explanation_shown {
        view_explanation(state)
    } else {
        view_comparison(state)
    }
}

/// Explanation screen shown before calibration starts.
fn view_explanation(state: &CalibrationState) -> Element<'_, Message> {
    let title = text("Calibrate Smart Suggestions").size(sz(22.0));
    let close_btn = button(text("x").size(sz(18.0)))
        .on_press(Message::CloseCalibration)
        .style(button::secondary);

    let header = row![title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let (comm_count, track_count) = state.community_summary();
    let description = text(
        "New music detected that isn't covered by your current \
         aggression calibration. Comparing a few pairs of tracks \
         will teach the suggestion engine what \"more intense\" \
         means for these styles."
    ).size(sz(14.0));

    let stats = text(format!(
        "{} new communit{} found ({} tracks)",
        comm_count,
        if comm_count == 1 { "y" } else { "ies" },
        track_count,
    )).size(sz(14.0)).color(Color::from_rgb(0.6, 0.8, 0.6));

    let estimate = text(format!(
        "Estimated: ~{} comparisons (~{:.0} minutes)",
        state.total_pairs_planned,
        state.estimated_minutes().max(1.0),
    )).size(sz(13.0)).color(Color::from_rgb(0.5, 0.5, 0.5));

    let continue_btn = button(text("Continue").size(sz(14.0)))
        .on_press(Message::CalibrationStart)
        .style(button::primary)
        .padding([8, 24]);

    let not_now_btn = button(text("Not Now").size(sz(14.0)))
        .on_press(Message::CloseCalibration)
        .style(button::secondary)
        .padding([8, 24]);

    let actions = row![
        Space::new().width(Length::Fill),
        continue_btn,
        not_now_btn,
    ]
    .spacing(12)
    .width(Length::Fill);

    let body = column![header, description, stats, estimate, actions]
        .spacing(18)
        .width(Length::Fixed(520.0));

    container(body)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Comparison screen with two track cards side by side.
fn view_comparison(state: &CalibrationState) -> Element<'_, Message> {
    let title = text("Which track sounds more intense?").size(sz(18.0));
    let close_btn = button(text("x").size(sz(18.0)))
        .on_press(Message::CloseCalibration)
        .style(button::secondary);

    let back_btn = if !state.history.is_empty() {
        button(text("<").size(sz(16.0)))
            .on_press(Message::CalibrationBack)
            .style(button::secondary)
            .padding([4, 10])
    } else {
        button(text("<").size(sz(16.0)))
            .style(button::secondary)
            .padding([4, 10])
    };

    let header = row![back_btn, title, Space::new().width(Length::Fill), close_btn]
        .align_y(Alignment::Center)
        .spacing(10)
        .width(Length::Fill);

    // Phase + progress info
    let (phase_current, phase_total) = state.phase.progress();
    let phase_label = text(format!(
        "Phase {}: {} ({}/{})",
        state.phase.phase_number(),
        state.phase.label(),
        phase_current,
        phase_total,
    )).size(sz(12.0)).color(Color::from_rgb(0.5, 0.5, 0.5));

    let total_done = state.completed_count + state.total_historical;
    let total_planned = state.total_pairs_planned + state.total_historical;
    let total_label = text(format!(
        "Total: {}/{}",
        total_done, total_planned,
    )).size(sz(12.0)).color(Color::from_rgb(0.5, 0.5, 0.5));

    let progress_row = row![phase_label, Space::new().width(Length::Fill), total_label]
        .width(Length::Fill);

    let progress_pct = if total_planned > 0 {
        total_done as f32 / total_planned as f32
    } else {
        0.0
    };
    let bar = progress_bar(0.0..=1.0, progress_pct);

    // Track cards
    let cards: Element<'_, Message> = if let Some(ref pair) = state.current_pair {
        let card_a = view_track_card(
            &pair.track_a.title,
            &pair.track_a.artist,
            &pair.track_a.genre,
            pair.track_a.bpm,
            pair.track_a.key.as_deref(),
            CalibrationSide::Left,
            state.playing_side == Some(CalibrationSide::Left),
        );
        let card_b = view_track_card(
            &pair.track_b.title,
            &pair.track_b.artist,
            &pair.track_b.genre,
            pair.track_b.bpm,
            pair.track_b.key.as_deref(),
            CalibrationSide::Right,
            state.playing_side == Some(CalibrationSide::Right),
        );

        row![card_a, card_b]
            .spacing(16)
            .width(Length::Fill)
            .into()
    } else {
        text("Loading next pair...").size(sz(14.0))
            .color(Color::from_rgb(0.5, 0.5, 0.5))
            .into()
    };

    // Middle action buttons
    let equal_btn = button(text("About the same").size(sz(13.0)))
        .on_press(Message::CalibrationEqual)
        .style(button::secondary)
        .padding([6, 16]);

    let skip_btn = button(text("Skip").size(sz(13.0)))
        .on_press(Message::CalibrationSkip)
        .style(button::secondary)
        .padding([6, 16]);

    let middle_actions = row![
        Space::new().width(Length::Fill),
        equal_btn,
        skip_btn,
        Space::new().width(Length::Fill),
    ]
    .spacing(12);

    // Bottom actions
    let mut bottom = row![].spacing(12).width(Length::Fill);

    if state.can_finish_early() {
        let finish_btn = button(text("Finish Early").size(sz(13.0)))
            .on_press(Message::CalibrationFinish)
            .style(button::secondary)
            .padding([6, 16]);
        bottom = bottom.push(finish_btn);
    }

    bottom = bottom.push(Space::new().width(Length::Fill));

    let cancel_btn = button(text("Cancel").size(sz(13.0)))
        .on_press(Message::CloseCalibration)
        .style(button::secondary)
        .padding([6, 16]);
    bottom = bottom.push(cancel_btn);

    // Accuracy display (if we have enough data)
    let accuracy_row: Element<'_, Message> = if state.model_accuracy > 0.0 && total_done >= 10 {
        text(format!("Model accuracy: {:.0}%", state.model_accuracy * 100.0))
            .size(sz(11.0))
            .color(Color::from_rgb(0.5, 0.5, 0.5))
            .into()
    } else {
        Space::new().height(0).into()
    };

    let body = column![
        header,
        progress_row,
        bar,
        cards,
        middle_actions,
        accuracy_row,
        bottom,
    ]
    .spacing(14)
    .width(Length::Fixed(620.0));

    container(body)
        .padding(30)
        .style(container::rounded_box)
        .into()
}

/// Render a single track comparison card.
fn view_track_card(
    title: &str,
    artist: &str,
    genre: &str,
    bpm: Option<f64>,
    key: Option<&str>,
    side: CalibrationSide,
    is_playing: bool,
) -> Element<'static, Message> {
    let play_icon = if is_playing { "||" } else { ">" };
    let play_color = if is_playing {
        Color::from_rgb(0.3, 0.8, 0.3)
    } else {
        Color::from_rgb(0.6, 0.6, 0.6)
    };

    let play_label = text(play_icon).size(sz(20.0)).color(play_color);

    let title_text = text(title.to_string()).size(sz(14.0));
    let artist_text = text(artist.to_string()).size(sz(12.0))
        .color(Color::from_rgb(0.6, 0.6, 0.6));
    let genre_text = text(genre.to_string()).size(sz(11.0))
        .color(Color::from_rgb(0.5, 0.7, 0.9));

    let bpm_str = bpm.map(|b| format!("{:.0} BPM", b)).unwrap_or_default();
    let key_str = key.unwrap_or("").to_string();
    let detail_text = if !bpm_str.is_empty() && !key_str.is_empty() {
        text(format!("{}  {}", bpm_str, key_str))
    } else {
        text(format!("{}{}", bpm_str, key_str))
    }.size(sz(11.0)).color(Color::from_rgb(0.5, 0.5, 0.5));

    // Preview area: play icon on left, metadata on right
    let preview_btn = button(
        row![
            play_label,
            column![title_text, artist_text, genre_text, detail_text]
                .spacing(3)
                .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
    )
    .on_press(Message::CalibrationPreviewToggle(side))
    .width(Length::Fill)
    .padding([12, 14])
    .style(move |theme: &iced::Theme, status| {
        let palette = theme.extended_palette();
        let bg = match status {
            iced::widget::button::Status::Hovered =>
                palette.background.weak.color,
            _ => Color::TRANSPARENT,
        };
        iced::widget::button::Style {
            background: Some(Background::Color(bg)),
            border: Border::default(),
            text_color: palette.background.base.text,
            ..Default::default()
        }
    });

    let select_btn = button(
        text("This one").size(sz(13.0))
    )
    .on_press(Message::CalibrationChoice(side))
    .style(button::primary)
    .padding([6, 16])
    .width(Length::Fill);

    // Wrap everything in a bordered container
    let card_content = column![preview_btn, select_btn]
        .spacing(6)
        .padding(4)
        .width(Length::Fill);

    container(card_content)
        .width(Length::FillPortion(1))
        .clip(true)
        .style(move |theme: &iced::Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(Background::Color(palette.background.strong.color)),
                border: Border {
                    color: if is_playing {
                        Color::from_rgb(0.3, 0.7, 0.3)
                    } else {
                        palette.background.weak.color
                    },
                    width: if is_playing { 2.0 } else { 1.0 },
                    radius: 8.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
}
