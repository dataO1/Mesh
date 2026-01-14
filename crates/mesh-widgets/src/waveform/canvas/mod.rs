//! Canvas Program implementations for waveform rendering
//!
//! These implement the iced canvas `Program` trait for custom waveform drawing.
//! Each canvas type takes callback closures for event handling, following
//! idiomatic iced 0.14 patterns.

mod player;

pub use player::{
    PlayerCanvas, PlayerInteraction,
    DECK_CELL_HEIGHT, DECK_GRID_GAP, DECK_INTERNAL_GAP,
    OVERVIEW_STACK_GAP, PLAYER_SECTION_GAP, ZOOMED_GRID_GAP,
};

use super::peak_computation::WindowInfo;
use super::state::{
    CombinedState, OverviewState, ZoomedState, ZoomedViewMode,
    COMBINED_WAVEFORM_GAP, MAX_ZOOM_BARS, MIN_ZOOM_BARS,
    WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT, ZOOM_PIXELS_PER_LEVEL,
};
use crate::{STEM_COLORS, CueMarker};
use iced::widget::canvas::{self, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{mouse, Color, Point, Rectangle, Size, Theme};

/// Stem rendering order (back to front): Drums, Bass, Vocals, Other
/// Drums drawn first (behind), Other drawn last (on top)
/// Index mapping: 0=Vocals, 1=Drums, 2=Bass, 3=Other
pub(super) const STEM_RENDER_ORDER: [usize; 4] = [1, 2, 0, 3];

/// Stem indicator order (top to bottom): Other, Vocals, Bass, Drums
/// This is the reverse of STEM_RENDER_ORDER, matching visual layering
/// (bottom indicator = back layer, top indicator = front layer)
pub(super) const STEM_INDICATOR_ORDER: [usize; 4] = [3, 0, 2, 1];

/// Gray colors for inactive stems (brightness matches Natural palette relationships)
/// Index: 0=Vocals, 1=Drums, 2=Bass, 3=Other
pub(super) const INACTIVE_STEM_GRAYS: [Color; 4] = [
    Color::from_rgb(0.40, 0.40, 0.40), // Vocals - medium-bright gray
    Color::from_rgb(0.30, 0.30, 0.30), // Drums - medium gray
    Color::from_rgb(0.35, 0.35, 0.35), // Bass - medium gray
    Color::from_rgb(0.45, 0.45, 0.45), // Other - lightest gray
];

// =============================================================================
// Stem-Specific Subsampling Configuration
// =============================================================================
// Drums need most detail (transients), Bass needs least (smooth low freq),
// Vocals and Other in between.
// Index: 0=Vocals, 1=Drums, 2=Bass, 3=Other

/// Max segments for overview waveform per stem type
/// Lower = more subsampling (coarser), Higher = less subsampling (finer detail)
const OVERVIEW_MAX_SEGMENTS: [usize; 4] = [
    400,  // Vocals - medium
    600,  // Drums - most detail (transients)
    250,  // Bass - most subsampling (smooth)
    400,  // Other - medium
];

/// Max segments for zoomed waveform (cached peaks) per stem type
const ZOOMED_MAX_SEGMENTS: [usize; 4] = [
    50000, // Vocals - no subsampling
    50000, // Drums - no subsampling
    800,   // Bass - light subsampling
    50000, // Other - no subsampling
];

/// Target pixels per point for highres zoomed rendering per stem type
/// Lower = more detail, Higher = more subsampling
const HIGHRES_PIXELS_PER_POINT: [f64; 4] = [
    1.0, // Vocals - no subsampling
    1.0, // Drums - no subsampling
    2.5, // Bass - light subsampling
    1.0, // Other - no subsampling
];

/// Gaussian smoothing radius multiplier per stem
/// Index: 0=Vocals, 1=Drums, 2=Bass, 3=Other
const SMOOTH_RADIUS_MULTIPLIER: [f64; 4] = [
    0.25, // Vocals
    0.1,  // Drums - light smoothing
    0.4,  // Bass - more smoothing
    0.4,  // Other - more smoothing
];

/// Waveform alpha (opacity) values
/// Higher values = more opaque/solid waveforms
pub(super) const OVERVIEW_WAVEFORM_ALPHA: f32 = 0.85;
pub(super) const ZOOMED_WAVEFORM_ALPHA: f32 = 0.9;

/// Get step size for overview waveform given stem index and width
#[inline]
fn overview_step(stem_idx: usize, width: usize) -> usize {
    1.max(width / OVERVIEW_MAX_SEGMENTS[stem_idx])
}

/// Get step size for zoomed waveform given stem index and width
#[inline]
pub(super) fn zoomed_step(stem_idx: usize, width: usize) -> usize {
    1.max(width / ZOOMED_MAX_SEGMENTS[stem_idx])
}

/// Get target pixels per point for highres rendering given stem index
#[inline]
pub(super) fn highres_target_pixels(stem_idx: usize) -> f64 {
    HIGHRES_PIXELS_PER_POINT[stem_idx]
}

/// Get smoothing radius for a given stem and step size
#[inline]
pub(super) fn smooth_radius_for_stem(stem_idx: usize, step: usize) -> usize {
    (step as f64 * SMOOTH_RADIUS_MULTIPLIER[stem_idx]).round() as usize
}

/// Compute Gaussian weight for distance from center
/// Uses sigma = radius / 2 for good falloff within the window
#[inline]
fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    (-0.5 * (distance / sigma).powi(2)).exp()
}

