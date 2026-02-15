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

/// Gap between deck cells in the 2x2 grid (horizontal mode)
pub const DECK_GRID_GAP: f32 = 10.0;

/// Gap between zoomed and overview within a deck cell
pub const DECK_INTERNAL_GAP: f32 = 2.0;

/// Total height of one deck cell (header + zoomed + gap + overview)
/// 16 + 120 + 2 + 35 = 173px
pub const DECK_CELL_HEIGHT: f32 =
    DECK_HEADER_HEIGHT + ZOOMED_WAVEFORM_HEIGHT + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;

// =============================================================================
// Vertical Layout Constants
// =============================================================================

/// Width of each overview column in vertical mode
const VERT_OVERVIEW_COL_WIDTH: f32 = 60.0;
/// Gap between overview columns within a pair (e.g. Ov3↔Ov1, Ov2↔Ov4)
const VERT_OVERVIEW_GAP: f32 = 2.0;
/// Gap between the two center overview columns (Ov1↔Ov2) — symmetry axis
const VERT_OVERVIEW_CENTER_GAP: f32 = 10.0;
/// Gap between sections (zoomed columns ↔ overview cluster)
const VERT_SECTION_GAP: f32 = 20.0;
/// Gap between two zoomed columns on the same side
const VERT_PAIR_GAP: f32 = 8.0;
/// Height of compact header above each zoomed column
const VERT_HEADER_HEIGHT: f32 = 32.0;
/// Height of stem indicator row (4 blocks side-by-side)
const VERT_STEM_INDICATOR_HEIGHT: f32 = 8.0;
/// Gap between stem indicator blocks
const VERT_STEM_INDICATOR_GAP: f32 = 2.0;

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
    /// Apply inverse BPM transform to a click position in the overview.
    ///
    /// When BPM alignment is active, display positions are scaled by `D`
    /// (the overview scale factor). This converts a clicked display position
    /// back to the actual source position for seeking.
    fn inverse_bpm_seek(&self, deck_idx: usize, display_ratio: f64) -> f64 {
        let scales = compute_overview_scales(self.state);
        if let Some(d) = scales[deck_idx] {
            // Inverse: source_pos = display_pos / D
            (display_ratio / d).clamp(0.0, 1.0)
        } else {
            display_ratio
        }
    }

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
        if self.state.is_vertical_layout() {
            return self.update_vertical(interaction, event, bounds, cursor);
        }

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

            // Determine which region within the cell: zoomed, header, or overview
            // Top row: zoomed → header → gap → overview
            // Bottom row (mirrored): overview → gap → header → zoomed
            let mirrored = row == 1;
            let (zoomed_start, zoomed_end, overview_start, overview_end) = if mirrored {
                // overview → gap → header → zoomed
                let overview_start = 0.0_f32;
                let overview_end = WAVEFORM_HEIGHT;
                let zoomed_start = WAVEFORM_HEIGHT + DECK_INTERNAL_GAP + DECK_HEADER_HEIGHT;
                let zoomed_end = zoomed_start + zoomed_height;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            } else {
                // zoomed → header → gap → overview
                let zoomed_start = 0.0_f32;
                let zoomed_end = zoomed_height;
                let overview_start = zoomed_height + DECK_HEADER_HEIGHT + DECK_INTERNAL_GAP;
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
                            // Calculate seek ratio, applying inverse BPM transform if active
                            let display_ratio = (local_x / cell_width).clamp(0.0, 1.0) as f64;
                            let seek_ratio = self.inverse_bpm_seek(deck_idx, display_ratio);
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
                                    let display_ratio = (local_x / cell_width).clamp(0.0, 1.0) as f64;
                                    let seek_ratio = self.inverse_bpm_seek(active_deck, display_ratio);
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
        if self.state.is_vertical_layout() {
            return self.mouse_interaction_vertical(interaction, bounds, cursor);
        }

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
                let zoomed_start = WAVEFORM_HEIGHT + DECK_INTERNAL_GAP + DECK_HEADER_HEIGHT;
                let zoomed_end = zoomed_start + zoomed_height;
                (zoomed_start, zoomed_end, overview_start, overview_end)
            } else {
                let zoomed_start = 0.0_f32;
                let zoomed_end = zoomed_height;
                let overview_start = zoomed_height + DECK_HEADER_HEIGHT + DECK_INTERNAL_GAP;
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
        if self.state.is_vertical_layout() {
            return self.draw_vertical(renderer, bounds);
        }

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

        // Pre-compute BPM-aligned overview scales for all decks.
        // Each D = this_track_display_dur / max_display_dur, ensuring all decks
        // share the same time-per-pixel rate so beat grids align visually.
        let overview_scales = compute_overview_scales(self.state);

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
                overview_scales[deck_idx],
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

/// Draw a complete deck quadrant (zoomed + header + overview)
///
/// Layout (top row):
/// ```text
/// +-------------------------------------+
/// |     Zoomed Waveform          180px |
/// +-------------------------------------+
/// | [N] Track Name Here          48px  | <- Header row (between waveforms)
/// +-------------------------------------+
/// |     Overview Waveform         81px |
/// +-------------------------------------+
/// ```
/// Bottom row is mirrored: overview → header → zoomed
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
    overview_scale: Option<f64>,
    loop_length_beats: Option<f32>,
    loop_active: bool,
    volume: f32,
    mirrored: bool,
) {
    use iced::widget::canvas::Text;
    use iced::alignment::{Horizontal, Vertical};

    // Stem indicator width (positioned on inner edge, towards center gap)
    const STEM_INDICATOR_WIDTH: f32 = 10.0;
    const STEM_INDICATOR_GAP: f32 = 2.0;

    // Compute Y positions based on mirrored layout
    // Header sits between zoomed and overview so overviews cluster in the center
    let (header_y, zoomed_y, overview_y) = if mirrored {
        // Bottom decks: overview → gap → header → zoomed
        let overview_y = y;
        let header_y = y + WAVEFORM_HEIGHT + DECK_INTERNAL_GAP;
        let zoomed_y = header_y + DECK_HEADER_HEIGHT;
        (header_y, zoomed_y, overview_y)
    } else {
        // Top decks: zoomed → header → gap → overview
        let zoomed_y = y;
        let header_y = y + zoomed_height;
        let overview_y = header_y + DECK_HEADER_HEIGHT + DECK_INTERNAL_GAP;
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

    // Draw stem status indicators on inner edge of zoomed waveform (towards center gap)
    let indicator_height = (zoomed_height - (STEM_INDICATOR_GAP * 3.0)) / 4.0;
    // Left column decks (0, 2): indicators on right edge; right column (1, 3): on left edge
    let indicator_x = if deck_idx % 2 == 0 {
        x + width - STEM_INDICATOR_WIDTH - 2.0
    } else {
        x + 2.0
    };

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
            Point::new(indicator_x, indicator_y),
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
        overview_scale,
    );

    // Draw volume dimming overlay over the waveform area (not the header)
    // At full volume (1.0) no dimming, at zero volume max dimming (0.4 alpha)
    if volume < 0.99 {
        let dim_alpha = (1.0 - volume) * 0.4;
        let waveform_area_y = overview_y.min(zoomed_y);
        let waveform_area_height = zoomed_height + DECK_HEADER_HEIGHT + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;
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

/// Pre-stretch a peak buffer for BPM-aligned overview rendering.
///
/// `display_fraction`: fraction of the output width this track occupies (0.0-1.0).
/// Source peaks fill [0, display_fraction] of the output; the rest is silence padding.
/// For each output pixel, inverse-maps back to the source peak position
/// and samples with linear interpolation.
fn stretch_peaks(
    peaks: &[(f32, f32)],
    display_fraction: f64,
    output_len: usize,
) -> Vec<(f32, f32)> {
    let peaks_len = peaks.len();
    if peaks_len == 0 || output_len == 0 || display_fraction <= 0.0 {
        return vec![(0.0, 0.0); output_len];
    }

    (0..output_len)
        .map(|i| {
            let display_pos = i as f64 / output_len as f64;
            // Inverse: source_pos = display_pos / display_fraction
            let source_pos = display_pos / display_fraction;

            if source_pos < 0.0 || source_pos >= 1.0 {
                return (0.0, 0.0); // Out of range → silence padding
            }

            // Map to peak index with linear interpolation
            let source_idx_f = source_pos * peaks_len as f64;
            let idx0 = source_idx_f.floor() as usize;
            let idx1 = (idx0 + 1).min(peaks_len - 1);
            let frac = (source_idx_f - idx0 as f64) as f32;

            let (min0, max0) = peaks[idx0];
            let (min1, max1) = peaks[idx1];

            (
                min0 + (min1 - min0) * frac,
                max0 + (max1 - max0) * frac,
            )
        })
        .collect()
}

/// Compute overview BPM-alignment scale for all 4 decks.
///
/// Returns `D` for each deck: the fraction of display width this track occupies
/// when all decks share a common time axis at the global BPM. `None` if no
/// transform is needed (single track, no BPM data, or D ≈ 1.0).
///
/// Formula: `D = (track_dur × track_bpm) / (display_bpm × max_display_dur)`
/// where `max_display_dur = max(track_dur_i × track_bpm_i / display_bpm)` across all decks.
fn compute_overview_scales(state: &PlayerCanvasState) -> [Option<f64>; 4] {
    // All decks share the same display_bpm (global BPM)
    let display_bpm = match state.display_bpm(0) {
        Some(dbpm) if dbpm > 0.0 => dbpm,
        _ => return [None; 4],
    };

    // Compute display duration for each deck and find the maximum
    let mut display_durs = [0.0f64; 4];
    let mut max_dur = 0.0f64;
    let mut count = 0usize;

    for i in 0..4 {
        let overview = &state.decks[i].overview;
        if overview.has_track && overview.duration_samples > 0 {
            if let Some(tbpm) = state.track_bpm(i) {
                if tbpm > 0.0 {
                    let dur_secs = overview.duration_samples as f64 / SAMPLE_RATE as f64;
                    let display_dur = dur_secs * tbpm / display_bpm;
                    display_durs[i] = display_dur;
                    max_dur = max_dur.max(display_dur);
                    count += 1;
                }
            }
        }
    }

    if count == 0 || max_dur <= 0.0 {
        return [None; 4];
    }

    let mut scales = [None; 4];
    for i in 0..4 {
        if display_durs[i] > 0.0 {
            let d = display_durs[i] / max_dur;
            // Only apply transform if D differs meaningfully from 1.0
            // (single track or same-length same-BPM tracks don't need it)
            if count > 1 || (d - 1.0).abs() > 0.001 {
                scales[i] = Some(d);
            }
        }
    }
    scales
}

/// Draw an overview waveform at a specific position
///
/// When linked stems exist, renders as split-view:
/// - Top half: Currently running stems (host or linked depending on toggle state)
/// - Bottom half: Non-running alternative stems (with drop marker alignment)
///
/// `overview_scale`: When Some(D), stretches overview so this track fills fraction D
/// of the display width, with silence padding for the rest. All decks share a common
/// time axis so beat grids align visually.
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
    overview_scale: Option<f64>,
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

    // Helper: convert normalized source position to pixel X.
    // When overview_scale is active, display_pos = source_pos * D (uniform scaling).
    let pos_to_x = |pos: f64| -> f32 {
        let display_pos = if let Some(d) = overview_scale { pos * d } else { pos };
        x + (display_pos * width as f64) as f32
    };

    // Helper: check if a transformed position is within visible bounds
    let pos_visible = |pos: f64| -> bool {
        let display_pos = if let Some(d) = overview_scale { pos * d } else { pos };
        display_pos >= -0.01 && display_pos <= 1.01
    };

    // Draw loop region
    if let Some((loop_start, loop_end)) = overview.loop_region {
        let start_x = pos_to_x(loop_start).max(x);
        let end_x = pos_to_x(loop_end).min(x + width);
        let loop_width = end_x - start_x;
        if loop_width > 0.0 {
            frame.fill_rectangle(
                Point::new(start_x, y),
                Size::new(loop_width, height),
                Color::from_rgba(0.2, 0.8, 0.2, 0.25),
            );
            if start_x >= x && start_x <= x + width {
                frame.stroke(
                    &Path::line(Point::new(start_x, y), Point::new(start_x, y + height)),
                    Stroke::default()
                        .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                        .with_width(2.0),
                );
            }
            if end_x >= x && end_x <= x + width {
                frame.stroke(
                    &Path::line(Point::new(end_x, y), Point::new(end_x, y + height)),
                    Stroke::default()
                        .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                        .with_width(2.0),
                );
            }
        }
    }

    // Draw slicer region (semi-transparent orange overlay with slice divisions)
    if let Some((slicer_start, slicer_end)) = overview.slicer_region {
        if let Some(d) = overview_scale {
            // Transform slicer bounds for BPM-aligned display
            let ts_norm = (slicer_start * d).clamp(0.0, 1.0);
            let te_norm = (slicer_end * d).clamp(0.0, 1.0);
            super::super::slicer_overlay::draw_slicer_overlay(
                frame, ts_norm, te_norm, overview.slicer_current_slice,
                x, y, width, height,
            );
        } else {
            super::super::slicer_overlay::draw_slicer_overlay(
                frame, slicer_start, slicer_end, overview.slicer_current_slice,
                x, y, width, height,
            );
        }
    }

    // Pre-stretch peaks when overview scale is active (D < 1.0 compresses, D > 1.0 expands)
    // Each output pixel maps back to source_pos = display_pos / D.
    // Positions beyond the track (source_pos >= 1.0) produce silence padding.
    let stretched_waveforms: Option<[Vec<(f32, f32)>; 4]> = overview_scale.map(|d| {
        let out_len = overview.stem_waveforms[0].len().max(width as usize);
        [
            stretch_peaks(&overview.stem_waveforms[0], d, out_len),
            stretch_peaks(&overview.stem_waveforms[1], d, out_len),
            stretch_peaks(&overview.stem_waveforms[2], d, out_len),
            stretch_peaks(&overview.stem_waveforms[3], d, out_len),
        ]
    });

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
            } else if let Some(ref stretched) = stretched_waveforms {
                // BPM-stretched host peaks
                Some(&stretched[stem_idx])
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
                    // Linked is active: host goes to bottom (use stretched if available)
                    if let Some(ref stretched) = stretched_waveforms {
                        Some(stretched[stem_idx].as_slice())
                    } else {
                        Some(overview.stem_waveforms[stem_idx].as_slice())
                    }
                } else {
                    // Host is active: linked goes to bottom (with alignment)
                    overview.linked_stem_waveforms[stem_idx].as_ref().map(|v| v.as_slice())
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
            // Use stretched peaks if BPM transform is active, otherwise original
            let stem_peaks: &[(f32, f32)] = if let Some(ref stretched) = stretched_waveforms {
                &stretched[stem_idx]
            } else {
                &overview.stem_waveforms[stem_idx]
            };
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
        if !pos_visible(beat_pos) {
            continue;
        }
        let beat_x = pos_to_x(beat_pos).max(x).min(x + width);
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
        if !pos_visible(marker.position) {
            continue;
        }
        let cue_x = pos_to_x(marker.position).max(x).min(x + width);
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
        if pos_visible(cue_pos) {
            let cue_x = pos_to_x(cue_pos).max(x).min(x + width);
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
    }

    // Draw playhead — transformed like all other positions when BPM-aligned
    if overview.duration_samples > 0 {
        let playhead_ratio = playhead as f64 / overview.duration_samples as f64;
        let playhead_x = pos_to_x(playhead_ratio).max(x).min(x + width);
        frame.stroke(
            &Path::line(Point::new(playhead_x, y), Point::new(playhead_x, y + height)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );
    }
}

// =============================================================================
// Vertical Layout: Geometry
// =============================================================================

/// Compute vertical layout geometry from canvas bounds.
///
/// Returns (center_x, center_width, zoomed_col_width, side_width) where:
/// - center_x: X offset where overview cluster begins
/// - center_width: total width of the 4 overview columns + gaps
/// - zoomed_col_width: width of each zoomed waveform column
/// - side_width: total width of each side (left or right pair)
fn vert_geometry(bounds_width: f32) -> (f32, f32, f32, f32) {
    // 4 overview columns: [Ov3 gap Ov1] CENTER_GAP [Ov2 gap Ov4]
    let center_width = 4.0 * VERT_OVERVIEW_COL_WIDTH
        + 2.0 * VERT_OVERVIEW_GAP        // Two within-pair gaps (Ov3↔Ov1, Ov2↔Ov4)
        + VERT_OVERVIEW_CENTER_GAP;       // One center gap (Ov1↔Ov2)
    let remaining = bounds_width - center_width - 2.0 * VERT_SECTION_GAP;
    let side_width = remaining / 2.0;
    let zoomed_col_width = (side_width - VERT_PAIR_GAP) / 2.0;
    let center_x = side_width + VERT_SECTION_GAP;
    (center_x, center_width, zoomed_col_width, side_width)
}

/// Compute X positions for each vertical column.
///
/// Returns array of 8 X positions:
/// [Zoom3, Zoom1, Ov3, Ov1, Ov2, Ov4, Zoom2, Zoom4]
fn vert_column_positions(center_x: f32, center_width: f32, zoomed_col_width: f32) -> [f32; 8] {
    let right_start = center_x + center_width + VERT_SECTION_GAP;

    // Left pair: Ov3, then gap, then Ov1
    let ov3_x = center_x;
    let ov1_x = ov3_x + VERT_OVERVIEW_COL_WIDTH + VERT_OVERVIEW_GAP;
    // Center gap between Ov1 and Ov2
    let ov2_x = ov1_x + VERT_OVERVIEW_COL_WIDTH + VERT_OVERVIEW_CENTER_GAP;
    // Right pair: Ov2, then gap, then Ov4
    let ov4_x = ov2_x + VERT_OVERVIEW_COL_WIDTH + VERT_OVERVIEW_GAP;

    [
        0.0,                                     // [0] Deck 3 zoomed (left outer)
        zoomed_col_width + VERT_PAIR_GAP,       // [1] Deck 1 zoomed (left inner)
        ov3_x,                                   // [2] Overview 3
        ov1_x,                                   // [3] Overview 1
        ov2_x,                                   // [4] Overview 2
        ov4_x,                                   // [5] Overview 4
        right_start,                             // [6] Deck 2 zoomed (right inner)
        right_start + zoomed_col_width + VERT_PAIR_GAP, // [7] Deck 4 zoomed (right outer)
    ]
}

/// Determine which deck (and whether zoomed or overview) from a cursor X position.
///
/// Returns (deck_idx, is_overview). None if in a gap area.
fn vert_hit_test(
    cursor_x: f32,
    cols: &[f32; 8],
    zoomed_col_width: f32,
) -> Option<(usize, bool)> {
    // Zoomed columns: [0]=deck3, [1]=deck1, [6]=deck2, [7]=deck4
    let zoomed_decks = [(0, 2), (1, 0), (6, 1), (7, 3)];
    for &(col_idx, deck_idx) in &zoomed_decks {
        if cursor_x >= cols[col_idx] && cursor_x < cols[col_idx] + zoomed_col_width {
            return Some((deck_idx, false));
        }
    }
    // Overview columns: [2]=deck3, [3]=deck1, [4]=deck2, [5]=deck4
    let overview_decks = [(2, 2), (3, 0), (4, 1), (5, 3)];
    for &(col_idx, deck_idx) in &overview_decks {
        if cursor_x >= cols[col_idx] && cursor_x < cols[col_idx] + VERT_OVERVIEW_COL_WIDTH {
            return Some((deck_idx, true));
        }
    }
    None
}

// =============================================================================
// Vertical Layout: Methods on PlayerCanvas
// =============================================================================

impl<'a, Message, SeekFn, ZoomFn> PlayerCanvas<'a, Message, SeekFn, ZoomFn>
where
    Message: Clone,
    SeekFn: Fn(usize, f64) -> Message,
    ZoomFn: Fn(usize, u32) -> Message,
{
    /// Draw the vertical layout (time flows top-to-bottom).
    fn draw_vertical(
        &self,
        renderer: &iced::Renderer,
        bounds: Rectangle,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let inverted = self.state.is_vertical_inverted();
        let (center_x, center_width, zoomed_col_width, _side_width) = vert_geometry(bounds.width);
        let cols = vert_column_positions(center_x, center_width, zoomed_col_width);

        let zoomed_y = VERT_HEADER_HEIGHT + VERT_STEM_INDICATOR_HEIGHT + VERT_STEM_INDICATOR_GAP;
        let zoomed_height = bounds.height - zoomed_y;

        let overview_scales = compute_overview_scales(self.state);

        // Deck ordering: left side has decks 3,1; right side has decks 2,4
        // Column indices: [0]=Zoom3, [1]=Zoom1, [6]=Zoom2, [7]=Zoom4
        let zoomed_columns = [
            (0, 2), // col[0] = deck 3
            (1, 0), // col[1] = deck 1
            (6, 1), // col[6] = deck 2
            (7, 3), // col[7] = deck 4
        ];

        for &(col_idx, deck_idx) in &zoomed_columns {
            let col_x = cols[col_idx];
            let playhead = self.state.interpolated_playhead(deck_idx, SAMPLE_RATE);
            let stem_active = self.state.stem_active(deck_idx);
            let stem_colors = self.state.stem_colors();
            let (_linked_stems, linked_active) = self.state.linked_stems(deck_idx);

            // Draw compact header
            draw_vertical_header(
                &mut frame,
                col_x,
                0.0,
                zoomed_col_width,
                deck_idx,
                self.state.track_name(deck_idx),
                self.state.track_key(deck_idx),
                self.state.track_bpm(deck_idx),
                self.state.is_master(deck_idx),
                self.state.cue_enabled(deck_idx),
                self.state.decks[deck_idx].zoomed.has_track,
            );

            // Draw stem indicators (horizontal bars below header)
            draw_vertical_stem_indicators(
                &mut frame,
                col_x,
                VERT_HEADER_HEIGHT,
                zoomed_col_width,
                stem_active,
                stem_colors,
            );

            // Draw vertical zoomed waveform
            draw_vertical_zoomed(
                &mut frame,
                &self.state.decks[deck_idx].zoomed,
                &self.state.decks[deck_idx].overview.highres_peaks,
                &self.state.decks[deck_idx].overview.linked_highres_peaks,
                self.state.decks[deck_idx].zoomed.lufs_gain,
                &self.state.decks[deck_idx].overview.linked_lufs_gains,
                self.state.decks[deck_idx].overview.duration_samples,
                playhead,
                col_x,
                zoomed_y,
                zoomed_col_width,
                zoomed_height,
                stem_colors,
                stem_active,
                linked_active,
                inverted,
            );

            // Volume dimming overlay
            let volume = self.state.volume(deck_idx);
            if volume < 0.99 {
                let dim_alpha = (1.0 - volume) * 0.4;
                frame.fill_rectangle(
                    Point::new(col_x, 0.0),
                    Size::new(zoomed_col_width, bounds.height),
                    Color::from_rgba(0.0, 0.0, 0.0, dim_alpha),
                );
            }
        }

        // Overview columns: [2]=deck3, [3]=deck1, [4]=deck2, [5]=deck4
        let overview_columns = [
            (2, 2), // col[2] = deck 3
            (3, 0), // col[3] = deck 1
            (4, 1), // col[4] = deck 2
            (5, 3), // col[5] = deck 4
        ];

        for &(col_idx, deck_idx) in &overview_columns {
            let col_x = cols[col_idx];
            let playhead = self.state.interpolated_playhead(deck_idx, SAMPLE_RATE);
            let stem_active = self.state.stem_active(deck_idx);
            let stem_colors = self.state.stem_colors();
            let (linked_stems, linked_active) = self.state.linked_stems(deck_idx);

            draw_vertical_overview(
                &mut frame,
                &self.state.decks[deck_idx].overview,
                playhead,
                col_x,
                0.0,
                VERT_OVERVIEW_COL_WIDTH,
                bounds.height,
                stem_colors,
                stem_active,
                linked_stems,
                linked_active,
                overview_scales[deck_idx],
                inverted,
            );

            // Volume dimming on overview too
            let volume = self.state.volume(deck_idx);
            if volume < 0.99 {
                let dim_alpha = (1.0 - volume) * 0.4;
                frame.fill_rectangle(
                    Point::new(col_x, 0.0),
                    Size::new(VERT_OVERVIEW_COL_WIDTH, bounds.height),
                    Color::from_rgba(0.0, 0.0, 0.0, dim_alpha),
                );
            }
        }

        vec![frame.into_geometry()]
    }

    /// Handle mouse interaction in vertical layout.
    fn update_vertical(
        &self,
        interaction: &mut PlayerInteraction,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let inverted = self.state.is_vertical_inverted();
        let (center_x, center_width, zoomed_col_width, _side_width) = vert_geometry(bounds.width);
        let cols = vert_column_positions(center_x, center_width, zoomed_col_width);

        let zoomed_y = VERT_HEADER_HEIGHT + VERT_STEM_INDICATOR_HEIGHT + VERT_STEM_INDICATOR_GAP;

        if let Some(position) = cursor.position_in(bounds) {
            if let Some((deck_idx, is_overview)) = vert_hit_test(position.x, &cols, zoomed_col_width) {
                if is_overview {
                    // Overview column: click/drag to seek (Y = track position)
                    match event {
                        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                            interaction.active_deck = Some(deck_idx);
                            interaction.is_seeking = true;
                            interaction.drag_start_y = None;

                            let overview = &self.state.decks[deck_idx].overview;
                            if overview.has_track && overview.duration_samples > 0 {
                                let raw_ratio = (position.y / bounds.height).clamp(0.0, 1.0) as f64;
                                let display_ratio = if inverted { 1.0 - raw_ratio } else { raw_ratio };
                                let seek_ratio = self.inverse_bpm_seek(deck_idx, display_ratio);
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
                                        let raw_ratio = (position.y / bounds.height).clamp(0.0, 1.0) as f64;
                                        let display_ratio = if inverted { 1.0 - raw_ratio } else { raw_ratio };
                                        let seek_ratio = self.inverse_bpm_seek(active_deck, display_ratio);
                                        return Some(canvas::Action::publish((self.on_seek)(active_deck, seek_ratio)));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                } else if position.y >= zoomed_y {
                    // Zoomed column: drag vertically to zoom
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
                                // Horizontal drag to zoom (drag right = zoom in, left = zoom out)
                                let delta = position.x - start_y; // Reusing drag_start_y for simplicity
                                let _ = delta;
                                // Use vertical drag for consistency with horizontal mode
                                let delta_v = start_y - position.y;
                                let zoom_change = (delta_v / ZOOM_PIXELS_PER_LEVEL) as i32;
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

    /// Mouse cursor icon in vertical layout.
    fn mouse_interaction_vertical(
        &self,
        interaction: &PlayerInteraction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let (center_x, center_width, zoomed_col_width, _side_width) = vert_geometry(bounds.width);
        let cols = vert_column_positions(center_x, center_width, zoomed_col_width);

        let zoomed_y = VERT_HEADER_HEIGHT + VERT_STEM_INDICATOR_HEIGHT + VERT_STEM_INDICATOR_GAP;

        if let Some(position) = cursor.position_in(bounds) {
            if let Some((_deck_idx, is_overview)) = vert_hit_test(position.x, &cols, zoomed_col_width) {
                if is_overview {
                    return mouse::Interaction::Pointer;
                } else if position.y >= zoomed_y {
                    if interaction.drag_start_y.is_some() {
                        return mouse::Interaction::ResizingVertically;
                    }
                    return mouse::Interaction::Grab;
                }
            }
        }
        mouse::Interaction::default()
    }
}

// =============================================================================
// Vertical Layout: Drawing Functions
// =============================================================================

/// Draw a compact header above a vertical zoomed column.
fn draw_vertical_header(
    frame: &mut Frame,
    x: f32,
    y: f32,
    width: f32,
    deck_idx: usize,
    track_name: &str,
    track_key: &str,
    track_bpm: Option<f64>,
    is_master: bool,
    cue_enabled: bool,
    has_track: bool,
) {
    use iced::widget::canvas::Text;
    use iced::alignment::{Horizontal, Vertical};

    // Background
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, VERT_HEADER_HEIGHT),
        Color::from_rgb(0.10, 0.10, 0.12),
    );

    // Deck number badge (compact)
    let badge_size = VERT_HEADER_HEIGHT - 8.0;
    let badge_x = x + 4.0;
    let badge_y = y + 4.0;

    let badge_bg = if cue_enabled {
        Color::from_rgb(0.35, 0.30, 0.10)
    } else if has_track {
        Color::from_rgb(0.15, 0.15, 0.25)
    } else {
        Color::from_rgb(0.15, 0.15, 0.15)
    };

    frame.fill_rectangle(
        Point::new(badge_x, badge_y),
        Size::new(badge_size, badge_size),
        badge_bg,
    );

    if is_master {
        let stroke = Stroke::default()
            .with_width(2.0)
            .with_color(Color::from_rgb(0.45, 0.8, 0.55));
        frame.stroke(
            &Path::rectangle(Point::new(badge_x, badge_y), Size::new(badge_size, badge_size)),
            stroke,
        );
    }

    let text_color = if cue_enabled {
        Color::from_rgb(1.0, 0.85, 0.3)
    } else if has_track {
        Color::from_rgb(0.7, 0.7, 0.9)
    } else {
        Color::from_rgb(0.5, 0.5, 0.5)
    };

    frame.fill_text(Text {
        content: format!("{}", deck_idx + 1),
        position: Point::new(badge_x + badge_size / 2.0, y + VERT_HEADER_HEIGHT / 2.0),
        size: 16.0.into(),
        color: text_color,
        align_x: Horizontal::Center.into(),
        align_y: Vertical::Center.into(),
        ..Text::default()
    });

    // Track name (truncated to fit)
    let name_x = badge_x + badge_size + 4.0;
    let available = width - badge_size - 12.0;

    if has_track && !track_name.is_empty() {
        let max_chars = (available / 8.0) as usize;
        let display = if track_name.len() > max_chars && max_chars > 3 {
            format!("{}...", &track_name[..max_chars.min(track_name.len()) - 3])
        } else {
            track_name.to_string()
        };

        frame.fill_text(Text {
            content: display,
            position: Point::new(name_x, y + VERT_HEADER_HEIGHT / 2.0 - 4.0),
            size: 11.0.into(),
            color: Color::from_rgb(0.75, 0.75, 0.75),
            align_x: Horizontal::Left.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });

        // Key + BPM on second line
        let mut info_parts = Vec::new();
        if !track_key.is_empty() {
            info_parts.push(track_key.to_string());
        }
        if let Some(bpm) = track_bpm {
            info_parts.push(format!("{:.0}", bpm));
        }
        if !info_parts.is_empty() {
            frame.fill_text(Text {
                content: info_parts.join(" | "),
                position: Point::new(name_x, y + VERT_HEADER_HEIGHT / 2.0 + 7.0),
                size: 9.0.into(),
                color: Color::from_rgb(0.55, 0.55, 0.65),
                align_x: Horizontal::Left.into(),
                align_y: Vertical::Center.into(),
                ..Text::default()
            });
        }
    }
}

/// Draw stem indicator blocks in a horizontal row below header (above zoomed waveform).
///
/// Rotated 90° from horizontal mode: 4 small blocks side-by-side instead of stacked.
fn draw_vertical_stem_indicators(
    frame: &mut Frame,
    x: f32,
    y: f32,
    width: f32,
    stem_active: &[bool; 4],
    stem_colors: &[Color; 4],
) {
    let total_gaps = 3.0 * VERT_STEM_INDICATOR_GAP;
    let block_width = (width - total_gaps) / 4.0;

    for (visual_idx, &stem_idx) in STEM_INDICATOR_ORDER.iter().enumerate() {
        let block_x = x + (visual_idx as f32) * (block_width + VERT_STEM_INDICATOR_GAP);
        let color = stem_colors[stem_idx];

        let indicator_color = if stem_active[stem_idx] {
            Color::from_rgb(color.r * 0.5, color.g * 0.5, color.b * 0.5)
        } else {
            Color::from_rgb(0.12, 0.12, 0.12)
        };

        frame.fill_rectangle(
            Point::new(block_x, y),
            Size::new(block_width, VERT_STEM_INDICATOR_HEIGHT),
            indicator_color,
        );
    }
}

/// Draw a vertical zoomed waveform (time flows top-to-bottom).
///
/// The center line runs vertically at `x + width/2`. Peaks extend left and right.
/// The playhead is a horizontal line. Beat markers are horizontal lines.
fn draw_vertical_zoomed(
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
    stem_colors: &[Color; 4],
    stem_active: &[bool; 4],
    linked_active: &[bool; 4],
    inverted: bool,
) {
    let center_x = x + width / 2.0;

    // Background
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, height),
        Color::from_rgb(0.08, 0.08, 0.1),
    );

    if !zoomed.has_track || zoomed.duration_samples == 0 {
        return;
    }

    let window = zoomed.visible_window(playhead);
    if window.total_samples == 0 {
        return;
    }

    // Helper: sample position → Y coordinate (instead of X in horizontal mode)
    // When inverted, time flows bottom-to-top: earlier samples at bottom, later at top
    let sample_to_y = |sample: u64| -> f32 {
        let frac = if sample < window.start {
            window.left_padding as f64 / window.total_samples as f64
        } else if sample > window.end {
            1.0
        } else {
            let offset = window.left_padding + (sample - window.start);
            offset as f64 / window.total_samples as f64
        };
        if inverted {
            y + height - (frac * height as f64) as f32
        } else {
            y + (frac * height as f64) as f32
        }
    };

    // Draw loop region (horizontal band)
    if let Some((loop_start_norm, loop_end_norm)) = zoomed.loop_region {
        let loop_start_sample = (loop_start_norm * zoomed.duration_samples as f64) as u64;
        let loop_end_sample = (loop_end_norm * zoomed.duration_samples as f64) as u64;

        if loop_end_sample > window.start && loop_start_sample < window.end {
            let y1 = sample_to_y(loop_start_sample.max(window.start));
            let y2 = sample_to_y(loop_end_sample.min(window.end));
            let start_y = y1.min(y2);
            let end_y = y1.max(y2);
            let loop_h = end_y - start_y;
            if loop_h > 0.0 {
                frame.fill_rectangle(
                    Point::new(x, start_y),
                    Size::new(width, loop_h),
                    Color::from_rgba(0.2, 0.8, 0.2, 0.25),
                );
                if loop_start_sample >= window.start && loop_start_sample <= window.end {
                    let ly = sample_to_y(loop_start_sample);
                    frame.stroke(
                        &Path::line(Point::new(x, ly), Point::new(x + width, ly)),
                        Stroke::default().with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8)).with_width(2.0),
                    );
                }
                if loop_end_sample >= window.start && loop_end_sample <= window.end {
                    let ly = sample_to_y(loop_end_sample);
                    frame.stroke(
                        &Path::line(Point::new(x, ly), Point::new(x + width, ly)),
                        Stroke::default().with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8)).with_width(2.0),
                    );
                }
            }
        }
    }

    // Draw slicer region (horizontal band)
    let slicer_bounds: Option<(u64, u64)> = zoomed.fixed_buffer_bounds.or_else(|| {
        zoomed.slicer_region.map(|(s, e)| {
            ((s * zoomed.duration_samples as f64) as u64, (e * zoomed.duration_samples as f64) as u64)
        })
    });

    if let Some((slicer_start, slicer_end)) = slicer_bounds {
        if slicer_end > window.start && slicer_start < window.end {
            let sy_raw = sample_to_y(slicer_start.max(window.start));
            let ey_raw = sample_to_y(slicer_end.min(window.end));
            let sy = sy_raw.min(ey_raw);
            let ey = sy_raw.max(ey_raw);
            let sh = ey - sy;
            if sh > 0.0 {
                frame.fill_rectangle(
                    Point::new(x, sy),
                    Size::new(width, sh),
                    Color::from_rgba(1.0, 0.5, 0.0, 0.12),
                );

                let samples_per_slice = (slicer_end - slicer_start) / SLICER_NUM_SLICES as u64;
                for i in 0..=SLICER_NUM_SLICES {
                    let slice_sample = slicer_start + samples_per_slice * i as u64;
                    if slice_sample >= window.start && slice_sample <= window.end {
                        let slice_y = sample_to_y(slice_sample);
                        let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
                        let lw = if is_boundary { 2.0 } else { 1.0 };
                        let alpha = if is_boundary { 0.8 } else { 0.5 };
                        frame.stroke(
                            &Path::line(Point::new(x, slice_y), Point::new(x + width, slice_y)),
                            Stroke::default().with_color(Color::from_rgba(1.0, 0.6, 0.1, alpha)).with_width(lw),
                        );
                    }
                }

                if let Some(current) = zoomed.slicer_current_slice {
                    let ss = slicer_start + samples_per_slice * current as u64;
                    let se = ss + samples_per_slice;
                    if se > window.start && ss < window.end {
                        let ssy = sample_to_y(ss.max(window.start));
                        let sey = sample_to_y(se.min(window.end));
                        let top = ssy.min(sey);
                        let bot = ssy.max(sey);
                        frame.fill_rectangle(
                            Point::new(x, top),
                            Size::new(width, bot - top),
                            Color::from_rgba(1.0, 0.6, 0.0, 0.2),
                        );
                    }
                }
            }
        }
    }

    // Draw beat markers (horizontal lines)
    for (i, &beat_sample) in zoomed.beat_grid.iter().enumerate() {
        if beat_sample >= window.start && beat_sample <= window.end {
            let beat_y = sample_to_y(beat_sample);
            let (color, w) = if i % 4 == 0 {
                (Color::from_rgba(1.0, 0.3, 0.3, 0.6), 2.0)
            } else {
                (Color::from_rgba(0.5, 0.5, 0.5, 0.4), 1.0)
            };
            frame.stroke(
                &Path::line(Point::new(x, beat_y), Point::new(x + width, beat_y)),
                Stroke::default().with_color(color).with_width(w),
            );
        }
    }

    // Draw peaks (axis-swapped: center line vertical, peaks extend left/right)
    let use_highres = !highres_peaks[0].is_empty() && duration_samples > 0;
    let use_cached = !use_highres
        && !zoomed.cached_peaks[0].is_empty()
        && (zoomed.cache_end > zoomed.cache_start || zoomed.cache_left_padding > 0);

    if use_highres || use_cached {
        let width_scale = width / 2.0 * 0.85;

        let cache_virtual_total = if use_cached {
            (zoomed.cache_end - zoomed.cache_start + zoomed.cache_left_padding) as usize
        } else {
            0
        };

        for &stem_idx in STEM_RENDER_ORDER.iter() {
            let peaks: &[(f32, f32)] = if linked_active[stem_idx] {
                if use_highres {
                    linked_highres_peaks[stem_idx]
                        .as_ref()
                        .map(|v| v.as_slice())
                        .unwrap_or(&highres_peaks[stem_idx])
                } else {
                    zoomed.linked_cached_peaks[stem_idx]
                        .as_ref()
                        .map(|v| v.as_slice())
                        .unwrap_or(&zoomed.cached_peaks[stem_idx])
                }
            } else if use_highres {
                &highres_peaks[stem_idx]
            } else {
                &zoomed.cached_peaks[stem_idx]
            };

            if peaks.is_empty() {
                continue;
            }
            let peaks_len = peaks.len();

            let waveform_color = if stem_active[stem_idx] {
                let base = stem_colors[stem_idx];
                Color::from_rgba(base.r, base.g, base.b, ZOOMED_WAVEFORM_ALPHA)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.5)
            };

            // Build filled path: left envelope top→bottom, right envelope bottom→top
            let path = Path::new(|builder| {
                let mut first_point = true;
                let mut left_points: Vec<(f32, f32)> = Vec::with_capacity(512);
                let mut right_points: Vec<(f32, f32)> = Vec::with_capacity(512);

                if use_highres {
                    let samples_per_peak = (duration_samples / peaks_len as u64) as f64;
                    let pixels_per_sample = height as f64 / window.total_samples as f64;
                    let pixels_per_peak = samples_per_peak * pixels_per_sample;

                    let center_sample = window.start as f64 - window.left_padding as f64 + (window.total_samples as f64 / 2.0);
                    let center_peak_f64 = center_sample / samples_per_peak;
                    let center_py = y + height / 2.0;
                    // When inverted, positive offset should go upward (negative Y direction)
                    let y_dir: f32 = if inverted { -1.0 } else { 1.0 };

                    let half_height_in_peaks = (height as f64 / 2.0 / pixels_per_peak).ceil() as usize;
                    let margin_peaks = half_height_in_peaks / 4 + 20;
                    let half_visible_peaks = half_height_in_peaks + margin_peaks;

                    let center_peak = center_peak_f64 as usize;
                    let first_peak = center_peak.saturating_sub(half_visible_peaks);
                    let last_peak = (center_peak + half_visible_peaks).min(peaks_len);

                    let target_pixels_per_point = highres_target_pixels(stem_idx);
                    let step = ((target_pixels_per_point / pixels_per_peak).round() as usize).max(1);
                    let smooth_radius = smooth_radius_for_stem(stem_idx, step);

                    let first_peak_aligned = ((first_peak + step / 2) / step) * step;
                    let mut peak_idx = first_peak_aligned;
                    while peak_idx < last_peak {
                        let relative_pos = peak_idx as f64 - center_peak_f64;
                        let py = center_py + y_dir * (relative_pos * pixels_per_peak) as f32;

                        if py >= y - 5.0 && py <= y + height + 5.0 {
                            let (min, max) = sample_peak_smoothed(peaks, peak_idx, smooth_radius, stem_idx);

                            let gain = if linked_active[stem_idx] {
                                linked_lufs_gains[stem_idx]
                            } else {
                                host_lufs_gain
                            };
                            let (min, max) = (min * gain, max * gain);

                            // Axis swap: amplitude maps to X, position maps to Y
                            let x_left = center_x + (min * width_scale);   // min is negative
                            let x_right = center_x + (max * width_scale);  // max is positive

                            let clamped_py = py.max(y).min(y + height);
                            left_points.push((x_left.max(x).min(x + width), clamped_py));
                            right_points.push((x_right.max(x).min(x + width), clamped_py));
                        }

                        peak_idx += step;
                    }
                } else {
                    // Cached peaks fallback
                    let height_usize = height as usize;
                    let total_samples = window.total_samples as usize;
                    let step = zoomed_step(stem_idx, height_usize);
                    let smooth_radius = smooth_radius_for_stem(stem_idx, step);

                    let mut py = 0;
                    while py < height_usize {
                        let window_offset = py * total_samples / height_usize;
                        let actual_sample = window.start as i64 - window.left_padding as i64 + window_offset as i64;

                        let current_py = py;
                        py += step;

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

                        let gain = if linked_active[stem_idx] {
                            linked_lufs_gains[stem_idx]
                        } else {
                            host_lufs_gain
                        };
                        let (min, max) = (min * gain, max * gain);

                        let x_left = center_x + (min * width_scale);
                        let x_right = center_x + (max * width_scale);

                        let pixel_y = if inverted {
                            y + height - current_py as f32
                        } else {
                            y + current_py as f32
                        };
                        left_points.push((x_left.max(x).min(x + width), pixel_y));
                        right_points.push((x_right.max(x).min(x + width), pixel_y));
                    }
                }

                if left_points.is_empty() {
                    return;
                }

                // Left envelope top→bottom
                for &(px, py) in left_points.iter() {
                    if first_point {
                        builder.move_to(Point::new(px, py));
                        first_point = false;
                    } else {
                        builder.line_to(Point::new(px, py));
                    }
                }

                // Right envelope bottom→top (closing the path)
                for &(px, py) in right_points.iter().rev() {
                    builder.line_to(Point::new(px, py));
                }

                builder.close();
            });

            frame.fill(&path, waveform_color);
        }
    }

    // Draw cue markers (horizontal lines with left-pointing triangle)
    for marker in &zoomed.cue_markers {
        let marker_sample = (marker.position * zoomed.duration_samples as f64) as u64;
        if marker_sample >= window.start && marker_sample <= window.end {
            let cue_y = sample_to_y(marker_sample);
            frame.fill_rectangle(
                Point::new(x, cue_y - 1.0),
                Size::new(width, 2.0),
                marker.color,
            );
            let triangle = Path::new(|builder| {
                builder.move_to(Point::new(x, cue_y));
                builder.line_to(Point::new(x + 8.0, cue_y - 4.0));
                builder.line_to(Point::new(x + 8.0, cue_y + 4.0));
                builder.close();
            });
            frame.fill(&triangle, marker.color);
        }
    }

    // Draw drop marker
    if let Some(drop_sample) = zoomed.drop_marker {
        if drop_sample >= window.start && drop_sample <= window.end {
            let drop_y = sample_to_y(drop_sample);
            frame.fill_rectangle(
                Point::new(x, drop_y - 1.0),
                Size::new(width, 2.0),
                DROP_MARKER_COLOR,
            );
            let diamond = Path::new(|builder| {
                builder.move_to(Point::new(x, drop_y));
                builder.line_to(Point::new(x + 8.0, drop_y - 6.0));
                builder.line_to(Point::new(x + 16.0, drop_y));
                builder.line_to(Point::new(x + 8.0, drop_y + 6.0));
                builder.close();
            });
            frame.fill(&diamond, DROP_MARKER_COLOR);
        }
    }

    // Playhead: horizontal white line
    let playhead_y = match zoomed.view_mode() {
        ZoomedViewMode::Scrolling => y + height / 2.0,
        ZoomedViewMode::FixedBuffer => {
            if window.total_samples > 0 && playhead >= window.start && playhead <= window.end {
                let frac = (playhead - window.start) as f64 / window.total_samples as f64;
                if inverted {
                    y + height - (frac * height as f64) as f32
                } else {
                    y + (frac * height as f64) as f32
                }
            } else if playhead < window.start {
                if inverted { y + height } else { y }
            } else {
                if inverted { y } else { y + height }
            }
        }
    };
    frame.stroke(
        &Path::line(Point::new(x, playhead_y), Point::new(x + width, playhead_y)),
        Stroke::default()
            .with_color(Color::from_rgb(1.0, 1.0, 1.0))
            .with_width(2.0),
    );

    // Zoom indicator (horizontal bar at bottom edge)
    let indicator_width = (zoomed.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * width;
    let indicator_height = 4.0;
    frame.fill_rectangle(
        Point::new(x + width - indicator_width, y + height - indicator_height),
        Size::new(indicator_width, indicator_height),
        Color::from_rgba(1.0, 1.0, 1.0, 0.5),
    );
}

