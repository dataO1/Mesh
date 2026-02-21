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

/// Audio engine sample rate — must match mesh_core::types::SAMPLE_RATE.
const SAMPLE_RATE: u64 = 48000;

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

        // Use f64 throughout to avoid precision loss on large sample positions.
        // A 4-min track at 48kHz = ~11.5M samples, exceeding f32's 2^23 = 8.4M
        // integer precision limit. f32 normalization would lose ~1 sample of
        // precision, causing visible drift over the track's duration.
        let dur_f64 = overview.duration_samples as f64;

        // Playhead position (normalized 0.0-1.0)
        let playhead = if overview.duration_samples > 0 {
            let ph = self.state.interpolated_playhead(self.deck_idx, SAMPLE_RATE as u32);
            (ph as f64 / dur_f64) as f32
        } else {
            0.0
        };

        // Peaks per stem
        let peaks_per_stem = if self.is_overview {
            overview.overview_peak_buffer.as_ref().map_or(0, |p| p.peaks_per_stem)
        } else {
            overview.highres_peak_buffer.as_ref().map_or(0, |p| p.peaks_per_stem)
        };

        // BPM used for both window sizing and beat grid fallback.
        // Always has a value (falls back to 120 BPM).
        let bpm = self.state.track_bpm(self.deck_idx).unwrap_or(120.0);

        // Window parameters for zoomed view
        // Uses signed f64 arithmetic for precision + edge padding.
        // The shader treats source_x outside [0, 1] as silence, providing symmetric
        // centering at track boundaries.
        //
        // CRITICAL: window_span is computed directly from window_samples / duration
        // in f64, NOT as (end_norm - start_norm) which would lose precision from
        // two independent f32 casts. This keeps peaks_per_pixel stable across frames.
        let (window_start, window_end, window_total, peaks_per_pixel) = if !self.is_overview && overview.duration_samples > 0 {
            let zoom_bars = deck.zoomed.zoom_bars;
            let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
            let samples_per_bar = samples_per_beat * 4;
            let window_samples = samples_per_bar * zoom_bars as u64;
            let ph = self.state.interpolated_playhead(self.deck_idx, SAMPLE_RATE as u32);

            // Allow window to extend before 0 and after duration for symmetric centering
            let half_window = window_samples as i64 / 2;
            let virtual_start = ph as i64 - half_window;
            let virtual_end = virtual_start + window_samples as i64;

            // Normalize in f64 before casting to f32 — avoids precision loss
            let start_norm = (virtual_start as f64 / dur_f64) as f32;
            let end_norm = (virtual_end as f64 / dur_f64) as f32;

            // Compute peaks_per_pixel on CPU for stability.
            // This is the SAME value the shader would compute as pps * px_in_source,
            // but computed directly from integers to avoid float subtraction noise.
            let window_span_f64 = window_samples as f64 / dur_f64;
            let ppp = (peaks_per_stem as f64 * window_span_f64 / bounds.width as f64) as f32;

            (start_norm, end_norm, peaks_per_stem as f32, ppp)
        } else {
            (0.0, 1.0, peaks_per_stem as f32, 0.0)
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
        // Prefer analyzed beat grid; fall back to procedural BPM grid.
        // Uses the same `bpm` variable as the window computation above,
        // so the grid always matches the window sizing (even at 120 BPM fallback).
        let (grid_step, first_beat) = if overview.beat_markers.len() > 1 {
            // Use analyzed beat grid (normalized positions 0.0-1.0)
            let total_span = overview.beat_markers.last().unwrap() - overview.beat_markers[0];
            let avg_interval = total_span as f32 / (overview.beat_markers.len() - 1) as f32;
            let first = *overview.beat_markers.first().unwrap() as f32;
            (avg_interval, first)
        } else if bpm > 0.0 && dur_f64 > 0.0 {
            // Fallback: procedural grid from BPM when beat_markers empty/single
            let samples_per_beat = SAMPLE_RATE as f64 * 60.0 / bpm;
            let grid_step_norm = (samples_per_beat / dur_f64) as f32;
            (grid_step_norm, 0.0)
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
            slicer_params: [slicer_start, slicer_end, current_slice, peaks_per_pixel],
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
            // stem_smooth[0] = peak_index_scale: corrects for integer division in
            // generate_peaks(). peaks_per_stem peaks don't span the full duration —
            // the last bin absorbs remainder samples. This scale factor maps normalized
            // source_x to the correct peak index.
            // Formula: duration / floor(duration / pps) = effective peaks that span duration
            stem_smooth: {
                let pis = if peaks_per_stem > 0 && overview.duration_samples > 0 {
                    let dur = overview.duration_samples as f64;
                    let spc = (overview.duration_samples / peaks_per_stem as u64) as f64;
                    if spc > 0.0 { (dur / spc) as f32 } else { peaks_per_stem as f32 }
                } else {
                    peaks_per_stem as f32
                };
                [pis, 0.0, 0.0, 0.0]
            },
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

/// Create the full 4-deck waveform display using hybrid canvas/shader rendering.
///
/// Architecture:
/// - **Bottom layer (Canvas)**: Headers + overview waveforms (static after track load,
///   only invalidated by structural changes like stem mutes, track loads, etc.)
/// - **Top layer (Shader)**: Zoomed waveforms rendered entirely on the GPU via WGSL
///   fragment shader. Peak data uploaded once; only 384-byte uniforms per frame.
///
/// Composed via `Stack`: shader widgets are positioned with spacers to overlay
/// exactly on top of the zoomed waveform areas in the canvas layout.
///
/// Layout: 2x2 grid with decks 0-1 on top (zoomed above overview) and
/// decks 2-3 on bottom (mirrored: overview above zoomed).
pub fn waveform_player_shader<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    on_action: impl Fn(WaveformAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    use iced::widget::{column, row, stack, Canvas, Space};
    use super::canvas::PlayerCanvas;
    use super::state::DECK_HEADER_HEIGHT;

    // Bottom layer: canvas draws headers + overview waveforms (no zoomed)
    let on_action_seek = on_action.clone();
    let on_action_zoom = on_action.clone();
    let canvas_layer: Element<'a, Message> = Canvas::new(PlayerCanvas {
        state,
        on_seek: move |deck, pos| on_action_seek(WaveformAction::Seek(deck, pos)),
        on_zoom: move |deck, bars| on_action_zoom(WaveformAction::SetZoom(deck, bars)),
        skip_zoomed: true,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    // Height of the non-zoomed content per deck cell (header + gap + overview)
    let spacer_h = DECK_HEADER_HEIGHT + DECK_INTERNAL_GAP + WAVEFORM_HEIGHT;

    // Top layer: shader widgets positioned to overlay the zoomed waveform areas.
    // Each deck column: [shader(Fill)] + [spacer(Fixed)] matching the canvas layout.
    // Top row (non-mirrored): zoomed is above header+overview
    // Bottom row (mirrored): header+overview is above zoomed
    let deck_shader = |idx: usize, mirrored: bool| -> Element<'a, Message> {
        let zoomed = waveform_shader_zoomed(state, idx, on_action.clone());
        if mirrored {
            column![
                Space::new().width(Length::Fill).height(Length::Fixed(spacer_h)),
                zoomed,
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            column![
                zoomed,
                Space::new().width(Length::Fill).height(Length::Fixed(spacer_h)),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
    };

    let shader_overlay: Element<'a, Message> = column![
        row![deck_shader(0, false), deck_shader(1, false)]
            .spacing(DECK_GRID_GAP)
            .width(Length::Fill)
            .height(Length::Fill),
        row![deck_shader(2, true), deck_shader(3, true)]
            .spacing(DECK_GRID_GAP)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .spacing(DECK_GRID_GAP)
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    stack![canvas_layer, shader_overlay]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
