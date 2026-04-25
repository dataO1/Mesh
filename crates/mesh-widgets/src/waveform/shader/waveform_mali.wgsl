// GPU waveform renderer — Mali G610 hyper-optimized variant.
//
// This shader produces visually identical core waveform rendering to waveform.wgsl
// but is hyper-optimized for the Mali Valhall TBDR architecture:
//
//   ARCHITECTURE: Peaks are precomputed on CPU — one (min,max) per pixel column
//   per stem. The shader does simple buffer reads with ZERO minmax_reduce loops.
//   This guarantees the 1:1 peak-per-pixel invariant at ALL zoom levels, enabling
//   direct peak load + analytical slope AA everywhere.
//
//   REMOVED: depth fade, peak width expansion, stem indicators, linked stem split mode,
//            motion blur branching, playhead proximity glow, fwidth() derivative,
//            minmax_reduce loop, sample_peak branching, subsampling logic
//
//   REPLACED: smoothstep → linear clamp (0.75/1.5 zone), sqrt L2 → linear slope,
//             dpdx/dpdy → per-edge slope from adjacent precomputed peaks,
//             blend_over → vec3 mix (background always opaque)
//
// Uniform layout is IDENTICAL to waveform.wgsl — same WaveformUniforms struct.
// view_params.z = view width in pixels (number of precomputed peaks per stem).
// render_options_2[2] = precomputed fw = 1.0/height.

struct Uniforms {
    bounds: vec4<f32>,
    view_params: vec4<f32>,
    window_params: vec4<f32>,
    stem_active: vec4<f32>,
    stem_color_0: vec4<f32>,
    stem_color_1: vec4<f32>,
    stem_color_2: vec4<f32>,
    stem_color_3: vec4<f32>,
    loop_params: vec4<f32>,
    beat_params: vec4<f32>,
    cue_params: vec4<f32>,
    slicer_params: vec4<f32>,
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
    stem_smooth: vec4<f32>,
    linked_stems: vec4<f32>,
    linked_active: vec4<f32>,
    render_options: vec4<f32>,
    render_options_2: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> u: Uniforms;

@group(0) @binding(1)
var<storage, read> peaks: array<vec2<f32>>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// =============================================================================
// Vertex shader — fullscreen triangle
// =============================================================================

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = select(-1.0, 3.0, vertex_index == 1u);
    let y = select(-1.0, 3.0, vertex_index == 2u);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

// =============================================================================
// Helpers — minimal for CPU-precomputed peaks
// =============================================================================

/// Read precomputed peak by pixel column index.
/// CPU computes one (min,max) per pixel per stem — the GPU just reads.
fn read_peak(stem_idx: u32, pixel_col: u32) -> vec2<f32> {
    let pps = u32(u.view_params.z);
    if (pps == 0u) { return vec2<f32>(0.0); }
    return peaks[stem_idx * pps + min(pixel_col, pps - 1u)];
}

fn get_stem_color(idx: u32) -> vec4<f32> {
    switch (idx) {
        case 0u: { return u.stem_color_0; }
        case 1u: { return u.stem_color_1; }
        case 2u: { return u.stem_color_2; }
        default: { return u.stem_color_3; }
    }
}

fn get_cue_pos(idx: u32) -> f32 {
    if (idx < 4u) { return u.cue_pos_0_3[idx]; }
    return u.cue_pos_4_7[idx - 4u];
}

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

// =============================================================================
// Fragment shader — hyper-optimized for Mali Valhall
// =============================================================================
//
// Key differences from waveform.wgsl:
// - CPU-precomputed peaks: 1 buffer read per pixel per stem, zero reduction loops
// - vec3 color accumulation (background always opaque, skip alpha math)
// - fw from uniform (no fwidth derivative)
// - Analytical slope AA from adjacent precomputed peak (no dpdx/dpdy)
// - Linear clamp AA (no smoothstep polynomial)
// - No depth fade, peak width, stem indicators, playhead glow

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let width = u.bounds.z;
    let height = u.bounds.w;
    let has_track = u.loop_params.w;
    let is_overview = u.view_params.w > 0.5;
    let pps = u32(u.view_params.z);

    // Background — vec3, no alpha tracking needed
    var color = vec3<f32>(0.08, 0.08, 0.08);

    // No track at all — dark background
    if (has_track < 0.5) {
        return vec4<f32>(color, 1.0);
    }

    // Loading pulse: > 0 while audio is loading, 0 when peaks have arrived
    let loading_pulse = u.render_options_2[3];

