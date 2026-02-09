// Knob shader for audio applications
// Renders circular knob with value arc and modulation range indicators

// All data via uniform buffer (updated per-primitive via dynamic offset)
struct Uniforms {
    // Widget bounds: [x, y, width, height] in pixels
    bounds: vec4<f32>,
    // value, dragging, bipolar, mod_count
    params: vec4<f32>,
    // display_value (for indicator dot), unused, unused, unused
    params2: vec4<f32>,
    // Modulation ranges: [min0, max0, min1, max1]
    mod_ranges_01: vec4<f32>,
    // Modulation ranges: [min2, max2, min3, max3]
    mod_ranges_23: vec4<f32>,

    // Modulation colors (RGBA)
    mod_color_0: vec4<f32>,
    mod_color_1: vec4<f32>,
    mod_color_2: vec4<f32>,
    mod_color_3: vec4<f32>,

    // Base colors
    bg_color: vec4<f32>,
    track_color: vec4<f32>,
    value_color: vec4<f32>,
    notch_color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Fullscreen triangle vertex shader
// Uses oversized triangle technique: vertices extend beyond clip space [-1,1]
// so GPU clipping produces a full rectangle covering the viewport
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // Generate oversized triangle that covers entire clip space when clipped:
    // vertex 0: (-1, -1) bottom-left
    // vertex 1: ( 3, -1) far right (clipped to 1)
    // vertex 2: (-1,  3) far top (clipped to 1)
    let x = select(-1.0, 3.0, vertex_index == 1u);
    let y = select(-1.0, 3.0, vertex_index == 2u);

    out.position = vec4<f32>(x, y, 0.0, 1.0);
    // UV maps clip space to 0-1 range (with Y flipped for screen coordinates)
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);

    return out;
}

// Constants
const PI: f32 = 3.14159265359;
const TWO_PI: f32 = 6.28318530718;

// Arc start/end angles (knob range: 135° to 405° = 270° sweep)
const ARC_START: f32 = 0.75 * PI;  // 135 degrees (bottom-left)
const ARC_END: f32 = 2.25 * PI;    // 405 degrees (bottom-right)
const ARC_RANGE: f32 = 1.5 * PI;   // 270 degrees total sweep

// Signed distance to a circular arc
// Handles angles that exceed 2PI (e.g., for arcs crossing the 0 degree line)
fn sd_arc(p: vec2<f32>, radius: f32, width: f32, start_angle: f32, end_angle: f32) -> f32 {
    let angle = atan2(p.y, p.x);

    // Normalize angle to 0..2PI
    var a = angle;
    if a < 0.0 {
        a += TWO_PI;
    }

    // Normalize start and end angles to 0..2PI range for comparison
    var s = start_angle;
    var e = end_angle;

    // Wrap angles to 0..2PI
    while s >= TWO_PI { s -= TWO_PI; }
    while s < 0.0 { s += TWO_PI; }
    while e >= TWO_PI { e -= TWO_PI; }
    while e < 0.0 { e += TWO_PI; }

    // Handle wrap-around for arcs that cross 0
    var in_arc = false;
    if s <= e {
        // Normal case: arc doesn't cross 0
        in_arc = a >= s && a <= e;
    } else {
        // Arc crosses 0: angle is in arc if >= start OR <= end
        in_arc = a >= s || a <= e;
    }

    // Distance from circle
    let dist_from_circle = abs(length(p) - radius);

    if in_arc {
        return dist_from_circle - width * 0.5;
    } else {
        // Distance to arc endpoints (use original angles for positions)
        let p1 = vec2<f32>(cos(start_angle), sin(start_angle)) * radius;
        let p2 = vec2<f32>(cos(end_angle), sin(end_angle)) * radius;
        let d1 = length(p - p1);
        let d2 = length(p - p2);
        return min(d1, d2) - width * 0.5;
    }
}

// Convert value (0-1) to angle
fn value_to_angle(v: f32) -> f32 {
    return ARC_START + v * ARC_RANGE;
}

