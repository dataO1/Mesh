//! Deck header as regular iced widgets (replaces canvas-drawn headers)
//!
//! Renders the deck header bar with: deck badge, linked stem diamonds,
//! track name, BPM, loop indicator, LUFS gain, and key+transpose display.
//! Uses regular iced text/container/row widgets for crisp text rendering
//! and zero GPU overhead.

use iced::widget::{container, row, text, Space};
use iced::{Color, Element, Length};

use super::super::state::{PlayerCanvasState, DECK_HEADER_HEIGHT};

/// Header background color
const HEADER_BG: Color = Color::from_rgb(0.10, 0.10, 0.12);

/// Badge dimensions (full header height for no dead space)
const BADGE_WIDTH: f32 = 48.0;
const BADGE_HEIGHT: f32 = DECK_HEADER_HEIGHT;

/// Create the deck header widget for one deck.
///
/// Layout: `[ badge | linked_diamonds | track_name | <spacer> | bpm | loop | lufs | key ]`
///
/// All data is read from `PlayerCanvasState` getters. The header is purely
/// informational — no interactive elements.
pub fn view_deck_header<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    deck_idx: usize,
) -> Element<'a, Message> {
    let deck = state.deck(deck_idx);
    let has_track = deck.overview.has_track;
    let track_name = state.track_name(deck_idx);
    let track_key = state.track_key(deck_idx);
    let track_bpm = state.track_bpm(deck_idx);
    let is_master = state.is_master(deck_idx);
    let cue_enabled = state.cue_enabled(deck_idx);
    let transpose = state.transpose(deck_idx);
    let key_match_enabled = state.key_match_enabled(deck_idx);
    let lufs_gain_db = state.lufs_gain_db(deck_idx);
    let loop_length_beats = state.loop_length_beats(deck_idx);
    let loop_active = state.loop_active(deck_idx);
    // -- Deck badge --
    let badge = view_badge(deck_idx, is_master, cue_enabled, has_track);

    // -- Track name --
    let name_widget: Element<'a, Message> = if has_track && !track_name.is_empty() {
        text(track_name)
            .size(24)
            .color(Color::from_rgb(0.75, 0.75, 0.75))
            .into()
    } else {
        text("No track")
            .size(22)
            .color(Color::from_rgb(0.4, 0.4, 0.4))
            .into()
    };

    // -- Right-side indicators (built right-to-left, displayed left-to-right) --
    let mut right_items: Vec<Element<'a, Message>> = Vec::new();

    // BPM
    if has_track {
        if let Some(bpm) = track_bpm {
            right_items.push(
                text(format!("{:.1}", bpm))
                    .size(20)
                    .color(Color::from_rgb(0.7, 0.7, 0.8))
                    .into(),
            );
        }
    }

    // Loop indicator
    if has_track {
        if let Some(beats) = loop_length_beats {
            let loop_text = if beats < 1.0 {
                format!("\u{21BB}1/{:.0}", 1.0 / beats)
            } else {
                format!("\u{21BB}{:.0}", beats)
            };
            let loop_color = if loop_active {
                Color::from_rgb(0.4, 0.9, 0.4)
            } else {
                Color::from_rgb(0.5, 0.5, 0.5)
            };
            right_items.push(text(loop_text).size(20).color(loop_color).into());
        }
    }

    // LUFS gain
    if has_track {
        if let Some(gain_db) = lufs_gain_db {
            let gain_text = if gain_db >= 0.0 {
                format!("+{:.1}dB", gain_db)
            } else {
                format!("{:.1}dB", gain_db)
            };
            let gain_color = if gain_db.abs() < 0.5 {
                Color::from_rgb(0.5, 0.5, 0.5)
            } else if gain_db > 0.0 {
                Color::from_rgb(0.5, 0.8, 0.9) // Cyan for boost
            } else {
                Color::from_rgb(0.9, 0.7, 0.5) // Orange for cut
            };
            right_items.push(text(gain_text).size(20).color(gain_color).into());
        }
    }

    // Key + transpose
    if has_track && !track_key.is_empty() {
        let (key_display, key_color) = if is_master || !key_match_enabled {
            (track_key.to_string(), Color::from_rgb(0.6, 0.8, 0.6))
        } else if transpose == 0 {
            (
                format!("{} \u{2713}", track_key),
                Color::from_rgb(0.5, 0.9, 0.5),
            )
        } else {
            let sign = if transpose > 0 { "+" } else { "" };
            (
                format!("{} \u{2192} {}{}", track_key, sign, transpose),
                Color::from_rgb(0.9, 0.7, 0.5),
            )
        };
        right_items.push(text(key_display).size(22).color(key_color).into());
    }

    // Compose the header row
    let right_section: Element<'a, Message> = row(right_items).spacing(18).into();

    let header_content = row![
        badge,
        Space::new().width(6),
        name_widget,
        Space::new().width(Length::Fill),
        right_section,
        Space::new().width(8),
    ]
    .align_y(iced::Alignment::Center);

    container(header_content)
        .width(Length::Fill)
        .height(Length::Fixed(DECK_HEADER_HEIGHT))
        .style(|_theme: &iced::Theme| container::Style {
            background: Some(HEADER_BG.into()),
            ..Default::default()
        })
        .padding([0, 6])
        .into()
}

/// Build the deck number badge with master/cue styling
fn view_badge<'a, Message: Clone + 'a>(
    deck_idx: usize,
    is_master: bool,
    cue_enabled: bool,
    has_track: bool,
) -> Element<'a, Message> {
    let deck_num = format!("{}", deck_idx + 1);

    // Badge background color
    let badge_bg = if cue_enabled {
        Color::from_rgb(0.35, 0.30, 0.10) // Dark amber for cue
    } else if has_track {
        Color::from_rgb(0.15, 0.15, 0.25) // Dark blue for loaded
    } else {
        Color::from_rgb(0.15, 0.15, 0.15) // Dark gray for empty
    };

    // Badge text color
    let text_color = if cue_enabled {
        Color::from_rgb(1.0, 0.85, 0.3) // Bright amber
    } else if has_track {
        Color::from_rgb(0.7, 0.7, 0.9) // Light blue
    } else {
        Color::from_rgb(0.5, 0.5, 0.5) // Gray
    };

    // Master border color (sage green)
    let border_color = if is_master {
        Color::from_rgb(0.45, 0.8, 0.55)
    } else {
        Color::TRANSPARENT
    };

    let badge_text = text(deck_num)
        .size(26)
        .color(text_color)
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center);

    container(badge_text)
        .width(Length::Fixed(BADGE_WIDTH))
        .height(Length::Fixed(BADGE_HEIGHT))
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center)
        .style(move |_theme: &iced::Theme| container::Style {
            background: Some(badge_bg.into()),
            border: iced::Border {
                color: border_color,
                width: if is_master { 2.0 } else { 0.0 },
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}
