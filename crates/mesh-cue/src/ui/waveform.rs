//! Waveform display component with click-to-seek
//!
//! Displays the audio waveform for a track with:
//! - 4 color-coded stem waveforms (Vocals, Drums, Bass, Other)
//! - Beat grid markers
//! - Cue point markers with labels
//! - Click-to-seek interactivity

use super::app::Message;
use iced::widget::canvas::{self, Canvas, Event, Frame, Geometry, Path, Program, Stroke};
use iced::{mouse, Color, Element, Length, Point, Rectangle, Size, Theme};
use mesh_core::audio_file::{CuePoint, LoadedTrack};
use std::sync::Arc;

/// Default display width for peak computation
const DEFAULT_WIDTH: usize = 800;

/// Waveform height in pixels
const WAVEFORM_HEIGHT: f32 = 150.0;

/// Stem colors (matching mesh-player)
const STEM_COLORS: [Color; 4] = [
    Color::from_rgb(0.0, 0.8, 0.8), // Vocals - Cyan
    Color::from_rgb(0.9, 0.3, 0.3), // Drums - Red
    Color::from_rgb(0.9, 0.8, 0.2), // Bass - Yellow
    Color::from_rgb(0.3, 0.8, 0.4), // Other - Green
];

/// Cue point colors (8 distinct colors for 8 cue points)
const CUE_COLORS: [Color; 8] = [
    Color::from_rgb(1.0, 0.3, 0.3), // Red
    Color::from_rgb(1.0, 0.6, 0.0), // Orange
    Color::from_rgb(1.0, 1.0, 0.0), // Yellow
    Color::from_rgb(0.3, 1.0, 0.3), // Green
    Color::from_rgb(0.0, 0.8, 0.8), // Cyan
    Color::from_rgb(0.3, 0.3, 1.0), // Blue
    Color::from_rgb(0.8, 0.3, 0.8), // Purple
    Color::from_rgb(1.0, 0.5, 0.8), // Pink
];

/// Cue point marker for display
#[derive(Debug, Clone)]
pub struct CueMarker {
    /// Normalized position (0.0 to 1.0)
    pub position: f64,
    /// Cue label text
    pub label: String,
    /// Marker color
    pub color: Color,
    /// Cue number (0-7)
    pub index: u8,
}

/// Waveform view state
#[derive(Debug, Clone)]
pub struct WaveformView {
    /// Cached waveform data per stem (min/max pairs per column)
    stem_waveforms: [Vec<(f32, f32)>; 4],
    /// Current playhead position (0.0 to 1.0)
    position: f64,
    /// Beat grid positions (normalized 0.0 to 1.0)
    beat_markers: Vec<f64>,
    /// Cue point markers
    cue_markers: Vec<CueMarker>,
    /// Track duration in samples
    duration_samples: u64,
    /// Track loaded
    has_track: bool,
    /// Audio is loading (show placeholder)
    loading: bool,
}