/// Sample a peak with Gaussian smoothing
///
/// Applies Gaussian-weighted average over the smoothing window.
/// Center samples contribute more than edge samples, giving smooth
/// results while preserving peak character better than box averaging.
#[inline]
pub(super) fn sample_peak_smoothed(
    peaks: &[(f32, f32)],
    peak_idx: usize,
    smooth_radius: usize,
    _stem_idx: usize,
) -> (f32, f32) {
    if smooth_radius == 0 {
        return peaks[peak_idx];
    }

    let peaks_len = peaks.len();
    let window_start = peak_idx.saturating_sub(smooth_radius);
    let window_end = (peak_idx + smooth_radius + 1).min(peaks_len);

    // Gaussian blur: weight samples by distance from center
    let sigma = (smooth_radius as f32) / 2.0;
    let mut min_sum = 0.0f32;
    let mut max_sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for i in window_start..window_end {
        let distance = (i as i32 - peak_idx as i32).abs() as f32;
        let weight = gaussian_weight(distance, sigma);
        min_sum += peaks[i].0 * weight;
        max_sum += peaks[i].1 * weight;
        weight_sum += weight;
    }

    (min_sum / weight_sum, max_sum / weight_sum)
}

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

// PlayerInteraction and layout constants are in player.rs

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
        draw_stem_waveforms(&mut frame, &self.state.stem_waveforms, width, height, center_y, OVERVIEW_WAVEFORM_ALPHA);

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
        // In FixedBuffer mode (slicer), zoom is locked - always show entire buffer
        if self.state.view_mode == ZoomedViewMode::FixedBuffer {
            return None;
        }

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

        // Get window with padding info for proper boundary handling
        let window = self.state.visible_window(self.playhead);

        if window.total_samples == 0 {
            return vec![frame.into_geometry()];
        }

        // Draw beat markers (uses WindowInfo for proper positioning with padding)
        draw_beat_markers_zoomed(&mut frame, &self.state.beat_grid, &window, width, height);

        // Draw stem waveforms from cached peaks (uses WindowInfo for padding)
        draw_cached_peaks(&mut frame, self.state, &window, width, center_y);

        // Draw cue markers (uses WindowInfo for proper positioning with padding)
        draw_cue_markers_zoomed(&mut frame, &self.state.cue_markers, self.state.duration_samples, &window, width, height);

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

        // Handle zoom gestures in zoomed region (disabled in FixedBuffer mode)
        let zoom_enabled = self.state.zoomed.view_mode != ZoomedViewMode::FixedBuffer;
        if zoom_enabled {
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
        draw_zoomed_section(&mut frame, &self.state.zoomed, &self.state.overview, self.playhead, width);

        // =====================================================================
        // OVERVIEW WAVEFORM (bottom section)
        // =====================================================================
        draw_overview_section(
            &mut frame,
            &self.state.overview,
            self.playhead,
            width,
            &self.state.stem_active,
            &self.state.linked_stems,
            &self.state.linked_active,
        );

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
pub(super) fn draw_stem_waveform_filled(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    width: f32,
    stem_idx: usize,
) {
    if peaks.is_empty() || width < 2.0 {
        return;
    }

    let peaks_len = peaks.len() as f32;

    // Stem-specific subsampling (drums=detail, bass=smooth)
    let step = overview_step(stem_idx, width as usize);

    let path = Path::new(|builder| {
        // Start at first point's max value (upper envelope)
        let (_, first_max) = peaks[0];
        let first_y = center_y - (first_max * height_scale);
        builder.move_to(Point::new(x_offset, first_y));

        // Draw upper envelope left to right (with step)
        let mut px = step;
        while px < width as usize {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx >= peaks.len() {
                break;
            }
            let (_, max) = peaks[peak_idx];
            let x = x_offset + px as f32;
            let y = center_y - (max * height_scale);
            builder.line_to(Point::new(x, y));
            px += step;
        }
        // Always include the last point
        if peaks.len() > 1 {
            let (_, last_max) = peaks[peaks.len() - 1];
            builder.line_to(Point::new(x_offset + width - 1.0, center_y - (last_max * height_scale)));
        }

        // Draw lower envelope right to left (with step)
        let mut px = (width as usize).saturating_sub(1);
        while px > 0 {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx < peaks.len() {
                let (min, _) = peaks[peak_idx];
                let x = x_offset + px as f32;
                let y = center_y - (min * height_scale);
                builder.line_to(Point::new(x, y));
            }
            px = px.saturating_sub(step);
        }
        // Close at start
        let (first_min, _) = peaks[0];
        builder.line_to(Point::new(x_offset, center_y - (first_min * height_scale)));

        builder.close();
    });

    frame.fill(&path, color);
}

/// Draw a linked stem waveform with alignment offset and duration scaling
///
/// This handles the case where the linked track has a different duration than the host.
/// The waveform is scaled proportionally and offset so drop markers align.
fn draw_stem_waveform_aligned(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    base_x: f32,
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    visible_width: f32,
    linked_duration: u64,
    host_duration: u64,
    stem_idx: usize,
) {
    if peaks.is_empty() || visible_width < 2.0 || host_duration == 0 {
        return;
    }

    // Calculate how linked waveform scales relative to host
    let duration_ratio = linked_duration as f64 / host_duration as f64;
    let linked_render_width = (visible_width as f64 * duration_ratio) as f32;
    let peaks_len = peaks.len() as f32;
    let step = overview_step(stem_idx, linked_render_width as usize);

    let path = Path::new(|builder| {
        let mut started = false;
        let mut upper_points: Vec<Point> = Vec::new();
        let mut lower_points: Vec<Point> = Vec::new();

        // Collect visible points for upper and lower envelopes (with step)
        let mut px = 0;
        while px < linked_render_width as usize {
            let actual_x = base_x + x_offset + px as f32;

            // Skip if outside visible bounds
            if actual_x >= base_x && actual_x <= base_x + visible_width {
                let peak_idx = ((px as f32 / linked_render_width) * peaks_len) as usize;
                if peak_idx >= peaks.len() {
                    break;
                }

                let (min, max) = peaks[peak_idx];
                upper_points.push(Point::new(actual_x, center_y - (max * height_scale)));
                lower_points.push(Point::new(actual_x, center_y - (min * height_scale)));
            }
            px += step;
        }

        if upper_points.is_empty() {
            return;
        }

        // Draw upper envelope
        builder.move_to(upper_points[0]);
        started = true;
        for point in upper_points.iter().skip(1) {
            builder.line_to(*point);
        }

        // Draw lower envelope in reverse
        for point in lower_points.iter().rev() {
            builder.line_to(*point);
        }

        if started {
            builder.close();
        }
    });

    frame.fill(&path, color);
}

/// Draw only the upper envelope (max values) of a waveform - for split view top half
///
/// Draws from center_y upward, creating a half-waveform that meets the bottom half at center_y
pub(super) fn draw_stem_waveform_upper(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    width: f32,
    stem_idx: usize,
) {
    if peaks.is_empty() || width < 2.0 {
        return;
    }

    let peaks_len = peaks.len() as f32;
    let step = overview_step(stem_idx, width as usize);

    let path = Path::new(|builder| {
        // Start at center line
        builder.move_to(Point::new(x_offset, center_y));

        // Draw upper envelope left to right (max values go UP from center)
        let mut px = 0;
        while px < width as usize {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx >= peaks.len() {
                break;
            }
            let (_, max) = peaks[peak_idx];
            let x = x_offset + px as f32;
            let y = center_y - (max * height_scale);
            builder.line_to(Point::new(x, y));
            px += step;
        }

        // Return along center line (right to left)
        builder.line_to(Point::new(x_offset + width - 1.0, center_y));
        builder.close();
    });

    frame.fill(&path, color);
}

/// Draw only the lower envelope (min values) of a waveform - for split view bottom half
///
/// Draws from center_y downward, creating a half-waveform that meets the top half at center_y
pub(super) fn draw_stem_waveform_lower(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    width: f32,
    stem_idx: usize,
) {
    if peaks.is_empty() || width < 2.0 {
        return;
    }

    let peaks_len = peaks.len() as f32;
    let step = overview_step(stem_idx, width as usize);

    let path = Path::new(|builder| {
        // Start at center line
        builder.move_to(Point::new(x_offset, center_y));

        // Draw lower envelope left to right (min values go DOWN from center)
        let mut px = 0;
        while px < width as usize {
            let peak_idx = ((px as f32 / width) * peaks_len) as usize;
            if peak_idx >= peaks.len() {
                break;
            }
            let (min, _) = peaks[peak_idx];
            let x = x_offset + px as f32;
            let y = center_y - (min * height_scale); // min is negative, so this goes DOWN
            builder.line_to(Point::new(x, y));
            px += step;
        }

        // Return along center line (right to left)
        builder.line_to(Point::new(x_offset + width - 1.0, center_y));
        builder.close();
    });

    frame.fill(&path, color);
}

/// Draw only upper envelope with alignment offset for linked stems
pub(super) fn draw_stem_waveform_upper_aligned(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    base_x: f32,
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    visible_width: f32,
    linked_duration: u64,
    host_duration: u64,
    stem_idx: usize,
) {
    if peaks.is_empty() || visible_width < 2.0 || host_duration == 0 {
        return;
    }

    let duration_ratio = linked_duration as f64 / host_duration as f64;
    let linked_render_width = (visible_width as f64 * duration_ratio) as f32;
    let peaks_len = peaks.len() as f32;
    let step = overview_step(stem_idx, linked_render_width as usize);

    let path = Path::new(|builder| {
        let mut started = false;
        let mut last_x = base_x;

        // Draw upper envelope with clipping (with step)
        let mut px = 0;
        while px < linked_render_width as usize {
            let actual_x = base_x + x_offset + px as f32;
            if actual_x >= base_x && actual_x <= base_x + visible_width {
                let peak_idx = ((px as f32 / linked_render_width) * peaks_len) as usize;
                if peak_idx < peaks.len() {
                    let (_, max) = peaks[peak_idx];
                    let y = center_y - (max * height_scale);

                    if !started {
                        builder.move_to(Point::new(actual_x, center_y));
                        builder.line_to(Point::new(actual_x, y));
                        started = true;
                    } else {
                        builder.line_to(Point::new(actual_x, y));
                    }
                    last_x = actual_x;
                }
            }
            px += step;
        }

        if started {
            builder.line_to(Point::new(last_x, center_y));
            builder.close();
        }
    });

    frame.fill(&path, color);
}

/// Draw only lower envelope with alignment offset for linked stems
pub(super) fn draw_stem_waveform_lower_aligned(
    frame: &mut Frame,
    peaks: &[(f32, f32)],
    base_x: f32,
    x_offset: f32,
    center_y: f32,
    height_scale: f32,
    color: Color,
    visible_width: f32,
    linked_duration: u64,
    host_duration: u64,
    stem_idx: usize,
) {
    if peaks.is_empty() || visible_width < 2.0 || host_duration == 0 {
        return;
    }

    let duration_ratio = linked_duration as f64 / host_duration as f64;
    let linked_render_width = (visible_width as f64 * duration_ratio) as f32;
    let peaks_len = peaks.len() as f32;
    let step = overview_step(stem_idx, linked_render_width as usize);

    let path = Path::new(|builder| {
        let mut started = false;
        let mut last_x = base_x;

        // Draw lower envelope with clipping (with step)
        let mut px = 0;
        while px < linked_render_width as usize {
            let actual_x = base_x + x_offset + px as f32;
            if actual_x >= base_x && actual_x <= base_x + visible_width {
                let peak_idx = ((px as f32 / linked_render_width) * peaks_len) as usize;
                if peak_idx < peaks.len() {
                    let (min, _) = peaks[peak_idx];
                    let y = center_y - (min * height_scale);

                    if !started {
                        builder.move_to(Point::new(actual_x, center_y));
                        builder.line_to(Point::new(actual_x, y));
                        started = true;
                    } else {
                        builder.line_to(Point::new(actual_x, y));
                    }
                    last_x = actual_x;
                }
            }
            px += step;
        }

        if started {
            builder.line_to(Point::new(last_x, center_y));
            builder.close();
        }
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
    // Draw stems in layered order: Drums (back) → Bass → Vocals → Other (front)
    for &stem_idx in STEM_RENDER_ORDER.iter() {
        let waveform_data = &stem_waveforms[stem_idx];
        if waveform_data.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, alpha);
        let height_scale = center_y * 0.9;

        draw_stem_waveform_filled(frame, waveform_data, 0.0, center_y, height_scale, waveform_color, width, stem_idx);
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

/// Orange color for drop marker
pub(super) const DROP_MARKER_COLOR: Color = Color::from_rgb(1.0, 0.5, 0.0);

/// Draw drop marker (orange diamond shape)
///
/// The drop marker indicates the structural reference point for linked stem alignment.
/// It's displayed as an orange diamond at the top of the waveform.
fn draw_drop_marker(
    frame: &mut Frame,
    drop_marker: Option<u64>,
    duration_samples: u64,
    width: f32,
    height: f32,
    y_offset: f32,
) {
    if let Some(drop_sample) = drop_marker {
        if duration_samples == 0 {
            return;
        }

        let x = (drop_sample as f64 / duration_samples as f64 * width as f64) as f32;

        // Draw vertical line (thinner than cue markers)
        frame.fill_rectangle(
            Point::new(x - 1.0, y_offset),
            Size::new(2.0, height),
            DROP_MARKER_COLOR,
        );

        // Draw diamond shape at top
        let diamond = Path::new(|builder| {
            builder.move_to(Point::new(x, y_offset));          // Top point
            builder.line_to(Point::new(x - 6.0, y_offset + 8.0)); // Left point
            builder.line_to(Point::new(x, y_offset + 16.0));      // Bottom point
            builder.line_to(Point::new(x + 6.0, y_offset + 8.0)); // Right point
            builder.close();
        });
        frame.fill(&diamond, DROP_MARKER_COLOR);
    }
}

/// Draw drop marker for zoomed view (using window coordinates)
fn draw_drop_marker_zoomed(
    frame: &mut Frame,
    drop_marker: Option<u64>,
    window: &WindowInfo,
    width: f32,
    height: f32,
) {
    if let Some(drop_sample) = drop_marker {
        // Only draw if within visible window
        if drop_sample < window.start || drop_sample > window.end {
            return;
        }

        // Calculate x position accounting for left_padding
        let offset_from_virtual_start = window.left_padding + (drop_sample - window.start);
        let x = (offset_from_virtual_start as f64 / window.total_samples as f64 * width as f64) as f32;

        // Draw vertical line
        frame.fill_rectangle(
            Point::new(x - 1.0, 0.0),
            Size::new(2.0, height),
            DROP_MARKER_COLOR,
        );

        // Draw diamond shape at top
        let diamond = Path::new(|builder| {
            builder.move_to(Point::new(x, 0.0));              // Top point
            builder.line_to(Point::new(x - 8.0, 10.0));       // Left point
            builder.line_to(Point::new(x, 20.0));             // Bottom point
            builder.line_to(Point::new(x + 8.0, 10.0));       // Right point
            builder.close();
        });
        frame.fill(&diamond, DROP_MARKER_COLOR);
    }
}

/// Draw beat markers for zoomed view
fn draw_beat_markers_zoomed(
    frame: &mut Frame,
    beat_grid: &[u64],
    window: &WindowInfo,
    width: f32,
    height: f32,
) {
    for (i, &beat_sample) in beat_grid.iter().enumerate() {
        // Only draw beats within actual audio range
        if beat_sample < window.start || beat_sample > window.end {
            continue;
        }

        // Calculate x position accounting for left_padding
        // Sample at window.start should appear at x = (left_padding / total_samples) * width
        let offset_from_virtual_start = window.left_padding + (beat_sample - window.start);
        let x = (offset_from_virtual_start as f64 / window.total_samples as f64 * width as f64) as f32;

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
///
/// Uses WindowInfo to properly handle track boundary padding:
/// - When playhead is at track start, left half of view shows silence
/// - When playhead is at track end, right half of view shows silence
/// - Playhead is always visually centered
fn draw_cached_peaks(
    frame: &mut Frame,
    state: &ZoomedState,
    window: &WindowInfo,
    width: f32,
    center_y: f32,
) {
    if state.cache_end <= state.cache_start && state.cache_left_padding == 0 {
        return;
    }

    // Cache includes left_padding worth of virtual samples (zeros before track start)
    let cache_virtual_total = (state.cache_end - state.cache_start + state.cache_left_padding) as usize;
    let height_scale = center_y * 0.85;

    // Draw stems in layered order: Drums (back) → Bass → Vocals → Other (front)
    for &stem_idx in STEM_RENDER_ORDER.iter() {
        let peaks = &state.cached_peaks[stem_idx];
        if peaks.is_empty() {
            continue;
        }

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, ZOOMED_WAVEFORM_ALPHA);
        let peaks_len = peaks.len();
        let width_usize = width as usize;
        let total_samples = window.total_samples as usize;
        let step = zoomed_step(stem_idx, width_usize);
        let smooth_radius = smooth_radius_for_stem(stem_idx, step);

        // Build filled path for this stem
        let path = Path::new(|builder| {
            let mut first_point = true;
            let mut upper_points: Vec<(f32, f32)> = Vec::with_capacity(width_usize / step + 2);
            let mut lower_points: Vec<(f32, f32)> = Vec::with_capacity(width_usize / step + 2);

            let mut x = 0;
            while x < width_usize {
                // Bresenham-style integer division: x * total_samples / width
                // This distributes remainder evenly with no floating-point
                let window_offset = x * total_samples / width_usize;

                // Convert window offset to actual sample position
                // window_offset 0 = left edge (may be before track start)
                // window_offset left_padding = track start (actual sample window.start)
                let actual_sample = window.start as i64 - window.left_padding as i64 + window_offset as i64;

                // Convert actual sample to cache virtual offset
                // Cache virtual offset = actual_sample - cache_start + cache_left_padding
                let cache_virtual_offset = actual_sample - state.cache_start as i64 + state.cache_left_padding as i64;

                // Increment before potential continue
                let current_x = x;
                x += step;

                if cache_virtual_offset < 0 || cache_virtual_offset as usize >= cache_virtual_total {
                    continue;
                }

                // Bresenham-style cache index: offset * peaks_len / total
                let cache_idx = (cache_virtual_offset as usize * peaks_len) / cache_virtual_total;
                if cache_idx >= peaks.len() {
                    continue;
                }

                let (min, max) = sample_peak_smoothed(peaks, cache_idx, smooth_radius, stem_idx);
                let y_max = center_y - (max * height_scale);
                let y_min = center_y - (min * height_scale);

                upper_points.push((current_x as f32, y_max));
                lower_points.push((current_x as f32, y_min));
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

/// Draw highres peaks for zoomed section (CombinedCanvas)
///
/// Uses pre-computed highres_peaks for smooth, jitter-free rendering.
/// Includes adaptive subsampling and grid alignment for stable visuals.
fn draw_highres_peaks_section(
    frame: &mut Frame,
    highres_peaks: &[Vec<(f32, f32)>; 4],
    duration_samples: u64,
    window: &WindowInfo,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    let center_y = y + height / 2.0;
    let height_scale = height / 2.0 * 0.85;

    // Draw stems in layered order: Drums (back) → Bass → Vocals → Other (front)
    for &stem_idx in STEM_RENDER_ORDER.iter() {
        let peaks = &highres_peaks[stem_idx];
        if peaks.is_empty() {
            continue;
        }
        let peaks_len = peaks.len();

        let base_color = STEM_COLORS[stem_idx];
        let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, ZOOMED_WAVEFORM_ALPHA);

        // Build filled path for this stem
        let path = Path::new(|builder| {
            let mut upper_points: Vec<(f32, f32)> = Vec::with_capacity(512);
            let mut lower_points: Vec<(f32, f32)> = Vec::with_capacity(512);

            // STABLE RENDERING: Direct peak-to-pixel mapping
            // IMPORTANT: Use integer division to match peak generation (peaks.rs)
            let samples_per_peak = (duration_samples / peaks_len as u64) as f64;
            let pixels_per_sample = width as f64 / window.total_samples as f64;
            let pixels_per_peak = samples_per_peak * pixels_per_sample;

            // Center position (where playhead is, accounting for padding)
            let center_sample = window.start as f64 - window.left_padding as f64
                + (window.total_samples as f64 / 2.0);
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

                    let y_max = center_y - (max * height_scale);
                    let y_min = center_y - (min * height_scale);

                    upper_points.push((px.max(x).min(x + width), y_max));
                    lower_points.push((px.max(x).min(x + width), y_min));
                }

                peak_idx += step;
            }

            if upper_points.is_empty() {
                return;
            }

            // Draw upper envelope left to right
            let (first_x, first_y) = upper_points[0];
            builder.move_to(Point::new(first_x, first_y));
            for &(px, py) in &upper_points[1..] {
                builder.line_to(Point::new(px, py));
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

/// Draw cue markers for zoomed view
fn draw_cue_markers_zoomed(
    frame: &mut Frame,
    cue_markers: &[CueMarker],
    duration_samples: u64,
    window: &WindowInfo,
    width: f32,
    height: f32,
) {
    for marker in cue_markers {
        let cue_sample = (marker.position * duration_samples as f64) as u64;

        // Only draw cues within actual audio range
        if cue_sample < window.start || cue_sample > window.end {
            continue;
        }

        // Calculate x position accounting for left_padding
        let offset_from_virtual_start = window.left_padding + (cue_sample - window.start);
        let x = (offset_from_virtual_start as f64 / window.total_samples as f64 * width as f64) as f32;

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
///
/// Uses pre-computed highres_peaks when available for smooth, jitter-free rendering.
/// Falls back to cached_peaks if highres_peaks is empty (for backwards compatibility).
fn draw_zoomed_section(
    frame: &mut Frame,
    zoomed: &ZoomedState,
    overview: &OverviewState,
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

    // Get window with padding info for proper boundary handling
    let window = zoomed.visible_window(playhead);

    if window.total_samples == 0 {
        return;
    }

    // Helper to convert sample position to x coordinate (accounting for padding)
    let sample_to_x = |sample: u64| -> f32 {
        if sample < window.start {
            // Before visible audio - clamp to left edge of audio region
            (window.left_padding as f64 / window.total_samples as f64 * width as f64) as f32
        } else if sample > window.end {
            // After visible audio - clamp to right edge
            width
        } else {
            let offset = window.left_padding + (sample - window.start);
            (offset as f64 / window.total_samples as f64 * width as f64) as f32
        }
    };

    {
        // Draw loop region (behind everything else)
        if let Some((loop_start_norm, loop_end_norm)) = zoomed.loop_region {
            let loop_start_sample = (loop_start_norm * zoomed.duration_samples as f64) as u64;
            let loop_end_sample = (loop_end_norm * zoomed.duration_samples as f64) as u64;

            // Only draw if loop overlaps the visible window
            if loop_end_sample > window.start && loop_start_sample < window.end {
                let start_x = sample_to_x(loop_start_sample.max(window.start));
                let end_x = sample_to_x(loop_end_sample.min(window.end));

                let loop_width = end_x - start_x;
                if loop_width > 0.0 {
                    frame.fill_rectangle(
                        Point::new(start_x, 0.0),
                        Size::new(loop_width, zoomed_height),
                        Color::from_rgba(0.2, 0.8, 0.2, 0.25),
                    );
                    // Draw loop boundaries (only if within visible audio range)
                    if loop_start_sample >= window.start && loop_start_sample <= window.end {
                        let x = sample_to_x(loop_start_sample);
                        frame.stroke(
                            &Path::line(Point::new(x, 0.0), Point::new(x, zoomed_height)),
                            Stroke::default()
                                .with_color(Color::from_rgba(0.2, 0.9, 0.2, 0.8))
                                .with_width(2.0),
                        );
                    }
                    if loop_end_sample >= window.start && loop_end_sample <= window.end {
                        let x = sample_to_x(loop_end_sample);
                        frame.stroke(
                            &Path::line(Point::new(x, 0.0), Point::new(x, zoomed_height)),
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

            // Only draw if slicer region overlaps the visible window
            if slicer_end_sample > window.start && slicer_start_sample < window.end {
                let start_x = sample_to_x(slicer_start_sample.max(window.start));
                let end_x = sample_to_x(slicer_end_sample.min(window.end));
                let slicer_width = end_x - start_x;

                if slicer_width > 0.0 {
                    // Orange overlay background
                    frame.fill_rectangle(
                        Point::new(start_x, 0.0),
                        Size::new(slicer_width, zoomed_height),
                        Color::from_rgba(1.0, 0.5, 0.0, 0.12),
                    );

                    // Draw slice division lines (16 slices)
                    let samples_per_slice = (slicer_end_sample - slicer_start_sample) / 16;
                    for i in 0..=16 {
                        let slice_sample = slicer_start_sample + samples_per_slice * i as u64;
                        if slice_sample >= window.start && slice_sample <= window.end {
                            let slice_x = sample_to_x(slice_sample);
                            let is_boundary = i == 0 || i == 16;
                            let line_width = if is_boundary { 2.0 } else { 1.0 };
                            let alpha = if is_boundary { 0.8 } else { 0.5 };

                            frame.stroke(
                                &Path::line(Point::new(slice_x, 0.0), Point::new(slice_x, zoomed_height)),
                                Stroke::default()
                                    .with_color(Color::from_rgba(1.0, 0.6, 0.1, alpha))
                                    .with_width(line_width),
                            );
                        }
                    }

                    // Highlight current playing slice with brighter overlay
                    if let Some(current) = zoomed.slicer_current_slice {
                        let slice_start = slicer_start_sample + samples_per_slice * current as u64;
                        let slice_end = slice_start + samples_per_slice;

                        if slice_end > window.start && slice_start < window.end {
                            let slice_start_x = sample_to_x(slice_start.max(window.start));
                            let slice_end_x = sample_to_x(slice_end.min(window.end));

                            frame.fill_rectangle(
                                Point::new(slice_start_x, 0.0),
                                Size::new(slice_end_x - slice_start_x, zoomed_height),
                                Color::from_rgba(1.0, 0.6, 0.0, 0.2),
                            );
                        }
                    }
                }
            }
        }

        // Draw beat markers (uses WindowInfo for proper positioning)
        draw_beat_markers_zoomed(frame, &zoomed.beat_grid, &window, width, zoomed_height);

        // Draw stem waveforms - prefer highres_peaks if available
        let use_highres = !overview.highres_peaks[0].is_empty() && overview.duration_samples > 0;
        if use_highres {
            draw_highres_peaks_section(
                frame,
                &overview.highres_peaks,
                overview.duration_samples,
                &window,
                0.0,
                0.0,
                width,
                zoomed_height,
            );
        } else {
            // Fallback to cached_peaks for backwards compatibility
            draw_cached_peaks(frame, zoomed, &window, width, zoomed_center_y);
        }

        // Draw cue markers (uses WindowInfo for proper positioning)
        draw_cue_markers_zoomed(frame, &zoomed.cue_markers, zoomed.duration_samples, &window, width, zoomed_height);
    }

    // Draw playhead - position depends on view mode
    let playhead_x = match zoomed.view_mode() {
        ZoomedViewMode::Scrolling => {
            // Scrolling mode: playhead fixed at center
            width / 2.0
        }
        ZoomedViewMode::FixedBuffer => {
            // Fixed buffer mode: playhead moves within view
            if window.total_samples > 0 && playhead >= window.start && playhead <= window.end {
                let offset = (playhead - window.start) as f64;
                (offset / window.total_samples as f64 * width as f64) as f32
            } else {
                // Playhead outside view - clamp to edges
                if playhead < window.start {
                    0.0
                } else {
                    width
                }
            }
        }
    };
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
///
/// Supports split-view mode when linked stems exist: active stem (original or linked)
/// is drawn in the upper half, inactive in the lower half.
fn draw_overview_section(
    frame: &mut Frame,
    overview: &OverviewState,
    playhead: u64,
    width: f32,
    stem_active: &[bool; 4],
    linked_stems: &[bool; 4],
    linked_active: &[bool; 4],
) {
    let overview_y = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP;
    let overview_height = WAVEFORM_HEIGHT;
    let overview_center_y = overview_y + overview_height / 2.0;

    // Check if any linked stems exist to determine split-view mode
    let any_linked = linked_stems.iter().any(|&has| has);

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
        super::slicer_overlay::draw_slicer_overlay(
            frame,
            slicer_start,
            slicer_end,
            overview.slicer_current_slice,
            0.0, // No x offset for combined waveform
            overview_y,
            width as f32,
            overview_height,
        );
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

    // Draw stem waveforms - split view when linked stems exist
    if any_linked {
        // --- SPLIT VIEW MODE ---
        // Shared center line where top and bottom halves meet
        let shared_center_y = overview_y + overview_height / 2.0;
        // Height scale for half the waveform (each half uses full available space)
        let half_height_scale = (overview_height / 2.0) * 0.85;

        // Draw stems in layered order
        for &stem_idx in STEM_RENDER_ORDER.iter() {
            let has_link = linked_stems[stem_idx];
            let is_linked_active = linked_active[stem_idx];

            // Get stem color (dimmed if muted)
            let active_color = if stem_active[stem_idx] {
                let base = STEM_COLORS[stem_idx];
                Color::from_rgba(base.r, base.g, base.b, OVERVIEW_WAVEFORM_ALPHA)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.4)
            };
            let inactive_color = if stem_active[stem_idx] {
                let base = STEM_COLORS[stem_idx];
                Color::from_rgba(base.r, base.g, base.b, 0.3)
            } else {
                let gray = INACTIVE_STEM_GRAYS[stem_idx];
                Color::from_rgba(gray.r, gray.g, gray.b, 0.25)
            };

            // --- TOP HALF: Upper envelope of currently active stem ---
            let top_peaks = if is_linked_active && has_link {
                // Linked is active: draw linked on top
                overview.linked_stem_waveforms[stem_idx].as_ref()
            } else {
                // Host is active: draw host on top
                Some(&overview.stem_waveforms[stem_idx])
            };

            if let Some(peaks) = top_peaks {
                if !peaks.is_empty() {
                    draw_stem_waveform_upper(
                        frame,
                        peaks,
                        0.0,
                        shared_center_y,
                        half_height_scale,
                        active_color,
                        width,
                        stem_idx,
                    );
                }
            }

            // --- BOTTOM HALF: Lower envelope of inactive stem (if split) ---
            if has_link {
                let bottom_peaks = if is_linked_active {
                    // Linked is active, show host below
                    Some(&overview.stem_waveforms[stem_idx])
                } else {
                    // Host is active, show linked below
                    overview.linked_stem_waveforms[stem_idx].as_ref()
                };

                if let Some(peaks) = bottom_peaks {
                    if !peaks.is_empty() {
                        draw_stem_waveform_lower(
                            frame,
                            peaks,
                            0.0,
                            shared_center_y,
                            half_height_scale,
                            inactive_color,
                            width,
                            stem_idx,
                        );
                    }
                }
            }
        }

        // Draw split indicator line at center
        frame.stroke(
            &Path::line(
                Point::new(0.0, shared_center_y),
                Point::new(width, shared_center_y),
            ),
            Stroke::default()
                .with_color(Color::from_rgba(1.0, 1.0, 1.0, 0.2))
                .with_width(1.0),
        );
    } else {
        // --- NORMAL MODE (no linked stems) ---
        let height_scale = overview_height / 2.0 * 0.85;
        for &stem_idx in STEM_RENDER_ORDER.iter() {
            let stem_peaks = &overview.stem_waveforms[stem_idx];
            if stem_peaks.is_empty() {
                continue;
            }

            let base_color = if stem_active[stem_idx] {
                STEM_COLORS[stem_idx]
            } else {
                INACTIVE_STEM_GRAYS[stem_idx]
            };
            let alpha = if stem_active[stem_idx] { OVERVIEW_WAVEFORM_ALPHA } else { 0.4 };
            let waveform_color = Color::from_rgba(base_color.r, base_color.g, base_color.b, alpha);

            draw_stem_waveform_filled(frame, stem_peaks, 0.0, overview_center_y, height_scale, waveform_color, width, stem_idx);
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

    // Draw drop marker (orange diamond for linked stem alignment)
    draw_drop_marker(
        frame,
        overview.drop_marker,
        overview.duration_samples,
        width,
        overview_height,
        overview_y,
    );

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

