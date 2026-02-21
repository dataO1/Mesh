// GPU waveform renderer — renders stem envelopes, beat markers, cue markers,
// loop/slicer regions, playhead, and volume dimming in a single fragment pass.
//
// Peak data arrives via storage buffer (uploaded once at track load).
// Only the 384-byte uniform buffer is updated per frame.

struct Uniforms {
    bounds: vec4<f32>,          // x, y, width, height (logical pixels)
    view_params: vec4<f32>,     // playhead_norm, height_scale, peaks_per_stem, is_overview
    window_params: vec4<f32>,   // window_start, window_end, window_total_peaks, bpm_scale
    stem_active: vec4<f32>,     // 0.0/1.0 per stem
    stem_color_0: vec4<f32>,
    stem_color_1: vec4<f32>,
    stem_color_2: vec4<f32>,
    stem_color_3: vec4<f32>,
    loop_params: vec4<f32>,     // loop_start, loop_end, loop_active, has_track
    beat_params: vec4<f32>,     // grid_step_norm, first_beat_norm, beats_per_bar, volume
    cue_params: vec4<f32>,      // cue_count, main_cue_pos, has_main_cue, slicer_active
    slicer_params: vec4<f32>,   // slicer_start, slicer_end, current_slice, peaks_per_pixel
    cue_pos_0_3: vec4<f32>,
    cue_pos_4_7: vec4<f32>,
    cue_color_0: vec4<f32>,
    cue_color_1: vec4<f32>,
    cue_color_2: vec4<f32>,
    cue_color_3: vec4<f32>,
    cue_color_4: vec4<f32>,
    cue_color_5: vec4<f32>,
    cue_color_6: vec4<f32>,
    cue_color_7: vec4<f32>,
    stem_smooth: vec4<f32>,  // [peak_index_scale, 0, 0, 0]
}

@group(0) @binding(0)
var<uniform> u: Uniforms;

@group(0) @binding(1)
var<storage, read> peaks: array<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// =============================================================================
// Vertex shader — fullscreen triangle (same technique as knob.wgsl)
// =============================================================================

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    // Oversized triangle clipped to viewport rectangle
    let x = select(-1.0, 3.0, vertex_index == 1u);
    let y = select(-1.0, 3.0, vertex_index == 2u);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// =============================================================================
// Helper functions
// =============================================================================

/// Read a single raw peak at integer index for a stem.
fn raw_peak(stem_idx: u32, idx: u32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    let clamped = min(idx, pps - 1u);
    let base = (stem_idx * pps + clamped) * 2u;
    return vec2<f32>(peaks[base], peaks[base + 1u]);
}

/// Per-stem subsampling target (pixels per rendered point).
/// Higher = more subsampling = more abstract/smooth appearance.
/// Tuned so that at 4-8 bars visible, the waveform has an abstract look
/// rather than showing every individual peak.
fn get_subsample_target(stem_idx: u32) -> f32 {
    switch (stem_idx) {
        case 0u: { return 2.5; }  // Vocals
        case 1u: { return 2.0; }  // Drums — slightly more detail than others
        case 2u: { return 3.0; }  // Bass — most abstract
        default: { return 2.5; }  // Other
    }
}

/// Min/max reduction over a range of peaks.
/// Returns vec2(min_of_mins, max_of_maxes) — the TRUE envelope.
/// This is the mathematically correct operation for waveform display:
/// if any peak in the range hit -0.8, the pixel must show -0.8.
/// Unlike Gaussian averaging, min/max is monotonic — adding peaks to
/// the range can only extend the envelope, never shrink it — so it
/// produces rock-stable display with no "dancing" artifacts.
fn minmax_reduce(stem_idx: u32, start: u32, end_idx: u32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    let s = min(start, pps - 1u);
    let e = min(end_idx, pps - 1u);

    // Cap iterations at 64 per grid point to stay in GPU budget
    let range = e - s + 1u;
    let step = max(1u, range / 64u);

    var result_min = 1.0;
    var result_max = -1.0;
    var i = s;
    loop {
        if (i > e) { break; }
        let p = raw_peak(stem_idx, i);
        result_min = min(result_min, p.x);
        result_max = max(result_max, p.y);
        i += step;
    }
    return vec2<f32>(result_min, result_max);
}

