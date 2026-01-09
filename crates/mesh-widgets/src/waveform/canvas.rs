//! Canvas Program implementations for waveform rendering
//!
//! These implement the iced canvas `Program` trait for custom waveform drawing.
//! Each canvas type takes callback closures for event handling, following
//! idiomatic iced 0.14 patterns.

use super::state::{
    CombinedState, OverviewState, PlayerCanvasState, ZoomedState, ZoomedViewMode,
    COMBINED_WAVEFORM_GAP, DECK_HEADER_HEIGHT, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};
use crate::{STEM_COLORS, CueMarker};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{mouse, Color, Point, Rectangle, Size, Theme};
use mesh_core::engine::SLICER_NUM_SLICES;
use mesh_core::types::SAMPLE_RATE;

// =============================================================================
// Canvas Interaction States
// =============================================================================

/// Canvas state for tracking overview waveform mouse interaction
#[derive(Debug, Clone, Copy, Default)]
pub struct OverviewInteraction {
    /// Whether mouse button is pressed (for drag seeking)
    pub is_dragging: bool,
}

/// Canvas state for tracking zoomed waveform interaction (zoom gesture)
#[derive(Debug, Clone, Copy, Default)]
pub struct ZoomedInteraction {
    /// Mouse Y position when drag started (for zoom gesture)
    pub drag_start_y: Option<f32>,
    /// Zoom level when drag started
    pub drag_start_zoom: u32,
}

/// Canvas state for combined waveform (tracks both interactions)
#[derive(Debug, Clone, Copy, Default)]
pub struct CombinedInteraction {
    /// Zoom gesture state for zoomed region
    pub drag_start_y: Option<f32>,
    /// Zoom level when drag started
    pub drag_start_zoom: u32,
    /// Whether dragging in overview region for seeking
    pub is_seeking: bool,
}

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
// Player Canvas Layout Constants
// =============================================================================

/// Gap between deck cells in the 2x2 grid
pub const DECK_GRID_GAP: f32 = 10.0;

/// Gap between zoomed and overview within a deck cell
pub const DECK_INTERNAL_GAP: f32 = 2.0;

/// Total height of one deck cell (header + zoomed + gap + overview)
/// 16 + 120 + 2 + 35 = 173px
pub const DECK_CELL_HEIGHT: f32 =
    DECK_HEADER_HEIGHT + ZOOMED_WAVEFORM_HEIGHT + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;

// Legacy constants (kept for backwards compatibility with CombinedCanvas)
/// Gap between zoomed waveform cells in the 2x2 grid
pub const ZOOMED_GRID_GAP: f32 = 4.0;

/// Gap between overview waveform rows in the stack
pub const OVERVIEW_STACK_GAP: f32 = 2.0;

/// Gap between the zoomed grid and overview stack
pub const PLAYER_SECTION_GAP: f32 = 8.0;

// =============================================================================
// Overview Canvas Program
// =============================================================================

/// Canvas program for overview waveform rendering with click-to-seek
///
/// Takes a callback closure `on_seek` that's called with the normalized
/// position (0.0 to 1.0) when the user clicks or drags on the canvas.
pub struct OverviewCanvas<'a, Message, F>
where
    F: Fn(f64) -> Message,
{
    pub state: &'a OverviewState,
    pub on_seek: F,
}

