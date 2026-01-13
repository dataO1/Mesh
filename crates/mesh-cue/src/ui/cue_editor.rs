//! Hot cue buttons component
//!
//! CDJ-style 8 hot cue buttons in a single row:
//! - Click on set cue → Jump to that cue position
//! - Click on empty slot → Set cue at current playhead position
//! - Shift+Click on set cue → Clear/delete that cue point
//!
//! Plus a DROP button for setting the drop marker (used for linked stem alignment).

use super::app::{LoadedTrackState, Message};
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Alignment, Color, Element, Length, Theme};
use mesh_core::types::SAMPLE_RATE;
use mesh_widgets::CUE_COLORS;
use mesh_widgets::{COMBINED_WAVEFORM_GAP, WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT};

/// Drop marker color (orange - same as used in waveform visualization)
const DROP_MARKER_COLOR: Color = Color::from_rgb(1.0, 0.5, 0.0);

/// Render the hot cue buttons (single row of 8 action buttons + DROP button)
pub fn view(state: &LoadedTrackState) -> Element<Message> {
    // Create all 8 hot cue buttons
    let mut buttons: Vec<Element<Message>> = (0..8)
        .map(|i| {
            let cue = state.cue_points.iter().find(|c| c.index == i as u8);
            create_hot_cue_button(i, cue)
        })
        .collect();

    // Add DROP button at the end
    buttons.push(create_drop_marker_button(state.drop_marker));

    let hot_cue_row = row(buttons).spacing(8).align_y(Alignment::Center);

    // No vertical padding - cue buttons should be directly under waveforms
    container(hot_cue_row)
        .padding([0, 10])  // [vertical, horizontal] - no top/bottom padding
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// Create the DROP marker button
fn create_drop_marker_button(drop_marker: Option<u64>) -> Element<'static, Message> {
    let label_text = if let Some(position) = drop_marker {
        let time = format_time_short(position);
        format!("DROP\n{}", time)
    } else {
        "DROP".to_string()
    };

    let btn = button(text(label_text).size(11).center())
        .width(Length::Fixed(60.0)) // Fixed width for DROP button
        .height(Length::Fixed(44.0));

    if drop_marker.is_some() {
        // Drop marker is set - orange button, shift+click to clear
        btn.on_press(Message::SetDropMarker) // Click updates position
            .style(move |theme: &Theme, status| {
                colored_button_style(theme, status, DROP_MARKER_COLOR)
            })
            .into()
    } else {
        // No drop marker - secondary button, click to set
        btn.on_press(Message::SetDropMarker)
            .style(iced::widget::button::secondary)
            .into()
    }
}

/// Create a single hot cue button
fn create_hot_cue_button(
    index: usize,
    cue: Option<&mesh_core::audio_file::CuePoint>,
) -> Element<'static, Message> {
    let label_text = if let Some(cue) = cue {
        // Show cue number and time
        let time = format_time_short(cue.sample_position);
        format!("{}\n{}", index + 1, time)
    } else {
        // Empty slot
        format!("{}", index + 1)
    };

    let btn = button(text(label_text).size(11).center())
        .width(Length::Fill)  // Dynamic width to match waveform
        .height(Length::Fixed(44.0));

    // If cue exists, use CDJ-style preview (hold to play, release to return)
    // Otherwise, click sets a new cue point
    if cue.is_some() {
        // Wrap in mouse_area for press/release detection (CDJ-style preview)
        let styled_btn = btn.style(move |theme: &Theme, status| {
            let color = CUE_COLORS[index];
            colored_button_style(theme, status, color)
        });

        mouse_area(styled_btn)
            .on_press(Message::HotCuePressed(index))
            .on_release(Message::HotCueReleased(index))
            .into()
    } else {
        // Empty slot - just set cue on click
        btn.on_press(Message::SetCuePoint(index))
            .style(iced::widget::button::secondary)
            .into()
    }
}

/// Create a colored button style
fn colored_button_style(
    _theme: &Theme,
    status: iced::widget::button::Status,
    color: Color,
) -> iced::widget::button::Style {
    let (bg_color, text_color) = match status {
        iced::widget::button::Status::Active => (color, Color::BLACK),
        iced::widget::button::Status::Hovered => {
            // Lighten on hover
            (
                Color::from_rgb(
                    (color.r + 0.2).min(1.0),
                    (color.g + 0.2).min(1.0),
                    (color.b + 0.2).min(1.0),
                ),
                Color::BLACK,
            )
        }
        iced::widget::button::Status::Pressed => {
            // Darken on press
            (
                Color::from_rgb(color.r * 0.8, color.g * 0.8, color.b * 0.8),
                Color::WHITE,
            )
        }
        iced::widget::button::Status::Disabled => (Color::from_rgb(0.3, 0.3, 0.3), Color::WHITE),
    };

    iced::widget::button::Style {
        background: Some(iced::Background::Color(bg_color)),
        text_color,
        border: iced::Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 4.0.into(),
        },
        shadow: iced::Shadow::default(),
        snap: false,
    }
}

/// Format sample position as short time string (S.ms)
fn format_time_short(samples: u64) -> String {
    let seconds = samples as f64 / SAMPLE_RATE as f64;
    if seconds < 60.0 {
        format!("{:.1}s", seconds)
    } else {
        let minutes = (seconds / 60.0).floor() as u64;
        let secs = (seconds % 60.0).floor() as u64;
        format!("{}:{:02}", minutes, secs)
    }
}

// ────────────────────────────────────────────────────────────────────────────────
// Stem Link Buttons (for prepared mode - links stored in mslk chunk)
// ────────────────────────────────────────────────────────────────────────────────

/// Stem names for display
const STEM_NAMES: [&str; 4] = ["VOC", "DRM", "BAS", "OTH"];

