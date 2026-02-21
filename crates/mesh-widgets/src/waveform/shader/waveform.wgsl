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
    slicer_params: vec4<f32>,   // slicer_start, slicer_end, current_slice, _pad
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
    stem_smooth: vec4<f32>,  // per-stem Gaussian smooth radius multiplier [vocals, drums, bass, other]
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

/// Get per-stem smooth radius multiplier from uniforms.
fn get_smooth_multiplier(stem_idx: u32) -> f32 {
    return u.stem_smooth[stem_idx];
}

/// Adaptive peak sampling with per-stem Gaussian smoothing.
///
/// Matches the old canvas rendering's abstract look:
/// - Gaussian-weighted averaging reduces detail proportional to `smooth_mult`
/// - Bass (0.4) gets heavy smoothing, Drums (0.1) stays sharp
/// - For overview (high peaks_per_pixel), uses min/max to preserve envelope shape
///
/// `peaks_per_pixel` = how many peak samples one screen pixel covers.
/// `smooth_mult` = per-stem Gaussian radius multiplier (0.1=sharp, 0.4=smooth).
fn sample_peak(stem_idx: u32, x_norm: f32, peaks_per_pixel: f32, smooth_mult: f32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    if (pps == 0u) {
        return vec2<f32>(0.0, 0.0);
    }

    let float_idx = x_norm * f32(pps);

    // Smooth radius in peak indices: scales with peaks_per_pixel and stem multiplier.
    // At 8 bars zoom with HIGHRES_WIDTH=65536, ppp ≈ 5-10, so radius ≈ 2-4 for bass.
    // Minimum radius of 2 peaks for zoomed views gives the abstract look.
    let base_radius = max(peaks_per_pixel * smooth_mult, select(0.0, 2.0 * smooth_mult, peaks_per_pixel > 0.5));
    let radius = base_radius;

    if (peaks_per_pixel > 40.0) {
        // Very zoomed out (overview): min/max aggregation for accurate envelope
        let half_range = peaks_per_pixel * 0.5;
        let start_f = max(0.0, float_idx - half_range);
        let end_f = min(f32(pps) - 1.0, float_idx + half_range);
        let start_idx = u32(start_f);
        let end_idx = u32(end_f);
        let range = end_idx - start_idx + 1u;
        let step = max(1u, range / 64u);

        var result_min = 1.0;
        var result_max = -1.0;
        var i = start_idx;
        loop {
            if (i > end_idx) { break; }
            let p = raw_peak(stem_idx, i);
            result_min = min(result_min, p.x);
            result_max = max(result_max, p.y);
            i += step;
        }
        return vec2<f32>(result_min, result_max);
    }

    // Zoomed/mid-range: Gaussian-weighted average for smooth abstract look.
    // sigma = radius / 2 gives good falloff (matches old canvas gaussian_weight).
    let sigma = max(radius * 0.5, 0.5);
    let int_radius = u32(ceil(radius));
    // Cap at 16 taps per side (33 total) — plenty for smooth results
    let capped_radius = min(int_radius, 16u);

    let center_idx = u32(clamp(floor(float_idx), 0.0, f32(pps) - 1.0));
    let window_start = select(0u, center_idx - capped_radius, center_idx >= capped_radius);
    let window_end = min(center_idx + capped_radius, pps - 1u);

    var min_sum = 0.0;
    var max_sum = 0.0;
    var weight_sum = 0.0;

    var i = window_start;
    loop {
        if (i > window_end) { break; }
        let dist = abs(f32(i) - float_idx);
        let w = exp(-0.5 * (dist / sigma) * (dist / sigma));
        let p = raw_peak(stem_idx, i);
        min_sum += p.x * w;
        max_sum += p.y * w;
        weight_sum += w;
        i += 1u;
    }

    if (weight_sum > 0.0) {
        return vec2<f32>(min_sum / weight_sum, max_sum / weight_sum);
    }
    return raw_peak(stem_idx, center_idx);
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

    // How many peak samples one screen pixel covers — drives adaptive sampling
    let peaks_per_pixel = f32(pps) * px_in_source;

    // Render order: Drums(1), Bass(2), Vocals(0), Other(3)
    let render_order = array<u32, 4>(1u, 2u, 0u, 3u);

    for (var i = 0u; i < 4u; i++) {
        let stem = render_order[i];
        let smooth_mult = get_smooth_multiplier(stem);
        let peak = sample_peak(stem, source_x, peaks_per_pixel, smooth_mult);
        let stem_color = get_stem_color(stem);
        let is_active = u.stem_active[stem] > 0.5;

        // Peak envelope: min is negative (below center), max is positive (above center)
        let y_min = center_y - peak.y * height_scale * 0.5; // max peak = top
        let y_max = center_y - peak.x * height_scale * 0.5; // min peak = bottom

        // Check if this pixel's Y falls within the envelope
        if (uv.y >= y_min && uv.y <= y_max) {
            // Edge anti-aliasing
            let pixel_h = 1.0 / height;
            let edge_top = smoothstep(y_min - pixel_h, y_min + pixel_h, uv.y);
            let edge_bot = 1.0 - smoothstep(y_max - pixel_h, y_max + pixel_h, uv.y);
            let edge_alpha = edge_top * edge_bot;

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
