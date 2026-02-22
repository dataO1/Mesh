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

mod header;
pub mod pipeline;

pub use header::view_deck_header;

use std::sync::Arc;

use iced::mouse;
use iced::widget::shader;
use iced::{Element, Length, Rectangle};

use iced::Color;

use super::state::{
    CombinedState, PlayerCanvasState, ZoomedViewMode,
    COMBINED_WAVEFORM_GAP, WAVEFORM_HEIGHT, ZOOMED_WAVEFORM_HEIGHT,
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

    /// Resample peaks to a target length via linear interpolation.
    ///
    /// Used when linked stem peaks have a different count than the original
    /// (different track duration → different peak count at the same resolution).
    fn resample_peaks(src: &[(f32, f32)], target_len: usize) -> Vec<(f32, f32)> {
        if src.len() == target_len || src.is_empty() || target_len == 0 {
            return src.to_vec();
        }
        let mut result = Vec::with_capacity(target_len);
        let ratio = (src.len() as f32 - 1.0) / (target_len as f32 - 1.0).max(1.0);
        for i in 0..target_len {
            let pos = i as f32 * ratio;
            let idx = pos as usize;
            let frac = pos - idx as f32;
            let a = src[idx.min(src.len() - 1)];
            let b = src[(idx + 1).min(src.len() - 1)];
            result.push((
                a.0 + (b.0 - a.0) * frac,
                a.1 + (b.1 - a.1) * frac,
            ));
        }
        result
    }

    /// Append one stem's peaks to the data buffer, applying LUFS gain and resampling if needed.
    fn append_stem(
        data: &mut Vec<f32>,
        peaks: &[(f32, f32)],
        target_pps: usize,
        gain: f32,
    ) {
        if peaks.len() == target_pps {
            for &(min, max) in peaks {
                data.push(min * gain);
                data.push(max * gain);
            }
        } else {
            let resampled = Self::resample_peaks(peaks, target_pps);
            for (min, max) in &resampled {
                data.push(min * gain);
                data.push(max * gain);
            }
        }
    }

    /// Build an 8-stem buffer with original and linked peaks side by side.
    ///
    /// Layout (fixed, independent of linked_active state):
    /// - Stems 0-3: original peaks (always)
    /// - Stems 4-7: linked peaks where available, zero-padded otherwise
    ///
    /// The shader uses `linked_active` uniforms to decide which set to display
    /// as active (top/primary) vs inactive (bottom/dimmed). This means the buffer
    /// only needs rebuilding when peak DATA changes, not on every toggle.
    ///
    /// Linked peaks are resampled to match the original's peak count if needed,
    /// and LUFS gain correction is applied during construction.
    pub fn from_linked(
        original: &[Vec<(f32, f32)>; 4],
        linked: &[Option<Vec<(f32, f32)>>; 4],
        lufs_gains: &[f32; 4],
    ) -> Option<Self> {
        if original[0].is_empty() {
            return None;
        }
        let pps = original[0].len();
        let mut data = Vec::with_capacity(pps * 8 * 2);

        // Stems 0-3: always original
        for stem_idx in 0..4 {
            Self::append_stem(&mut data, &original[stem_idx], pps, 1.0);
        }

        // Stems 4-7: linked if available, zero-padded otherwise
        for stem_idx in 0..4 {
            if let Some(linked_peaks) = &linked[stem_idx] {
                Self::append_stem(&mut data, linked_peaks, pps, lufs_gains[stem_idx]);
            } else {
                data.extend(std::iter::repeat(0.0).take(pps * 2));
            }
        }

        Some(Self {
            data: Arc::new(data),
            peaks_per_stem: pps as u32,
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

        // Use cached 8-stem linked buffer if available, otherwise fall back to 4-stem original.
        // The linked buffers are rebuilt by OverviewState when peak data changes (not per-frame).
        // The shader uses linked_active uniforms to decide which stems are shown as active.
        let peaks = if self.is_overview {
            deck.overview.linked_overview_buffer.clone()
                .or_else(|| deck.overview.overview_peak_buffer.clone())
        } else {
            deck.overview.linked_highres_buffer.clone()
                .or_else(|| deck.overview.highres_peak_buffer.clone())
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

        // Peaks per stem — read from the actual buffer being used (linked or original)
        let peaks_per_stem = if self.is_overview {
            overview.linked_overview_buffer.as_ref()
                .or(overview.overview_peak_buffer.as_ref())
                .map_or(0, |p| p.peaks_per_stem)
        } else {
            overview.linked_highres_buffer.as_ref()
                .or(overview.highres_peak_buffer.as_ref())
                .map_or(0, |p| p.peaks_per_stem)
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

            log::debug!(
                "[RENDER] deck={} zoom={}bars | bounds={:.0}x{:.0} | bpm={:.1} spb={} spbar={} | \
                 window={}samples ({:.4}..{:.4}) | peaks_per_stem={} | pp/px={:.3} | \
                 abstraction={} blur={} depth_fade={} inverted={}",
                self.deck_idx, zoom_bars, bounds.width, bounds.height,
                bpm, samples_per_beat, samples_per_bar,
                window_samples, start_norm, end_norm,
                peaks_per_stem, ppp,
                self.state.abstraction_level, self.state.motion_blur_level,
                self.state.depth_fade_level, self.state.depth_fade_inverted,
            );

            (start_norm, end_norm, peaks_per_stem as f32, ppp)
        } else {
            (0.0, 1.0, peaks_per_stem as f32, 0.0)
        };

        // BPM-aligned display fraction for overview.
        // D = fraction of width this deck occupies when all decks share a common
        // time axis. The longest track (in beats) gets D=1.0; shorter ones get
        // D<1.0 with silence padding. The shader divides uv.x by D, so positions
        // past D map to source_x > 1.0 → silence.
        let bpm_scale = if self.is_overview {
            self.state.overview_display_fraction(self.deck_idx)
                .map(|d| d as f32)
                .unwrap_or(0.0)
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
            view_params: [playhead, deck.zoomed.lufs_gain, peaks_per_stem as f32, if self.is_overview { 1.0 } else { 0.0 }],
            window_params: [window_start, window_end, window_total, bpm_scale],
            stem_active,
            stem_color_0: color_to_arr(colors[0]),
            stem_color_1: color_to_arr(colors[1]),
            stem_color_2: color_to_arr(colors[2]),
            stem_color_3: color_to_arr(colors[3]),
            loop_params: [loop_start, loop_end, loop_active_f, if overview.has_track { 1.0 } else { 0.0 }],
            beat_params: [grid_step, first_beat, if self.is_overview { overview.grid_bars as f32 } else { 4.0 }, volume],
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
                // For overview: pack the zoomed window start/end so the overview
                // can render a highlight showing the currently visible region.
                let (zoom_start, zoom_end) = if self.is_overview && dur_f64 > 0.0 {
                    let zoom_bars = deck.zoomed.zoom_bars;
                    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
                    let samples_per_bar = samples_per_beat * 4;
                    let window_samples = samples_per_bar * zoom_bars as u64;
                    let ph = self.state.interpolated_playhead(self.deck_idx, SAMPLE_RATE as u32);
                    let half = window_samples as i64 / 2;
                    let vs = ph as i64 - half;
                    let ve = vs + window_samples as i64;
                    // Clamp to [0, 1] for valid display range
                    let s = (vs as f64 / dur_f64).max(0.0) as f32;
                    let e = (ve as f64 / dur_f64).min(1.0) as f32;
                    (s, e)
                } else {
                    (0.0, 0.0)
                };
                // Mirror flag: decks 0,2 (1,3 in UI) = left edge, decks 1,3 (2,4 in UI) = right edge
                let mirror = if self.deck_idx % 2 == 0 { 1.0 } else { 0.0 };
                [pis, zoom_start, zoom_end, mirror]
            },
            linked_stems: {
                let (ls, _) = self.state.linked_stems(self.deck_idx);
                [
                    if ls[0] { 1.0 } else { 0.0 },
                    if ls[1] { 1.0 } else { 0.0 },
                    if ls[2] { 1.0 } else { 0.0 },
                    if ls[3] { 1.0 } else { 0.0 },
                ]
            },
            linked_active: {
                let (_, la) = self.state.linked_stems(self.deck_idx);
                [
                    if la[0] { 1.0 } else { 0.0 },
                    if la[1] { 1.0 } else { 0.0 },
                    if la[2] { 1.0 } else { 0.0 },
                    if la[3] { 1.0 } else { 0.0 },
                ]
            },
            render_options: [
                self.state.abstraction_level as f32 + 1.0, // 1.0=low, 2.0=medium, 3.0=high (0.0=off/raw)
                self.state.motion_blur_level as f32,        // 0.0=low, 1.0=medium, 2.0=high
                self.state.depth_fade_level as f32,          // 0.0=off, 1.0=low, 2.0=medium, 3.0=high
                if self.state.depth_fade_inverted { 1.0 } else { 0.0 },
            ],
            render_options_2: [
                self.state.peak_width_mult,                  // 0.0=off, 0.75=thin, 1.5=medium, 2.5=wide
                self.state.edge_aa_level as f32,             // 0=standard, 1=slopeL1, 2=slopeL2, 3=slopeL2Clamped
                0.0, 0.0,
            ],
        }
    }
}

// =============================================================================
// SingleDeckAction — user interaction events (mesh-cue)
// =============================================================================

/// Actions emitted by the single-deck shader waveform widget (used by mesh-cue).
///
/// Unlike `WaveformAction`, these don't carry a `deck_idx` since the widget
/// represents a single deck. Includes scratch (vinyl scrubbing) support.
#[derive(Debug, Clone)]
pub enum SingleDeckAction {
    /// User clicked overview waveform to seek (normalized position 0.0-1.0)
    Seek(f64),
    /// User dragged zoomed waveform vertically to change zoom level (new zoom in bars)
    SetZoom(u32),
    /// User started horizontal drag on zoomed waveform (vinyl touch)
    ScratchStart,
    /// User is dragging horizontally on zoomed waveform (normalized position 0.0-1.0)
    ScratchMove(f64),
    /// User released horizontal drag on zoomed waveform (vinyl release)
    ScratchEnd,
}

// =============================================================================
// SingleDeckProgram — shader::Program for single-deck use (mesh-cue)
// =============================================================================

/// Gesture detection state for zoomed waveform drag interaction.
///
/// Detects drag direction after 5px of movement, then locks to either
/// horizontal (scratch/scrub) or vertical (zoom) for the gesture duration.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum SingleDeckGesture {
    #[default]
    None,
    /// Drag started, waiting for 5px movement to detect direction
    Pending,
    /// Horizontal drag locked — vinyl scrubbing
    Scrubbing,
    /// Vertical drag locked — zoom adjustment
    Zooming,
}

/// Interaction state for single-deck shader waveform.
#[derive(Default)]
pub struct SingleDeckInteraction {
    gesture: SingleDeckGesture,
    drag_start_x: Option<f32>,
    drag_start_y: Option<f32>,
    drag_start_zoom: u32,
    scrub_start_ratio: f64,
    is_seeking: bool,
}

/// Shader program for a single-deck waveform view (used by mesh-cue).
///
/// Unlike `WaveformProgram` which reads from `PlayerCanvasState` with a deck index,
/// this reads directly from a `CombinedState` with an explicit playhead position.
/// The zoomed view supports scratch gesture detection (horizontal drag = scrub,
/// vertical drag = zoom, direction detected after 5px threshold).
pub struct SingleDeckProgram<'a, Message, ActionFn>
where
    ActionFn: Fn(SingleDeckAction) -> Message,
{
    pub state: &'a CombinedState,
    pub playhead: u64,
    pub stem_colors: [Color; 4],
    pub is_overview: bool,
    pub view_id: u64,
    pub on_action: ActionFn,
}

impl<'a, Message, ActionFn> shader::Program<Message> for SingleDeckProgram<'a, Message, ActionFn>
where
    Message: Clone + 'a,
    ActionFn: Fn(SingleDeckAction) -> Message,
{
    type State = SingleDeckInteraction;
    type Primitive = WaveformPrimitive;

    fn update(
        &self,
        interaction: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        use iced::mouse::{Button, Event as MouseEvent};

        let has_track = self.state.zoomed.has_track && self.state.zoomed.duration_samples > 0;

        match event {
            iced::Event::Mouse(MouseEvent::ButtonReleased(Button::Left)) => {
                // Handle release even if cursor is outside bounds
                let was_scrubbing = interaction.gesture == SingleDeckGesture::Scrubbing;
                interaction.gesture = SingleDeckGesture::None;
                interaction.drag_start_x = None;
                interaction.drag_start_y = None;
                interaction.is_seeking = false;
                if was_scrubbing && has_track {
                    return Some(iced::widget::Action::publish(
                        (self.on_action)(SingleDeckAction::ScratchEnd),
                    ));
                }
                return None;
            }
            _ => {}
        }

        let cursor_pos = cursor.position_in(bounds)?;

        match event {
            iced::Event::Mouse(MouseEvent::ButtonPressed(Button::Left)) => {
                if self.is_overview {
                    // Overview: click-to-seek
                    let norm_x = (cursor_pos.x / bounds.width) as f64;
                    let norm_x = norm_x.clamp(0.0, 1.0);
                    interaction.is_seeking = true;
                    Some(iced::widget::Action::publish(
                        (self.on_action)(SingleDeckAction::Seek(norm_x)),
                    ).and_capture())
                } else {
                    // Zoomed: start gesture detection
                    interaction.gesture = SingleDeckGesture::Pending;
                    interaction.drag_start_x = Some(cursor_pos.x);
                    interaction.drag_start_y = Some(cursor_pos.y);
                    interaction.drag_start_zoom = self.state.zoomed.zoom_bars;
                    if has_track {
                        interaction.scrub_start_ratio =
                            self.playhead as f64 / self.state.zoomed.duration_samples as f64;
                    }
                    None
                }
            }
            iced::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                if self.is_overview && interaction.is_seeking {
                    // Overview: drag-to-seek
                    let norm_x = (cursor_pos.x / bounds.width) as f64;
                    let norm_x = norm_x.clamp(0.0, 1.0);
                    return Some(iced::widget::Action::publish(
                        (self.on_action)(SingleDeckAction::Seek(norm_x)),
                    ).and_capture());
                }

                // Detect direction if pending
                if interaction.gesture == SingleDeckGesture::Pending {
                    if let (Some(sx), Some(sy)) = (interaction.drag_start_x, interaction.drag_start_y) {
                        let dx = (cursor_pos.x - sx).abs();
                        let dy = (cursor_pos.y - sy).abs();
                        if dx > 5.0 || dy > 5.0 {
                            if dx > dy && has_track {
                                interaction.gesture = SingleDeckGesture::Scrubbing;
                                return Some(iced::widget::Action::publish(
                                    (self.on_action)(SingleDeckAction::ScratchStart),
                                ).and_capture());
                            } else {
                                let zoom_enabled = self.state.zoomed.view_mode != ZoomedViewMode::FixedBuffer;
                                if zoom_enabled {
                                    interaction.gesture = SingleDeckGesture::Zooming;
                                }
                            }
                        }
                    }
                }

                // Handle scrubbing
                if interaction.gesture == SingleDeckGesture::Scrubbing && has_track {
                    if let Some(sx) = interaction.drag_start_x {
                        let delta_x = cursor_pos.x - sx;
                        let bpm = if self.state.zoomed.bpm > 0.0 { self.state.zoomed.bpm } else { 120.0 };
                        let samples_per_beat = (60.0 / bpm) * SAMPLE_RATE as f64;
                        let visible_samples = samples_per_beat * 4.0 * self.state.zoomed.zoom_bars as f64;
                        let samples_per_pixel = visible_samples / bounds.width as f64;
                        let sample_delta = delta_x as f64 * samples_per_pixel;
                        // Drag right = waveform moves right = playhead backward (subtract)
                        let delta_ratio = sample_delta / self.state.zoomed.duration_samples as f64;
                        let new_ratio = (interaction.scrub_start_ratio - delta_ratio).clamp(0.0, 1.0);
                        return Some(iced::widget::Action::publish(
                            (self.on_action)(SingleDeckAction::ScratchMove(new_ratio)),
                        ).and_capture());
                    }
                }

                // Handle zooming
                if interaction.gesture == SingleDeckGesture::Zooming {
                    if let Some(sy) = interaction.drag_start_y {
                        let dy = cursor_pos.y - sy;
                        let zoom_delta = (dy / ZOOM_PIXELS_PER_LEVEL) as i32;
                        let new_zoom = (interaction.drag_start_zoom as i32 + zoom_delta)
                            .clamp(MIN_ZOOM_BARS as i32, MAX_ZOOM_BARS as i32) as u32;
                        return Some(iced::widget::Action::publish(
                            (self.on_action)(SingleDeckAction::SetZoom(new_zoom)),
                        ).and_capture());
                    }
                }

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
        // Use cached 8-stem linked buffer if available, otherwise fall back to 4-stem original.
        let peaks = if self.is_overview {
            self.state.overview.linked_overview_buffer.clone()
                .or_else(|| self.state.overview.overview_peak_buffer.clone())
        } else {
            self.state.overview.linked_highres_buffer.clone()
                .or_else(|| self.state.overview.highres_peak_buffer.clone())
        };

        WaveformPrimitive {
            id: self.view_id,
            uniforms: self.build_uniforms(bounds),
            peaks,
        }
    }

    fn mouse_interaction(
        &self,
        interaction: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.is_overview {
            mouse::Interaction::Pointer
        } else {
            match interaction.gesture {
                SingleDeckGesture::Scrubbing => mouse::Interaction::Grabbing,
                SingleDeckGesture::Zooming => mouse::Interaction::ResizingVertically,
                _ => mouse::Interaction::Grab,
            }
        }
    }
}

impl<'a, Message, ActionFn> SingleDeckProgram<'a, Message, ActionFn>
where
    ActionFn: Fn(SingleDeckAction) -> Message,
{
    /// Build GPU uniforms from CombinedState fields.
    ///
    /// Same uniform layout as `WaveformProgram::build_uniforms()` but reads from
    /// `CombinedState` directly instead of `PlayerCanvasState` + deck index.
    fn build_uniforms(&self, bounds: Rectangle) -> WaveformUniforms {
        let overview = &self.state.overview;
        let dur_f64 = overview.duration_samples as f64;

        // Playhead (direct sample position, no interpolation needed)
        let playhead = if overview.duration_samples > 0 {
            (self.playhead as f64 / dur_f64) as f32
        } else {
            0.0
        };

        // Peaks per stem — read from the actual buffer being used (linked or original)
        let peaks_per_stem = if self.is_overview {
            overview.linked_overview_buffer.as_ref()
                .or(overview.overview_peak_buffer.as_ref())
                .map_or(0, |p| p.peaks_per_stem)
        } else {
            overview.linked_highres_buffer.as_ref()
                .or(overview.highres_peak_buffer.as_ref())
                .map_or(0, |p| p.peaks_per_stem)
        };

        let bpm = if self.state.zoomed.bpm > 0.0 { self.state.zoomed.bpm } else { 120.0 };

        // Window parameters for zoomed view
        let (window_start, window_end, window_total, peaks_per_pixel) = if !self.is_overview && overview.duration_samples > 0 {
            let zoom_bars = self.state.zoomed.zoom_bars;
            let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
            let samples_per_bar = samples_per_beat * 4;
            let window_samples = samples_per_bar * zoom_bars as u64;

            let half_window = window_samples as i64 / 2;
            let virtual_start = self.playhead as i64 - half_window;
            let virtual_end = virtual_start + window_samples as i64;

            let start_norm = (virtual_start as f64 / dur_f64) as f32;
            let end_norm = (virtual_end as f64 / dur_f64) as f32;

            let window_span_f64 = window_samples as f64 / dur_f64;
            let ppp = (peaks_per_stem as f64 * window_span_f64 / bounds.width as f64) as f32;

            log::debug!(
                "[RENDER] single-deck zoom={}bars | bounds={:.0}x{:.0} | bpm={:.1} spbar={} | \
                 window={}samples ({:.4}..{:.4}) | peaks_per_stem={} | pp/px={:.3}",
                zoom_bars, bounds.width, bounds.height,
                bpm, samples_per_bar,
                window_samples, start_norm, end_norm,
                peaks_per_stem, ppp,
            );

            (start_norm, end_norm, peaks_per_stem as f32, ppp)
        } else {
            (0.0, 1.0, peaks_per_stem as f32, 0.0)
        };

        // No BPM stretch for single-deck (mesh-cue doesn't sync BPM)
        let bpm_scale = 0.0f32;

        // Stem active flags from CombinedState
        let stem_active = [
            if self.state.stem_active[0] { 1.0 } else { 0.0 },
            if self.state.stem_active[1] { 1.0 } else { 0.0 },
            if self.state.stem_active[2] { 1.0 } else { 0.0 },
            if self.state.stem_active[3] { 1.0 } else { 0.0 },
        ];

        let color_to_arr = |c: Color| [c.r, c.g, c.b, c.a];

        // Loop — active if loop_region is set (mesh-cue controls this directly)
        let (loop_start, loop_end, loop_active_f) = match overview.loop_region {
            Some((start, end)) => (start as f32, end as f32, 1.0),
            None => (0.0, 0.0, 0.0),
        };

        // Beat grid
        let (grid_step, first_beat) = if overview.beat_markers.len() > 1 {
            let total_span = overview.beat_markers.last().unwrap() - overview.beat_markers[0];
            let avg_interval = total_span as f32 / (overview.beat_markers.len() - 1) as f32;
            let first = *overview.beat_markers.first().unwrap() as f32;
            (avg_interval, first)
        } else if bpm > 0.0 && dur_f64 > 0.0 {
            let samples_per_beat = SAMPLE_RATE as f64 * 60.0 / bpm;
            let grid_step_norm = (samples_per_beat / dur_f64) as f32;
            (grid_step_norm, 0.0)
        } else {
            (0.0, 0.0)
        };

        // Volume (always 1.0 for mesh-cue)
        let volume = 1.0f32;

        // Cue markers (up to 8)
        let mut cue_positions = [[0.0f32; 4]; 2];
        let mut cue_colors = [[0.0f32; 4]; 8];
        let cue_count = overview.cue_markers.len().min(8);
        for (i, cue) in overview.cue_markers.iter().take(8).enumerate() {
            let group = i / 4;
            let slot = i % 4;
            cue_positions[group][slot] = cue.position as f32;
            cue_colors[i] = color_to_arr(cue.color);
        }

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
            view_params: [playhead, self.state.zoomed.lufs_gain, peaks_per_stem as f32, if self.is_overview { 1.0 } else { 0.0 }],
            window_params: [window_start, window_end, window_total, bpm_scale],
            stem_active,
            stem_color_0: color_to_arr(self.stem_colors[0]),
            stem_color_1: color_to_arr(self.stem_colors[1]),
            stem_color_2: color_to_arr(self.stem_colors[2]),
            stem_color_3: color_to_arr(self.stem_colors[3]),
            loop_params: [loop_start, loop_end, loop_active_f, if overview.has_track { 1.0 } else { 0.0 }],
            beat_params: [grid_step, first_beat, if self.is_overview { overview.grid_bars as f32 } else { 4.0 }, volume],
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
            stem_smooth: {
                let pis = if peaks_per_stem > 0 && overview.duration_samples > 0 {
                    let dur = overview.duration_samples as f64;
                    let spc = (overview.duration_samples / peaks_per_stem as u64) as f64;
                    if spc > 0.0 { (dur / spc) as f32 } else { peaks_per_stem as f32 }
                } else {
                    peaks_per_stem as f32
                };
                // Zoom window highlight for overview
                let (zoom_start, zoom_end) = if self.is_overview && dur_f64 > 0.0 {
                    let zoom_bars = self.state.zoomed.zoom_bars;
                    let samples_per_beat = (SAMPLE_RATE as f64 * 60.0 / bpm) as u64;
                    let samples_per_bar = samples_per_beat * 4;
                    let window_samples = samples_per_bar * zoom_bars as u64;
                    let half = window_samples as i64 / 2;
                    let vs = self.playhead as i64 - half;
                    let ve = vs + window_samples as i64;
                    let s = (vs as f64 / dur_f64).max(0.0) as f32;
                    let e = (ve as f64 / dur_f64).min(1.0) as f32;
                    (s, e)
                } else {
                    (0.0, 0.0)
                };
                [pis, zoom_start, zoom_end, 0.0]
            },
            linked_stems: [
                if self.state.linked_stems[0] { 1.0 } else { 0.0 },
                if self.state.linked_stems[1] { 1.0 } else { 0.0 },
                if self.state.linked_stems[2] { 1.0 } else { 0.0 },
                if self.state.linked_stems[3] { 1.0 } else { 0.0 },
            ],
            linked_active: [
                if self.state.linked_active[0] { 1.0 } else { 0.0 },
                if self.state.linked_active[1] { 1.0 } else { 0.0 },
                if self.state.linked_active[2] { 1.0 } else { 0.0 },
                if self.state.linked_active[3] { 1.0 } else { 0.0 },
            ],
            render_options: [2.0, 0.0, 2.0, 0.0], // mesh-cue: medium abstraction, low blur, medium depth fade
            render_options_2: [1.5, 3.0, 0.0, 0.0], // mesh-cue: medium peak width, L2 clamped AA
        }
    }
}

// =============================================================================
// View helper functions — 4-deck (mesh-player)
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

/// Create the full 4-deck waveform display using pure GPU shader rendering.
///
/// Architecture: Each deck is a column of three elements:
/// - **Zoomed waveform** (GPU shader, `Fill` height — gets all remaining space)
/// - **Deck header** (iced widgets, fixed 48px — text, badge, indicators)
/// - **Overview waveform** (GPU shader, fixed 81px — full track view)
///
/// Decks 0-1 (top row): zoomed → header → overview
/// Decks 2-3 (bottom row, mirrored): overview → header → zoomed
/// This layout clusters overviews towards the center gap.
///
/// Zero CPU tessellation — peak data uploaded once to GPU storage buffer,
/// only 400-byte uniform buffers updated per frame per view.
pub fn waveform_player_shader<'a, Message: Clone + 'a>(
    state: &'a PlayerCanvasState,
    on_action: impl Fn(WaveformAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    use iced::widget::{column, row};

    let deck_view = |idx: usize, mirrored: bool| -> Element<'a, Message> {
        let zoomed = waveform_shader_zoomed(state, idx, on_action.clone());
        let overview = waveform_shader_overview(state, idx, on_action.clone());
        let header = view_deck_header(state, idx);

        if mirrored {
            // Bottom decks: overview → header → zoomed
            column![overview, header, zoomed]
        } else {
            // Top decks: zoomed → header → overview
            column![zoomed, header, overview]
        }
        .spacing(DECK_INTERNAL_GAP)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
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

// =============================================================================
// View helper functions — single-deck (mesh-cue)
// =============================================================================

/// Create a GPU-accelerated zoomed waveform for a single deck.
///
/// Supports vinyl scratch gestures: horizontal drag = scrub, vertical drag = zoom.
/// Direction is detected after 5px of movement and locked for the gesture.
pub fn waveform_shader_single_zoomed<'a, Message: Clone + 'a>(
    state: &'a CombinedState,
    playhead: u64,
    stem_colors: [Color; 4],
    on_action: impl Fn(SingleDeckAction) -> Message + 'a,
) -> Element<'a, Message> {
    shader(SingleDeckProgram {
        state,
        playhead,
        stem_colors,
        is_overview: false,
        view_id: 100, // Distinct from 4-deck view IDs (0-7)
        on_action,
    })
    .width(Length::Fill)
    .height(Length::Fixed(ZOOMED_WAVEFORM_HEIGHT))
    .into()
}

/// Create a GPU-accelerated overview waveform for a single deck.
pub fn waveform_shader_single_overview<'a, Message: Clone + 'a>(
    state: &'a CombinedState,
    playhead: u64,
    stem_colors: [Color; 4],
    on_action: impl Fn(SingleDeckAction) -> Message + 'a,
) -> Element<'a, Message> {
    shader(SingleDeckProgram {
        state,
        playhead,
        stem_colors,
        is_overview: true,
        view_id: 101,
        on_action,
    })
    .width(Length::Fill)
    .height(Length::Fixed(WAVEFORM_HEIGHT))
    .into()
}

/// Create a combined single-deck shader waveform (zoomed + overview in a column).
///
/// Replaces the canvas-based `waveform_combined()` with GPU shader rendering.
/// Layout: zoomed (fixed height) on top, overview (fixed height) below,
/// with `COMBINED_WAVEFORM_GAP` spacing between them.
pub fn waveform_shader_combined<'a, Message: Clone + 'a>(
    state: &'a CombinedState,
    playhead: u64,
    stem_colors: [Color; 4],
    on_action: impl Fn(SingleDeckAction) -> Message + Clone + 'a,
) -> Element<'a, Message> {
    use iced::widget::column;

    let zoomed = waveform_shader_single_zoomed(state, playhead, stem_colors, on_action.clone());
    let overview = waveform_shader_single_overview(state, playhead, stem_colors, on_action);

    let combined_height = ZOOMED_WAVEFORM_HEIGHT + COMBINED_WAVEFORM_GAP + WAVEFORM_HEIGHT;

    column![zoomed, overview]
        .spacing(COMBINED_WAVEFORM_GAP)
        .width(Length::Fill)
        .height(Length::Fixed(combined_height))
        .into()
}
