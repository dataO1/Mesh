//! View function for the slice editor widget
//!
//! Uses native iced buttons instead of Canvas to avoid iced bug #3040
//! where multiple Canvas widgets don't render properly together.

use super::state::{SliceEditorState, NUM_PRESETS, NUM_SLICES, NUM_STEMS, NUM_STEPS, STEM_NAMES};
use crate::theme::STEM_COLORS;
use iced::widget::{button, column, row, text, Row};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};

/// Cell dimensions (wider than high, rectangular)
const CELL_WIDTH: f32 = 30.0;
const CELL_HEIGHT: f32 = 15.0;

/// Stem button dimensions
const STEM_BUTTON_WIDTH: f32 = 48.0;
const STEM_BUTTON_HEIGHT: f32 = 60.0; // Grid height / 4 stems

/// Mute button dimensions
const MUTE_BUTTON_WIDTH: f32 = 30.0;
const MUTE_BUTTON_HEIGHT: f32 = 20.0;

/// Preset tab height (width uses FillPortion to span full widget)
const PRESET_TAB_HEIGHT: f32 = 20.0;

/// Colors
const COLOR_BLACK: Color = Color::from_rgb(0.1, 0.1, 0.1);
const COLOR_WHITE: Color = Color::from_rgb(0.95, 0.95, 0.95);
const COLOR_DARK_GRAY: Color = Color::from_rgb(0.25, 0.25, 0.25);
const COLOR_MUTED_BG: Color = Color::from_rgb(0.35, 0.35, 0.35);

/// Create a slice editor element
///
/// # Arguments
/// * `state` - The slice editor state
/// * `on_cell_toggle` - Called when a grid cell is clicked (step, slice)
/// * `on_mute_toggle` - Called when a mute button is clicked (step)
/// * `on_stem_click` - Called when a stem button is clicked (stem_idx)
/// * `on_preset_select` - Called when a preset tab is clicked (preset_idx)
pub fn slice_editor<'a, Message: Clone + 'a>(
    state: &'a SliceEditorState,
    on_cell_toggle: impl Fn(usize, u8) -> Message + 'a + Clone,
    on_mute_toggle: impl Fn(usize) -> Message + 'a + Clone,
    on_stem_click: impl Fn(usize) -> Message + 'a + Clone,
    on_preset_select: impl Fn(usize) -> Message + 'a + Clone,
) -> Element<'a, Message> {
    // Build preset tabs row
    let preset_tabs = build_preset_tabs(state, on_preset_select);

    // Build stem buttons column
    let stem_buttons = build_stem_buttons(state, on_stem_click);

    // Build mute buttons row
    let mute_row = build_mute_row(state, on_mute_toggle.clone());

    // Build the 16x16 grid
    let grid = build_grid(state, on_cell_toggle);

    // Layout:
    // [Preset tabs                    ]
    // [Stem btns] [Mute row           ]
    // [         ] [Grid 16x16         ]

    let grid_with_mute = column![mute_row, grid].spacing(0);

    let main_content = row![stem_buttons, grid_with_mute]
        .spacing(2)
        .align_y(Alignment::End);

    column![preset_tabs, main_content]
        .spacing(4)
        .into()
}

/// Build the preset tabs row (8 numbered tabs spanning full widget width)
fn build_preset_tabs<'a, Message: Clone + 'a>(
    state: &'a SliceEditorState,
    on_preset_select: impl Fn(usize) -> Message + 'a + Clone,
) -> Element<'a, Message> {
    let tabs: Vec<Element<'a, Message>> = (0..NUM_PRESETS)
        .map(|i| {
            let is_selected = state.selected_preset == i;
            let label = format!("{}", i + 1);

            let style = if is_selected {
                PresetTabStyle::Selected
            } else {
                PresetTabStyle::Normal
            };

            let on_select = on_preset_select.clone();
            button(text(label).size(14))
                .width(Length::FillPortion(1)) // Equal width for all 8 tabs
                .height(PRESET_TAB_HEIGHT)
                .padding(Padding::from([4, 8]))
                .style(move |_theme, status| style.appearance(status))
                .on_press(on_select(i))
                .into()
        })
        .collect();

    // No spacing - tabs fill the full width flush
    Row::from_vec(tabs)
        .spacing(0)
        .width(Length::Fixed(STEM_BUTTON_WIDTH + 2.0 + (16.0 * CELL_WIDTH))) // Match main content width
        .into()
}