    // -----------------------------------------------------------------
    // Map UV to source_x (needed for beat grid, cues, loop regions)
    // -----------------------------------------------------------------
    var source_x: f32;
    if (is_overview) {
        let bpm_scale = u.window_params.w;
        if (bpm_scale > 0.01) {
            source_x = uv.x / bpm_scale;
        } else {
            source_x = uv.x;
        }
        if (source_x > 1.0) {
            return vec4<f32>(color, 1.0);
        }
    } else {
        let win_start = u.window_params.x;
        let win_end = u.window_params.y;
        source_x = win_start + uv.x * (win_end - win_start);
        if (source_x < 0.0 || source_x > 1.0) {
            return vec4<f32>(color, 1.0);
        }
    }

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
    // Loop region tint
    // -----------------------------------------------------------------
    if (u.loop_params.z > 0.5) {
        let ls = u.loop_params.x;
        let le = u.loop_params.y;
        if (source_x >= ls && source_x <= le) {
            color = mix(color, vec3<f32>(0.1, 0.5, 0.1), 0.3);
        }
    }

    // -----------------------------------------------------------------
    // Slicer region
    // -----------------------------------------------------------------
    if (u.cue_params.w > 0.5) {
        let sl_s = u.slicer_params.x;
        let sl_e = u.slicer_params.y;
        if (source_x >= sl_s && source_x <= sl_e) {
            color = mix(color, vec3<f32>(0.4, 0.2, 0.0), 0.15);
            let sw = sl_e - sl_s;
            if (sw > 0.0) {
                let sf = (source_x - sl_s) / sw * 16.0;
                let at_div = fract(sf);
                let lw = 16.0 * px_in_source / sw;
                let cur = u.slicer_params.z;
                if (floor(sf) == cur) {
                    color = mix(color, vec3<f32>(1.0, 0.6, 0.0), 0.25);
                }
                if (at_div < lw || at_div > 1.0 - lw) {
                    let after = cur + 1.0;
                    if (after < 16.0 && abs(floor(sf) - after) < 0.5) {
                        color = mix(color, vec3<f32>(1.0, 0.8, 0.2), 0.9);
                    } else {
                        color = mix(color, vec3<f32>(1.0, 0.5, 0.0), 0.6);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------
    // Beat grid
    // -----------------------------------------------------------------
    let grid_step = u.beat_params.x;
    let first_beat = u.beat_params.y;
    let grid_beats = u.beat_params.z;

    if (grid_step > 0.0000001) {
        let bp = (source_x - first_beat) / grid_step;
        let mi = grid_beats;
        let mf = fract(bp / mi);
        let mt = px_in_source / (grid_step * mi);
        let ma = select(0.5, 0.35, is_overview);

        // Phrase highlight: overview only, every 4th red bar marker.
        // Reuses the vocals stem color (green across all built-in themes).
        // Phrase fires only WHERE a red bar fires AND the rounded bar index is a
        // multiple of 4. Integer round + modulo is stable in both directions of
        // the anchor (no fract() drift on a 4×-wider period). Same pixel width
        // as red, both pre- and post-anchor.
        let red_hit = mf < mt || mf > 1.0 - mt;
        let bar_idx = i32(round(bp / mi));
        let phrase_hit = is_overview && red_hit && (bar_idx % 4 == 0);

        if (phrase_hit) {
            color = mix(color, u.stem_color_0.rgb, 0.9);
        } else if (red_hit) {
            color = mix(color, vec3<f32>(1.0, 0.3, 0.3), ma);
        } else {
            let ni = select(1.0, grid_beats / 4.0, is_overview);
            let nf = fract(bp / ni);
            let nt = px_in_source / (grid_step * ni);
            let na = select(0.3, 0.18, is_overview);
            if (nf < nt || nf > 1.0 - nt) {
                color = mix(color, vec3<f32>(0.3, 0.3, 0.3), na);
            }
        }
    }

    // -----------------------------------------------------------------
    // Overview: zoomed window indicator
    // -----------------------------------------------------------------
    if (is_overview) {
        let zs = u.stem_smooth[1];
        let ze = u.stem_smooth[2];
        if (ze > zs) {
            if (source_x >= zs && source_x <= ze) {
                color = mix(color, vec3<f32>(0.4, 0.4, 0.5), 0.12);
            }
        }
    }

    // -----------------------------------------------------------------
    // Stem envelopes — CPU-precomputed peak reads
    //   Only rendered when peak data is available (pps > 0)
    // -----------------------------------------------------------------
    if (pps > 0u) {
    let center_y = 0.5;
    let height_scale = u.view_params.y;
    let half_hs = height_scale * 0.5;

    // Pre-computed pixel size from CPU (exact, no fwidth derivative)
    let fw = u.render_options_2[2];

    // AA constants — tightened linear clamp (1.5fw zone ≈ smoothstep 2.5fw crispness)
    let fw3 = fw * 3.0;
    let fw_muted_inv = 1.0 / (fw * 1.5); // precomputed reciprocal for muted stems

    let render_order = array<u32, 4>(1u, 2u, 0u, 3u);

    // Pixel column index — CPU guarantees 1:1 peak per pixel at ALL zoom levels
    let width_u = max(u32(width), 1u);
    let pixel_col = min(u32(uv.x * width), width_u - 1u);

    for (var i = 0u; i < 4u; i++) {
        let stem = render_order[i];
        let is_active = u.stem_active[stem] > 0.5;

        // Linked stem swap (zoomed view only, no split rendering)
        var effective_stem = stem;
        if (!is_overview && u.linked_stems[stem] > 0.5 && u.linked_active[stem] > 0.5) {
            effective_stem = stem + 4u;
        }

        let peak = read_peak(effective_stem, pixel_col);

        // Core envelope
        let env_top = center_y - peak.y * half_hs;
        let env_bot = center_y - peak.x * half_hs;
        let d_top = uv.y - env_top;
        let d_bot = env_bot - uv.y;

        // --- Muted stem: simplified AA (no slope, precomputed reciprocal) ---
        if (!is_active) {
            let aa_top = clamp(d_top * fw_muted_inv + 0.5, 0.0, 1.0);
            let aa_bot = clamp(d_bot * fw_muted_inv + 0.5, 0.0, 1.0);
            let ea = aa_top * aa_bot;
            if (ea > 0.005) {
                color = mix(color, vec3<f32>(0.35), 0.5 * ea);
            }
            continue;
        }

        // --- Active stem: per-edge slope-aware AA (no sqrt, no max) ---
        // Adjacent precomputed peak gives visual slope — separate per edge so a
        // flat top doesn't inherit a steep bottom's blur width.
        let pn = read_peak(effective_stem, min(pixel_col + 1u, width_u - 1u));
        let fw_top = clamp(abs(pn.y - peak.y) * half_hs + fw, fw, fw3);
        let fw_bot = clamp(abs(pn.x - peak.x) * half_hs + fw, fw, fw3);

        // Linear clamp AA: d/(fw*1.5) + 0.5 (0.75/1.5 = 0.5 offset)
        let aa_top = clamp(d_top / (fw_top * 1.5) + 0.5, 0.0, 1.0);
        let aa_bot = clamp(d_bot / (fw_bot * 1.5) + 0.5, 0.0, 1.0);
        let edge_alpha = aa_top * aa_bot;

        if (edge_alpha > 0.005) {
            let sc = get_stem_color(stem);
            color = mix(color, sc.rgb, 0.85 * edge_alpha);
        }
    }
    } // end if (pps > 0u) — stem envelopes

    // -----------------------------------------------------------------
    // Cue markers (linear AA, no smoothstep)
    // -----------------------------------------------------------------
    let cue_count = u32(u.cue_params.x);
    let cue_line_w = px_in_source * 2.0;

    for (var i = 0u; i < cue_count; i++) {
        let cp = get_cue_pos(i);
        let dist = abs(source_x - cp);
        if (dist < cue_line_w) {
            let cc = get_cue_color(i);
            let alpha = 1.0 - dist / cue_line_w;
            color = mix(color, cc.rgb, 0.8 * alpha);

            // Triangle indicator at top
            let tri_size = 6.0 / height;
            if (uv.y < tri_size) {
                let ta = 1.0 - max(uv.y, dist) / tri_size;
                color = mix(color, cc.rgb, ta);
            }
        }
    }

    // Main cue marker
    if (u.cue_params.z > 0.5) {
        let mp = u.cue_params.y;
        let md = abs(source_x - mp);
        let mlw = px_in_source * 2.5;
        if (md < mlw) {
            let alpha = 1.0 - md / mlw;
            color = mix(color, vec3<f32>(1.0), 0.9 * alpha);
        }
    }

    // -----------------------------------------------------------------
    // Playhead (linear AA)
    // -----------------------------------------------------------------
    var ph_x: f32;
    if (is_overview) {
        ph_x = u.view_params.x;
        let bpm_scale = u.window_params.w;
        if (bpm_scale > 0.01) {
            ph_x = u.view_params.x * bpm_scale;
        }
    } else {
        ph_x = 0.5;
    }

    let ph_dist = abs(uv.x - ph_x);
    let ph_lw = 1.5 / width;
    if (ph_dist < ph_lw) {
        let pa = 1.0 - ph_dist / ph_lw;
        color = mix(color, vec3<f32>(1.0), pa);
    }

    // -----------------------------------------------------------------
    // Volume dimming
    // -----------------------------------------------------------------
    let volume = u.beat_params.w;
    if (!is_overview && volume < 0.999) {
        let dim = (1.0 - volume) * 0.4;
        color = color * (1.0 - dim);
    }

    // Loading pulse (pulsing brightness while audio loads)
    if (loading_pulse > 0.001) {
        color = mix(color, vec3<f32>(1.0), loading_pulse * 0.07);
    }

    return vec4<f32>(color, 1.0);
}
