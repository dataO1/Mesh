// Live-mode waveline over a note roll, with local VFX, two passes per lane.
//
// The view is centred on the sample playing now: the waveline fills the left
// half (the signal just played), the roll (the bar's notes or drum hits,
// looping) the right half (what is about to play). Columns and roll bits arrive precomputed (no loops: Mali).
//
// fs_feedback renders into the lane's own texture: roll, waveline,
// bitcrush (columns held, levels stepped, pixels grown), tears, the kick
// shake, the swap transition, plus the previous frame zoomed and faded
// (feedback). fs_composite draws that texture into the frame with an RGB
// split, grain and scanlines, only where something is drawn (premultiplied
// alpha over the background).
//
// Each effect changes character as its knob rises, so low and high read
// differently: the trail goes from a short ghost to a zooming tunnel, the
// crush from stepped levels to big pixel blocks, the rumble from jitter to
// torn bands, the split from a thin fringe to channels thrown apart in two
// directions.

struct Uniforms {
    // Lane bounds [x, y, width, height] in physical pixels.
    bounds: vec4<f32>,
    // Lane tint, alpha = lane level (muted lanes are dim).
    tint: vec4<f32>,
    // [column count, time s, envelope 0..1, shake px]
    a: vec4<f32>,
    // Effective FX 0..1: [weight, rgb split, trail, rumble].
    fx: vec4<f32>,
    // [crush, roll rows (0 = none), muted row bits, swap 0..1]
    g: vec4<f32>,
    // Roll columns: [held, onset] row bits, two columns per vec4.
    roll: array<vec4<u32>, 256>,
    // Waveline columns: (min, max), two per vec4.
    cols: array<vec4<f32>, 512>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

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

fn hash(x: f32) -> f32 {
    return fract(sin(x * 127.1 + 311.7) * 43758.5453);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn col(i: i32) -> vec2<f32> {
    let n = i32(u.a.x);
    let c = clamp(i, 0, n - 1);
    let v = u.cols[c / 2];
    return select(v.zw, v.xy, c % 2 == 0);
}

// Waveline brightness at pixel p (lane-local physical px).
fn trace(p: vec2<f32>) -> f32 {
    let w = u.bounds.z;
    let h = u.bounds.w;
    if (w < 4.0 || u.a.x < 2.0) {
        return 0.0;
    }
    let crush = u.g.x;
    let f = clamp(p.x / w, 0.0, 1.0);
    let mid = h * 0.5;
    // Playhead: now.
    let ph = clamp(1.0 - abs(p.x - w * 0.5), 0.0, 1.0) * (0.3 + 0.4 * u.a.z);
    if (f > 0.5) {
        return ph + clamp(1.0 - abs(p.y - mid), 0.0, 1.0) * 0.06;
    }
    // Bitcrush: hold columns (sample rate) and step the level (bit depth).
    let hold = 1.0 + crush * 18.0;
    let fx = floor(f * (u.a.x - 1.0) / hold) * hold;
    let i = i32(floor(fx));
    var c = mix(col(i), col(i + 1), select(fx - f32(i), 0.0, crush > 0.02));
    let levels = mix(48.0, 3.0, crush);
    if (crush > 0.02) {
        c = vec2<f32>(floor(c.x * levels) / levels, ceil(c.y * levels) / levels);
    }
    let amp = h * 0.45 * (0.85 + 0.3 * u.fx.x);
    let lo = mid - c.y * amp;
    let hi = mid - c.x * amp;
    let d = max(max(lo - p.y, p.y - hi), 0.0);
    let hw = 0.7 + 1.1 * u.fx.x;
    let line = clamp(1.0 - (d - hw) / 1.2, 0.0, 1.0);
    let glow = exp(-d / (3.0 + 8.0 * u.fx.x)) * (0.1 + 0.35 * u.fx.x);
    let centre = clamp(1.0 - abs(p.y - mid), 0.0, 1.0) * 0.12;
    // The past fades to the left.
    let age = mix(0.45, 1.0, f * 2.0);
    return (max(line, glow) + centre) * age + ph;
}

// Note roll brightness at p.
fn roll(p: vec2<f32>) -> f32 {
    let rows = u.g.y;
    if (rows < 1.0) {
        return 0.0;
    }
    let w = u.bounds.z;
    let h = u.bounds.w;
    let f = clamp(p.x / w, 0.0, 0.9999);
    // Only what is about to play: the right half.
    if (f < 0.5) {
        return 0.0;
    }
    let c = i32(floor(f * 512.0));
    let v = u.roll[c / 2];
    let held = select(v.z, v.x, c % 2 == 0);
    let onset = select(v.w, v.y, c % 2 == 0);
    let ry = (h - p.y) / h * rows;
    let r = i32(floor(ry));
    if (r < 0 || f32(r) >= rows) {
        return 0.0;
    }
    let bit = 1u << u32(r);
    if ((held & bit) == 0u) {
        return 0.0;
    }
    let thick = select(0.3, 0.38, rows <= 8.0);
    if (abs(fract(ry) - 0.5) > thick) {
        return 0.0;
    }
    var b = 0.2;
    if ((onset & bit) != 0u) {
        b = b * 2.2;
    }
    // Sounding now.
    if (abs(f - 0.5) < 0.004) {
        b = b + 0.35;
    }
    let muted = r < 8 && ((u32(u.g.z) >> u32(r)) & 1u) == 1u;
    return select(b, b * 0.3, muted);
}

@fragment
fn fs_feedback(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = u.bounds.zw;
    let t = u.a.y;
    let trail = u.fx.z;
    let ru = u.fx.w;
    let crush = u.g.x;

    let sw = u.g.w;
    let tick = floor(t * 24.0);

    // Last frame, zoomed out from the centre and drifting up: feedback. Low:
    // a short ghost; high: a long, zooming tunnel. A swap wipes it.
    let c = vec2<f32>(0.5, 0.5);
    let z = 1.0 - (0.004 + 0.03 * trail * trail);
    let puv = c + (in.uv - c) * z + vec2<f32>(0.0006, -0.005) * trail;
    let prev = textureSample(tex, samp, puv).r * trail * (0.75 + 0.2 * trail) * (1.0 - sw);

    // Bitcrush pixelates the lane, in blocks that grow late.
    let px = 1.0 + floor(crush * crush * 9.0);
    var p = floor(in.uv * size / px) * px + px * 0.5;

    // Rumble. Low: rows jitter; high: bands of rows tear sideways.
    var dx = ru * (hash(floor(p.y / 3.0) * 0.37 + tick) * 2.0 - 1.0) * 2.5;
    let band = floor(p.y / 14.0);
    let tear = floor(t * 9.0);
    if (hash(band * 13.7 + tear * 3.1) > 1.0 - 0.14 * ru * ru) {
        dx += ru * ru * 90.0 * (hash(band + tear * 1.3) * 2.0 - 1.0);
    }
    // Swap: wide bands thrown sideways.
    let sband = floor(p.y / 9.0);
    if (hash(sband * 7.1 + tick * 0.37) < sw * 0.8) {
        dx += sw * 220.0 * (hash(sband * 3.3 + tick) - 0.5);
    }
    // Kick shake: a small vertical jolt.
    let dy = u.a.w * (hash(floor(t * 40.0)) * 2.0 - 1.0);
    p = p + vec2<f32>(dx, dy);

    var v = trace(p) + roll(p);
    // Kick: the playhead blurs and breaks up into particles.
    let kick = clamp(u.a.w / 6.0, 0.0, 1.0);
    if (kick > 0.02) {
        let cx = size.x * 0.5;
        let dist = abs(p.x - cx);
        let reach = 6.0 + 90.0 * kick;
        if (dist < reach) {
            // Horizontal smear of the trace around now.
            let r = 3.0 + 18.0 * kick;
            let smear = 0.25 * (trace(p + vec2<f32>(r, 0.0)) + trace(p - vec2<f32>(r, 0.0))
                + trace(p + vec2<f32>(2.0 * r, 0.0)) + trace(p - vec2<f32>(2.0 * r, 0.0)));
            v = mix(v, max(v, smear), kick * (1.0 - dist / reach));
        }
        // Particles: grains scattered from the playhead, thinning outwards.
        let cell = floor(p / 3.0);
        let fall = exp(-dist / (8.0 + 70.0 * kick));
        if (hash2(cell + vec2<f32>(tick * 1.7, tick * 0.3)) > 1.0 - 0.35 * kick * fall) {
            v = v + 0.9 * kick * hash2(cell * 1.3 + vec2<f32>(tick, 5.0));
        }
    }
    // Swap: flash, then dissolve in blocks back to the new sound.
    v = v * (1.0 + sw * 1.5 * hash(tick + 0.5));
    if (hash2(floor(in.uv * size / 4.0) + vec2<f32>(tick, 0.0)) < sw * 0.55) {
        v = 0.0;
    }
    return vec4<f32>(max(prev, v), 0.0, 0.0, 1.0);
}

@fragment
fn fs_composite(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = u.bounds.zw;
    let p = in.uv * size;
    let t = u.a.y;

    // RGB split: red left, green in place, blue right. Low: a thin fringe;
    // high: the channels thrown apart, sideways and up / down. A swap
    // splits fully.
    let s = max(u.fx.y, u.g.w * 0.9);
    let ab = (2.0 + 22.0 * s) * s / size.x;
    let vy = max(s - 0.45, 0.0) * 10.0 / size.y;
    let vr = textureSample(tex, samp, in.uv + vec2<f32>(-ab, vy)).r;
    let vg = textureSample(tex, samp, in.uv).r;
    let vb = textureSample(tex, samp, in.uv + vec2<f32>(ab, -vy)).r;

    // Grain only near the trace, from the rumble.
    let r = vec2<f32>(10.0, 6.0) / size;
    let near = 0.25 * (textureSample(tex, samp, in.uv + vec2<f32>(r.x, r.y)).r
        + textureSample(tex, samp, in.uv + vec2<f32>(-r.x, r.y)).r
        + textureSample(tex, samp, in.uv + vec2<f32>(r.x, -r.y)).r
        + textureSample(tex, samp, in.uv + vec2<f32>(-r.x, -r.y)).r);
    let grain = (hash2(floor(p) + vec2<f32>(floor(t * 30.0) * 17.0, 0.0)) - 0.5) * near * 0.6 * u.fx.w;

    // Ink in the lane tint; the split pulls the channels apart in pure
    // colour (red, green, blue), so fringes read as colour, not grey.
    let tint = u.tint.rgb;
    let ink = tint * (vg + grain);
    let split = vec3<f32>(vr * 1.3, vg * 0.9, vb * 1.4);
    var rgb = mix(ink, split, s);
    var a = clamp(max(vg, max(vr, vb) * s) + grain, 0.0, 1.0);

    // Scanlines inside the lane.
    let scan = select(1.0, 0.8, fract(p.y / 3.0) < 0.34);
    let level = u.tint.a;
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.5)) * scan * level;
    a = a * scan * level;
    return vec4<f32>(min(rgb, vec3<f32>(a)), a);
}