/// Stem colors (matching mesh_widgets::STEM_COLORS)
const STEM_COLORS: [Color; 4] = [
    Color::from_rgb(0.0, 0.8, 0.4),  // Vocals - green
    Color::from_rgb(0.8, 0.2, 0.2),  // Drums - red
    Color::from_rgb(0.2, 0.4, 0.9),  // Bass - blue
    Color::from_rgb(0.9, 0.7, 0.1),  // Other - yellow
];

/// Render the stem link buttons (4 buttons, one per stem)
///
/// Each button shows the stem name and the linked track (if any).
/// Click to start link selection, Shift+click to clear.
pub fn view_stem_links(
    state: &LoadedTrackState,
    stem_link_selection: Option<usize>,
) -> Element<Message> {
    use mesh_core::audio_file::StemLinkReference;

    let buttons: Vec<Element<Message>> = (0..4)
        .map(|stem_idx| {
            // Find if this stem has a link
            let link: Option<&StemLinkReference> = state
                .stem_links
                .iter()
                .find(|l| l.stem_index == stem_idx as u8);

            create_stem_link_button(stem_idx, link, stem_link_selection)
        })
        .collect();

    let row = row(buttons).spacing(8).align_y(Alignment::Center);

    container(row)
        .padding([4, 10])
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// Render the stem link buttons as a vertical column (for right of waveforms)
///
/// Each button shows the stem name and the linked track (if any).
/// Click to start link selection, Shift+click to clear.
/// The column height matches the combined waveform height.
pub fn view_stem_links_column(
    state: &LoadedTrackState,
    stem_link_selection: Option<usize>,
) -> Element<Message> {
    use mesh_core::audio_file::StemLinkReference;

    // Total waveform height
    let total_height = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP + WAVEFORM_HEIGHT;
    let button_height = total_height / 4.0;

    let buttons: Vec<Element<Message>> = (0..4)
        .map(|stem_idx| {
            // Find if this stem has a link
            let link: Option<&StemLinkReference> = state
                .stem_links
                .iter()
                .find(|l| l.stem_index == stem_idx as u8);

            create_stem_link_button_vertical(stem_idx, link, stem_link_selection, button_height)
        })
        .collect();

    column(buttons)
        .spacing(0)
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(total_height))
        .into()
}

/// Create a single stem link button for vertical column layout
fn create_stem_link_button_vertical(
    stem_idx: usize,
    link: Option<&mesh_core::audio_file::StemLinkReference>,
    stem_link_selection: Option<usize>,
    height: f32,
) -> Element<'static, Message> {
    let stem_name = STEM_NAMES[stem_idx];
    let color = STEM_COLORS[stem_idx];
    let is_selecting = stem_link_selection == Some(stem_idx);

    // Shorter labels for vertical layout
    let label_text = if link.is_some() {
        stem_name.to_string()
    } else if is_selecting {
        format!("{}...", stem_name)
    } else {
        stem_name.to_string()
    };

    let btn = button(text(label_text).size(10).center())
        .width(Length::Fill)
        .height(Length::Fixed(height));

    if link.is_some() {
        // Has link - show in stem color
        btn.on_press(Message::StartStemLinkSelection(stem_idx))
            .style(move |theme: &Theme, status| {
                colored_button_style(theme, status, color)
            })
            .into()
    } else if is_selecting {
        // Currently selecting - highlight
        btn.on_press(Message::ConfirmStemLink(stem_idx))
            .style(move |theme: &Theme, status| {
                let bright_color = Color::from_rgb(
                    (color.r + 0.3).min(1.0),
                    (color.g + 0.3).min(1.0),
                    (color.b + 0.3).min(1.0),
                );
                colored_button_style(theme, status, bright_color)
            })
            .into()
    } else {
        // No link - secondary style
        btn.on_press(Message::StartStemLinkSelection(stem_idx))
            .style(iced::widget::button::secondary)
            .into()
    }
}

/// Create a single stem link button
fn create_stem_link_button(
    stem_idx: usize,
    link: Option<&mesh_core::audio_file::StemLinkReference>,
    stem_link_selection: Option<usize>,
) -> Element<'static, Message> {
    let stem_name = STEM_NAMES[stem_idx];
    let color = STEM_COLORS[stem_idx];
    let is_selecting = stem_link_selection == Some(stem_idx);

    let label_text = if let Some(link_ref) = link {
        // Show stem name and linked track name
        let track_name = link_ref
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?");
        // Truncate long names
        let short_name: String = track_name.chars().take(12).collect();
        format!("{}\n{}", stem_name, short_name)
    } else if is_selecting {
        // Show that we're selecting
        format!("{}\n...", stem_name)
    } else {
        // Empty slot
        format!("{}\nLink", stem_name)
    };

    let btn = button(text(label_text).size(10).center())
        .width(Length::Fill)
        .height(Length::Fixed(40.0));

    if link.is_some() {
        // Has link - show in stem color
        btn.on_press(Message::StartStemLinkSelection(stem_idx))
            .style(move |theme: &Theme, status| {
                colored_button_style(theme, status, color)
            })
            .into()
    } else if is_selecting {
        // Currently selecting - highlight
        btn.on_press(Message::ConfirmStemLink(stem_idx))
            .style(move |theme: &Theme, status| {
                // Brighter version of stem color
                let bright_color = Color::from_rgb(
                    (color.r + 0.3).min(1.0),
                    (color.g + 0.3).min(1.0),
                    (color.b + 0.3).min(1.0),
                );
                colored_button_style(theme, status, bright_color)
            })
            .into()
    } else {
        // No link - secondary style
        btn.on_press(Message::StartStemLinkSelection(stem_idx))
            .style(iced::widget::button::secondary)
            .into()
    }
}
