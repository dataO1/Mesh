//! Waveform canvas widget
//!
//! Displays the audio waveform for a track with:
//! - Overview waveform (full track)
//! - Scrolling waveform (current position)
//! - Beat grid markers
//! - Cue point markers
//! - Loop region highlighting

use iced::widget::canvas::{Canvas, Frame, Geometry, Path, Program, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme, mouse};

/// Waveform view state
pub struct WaveformView {
    /// Cached waveform data (min/max pairs per column)
    waveform_data: Vec<(f32, f32)>,
    /// Current playhead position (0.0 to 1.0)
    position: f64,
    /// Beat grid positions (normalized 0.0 to 1.0)
    beat_markers: Vec<f64>,
    /// Cue point positions (normalized 0.0 to 1.0)
    cue_markers: Vec<(f64, Color)>,
    /// Loop region (start, end) normalized
    loop_region: Option<(f64, f64)>,
    /// Track loaded
    has_track: bool,
}

impl WaveformView {
    /// Create a new waveform view
    pub fn new() -> Self {
        Self {
            waveform_data: Vec::new(),
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            loop_region: None,
            has_track: false,
        }
    }

    /// Set waveform data from audio samples
    /// Downsamples the audio to one min/max pair per pixel column
    #[allow(dead_code)]
    pub fn set_waveform(&mut self, samples: &[f32], width: usize) {
        if samples.is_empty() || width == 0 {
            self.waveform_data.clear();
            self.has_track = false;
            return;
        }

        let samples_per_column = samples.len() / width;
        self.waveform_data = (0..width)
            .map(|col| {
                let start = col * samples_per_column;
                let end = (start + samples_per_column).min(samples.len());
                let chunk = &samples[start..end];

                let min = chunk.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = chunk.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

                (min, max)
            })
            .collect();

        self.has_track = true;
    }

    /// Update playhead position
    #[allow(dead_code)]
    pub fn set_position(&mut self, position: f64) {
        self.position = position.clamp(0.0, 1.0);
    }

    /// Set beat markers
    #[allow(dead_code)]
    pub fn set_beats(&mut self, beats: Vec<f64>) {
        self.beat_markers = beats;
    }

    /// Add a cue marker
    #[allow(dead_code)]
    pub fn add_cue(&mut self, position: f64, color: Color) {
        self.cue_markers.push((position, color));
    }

    /// Clear cue markers
    #[allow(dead_code)]
    pub fn clear_cues(&mut self) {
        self.cue_markers.clear();
    }

    /// Set loop region
    #[allow(dead_code)]
    pub fn set_loop(&mut self, start: Option<f64>, end: Option<f64>) {
        self.loop_region = match (start, end) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        };
    }

    /// Create the canvas element
    #[allow(dead_code)]
    pub fn view(&self) -> Element<()> {
        Canvas::new(WaveformCanvas {
            waveform_data: self.waveform_data.clone(),
            position: self.position,
            beat_markers: self.beat_markers.clone(),
            cue_markers: self.cue_markers.clone(),
            loop_region: self.loop_region,
            has_track: self.has_track,
        })
        .width(Length::Fill)
        .height(80)
        .into()
    }
}

impl Default for WaveformView {
    fn default() -> Self {
        Self::new()
    }
}

/// Canvas program for waveform rendering
struct WaveformCanvas {
    waveform_data: Vec<(f32, f32)>,
    position: f64,
    beat_markers: Vec<f64>,
    cue_markers: Vec<(f64, Color)>,
    loop_region: Option<(f64, f64)>,
    has_track: bool,
}

impl Program<()> for WaveformCanvas {
    type State = ();

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

        if !self.has_track {
            // No track loaded - show placeholder
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let center_y = height / 2.0;

        // Draw loop region if active
        if let Some((start, end)) = self.loop_region {
            let x1 = (start * width as f64) as f32;
            let x2 = (end * width as f64) as f32;
            frame.fill_rectangle(
                Point::new(x1, 0.0),
                Size::new(x2 - x1, height),
                Color::from_rgba(0.3, 0.5, 0.3, 0.3),
            );
        }

        // Draw beat markers
        let beat_color = Color::from_rgba(0.5, 0.5, 0.5, 0.5);
        for &beat in &self.beat_markers {
            let x = (beat * width as f64) as f32;
            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke::default().with_color(beat_color).with_width(1.0),
            );
        }

        // Draw waveform
        let waveform_color = Color::from_rgb(0.3, 0.6, 0.9);

        for (x, &(min, max)) in self.waveform_data.iter().enumerate() {
            let x = x as f32;
            if x >= width {
                break;
            }

            // Scale to fit in view (assuming samples are -1 to 1)
            let y1 = center_y - (max * center_y * 0.9);
            let y2 = center_y - (min * center_y * 0.9);

            frame.stroke(
                &Path::line(Point::new(x, y1), Point::new(x, y2)),
                Stroke::default().with_color(waveform_color).with_width(1.0),
            );
        }

        // Draw cue markers
        for &(pos, color) in &self.cue_markers {
            let x = (pos * width as f64) as f32;
            frame.fill_rectangle(
                Point::new(x - 2.0, 0.0),
                Size::new(4.0, height),
                color,
            );
        }

        // Draw playhead
        let playhead_x = (self.position * width as f64) as f32;
        frame.stroke(
            &Path::line(Point::new(playhead_x, 0.0), Point::new(playhead_x, height)),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 0.3, 0.3))
                .with_width(2.0),
        );

        vec![frame.into_geometry()]
    }
}