impl<'a, Message, F> Program<Message> for OverviewCanvas<'a, Message, F>
where
    Message: Clone,
    F: Fn(f64) -> Message,
{
    type State = OverviewInteraction;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let Some(position) = cursor.position_in(bounds) {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    interaction.is_dragging = true;
                    let seek_position = (position.x / bounds.width).clamp(0.0, 1.0) as f64;
                    return Some(canvas::Action::publish((self.on_seek)(seek_position)));
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    interaction.is_dragging = false;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if interaction.is_dragging {
                        let seek_position = (position.x / bounds.width).clamp(0.0, 1.0) as f64;
                        return Some(canvas::Action::publish((self.on_seek)(seek_position)));
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            interaction.is_dragging = false;
        }

        None
    }

    fn mouse_interaction(
        &self,
        _interaction: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
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

        // Background
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color::from_rgb(0.1, 0.1, 0.12),
        );

        if !self.state.has_track {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let center_y = height / 2.0;

        // Show loading indicator if audio is still loading
        if self.state.loading {
            let loading_color = Color::from_rgba(0.6, 0.6, 0.6, 0.8);
            frame.fill_rectangle(
                Point::new(width * 0.3, center_y - 2.0),
                Size::new(width * 0.4, 4.0),
                loading_color,
            );
            return vec![frame.into_geometry()];
        }

        // Show missing preview indicator
        if self.state.missing_preview_message.is_some() {
            let missing_color = Color::from_rgba(0.5, 0.4, 0.3, 0.6);
            for x in (0..(width as usize)).step_by(20) {
                frame.fill_rectangle(
                    Point::new(x as f32, center_y - 1.0),
                    Size::new(10.0, 2.0),
                    missing_color,
                );
            }
            return vec![frame.into_geometry()];
        }

        // Draw beat markers (first beat of each bar highlighted in red)
        for (i, &beat) in self.state.beat_markers.iter().enumerate() {
            let x = (beat * width as f64) as f32;
            let (color, line_width) = if i % 4 == 0 {
                (Color::from_rgba(1.0, 0.3, 0.3, 0.8), 2.0)
            } else {
                (Color::from_rgba(0.4, 0.4, 0.4, 0.6), 1.0)
            };
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke::default().with_color(color).with_width(line_width),
            );
        }

        // Draw all 4 stem waveforms overlapped
        draw_stem_waveforms(&mut frame, &self.state.stem_waveforms, width, height, center_y, 0.6);

        // Draw cue markers
        draw_cue_markers(&mut frame, &self.state.cue_markers, width, height, 0.0);

        // Draw playhead
        let playhead_x = (self.state.position * width as f64) as f32;
        frame.stroke(
            &Path::line(Point::new(playhead_x, 0.0), Point::new(playhead_x, height)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );

        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Zoomed Canvas Program
// =============================================================================

/// Canvas program for zoomed waveform rendering with zoom gesture
///
/// Takes a callback closure `on_zoom` that's called with the new zoom level
/// (in bars) when the user drags up/down on the canvas.
pub struct ZoomedCanvas<'a, Message, F>
where
    F: Fn(u32) -> Message,
{
    pub state: &'a ZoomedState,
    pub playhead: u64,
    pub on_zoom: F,
}

impl<'a, Message, F> Program<Message> for ZoomedCanvas<'a, Message, F>
where
    Message: Clone,
    F: Fn(u32) -> Message,
{
    type State = ZoomedInteraction;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let Some(position) = cursor.position_in(bounds) {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    interaction.drag_start_y = Some(position.y);
                    interaction.drag_start_zoom = self.state.zoom_bars;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    interaction.drag_start_y = None;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if let Some(start_y) = interaction.drag_start_y {
                        let delta = start_y - position.y;
                        let zoom_change = (delta / ZOOM_PIXELS_PER_LEVEL) as i32;
                        let new_zoom = (interaction.drag_start_zoom as i32 - zoom_change)
                            .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32)
                            as u32;

                        if new_zoom != self.state.zoom_bars {
                            return Some(canvas::Action::publish((self.on_zoom)(new_zoom)));
                        }
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            interaction.drag_start_y = None;
        }

        None
    }

    fn mouse_interaction(
        &self,
        interaction: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            if interaction.drag_start_y.is_some() {
                mouse::Interaction::ResizingVertically
            } else {
                mouse::Interaction::Grab
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

        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color::from_rgb(0.08, 0.08, 0.1),
        );

        if !self.state.has_track || self.state.duration_samples == 0 {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let center_y = height / 2.0;

        let (view_start, view_end) = self.state.visible_range(self.playhead);
        let view_samples = (view_end - view_start) as f64;

        if view_samples <= 0.0 {
            return vec![frame.into_geometry()];
        }

        // Draw beat markers (only those in visible range)
        draw_beat_markers_zoomed(&mut frame, &self.state.beat_grid, view_start, view_end, view_samples, width, height);

        // Draw stem waveforms from cached peaks
        draw_cached_peaks(&mut frame, self.state, view_start, view_samples, width, center_y);

        // Draw cue markers in visible range
        draw_cue_markers_zoomed(&mut frame, &self.state.cue_markers, self.state.duration_samples, view_start, view_end, view_samples, width, height);

        // Draw playhead at center
        let playhead_x = width / 2.0;
        frame.stroke(
            &Path::line(Point::new(playhead_x, 0.0), Point::new(playhead_x, height)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );

        // Draw zoom indicator - vertical bar on right edge
        // Height represents zoom level (0-100% of waveform height)
        let indicator_height = (self.state.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * height;
        let indicator_width = 4.0;
        frame.fill_rectangle(
            Point::new(width - indicator_width, height - indicator_height),
            Size::new(indicator_width, indicator_height),
            Color::from_rgba(1.0, 1.0, 1.0, 0.5),
        );

        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Combined Canvas Program
// =============================================================================

/// Canvas program for combined waveform rendering (zoomed + overview)
///
/// Takes callback closures for both seek and zoom operations.
/// This workaround combines both views into a single canvas due to iced bug #3040.
pub struct CombinedCanvas<'a, Message, SeekFn, ZoomFn>
where
    SeekFn: Fn(f64) -> Message,
    ZoomFn: Fn(u32) -> Message,
{
    pub state: &'a CombinedState,
    pub playhead: u64,
    pub on_seek: SeekFn,
    pub on_zoom: ZoomFn,
}

impl<'a, Message, SeekFn, ZoomFn> Program<Message> for CombinedCanvas<'a, Message, SeekFn, ZoomFn>
where
    Message: Clone,
    SeekFn: Fn(f64) -> Message,
    ZoomFn: Fn(u32) -> Message,
{
    type State = CombinedInteraction;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        // Zoomed waveform region (top)
        let zoomed_bounds = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: ZOOMED_WAVEFORM_HEIGHT,
        };

        // Handle zoom gestures in zoomed region
        if let Some(position) = cursor.position_in(zoomed_bounds) {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    interaction.drag_start_y = Some(position.y);
                    interaction.drag_start_zoom = self.state.zoomed.zoom_bars;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    interaction.drag_start_y = None;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if let Some(start_y) = interaction.drag_start_y {
                        let delta = start_y - position.y;
                        let zoom_change = (delta / ZOOM_PIXELS_PER_LEVEL) as i32;
                        let new_zoom = (interaction.drag_start_zoom as i32 - zoom_change)
                            .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32)
                            as u32;

                        if new_zoom != self.state.zoomed.zoom_bars {
                            return Some(canvas::Action::publish((self.on_zoom)(new_zoom)));
                        }
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            interaction.drag_start_y = None;
        }

        // Overview waveform region (bottom)
        let overview_y = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP;
        let overview_bounds = Rectangle {
            x: bounds.x,
            y: bounds.y + overview_y,
            width: bounds.width,
            height: WAVEFORM_HEIGHT,
        };

        // Handle seek clicks in overview region
        if let Some(position) = cursor.position_in(overview_bounds) {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    interaction.is_seeking = true;
                    if self.state.overview.has_track && self.state.overview.duration_samples > 0 {
                        let seek_ratio = (position.x / bounds.width) as f64;
                        return Some(canvas::Action::publish((self.on_seek)(seek_ratio)));
                    }
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    interaction.is_seeking = false;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if interaction.is_seeking && self.state.overview.has_track && self.state.overview.duration_samples > 0 {
                        let seek_ratio = (position.x / bounds.width).clamp(0.0, 1.0) as f64;
                        return Some(canvas::Action::publish((self.on_seek)(seek_ratio)));
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
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
        let zoomed_bounds = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: ZOOMED_WAVEFORM_HEIGHT,
        };

        if cursor.is_over(zoomed_bounds) {
            if interaction.drag_start_y.is_some() {
                mouse::Interaction::ResizingVertically
            } else {
                mouse::Interaction::Grab
            }
        } else {
            let overview_y = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP;
            let overview_bounds = Rectangle {
                x: bounds.x,
                y: bounds.y + overview_y,
                width: bounds.width,
                height: WAVEFORM_HEIGHT,
            };

            if cursor.is_over(overview_bounds) {
                mouse::Interaction::Pointer
            } else {
                mouse::Interaction::default()
            }
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

        // =====================================================================
        // ZOOMED WAVEFORM (top section)
        // =====================================================================
        draw_zoomed_section(&mut frame, &self.state.zoomed, self.playhead, width);

        // =====================================================================
        // OVERVIEW WAVEFORM (bottom section)
        // =====================================================================
        draw_overview_section(&mut frame, &self.state.overview, self.playhead, width);

        vec![frame.into_geometry()]
    }
}

// =============================================================================
// Drawing Helper Functions
// =============================================================================

/// Draw a single stem waveform as a filled path for smoother appearance
///
/// Instead of drawing hundreds of individual vertical lines, this builds a single
/// filled path tracing the upper envelope (max values) left-to-right, then the
/// lower envelope (min values) right-to-left, creating a smooth filled shape.
///
/// This approach:
/// - Uses 1 GPU draw call instead of hundreds
/// - Produces naturally anti-aliased edges from filled polygon rasterization
/// - Renders faster and looks smoother
fn draw_stem_waveform_filled(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    width: f32,
) {
    if peaks.is_empty() || width < 2.0 {
        return;
    }

    let peaks_len = peaks.len() as f32;

    let path = Path::new(|builder| {
        // Start at first point's max value (upper envelope)
        let (_, first_max) = peaks[0];
        let first_y = center_y - (first_max * height_scale);
        builder.move_to(Point::new(x_offset, first_y));

        // Draw upper envelope left to right
        for px in 1..(width as usize) {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx >= peaks.len() {
                break;
            }
            let (_, max) = peaks[peak_idx];
            let x = x_offset + px as f32;
            let y = center_y - (max * height_scale);
            builder.line_to(Point::new(x, y));
        }

        // Draw lower envelope right to left (closing the shape)
        for px in (0..(width as usize)).rev() {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx >= peaks.len() {
                continue;
            }
            let (min, _) = peaks[peak_idx];
            let x = x_offset + px as f32;
            let y = center_y - (min * height_scale);
            builder.line_to(Point::new(x, y));
        }

        builder.close();
    });

    frame.fill(&path, color);
}

/// Draw stem waveforms from peak data using filled paths
fn draw_stem_waveforms(
    frame: &mut Frame,
    stem_waveforms: &[Vec<(f32, f32)>; 4],
    width: f32,
    _height: f32,
    center_y: f32,
    alpha: f32,
) {
    // Draw stems in reverse order for proper layering (Other behind, Vocals on top)
    for stem_idx in (0..4).rev() {
        let waveform_data = &stem_waveforms[stem_idx];
        if waveform_data.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, alpha);
        let height_scale = center_y * 0.9;

        draw_stem_waveform_filled(frame, waveform_data, 0.0, center_y, height_scale, waveform_color, width);
    }
}

/// Draw cue markers
fn draw_cue_markers(
    frame: &mut Frame,
    cue_markers: &[CueMarker],
    width: f32,
    height: f32,
    y_offset: f32,
) {
    for marker in cue_markers {
        let x = (marker.position * width as f64) as f32;

        // Draw vertical line
        frame.fill_rectangle(
            Point::new(x - 1.5, y_offset),
            Size::new(3.0, height),
            marker.color,
        );

        // Draw triangle at top
        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(x, y_offset));
            builder.line_to(Point::new(x - 8.0, y_offset + 15.0));
            builder.line_to(Point::new(x + 8.0, y_offset + 15.0));
            builder.close();
        });
        frame.fill(&triangle, marker.color);
    }
}

/// Draw beat markers for zoomed view
fn draw_beat_markers_zoomed(
    frame: &mut Frame,
    beat_grid: &[u64],
    view_start: u64,
    view_end: u64,
    view_samples: f64,
    width: f32,
    height: f32,
) {
    for (i, &beat_sample) in beat_grid.iter().enumerate() {
        if beat_sample < view_start || beat_sample > view_end {
            continue;
        }

        let x = ((beat_sample - view_start) as f64 / view_samples * width as f64) as f32;

        let (color, line_width) = if i % 4 == 0 {
            (Color::from_rgba(1.0, 0.3, 0.3, 0.9), 2.0)
        } else {
            (Color::from_rgba(0.5, 0.5, 0.5, 0.7), 1.0)
        };

        frame.stroke(
            &Path::line(Point::new(x, 0.0), Point::new(x, height)),
            Stroke::default().with_color(color).with_width(line_width),
        );
    }
}

/// Draw cached peaks for zoomed view using filled paths
fn draw_cached_peaks(
    frame: &mut Frame,
    state: &ZoomedState,
    view_start: u64,
    view_samples: f64,
    width: f32,
    center_y: f32,
) {
    if state.cache_end <= state.cache_start {
        return;
    }

    let cache_samples = (state.cache_end - state.cache_start) as f64;
    let height_scale = center_y * 0.85;

    for stem_idx in (0..4).rev() {
        let peaks = &state.cached_peaks[stem_idx];
        if peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.7);
        let peaks_len = peaks.len() as f64;

        // Build filled path for this stem
        let path = Path::new(|builder| {
            let mut first_point = true;
            let mut upper_points: Vec<(f32, f32)> = Vec::with_capacity(width as usize);
            let mut lower_points: Vec<(f32, f32)> = Vec::with_capacity(width as usize);

            for x in 0..(width as usize) {
                let view_sample = view_start as f64 + (x as f64 / width as f64) * view_samples;
                let cache_offset = view_sample - state.cache_start as f64;
                if cache_offset < 0.0 || cache_offset >= cache_samples {
                    continue;
                }

                let cache_idx = (cache_offset / cache_samples * peaks_len) as usize;
                if cache_idx >= peaks.len() {
                    continue;
                }

                let (min, max) = peaks[cache_idx];
                let y_max = center_y - (max * height_scale);
                let y_min = center_y - (min * height_scale);

                upper_points.push((x as f32, y_max));
                lower_points.push((x as f32, y_min));
            }

            if upper_points.is_empty() {
                return;
            }

            // Draw upper envelope left to right
            for &(x, y) in upper_points.iter() {
                if first_point {
                    builder.move_to(Point::new(x, y));
                    first_point = false;
                } else {
                    builder.line_to(Point::new(x, y));
                }
            }

            // Draw lower envelope right to left
            for &(x, y) in lower_points.iter().rev() {
                builder.line_to(Point::new(x, y));
            }

            builder.close();
        });

        frame.fill(&path, waveform_color);
    }
}

/// Draw cue markers for zoomed view
fn draw_cue_markers_zoomed(
    frame: &mut Frame,
    cue_markers: &[CueMarker],
    duration_samples: u64,
    view_start: u64,
    view_end: u64,
    view_samples: f64,
    width: f32,
    height: f32,
) {
    for marker in cue_markers {
        let cue_sample = (marker.position * duration_samples as f64) as u64;

        if cue_sample < view_start || cue_sample > view_end {
            continue;
        }

        let x = ((cue_sample - view_start) as f64 / view_samples * width as f64) as f32;

        frame.fill_rectangle(
            Point::new(x - 1.0, 0.0),
            Size::new(2.0, height),
            marker.color,
        );

        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(x, 0.0));
            builder.line_to(Point::new(x - 6.0, 12.0));
            builder.line_to(Point::new(x + 6.0, 12.0));
            builder.close();
        });
        frame.fill(&triangle, marker.color);
    }
}

