//! GPU-accelerated waveform rendering via iced's Shader widget
//!
//! Replaces the canvas-based lyon tessellation path with a WGSL fragment shader
//! that renders stem envelopes, beat markers, cue markers, loop/slicer regions,
//! playhead, and volume dimming entirely on the GPU.
//!
//! ## Architecture
//!
//! - `PeakBuffer`: Flattened peak data for GPU storage buffer (created once at track load)
//! - `WaveformPrimitive` / `WaveformPipeline`: iced `shader::Primitive` / `shader::Pipeline`
//! - `WaveformProgram`: iced `shader::Program<Message>` with seek/zoom interaction
//! - View helpers: `waveform_shader_zoomed()`, `waveform_shader_overview()`, `waveform_player_shader()`

pub mod pipeline;

use std::sync::Arc;

use iced::mouse;
use iced::widget::shader;
use iced::{Element, Length, Rectangle};

use super::state::{
    PlayerCanvasState, WAVEFORM_HEIGHT,
    MIN_ZOOM_BARS, MAX_ZOOM_BARS, ZOOM_PIXELS_PER_LEVEL,
};
use pipeline::{WaveformPrimitive, WaveformUniforms};

// Layout constants matching the old canvas renderer
/// Gap between deck cells in the 2x2 grid
pub const DECK_GRID_GAP: f32 = 10.0;
/// Gap between zoomed/header/overview within a deck
pub const DECK_INTERNAL_GAP: f32 = 2.0;

// =============================================================================
// PeakBuffer — GPU-ready peak data
// =============================================================================

/// Flattened peak data for GPU upload via storage buffer.
///
/// Created once at track load, `Arc`-cloned per frame for zero-copy sharing.
/// The pipeline uses `Arc::as_ptr()` comparison for change detection — if the
/// pointer hasn't changed, the GPU buffer is not re-uploaded.
///
/// ## Layout
///
/// Interleaved min/max pairs, stems concatenated:
/// ```text
/// [stem0_min0, stem0_max0, stem0_min1, stem0_max1, ...,
///  stem1_min0, stem1_max0, stem1_min1, stem1_max1, ...,
///  stem2_min0, stem2_max0, ...,
///  stem3_min0, stem3_max0, ...]
/// ```
///
/// Total elements: `peaks_per_stem * 4 stems * 2 (min+max)`
#[derive(Debug, Clone)]
pub struct PeakBuffer {
    /// Flattened peak data (min/max interleaved, all 4 stems concatenated)
    pub data: Arc<Vec<f32>>,
    /// Number of peak samples per stem
    pub peaks_per_stem: u32,
}

impl PeakBuffer {
    /// Create a PeakBuffer from 4 stem peak arrays.
    ///
    /// Returns `None` if the first stem has no peaks (no track loaded).
    pub fn from_stem_peaks(stem_peaks: &[Vec<(f32, f32)>; 4]) -> Option<Self> {
        if stem_peaks[0].is_empty() {
            return None;
        }
        let pps = stem_peaks[0].len() as u32;
        let mut data = Vec::with_capacity(pps as usize * 4 * 2);
        for stem in stem_peaks {
            for &(min, max) in stem {
                data.push(min);
                data.push(max);
            }
        }
        Some(Self {
            data: Arc::new(data),
            peaks_per_stem: pps,
        })
    }
}

// =============================================================================
// WaveformAction — user interaction events
// =============================================================================

/// Actions emitted by the waveform shader widget
#[derive(Debug, Clone)]
pub enum WaveformAction {
    /// User clicked overview waveform to seek (deck_idx, normalized position 0.0-1.0)
    Seek(usize, f64),
    /// User dragged zoomed waveform to change zoom level (deck_idx, new zoom in bars)
    SetZoom(usize, u32),
}

// =============================================================================
// WaveformProgram — shader::Program implementation
// =============================================================================