// Anti-aliased step
fn aa_step(d: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(-aa, aa, d);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Use UV from vertex shader (0-1 across the widget viewport)
    // iced sets the viewport to widget bounds, so in.uv maps correctly
    let value = uniforms.params.x;
    let is_dragging = uniforms.params.y > 0.5;
    let is_bipolar = uniforms.params.z > 0.5;
    let mod_count = i32(uniforms.params.w);
    let bounds_w = uniforms.bounds.z;
    let bounds_h = uniforms.bounds.w;

    // Convert UV (0-1) to centered coordinates (-1 to 1)
    let uv = (in.uv - 0.5) * 2.0;
    let dist = length(uv);

    // Anti-aliasing factor based on widget size
    let aa = 2.0 / min(bounds_w, bounds_h);

    // Knob dimensions (in -1 to 1 coordinate space)
    let outer_radius = 0.85;
    let arc_width = 0.15;
    let track_radius = outer_radius - arc_width * 0.5;
    let notch_radius = 0.65;
    let notch_width = 0.08;
    let inner_radius = 0.55;

    // Start with transparent
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    // Draw background circle (inner fill)
    let bg_dist = dist - inner_radius;
    let bg_alpha = aa_step(bg_dist, aa);
    color = mix(color, uniforms.bg_color, bg_alpha * uniforms.bg_color.a);

    // Draw track arc (unfilled portion - full arc)
    let track_dist = sd_arc(uv, track_radius, arc_width, ARC_START, ARC_END);
    let track_alpha = aa_step(track_dist, aa);
    color = mix(color, uniforms.track_color, track_alpha * uniforms.track_color.a);

    // Draw value arc
    if value > 0.001 {
        var value_start: f32;
        var value_end: f32;

        if is_bipolar {
            // Bipolar: arc extends from center (0.5) in both directions
            let center_angle = value_to_angle(0.5);
            let value_angle = value_to_angle(value);
            if value > 0.5 {
                value_start = center_angle;
                value_end = value_angle;
            } else {
                value_start = value_angle;
                value_end = center_angle;
            }
        } else {
            // Unipolar: arc extends from start to value
            value_start = ARC_START;
            value_end = value_to_angle(value);
        }

        let value_dist = sd_arc(uv, track_radius, arc_width, value_start, value_end);
        let value_alpha = aa_step(value_dist, aa);
        color = mix(color, uniforms.value_color, value_alpha * uniforms.value_color.a);
    }

    // Draw modulation ranges (outer indicators)
    let mod_radius = outer_radius + 0.06;
    let mod_width = 0.10;

    // Modulation 0
    if mod_count > 0 {
        let m0_min = uniforms.mod_ranges_01.x;
        let m0_max = uniforms.mod_ranges_01.y;
        if m0_max > m0_min {
            let m0_start = value_to_angle(m0_min);
            let m0_end = value_to_angle(m0_max);
            let m0_dist = sd_arc(uv, mod_radius, mod_width, m0_start, m0_end);
            let m0_alpha = aa_step(m0_dist, aa);
            color = mix(color, uniforms.mod_color_0, m0_alpha * uniforms.mod_color_0.a);
        }
    }

    // Modulation 1
    if mod_count > 1 {
        let m1_min = uniforms.mod_ranges_01.z;
        let m1_max = uniforms.mod_ranges_01.w;
        if m1_max > m1_min {
            let m1_start = value_to_angle(m1_min);
            let m1_end = value_to_angle(m1_max);
            let m1_dist = sd_arc(uv, mod_radius + 0.06, mod_width, m1_start, m1_end);
            let m1_alpha = aa_step(m1_dist, aa);
            color = mix(color, uniforms.mod_color_1, m1_alpha * uniforms.mod_color_1.a);
        }
    }

    // Draw notch/indicator at display value (actual modulated position)
    let display_value = uniforms.params2.x;
    let notch_angle = value_to_angle(display_value);
    let notch_dir = vec2<f32>(cos(notch_angle), sin(notch_angle));
    let notch_center = notch_dir * notch_radius;
    let notch_dist = length(uv - notch_center) - notch_width;
    let notch_alpha = aa_step(notch_dist, aa);

    // Make notch brighter when dragging
    var notch_color = uniforms.notch_color;
    if is_dragging {
        notch_color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    }
    color = mix(color, notch_color, notch_alpha * notch_color.a);

    return color;
}