/// Build the stem buttons column (4 buttons)
fn build_stem_buttons<'a, Message: Clone + 'a>(
    state: &'a SliceEditorState,
    on_stem_click: impl Fn(usize) -> Message + 'a + Clone,
) -> Element<'a, Message> {
    // Order from top to bottom: OTH, VOC, BAS, DRM (drums at bottom)
    // Indices: VOC=0, DRM=1, BAS=2, OTH=3
    const STEM_ORDER: [usize; 4] = [3, 0, 2, 1]; // OTH, VOC, BAS, DRM (top to bottom)

    let buttons: Vec<Element<'a, Message>> = STEM_ORDER
        .iter()
        .map(|&stem_idx| {
            let is_enabled = state.stem_enabled[stem_idx];
            let is_selected = state.selected_stem == Some(stem_idx);
            let color = STEM_COLORS[stem_idx];

            let style = StemButtonStyle {
                enabled: is_enabled,
                selected: is_selected,
                color,
            };

            let on_click = on_stem_click.clone();
            button(text(STEM_NAMES[stem_idx]).size(14))
                .width(STEM_BUTTON_WIDTH)
                .height(STEM_BUTTON_HEIGHT)
                .padding(Padding::from([4, 8]))
                .style(move |_theme, status| style.appearance(status))
                .on_press(on_click(stem_idx))
                .into()
        })
        .collect();

    column(buttons)
        .spacing(0)
        .into()
}

/// Build the mute buttons row (16 buttons)
fn build_mute_row<'a, Message: Clone + 'a>(
    state: &'a SliceEditorState,
    on_mute_toggle: impl Fn(usize) -> Message + 'a + Clone,
) -> Element<'a, Message> {
    let buttons: Vec<Element<'a, Message>> = (0..NUM_STEPS)
        .map(|step| {
            let is_muted = state.is_step_muted(step);
            let style = MuteButtonStyle { muted: is_muted };

            let on_toggle = on_mute_toggle.clone();
            button(text("").size(8))
                .width(MUTE_BUTTON_WIDTH)
                .height(MUTE_BUTTON_HEIGHT)
                .padding(0)
                .style(move |_theme, status| style.appearance(status))
                .on_press(on_toggle(step))
                .into()
        })
        .collect();

    Row::from_vec(buttons)
        .spacing(0)
        .into()
}

/// Build the 16x16 grid of cells
fn build_grid<'a, Message: Clone + 'a>(
    state: &'a SliceEditorState,
    on_cell_toggle: impl Fn(usize, u8) -> Message + 'a + Clone,
) -> Element<'a, Message> {
    // Build rows from top to bottom (slice 15 at top, slice 0 at bottom)
    let rows: Vec<Element<'a, Message>> = (0..NUM_SLICES)
        .rev() // Reverse: row 15 at top, row 0 at bottom (Y origin at bottom-left)
        .map(|slice| {
            let slice_u8 = slice as u8;
            let cells: Vec<Element<'a, Message>> = (0..NUM_STEPS)
                .map(|step| {
                    let is_active = state.is_cell_active(step, slice_u8);
                    let is_muted = state.is_step_muted(step);
                    let is_default = SliceEditorState::is_default_position(step, slice_u8);

                    let style = CellStyle {
                        active: is_active,
                        muted: is_muted,
                        default_pos: is_default,
                    };

                    let on_toggle = on_cell_toggle.clone();
                    button(text("").size(6))
                        .width(CELL_WIDTH)
                        .height(CELL_HEIGHT)
                        .padding(0)
                        .style(move |_theme, status| style.appearance(status))
                        .on_press(on_toggle(step, slice_u8))
                        .into()
                })
                .collect();

            Row::from_vec(cells).spacing(0).into()
        })
        .collect();

    column(rows)
        .spacing(0)
        .into()
}

