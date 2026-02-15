//! PlayerCanvas - 4-deck unified waveform view
//!
//! Displays all 4 decks in a single canvas with zoomed and overview waveforms.

use super::super::state::{
    CombinedState, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    DECK_HEADER_HEIGHT, MAX_ZOOM_BARS, MIN_ZOOM_BARS, WAVEFORM_HEIGHT,
    ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};
use super::{
    draw_stem_waveform_filled, draw_stem_waveform_lower, draw_stem_waveform_lower_aligned,
    draw_stem_waveform_upper, draw_stem_waveform_upper_aligned, highres_target_pixels,
    sample_peak_smoothed, smooth_radius_for_stem, zoomed_step, DROP_MARKER_COLOR,
    INACTIVE_STEM_GRAYS, OVERVIEW_WAVEFORM_ALPHA, STEM_INDICATOR_ORDER, STEM_RENDER_ORDER,
    ZOOMED_WAVEFORM_ALPHA,
};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{mouse, Color, Point, Rectangle, Size, Theme};
use mesh_core::engine::SLICER_NUM_SLICES;
use mesh_core::types::SAMPLE_RATE;

// =============================================================================
// Player Canvas Layout Constants
// =============================================================================

/// Gap between deck cells in the 2x2 grid
/// Kept tight (4px) so mirrored overview waveforms cluster in the middle.
pub const DECK_GRID_GAP: f32 = 4.0;

/// Gap between zoomed and overview within a deck cell
pub const DECK_INTERNAL_GAP: f32 = 2.0;

/// Total height of one deck cell (header + zoomed + gap + overview)
/// 16 + 120 + 2 + 35 = 173px
pub const DECK_CELL_HEIGHT: f32 =
    DECK_HEADER_HEIGHT + ZOOMED_WAVEFORM_HEIGHT + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;

/// Compute dynamic cell layout from available canvas height.
/// Header, overview, and gaps stay fixed; extra space goes to zoomed waveform.
fn cell_height_from_bounds(bounds_height: f32) -> (f32, f32) {
    let cell_height = (bounds_height - DECK_GRID_GAP) / 2.0;
    let zoomed_height = (cell_height - DECK_HEADER_HEIGHT - DECK_INTERNAL_GAP - WAVEFORM_HEIGHT)
        .max(ZOOMED_WAVEFORM_HEIGHT); // never shrink below the original size
    (cell_height, zoomed_height)
}

// =============================================================================
// Player Canvas Interaction State
// =============================================================================

/// Canvas state for 4-deck player canvas
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInteraction {
    /// Which deck is currently being interacted with (0-3), None if no interaction
    pub active_deck: Option<usize>,
    /// Zoom gesture state (drag Y position when started)
    pub drag_start_y: Option<f32>,
    /// Zoom level when drag started
    pub drag_start_zoom: u32,
    /// Whether dragging in overview region for seeking
    pub is_seeking: bool,
}

// =============================================================================
// Player Canvas Program
// =============================================================================

/// Canvas program for 4-deck player waveform rendering
///
/// Displays all 4 decks in a single canvas:
/// - **Zoomed grid** (2x2): Deck 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
/// - **Overview stack**: Decks 1-4 stacked vertically below the grid
///
/// Takes callback closures with deck index for both seek and zoom operations.
pub struct PlayerCanvas<'a, Message, SeekFn, ZoomFn>
where
    SeekFn: Fn(usize, f64) -> Message,
    ZoomFn: Fn(usize, u32) -> Message,
{
    pub state: &'a PlayerCanvasState,
    pub on_seek: SeekFn,
    pub on_zoom: ZoomFn,
}

impl<'a, Message, SeekFn, ZoomFn> PlayerCanvas<'a, Message, SeekFn, ZoomFn>
where
    SeekFn: Fn(usize, f64) -> Message,
    ZoomFn: Fn(usize, u32) -> Message,
{
    /// Get deck index from zoomed grid position (row, col)
    /// Layout: 1=top-left, 2=top-right, 3=bottom-left, 4=bottom-right
    fn deck_from_grid(row: usize, col: usize) -> usize {
        match (row, col) {
            (0, 0) => 0, // Deck 1
            (0, 1) => 1, // Deck 2
            (1, 0) => 2, // Deck 3
            (1, 1) => 3, // Deck 4
            _ => 0,
        }
    }
}