/// Shader program for a single waveform view (zoomed or overview).
///
/// Each deck has two instances: one for the zoomed view (scrolls with playhead)
/// and one for the overview (full track). The `is_overview` flag determines which
/// peak buffer to use and what interaction behavior to provide.
pub struct WaveformProgram<'a, Message, ActionFn>
where
    ActionFn: Fn(WaveformAction) -> Message,
{
    pub state: &'a PlayerCanvasState,
    pub deck_idx: usize,
    pub is_overview: bool,
    pub view_id: u64,
    pub on_action: ActionFn,
}

/// Interaction state persisted across frames for drag-to-zoom
#[derive(Default)]
pub struct WaveformInteraction {
    drag_start_y: Option<f32>,
    drag_start_zoom: u32,
    is_seeking: bool,
}

impl<'a, Message, ActionFn> shader::Program<Message> for WaveformProgram<'a, Message, ActionFn>
where
    Message: Clone + 'a,
    ActionFn: Fn(WaveformAction) -> Message,
{
    type State = WaveformInteraction;
    type Primitive = WaveformPrimitive;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        use iced::mouse::{Button, Event as MouseEvent};

        let cursor_pos = cursor.position_in(bounds)?;

        match event {
            iced::Event::Mouse(MouseEvent::ButtonPressed(Button::Left)) => {
                if self.is_overview {
                    // Overview: click-to-seek
                    let norm_x = (cursor_pos.x / bounds.width) as f64;
                    let norm_x = norm_x.clamp(0.0, 1.0);
                    let msg = (self.on_action)(WaveformAction::Seek(self.deck_idx, norm_x));
                    interaction.is_seeking = true;
                    Some(iced::widget::Action::publish(msg).and_capture())
                } else {
                    // Zoomed: start drag-to-zoom
                    let zoom_bars = self.state.deck(self.deck_idx).zoomed.zoom_bars;
                    interaction.drag_start_y = Some(cursor_pos.y);
                    interaction.drag_start_zoom = zoom_bars;
                    None
                }
            }
            iced::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                if self.is_overview && interaction.is_seeking {
                    // Overview: drag-to-seek
                    let norm_x = (cursor_pos.x / bounds.width) as f64;
                    let norm_x = norm_x.clamp(0.0, 1.0);
                    let msg = (self.on_action)(WaveformAction::Seek(self.deck_idx, norm_x));
                    Some(iced::widget::Action::publish(msg).and_capture())
                } else if let Some(start_y) = interaction.drag_start_y {
                    // Zoomed: drag-to-zoom
                    let dy = cursor_pos.y - start_y;
                    let zoom_delta = (dy / ZOOM_PIXELS_PER_LEVEL) as i32;
                    let new_zoom = (interaction.drag_start_zoom as i32 + zoom_delta)
                        .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32) as u32;
                    let msg = (self.on_action)(WaveformAction::SetZoom(self.deck_idx, new_zoom));
                    Some(iced::widget::Action::publish(msg).and_capture())
                } else {
                    None
                }
            }
            iced::Event::Mouse(MouseEvent::ButtonReleased(Button::Left)) => {
                interaction.drag_start_y = None;
                interaction.is_seeking = false;
                None
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _interaction: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        let deck = self.state.deck(self.deck_idx);
        let peaks = if self.is_overview {
            deck.overview.overview_peak_buffer.clone()
        } else {
            deck.overview.highres_peak_buffer.clone()
        };

        WaveformPrimitive {
            id: self.view_id,
            uniforms: self.build_uniforms(bounds),
            peaks,
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.is_overview {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::Grab
        }
    }
}

