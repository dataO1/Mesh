// Oscilloscope trace: one decimated signal drawn as an anti-aliased line with
// a faint fill to the centre line. One fullscreen triangle per scope; the
// viewport is the widget's bounds (see `ScopePrimitive::draw`).

struct Uniforms {
    // Widget bounds [x, y, width, height] in physical pixels
    bounds: vec4<f32>,
    // Trace colour (alpha scales everything)
    color: vec4<f32>,
    // [point count, gain, line thickness px, fill alpha]
    params: vec4<f32>,
    // Up to 512 points, oldest first, packed 4 per vec4
    points: array<vec4<f32>, 128>,
}

@group(0) @binding(0)
var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = select(-1.0, 3.0, vertex_index == 1u);
    let y = select(-1.0, 3.0, vertex_index == 2u);
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, (1.0 - y) * 0.5);
    return out;
}

fn point(i: i32) -> f32 {
    let c = clamp(i, 0, i32(u.params.x) - 1);
    return u.points[c / 4][c % 4] * u.params.y;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = i32(u.params.x);
    if (n < 2) {
        return vec4<f32>(0.0);
    }
    let h = u.bounds.w;
    let py = in.uv.y * h;
    let mid = h * 0.5;
    let amp = h * 0.46;

    let fx = clamp(in.uv.x, 0.0, 1.0) * f32(n - 1);
    let i = i32(floor(fx));
    let a = clamp(point(i), -1.0, 1.0);
    let b = clamp(point(i + 1), -1.0, 1.0);
    let v = mix(a, b, fx - f32(i));

    let y_a = mid - a * amp;
    let y_b = mid - b * amp;
    let y_v = mid - v * amp;

    // Distance to the vertical span between neighbouring points, so steep
    // transients stay connected.
    let lo = min(y_a, y_b);
    let hi = max(y_a, y_b);
    let d = max(max(lo - py, py - hi), 0.0);
    let half_w = u.params.z * 0.5;
    let line = 1.0 - smoothstep(half_w - 0.75, half_w + 0.75, d);

    let in_fill = py >= min(mid, y_v) && py <= max(mid, y_v);
    let fill = select(0.0, u.params.w, in_fill);
    let centre = (1.0 - smoothstep(0.0, 1.0, abs(py - mid))) * 0.15;

    let alpha = max(line, max(fill, centre)) * u.color.a;
    return vec4<f32>(u.color.rgb, alpha);
}
