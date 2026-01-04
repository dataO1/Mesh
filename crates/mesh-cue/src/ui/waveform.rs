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
use mesh_core::audio_file::{
    dequantize_peak, quantize_peak, CuePoint, LoadedTrack, StemPeaks, StemBuffers, WaveformPreview,
};
use mesh_core::types::SAMPLE_RATE;
use std::sync::Arc;

/// Default display width for peak computation
const DEFAULT_WIDTH: usize = 800;

/// Waveform height in pixels
const WAVEFORM_HEIGHT: f32 = 150.0;

/// Zoomed waveform height in pixels
const ZOOMED_WAVEFORM_HEIGHT: f32 = 120.0;

/// Minimum zoom level in bars
const MIN_ZOOM_BARS: u32 = 1;

/// Maximum zoom level in bars
const MAX_ZOOM_BARS: u32 = 64;

/// Default zoom level in bars
const DEFAULT_ZOOM_BARS: u32 = 8;

/// Pixels of drag movement per zoom level change
const ZOOM_PIXELS_PER_LEVEL: f32 = 20.0;

/// Smoothing window size for peaks (moving average)
const PEAK_SMOOTHING_WINDOW: usize = 3;

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
    /// Missing preview message (when no waveform data available)
    missing_preview_message: Option<String>,
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
            missing_preview_message: None,
        }
    }

    /// Create a waveform view from a cached preview
    ///
    /// This provides instant waveform display without recomputing from stems.
    pub fn from_preview(
        preview: &WaveformPreview,
        beat_grid: &[u64],
        cue_points: &[CuePoint],
        duration_samples: u64,
    ) -> Self {
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

        // Convert beat grid to normalized positions
        let beat_markers: Vec<f64> = if duration_samples > 0 {
            beat_grid
                .iter()
                .map(|&pos| pos as f64 / duration_samples as f64)
                .collect()
        } else {
            Vec::new()
        };

        // Convert cue points to markers
        let cue_markers: Vec<CueMarker> = cue_points
            .iter()
            .map(|cue| {
                let position = if duration_samples > 0 {
                    cue.sample_position as f64 / duration_samples as f64
                } else {
                    0.0
                };
                let color = CUE_COLORS[(cue.index as usize) % 8];
                CueMarker {
                    position,
                    label: cue.label.clone(),
                    color,
                    index: cue.index,
                }
            })
            .collect();

        log::debug!(
            "Created WaveformView from preview: {} peaks, {} cue markers",
            stem_waveforms[0].len(),
            cue_markers.len()
        );

        Self {
            stem_waveforms,
            position: 0.0,
            beat_markers,
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: None,
        }
    }

    /// Create an empty waveform view with a message explaining why data is missing
    ///
    /// Used when a track doesn't have a cached waveform preview (needs re-analysis).
    pub fn empty_with_message(message: &str, cue_points: &[CuePoint], duration_samples: u64) -> Self {
        // Convert cue points to markers (we can still show these)
        let cue_markers: Vec<CueMarker> = cue_points
            .iter()
            .map(|cue| {
                let position = if duration_samples > 0 {
                    cue.sample_position as f64 / duration_samples as f64
                } else {
                    0.0
                };
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
            stem_waveforms: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            position: 0.0,
            beat_markers: Vec::new(),
            cue_markers,
            duration_samples,
            has_track: true,
            loading: false,
            missing_preview_message: Some(message.to_string()),
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
            missing_preview_message: None,
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
            missing_preview_message: None,
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
        use iced::widget::{container, text};

        // If there's a missing preview message, show it as centered text
        if let Some(ref message) = self.missing_preview_message {
            // Overlay the message on top of the canvas
            let message_text = text(message)
                .size(14)
                .color(Color::from_rgb(0.7, 0.6, 0.5));

            // Use a stack/overlay - for simplicity, just return the message in a container
            // with the canvas as background
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
            Canvas::new(WaveformCanvas { waveform: self })
                .width(Length::Fill)
                .height(Length::Fixed(WAVEFORM_HEIGHT))
                .into()
        }
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

        // Show missing preview message if waveform data is unavailable
        if self.waveform.missing_preview_message.is_some() {
            // Draw a muted indicator showing preview is missing
            // The actual text is shown via the view function using iced widgets
            let missing_color = Color::from_rgba(0.5, 0.4, 0.3, 0.6);
            // Draw dashed line pattern to indicate missing data
            for x in (0..(width as usize)).step_by(20) {
                frame.fill_rectangle(
                    Point::new(x as f32, center_y - 1.0),
                    iced::Size::new(10.0, 2.0),
                    missing_color,
                );
            }
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

// ============================================================================
// Zoomed Waveform View (detail view centered on playhead)
// ============================================================================

/// Zoomed waveform view state
///
/// Shows a detailed view of the waveform centered on the playhead position.
/// Supports zoom levels from 1 to 64 bars via click+drag gesture.
#[derive(Debug, Clone)]
pub struct ZoomedWaveformView {
    /// Cached peak data for visible window [stem_idx] = Vec<(min, max)>
    cached_peaks: [Vec<(f32, f32)>; 4],
    /// Start sample of cached window
    cache_start: u64,
    /// End sample of cached window
    cache_end: u64,
    /// Current zoom level in bars (1-64)
    zoom_bars: u32,
    /// Beat grid positions in samples
    beat_grid: Vec<u64>,
    /// Cue markers with sample positions
    cue_markers: Vec<CueMarker>,
    /// Track duration in samples
    duration_samples: u64,
    /// Detected BPM (for bar calculation)
    bpm: f64,
    /// Whether the view has valid data
    has_track: bool,
}

impl ZoomedWaveformView {
    /// Create a new empty zoomed waveform view
    pub fn new() -> Self {
        Self {
            cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cache_start: 0,
            cache_end: 0,
            zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid: Vec::new(),
            cue_markers: Vec::new(),
            duration_samples: 0,
            bpm: 120.0,
            has_track: false,
        }
    }

    /// Create from track metadata
    pub fn from_metadata(bpm: f64, beat_grid: Vec<u64>, cue_markers: Vec<CueMarker>) -> Self {
        Self {
            cached_peaks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            cache_start: 0,
            cache_end: 0,
            zoom_bars: DEFAULT_ZOOM_BARS,
            beat_grid,
            cue_markers,
            duration_samples: 0,
            bpm: if bpm > 0.0 { bpm } else { 120.0 },
            has_track: true,
        }
    }

    /// Set track duration (called when stems are loaded)
    pub fn set_duration(&mut self, duration_samples: u64) {
        self.duration_samples = duration_samples;
    }

    /// Update BPM (called when track is loaded or BPM is changed)
    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = if bpm > 0.0 { bpm } else { 120.0 };
    }

    /// Update beat grid
    pub fn set_beat_grid(&mut self, beat_grid: Vec<u64>) {
        self.beat_grid = beat_grid;
    }

    /// Update cue markers
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

    /// Samples per bar at current BPM
    fn samples_per_bar(&self) -> u64 {
        let beats_per_bar = 4;
        let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / self.bpm) as u64;
        samples_per_beat * beats_per_bar
    }

    /// Calculate visible window centered on playhead
    pub fn visible_range(&self, playhead: u64) -> (u64, u64) {
        let window_samples = self.samples_per_bar() * self.zoom_bars as u64;
        let half_window = window_samples / 2;

        let start = playhead.saturating_sub(half_window);
        let end = (playhead + half_window).min(self.duration_samples);

        (start, end)
    }

    /// Set zoom level (clamped to valid range)
    pub fn set_zoom(&mut self, bars: u32) {
        self.zoom_bars = bars.clamp(MIN_ZOOM_BARS, MAX_ZOOM_BARS);
        // Invalidate cache when zoom changes
        self.cache_start = 0;
        self.cache_end = 0;
    }

    /// Get current zoom level
    pub fn zoom_bars(&self) -> u32 {
        self.zoom_bars
    }

    /// Check if cache is valid for current playhead position
    /// Returns true if cache needs recomputation
    pub fn needs_recompute(&self, playhead: u64) -> bool {
        if self.cached_peaks[0].is_empty() {
            return true;
        }

        let (start, end) = self.visible_range(playhead);

        // Recompute if visible range is outside cached range
        // Add some margin to reduce frequent recomputation
        let margin = (end - start) / 4;
        start < self.cache_start.saturating_add(margin)
            || end > self.cache_end.saturating_sub(margin)
    }

    /// Compute peaks for the visible window from stem data
    pub fn compute_peaks(&mut self, stems: &StemBuffers, playhead: u64, width: usize) {
        let (start, end) = self.visible_range(playhead);

        // Cache a larger window to reduce recomputation frequency
        let window_size = end - start;
        let cache_start = start.saturating_sub(window_size / 2);
        let cache_end = (end + window_size / 2).min(self.duration_samples);

        self.cache_start = cache_start;
        self.cache_end = cache_end;

        let cache_len = (cache_end - cache_start) as usize;
        if cache_len == 0 || width == 0 {
            self.cached_peaks = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            return;
        }

        // Compute peaks for the cached window
        self.cached_peaks = generate_peaks_for_range(stems, cache_start, cache_end, width);

        // Apply smoothing
        for stem_idx in 0..4 {
            if self.cached_peaks[stem_idx].len() >= PEAK_SMOOTHING_WINDOW {
                self.cached_peaks[stem_idx] = smooth_peaks(&self.cached_peaks[stem_idx]);
            }
        }
    }

    /// Create the canvas element
    pub fn view(&self, playhead: u64) -> Element<Message> {
        Canvas::new(ZoomedWaveformCanvas {
            waveform: self,
            playhead,
        })
        .width(Length::Fill)
        .height(Length::Fixed(ZOOMED_WAVEFORM_HEIGHT))
        .into()
    }
}

impl Default for ZoomedWaveformView {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate peak data for a specific sample range
fn generate_peaks_for_range(
    stems: &StemBuffers,
    start_sample: u64,
    end_sample: u64,
    width: usize,
) -> [Vec<(f32, f32)>; 4] {
    let len = stems.len();
    let start = start_sample as usize;
    let end = (end_sample as usize).min(len);

    if start >= end || width == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let range_len = end - start;
    let samples_per_column = range_len / width;
    if samples_per_column == 0 {
        return [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    }

    let stem_refs = [&stems.vocals, &stems.drums, &stems.bass, &stems.other];
    let mut result: [Vec<(f32, f32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];

    for (stem_idx, stem_buffer) in stem_refs.iter().enumerate() {
        result[stem_idx] = (0..width)
            .map(|col| {
                let col_start = start + col * samples_per_column;
                let col_end = (start + (col + 1) * samples_per_column).min(end);

                let mut min = f32::INFINITY;
                let mut max = f32::NEG_INFINITY;

                for i in col_start..col_end {
                    if i < stem_buffer.len() {
                        let sample = (stem_buffer[i].left + stem_buffer[i].right) / 2.0;
                        min = min.min(sample);
                        max = max.max(sample);
                    }
                }

                if min == f32::INFINITY {
                    (0.0, 0.0)
                } else {
                    (min, max)
                }
            })
            .collect();
    }

    result
}

/// Apply moving average smoothing to peaks
fn smooth_peaks(peaks: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if peaks.len() < PEAK_SMOOTHING_WINDOW {
        return peaks.to_vec();
    }

    peaks
        .windows(PEAK_SMOOTHING_WINDOW)
        .map(|w| {
            let min_avg = w.iter().map(|(m, _)| m).sum::<f32>() / PEAK_SMOOTHING_WINDOW as f32;
            let max_avg = w.iter().map(|(_, m)| m).sum::<f32>() / PEAK_SMOOTHING_WINDOW as f32;
            (min_avg, max_avg)
        })
        .collect()
}

/// Generate a waveform preview for storage in WAV file
///
/// This creates a quantized preview at the standard width (800 pixels)
/// that can be stored in the wvfm chunk for instant display on load.
pub fn generate_waveform_preview(stems: &StemBuffers) -> WaveformPreview {
    let width = WaveformPreview::STANDARD_WIDTH as usize;

    // Generate peaks like the regular generate_peaks function
    // Note: We skip smoothing here - at 800 pixels the resolution is already
    // low enough, and smooth_peaks() reduces array length by (window_size - 1),
    // causing a mismatch between the width field and actual data length.
    let peaks = generate_peaks(stems, width);

    // Convert to quantized StemPeaks
    let mut preview = WaveformPreview {
        width: width as u16,
        stems: Default::default(),
    };

    for (stem_idx, stem_peaks) in peaks.iter().enumerate() {
        let mut min_values = Vec::with_capacity(stem_peaks.len());
        let mut max_values = Vec::with_capacity(stem_peaks.len());

        for &(min, max) in stem_peaks {
            min_values.push(quantize_peak(min));
            max_values.push(quantize_peak(max));
        }

        preview.stems[stem_idx] = StemPeaks {
            min: min_values,
            max: max_values,
        };
    }

    log::debug!(
        "Generated waveform preview: {}px width, {} samples per stem",
        preview.width,
        preview.stems[0].min.len()
    );

    preview
}

/// Canvas state for zoomed waveform (tracks zoom gesture)
#[derive(Debug, Clone, Copy, Default)]
struct ZoomedWaveformState {
    /// Mouse Y position when drag started (for zoom gesture)
    drag_start_y: Option<f32>,
    /// Zoom level when drag started
    drag_start_zoom: u32,
}

/// Canvas program for zoomed waveform rendering
struct ZoomedWaveformCanvas<'a> {
    waveform: &'a ZoomedWaveformView,
    playhead: u64,
}

impl<'a> Program<Message> for ZoomedWaveformCanvas<'a> {
    type State = ZoomedWaveformState;

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
                    // Start zoom gesture
                    state.drag_start_y = Some(position.y);
                    state.drag_start_zoom = self.waveform.zoom_bars;
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    state.drag_start_y = None;
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if let Some(start_y) = state.drag_start_y {
                        // Drag up = zoom in (fewer bars), drag down = zoom out (more bars)
                        let delta = start_y - position.y;
                        let zoom_change = (delta / ZOOM_PIXELS_PER_LEVEL) as i32;
                        let new_zoom = (state.drag_start_zoom as i32 - zoom_change)
                            .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32)
                            as u32;

                        if new_zoom != self.waveform.zoom_bars {
                            return Some(canvas::Action::publish(Message::SetZoomBars(new_zoom)));
                        }
                    }
                }
                _ => {}
            }
        } else if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(_))) {
            state.drag_start_y = None;
        }

        None
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            if state.drag_start_y.is_some() {
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
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        // Background (slightly different from overview)
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color::from_rgb(0.08, 0.08, 0.1),
        );

        if !self.waveform.has_track {
            return vec![frame.into_geometry()];
        }

        // If duration not yet known (stems still loading), can't compute visible range
        // The canvas will be redrawn once TrackStemsLoaded sets the duration
        if self.waveform.duration_samples == 0 {
            return vec![frame.into_geometry()];
        }

        let width = bounds.width;
        let height = bounds.height;
        let center_y = height / 2.0;

        let (view_start, view_end) = self.waveform.visible_range(self.playhead);
        let view_samples = (view_end - view_start) as f64;

        if view_samples <= 0.0 {
            return vec![frame.into_geometry()];
        }

        // Draw beat markers (only those in visible range)
        for (i, &beat_sample) in self.waveform.beat_grid.iter().enumerate() {
            if beat_sample < view_start || beat_sample > view_end {
                continue;
            }

            // Convert sample position to x coordinate
            let x = ((beat_sample - view_start) as f64 / view_samples * width as f64) as f32;

            let (color, line_width) = if i % 4 == 0 {
                // Downbeat - red and thicker
                (Color::from_rgba(1.0, 0.3, 0.3, 0.9), 2.0)
            } else {
                // Regular beat - gray
                (Color::from_rgba(0.5, 0.5, 0.5, 0.7), 1.0)
            };

            frame.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, height)),
                Stroke::default().with_color(color).with_width(line_width),
            );
        }

        // Draw stem waveforms from cached peaks
        // Map cached peaks to visible range
        if self.waveform.cache_end > self.waveform.cache_start {
            let cache_samples = (self.waveform.cache_end - self.waveform.cache_start) as f64;

            for stem_idx in (0..4).rev() {
                let peaks = &self.waveform.cached_peaks[stem_idx];
                if peaks.is_empty() {
                    continue;
                }

                let base_color = STEM_COLORS[stem_idx];
                let waveform_color =
                    Color::from_rgba(base_color.r, base_color.g, base_color.b, 0.7);

                let peaks_len = peaks.len() as f64;

                for x in 0..(width as usize) {
                    // Map x to sample position in view
                    let view_sample =
                        view_start as f64 + (x as f64 / width as f64) * view_samples;

                    // Map sample position to cache index
                    let cache_offset = view_sample - self.waveform.cache_start as f64;
                    if cache_offset < 0.0 || cache_offset >= cache_samples {
                        continue;
                    }

                    let cache_idx = (cache_offset / cache_samples * peaks_len) as usize;
                    if cache_idx >= peaks.len() {
                        continue;
                    }

                    let (min, max) = peaks[cache_idx];

                    // Scale to fit in view
                    let y1 = center_y - (max * center_y * 0.85);
                    let y2 = center_y - (min * center_y * 0.85);

                    frame.stroke(
                        &Path::line(Point::new(x as f32, y1), Point::new(x as f32, y2)),
                        Stroke::default()
                            .with_color(waveform_color)
                            .with_width(1.0),
                    );
                }
            }
        }

        // Draw cue markers (only those in visible range)
        for marker in &self.waveform.cue_markers {
            let cue_sample = (marker.position * self.waveform.duration_samples as f64) as u64;

            if cue_sample < view_start || cue_sample > view_end {
                continue;
            }

            let x = ((cue_sample - view_start) as f64 / view_samples * width as f64) as f32;

            // Draw vertical line
            frame.fill_rectangle(
                Point::new(x - 1.0, 0.0),
                Size::new(2.0, height),
                marker.color,
            );

            // Draw triangle at top
            let triangle = Path::new(|builder| {
                builder.move_to(Point::new(x, 0.0));
                builder.line_to(Point::new(x - 6.0, 12.0));
                builder.line_to(Point::new(x + 6.0, 12.0));
                builder.close();
            });
            frame.fill(&triangle, marker.color);
        }

        // Draw playhead at center (always centered)
        let playhead_x = width / 2.0;
        frame.stroke(
            &Path::line(
                Point::new(playhead_x, 0.0),
                Point::new(playhead_x, height),
            ),
            Stroke::default()
                .with_color(Color::from_rgb(1.0, 1.0, 1.0))
                .with_width(2.0),
        );

        // Draw zoom indicator bar in corner (width represents zoom level)
        // Note: iced canvas doesn't support text, so we use a visual indicator
        let indicator_width = (self.waveform.zoom_bars as f32 / MAX_ZOOM_BARS as f32) * 60.0;
        frame.fill_rectangle(
            Point::new(width - 70.0, 5.0),
            Size::new(indicator_width, 4.0),
            Color::from_rgba(1.0, 1.0, 1.0, 0.5),
        );

        vec![frame.into_geometry()]
    }
}