impl<'a, Message, ActionFn> WaveformProgram<'a, Message, ActionFn>
where
    ActionFn: Fn(WaveformAction) -> Message,
{
    /// Pack all waveform state into the GPU uniform buffer.
    fn build_uniforms(&self, bounds: Rectangle) -> WaveformUniforms {
        let deck = self.state.deck(self.deck_idx);
        let overview = &deck.overview;

        // Playhead position (normalized 0.0-1.0)
        let playhead = if overview.duration_samples > 0 {
            let ph = self.state.interpolated_playhead(self.deck_idx, 44100);
            ph as f32 / overview.duration_samples as f32
        } else {
            0.0
        };

        // Peaks per stem
        let peaks_per_stem = if self.is_overview {
            overview.overview_peak_buffer.as_ref().map_or(0, |p| p.peaks_per_stem)
        } else {
            overview.highres_peak_buffer.as_ref().map_or(0, |p| p.peaks_per_stem)
        };

        // Window parameters for zoomed view
        let (window_start, window_end, window_total) = if !self.is_overview && overview.duration_samples > 0 {
            let zoom_bars = deck.zoomed.zoom_bars;
            let sample_rate = 44100u64; // TODO: pass actual sample rate
            let bpm = self.state.track_bpm(self.deck_idx).unwrap_or(120.0);
            let samples_per_beat = (sample_rate as f64 * 60.0 / bpm) as u64;
            let samples_per_bar = samples_per_beat * 4;
            let window_samples = samples_per_bar * zoom_bars as u64;
            let ph = self.state.interpolated_playhead(self.deck_idx, sample_rate as u32);

            let half_window = window_samples / 2;
            let window_start = ph.saturating_sub(half_window);
            let window_end = (window_start + window_samples).min(overview.duration_samples);

            let start_norm = window_start as f32 / overview.duration_samples as f32;
            let end_norm = window_end as f32 / overview.duration_samples as f32;
            (start_norm, end_norm, peaks_per_stem as f32)
        } else {
            (0.0, 1.0, peaks_per_stem as f32)
        };

        // BPM stretch for overview
        let bpm_scale = if self.is_overview {
            // Display BPM scaling: if display_bpm != track_bpm, scale the waveform
            match (self.state.display_bpm(self.deck_idx), self.state.track_bpm(self.deck_idx)) {
                (Some(display), Some(track)) if track > 0.0 => {
                    let scale = display / track;
                    if (scale - 1.0).abs() > 0.001 { scale as f32 } else { 0.0 }
                }
                _ => 0.0,
            }
        } else {
            0.0
        };

        // Stem active flags
        let stem_active_arr = self.state.stem_active(self.deck_idx);
        let stem_active = [
            if stem_active_arr[0] { 1.0 } else { 0.0 },
            if stem_active_arr[1] { 1.0 } else { 0.0 },
            if stem_active_arr[2] { 1.0 } else { 0.0 },
            if stem_active_arr[3] { 1.0 } else { 0.0 },
        ];

        // Stem colors
        let colors = self.state.stem_colors();
        let color_to_arr = |c: iced::Color| [c.r, c.g, c.b, c.a];

        // Loop parameters
        let (loop_start, loop_end, loop_active_f) = match overview.loop_region {
            Some((start, end)) if self.state.loop_active(self.deck_idx) => {
                (start as f32, end as f32, 1.0)
            }
            _ => (0.0, 0.0, 0.0),
        };

        // Beat grid parameters
        let (grid_step, first_beat) = if !overview.beat_markers.is_empty() {
            // Compute average beat interval from the beat markers
            let avg_interval = if overview.beat_markers.len() > 1 {
                let total_span = overview.beat_markers.last().unwrap() - overview.beat_markers[0];
                total_span as f32 / (overview.beat_markers.len() - 1) as f32
            } else {
                0.0
            };
            let first = *overview.beat_markers.first().unwrap() as f32;
            (avg_interval, first)
        } else {
            (0.0, 0.0)
        };

        // Volume
        let volume = self.state.volume(self.deck_idx);

        // Cue markers (up to 8)
        let mut cue_positions = [[0.0f32; 4]; 2]; // cue_pos_0_3, cue_pos_4_7
        let mut cue_colors = [[0.0f32; 4]; 8];
        let cue_count = overview.cue_markers.len().min(8);
        for (i, cue) in overview.cue_markers.iter().take(8).enumerate() {
            let group = i / 4;
            let slot = i % 4;
            cue_positions[group][slot] = cue.position as f32;
            cue_colors[i] = color_to_arr(cue.color);
        }

        // Main cue position
        let (main_cue_pos, has_main_cue) = match overview.cue_position {
            Some(pos) => (pos as f32, 1.0),
            None => (0.0, 0.0),
        };

        // Slicer parameters
        let (slicer_start, slicer_end, slicer_active, current_slice) = match overview.slicer_region {
            Some((start, end)) => {
                let slice = overview.slicer_current_slice.unwrap_or(0) as f32;
                (start as f32, end as f32, 1.0, slice)
            }
            None => (0.0, 0.0, 0.0, 0.0),
        };

        WaveformUniforms {
            bounds: [bounds.x, bounds.y, bounds.width, bounds.height],
            view_params: [playhead, 1.0, peaks_per_stem as f32, if self.is_overview { 1.0 } else { 0.0 }],
            window_params: [window_start, window_end, window_total, bpm_scale],
            stem_active,
            stem_color_0: color_to_arr(colors[0]),
            stem_color_1: color_to_arr(colors[1]),
            stem_color_2: color_to_arr(colors[2]),
            stem_color_3: color_to_arr(colors[3]),
            loop_params: [loop_start, loop_end, loop_active_f, if overview.has_track { 1.0 } else { 0.0 }],
            beat_params: [grid_step, first_beat, 4.0, volume],
            cue_params: [cue_count as f32, main_cue_pos, has_main_cue, slicer_active],
            slicer_params: [slicer_start, slicer_end, current_slice, 0.0],
            cue_pos_0_3: cue_positions[0],
            cue_pos_4_7: cue_positions[1],
            cue_color_0: cue_colors[0],
            cue_color_1: cue_colors[1],
            cue_color_2: cue_colors[2],
            cue_color_3: cue_colors[3],
            cue_color_4: cue_colors[4],
            cue_color_5: cue_colors[5],
            cue_color_6: cue_colors[6],
            cue_color_7: cue_colors[7],
        }
    }
}

