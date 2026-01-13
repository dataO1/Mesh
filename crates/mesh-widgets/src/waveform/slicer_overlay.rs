//! Slicer overlay drawing utilities
//!
//! Reusable functions for drawing slicer region overlays on waveform canvases.
//! Used by both mesh-player and mesh-cue for consistent slice visualization.

use iced::widget::canvas::{Frame, Path, Stroke};
use iced::{Color, Point, Size};
use mesh_core::engine::SLICER_NUM_SLICES;

/// Draw slicer region overlay on a waveform canvas
///
/// Renders a semi-transparent orange overlay with slice division lines.
/// Highlights the current playing slice with a brighter overlay.
///
/// # Arguments
/// * `frame` - The iced canvas frame to draw on
/// * `slicer_start` - Normalized start position (0.0-1.0)
/// * `slicer_end` - Normalized end position (0.0-1.0)
/// * `current_slice` - Currently playing slice index (0-15), None if not playing
/// * `region_x` - X offset where the region should be drawn
/// * `region_y` - Y offset where the region should be drawn
/// * `region_width` - Width of the waveform region
/// * `region_height` - Height of the region
pub fn draw_slicer_overlay(
    frame: &mut Frame,
    slicer_start: f64,
    slicer_end: f64,
    current_slice: Option<u8>,
    region_x: f32,
    region_y: f32,
    region_width: f32,
    region_height: f32,
) {
    let start_x = region_x + (slicer_start * region_width as f64) as f32;
    let end_x = region_x + (slicer_end * region_width as f64) as f32;
    let slicer_width = end_x - start_x;

    if slicer_width <= 0.0 {
        return;
    }

    // Orange overlay for slicer buffer
    frame.fill_rectangle(
        Point::new(start_x, region_y),
        Size::new(slicer_width, region_height),
        Color::from_rgba(1.0, 0.5, 0.0, 0.15), // Semi-transparent orange
    );

    // Draw slice divisions
    let slice_width = slicer_width / SLICER_NUM_SLICES as f32;
    for i in 0..=SLICER_NUM_SLICES {
        let x = start_x + slice_width * i as f32;
        let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
        let line_width = if is_boundary { 2.0 } else { 1.0 };
        let alpha = if is_boundary { 0.8 } else { 0.4 };

        // Highlight current slice divider
        let color = if !is_boundary {
            if let Some(current) = current_slice {
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
                Point::new(x, region_y),
                Point::new(x, region_y + region_height),
            ),
            Stroke::default().with_color(color).with_width(line_width),
        );
    }

    // Highlight current playing slice with brighter overlay
    if let Some(current) = current_slice {
        let slice_x = start_x + slice_width * current as f32;
        frame.fill_rectangle(
            Point::new(slice_x, region_y),
            Size::new(slice_width, region_height),
            Color::from_rgba(1.0, 0.6, 0.0, 0.25), // Brighter orange for current slice
        );
    }
}

/// Draw slicer overlay for a zoomed waveform view
///
/// Similar to `draw_slicer_overlay` but handles sample-based positioning
/// and clipping to the visible window bounds.
///
/// # Arguments
/// * `frame` - The iced canvas frame to draw on
/// * `slicer_start_sample` - Buffer start in samples
/// * `slicer_end_sample` - Buffer end in samples
/// * `current_slice` - Currently playing slice index (0-15)
/// * `window_start` - Visible window start in samples
/// * `window_end` - Visible window end in samples
/// * `region_y` - Y offset where the region should be drawn
/// * `region_height` - Height of the region
/// * `total_width` - Canvas width for this region
pub fn draw_slicer_overlay_zoomed(
    frame: &mut Frame,
    slicer_start_sample: u64,
    slicer_end_sample: u64,
    current_slice: Option<u8>,
    window_start: u64,
    window_end: u64,
    region_y: f32,
    region_height: f32,
    total_width: f32,
) {
    // Check if slicer region overlaps with visible window
    if slicer_end_sample <= window_start || slicer_start_sample >= window_end {
        return;
    }

    let window_width = window_end - window_start;
    if window_width == 0 {
        return;
    }

    // Calculate pixel positions within the visible window
    let samples_per_pixel = window_width as f32 / total_width;

    // Clamp slicer region to visible window
    let visible_start = slicer_start_sample.max(window_start);
    let visible_end = slicer_end_sample.min(window_end);

    let start_x = (visible_start - window_start) as f32 / samples_per_pixel;
    let end_x = (visible_end - window_start) as f32 / samples_per_pixel;
    let slicer_width = end_x - start_x;

    if slicer_width <= 0.0 {
        return;
    }

    // Orange overlay for visible portion of slicer buffer
    frame.fill_rectangle(
        Point::new(start_x, region_y),
        Size::new(slicer_width, region_height),
        Color::from_rgba(1.0, 0.5, 0.0, 0.15),
    );

    // Calculate slice positions and draw divisions
    let total_slicer_samples = slicer_end_sample - slicer_start_sample;
    let samples_per_slice = total_slicer_samples as f32 / SLICER_NUM_SLICES as f32;

    for i in 0..=SLICER_NUM_SLICES {
        let slice_sample = slicer_start_sample as f32 + samples_per_slice * i as f32;

        // Skip if outside visible window
        if slice_sample < window_start as f32 || slice_sample > window_end as f32 {
            continue;
        }

        let x = (slice_sample - window_start as f32) / samples_per_pixel;
        let is_boundary = i == 0 || i == SLICER_NUM_SLICES;
        let line_width = if is_boundary { 2.0 } else { 1.0 };
        let alpha = if is_boundary { 0.8 } else { 0.4 };

        let color = if !is_boundary {
            if let Some(current) = current_slice {
                if i as u8 == current + 1 {
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
            &Path::line(Point::new(x, region_y), Point::new(x, region_y + region_height)),
            Stroke::default().with_color(color).with_width(line_width),
        );
    }

    // Highlight current slice
    if let Some(current) = current_slice {
        let slice_start_sample =
            slicer_start_sample as f32 + samples_per_slice * current as f32;
        let slice_end_sample = slice_start_sample + samples_per_slice;

        // Clamp to visible window
        let visible_slice_start = (slice_start_sample as u64).max(window_start);
        let visible_slice_end = (slice_end_sample as u64).min(window_end);

        if visible_slice_end > visible_slice_start {
            let slice_x = (visible_slice_start - window_start) as f32 / samples_per_pixel;
            let slice_w = (visible_slice_end - visible_slice_start) as f32 / samples_per_pixel;

            frame.fill_rectangle(
                Point::new(slice_x, region_y),
                Size::new(slice_w, region_height),
                Color::from_rgba(1.0, 0.6, 0.0, 0.25),
            );
        }
    }
}
