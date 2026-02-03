// Knob shader for audio applications
// Renders circular knob with value arc and modulation range indicators

// All data via uniform buffer (updated per-primitive via dynamic offset)
struct Uniforms {
    // Widget bounds: [x, y, width, height] in pixels
    bounds: vec4<f32>,
    // value, dragging, bipolar, mod_count
    params: vec4<f32>,
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
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;

    // Generate fullscreen triangle
    let x = f32(i32(vertex_index) - 1);
    let y = f32(i32(vertex_index & 1u) * 2 - 1);

    out.position = vec4<f32>(x, y, 0.0, 1.0);
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
fn sd_arc(p: vec2<f32>, radius: f32, width: f32, start_angle: f32, end_angle: f32) -> f32 {
    let angle = atan2(p.y, p.x);

    // Normalize angle to 0..2PI
    var a = angle;
    if a < 0.0 {
        a += TWO_PI;
    }

    // Handle wrap-around for arcs that cross 0
    var in_arc = false;
    if start_angle < end_angle {
        in_arc = a >= start_angle && a <= end_angle;
    } else {
        in_arc = a >= start_angle || a <= end_angle;
    }

    // Distance from circle
    let dist_from_circle = abs(length(p) - radius);

    if in_arc {
        return dist_from_circle - width * 0.5;
    } else {
        // Distance to arc endpoints
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
    // Calculate local UV based on widget bounds from uniforms
    let pixel_pos = in.position.xy;

    // Get bounds from uniforms: x, y, width, height
    let bounds_x = uniforms.bounds.x;
    let bounds_y = uniforms.bounds.y;
    let bounds_w = uniforms.bounds.z;
    let bounds_h = uniforms.bounds.w;

    // Get per-instance parameters from uniforms
    let value = uniforms.params.x;
    let dragging = uniforms.params.y;
    let bipolar = uniforms.params.z;
    let mod_count = i32(uniforms.params.w);

    // Calculate normalized position within widget (0..1)
    let local_uv = vec2<f32>(
        (pixel_pos.x - bounds_x) / bounds_w,
        (pixel_pos.y - bounds_y) / bounds_h
    );

    // Center UV and scale to -1..1
    let uv = (local_uv - 0.5) * 2.0;
    let dist = length(uv);

    // Anti-aliasing amount - scale based on widget size for crisp edges
    let aa = 2.0 / min(bounds_w, bounds_h);

    // Layer radii (from outside to inside)
    let outer_radius = 0.92;
    let track_radius = 0.78;
    let track_width = 0.14;      // Bolder track arc
    let value_radius = 0.78;
    let value_width = 0.14;      // Bolder value arc
    let mod_radius = 0.92;
    let mod_width = 0.10;        // Bolder modulation indicators
    let inner_radius = 0.62;
    let notch_length = 0.18;
    let notch_width = 0.06;      // Bolder notch indicator

    // Start with background/transparent
    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    // Draw outer ring (knob body)
    let body_dist = abs(dist - outer_radius) - 0.03;
    let body_alpha = aa_step(body_dist, aa);
    let body_color = uniforms.bg_color;
    color = mix(color, body_color, body_alpha * body_color.a);

    // Draw inner fill (knob face)
    let inner_dist = dist - inner_radius;
    let inner_alpha = aa_step(inner_dist, aa);
    let face_color = vec4<f32>(uniforms.bg_color.rgb * 0.7, uniforms.bg_color.a);
    color = mix(color, face_color, inner_alpha * face_color.a);

    // Draw track (background arc)
    let track_dist = sd_arc(uv, track_radius, track_width, ARC_START, ARC_END);
    let track_alpha = aa_step(track_dist, aa);
    color = mix(color, uniforms.track_color, track_alpha * uniforms.track_color.a);

    // Draw modulation ranges (behind value arc) - ranges from uniforms
    if mod_count > 0 {
        let mod_start = value_to_angle(uniforms.mod_ranges_01.x);
        let mod_end = value_to_angle(uniforms.mod_ranges_01.y);
        let mod_dist = sd_arc(uv, mod_radius, mod_width, min(mod_start, mod_end), max(mod_start, mod_end));
        let mod_alpha = aa_step(mod_dist, aa);
        color = mix(color, uniforms.mod_color_0, mod_alpha * uniforms.mod_color_0.a * 0.6);
    }

    if mod_count > 1 {
        let mod_start = value_to_angle(uniforms.mod_ranges_01.z);
        let mod_end = value_to_angle(uniforms.mod_ranges_01.w);
        let mod_dist = sd_arc(uv, mod_radius - 0.12, mod_width, min(mod_start, mod_end), max(mod_start, mod_end));
        let mod_alpha = aa_step(mod_dist, aa);
        color = mix(color, uniforms.mod_color_1, mod_alpha * uniforms.mod_color_1.a * 0.6);
    }

    if mod_count > 2 {
        let mod_start = value_to_angle(uniforms.mod_ranges_23.x);
        let mod_end = value_to_angle(uniforms.mod_ranges_23.y);
        let mod_dist = sd_arc(uv, mod_radius - 0.24, mod_width, min(mod_start, mod_end), max(mod_start, mod_end));
        let mod_alpha = aa_step(mod_dist, aa);
        color = mix(color, uniforms.mod_color_2, mod_alpha * uniforms.mod_color_2.a * 0.6);
    }

    if mod_count > 3 {
        let mod_start = value_to_angle(uniforms.mod_ranges_23.z);
        let mod_end = value_to_angle(uniforms.mod_ranges_23.w);
        let mod_dist = sd_arc(uv, mod_radius - 0.36, mod_width, min(mod_start, mod_end), max(mod_start, mod_end));
        let mod_alpha = aa_step(mod_dist, aa);
        color = mix(color, uniforms.mod_color_3, mod_alpha * uniforms.mod_color_3.a * 0.6);
    }

    // Draw value arc - value from push constants
    let value_angle = value_to_angle(value);
    var value_start = ARC_START;
    var value_end = value_angle;

    // For bipolar mode, draw from center
    if bipolar > 0.5 {
        let center_angle = value_to_angle(0.5);
        if value >= 0.5 {
            value_start = center_angle;
            value_end = value_angle;
        } else {
            value_start = value_angle;
            value_end = center_angle;
        }
    }

    if value_start < value_end {
        let value_dist = sd_arc(uv, value_radius, value_width, value_start, value_end);
        let value_alpha = aa_step(value_dist, aa);
        var val_color = uniforms.value_color;

        // Brighten when dragging
        if dragging > 0.5 {
            val_color = vec4<f32>(min(val_color.rgb * 1.3, vec3<f32>(1.0)), val_color.a);
        }

        color = mix(color, val_color, value_alpha * val_color.a);
    }

    // Draw position notch/indicator
    let notch_angle = value_angle;
    let notch_dir = vec2<f32>(cos(notch_angle), sin(notch_angle));
    let notch_center = notch_dir * (inner_radius + notch_length * 0.5);

    // Project point onto notch line
    let to_point = uv - notch_dir * inner_radius;
    let along = dot(to_point, notch_dir);
    let perp = length(to_point - notch_dir * along);

    if along > 0.0 && along < notch_length {
        let notch_dist = perp - notch_width * 0.5;
        let notch_alpha = aa_step(notch_dist, aa);
        var notch_col = uniforms.notch_color;
        if dragging > 0.5 {
            notch_col = vec4<f32>(1.0, 1.0, 1.0, 1.0);
        }
        color = mix(color, notch_col, notch_alpha * notch_col.a);
    }

    // Clip to circle
    let clip_dist = dist - outer_radius - 0.02;
    let clip_alpha = aa_step(clip_dist, aa);
    color.a *= 1.0 - clip_alpha;

    return color;
}