// =============================================================================
// View helper functions
// =============================================================================

/// Create a GPU-accelerated zoomed waveform view for a single deck.
pub fn waveform_shader_zoomed<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    deck_idx: usize,
    on_action: impl Fn(WaveformAction) -> Message + 'a,
) -> Element<'a, Message> {
    shader(WaveformProgram {
        state,
        deck_idx,
        is_overview: false,
        view_id: deck_idx as u64 * 2,
        on_action,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Create a GPU-accelerated overview waveform view for a single deck.
pub fn waveform_shader_overview<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    deck_idx: usize,
    on_action: impl Fn(WaveformAction) -> Message + 'a,
) -> Element<'a, Message> {
    shader(WaveformProgram {
        state,
        deck_idx,
        is_overview: true,
        view_id: deck_idx as u64 * 2 + 1,
        on_action,
    })
    .width(Length::Fill)
    .height(Length::Fixed(WAVEFORM_HEIGHT))
    .into()
}

/// Create the full 4-deck waveform display using GPU shader rendering.
///
/// Layout: 2x2 grid with decks 0-1 on top (zoomed above overview) and
/// decks 2-3 on bottom (mirrored: overview above zoomed).
pub fn waveform_player_shader<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    on_action: impl Fn(WaveformAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    use iced::widget::{column, row};

    let deck_view = |idx: usize, mirrored: bool| -> Element<'a, Message> {
        let zoomed = waveform_shader_zoomed(state, idx, on_action.clone());
        let overview = waveform_shader_overview(state, idx, on_action.clone());

        if mirrored {
            column![overview, zoomed]
                .spacing(DECK_INTERNAL_GAP)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            column![zoomed, overview]
                .spacing(DECK_INTERNAL_GAP)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    };

    column![
        row![deck_view(0, false), deck_view(1, false)]
            .spacing(DECK_GRID_GAP)
            .width(Length::Fill)
            .height(Length::Fill),
        row![deck_view(2, true), deck_view(3, true)]
            .spacing(DECK_GRID_GAP)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .spacing(DECK_GRID_GAP)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
