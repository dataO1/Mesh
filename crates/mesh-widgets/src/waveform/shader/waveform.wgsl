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
    beat_params: vec4<f32>,     // grid_step_norm, first_beat_norm, grid_bars, volume
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
    stem_smooth: vec4<f32>,  // [peak_index_scale, zoomed_win_start, zoomed_win_end, 0]
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
fn get_subsample_target(stem_idx: u32) -> f32 {
    switch (stem_idx) {
        case 0u: { return 2.5; }  // Vocals
        case 1u: { return 2.0; }  // Drums — slightly more detail
        case 2u: { return 3.0; }  // Bass — most abstract
        default: { return 2.5; }  // Other
    }
}

/// Min/max reduction over a range of peaks.
/// Returns vec2(min_of_mins, max_of_maxes) — the TRUE envelope.
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
fn sample_peak(stem_idx: u32, x_norm: f32, peaks_per_pixel: f32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    if (pps == 0u) {
        return vec2<f32>(0.0, 0.0);
    }

    // peak_index_scale corrects for integer division in generate_peaks().
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
    let subsample_target = get_subsample_target(stem_idx);
    let step_f = max(round(subsample_target * peaks_per_pixel), 1.0);

    let grid_pos = float_idx / step_f;
    let grid_floor = floor(grid_pos);
    let grid_frac = grid_pos - grid_floor;

    let center_a = u32(clamp(grid_floor * step_f, 0.0, f32(pps) - 1.0));
    let center_b = u32(clamp((grid_floor + 1.0) * step_f, 0.0, f32(pps) - 1.0));

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

/// Blend src over dst using straight (non-premultiplied) alpha.
fn blend_over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        mix(dst.rgb, src.rgb, src.a),
        src.a + dst.a * (1.0 - src.a),
    );
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
        if (source_x >= loop_start && source_x <= loop_end) {
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
            color = blend_over(color, vec4<f32>(0.4, 0.2, 0.0, 0.15));

            let slicer_width = sl_end - sl_start;
            if (slicer_width > 0.0) {
                let slice_frac = (source_x - sl_start) / slicer_width * 16.0;
                let at_division = fract(slice_frac);
                let line_width = 16.0 * px_in_source / slicer_width;

                // Current slice highlight (brighter orange overlay)
                let current_sl = u.slicer_params.z;
                let slice_idx = floor(slice_frac);
                if (slice_idx == current_sl) {
                    color = blend_over(color, vec4<f32>(1.0, 0.6, 0.0, 0.25));
                }

                // Slice division lines
                if (at_division < line_width || at_division > 1.0 - line_width) {
                    // Highlight divider after current slice (yellow accent)
                    let divider_after = current_sl + 1.0;
                    if (divider_after < 16.0 && abs(slice_idx - divider_after) < 0.5) {
                        color = blend_over(color, vec4<f32>(1.0, 0.8, 0.2, 0.9));
                    } else {
                        color = blend_over(color, vec4<f32>(1.0, 0.5, 0.0, 0.6));
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // 3. Beat markers (grid lines)
    // -----------------------------------------------------------------
    let grid_step = u.beat_params.x;
    let first_beat = u.beat_params.y;
    let grid_bars = u.beat_params.z;       // overview: 4/8/16/32, zoomed: 1
    let beats_per_bar = 4.0;

    if (grid_step > 0.001) {
        let beat_phase = (source_x - first_beat) / grid_step;

        // Major grid lines — red, every (grid_bars * beats_per_bar) beats
        let major_interval = beats_per_bar * grid_bars;
        let major_phase = beat_phase / major_interval;
        let major_frac = fract(major_phase);
        let major_threshold = px_in_source / (grid_step * major_interval);
        if ((major_frac < major_threshold || major_frac > 1.0 - major_threshold) && beat_phase > -0.5) {
            color = blend_over(color, vec4<f32>(1.0, 0.3, 0.3, 0.5));
        } else if (!is_overview) {
            // Beat lines — subtle gray (zoomed view only)
            let beat_frac = fract(beat_phase);
            let beat_threshold = px_in_source / grid_step;
            if ((beat_frac < beat_threshold || beat_frac > 1.0 - beat_threshold) && beat_phase > -0.5) {
                color = blend_over(color, vec4<f32>(0.3, 0.3, 0.3, 0.3));
            }
        }
    }

    // -----------------------------------------------------------------
    // 4. Playhead proximity factor (used by stem brightness below)
    // -----------------------------------------------------------------
    // Inverse exponential: tight bright zone around playhead, rapid falloff.
    // Only active in zoomed view — overview has no spatial playhead focus.
    var playhead_proximity = 0.0;
    if (!is_overview) {
        let glow_x = clamp(abs(uv.x - 0.5) / 0.5, 0.0, 1.0);
        playhead_proximity = exp(-5.0 * glow_x);
    }

    // -----------------------------------------------------------------
    // 5. Overview: zoomed window indicator (highlight visible region)
    // -----------------------------------------------------------------
    if (is_overview) {
        let zoom_win_start = u.stem_smooth[1];
        let zoom_win_end = u.stem_smooth[2];
        // Only draw if we have valid window data
        if (zoom_win_end > zoom_win_start) {
            var indicator_x = source_x;
            // Apply BPM stretch to indicator position to match overview stretch
            let bpm_scale_ind = u.window_params.w;
            if (bpm_scale_ind > 0.01) {
                indicator_x = source_x; // source_x already un-stretched
            }
            if (indicator_x >= zoom_win_start && indicator_x <= zoom_win_end) {
                // Subtle bright tint for the visible region
                color = blend_over(color, vec4<f32>(0.4, 0.4, 0.5, 0.12));
            }
        }
    }

    // -----------------------------------------------------------------
    // 6. Stem envelopes (back-to-front: Drums, Bass, Vocals, Other)
    // -----------------------------------------------------------------
    let center_y = 0.5;
    let height_scale = u.view_params.y;

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
    let fw = fwidth(uv.y);

    for (var i = 0u; i < 4u; i++) {
        let stem = render_order[i];
        let peak = sample_peak(stem, source_x, peaks_per_pixel);
        let stem_color = get_stem_color(stem);
        let is_active = u.stem_active[stem] > 0.5;

        // Peak envelope: min is negative (below center), max is positive (above center)
        let y_min = center_y - peak.y * height_scale * 0.5; // max peak = top
        let y_max = center_y - peak.x * height_scale * 0.5; // min peak = bottom

        // Soft anti-aliased edge with outside glow for smooth appearance.
        let d_top = uv.y - y_min;
        let d_bot = y_max - uv.y;
        let outside_ext = fw * 1.5;
        let aa_top = smoothstep(-outside_ext, fw, d_top);
        let aa_bot = smoothstep(-outside_ext, fw, d_bot);
        var edge_alpha = aa_top * aa_bot;

        // Sub-pixel thin envelope coverage boost
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
                // Envelope-relative brightness modulated by playhead proximity.
                // Near playhead: center slightly brighter, edges glow strongly.
                // Far from playhead: normal flat brightness.
                let env_center = (y_min + y_max) * 0.5;
                let env_half = max((y_max - y_min) * 0.5, fw);
                let rel_pos = clamp(abs(uv.y - env_center) / env_half, 0.0, 1.0);
                // 0.15 base boost (center less dark) + 0.45 edge boost (edges glow)
                let edge_boost = 1.0 + playhead_proximity * (0.15 + 0.45 * rel_pos);
                stem_rgba = vec4<f32>(stem_color.rgb * edge_boost, 0.85 * edge_alpha);
            } else {
                // Muted: gray with reduced opacity
                stem_rgba = vec4<f32>(0.35, 0.35, 0.35, 0.5 * edge_alpha);
            }
            color = blend_over(color, stem_rgba);
        }
    }

    // -----------------------------------------------------------------
    // 7. Cue markers (colored vertical lines + triangle)
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
    // 8. Playhead (white vertical line)
    // -----------------------------------------------------------------
    var ph_x: f32;
    if (is_overview) {
        ph_x = playhead;
        let bpm_scale = u.window_params.w;
        if (bpm_scale > 0.01) {
            ph_x = playhead * bpm_scale;
        }
    } else {
        // Zoomed: playhead is at center
        ph_x = 0.5;
    }

    let ph_dist: f32 = abs(uv.x - ph_x);
    let ph_line_w = 1.5 / width;
    if (ph_dist < ph_line_w) {
        let ph_alpha = smoothstep(ph_line_w, 0.0, ph_dist);
        color = blend_over(color, vec4<f32>(1.0, 1.0, 1.0, ph_alpha));
    }

    // -----------------------------------------------------------------
    // 9. Volume dimming (semi-transparent black overlay)
    // -----------------------------------------------------------------
    let volume = u.beat_params.w;
    if (volume < 0.999) {
        let dim_alpha = (1.0 - volume) * 0.4;
        color = blend_over(color, vec4<f32>(0.0, 0.0, 0.0, dim_alpha));
    }

    return color;
}