/// Draw the zoomed waveform section of the combined view
fn draw_zoomed_section(
    frame: &mut Frame,
    zoomed: &ZoomedState,
    playhead: u64,
    width: f32,
) {
    let zoomed_height = ZOOMED_WAVEFORM_HEIGHT;
    let zoomed_center_y = zoomed_height / 2.0;

    // Background
    frame.fill_rectangle(
        Point::ORIGIN,
        Size::new(width, zoomed_height),
        Color::from_rgb(0.08, 0.08, 0.1),
    );

    if !zoomed.has_track || zoomed.duration_samples == 0 {
        return;
    }

    let (view_start, view_end) = zoomed.visible_range(playhead);
    let view_samples = (view_end - view_start) as f64;

    if view_samples > 0.0 {
        // Draw loop region (behind everything else)
        if let Some((loop_start_norm, loop_end_norm)) = zoomed.loop_region {
            // Convert normalized positions to sample positions
            let loop_start_sample = (loop_start_norm * zoomed.duration_samples as f64) as u64;
            let loop_end_sample = (loop_end_norm * zoomed.duration_samples as f64) as u64;

            // Only draw if loop overlaps the visible window
            if loop_end_sample > view_start && loop_start_sample < view_end {
                // Calculate x positions (clamp to visible window)
                let start_x = if loop_start_sample <= view_start {
                    0.0
                } else {
                    ((loop_start_sample - view_start) as f64 / view_samples * width as f64) as f32
                };
                let end_x = if loop_end_sample >= view_end {
                    width
                } else {
                    ((loop_end_sample - view_start) as f64 / view_samples * width as f64) as f32
                };

                let loop_width = end_x - start_x;
                if loop_width > 0.0 {
                    // Semi-transparent green fill
                    frame.fill_rectangle(
                        Point::new(start_x, 0.0),
                        Size::new(loop_width, zoomed_height),
                        Color::from_rgba(0.2, 0.8, 0.2, 0.25),
                    );
                    // Draw loop boundaries (only if visible)
                    if loop_start_sample > view_start && loop_start_sample < view_end {
                        frame.stroke(
                            &Path::line(
                                Point::new(start_x, 0.0),
                                Point::new(start_x, zoomed_height),
                            ),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                    if loop_end_sample > view_start && loop_end_sample < view_end {
                        frame.stroke(
                            &Path::line(
                                Point::new(end_x, 0.0),
                                Point::new(end_x, zoomed_height),
                            ),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                }
            }
        }

        // Draw beat markers
        draw_beat_markers_zoomed(frame, &zoomed.beat_grid, view_start, view_end, view_samples, width, zoomed_height);

        // Draw stem waveforms
        draw_cached_peaks(frame, zoomed, view_start, view_samples, width, zoomed_center_y);

        // Draw cue markers
        draw_cue_markers_zoomed(frame, &zoomed.cue_markers, zoomed.duration_samples, view_start, view_end, view_samples, width, zoomed_height);
    }

    // Draw playhead at center
    let playhead_x = width / 2.0;
    frame.stroke(
        &Path::line(Point::new(playhead_x, 0.0), Point::new(playhead_x, zoomed_height)),
        Stroke::default()
            .with_color(Color::from_rgb(1.0, 1.0, 1.0))
            .with_width(2.0),
    );

    // Draw zoom indicator - vertical bar on right edge
    let indicator_height = (zoomed.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * zoomed_height;
    let indicator_width = 4.0;
    frame.fill_rectangle(
        Point::new(width - indicator_width, zoomed_height - indicator_height),
        Size::new(indicator_width, indicator_height),
        Color::from_rgba(1.0, 1.0, 1.0, 0.5),
    );
}

/// Draw the overview waveform section of the combined view
fn draw_overview_section(
    frame: &mut Frame,
    overview: &OverviewState,
    playhead: u64,
    width: f32,
) {
    let overview_y = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP;
    let overview_height = WAVEFORM_HEIGHT;
    let overview_center_y = overview_y + overview_height / 2.0;

    // Background
    frame.fill_rectangle(
        Point::new(0.0, overview_y),
        Size::new(width, overview_height),
        Color::from_rgb(0.05, 0.05, 0.08),
    );

    if !overview.has_track || overview.duration_samples == 0 {
        return;
    }

    // Draw loop region (semi-transparent green overlay)
    if let Some((loop_start, loop_end)) = overview.loop_region {
        let start_x = (loop_start * width as f64) as f32;
        let end_x = (loop_end * width as f64) as f32;
        let loop_width = end_x - start_x;
        if loop_width > 0.0 {
            frame.fill_rectangle(
                Point::new(start_x, overview_y),
                Size::new(loop_width, overview_height),
                Color::from_rgba(0.2, 0.8, 0.2, 0.25), // Semi-transparent green
            );
            // Draw loop boundaries
            frame.stroke(
                &Path::line(
                    Point::new(start_x, overview_y),
                    Point::new(start_x, overview_y + overview_height),
                ),
                Stroke::default()
                    .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                    .with_width(2.0),
            );
            frame.stroke(
                &Path::line(
                    Point::new(end_x, overview_y),
                    Point::new(end_x, overview_y + overview_height),
                ),
                Stroke::default()
                    .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                    .with_width(2.0),
            );
        }
    }

    // Draw slicer region (semi-transparent orange overlay with slice divisions)
    if let Some((slicer_start, slicer_end)) = overview.slicer_region {
        let start_x = (slicer_start * width as f64) as f32;
        let end_x = (slicer_end * width as f64) as f32;
        let slicer_width = end_x - start_x;
        if slicer_width > 0.0 {
            // Orange overlay for slicer buffer
            frame.fill_rectangle(
                Point::new(start_x, overview_y),
                Size::new(slicer_width, overview_height),
                Color::from_rgba(1.0, 0.5, 0.0, 0.15), // Semi-transparent orange
            );

            // Draw slice divisions
            let slice_width = slicer_width / SLICER_NUM_SLICES as f32;
            for i in 0..=SLICER_NUM_SLICES {
                let x = start_x + slice_width * i as f32;
                let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
                let line_width = if is_boundary { 2.0 } else { 1.0 };
                let alpha = if is_boundary { 0.8 } else { 0.4 };

                // Highlight current slice
                let color = if !is_boundary {
                    if let Some(current) = overview.slicer_current_slice {
                        if i as u8 == current + 1 {
                            // Highlight line after current slice
                            Color::from_rgba(1.0, 0.8, 0.2, 0.9)
                        } else {
                            Color::from_rgba(1.0, 0.6, 0.1, alpha)
                        }
                    } else {
                        Color::from_rgba(1.0, 0.6, 0.1, alpha)
                    }
                } else {
                    Color::from_rgba(1.0, 0.6, 0.1, alpha)
                };

                frame.stroke(
                    &Path::line(
                        Point::new(x, overview_y),
                        Point::new(x, overview_y + overview_height),
                    ),
                    Stroke::default().with_color(color).with_width(line_width),
                );
            }

            // Highlight current playing slice with brighter overlay
            if let Some(current) = overview.slicer_current_slice {
                let slice_x = start_x + slice_width * current as f32;
                frame.fill_rectangle(
                    Point::new(slice_x, overview_y),
                    Size::new(slice_width, overview_height),
                    Color::from_rgba(1.0, 0.6, 0.0, 0.25), // Brighter orange for current slice
                );
            }
        }
    }

    // Draw beat markers with configurable density
    let step = (overview.grid_bars * 4) as usize;
    for (i, &beat_pos) in overview.beat_markers.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        let x = (beat_pos * width as f64) as f32;
        let (color, line_height) = if (i / step) % 4 == 0 {
            (Color::from_rgba(1.0, 0.3, 0.3, 0.6), overview_height)
        } else {
            (Color::from_rgba(0.5, 0.5, 0.5, 0.4), overview_height * 0.5)
        };
        frame.stroke(
            &Path::line(
                Point::new(x, overview_y + (overview_height - line_height) / 2.0),
                Point::new(x, overview_y + (overview_height + line_height) / 2.0),
            ),
            Stroke::default().with_color(color).with_width(1.0),
        );
    }

    // Draw stem waveforms using filled paths
    let height_scale = overview_height / 2.0 * 0.85;
    for stem_idx in (0..4).rev() {
        let stem_peaks = &overview.stem_waveforms[stem_idx];
        if stem_peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.6);

        draw_stem_waveform_filled(frame, stem_peaks, 0.0, overview_center_y, height_scale, waveform_color, width);
    }

    // Draw cue markers
    for marker in &overview.cue_markers {
        let x = (marker.position * width as f64) as f32;
        frame.fill_rectangle(
            Point::new(x - 1.0, overview_y),
            Size::new(2.0, overview_height),
            marker.color,
        );
        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(x, overview_y));
            builder.line_to(Point::new(x - 5.0, overview_y + 10.0));
            builder.line_to(Point::new(x + 5.0, overview_y + 10.0));
            builder.close();
        });
        frame.fill(&triangle, marker.color);
    }

    // Draw main cue point marker (orange)
    if let Some(cue_pos) = overview.cue_position {
        let cue_x = (cue_pos * width as f64) as f32;
        let cue_color = Color::from_rgb(0.6, 0.6, 0.6);
        frame.stroke(
            &Path::line(
                Point::new(cue_x, overview_y),
                Point::new(cue_x, overview_y + overview_height),
            ),
            Stroke::default().with_color(cue_color).with_width(2.0),
        );
        let triangle = Path::new(|builder| {
            builder.move_to(Point::new(cue_x, overview_y));
            builder.line_to(Point::new(cue_x - 5.0, overview_y + 8.0));
            builder.line_to(Point::new(cue_x + 5.0, overview_y + 8.0));
            builder.close();
        });
        frame.fill(&triangle, cue_color);
    }

    // Draw playhead
    if overview.duration_samples > 0 {
        let playhead_ratio = playhead as f64 / overview.duration_samples as f64;
        let playhead_x = (playhead_ratio * width as f64) as f32;
        frame.stroke(
            &Path::line(
                Point::new(playhead_x, overview_y),
                Point::new(playhead_x, overview_y + overview_height),
            ),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );
    }
}