impl<'a, Message, SeekFn, ZoomFn> Program<Message> for PlayerCanvas<'a, Message, SeekFn, ZoomFn>
where
    Message: Clone,
    SeekFn: Fn(usize, f64) -> Message,
    ZoomFn: Fn(usize, u32) -> Message,
{
    type State = PlayerInteraction;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let width = bounds.width;
        let cell_width = (width - DECK_GRID_GAP) / 2.0;
        let (cell_height, zoomed_height) = cell_height_from_bounds(bounds.height);

        // Determine which deck quadrant the cursor is in (if any)
        if let Some(position) = cursor.position_in(bounds) {
            let col = if position.x < cell_width { 0 } else { 1 };
            let row = if position.y < cell_height { 0 } else { 1 };
            let deck_idx = Self::deck_from_grid(row, col);

            // Calculate position within the deck cell
            let cell_x = if col == 0 { 0.0 } else { cell_width + DECK_GRID_GAP };
            let cell_y = if row == 0 { 0.0 } else { cell_height + DECK_GRID_GAP };
            let local_x = position.x - cell_x;
            let local_y = position.y - cell_y;

            // Determine which region within the cell: header, zoomed, or overview
            // Bottom row (row 1) is mirrored: overview → gap → zoomed → header
            let mirrored = row == 1;
            let (zoomed_start, zoomed_end, overview_start, overview_end) = if mirrored {
                // overview → gap → zoomed → header
                let overview_start = 0.0_f32;
                let overview_end = WAVEFORM_HEIGHT;
                let zoomed_start = WAVEFORM_HEIGHT + DECK_INTERNAL_GAP;
                let zoomed_end = zoomed_start + zoomed_height;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            } else {
                // header → zoomed → gap → overview
                let zoomed_start = DECK_HEADER_HEIGHT;
                let zoomed_end = zoomed_start + zoomed_height;
                let overview_start = zoomed_end + DECK_INTERNAL_GAP;
                let overview_end = overview_start + WAVEFORM_HEIGHT;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            };

            // Check if in zoomed region (drag to zoom)
            if local_y >= zoomed_start && local_y < zoomed_end {
                match event {
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                        interaction.active_deck = Some(deck_idx);
                        interaction.drag_start_y = Some(position.y);
                        interaction.drag_start_zoom = self.state.decks[deck_idx].zoomed.zoom_bars;
                        interaction.is_seeking = false;
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        interaction.drag_start_y = None;
                        interaction.active_deck = None;
                    }
                    Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                        if let (Some(start_y), Some(active_deck)) = (interaction.drag_start_y, interaction.active_deck) {
                            let delta = start_y - position.y;
                            let zoom_change = (delta / ZOOM_PIXELS_PER_LEVEL) as i32;
                            let new_zoom = (interaction.drag_start_zoom as i32 - zoom_change)
                                .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32)
                                as u32;

                            if new_zoom != self.state.decks[active_deck].zoomed.zoom_bars {
                                return Some(canvas::Action::publish((self.on_zoom)(active_deck, new_zoom)));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Check if in overview region (click to seek)
            else if local_y >= overview_start && local_y < overview_end {
                match event {
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                        interaction.active_deck = Some(deck_idx);
                        interaction.is_seeking = true;
                        interaction.drag_start_y = None;

                        let overview = &self.state.decks[deck_idx].overview;
                        if overview.has_track && overview.duration_samples > 0 {
                            // Calculate seek ratio relative to cell width
                            let seek_ratio = (local_x / cell_width).clamp(0.0, 1.0) as f64;
                            return Some(canvas::Action::publish((self.on_seek)(deck_idx, seek_ratio)));
                        }
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        interaction.is_seeking = false;
                        interaction.active_deck = None;
                    }
                    Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                        if interaction.is_seeking {
                            if let Some(active_deck) = interaction.active_deck {
                                let overview = &self.state.decks[active_deck].overview;
                                if overview.has_track && overview.duration_samples > 0 {
                                    let seek_ratio = (local_x / cell_width).clamp(0.0, 1.0) as f64;
                                    return Some(canvas::Action::publish((self.on_seek)(active_deck, seek_ratio)));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Handle button release outside bounds
        if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            interaction.drag_start_y = None;
            interaction.active_deck = None;
            interaction.is_seeking = false;
        }

        None
    }

    fn mouse_interaction(
        &self,
        interaction: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(position) = cursor.position_in(bounds) {
            let (cell_height, zoomed_height) = cell_height_from_bounds(bounds.height);

            // Determine which row we're in
            let row = if position.y < cell_height { 0 } else { 1 };
            let cell_y = if row == 0 { 0.0 } else { cell_height + DECK_GRID_GAP };
            let local_y = position.y - cell_y;

            // Regions within cell — mirrored for bottom row
            let mirrored = row == 1;
            let (zoomed_start, zoomed_end, overview_start, overview_end) = if mirrored {
                let overview_start = 0.0_f32;
                let overview_end = WAVEFORM_HEIGHT;
                let zoomed_start = WAVEFORM_HEIGHT + DECK_INTERNAL_GAP;
                let zoomed_end = zoomed_start + zoomed_height;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            } else {
                let zoomed_start = DECK_HEADER_HEIGHT;
                let zoomed_end = zoomed_start + zoomed_height;
                let overview_start = zoomed_end + DECK_INTERNAL_GAP;
                let overview_end = overview_start + WAVEFORM_HEIGHT;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            };

            if local_y >= zoomed_start && local_y < zoomed_end {
                // In zoomed region
                if interaction.drag_start_y.is_some() {
                    mouse::Interaction::ResizingVertically
                } else {
                    mouse::Interaction::Grab
                }
            } else if local_y >= overview_start && local_y < overview_end {
                // In overview region
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::default()
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _interaction: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let cell_width = (width - DECK_GRID_GAP) / 2.0;
        let (cell_height, zoomed_height) = cell_height_from_bounds(bounds.height);

        // =====================================================================
        // DECK QUADRANTS (2x2 grid, each with header + zoomed + overview)
        // =====================================================================
        // Deck 1 = top-left, Deck 2 = top-right
        // Deck 3 = bottom-left, Deck 4 = bottom-right
        let grid_positions = [
            (0.0, 0.0),                                      // Deck 1: top-left
            (cell_width + DECK_GRID_GAP, 0.0),              // Deck 2: top-right
            (0.0, cell_height + DECK_GRID_GAP),             // Deck 3: bottom-left
            (cell_width + DECK_GRID_GAP, cell_height + DECK_GRID_GAP), // Deck 4: bottom-right
        ];

        for (deck_idx, (x, y)) in grid_positions.iter().enumerate() {
            // Use interpolated playhead for smooth animation
            let playhead = self.state.interpolated_playhead(deck_idx, SAMPLE_RATE);
            let is_master = self.state.is_master(deck_idx);
            let track_name = self.state.track_name(deck_idx);
            let track_key = self.state.track_key(deck_idx);
            let stem_active = self.state.stem_active(deck_idx);
            let transpose = self.state.transpose(deck_idx);
            let key_match_enabled = self.state.key_match_enabled(deck_idx);
            let (linked_stems, linked_active) = self.state.linked_stems(deck_idx);

            let lufs_gain_db = self.state.lufs_gain_db(deck_idx);
            let track_bpm = self.state.track_bpm(deck_idx);

            let cue_enabled = self.state.cue_enabled(deck_idx);
            let loop_length_beats = self.state.loop_length_beats(deck_idx);
            let loop_active = self.state.loop_active(deck_idx);
            let volume = self.state.volume(deck_idx);

            let mirrored = deck_idx >= 2; // Bottom row decks are mirrored

            draw_deck_quadrant(
                &mut frame,
                &self.state.decks[deck_idx],
                playhead,
                *x,
                *y,
                cell_width,
                zoomed_height,
                deck_idx,
                track_name,
                track_key,
                is_master,
                cue_enabled,
                stem_active,
                transpose,
                key_match_enabled,
                self.state.stem_colors(),
                linked_stems,
                linked_active,
                lufs_gain_db,
                track_bpm,
                loop_length_beats,
                loop_active,
                volume,
                mirrored,
            );
        }

        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Offset-Aware Drawing Helpers (for PlayerCanvas)
// =============================================================================

/// Draw a complete deck quadrant (header + zoomed + overview)
///
/// Layout:
/// ```text
/// +-------------------------------------+
/// | [N] Track Name Here         16px   | <- Header row
/// +-------------------------------------+
/// |                                     |
/// |     Zoomed Waveform          120px |
/// |                                     |
/// +-------------------------------------+
/// |     Overview Waveform         35px |
/// +-------------------------------------+
/// ```
fn draw_deck_quadrant(
    frame: &mut Frame,
    deck: &CombinedState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    zoomed_height: f32,
    deck_idx: usize,
    track_name: &str,
    track_key: &str,
    is_master: bool,
    cue_enabled: bool,
    stem_active: &[bool; 4],
    transpose: i8,
    key_match_enabled: bool,
    stem_colors: &[Color; 4],
    linked_stems: &[bool; 4],
    linked_active: &[bool; 4],
    lufs_gain_db: Option<f32>,
    track_bpm: Option<f64>,
    loop_length_beats: Option<f32>,
    loop_active: bool,
    volume: f32,
    mirrored: bool,
) {
    use iced::widget::canvas::Text;
    use iced::alignment::{Horizontal, Vertical};

    // Stem indicator width on left side
    const STEM_INDICATOR_WIDTH: f32 = 6.0;
    const STEM_INDICATOR_GAP: f32 = 2.0;

    // Compute Y positions based on mirrored layout
    let (header_y, zoomed_y, overview_y) = if mirrored {
        // Bottom decks: overview → gap → zoomed → header
        let overview_y = y;
        let zoomed_y = y + WAVEFORM_HEIGHT + DECK_INTERNAL_GAP;
        let header_y = zoomed_y + zoomed_height;
        (header_y, zoomed_y, overview_y)
    } else {
        // Top decks: header → zoomed → gap → overview
        let header_y = y;
        let zoomed_y = y + DECK_HEADER_HEIGHT;
        let overview_y = zoomed_y + zoomed_height + DECK_INTERNAL_GAP;
        (header_y, zoomed_y, overview_y)
    };

    // Draw header background
    let header_bg_color = Color::from_rgb(0.10, 0.10, 0.12);
    frame.fill_rectangle(
        Point::new(x, header_y),
        Size::new(width, DECK_HEADER_HEIGHT),
        header_bg_color,
    );

    // Draw deck number badge background
    let badge_width = 48.0;
    let badge_margin = 6.0;
    let badge_height = DECK_HEADER_HEIGHT - 10.0;
    let badge_y = header_y + 5.0;

    // Badge background color based on state (cue takes priority for fill)
    let badge_bg_color = if cue_enabled {
        Color::from_rgb(0.35, 0.30, 0.10) // Dark yellow/amber for cue
    } else if deck.zoomed.has_track {
        Color::from_rgb(0.15, 0.15, 0.25) // Dark blue for loaded
    } else {
        Color::from_rgb(0.15, 0.15, 0.15) // Dark gray for empty
    };

    frame.fill_rectangle(
        Point::new(x + badge_margin, badge_y),
        Size::new(badge_width, badge_height),
        badge_bg_color,
    );

    // Draw green border around badge when master (uses Vocals stem green)
    if is_master {
        let border_color = Color::from_rgb(0.45, 0.8, 0.55); // Sage green (matches stem colors)
        let stroke = Stroke::default().with_width(2.0).with_color(border_color);
        frame.stroke(
            &Path::rectangle(
                Point::new(x + badge_margin, badge_y),
                Size::new(badge_width, badge_height),
            ),
            stroke,
        );
    }

    // Draw deck number text
    let deck_num_text = format!("{}", deck_idx + 1);
    let text_color = if cue_enabled {
        Color::from_rgb(1.0, 0.85, 0.3) // Bright yellow/amber for cue
    } else if deck.zoomed.has_track {
        Color::from_rgb(0.7, 0.7, 0.9) // Light blue for loaded
    } else {
        Color::from_rgb(0.5, 0.5, 0.5) // Gray for empty
    };

    frame.fill_text(Text {
        content: deck_num_text,
        position: Point::new(x + badge_margin + badge_width / 2.0, header_y + DECK_HEADER_HEIGHT / 2.0),
        size: 24.0.into(),
        color: text_color,
        align_x: Horizontal::Center.into(),
        align_y: Vertical::Center.into(),
        ..Text::default()
    });

    // Calculate reserved space for right-side elements (Key, LUFS, Loop, BPM)
    let key_space = if deck.overview.has_track && !track_key.is_empty() { 150.0 } else { 0.0 };
    let lufs_space = if deck.overview.has_track && lufs_gain_db.is_some() { 90.0 } else { 0.0 };
    let loop_space = if deck.overview.has_track && loop_length_beats.is_some() { 70.0 } else { 0.0 };
    let bpm_space = if deck.overview.has_track && track_bpm.is_some() { 100.0 } else { 0.0 };

    // Draw BPM indicator (to the left of loop length)
    let bpm_display_width = if deck.overview.has_track {
        if let Some(bpm) = track_bpm {
            let bpm_text = format!("{:.1}", bpm);

            // Position to the left of loop (which is left of LUFS, which is left of key)
            let bpm_x = x + width - key_space - lufs_space - loop_space - 12.0;
            frame.fill_text(Text {
                content: bpm_text,
                position: Point::new(bpm_x, header_y + DECK_HEADER_HEIGHT / 2.0),
                size: 18.0.into(),
                color: Color::from_rgb(0.7, 0.7, 0.8), // Light blue-gray
                align_x: Horizontal::Right.into(),
                align_y: Vertical::Center.into(),
                ..Text::default()
            });
            bpm_space
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Draw loop length indicator (between BPM and LUFS gain)
    let loop_display_width = if deck.overview.has_track {
        if let Some(beats) = loop_length_beats {
            let loop_text = if beats < 1.0 {
                format!("1/{:.0}", 1.0 / beats)
            } else {
                format!("{:.0}", beats)
            };

            // Color: bright when loop is active, dim when inactive
            let loop_color = if loop_active {
                Color::from_rgb(0.4, 0.9, 0.4) // Green when active
            } else {
                Color::from_rgb(0.5, 0.5, 0.5) // Gray when inactive
            };

            let loop_x = x + width - key_space - lufs_space - 12.0;
            frame.fill_text(Text {
                content: format!("\u{21BB}{}", loop_text),
                position: Point::new(loop_x, header_y + DECK_HEADER_HEIGHT / 2.0),
                size: 18.0.into(),
                color: loop_color,
                align_x: Horizontal::Right.into(),
                align_y: Vertical::Center.into(),
                ..Text::default()
            });
            loop_space
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Draw LUFS gain compensation indicator (to the left of key)
    let gain_display_width = if deck.overview.has_track {
        if let Some(gain_db) = lufs_gain_db {
            let gain_text = if gain_db >= 0.0 {
                format!("+{:.1}dB", gain_db)
            } else {
                format!("{:.1}dB", gain_db)
            };

            // Color: cyan for boost, orange for cut, gray for near-unity
            let gain_color = if gain_db.abs() < 0.5 {
                Color::from_rgb(0.5, 0.5, 0.5) // Gray for negligible gain
            } else if gain_db > 0.0 {
                Color::from_rgb(0.5, 0.8, 0.9) // Cyan for boost (quiet track)
            } else {
                Color::from_rgb(0.9, 0.7, 0.5) // Orange for cut (loud track)
            };

            // Position to the left of the key display
            frame.fill_text(Text {
                content: gain_text,
                position: Point::new(x + width - key_space - 12.0, header_y + DECK_HEADER_HEIGHT / 2.0),
                size: 18.0.into(),
                color: gain_color,
                align_x: Horizontal::Right.into(),
                align_y: Vertical::Center.into(),
                ..Text::default()
            });
            lufs_space
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Draw track key in top right corner (if loaded)
    if deck.overview.has_track && !track_key.is_empty() {
        let (key_display, key_color) = if is_master || !key_match_enabled {
            // Master deck or key match disabled: just show key
            (track_key.to_string(), Color::from_rgb(0.6, 0.8, 0.6))
        } else if transpose == 0 {
            // Key match enabled, compatible keys (no transpose needed)
            (format!("{} \u{2713}", track_key), Color::from_rgb(0.5, 0.9, 0.5)) // Brighter green
        } else {
            // Key match enabled, transposing
            let sign = if transpose > 0 { "+" } else { "" };
            (format!("{} \u{2192} {}{}", track_key, sign, transpose), Color::from_rgb(0.9, 0.7, 0.5)) // Orange tint
        };

        frame.fill_text(Text {
            content: key_display,
            position: Point::new(x + width - 12.0, header_y + DECK_HEADER_HEIGHT / 2.0),
            size: 20.0.into(),
            color: key_color,
            align_x: Horizontal::Right.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    }

    // Draw linked stem indicators (small diamonds between deck badge and track name)
    let has_any_links = linked_stems.iter().any(|&has| has);
    if has_any_links {
        let link_x_start = x + badge_margin + badge_width + 6.0;
        let link_y = header_y + DECK_HEADER_HEIGHT / 2.0;
        let diamond_size = 6.0;
        let diamond_gap = 3.0;

        for (stem_idx, &has_link) in linked_stems.iter().enumerate() {
            if has_link {
                let dx = link_x_start + (stem_idx as f32) * (diamond_size * 2.0 + diamond_gap);
                let is_active = linked_active[stem_idx];

                // Use stem color for the diamond, brighter if active
                let base_color = stem_colors[stem_idx];
                let diamond_color = if is_active {
                    // Bright stem color when linked stem is active
                    Color::from_rgba(base_color.r, base_color.g, base_color.b, 1.0)
                } else {
                    // Dimmed when link exists but using original
                    Color::from_rgba(base_color.r * 0.5, base_color.g * 0.5, base_color.b * 0.5, 0.6)
                };

                // Draw diamond shape
                let diamond = Path::new(|builder| {
                    builder.move_to(Point::new(dx, link_y - diamond_size)); // Top
                    builder.line_to(Point::new(dx - diamond_size, link_y)); // Left
                    builder.line_to(Point::new(dx, link_y + diamond_size)); // Bottom
                    builder.line_to(Point::new(dx + diamond_size, link_y)); // Right
                    builder.close();
                });
                frame.fill(&diamond, diamond_color);

                // Add small "L" indicator for "linked" when active
                if is_active {
                    frame.stroke(
                        &diamond,
                        Stroke::default()
                            .with_color(Color::WHITE)
                            .with_width(0.5),
                    );
                }
            }
        }
    }

    // Draw track name text (if loaded)
    let linked_indicator_space = if has_any_links { 68.0 } else { 0.0 };
    let name_x = x + badge_margin + badge_width + 12.0 + linked_indicator_space;
    let max_name_width = width - badge_width - badge_margin * 2.0 - 24.0 - key_space - linked_indicator_space - gain_display_width - bpm_display_width - loop_display_width;

    if deck.overview.has_track && !track_name.is_empty() {
        // Truncate track name if too long
        let max_chars = (max_name_width / 13.0) as usize;
        let display_name = if track_name.len() > max_chars && max_chars > 3 {
            format!("{}...", &track_name[..max_chars - 3])
        } else {
            track_name.to_string()
        };

        frame.fill_text(Text {
            content: display_name,
            position: Point::new(name_x, header_y + DECK_HEADER_HEIGHT / 2.0),
            size: 22.0.into(),
            color: Color::from_rgb(0.75, 0.75, 0.75),
            align_x: Horizontal::Left.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    } else {
        // Show "No track" for empty decks
        frame.fill_text(Text {
            content: "No track".to_string(),
            position: Point::new(name_x, header_y + DECK_HEADER_HEIGHT / 2.0),
            size: 20.0.into(),
            color: Color::from_rgb(0.4, 0.4, 0.4),
            align_x: Horizontal::Left.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    }

    // Draw zoomed waveform
    draw_zoomed_at(
        frame,
        &deck.zoomed,
        &deck.overview.highres_peaks,
        &deck.overview.linked_highres_peaks,
        deck.zoomed.lufs_gain,
        &deck.overview.linked_lufs_gains,
        deck.overview.duration_samples,
        playhead,
        x,
        zoomed_y,
        width,
        zoomed_height,
        is_master,
        stem_colors,
        stem_active,
        linked_active,
    );

    // Draw stem status indicators on left side of zoomed waveform only
    let indicator_height = (zoomed_height - (STEM_INDICATOR_GAP * 3.0)) / 4.0;

    for (visual_idx, &stem_idx) in STEM_INDICATOR_ORDER.iter().enumerate() {
        let indicator_y = zoomed_y + (visual_idx as f32) * (indicator_height + STEM_INDICATOR_GAP);
        let color = stem_colors[stem_idx];

        // Simple bypass toggle: 50% brightness if active, dark if bypassed
        let indicator_color = if stem_active[stem_idx] {
            Color::from_rgb(
                color.r * 0.5,
                color.g * 0.5,
                color.b * 0.5,
            )
        } else {
            Color::from_rgb(0.12, 0.12, 0.12)
        };

        frame.fill_rectangle(
            Point::new(x + 2.0, indicator_y),
            Size::new(STEM_INDICATOR_WIDTH, indicator_height),
            indicator_color,
        );
    }
    draw_overview_at(
        frame,
        &deck.overview,
        playhead,
        x,
        overview_y,
        width,
        stem_colors,
        stem_active,
        linked_stems,
        linked_active,
    );

    // Draw volume dimming overlay over the waveform area (not the header)
    // At full volume (1.0) no dimming, at zero volume max dimming (0.4 alpha)
    if volume < 0.99 {
        let dim_alpha = (1.0 - volume) * 0.4;
        let waveform_area_y = overview_y.min(zoomed_y);
        let waveform_area_height = zoomed_height + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;
        frame.fill_rectangle(
            Point::new(x, waveform_area_y),
            Size::new(width, waveform_area_height),
            Color::from_rgba(0.0, 0.0, 0.0, dim_alpha),
        );
    }
}

/// Draw a zoomed waveform at a specific position
///
/// Uses pre-computed high-resolution peaks when available for smooth playback
/// without recomputation. Falls back to cached_peaks if highres_peaks is empty.
/// When a linked stem is active, uses linked_highres_peaks for that stem.
fn draw_zoomed_at(
    frame: &mut Frame,
    zoomed: &ZoomedState,
    highres_peaks: &[Vec<(f32, f32)>; 4],
    linked_highres_peaks: &[Option<Vec<(f32, f32)>>; 4],
    host_lufs_gain: f32,
    linked_lufs_gains: &[f32; 4],
    duration_samples: u64,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    is_master: bool,
    stem_colors: &[Color; 4],
    stem_active: &[bool; 4],
    linked_active: &[bool; 4],
) {
    let center_y = y + height / 2.0;

    // Background
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, height),
        Color::from_rgb(0.08, 0.08, 0.1),
    );

    if !zoomed.has_track || zoomed.duration_samples == 0 {
        return;
    }

    // Get window with padding info for proper boundary handling
    let window = zoomed.visible_window(playhead);

    if window.total_samples == 0 {
        return;
    }

    // Helper to convert sample position to x coordinate (accounting for padding)
    let sample_to_x = |sample: u64| -> f32 {
        if sample < window.start {
            x + (window.left_padding as f64 / window.total_samples as f64 * width as f64) as f32
        } else if sample > window.end {
            x + width
        } else {
            let offset = window.left_padding + (sample - window.start);
            x + (offset as f64 / window.total_samples as f64 * width as f64) as f32
        }
    };

    {
        // Draw loop region (behind everything else)
        if let Some((loop_start_norm, loop_end_norm)) = zoomed.loop_region {
            let loop_start_sample = (loop_start_norm * zoomed.duration_samples as f64) as u64;
            let loop_end_sample = (loop_end_norm * zoomed.duration_samples as f64) as u64;

            if loop_end_sample > window.start && loop_start_sample < window.end {
                let start_x = sample_to_x(loop_start_sample.max(window.start));
                let end_x = sample_to_x(loop_end_sample.min(window.end));

                let loop_width = end_x - start_x;
                if loop_width > 0.0 {
                    frame.fill_rectangle(
                        Point::new(start_x, y),
                        Size::new(loop_width, height),
                        Color::from_rgba(0.2, 0.8, 0.2, 0.25),
                    );
                    if loop_start_sample >= window.start && loop_start_sample <= window.end {
                        let lx = sample_to_x(loop_start_sample);
                        frame.stroke(
                            &Path::line(Point::new(lx, y), Point::new(lx, y + height)),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                    if loop_end_sample >= window.start && loop_end_sample <= window.end {
                        let lx = sample_to_x(loop_end_sample);
                        frame.stroke(
                            &Path::line(Point::new(lx, y), Point::new(lx, y + height)),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                }
            }
        }

        // Draw slicer region (orange overlay with slice divisions)
        let slicer_bounds: Option<(u64, u64)> = zoomed.fixed_buffer_bounds.or_else(|| {
            zoomed.slicer_region.map(|(start_norm, end_norm)| {
                let start = (start_norm * zoomed.duration_samples as f64) as u64;
                let end = (end_norm * zoomed.duration_samples as f64) as u64;
                (start, end)
            })
        });

        if let Some((slicer_start_sample, slicer_end_sample)) = slicer_bounds {
            if slicer_end_sample > window.start && slicer_start_sample < window.end {
                let start_x = sample_to_x(slicer_start_sample.max(window.start));
                let end_x = sample_to_x(slicer_end_sample.min(window.end));

                let slicer_width = end_x - start_x;
                if slicer_width > 0.0 {
                    // Orange overlay for slicer buffer
                    frame.fill_rectangle(
                        Point::new(start_x, y),
                        Size::new(slicer_width, height),
                        Color::from_rgba(1.0, 0.5, 0.0, 0.12),
                    );

                    // Draw slice divisions (if they fit in view)
                    let samples_per_slice = (slicer_end_sample - slicer_start_sample) / SLICER_NUM_SLICES as u64;

                    for i in 0..=SLICER_NUM_SLICES {
                        let slice_sample = slicer_start_sample + samples_per_slice * i as u64;

                        if slice_sample >= window.start && slice_sample <= window.end {
                            let slice_x = sample_to_x(slice_sample);
                            let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
                            let line_width = if is_boundary { 2.0 } else { 1.0 };
                            let alpha = if is_boundary { 0.8 } else { 0.5 };

                            frame.stroke(
                                &Path::line(Point::new(slice_x, y), Point::new(slice_x, y + height)),
                                Stroke::default()
                                    .with_color(Color::from_rgba(1.0, 0.6, 0.1, alpha))
                                    .with_width(line_width),
                            );
                        }
                    }

                    // Highlight current playing slice with brighter overlay
                    if let Some(current) = zoomed.slicer_current_slice {
                        let slice_start_sample = slicer_start_sample + samples_per_slice * current as u64;
                        let slice_end_sample = slice_start_sample + samples_per_slice;

                        if slice_end_sample > window.start && slice_start_sample < window.end {
                            let slice_start_x = sample_to_x(slice_start_sample.max(window.start));
                            let slice_end_x = sample_to_x(slice_end_sample.min(window.end));

                            frame.fill_rectangle(
                                Point::new(slice_start_x, y),
                                Size::new(slice_end_x - slice_start_x, height),
                                Color::from_rgba(1.0, 0.6, 0.0, 0.2),
                            );
                        }
                    }
                }
            }
        }

        // Draw beat markers (only within actual audio range)
        for (i, &beat_sample) in zoomed.beat_grid.iter().enumerate() {
            if beat_sample >= window.start && beat_sample <= window.end {
                let beat_x = sample_to_x(beat_sample);
                let (color, w) = if i % 4 == 0 {
                    (Color::from_rgba(1.0, 0.3, 0.3, 0.6), 2.0)
                } else {
                    (Color::from_rgba(0.5, 0.5, 0.5, 0.4), 1.0)
                };
                frame.stroke(
                    &Path::line(Point::new(beat_x, y), Point::new(beat_x, y + height)),
                    Stroke::default().with_color(color).with_width(w),
                );
            }
        }

        // Draw peaks using filled paths
        let use_highres = !highres_peaks[0].is_empty() && duration_samples > 0;
        let use_cached = !use_highres
            && !zoomed.cached_peaks[0].is_empty()
            && (zoomed.cache_end > zoomed.cache_start || zoomed.cache_left_padding > 0);

        if use_highres || use_cached {
            let height_scale = height / 2.0 * 0.85;

            // For cached peaks fallback
            let cache_virtual_total = if use_cached {
                (zoomed.cache_end - zoomed.cache_start + zoomed.cache_left_padding) as usize
            } else {
                0
            };

            // Draw stems in layered order: Drums (back) -> Bass -> Vocals -> Other (front)
            for &stem_idx in STEM_RENDER_ORDER.iter() {
                // Choose peaks source based on what's available
                let peaks: &[(f32, f32)] = if linked_active[stem_idx] {
                    // Linked stem is active - prefer linked highres peaks
                    if use_highres {
                        linked_highres_peaks[stem_idx]
                            .as_ref()
                            .map(|v| v.as_slice())
                            .unwrap_or(&highres_peaks[stem_idx])
                    } else {
                        // Fallback: use linked cached peaks if available
                        zoomed.linked_cached_peaks[stem_idx]
                            .as_ref()
                            .map(|v| v.as_slice())
                            .unwrap_or(&zoomed.cached_peaks[stem_idx])
                    }
                } else if use_highres {
                    // Host track - use host highres_peaks
                    &highres_peaks[stem_idx]
                } else {
                    &zoomed.cached_peaks[stem_idx]
                };

                if peaks.is_empty() {
                    continue;
                }
                let peaks_len = peaks.len();

                // Use stem color if active, gray tone if inactive
                let waveform_color = if stem_active[stem_idx] {
                    let base_color = stem_colors[stem_idx];
                    Color::from_rgba(base_color.r, base_color.g, base_color.b, ZOOMED_WAVEFORM_ALPHA)
                } else {
                    let gray = INACTIVE_STEM_GRAYS[stem_idx];
                    Color::from_rgba(gray.r, gray.g, gray.b, 0.5)
                };

                // Build filled path for this stem
                let path = Path::new(|builder| {
                    let mut first_point = true;
                    let mut upper_points: Vec<(f32, f32)> = Vec::with_capacity(512);
                    let mut lower_points: Vec<(f32, f32)> = Vec::with_capacity(512);

                    if use_highres {
                        // STABLE RENDERING: Direct peak-to-pixel mapping
                        let samples_per_peak = (duration_samples / peaks_len as u64) as f64;
                        let pixels_per_sample = width as f64 / window.total_samples as f64;
                        let pixels_per_peak = samples_per_peak * pixels_per_sample;

                        // Center position (where playhead is, accounting for padding)
                        let center_sample = window.start as f64 - window.left_padding as f64 + (window.total_samples as f64 / 2.0);
                        let center_peak_f64 = center_sample / samples_per_peak;
                        let center_x = x + width / 2.0;

                        // Calculate visible peak range with margin to prevent edge popping
                        let half_width_in_peaks = (width as f64 / 2.0 / pixels_per_peak).ceil() as usize;
                        let margin_peaks = half_width_in_peaks / 4 + 20;
                        let half_visible_peaks = half_width_in_peaks + margin_peaks;

                        // Calculate first and last peak to draw (with margin)
                        let center_peak = center_peak_f64 as usize;
                        let first_peak = center_peak.saturating_sub(half_visible_peaks);
                        let last_peak = (center_peak + half_visible_peaks).min(peaks_len);

                        // Stem-specific subsampling (drums=detail, bass=smooth)
                        let target_pixels_per_point = highres_target_pixels(stem_idx);
                        let step = ((target_pixels_per_point / pixels_per_peak).round() as usize).max(1);
                        let smooth_radius = smooth_radius_for_stem(stem_idx, step);

                        // Align to grid for stability (round to nearest)
                        let first_peak_aligned = ((first_peak + step / 2) / step) * step;
                        let mut peak_idx = first_peak_aligned;
                        while peak_idx < last_peak {
                            // SIMPLE LINEAR MAPPING: pixel position from peak index
                            let relative_pos = peak_idx as f64 - center_peak_f64;
                            let px = center_x + (relative_pos * pixels_per_peak) as f32;

                            // Clip to canvas bounds (with small margin for line continuity)
                            if px >= x - 5.0 && px <= x + width + 5.0 {
                                let (min, max) = sample_peak_smoothed(peaks, peak_idx, smooth_radius, stem_idx);

                                // Apply LUFS gain correction
                                let gain = if linked_active[stem_idx] {
                                    linked_lufs_gains[stem_idx]
                                } else {
                                    host_lufs_gain
                                };
                                let (min, max) = (min * gain, max * gain);

                                let y_max = center_y - (max * height_scale);
                                let y_min = center_y - (min * height_scale);

                                upper_points.push((px.max(x).min(x + width), y_max));
                                lower_points.push((px.max(x).min(x + width), y_min));
                            }

                            peak_idx += step;
                        }
                    } else {
                        // Fallback: cached peaks - use old pixel-based iteration
                        let width_usize = width as usize;
                        let total_samples = window.total_samples as usize;
                        let step = zoomed_step(stem_idx, width_usize);
                        let smooth_radius = smooth_radius_for_stem(stem_idx, step);

                        let mut px = 0;
                        while px < width_usize {
                            let window_offset = px * total_samples / width_usize;
                            let actual_sample = window.start as i64 - window.left_padding as i64 + window_offset as i64;

                            let current_px = px;
                            px += step;

                            if actual_sample < 0 || actual_sample >= duration_samples as i64 {
                                continue;
                            }

                            let cache_virtual_offset = actual_sample - zoomed.cache_start as i64 + zoomed.cache_left_padding as i64;
                            if cache_virtual_offset < 0 || cache_virtual_offset as usize >= cache_virtual_total {
                                continue;
                            }
                            let peak_idx = (cache_virtual_offset as usize * peaks_len) / cache_virtual_total;

                            if peak_idx >= peaks_len {
                                continue;
                            }

                            let (min, max) = sample_peak_smoothed(peaks, peak_idx, smooth_radius, stem_idx);

                            // Apply LUFS gain correction
                            let gain = if linked_active[stem_idx] {
                                linked_lufs_gains[stem_idx]
                            } else {
                                host_lufs_gain
                            };
                            let (min, max) = (min * gain, max * gain);

                            let y_max = center_y - (max * height_scale);
                            let y_min = center_y - (min * height_scale);

                            upper_points.push((x + current_px as f32, y_max));
                            lower_points.push((x + current_px as f32, y_min));
                        }
                    }

                    if upper_points.is_empty() {
                        return;
                    }

                    // Draw upper envelope left to right
                    for &(px, py) in upper_points.iter() {
                        if first_point {
                            builder.move_to(Point::new(px, py));
                            first_point = false;
                        } else {
                            builder.line_to(Point::new(px, py));
                        }
                    }

                    // Draw lower envelope right to left
                    for &(px, py) in lower_points.iter().rev() {
                        builder.line_to(Point::new(px, py));
                    }

                    builder.close();
                });

                frame.fill(&path, waveform_color);
            }
        }

        // Draw cue markers (using sample_to_x for correct padding handling)
        for marker in &zoomed.cue_markers {
            let marker_sample = (marker.position * zoomed.duration_samples as f64) as u64;
            if marker_sample >= window.start && marker_sample <= window.end {
                let cue_x = sample_to_x(marker_sample);
                frame.fill_rectangle(
                    Point::new(cue_x - 1.0, y),
                    Size::new(2.0, height),
                    marker.color,
                );
                let triangle = Path::new(|builder| {
                    builder.move_to(Point::new(cue_x, y));
                    builder.line_to(Point::new(cue_x - 4.0, y + 8.0));
                    builder.line_to(Point::new(cue_x + 4.0, y + 8.0));
                    builder.close();
                });
                frame.fill(&triangle, marker.color);
            }
        }

        // Draw drop marker (using sample_to_x for correct padding handling)
        if let Some(drop_sample) = zoomed.drop_marker {
            if drop_sample >= window.start && drop_sample <= window.end {
                let drop_x = sample_to_x(drop_sample);
                frame.fill_rectangle(
                    Point::new(drop_x - 1.0, y),
                    Size::new(2.0, height),
                    DROP_MARKER_COLOR,
                );
                let diamond = Path::new(|builder| {
                    builder.move_to(Point::new(drop_x, y));              // Top point
                    builder.line_to(Point::new(drop_x - 6.0, y + 8.0));  // Left point
                    builder.line_to(Point::new(drop_x, y + 16.0));       // Bottom point
                    builder.line_to(Point::new(drop_x + 6.0, y + 8.0));  // Right point
                    builder.close();
                });
                frame.fill(&diamond, DROP_MARKER_COLOR);
            }
        }
    }

    // Draw playhead - position depends on view mode
    let playhead_x = match zoomed.view_mode() {
        ZoomedViewMode::Scrolling => {
            // Scrolling mode: playhead fixed at center
            x + width / 2.0
        }
        ZoomedViewMode::FixedBuffer => {
            // Fixed buffer mode: playhead moves within view (no padding in this mode)
            if window.total_samples > 0 && playhead >= window.start && playhead <= window.end {
                let offset = (playhead - window.start) as f64;
                x + (offset / window.total_samples as f64 * width as f64) as f32
            } else {
                // Playhead outside view - clamp to edges
                if playhead < window.start {
                    x
                } else {
                    x + width
                }
            }
        }
    };
    frame.stroke(
        &Path::line(Point::new(playhead_x, y), Point::new(playhead_x, y + height)),
        Stroke::default()
            .with_color(Color::from_rgb(1.0, 1.0, 1.0))
            .with_width(2.0),
    );

    // Draw zoom indicator - vertical bar on right edge
    let indicator_height = (zoomed.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * height;
    let indicator_width = 4.0;
    frame.fill_rectangle(
        Point::new(x + width - indicator_width, y + height - indicator_height),
        Size::new(indicator_width, indicator_height),
        Color::from_rgba(1.0, 1.0, 1.0, 0.5),
    );

    // Master indicator removed - deck border color indicates master status instead
    let _ = is_master;
}

/// Draw an overview waveform at a specific position
///
/// When linked stems exist, renders as split-view:
/// - Top half: Currently running stems (host or linked depending on toggle state)
/// - Bottom half: Non-running alternative stems (with drop marker alignment)
fn draw_overview_at(
    frame: &mut Frame,
    overview: &OverviewState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    stem_colors: &[Color; 4],
    stem_active: &[bool; 4],
    linked_stems: &[bool; 4],
    linked_active: &[bool; 4],
) {
    let height = WAVEFORM_HEIGHT;

    // Check if any linked stems exist to determine split-view mode
    let any_linked = linked_stems.iter().any(|&has| has);
    // center_y is always at the middle - in split mode, top and bottom envelopes meet here
    let center_y = y + height / 2.0;

    // Background
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, height),
        Color::from_rgb(0.05, 0.05, 0.08),
    );

    if !overview.has_track || overview.duration_samples == 0 {
        return;
    }

    // Draw loop region
    if let Some((loop_start, loop_end)) = overview.loop_region {
        let start_x = x + (loop_start * width as f64) as f32;
        let end_x = x + (loop_end * width as f64) as f32;
        let loop_width = end_x - start_x;
        if loop_width > 0.0 {
            frame.fill_rectangle(
                Point::new(start_x, y),
                Size::new(loop_width, height),
                Color::from_rgba(0.2, 0.8, 0.2, 0.25),
            );
            frame.stroke(
                &Path::line(Point::new(start_x, y), Point::new(start_x, y + height)),
                Stroke::default()
                    .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                    .with_width(2.0),
            );
            frame.stroke(
                &Path::line(Point::new(end_x, y), Point::new(end_x, y + height)),
                Stroke::default()
                    .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                    .with_width(2.0),
            );
        }
    }

    // Draw slicer region (semi-transparent orange overlay with slice divisions)
    if let Some((slicer_start, slicer_end)) = overview.slicer_region {
        super::super::slicer_overlay::draw_slicer_overlay(
            frame,
            slicer_start,
            slicer_end,
            overview.slicer_current_slice,
            x,
            y,
            width,
            height,
        );
    }

    // Draw stem waveforms - split view when linked stems exist
    if any_linked {
        // --- SPLIT VIEW MODE ---
        let shared_center_y = y + height / 2.0;
        let half_height_scale = (height / 2.0) * 0.85;

        // Draw stems in layered order
        for &stem_idx in STEM_RENDER_ORDER.iter() {
            let has_link = linked_stems[stem_idx];
            let is_linked_active = linked_active[stem_idx];

            // Get stem color
            let active_color = if stem_active[stem_idx] {
                let base = stem_colors[stem_idx];
                Color::from_rgba(base.r, base.g, base.b, OVERVIEW_WAVEFORM_ALPHA)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.4)
            };
            let inactive_color = if stem_active[stem_idx] {
                let base = stem_colors[stem_idx];
                Color::from_rgba(base.r, base.g, base.b, 0.3)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.25)
            };

            // --- TOP HALF: Upper envelope of currently running stem ---
            let top_peaks = if is_linked_active && has_link {
                // Linked is active: draw linked on top (with alignment)
                overview.linked_stem_waveforms[stem_idx].as_ref()
            } else {
                // Host is active: draw host on top
                Some(&overview.stem_waveforms[stem_idx])
            };

            if let Some(peaks) = top_peaks {
                if !peaks.is_empty() {
                    if is_linked_active && has_link {
                        // Linked stem on top: peaks are pre-aligned in loader, no offset needed
                        let linked_dur = overview.linked_durations[stem_idx].unwrap_or(overview.duration_samples);
                        let x_offset = 0.0; // Pre-alignment baked into peaks
                        draw_stem_waveform_upper_aligned(
                            frame,
                            peaks,
                            x,
                            x_offset,
                            shared_center_y,
                            half_height_scale,
                            active_color,
                            width,
                            linked_dur,
                            overview.duration_samples,
                            stem_idx,
                        );
                    } else {
                        // Host stem on top: draw upper envelope only
                        draw_stem_waveform_upper(frame, peaks, x, shared_center_y, half_height_scale, active_color, width, stem_idx);
                    }
                }
            }

            // --- BOTTOM HALF: Lower envelope of non-running alternative ---
            if has_link {
                let bottom_peaks = if is_linked_active {
                    // Linked is active: host goes to bottom
                    Some(&overview.stem_waveforms[stem_idx])
                } else {
                    // Host is active: linked goes to bottom (with alignment)
                    overview.linked_stem_waveforms[stem_idx].as_ref()
                };

                if let Some(peaks) = bottom_peaks {
                    if !peaks.is_empty() {
                        if !is_linked_active {
                            // Linked stem on bottom: peaks are pre-aligned in loader, no offset needed
                            let linked_dur = overview.linked_durations[stem_idx].unwrap_or(overview.duration_samples);
                            let x_offset = 0.0; // Pre-alignment baked into peaks
                            draw_stem_waveform_lower_aligned(
                                frame,
                                peaks,
                                x,
                                x_offset,
                                shared_center_y,
                                half_height_scale,
                                inactive_color,
                                width,
                                linked_dur,
                                overview.duration_samples,
                                stem_idx,
                            );
                        } else {
                            // Host stem on bottom: draw lower envelope only (dimmed)
                            draw_stem_waveform_lower(frame, peaks, x, shared_center_y, half_height_scale, inactive_color, width, stem_idx);
                        }
                    }
                }
            }
        }
    } else {
        // --- SINGLE PANE MODE (no linked stems) ---
        let height_scale = height / 2.0 * 0.85;
        for &stem_idx in STEM_RENDER_ORDER.iter() {
            let stem_peaks = &overview.stem_waveforms[stem_idx];
            if stem_peaks.is_empty() {
                continue;
            }

            let waveform_color = if stem_active[stem_idx] {
                let base_color = stem_colors[stem_idx];
                Color::from_rgba(base_color.r, base_color.g, base_color.b, OVERVIEW_WAVEFORM_ALPHA)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.4)
            };

            draw_stem_waveform_filled(frame, stem_peaks, x, center_y, height_scale, waveform_color, width, stem_idx);
        }
    }

    // Draw beat markers with configurable density (on top of waveforms)
    let step = (overview.grid_bars * 4) as usize;
    for (i, &beat_pos) in overview.beat_markers.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        let beat_x = x + (beat_pos * width as f64) as f32;
        let color = if (i / step) % 4 == 0 {
            Color::from_rgba(1.0, 0.3, 0.3, 0.6)
        } else {
            Color::from_rgba(0.5, 0.5, 0.5, 0.4)
        };
        frame.stroke(
            &Path::line(Point::new(beat_x, y), Point::new(beat_x, y + height)),
            Stroke::default().with_color(color).with_width(1.0),
        );
    }

    // Draw cue markers
    for marker in &overview.cue_markers {
        let cue_x = x + (marker.position * width as f64) as f32;
        frame.fill_rectangle(
            Point::new(cue_x - 1.0, y),
            Size::new(2.0, height),
            marker.color,
        );
        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(cue_x, y));
            builder.line_to(Point::new(cue_x - 4.0, y + 8.0));
            builder.line_to(Point::new(cue_x + 4.0, y + 8.0));
            builder.close();
        });
        frame.fill(&triangle, marker.color);
    }

    // Draw main cue point marker (orange)
    if let Some(cue_pos) = overview.cue_position {
        let cue_x = x + (cue_pos * width as f64) as f32;
        let cue_color = Color::from_rgb(0.6, 0.6, 0.6);
        frame.stroke(
            &Path::line(Point::new(cue_x, y), Point::new(cue_x, y + height)),
            Stroke::default().with_color(cue_color).with_width(2.0),
        );
        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(cue_x, y));
            builder.line_to(Point::new(cue_x - 4.0, y + 6.0));
            builder.line_to(Point::new(cue_x + 4.0, y + 6.0));
            builder.close();
        });
        frame.fill(&triangle, cue_color);
    }

    // Draw playhead
    if overview.duration_samples > 0 {
        let playhead_ratio = playhead as f64 / overview.duration_samples as f64;
        let playhead_x = x + (playhead_ratio * width as f64) as f32;
        frame.stroke(
            &Path::line(Point::new(playhead_x, y), Point::new(playhead_x, y + height)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );
    }
}
