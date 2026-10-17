// Live-mode screen overlay: faint scanlines and a soft vignette over the
// whole screen. The strong effects live in each lane's waveline (wave.wgsl).

struct Uniforms {
    // Bounds [x, y, width, height] in physical pixels.
    bounds: vec4<f32>,
    // [time s, scanline strength, vignette strength, 0]
    a: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.uv * u.bounds.zw;
    let scan = select(0.0, u.a.y, fract(p.y / 3.0) < 0.34);
    let d = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.25, 1.05);
    let vig = smoothstep(0.4, 0.8, length(d)) * u.a.z;
    // Premultiplied black at this alpha darkens what is below.
    return vec4<f32>(0.0, 0.0, 0.0, clamp(scan + vig, 0.0, 0.9));
}