impl WaveformView {
    /// Create a new empty waveform view
    pub fn new() -> Self {
        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            has_track: false,
            loading: false,
        }
    }

    /// Create a placeholder waveform from metadata only (no audio data yet)
    ///
    /// Shows beat grid and cue markers while audio loads in background.
    pub fn from_metadata(metadata: &mesh_core::audio_file::TrackMetadata) -> Self {
        // We don't know duration yet, so beat markers will be set when stems load
        // For now, just mark as loading
        let cue_markers: Vec<CueMarker> = metadata
            .cue_points
            .iter()
            .map(|cue| {
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position: 0.0, // Will be normalized when duration is known
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();

        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            beat_markers: Vec::new(), // Will be set when stems load
            cue_markers,
            duration_samples: 0,
            has_track: true,
            loading: true,
        }
    }

    /// Set stems and generate waveform data (called when audio finishes loading)
    pub fn set_stems(
        &mut self,
        stems: &mesh_core::audio_file::StemBuffers,
        cue_points: &[CuePoint],
        beat_grid: &[u64],
    ) {
        let duration_samples = stems.len() as u64;
        self.duration_samples = duration_samples;
        self.loading = false;

        // Generate peak data for each stem
        self.stem_waveforms = generate_peaks(stems, DEFAULT_WIDTH);

        // Convert beat grid to normalized positions
        if duration_samples > 0 {
            self.beat_markers = beat_grid
                .iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect();

            // Update cue markers with correct normalized positions
            self.cue_markers = cue_points
                .iter()
                .map(|cue| {
                    let position = cue.sample_position as f64 / duration_samples as f64;
                    let color = CUE_COLORS[(cue.index as usize) % 8];
                    CueMarker {
                        position,
                        label: cue.label.clone(),
                        color,
                        index: cue.index,
                    }
                })
                .collect();
        }
    }

    /// Create a waveform view from a loaded track
    pub fn from_track(track: &Arc<LoadedTrack>, cue_points: &[CuePoint]) -> Self {
        let duration_samples = track.duration_samples as u64;

        // Generate peak data for each stem
        let stem_waveforms = generate_peaks(&track.stems, DEFAULT_WIDTH);

        // Convert beat grid to normalized positions
        let beat_markers: Vec<f64> = track
            .metadata
            .beat_grid
            .beats
            .iter()
            .map(|&pos| pos as f64 / duration_samples as f64)
            .collect();

        // Convert cue points to markers
        let cue_markers: Vec<CueMarker> = cue_points
            .iter()
            .map(|cue| {
                let position = cue.sample_position as f64 / duration_samples as f64;
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position,
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();

        Self {
            stem_waveforms,
            position: 0.0,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
        }
    }

    /// Update playhead position
    pub fn set_position(&mut self, position: f64) {
        self.position = position.clamp(0.0, 1.0);
    }

    /// Update cue markers (when cue points are edited)
    pub fn update_cue_markers(&mut self, cue_points: &[CuePoint]) {
        if self.duration_samples == 0 {
            return;
        }

        self.cue_markers = cue_points
            .iter()
            .map(|cue| {
                let position = cue.sample_position as f64 / self.duration_samples as f64;
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position,
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();
    }

    /// Create the canvas element
    pub fn view(&self) -> Element<Message> {
        Canvas::new(WaveformCanvas { waveform: self })
            .width(Length::Fill)
            .height(Length::Fixed(WAVEFORM_HEIGHT))
            .into()
    }
}

impl Default for WaveformView {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate peak data from stem buffers
///
/// Downsamples the audio to one min/max pair per pixel column
fn generate_peaks(
    stems: &mesh_core::audio_file::StemBuffers,
    width: usize,
) -> [Vec<(f32, f32)>; 4] {
    let len = stems.len();
    if len == 0 || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let samples_per_column = len / width;
    if samples_per_column == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    // Get references to each stem buffer
    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];

    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (stem_idx, stem_buffer) in stem_refs.iter().enumerate() {
        result[stem_idx] = (0..width)
            .map(|col| {
                let start = col * samples_per_column;
                let end = ((col + 1) * samples_per_column).min(len);

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in start..end {
                    // Convert stereo to mono by averaging
                    let sample = (stem_buffer[i].left + stem_buffer[i].right) / 2.0;
                    min = min.min(sample);
                    max = max.max(sample);
                }

                (min, max)
            })
            .collect();
    }

    result
}

/// Canvas state for tracking mouse interaction
#[derive(Debug, Clone, Copy, Default)]
struct WaveformState {
    /// Whether mouse button is pressed (for drag seeking)
    is_dragging: bool,
}

/// Canvas program for waveform rendering with click-to-seek
struct WaveformCanvas<'a> {
    waveform: &'a WaveformView,
}

impl<'a> Program<Message> for WaveformCanvas<'a> {
    type State = WaveformState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let Some(position) = cursor.position_in(bounds) {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    state.is_dragging = true;
                    let seek_position = (position.x / bounds.width).clamp(0.0, 1.0) as f64;
                    return Some(canvas::Action::publish(Message::Seek(seek_position)));
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    state.is_dragging = false;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if state.is_dragging {
                        let seek_position = (position.x / bounds.width).clamp(0.0, 1.0) as f64;
                        return Some(canvas::Action::publish(Message::Seek(seek_position)));
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            state.is_dragging = false;
        }

        None
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
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
        _state: &Self::State,
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

        if !self.waveform.has_track {
            // No track loaded - show placeholder text
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let center_y = height / 2.0;

        // Show loading indicator if audio is still loading
        if self.waveform.loading {
            // Draw pulsing "Loading..." text in center
            let loading_color = Color::from_rgba(0.6, 0.6, 0.6, 0.8);
            // Draw a simple animated bar to indicate loading
            frame.fill_rectangle(
                Point::new(width * 0.3, center_y - 2.0),
                iced::Size::new(width * 0.4, 4.0),
                loading_color,
            );
            // Note: iced canvas doesn't support text directly, so we use a simple bar
            // The UI text "Loading..." would need to be added via the view function
            return vec![frame.into_geometry()];
        }

        // Draw beat markers (first beat of each bar highlighted in red)
        for (i, &beat) in self.waveform.beat_markers.iter().enumerate() {
            let x = (beat * width as f64) as f32;
            let (color, line_width) = if i % 4 == 0 {
                // First beat of bar (downbeat) - red and thicker
                (Color::from_rgba(1.0, 0.3, 0.3, 0.8), 2.0)
            } else {
                // Regular beat - gray
                (Color::from_rgba(0.4, 0.4, 0.4, 0.6), 1.0)
            };
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke::default().with_color(color).with_width(line_width),
            );
        }

        // Draw all 4 stem waveforms overlapped
        // Draw in reverse order so vocals (most prominent) render on top
        for stem_idx in (0..4).rev() {
            let waveform_data = &self.waveform.stem_waveforms[stem_idx];
            if waveform_data.is_empty() {
                continue;
            }

            // Use semi-transparent colors for overlapping
            let base_color = STEM_COLORS[stem_idx];
            let waveform_color =
                Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.6);

            // Scale waveform data to current width
            let data_width = waveform_data.len();
            let scale = data_width as f32 / width;

            for x in 0..(width as usize) {
                let data_idx = ((x as f32) * scale) as usize;
                if data_idx >= data_width {
                    break;
                }

                let (min, max) = waveform_data[data_idx];

                // Scale to fit in view (assuming samples are -1 to 1)
                let y1 = center_y - (max * center_y * 0.9);
                let y2 = center_y - (min * center_y * 0.9);

                frame.stroke(
                    &Path::line(Point::new(x as f32, y1), Point::new(x as f32, y2)),
                    Stroke::default()
                        .with_color(waveform_color)
                        .with_width(1.0),
                );
            }
        }

        // Draw cue markers
        for marker in &self.waveform.cue_markers {
            let x = (marker.position * width as f64) as f32;

            // Draw vertical line
            frame.fill_rectangle(
                Point::new(x - 1.5, 0.0),
                Size::new(3.0, height),
                marker.color,
            );

            // Draw triangle at top
            let triangle = Path::new(|builder| {
                builder.move_to(Point::new(x, 0.0));
                builder.line_to(Point::new(x - 8.0, 15.0));
                builder.line_to(Point::new(x + 8.0, 15.0));
                builder.close();
            });
            frame.fill(&triangle, marker.color);

            // Draw cue number in triangle (simplified - just offset text position)
            // Note: iced canvas doesn't have text drawing, so we use the cue index
            // The label is shown in the cue editor list instead
        }

        // Draw playhead
        let playhead_x = (self.waveform.position * width as f64) as f32;
        frame.stroke(
            &Path::line(
                Point::new(playhead_x, 0.0),
                Point::new(playhead_x, height),
            ),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );

        vec![frame.into_geometry()]
    }
}