/// Draw a vertical overview waveform (time flows top-to-bottom, peaks extend left/right).
fn draw_vertical_overview(
    frame: &mut Frame,
    overview: &OverviewState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stem_colors: &[Color; 4],
    stem_active: &[bool; 4],
    _linked_stems: &[bool; 4],
    _linked_active: &[bool; 4],
    overview_scale: Option<f64>,
    inverted: bool,
) {
    let center_x = x + width / 2.0;

    // Background
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, height),
        Color::from_rgb(0.05, 0.05, 0.08),
    );

    if !overview.has_track || overview.duration_samples == 0 {
        return;
    }

    // Helper: normalized source position → Y coordinate
    // When inverted, position 0.0 maps to bottom, 1.0 to top
    let pos_to_y = |pos: f64| -> f32 {
        let display_pos = if let Some(d) = overview_scale { pos * d } else { pos };
        if inverted {
            y + ((1.0 - display_pos) * height as f64) as f32
        } else {
            y + (display_pos * height as f64) as f32
        }
    };

    let pos_visible = |pos: f64| -> bool {
        let display_pos = if let Some(d) = overview_scale { pos * d } else { pos };
        display_pos >= -0.01 && display_pos <= 1.01
    };

    // Pre-stretch peaks if BPM scaling active
    let stretched_waveforms: Option<[Vec<(f32, f32)>; 4]> = overview_scale.map(|d| {
        let out_len = overview.stem_waveforms[0].len().max(height as usize);
        [
            stretch_peaks(&overview.stem_waveforms[0], d, out_len),
            stretch_peaks(&overview.stem_waveforms[1], d, out_len),
            stretch_peaks(&overview.stem_waveforms[2], d, out_len),
            stretch_peaks(&overview.stem_waveforms[3], d, out_len),
        ]
    });

    // Draw loop region (horizontal band)
    if let Some((loop_start, loop_end)) = overview.loop_region {
        let y1 = pos_to_y(loop_start);
        let y2 = pos_to_y(loop_end);
        let sy = y1.min(y2).max(y);
        let ey = y1.max(y2).min(y + height);
        let lh = ey - sy;
        if lh > 0.0 {
            frame.fill_rectangle(
                Point::new(x, sy),
                Size::new(width, lh),
                Color::from_rgba(0.2, 0.8, 0.2, 0.25),
            );
        }
    }

    // Draw stem waveforms (single pane — no split view in overview for simplicity)
    let width_scale = width / 2.0 * 0.85;
    for &stem_idx in STEM_RENDER_ORDER.iter() {
        let stem_peaks: &[(f32, f32)] = if let Some(ref stretched) = stretched_waveforms {
            &stretched[stem_idx]
        } else {
            &overview.stem_waveforms[stem_idx]
        };
        if stem_peaks.is_empty() {
            continue;
        }

        let waveform_color = if stem_active[stem_idx] {
            let base = stem_colors[stem_idx];
            Color::from_rgba(base.r, base.g, base.b, OVERVIEW_WAVEFORM_ALPHA)
        } else {
            let gray = INACTIVE_STEM_GRAYS[stem_idx];
            Color::from_rgba(gray.r, gray.g, gray.b, 0.4)
        };

        // Build vertical filled path
        let path = Path::new(|builder| {
            let peaks_len = stem_peaks.len();
            if peaks_len == 0 {
                return;
            }

            let mut left_points: Vec<(f32, f32)> = Vec::with_capacity(peaks_len);
            let mut right_points: Vec<(f32, f32)> = Vec::with_capacity(peaks_len);

            // Map each peak to a Y position, amplitude to X
            let step = (peaks_len / (height as usize)).max(1);
            let mut i = 0;
            while i < peaks_len {
                let (min, max) = stem_peaks[i];
                let frac = i as f32 / peaks_len as f32;
                let py = if inverted {
                    y + height - frac * height
                } else {
                    y + frac * height
                };

                let x_left = center_x + (min * width_scale);
                let x_right = center_x + (max * width_scale);

                left_points.push((x_left.max(x).min(x + width), py));
                right_points.push((x_right.max(x).min(x + width), py));

                i += step;
            }

            if left_points.is_empty() {
                return;
            }

            // Left envelope top→bottom
            builder.move_to(Point::new(left_points[0].0, left_points[0].1));
            for &(px, py) in left_points.iter().skip(1) {
                builder.line_to(Point::new(px, py));
            }

            // Right envelope bottom→top
            for &(px, py) in right_points.iter().rev() {
                builder.line_to(Point::new(px, py));
            }

            builder.close();
        });

        frame.fill(&path, waveform_color);
    }

    // Beat markers (horizontal lines)
    let step = (overview.grid_bars * 4) as usize;
    for (i, &beat_pos) in overview.beat_markers.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        if !pos_visible(beat_pos) {
            continue;
        }
        let beat_y = pos_to_y(beat_pos).max(y).min(y + height);
        let color = if (i / step) % 4 == 0 {
            Color::from_rgba(1.0, 0.3, 0.3, 0.6)
        } else {
            Color::from_rgba(0.5, 0.5, 0.5, 0.4)
        };
        frame.stroke(
            &Path::line(Point::new(x, beat_y), Point::new(x + width, beat_y)),
            Stroke::default().with_color(color).with_width(1.0),
        );
    }

    // Cue markers (horizontal lines)
    for marker in &overview.cue_markers {
        if !pos_visible(marker.position) {
            continue;
        }
        let cue_y = pos_to_y(marker.position).max(y).min(y + height);
        frame.fill_rectangle(
            Point::new(x, cue_y - 1.0),
            Size::new(width, 2.0),
            marker.color,
        );
    }

    // Main cue point
    if let Some(cue_pos) = overview.cue_position {
        if pos_visible(cue_pos) {
            let cue_y = pos_to_y(cue_pos).max(y).min(y + height);
            frame.stroke(
                &Path::line(Point::new(x, cue_y), Point::new(x + width, cue_y)),
                Stroke::default().with_color(Color::from_rgb(0.6, 0.6, 0.6)).with_width(2.0),
            );
        }
    }

    // Playhead (horizontal white line)
    if overview.duration_samples > 0 {
        let playhead_ratio = playhead as f64 / overview.duration_samples as f64;
        let playhead_y = pos_to_y(playhead_ratio).max(y).min(y + height);
        frame.stroke(
            &Path::line(Point::new(x, playhead_y), Point::new(x + width, playhead_y)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );
    }
}