// =============================================================================
// Player Canvas Program (4-Deck Unified View)
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
        let cell_height = DECK_CELL_HEIGHT;

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
            let header_end = DECK_HEADER_HEIGHT;
            let zoomed_end = header_end + ZOOMED_WAVEFORM_HEIGHT;
            let overview_start = zoomed_end + DECK_INTERNAL_GAP;
            let overview_end = overview_start + WAVEFORM_HEIGHT;

            // Check if in zoomed region (drag to zoom)
            if local_y >= header_end && local_y < zoomed_end {
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
            let cell_height = DECK_CELL_HEIGHT;

            // Determine which row we're in
            let row = if position.y < cell_height { 0 } else { 1 };
            let cell_y = if row == 0 { 0.0 } else { cell_height + DECK_GRID_GAP };
            let local_y = position.y - cell_y;

            // Regions within cell
            let header_end = DECK_HEADER_HEIGHT;
            let zoomed_end = header_end + ZOOMED_WAVEFORM_HEIGHT;
            let overview_start = zoomed_end + DECK_INTERNAL_GAP;
            let overview_end = overview_start + WAVEFORM_HEIGHT;

            if local_y >= header_end && local_y < zoomed_end {
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
        let cell_height = DECK_CELL_HEIGHT;

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

            draw_deck_quadrant(
                &mut frame,
                &self.state.decks[deck_idx],
                playhead,
                *x,
                *y,
                cell_width,
                deck_idx,
                track_name,
                track_key,
                is_master,
                stem_active,
                transpose,
                key_match_enabled,
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
/// ┌─────────────────────────────────────┐
/// │ [N] Track Name Here         16px   │ ← Header row
/// ├─────────────────────────────────────┤
/// │                                     │
/// │     Zoomed Waveform          120px │
/// │                                     │
/// ├─────────────────────────────────────┤
/// │     Overview Waveform         35px │
/// └─────────────────────────────────────┘
/// ```
fn draw_deck_quadrant(
    frame: &mut Frame,
    deck: &CombinedState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    deck_idx: usize,
    track_name: &str,
    track_key: &str,
    is_master: bool,
    stem_active: &[bool; 4],
    transpose: i8,
    key_match_enabled: bool,
) {
    use iced::widget::canvas::Text;
    use iced::alignment::{Horizontal, Vertical};

    // Stem indicator width on left side
    const STEM_INDICATOR_WIDTH: f32 = 6.0;
    const STEM_INDICATOR_GAP: f32 = 2.0;

    // Draw header background
    let header_bg_color = Color::from_rgb(0.10, 0.10, 0.12);
    frame.fill_rectangle(
        Point::new(x, y),
        Size::new(width, DECK_HEADER_HEIGHT),
        header_bg_color,
    );

    // Draw deck number badge background
    let badge_width = 28.0;
    let badge_margin = 4.0;
    let badge_height = DECK_HEADER_HEIGHT - 6.0;
    let badge_y = y + 3.0;

    // Badge background color based on state
    let badge_bg_color = if is_master {
        Color::from_rgb(0.15, 0.35, 0.15) // Dark green for master
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

    // Draw deck number text
    let deck_num_text = format!("{}", deck_idx + 1);
    let text_color = if is_master {
        Color::from_rgb(0.4, 1.0, 0.4) // Bright green for master
    } else if deck.zoomed.has_track {
        Color::from_rgb(0.7, 0.7, 0.9) // Light blue for loaded
    } else {
        Color::from_rgb(0.5, 0.5, 0.5) // Gray for empty
    };

    frame.fill_text(Text {
        content: deck_num_text,
        position: Point::new(x + badge_margin + badge_width / 2.0, y + DECK_HEADER_HEIGHT / 2.0),
        size: 14.0.into(),
        color: text_color,
        align_x: Horizontal::Center.into(),
        align_y: Vertical::Center.into(),
        ..Text::default()
    });

    // Draw track key in top right corner (if loaded)
    // Format: "Am" (normal), "Am → +2" (transposing), "Am ✓" (compatible/no transpose)
    if deck.overview.has_track && !track_key.is_empty() {
        let (key_display, key_color) = if is_master || !key_match_enabled {
            // Master deck or key match disabled: just show key
            (track_key.to_string(), Color::from_rgb(0.6, 0.8, 0.6))
        } else if transpose == 0 {
            // Key match enabled, compatible keys (no transpose needed)
            (format!("{} ✓", track_key), Color::from_rgb(0.5, 0.9, 0.5)) // Brighter green
        } else {
            // Key match enabled, transposing
            let sign = if transpose > 0 { "+" } else { "" };
            (format!("{} → {}{}", track_key, sign, transpose), Color::from_rgb(0.9, 0.7, 0.5)) // Orange tint
        };

        frame.fill_text(Text {
            content: key_display,
            position: Point::new(x + width - 8.0, y + DECK_HEADER_HEIGHT / 2.0),
            size: 11.0.into(),
            color: key_color,
            align_x: Horizontal::Right.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    }

    // Draw track name text (if loaded) - leave space for key on right
    let name_x = x + badge_margin + badge_width + 8.0;
    // More space needed when showing transpose info (e.g. "Am → +2")
    let key_space = if !track_key.is_empty() { 80.0 } else { 0.0 };
    let max_name_width = width - badge_width - badge_margin * 2.0 - 16.0 - key_space;

    if deck.overview.has_track && !track_name.is_empty() {
        // Truncate track name if too long (rough estimate: ~7px per char)
        let max_chars = (max_name_width / 7.0) as usize;
        let display_name = if track_name.len() > max_chars && max_chars > 3 {
            format!("{}...", &track_name[..max_chars - 3])
        } else {
            track_name.to_string()
        };

        frame.fill_text(Text {
            content: display_name,
            position: Point::new(name_x, y + DECK_HEADER_HEIGHT / 2.0),
            size: 12.0.into(),
            color: Color::from_rgb(0.75, 0.75, 0.75),
            align_x: Horizontal::Left.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    } else {
        // Show "No track" for empty decks
        frame.fill_text(Text {
            content: "No track".to_string(),
            position: Point::new(name_x, y + DECK_HEADER_HEIGHT / 2.0),
            size: 11.0.into(),
            color: Color::from_rgb(0.4, 0.4, 0.4),
            align_x: Horizontal::Left.into(),
            align_y: Vertical::Center.into(),
            ..Text::default()
        });
    }

    // Draw zoomed waveform below header
    let zoomed_y = y + DECK_HEADER_HEIGHT;
    draw_zoomed_at(
        frame,
        &deck.zoomed,
        playhead,
        x,
        zoomed_y,
        width,
        is_master,
    );

    // Draw overview waveform below zoomed
    let overview_y = zoomed_y + ZOOMED_WAVEFORM_HEIGHT + DECK_INTERNAL_GAP;

    // Draw stem status indicators on left side of zoomed waveform only
    // Stem colors: Vocals=cyan, Drums=yellow, Bass=magenta, Other=green
    let stem_colors = [
        Color::from_rgb(0.0, 0.8, 0.8),  // Vocals - cyan
        Color::from_rgb(0.9, 0.9, 0.2),  // Drums - yellow
        Color::from_rgb(0.9, 0.3, 0.9),  // Bass - magenta
        Color::from_rgb(0.3, 0.9, 0.3),  // Other - green
    ];

    // Calculate indicator height to fit within zoomed waveform only
    let indicator_height = (ZOOMED_WAVEFORM_HEIGHT - (STEM_INDICATOR_GAP * 3.0)) / 4.0;

    for (stem_idx, &color) in stem_colors.iter().enumerate() {
        let indicator_y = zoomed_y + (stem_idx as f32) * (indicator_height + STEM_INDICATOR_GAP);

        // Simple bypass toggle: 50% brightness if active, dark if bypassed
        let indicator_color = if stem_active[stem_idx] {
            // Active: 50% brightness of stem color
            Color::from_rgb(
                color.r * 0.5,
                color.g * 0.5,
                color.b * 0.5,
            )
        } else {
            // Bypassed: dark/off
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
    );
}

/// Draw a zoomed waveform at a specific position
fn draw_zoomed_at(
    frame: &mut Frame,
    zoomed: &ZoomedState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
    is_master: bool,
) {
    let height = ZOOMED_WAVEFORM_HEIGHT;
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

    let (view_start, view_end) = zoomed.visible_range(playhead);
    let view_samples = (view_end - view_start) as f64;

    if view_samples > 0.0 {
        // Draw loop region (behind everything else)
        if let Some((loop_start_norm, loop_end_norm)) = zoomed.loop_region {
            let loop_start_sample = (loop_start_norm * zoomed.duration_samples as f64) as u64;
            let loop_end_sample = (loop_end_norm * zoomed.duration_samples as f64) as u64;

            if loop_end_sample > view_start && loop_start_sample < view_end {
                let start_x = if loop_start_sample <= view_start {
                    x
                } else {
                    x + ((loop_start_sample - view_start) as f64 / view_samples * width as f64) as f32
                };
                let end_x = if loop_end_sample >= view_end {
                    x + width
                } else {
                    x + ((loop_end_sample - view_start) as f64 / view_samples * width as f64) as f32
                };

                let loop_width = end_x - start_x;
                if loop_width > 0.0 {
                    frame.fill_rectangle(
                        Point::new(start_x, y),
                        Size::new(loop_width, height),
                        Color::from_rgba(0.2, 0.8, 0.2, 0.25),
                    );
                    if loop_start_sample > view_start && loop_start_sample < view_end {
                        frame.stroke(
                            &Path::line(Point::new(start_x, y), Point::new(start_x, y + height)),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                    if loop_end_sample > view_start && loop_end_sample < view_end {
                        frame.stroke(
                            &Path::line(Point::new(end_x, y), Point::new(end_x, y + height)),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                }
            }
        }

        // Draw slicer region (orange overlay with slice divisions)
        if let Some((slicer_start_norm, slicer_end_norm)) = zoomed.slicer_region {
            let slicer_start_sample = (slicer_start_norm * zoomed.duration_samples as f64) as u64;
            let slicer_end_sample = (slicer_end_norm * zoomed.duration_samples as f64) as u64;

            if slicer_end_sample > view_start && slicer_start_sample < view_end {
                let start_x = if slicer_start_sample <= view_start {
                    x
                } else {
                    x + ((slicer_start_sample - view_start) as f64 / view_samples * width as f64) as f32
                };
                let end_x = if slicer_end_sample >= view_end {
                    x + width
                } else {
                    x + ((slicer_end_sample - view_start) as f64 / view_samples * width as f64) as f32
                };

                let slicer_width = end_x - start_x;
                if slicer_width > 0.0 {
                    // Orange overlay for slicer buffer
                    frame.fill_rectangle(
                        Point::new(start_x, y),
                        Size::new(slicer_width, height),
                        Color::from_rgba(1.0, 0.5, 0.0, 0.12),
                    );

                    // Draw slice divisions (if they fit in view)
                    let total_slicer_samples = (slicer_end_sample - slicer_start_sample) as f64;
                    let samples_per_slice = total_slicer_samples / SLICER_NUM_SLICES as f64;

                    for i in 0..=SLICER_NUM_SLICES {
                        let slice_sample = slicer_start_sample as f64 + samples_per_slice * i as f64;
                        let slice_sample_u64 = slice_sample as u64;

                        if slice_sample_u64 >= view_start && slice_sample_u64 <= view_end {
                            let slice_x = x + ((slice_sample_u64 - view_start) as f64 / view_samples * width as f64) as f32;
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
                        let slice_start_sample = slicer_start_sample as f64 + samples_per_slice * current as f64;
                        let slice_end_sample = slice_start_sample + samples_per_slice;

                        if (slice_end_sample as u64) > view_start && (slice_start_sample as u64) < view_end {
                            let slice_start_x = if (slice_start_sample as u64) <= view_start {
                                x
                            } else {
                                x + ((slice_start_sample as u64 - view_start) as f64 / view_samples * width as f64) as f32
                            };
                            let slice_end_x = if (slice_end_sample as u64) >= view_end {
                                x + width
                            } else {
                                x + ((slice_end_sample as u64 - view_start) as f64 / view_samples * width as f64) as f32
                            };

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

        // Draw beat markers
        for &beat_sample in &zoomed.beat_grid {
            if beat_sample >= view_start && beat_sample <= view_end {
                let beat_x = x + ((beat_sample - view_start) as f64 / view_samples * width as f64) as f32;
                let beat_idx = zoomed.beat_grid.iter().position(|&b| b == beat_sample).unwrap_or(0);
                let (color, w) = if beat_idx % 4 == 0 {
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

        // Draw cached peaks using filled paths
        if !zoomed.cached_peaks[0].is_empty() {
            let cache_start = zoomed.cache_start;
            let cache_end = zoomed.cache_end;
            let cache_samples = (cache_end - cache_start) as f64;
            let height_scale = height / 2.0 * 0.85;

            for stem_idx in (0..4).rev() {
                let peaks = &zoomed.cached_peaks[stem_idx];
                if peaks.is_empty() {
                    continue;
                }
                let peaks_len = peaks.len() as f64;
                let base_color = STEM_COLORS[stem_idx];
                let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.7);

                // Build filled path for this stem
                let path = Path::new(|builder| {
                    let mut first_point = true;
                    let mut upper_points: Vec<(f32, f32)> = Vec::with_capacity(width as usize);
                    let mut lower_points: Vec<(f32, f32)> = Vec::with_capacity(width as usize);

                    for px in 0..(width as usize) {
                        let view_sample = view_start as f64 + (px as f64 / width as f64) * view_samples;
                        if view_sample < cache_start as f64 || view_sample > cache_end as f64 {
                            continue;
                        }
                        let cache_offset = view_sample - cache_start as f64;
                        let cache_idx = (cache_offset / cache_samples * peaks_len) as usize;
                        if cache_idx >= peaks.len() {
                            continue;
                        }

                        let (min, max) = peaks[cache_idx];
                        let y_max = center_y - (max * height_scale);
                        let y_min = center_y - (min * height_scale);

                        upper_points.push((x + px as f32, y_max));
                        lower_points.push((x + px as f32, y_min));
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

        // Draw cue markers
        for marker in &zoomed.cue_markers {
            let marker_sample = (marker.position * zoomed.duration_samples as f64) as u64;
            if marker_sample >= view_start && marker_sample <= view_end {
                let cue_x = x + ((marker_sample - view_start) as f64 / view_samples * width as f64) as f32;
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
    }

    // Draw playhead - position depends on view mode
    let playhead_x = match zoomed.view_mode() {
        ZoomedViewMode::Scrolling => {
            // Scrolling mode: playhead fixed at center
            x + width / 2.0
        }
        ZoomedViewMode::FixedBuffer => {
            // Fixed buffer mode: playhead moves within view
            let (view_start, view_end) = zoomed.visible_range(playhead);
            let view_samples = (view_end - view_start) as f64;
            if view_samples > 0.0 && playhead >= view_start && playhead <= view_end {
                let offset = (playhead - view_start) as f64;
                x + (offset / view_samples * width as f64) as f32
            } else {
                // Playhead outside view - clamp to edges
                if playhead < view_start {
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
fn draw_overview_at(
    frame: &mut Frame,
    overview: &OverviewState,
    playhead: u64,
    x: f32,
    y: f32,
    width: f32,
) {
    let height = WAVEFORM_HEIGHT;
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
        let start_x = x + (slicer_start * width as f64) as f32;
        let end_x = x + (slicer_end * width as f64) as f32;
        let slicer_width = end_x - start_x;
        if slicer_width > 0.0 {
            // Orange overlay for slicer buffer
            frame.fill_rectangle(
                Point::new(start_x, y),
                Size::new(slicer_width, height),
                Color::from_rgba(1.0, 0.5, 0.0, 0.15),
            );

            // Draw slice divisions
            let slice_width = slicer_width / SLICER_NUM_SLICES as f32;
            for i in 0..=SLICER_NUM_SLICES {
                let slice_x = start_x + slice_width * i as f32;
                let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
                let line_width = if is_boundary { 2.0 } else { 1.0 };
                let alpha = if is_boundary { 0.8 } else { 0.4 };

                frame.stroke(
                    &Path::line(Point::new(slice_x, y), Point::new(slice_x, y + height)),
                    Stroke::default()
                        .with_color(Color::from_rgba(1.0, 0.6, 0.1, alpha))
                        .with_width(line_width),
                );
            }

            // Highlight current playing slice with brighter overlay
            if let Some(current) = overview.slicer_current_slice {
                let slice_x = start_x + slice_width * current as f32;
                frame.fill_rectangle(
                    Point::new(slice_x, y),
                    Size::new(slice_width, height),
                    Color::from_rgba(1.0, 0.6, 0.0, 0.25),
                );
            }
        }
    }

    // Draw beat markers with configurable density
    let step = (overview.grid_bars * 4) as usize;
    for (i, &beat_pos) in overview.beat_markers.iter().enumerate() {
        if i % step != 0 {
            continue;
        }
        let beat_x = x + (beat_pos * width as f64) as f32;
        let (color, line_height) = if (i / step) % 4 == 0 {
            (Color::from_rgba(1.0, 0.3, 0.3, 0.6), height)
        } else {
            (Color::from_rgba(0.5, 0.5, 0.5, 0.4), height * 0.5)
        };
        frame.stroke(
            &Path::line(
                Point::new(beat_x, y + (height - line_height) / 2.0),
                Point::new(beat_x, y + (height + line_height) / 2.0),
            ),
            Stroke::default().with_color(color).with_width(1.0),
        );
    }

    // Draw stem waveforms using filled paths
    let height_scale = height / 2.0 * 0.85;
    for stem_idx in (0..4).rev() {
        let stem_peaks = &overview.stem_waveforms[stem_idx];
        if stem_peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.6);

        draw_stem_waveform_filled(frame, stem_peaks, x, center_y, height_scale, waveform_color, width);
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