// =============================================================================
// Button Styles
// =============================================================================

/// Style for preset tab buttons
#[derive(Clone, Copy)]
enum PresetTabStyle {
    Normal,
    Selected,
}

impl PresetTabStyle {
    fn appearance(&self, status: button::Status) -> button::Style {
        let (bg, text_color) = match self {
            PresetTabStyle::Selected => (Color::from_rgb(0.3, 0.5, 0.8), Color::WHITE),
            PresetTabStyle::Normal => (COLOR_DARK_GRAY, Color::from_rgb(0.7, 0.7, 0.7)),
        };

        let bg = match status {
            button::Status::Hovered => lighten(bg, 0.1),
            button::Status::Pressed => darken(bg, 0.1),
            _ => bg,
        };

        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

/// Style for stem buttons
#[derive(Clone, Copy)]
struct StemButtonStyle {
    enabled: bool,
    selected: bool,
    color: Color,
}

impl StemButtonStyle {
    fn appearance(&self, status: button::Status) -> button::Style {
        let bg = if self.enabled {
            if self.selected {
                self.color // Bright when selected
            } else {
                darken(self.color, 0.3) // Dimmed when enabled but not selected
            }
        } else {
            COLOR_DARK_GRAY // Gray when disabled
        };

        let bg = match status {
            button::Status::Hovered => lighten(bg, 0.1),
            button::Status::Pressed => darken(bg, 0.1),
            _ => bg,
        };

        let text_color = if self.enabled {
            Color::WHITE
        } else {
            Color::from_rgb(0.5, 0.5, 0.5)
        };

        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

/// Style for mute buttons
#[derive(Clone, Copy)]
struct MuteButtonStyle {
    muted: bool,
}

impl MuteButtonStyle {
    fn appearance(&self, status: button::Status) -> button::Style {
        let bg = if self.muted {
            Color::from_rgb(0.8, 0.3, 0.3) // Red-ish when muted
        } else {
            COLOR_DARK_GRAY
        };

        let bg = match status {
            button::Status::Hovered => lighten(bg, 0.15),
            button::Status::Pressed => darken(bg, 0.1),
            _ => bg,
        };

        button::Style {
            background: Some(Background::Color(bg)),
            text_color: Color::WHITE,
            border: Border {
                color: Color::from_rgb(0.3, 0.3, 0.3),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

/// Style for grid cells
#[derive(Clone, Copy)]
struct CellStyle {
    active: bool,
    muted: bool,
    default_pos: bool,
}

impl CellStyle {
    fn appearance(&self, status: button::Status) -> button::Style {
        // Determine base color
        let bg = if self.muted {
            COLOR_MUTED_BG // Gray for muted columns
        } else if self.active {
            COLOR_BLACK // Black when ON
        } else if self.default_pos {
            COLOR_WHITE // White for default diagonal (x=y)
        } else {
            COLOR_DARK_GRAY // Dark gray for empty cells
        };

        let bg = match status {
            button::Status::Hovered => lighten(bg, 0.15),
            button::Status::Pressed => darken(bg, 0.1),
            _ => bg,
        };

        // Border: subtle for grid visibility
        let border_color = Color::from_rgb(0.2, 0.2, 0.2);

        button::Style {
            background: Some(Background::Color(bg)),
            text_color: Color::WHITE,
            border: Border {
                color: border_color,
                width: 0.5,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

// =============================================================================
// Color Utilities
// =============================================================================

fn lighten(color: Color, amount: f32) -> Color {
    Color::from_rgb(
        (color.r + amount).min(1.0),
        (color.g + amount).min(1.0),
        (color.b + amount).min(1.0),
    )
}

fn darken(color: Color, amount: f32) -> Color {
    Color::from_rgb(
        (color.r - amount).max(0.0),
        (color.g - amount).max(0.0),
        (color.b - amount).max(0.0),
    )
}
