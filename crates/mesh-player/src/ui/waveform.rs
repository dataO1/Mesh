//! Waveform canvas widget
//!
//! Displays the audio waveform for a track with:
//! - 4 color-coded stem waveforms (Vocals, Drums, Bass, Other)
//! - Beat grid markers
//! - Cue point markers
//! - Loop region highlighting
//! - Playhead indicator

use iced::widget::canvas::{Canvas, Frame, Geometry, Path, Program, Stroke};
use iced::widget::{container, text};
use iced::{Color, Element, Length, Point, Rectangle, Size, Theme, mouse};
use mesh_core::audio_file::{dequantize_peak, WaveformPreview};

/// Stem indices
pub const STEM_VOCALS: usize = 0;
pub const STEM_DRUMS: usize = 1;
pub const STEM_BASS: usize = 2;
pub const STEM_OTHER: usize = 3;

/// Stem colors per design document
const STEM_COLORS: [Color; 4] = [
    Color::from_rgb(0.0, 0.8, 0.8),   // Vocals - Cyan
    Color::from_rgb(0.9, 0.3, 0.3),   // Drums - Red
    Color::from_rgb(0.9, 0.8, 0.2),   // Bass - Yellow
    Color::from_rgb(0.3, 0.8, 0.4),   // Other - Green
];

/// Waveform view state
pub struct WaveformView {
    /// Cached waveform data per stem (min/max pairs per column)
    stem_waveforms: [Vec<(f32, f32)>; 4],
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
    /// Missing preview message (when no waveform data available)
    missing_preview_message: Option<String>,
}

impl WaveformView {
    /// Create a new waveform view
    pub fn new() -> Self {
        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            loop_region: None,
            has_track: false,
            missing_preview_message: None,
        }
    }

    /// Create a waveform view from a cached preview
    ///
    /// This provides instant waveform display without recomputing from stems.
    pub fn from_preview(preview: &WaveformPreview) -> Self {
        // Dequantize the preview data back to f32 peaks
        let mut stem_waveforms: [Vec<(f32, f32)>; 4] =
            [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

        for (stem_idx, stem_peaks) in preview.stems.iter().enumerate() {
            let peaks: Vec<(f32, f32)> = stem_peaks
                .min
                .iter()
                .zip(stem_peaks.max.iter())
                .map(|(&min, &max)| (dequantize_peak(min), dequantize_peak(max)))
                .collect();
            stem_waveforms[stem_idx] = peaks;
        }

        Self {
            stem_waveforms,
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            loop_region: None,
            has_track: true,
            missing_preview_message: None,
        }
    }

    /// Create an empty waveform view with a message explaining why data is missing
    ///
    /// Used when a track doesn't have a cached waveform preview (needs re-analysis).
    pub fn empty_with_message(message: &str) -> Self {
        Self {
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers: Vec::new(),
            loop_region: None,
            has_track: true,
            missing_preview_message: Some(message.to_string()),
        }
    }

    /// Set waveform data for a specific stem from audio samples
    /// Downsamples the audio to one min/max pair per pixel column
    pub fn set_stem_waveform(&mut self, stem_idx: usize, samples: &[f32], width: usize) {
        if stem_idx >= 4 || samples.is_empty() || width == 0 {
            if stem_idx < 4 {
                self.stem_waveforms[stem_idx].clear();
            }
            return;
        }

        let samples_per_column = samples.len() / width;
        self.stem_waveforms[stem_idx] = (0..width)
            .map(|col| {
                let start = col * samples_per_column;
                let end = (start + samples_per_column).min(samples.len());
                let chunk = &samples[start..end];

                let min = chunk.iter().cloned().fold(f32::INFINITY, f32::min);
                let max = chunk.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

                (min, max)
            })
            .collect();

        // Mark as having a track if any stem has data
        self.has_track = self.stem_waveforms.iter().any(|w| !w.is_empty());
    }

    /// Clear all waveform data
    pub fn clear(&mut self) {
        for waveform in &mut self.stem_waveforms {
            waveform.clear();
        }
        self.beat_markers.clear();
        self.cue_markers.clear();
        self.loop_region = None;
        self.has_track = false;
        self.position = 0.0;
    }

    /// Update playhead position
    pub fn set_position(&mut self, position: f64) {
        self.position = position.clamp(0.0, 1.0);
    }

    /// Set beat markers
    pub fn set_beats(&mut self, beats: Vec<f64>) {
        self.beat_markers = beats;
    }

    /// Add a cue marker
    pub fn add_cue(&mut self, position: f64, color: Color) {
        self.cue_markers.push((position, color));
    }

    /// Clear cue markers
    pub fn clear_cues(&mut self) {
        self.cue_markers.clear();
    }

    /// Set loop region
    pub fn set_loop(&mut self, start: Option<f64>, end: Option<f64>) {
        self.loop_region = match (start, end) {
            (Some(s), Some(e)) => Some((s, e)),
            _ => None,
        };
    }

    /// Create the canvas element
    pub fn view(&self) -> Element<()> {
        const WAVEFORM_HEIGHT: f32 = 80.0;

        // If there's a missing preview message, show it as centered text
        if let Some(ref message) = self.missing_preview_message {
            let message_text = text(message)
                .size(12)
                .color(Color::from_rgb(0.7, 0.6, 0.5));

            container(
                container(message_text)
                    .center_x(Length::Fill)
                    .center_y(Length::Fixed(WAVEFORM_HEIGHT))
            )
            .width(Length::Fill)
            .height(Length::Fixed(WAVEFORM_HEIGHT))
            .style(|_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.1, 0.1, 0.12))),
                ..Default::default()
            })
            .into()
        } else {
            Canvas::new(WaveformCanvas {
                stem_waveforms: self.stem_waveforms.clone(),
                position: self.position,
                beat_markers: self.beat_markers.clone(),
                cue_markers: self.cue_markers.clone(),
                loop_region: self.loop_region,
                has_track: self.has_track,
            })
            .width(Length::Fill)
            .height(WAVEFORM_HEIGHT)
            .into()
        }
    }
}

impl Default for WaveformView {
    fn default() -> Self {
        Self::new()
    }
}

/// Canvas program for waveform rendering
struct WaveformCanvas {
    stem_waveforms: [Vec<(f32, f32)>; 4],
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

        // Draw all 4 stem waveforms overlapped
        // Draw in reverse order so vocals (most prominent) render on top
        for stem_idx in (0..4).rev() {
            let waveform_data = &self.stem_waveforms[stem_idx];
            if waveform_data.is_empty() {
                continue;
            }

            // Use semi-transparent colors for overlapping
            let base_color = STEM_COLORS[stem_idx];
            let waveform_color = Color::from_rgba(
                base_color.r,
                base_color.g,
                base_color.b,
                0.6,
            );

            for (x, &(min, max)) in waveform_data.iter().enumerate() {
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