/// Stable peak sampling with grid-aligned min/max reduction.
///
/// Algorithm:
/// 1. Compute step = round(subsample_target * peaks_per_pixel)
///    peaks_per_pixel is a CPU-computed uniform (not derived from
///    win_end - win_start in the shader), eliminating float instability.
/// 2. Snap to step-aligned grid anchored at peak index 0.
///    The grid never shifts relative to the track.
/// 3. At each grid point, take TRUE min/max over the half-step range.
///    This is correct for waveform envelopes (not averaging).
/// 4. Linearly interpolate between the two nearest grid points.
///    This produces the abstract "fewer points connected by lines" look
///    matching the old canvas path rendering.
///
/// For overview (very zoomed out): full min/max over the pixel's range.
/// For deep zoom: grid path with step_f=1 degenerates to linear interpolation
/// between adjacent raw peaks — same result but no discontinuous strategy switch.
fn sample_peak(stem_idx: u32, x_norm: f32, peaks_per_pixel: f32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    if (pps == 0u) {
        return vec2<f32>(0.0, 0.0);
    }

    // peak_index_scale corrects for integer division in generate_peaks().
    // The peaks array has `pps` entries, but each covers floor(duration/pps) samples,
    // NOT duration/pps samples. The last bin absorbs the remainder.
    // peak_index_scale = duration / floor(duration/pps) = the "effective pps" that
    // correctly maps normalized position to peak index.
    let peak_index_scale = u.stem_smooth[0];
    let effective_pps = select(f32(pps), peak_index_scale, peak_index_scale > 0.0);
    let float_idx = x_norm * effective_pps;

    // Overview (very zoomed out): simple min/max over pixel's range
    if (peaks_per_pixel > 40.0) {
        let half_range = peaks_per_pixel * 0.5;
        let start_idx = u32(max(0.0, float_idx - half_range));
        let end_idx = u32(min(f32(pps) - 1.0, float_idx + half_range));
        return minmax_reduce(stem_idx, start_idx, end_idx);
    }

    // --- Grid-aligned min/max sampling (zoomed/mid-range) ---
    //
    // Step size: how many peaks each rendered "point" covers.
    // peaks_per_pixel is a stable CPU-computed uniform, so step_f is
    // constant across all pixels and all frames at the same zoom level.
    // This eliminates the grid restructuring that caused dancing.
    let subsample_target = get_subsample_target(stem_idx);
    let step_f = max(round(subsample_target * peaks_per_pixel), 1.0);

    // Grid position: snap to multiples of step_f anchored at peak 0.
    // As the window scrolls, only grid_frac changes (smoothly 0→1).
    // At each grid crossing, idx_a/idx_b shift to the next pair —
    // same as the old canvas scrolling behavior.
    let grid_pos = float_idx / step_f;
    let grid_floor = floor(grid_pos);
    let grid_frac = grid_pos - grid_floor;

    // Two nearest grid-aligned peak indices
    let center_a = u32(clamp(grid_floor * step_f, 0.0, f32(pps) - 1.0));
    let center_b = u32(clamp((grid_floor + 1.0) * step_f, 0.0, f32(pps) - 1.0));

    // Min/max reduction over the half-step range at each grid point.
    // This captures the true envelope around each grid point without
    // the averaging artifacts of Gaussian smoothing.
    let half_step = u32(step_f * 0.5);
    let p_a = minmax_reduce(
        stem_idx,
        select(0u, center_a - half_step, center_a >= half_step),
        min(center_a + half_step, pps - 1u)
    );
    let p_b = minmax_reduce(
        stem_idx,
        select(0u, center_b - half_step, center_b >= half_step),
        min(center_b + half_step, pps - 1u)
    );

    // Linear interpolation between grid-aligned min/max samples.
    // This is the shader equivalent of the canvas path connecting
    // step-aligned points with straight lines — producing the
    // abstract reduced-detail look.
    return mix(p_a, p_b, vec2<f32>(grid_frac));
}

