//! Canvas Program implementations for waveform rendering
//!
//! These implement the iced canvas `Program` trait for custom waveform drawing.
//! Each canvas type takes callback closures for event handling, following
//! idiomatic iced 0.14 patterns.

use super::state::{
    CombinedState, OverviewState, ZoomedState,
    COMBINED_WAVEFORM_GAP, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};
use crate::{STEM_COLORS, CueMarker};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{mouse, Color, Point, Rectangle, Size, Theme};

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

        // Draw zoom indicator bar in corner
        let indicator_width = (self.state.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * 60.0;
        frame.fill_rectangle(
            Point::new(width - 70.0, 5.0),
            Size::new(indicator_width, 4.0),
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

/// Draw stem waveforms from peak data
fn draw_stem_waveforms(
    frame: &mut Frame,
    stem_waveforms: &[Vec<(f32, f32)>; 4],
    width: f32,
    _height: f32,
    center_y: f32,
    alpha: f32,
) {
    for stem_idx in (0..4).rev() {
        let waveform_data = &stem_waveforms[stem_idx];
        if waveform_data.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, alpha);

        let data_width = waveform_data.len();
        let scale = data_width as f32 / width;

        for x in 0..(width as usize) {
            let data_idx = ((x as f32) * scale) as usize;
            if data_idx >= data_width {
                break;
            }

            let (min, max) = waveform_data[data_idx];
            let y1 = center_y - (max * center_y * 0.9);
            let y2 = center_y - (min * center_y * 0.9);

            frame.stroke(
                &Path::line(Point::new(x as f32, y1), Point::new(x as f32, y2)),
                Stroke::default().with_color(waveform_color).with_width(1.0),
            );
        }
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

/// Draw cached peaks for zoomed view
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

    for stem_idx in (0..4).rev() {
        let peaks = &state.cached_peaks[stem_idx];
        if peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.7);
        let peaks_len = peaks.len() as f64;

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
            let y1 = center_y - (max * center_y * 0.85);
            let y2 = center_y - (min * center_y * 0.85);

            frame.stroke(
                &Path::line(Point::new(x as f32, y1), Point::new(x as f32, y2)),
                Stroke::default().with_color(waveform_color).with_width(1.0),
            );
        }
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

    // Draw zoom indicator
    let indicator_width = (zoomed.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * 60.0;
    frame.fill_rectangle(
        Point::new(width - 70.0, 5.0),
        Size::new(indicator_width, 4.0),
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

    // Draw stem waveforms
    for stem_idx in (0..4).rev() {
        let stem_peaks = &overview.stem_waveforms[stem_idx];
        if stem_peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.6);
        let peaks_len = stem_peaks.len() as f32;

        for x in 0..(width as usize) {
            let peak_idx = (x as f32 / width * peaks_len) as usize;
            if peak_idx >= stem_peaks.len() {
                continue;
            }

            let (min, max) = stem_peaks[peak_idx];
            let y1 = overview_center_y - (max * overview_height / 2.0 * 0.85);
            let y2 = overview_center_y - (min * overview_height / 2.0 * 0.85);

            frame.stroke(
                &Path::line(Point::new(x as f32, y1), Point::new(x as f32, y2)),
                Stroke::default().with_color(waveform_color).with_width(1.0),
            );
        }
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
        let cue_color = Color::from_rgb(1.0, 0.5, 0.0);
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