/// Get stem color by index
fn get_stem_color(idx: u32) -> vec4<f32> {
    switch (idx) {
        case 0u: { return u.stem_color_0; }
        case 1u: { return u.stem_color_1; }
        case 2u: { return u.stem_color_2; }
        default: { return u.stem_color_3; }
    }
}

/// Get cue position by index
fn get_cue_pos(idx: u32) -> f32 {
    if (idx < 4u) {
        return u.cue_pos_0_3[idx];
    }
    return u.cue_pos_4_7[idx - 4u];
}

/// Get cue color by index
fn get_cue_color(idx: u32) -> vec4<f32> {
    switch (idx) {
        case 0u: { return u.cue_color_0; }
        case 1u: { return u.cue_color_1; }
        case 2u: { return u.cue_color_2; }
        case 3u: { return u.cue_color_3; }
        case 4u: { return u.cue_color_4; }
        case 5u: { return u.cue_color_5; }
        case 6u: { return u.cue_color_6; }
        default: { return u.cue_color_7; }
    }
}

/// Blend src over dst using premultiplied alpha
fn blend_over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

// =============================================================================
// Fragment shader — renders everything in one pass, back-to-front
// =============================================================================

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let width = u.bounds.z;
    let height = u.bounds.w;
    let has_track = u.loop_params.w;
    let is_overview = u.view_params.w > 0.5;
    let playhead = u.view_params.x;
    let pps = u32(u.view_params.z);

    // Background
    var color = vec4<f32>(0.08, 0.08, 0.08, 1.0);

    // No track loaded — just show dark background
    if (has_track < 0.5 || pps == 0u) {
        return color;
    }

    // Map UV to track-space x coordinate
    var source_x: f32;
    if (is_overview) {
        // Overview: BPM stretching
        let bpm_scale = u.window_params.w;
        if (bpm_scale > 0.01) {
            source_x = uv.x / bpm_scale;
        } else {
            source_x = uv.x;
        }
        // Beyond stretched range = silence
        if (source_x > 1.0) {
            return color;
        }
    } else {
        // Zoomed: window into the track
        let win_start = u.window_params.x;
        let win_end = u.window_params.y;
        source_x = win_start + uv.x * (win_end - win_start);

        // Clamp to track bounds
        if (source_x < 0.0 || source_x > 1.0) {
            return color;
        }
    }

    // Compute source_x change per pixel — needed for correct line widths.
    // Overview: one pixel = 1/width of track (or 1/(width*bpm_scale) with stretch)
    // Zoomed: one pixel = (win_end - win_start) / width of track
    var px_in_source: f32;
    if (is_overview) {
        let bpm_scale_px = u.window_params.w;
        if (bpm_scale_px > 0.01) {
            px_in_source = 1.0 / (width * bpm_scale_px);
        } else {
            px_in_source = 1.0 / width;
        }
    } else {
        px_in_source = (u.window_params.y - u.window_params.x) / width;
    }

    // -----------------------------------------------------------------
    // 1. Loop region (green tint if active)
    // -----------------------------------------------------------------
    let loop_active = u.loop_params.z > 0.5;
    if (loop_active) {
        let loop_start = u.loop_params.x;
        let loop_end = u.loop_params.y;
        var in_loop: bool;
        if (is_overview) {
            in_loop = source_x >= loop_start && source_x <= loop_end;
        } else {
            in_loop = source_x >= loop_start && source_x <= loop_end;
        }
        if (in_loop) {
            color = blend_over(color, vec4<f32>(0.0, 0.3, 0.0, 0.25));
        }
    }

    // -----------------------------------------------------------------
    // 2. Slicer region (orange tint + slice lines)
    // -----------------------------------------------------------------
    let slicer_active = u.cue_params.w > 0.5;
    if (slicer_active) {
        let sl_start = u.slicer_params.x;
        let sl_end = u.slicer_params.y;
        if (source_x >= sl_start && source_x <= sl_end) {
            // Orange tint for slicer region
            color = blend_over(color, vec4<f32>(0.4, 0.2, 0.0, 0.15));

            // Slice division lines (8 slices)
            let slicer_width = sl_end - sl_start;
            if (slicer_width > 0.0) {
                let slice_frac = (source_x - sl_start) / slicer_width * 8.0;
                let at_division = fract(slice_frac);
                // One pixel in fract(slice_frac) space = 8 * px_in_source / slicer_width
                let line_width = 8.0 * px_in_source / slicer_width;
                if (at_division < line_width || at_division > 1.0 - line_width) {
                    color = blend_over(color, vec4<f32>(1.0, 0.5, 0.0, 0.6));
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // 3. Beat markers (procedural from BPM)
    // -----------------------------------------------------------------
    let grid_step = u.beat_params.x;
    let first_beat = u.beat_params.y;
    let beats_per_bar = u.beat_params.z;

    if (grid_step > 0.001) {
        let beat_phase = (source_x - first_beat) / grid_step;

        // Bar lines (every beats_per_bar beats) — brighter
        // Threshold: how many bars does 2 pixels span? = 2 * px_in_source / (grid_step * beats_per_bar)
        let bar_phase = beat_phase / beats_per_bar;
        let bar_frac = fract(bar_phase);
        let bar_threshold = px_in_source * 2.0 / (grid_step * beats_per_bar);
        if (bar_frac < bar_threshold || bar_frac > 1.0 - bar_threshold) {
            if (beat_phase > -0.5) { // Only after first beat
                color = blend_over(color, vec4<f32>(0.4, 0.4, 0.4, 0.5));
            }
        } else {
            // Beat lines — subtle
            // Threshold: how many beats does 1 pixel span? = px_in_source / grid_step
            let beat_frac = fract(beat_phase);
            let beat_threshold = px_in_source / grid_step;
            if (beat_frac < beat_threshold || beat_frac > 1.0 - beat_threshold) {
                if (beat_phase > -0.5) {
                    color = blend_over(color, vec4<f32>(0.3, 0.3, 0.3, 0.3));
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // 4. Stem envelopes (back-to-front: Drums, Bass, Vocals, Other)
    // -----------------------------------------------------------------
    let center_y = 0.5;
    let height_scale = u.view_params.y;

    // peaks_per_pixel: CPU-computed stable uniform for zoomed view,
    // or derived from px_in_source for overview.
    // Using the CPU uniform avoids float subtraction noise from
    // (win_end - win_start) / width that caused step_f instability.
    var peaks_per_pixel: f32;
    let cpu_ppp = u.slicer_params.w;
    if (is_overview || cpu_ppp <= 0.0) {
        peaks_per_pixel = f32(pps) * px_in_source;
    } else {
        peaks_per_pixel = cpu_ppp;
    }

    // Render order: Drums(1), Bass(2), Vocals(0), Other(3)
    let render_order = array<u32, 4>(1u, 2u, 0u, 3u);

    // Resolution-independent pixel size via screen-space derivatives.
    // fwidth(uv.y) = how much uv.y changes per pixel — adapts to DPI & viewport.
    let fw = fwidth(uv.y);

    for (var i = 0u; i < 4u; i++) {
        let stem = render_order[i];
        let peak = sample_peak(stem, source_x, peaks_per_pixel);
        let stem_color = get_stem_color(stem);
        let is_active = u.stem_active[stem] > 0.5;

        // Peak envelope: min is negative (below center), max is positive (above center)
        let y_min = center_y - peak.y * height_scale * 0.5; // max peak = top
        let y_max = center_y - peak.x * height_scale * 0.5; // min peak = bottom

        // Inside-only anti-aliasing using fwidth — NO hard if-guard.
        //
        // The old hard guard `if (uv.y >= y_min && uv.y <= y_max)` caused thin peaks
        // to "dance": a ~1px peak only passes the test for ONE pixel row, and sub-pixel
        // position shifts between frames cause it to jump to a different row.
        //
        // smoothstep(0, fw, d): transition is entirely INSIDE the envelope.
        // Outside pixels get alpha=0, so no stem color bleeds beyond its envelope.
        // This prevents mixed-color outlines where two stems overlap.
        // Thin sub-pixel peaks are handled by the coverage boost below.
        let d_top = uv.y - y_min;  // positive = inside envelope
        let d_bot = y_max - uv.y;  // positive = inside envelope
        let aa_top = smoothstep(0.0, fw, d_top);
        let aa_bot = smoothstep(0.0, fw, d_bot);
        var edge_alpha = aa_top * aa_bot;

        // For sub-pixel thin envelopes (< 2px), the overlapping smoothstep
        // transitions produce very low alpha. Boost proportional to coverage,
        // but ONLY for pixels near the envelope center — without the proximity
        // check, `max(edge_alpha, coverage)` would override the smoothstep's
        // spatial falloff and paint the entire column with the stem color.
        let thickness = y_max - y_min;
        if (thickness < 2.0 * fw && thickness > 0.0) {
            let coverage = thickness / (2.0 * fw);
            let center = (y_min + y_max) * 0.5;
            let proximity = smoothstep(fw, 0.0, abs(uv.y - center));
            edge_alpha = max(edge_alpha, proximity * coverage * 0.8);
        }

        if (edge_alpha > 0.005) {
            var stem_rgba: vec4<f32>;
            if (is_active) {
                stem_rgba = vec4<f32>(stem_color.rgb, 0.85 * edge_alpha);
            } else {
                // Muted: gray with reduced opacity
                stem_rgba = vec4<f32>(0.35, 0.35, 0.35, 0.5 * edge_alpha);
            }
            color = blend_over(color, stem_rgba);
        }
    }

    // -----------------------------------------------------------------
    // 5. Cue markers (colored vertical lines + triangle)
    // -----------------------------------------------------------------
    let cue_count = u32(u.cue_params.x);
    let cue_line_w = px_in_source * 2.0;

    for (var i = 0u; i < cue_count; i++) {
        let cue_pos = get_cue_pos(i);
        let cue_col = get_cue_color(i);
        let dist = abs(source_x - cue_pos);

        if (dist < cue_line_w) {
            let alpha = smoothstep(cue_line_w, 0.0, dist);
            color = blend_over(color, vec4<f32>(cue_col.rgb, 0.8 * alpha));
        }

        // Small triangle indicator at top
        let tri_size = 6.0 / height;
        if (uv.y < tri_size && dist < tri_size) {
            let tri_alpha = smoothstep(tri_size, 0.0, max(uv.y, dist));
            color = blend_over(color, vec4<f32>(cue_col.rgb, tri_alpha));
        }
    }

    // Main cue marker (wider, white)
    let has_main_cue = u.cue_params.z > 0.5;
    if (has_main_cue) {
        let main_pos = u.cue_params.y;
        let main_dist = abs(source_x - main_pos);
        let main_line_w = px_in_source * 2.5;
        if (main_dist < main_line_w) {
            let alpha = smoothstep(main_line_w, 0.0, main_dist);
            color = blend_over(color, vec4<f32>(1.0, 1.0, 1.0, 0.9 * alpha));
        }
    }

    // -----------------------------------------------------------------
    // 6. Playhead (white vertical line)
    // -----------------------------------------------------------------
    var ph_x: f32;
    if (is_overview) {
        ph_x = playhead;
        // Apply BPM stretch to playhead position
        let bpm_scale = u.window_params.w;
        if (bpm_scale > 0.01) {
            ph_x = playhead * bpm_scale;
        }
    } else {
        // Zoomed: playhead is at center
        ph_x = 0.5;
        // Convert to UV space since we're using source_x for track-space
    }

    let ph_dist: f32 = abs(uv.x - ph_x);
    let ph_line_w = 1.5 / width;
    if (ph_dist < ph_line_w) {
        let ph_alpha = smoothstep(ph_line_w, 0.0, ph_dist);
        color = blend_over(color, vec4<f32>(1.0, 1.0, 1.0, ph_alpha));
    }

    // -----------------------------------------------------------------
    // 7. Volume dimming (semi-transparent black overlay)
    // -----------------------------------------------------------------
    let volume = u.beat_params.w;
    if (volume < 0.999) {
        let dim_alpha = (1.0 - volume) * 0.4;
        color = blend_over(color, vec4<f32>(0.0, 0.0, 0.0, dim_alpha));
    }

    return color;
}
