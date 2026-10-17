//! Live-mode screen visuals: a scrolling waveline per lane with local VFX
//! that its knobs and its own audio drive, over a scrolling note roll, and
//! a faint screen overlay.
//!
//! Used only by the live mode (`mesh-player` `live::screen`); the DJ
//! widgets are separate.
//!
//! # Waveline ([`live_wave`])
//!
//! The view is centred on the sample playing now and spans a fixed number
//! of beats, so every lane scrolls with the tempo. The left half is the
//! signal just played, the right half the lane's note roll (the bar's
//! notes, or the drum hits per voice): what is about to play.
//!
//! Two passes per lane. The feedback pass renders into the lane's own
//! texture: the roll, the waveline, bitcrush, tears and shake, plus the
//! previous frame zoomed and faded. It runs from `prepare` in its own small
//! command buffer, so the frame's render pass is never split (Mali is a
//! tiler: an extra pass on the frame would store and reload every tile).
//! The composite pass draws that texture inside the frame's pass with an
//! RGB split, grain and scanlines, only where the trace is. Each lane keeps
//! two textures (ping-pong) under its `id`.
//!
//! The signal arrives as one (min, max) pair per column ([`columns`]) and
//! the roll as row bits per column ([`roll`]), both computed on the CPU, so
//! the shader does no loops (Mali, as the DJ waveform).
//!
//! # Overlay ([`live_screen`])
//!
//! Faint scanlines and a soft vignette over the whole screen, one fullscreen
//! triangle.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use iced::widget::shader;
use iced::{mouse, Color, Element, Length, Rectangle};

/// Column pairs the shader holds.
pub const WAVE_COLUMNS: usize = 1024;
/// Roll columns and rows.
pub const ROLL_COLUMNS: usize = 512;
pub const ROLL_ROWS: usize = 32;

/// Seconds since the first frame (shared animation clock).
fn now_secs() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// One (min, max) pair per column, centred on now. `points` are oldest
/// first (the last one is now); the view covers `span` points, half before
/// now (the right half, still to come, stays silent). Values are scaled by
/// `gain` and clamped to `-1..=1`.
pub fn columns(points: &[f32], span: f32, gain: f32, out: &mut [[f32; 2]; WAVE_COLUMNS]) {
    let n = points.len() as i64;
    let per = (span / WAVE_COLUMNS as f32).max(1e-3);
    let now = n - 1;
    for (c, o) in out.iter_mut().enumerate() {
        // Offsets from now, in points, of this column's edges.
        let a = (c as f32 - WAVE_COLUMNS as f32 * 0.5) * per;
        let b = a + per;
        if a >= 0.0 {
            *o = [0.0, 0.0];
            continue;
        }
        // Points in (now + a, now + b].
        let first = now + a.floor() as i64 + 1;
        let last = (now + b.ceil() as i64).max(first);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for i in first..=last {
            let v = if (0..n).contains(&i) { points[i as usize] } else { 0.0 };
            let v = if v.is_finite() { (v * gain).clamp(-1.0, 1.0) } else { 0.0 };
            lo = lo.min(v);
            hi = hi.max(v);
        }
        *o = [lo, hi];
    }
}

/// One note of a looping roll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollNote {
    /// Start and length in ticks within the loop.
    pub start: f32,
    pub len: f32,
    /// Row, 0 = bottom, below [`ROLL_ROWS`].
    pub row: u8,
}

/// Row bits per column: `[held, onset]`. The view spans `span` ticks
/// centred on `now` (a tick within the loop of `loop_len` ticks); the notes
/// repeat every loop.
pub fn roll(notes: &[RollNote], loop_len: f32, now: f64, span: f32, out: &mut [[u32; 2]; ROLL_COLUMNS]) {
    let per = span / ROLL_COLUMNS as f32;
    let l = loop_len.max(1.0) as f64;
    for (c, o) in out.iter_mut().enumerate() {
        let a = now + ((c as f32 - ROLL_COLUMNS as f32 * 0.5) * per) as f64;
        let (mut held, mut onset) = (0u32, 0u32);
        for n in notes.iter().filter(|n| (n.row as usize) < ROLL_ROWS) {
            let bit = 1u32 << n.row;
            // Column start relative to the note start, within one loop.
            let rel = (a - n.start as f64).rem_euclid(l);
            let len = n.len.max(per) as f64;
            if rel < len || rel + per as f64 > l {
                held |= bit;
            }
            if (n.start as f64 - a).rem_euclid(l) < per as f64 {
                onset |= bit;
            }
        }
        *o = [held, onset];
    }
}

/// Effect amounts of a lane, 0..1 each (knob value times the envelope).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WaveFx {
    /// Line weight and glow.
    pub weight: f32,
    /// RGB split (chromatic aberration).
    pub split: f32,
    /// Bitcrush: held columns, stepped levels, pixelated lane.
    pub crush: f32,
    /// Feedback trail.
    pub trail: f32,
    /// Tears and jitter.
    pub rumble: f32,
}

/// Display settings of one lane's waveline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaveStyle {
    pub tint: Color,
    /// Lane level 0..1 (muted lanes dim).
    pub level: f32,
    /// Envelope 0..1 (reactive amounts already include it).
    pub envelope: f32,
    pub fx: WaveFx,
    /// Roll rows in use (0: no roll).
    pub rows: u8,
    /// Dimmed roll rows (muted drum voices), bit per row, first 8 rows.
    pub muted_rows: u8,
    /// Vertical shake in logical pixels (the kick).
    pub shake: f32,
    /// Swap transition 0..1 (seed or instrument changed): the lane tears,
    /// flashes and dissolves back in.
    pub swap: f32,
}

/// A lane waveline over its roll; `id` must be stable per lane.
pub fn live_wave<'a, Message: 'a>(
    id: u64,
    columns: Arc<[[f32; 2]; WAVE_COLUMNS]>,
    roll: Arc<[[u32; 2]; ROLL_COLUMNS]>,
    style: WaveStyle,
    width: Length,
    height: Length,
) -> Element<'a, Message> {
    shader(WaveProgram { id, columns, roll, style }).width(width).height(height).into()
}

struct WaveProgram {
    id: u64,
    columns: Arc<[[f32; 2]; WAVE_COLUMNS]>,
    roll: Arc<[[u32; 2]; ROLL_COLUMNS]>,
    style: WaveStyle,
}

impl<Message> shader::Program<Message> for WaveProgram {
    type State = ();
    type Primitive = WavePrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> WavePrimitive {
        WavePrimitive {
            id: self.id,
            columns: Arc::clone(&self.columns),
            roll: Arc::clone(&self.roll),
            style: self.style,
            time: now_secs(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct WaveUniforms {
    bounds: [f32; 4],
    tint: [f32; 4],
    a: [f32; 4],
    fx: [f32; 4],
    g: [f32; 4],
    roll: [[u32; 4]; ROLL_COLUMNS / 2],
    cols: [[f32; 4]; WAVE_COLUMNS / 2],
}

#[derive(Debug)]
pub struct WavePrimitive {
    id: u64,
    columns: Arc<[[f32; 2]; WAVE_COLUMNS]>,
    roll: Arc<[[u32; 2]; ROLL_COLUMNS]>,
    style: WaveStyle,
    time: f32,
}

impl WavePrimitive {
    fn uniforms(&self, bounds: &Rectangle, scale: f32) -> WaveUniforms {
        let mut cols = [[0.0f32; 4]; WAVE_COLUMNS / 2];
        for (i, c) in self.columns.iter().enumerate() {
            cols[i / 2][(i % 2) * 2] = c[0];
            cols[i / 2][(i % 2) * 2 + 1] = c[1];
        }
        let mut roll = [[0u32; 4]; ROLL_COLUMNS / 2];
        for (i, c) in self.roll.iter().enumerate() {
            roll[i / 2][(i % 2) * 2] = c[0];
            roll[i / 2][(i % 2) * 2 + 1] = c[1];
        }
        let s = &self.style;
        let fx = s.fx;
        WaveUniforms {
            bounds: [bounds.x * scale, bounds.y * scale, bounds.width * scale, bounds.height * scale],
            tint: [s.tint.r, s.tint.g, s.tint.b, s.level.clamp(0.0, 1.0)],
            a: [WAVE_COLUMNS as f32, self.time, s.envelope.clamp(0.0, 1.0), s.shake * scale],
            fx: [fx.weight, fx.split, fx.trail, fx.rumble].map(|v| v.clamp(0.0, 1.0)),
            g: [fx.crush.clamp(0.0, 1.0), s.rows.min(ROLL_ROWS as u8) as f32, s.muted_rows as f32, s.swap.clamp(0.0, 1.0)],
            roll,
            cols,
        }
    }
}

const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct WaveResources {
    buffer: wgpu::Buffer,
    /// Ping-pong feedback textures and their views.
    views: [wgpu::TextureView; 2],
    /// Feedback pass: writes `views[k]`, reads `views[1 - k]`.
    bind_feedback: [wgpu::BindGroup; 2],
    /// Composite pass: reads `views[k]`.
    bind_composite: [wgpu::BindGroup; 2],
    size: (u32, u32),
    /// Texture written this frame.
    k: usize,
}

pub struct WavePipeline {
    feedback: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    resources: HashMap<u64, WaveResources>,
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    entry: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

impl shader::Pipeline for WavePipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Live Wave Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("wave.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Live Wave Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Live Wave Pipeline Layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let feedback = fullscreen_pipeline(
            device,
            "Live Wave Feedback",
            &module,
            &pipeline_layout,
            "fs_feedback",
            FEEDBACK_FORMAT,
            None,
        );
        let composite = fullscreen_pipeline(
            device,
            "Live Wave Composite",
            &module,
            &pipeline_layout,
            "fs_composite",
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Live Wave Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { feedback, composite, layout, sampler, resources: HashMap::new() }
    }
}

impl WavePipeline {
    fn create_resources(&self, device: &wgpu::Device, size: (u32, u32)) -> WaveResources {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Live Wave Uniforms"),
            size: std::mem::size_of::<WaveUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views: [wgpu::TextureView; 2] = std::array::from_fn(|_| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Live Wave Feedback Texture"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FEEDBACK_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let bind = |view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Live Wave Bind Group"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            })
        };
        let bind_feedback = [bind(&views[1]), bind(&views[0])];
        let bind_composite = [bind(&views[0]), bind(&views[1])];
        WaveResources { buffer, views, bind_feedback, bind_composite, size, k: 0 }
    }
}

impl shader::Primitive for WavePrimitive {
    type Pipeline = WavePipeline;

    fn prepare(
        &self,
        pipeline: &mut WavePipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        let scale = if scale > 0.0 && scale.is_finite() { scale } else { 1.0 };
        let size = ((bounds.width * scale).round().max(1.0) as u32, (bounds.height * scale).round().max(1.0) as u32);
        let stale = pipeline.resources.get(&self.id).is_none_or(|r| r.size != size);
        if stale {
            let res = pipeline.create_resources(device, size);
            pipeline.resources.insert(self.id, res);
        }
        let WavePipeline { feedback, resources, .. } = pipeline;
        let Some(res) = resources.get_mut(&self.id) else { return };
        res.k ^= 1;
        let uniforms = self.uniforms(bounds, scale);
        queue.write_buffer(&res.buffer, 0, bytemuck::bytes_of(&uniforms));

        // Feedback pass into the lane's texture, submitted ahead of the frame.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Live Wave Feedback") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Live Wave Feedback Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &res.views[res.k],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(feedback);
            pass.set_bind_group(0, &res.bind_feedback[res.k], &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }

    fn draw(&self, pipeline: &WavePipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(res) = pipeline.resources.get(&self.id) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.composite);
        render_pass.set_bind_group(0, &res.bind_composite[res.k], &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

// ── Screen overlay ──

/// Overlay strengths, 0..1 each.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenStyle {
    pub scanlines: f32,
    pub vignette: f32,
}

impl Default for ScreenStyle {
    fn default() -> Self {
        Self { scanlines: 0.08, vignette: 0.3 }
    }
}

/// The screen overlay, to stack over the whole live screen.
pub fn live_screen<'a, Message: 'a>(style: ScreenStyle) -> Element<'a, Message> {
    shader(ScreenProgram { style }).width(Length::Fill).height(Length::Fill).into()
}

struct ScreenProgram {
    style: ScreenStyle,
}

impl<Message> shader::Program<Message> for ScreenProgram {
    type State = ();
    type Primitive = ScreenPrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> ScreenPrimitive {
        ScreenPrimitive { style: self.style, time: now_secs() }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ScreenUniforms {
    bounds: [f32; 4],
    a: [f32; 4],
}

#[derive(Debug)]
pub struct ScreenPrimitive {
    style: ScreenStyle,
    time: f32,
}

pub struct ScreenPipeline {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl shader::Pipeline for ScreenPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Live Screen Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("screen.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Live Screen Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Live Screen Pipeline Layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = fullscreen_pipeline(
            device,
            "Live Screen Pipeline",
            &module,
            &pipeline_layout,
            "fs_main",
            format,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Live Screen Uniforms"),
            size: std::mem::size_of::<ScreenUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Live Screen Bind Group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        Self { pipeline, buffer, bind_group }
    }
}

impl shader::Primitive for ScreenPrimitive {
    type Pipeline = ScreenPipeline;

    fn prepare(
        &self,
        pipeline: &mut ScreenPipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        let scale = if scale > 0.0 && scale.is_finite() { scale } else { 1.0 };
        let s = self.style;
        let u = ScreenUniforms {
            bounds: [bounds.x * scale, bounds.y * scale, bounds.width * scale, bounds.height * scale],
            a: [self.time, s.scanlines, s.vignette, 0.0],
        };
        queue.write_buffer(&pipeline.buffer, 0, bytemuck::bytes_of(&u));
    }

    fn draw(&self, pipeline: &ScreenPipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &pipeline.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaders_validate() {
        use wgpu::naga;
        for (name, src) in [("wave", include_str!("wave.wgsl")), ("screen", include_str!("screen.wgsl"))] {
            let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn columns_centre_on_now_and_leave_the_future_silent() {
        // Point i holds i / 4000; now is the last point.
        let points: Vec<f32> = (0..4000).map(|i| i as f32 / 4000.0).collect();
        let mut out = [[0.0; 2]; WAVE_COLUMNS];
        let span = WAVE_COLUMNS as f32 * 2.0;
        columns(&points, span, 1.0, &mut out);
        let half = WAVE_COLUMNS / 2;
        // Just before now: the newest points.
        assert_eq!(out[half - 1], [3998.0 / 4000.0, 3999.0 / 4000.0]);
        // The left edge: half a span back.
        assert_eq!(out[0][0], (4000.0 - span / 2.0) / 4000.0);
        assert!(out[half..].iter().all(|c| *c == [0.0, 0.0]));
        // Gain clamps, non-finite reads as silence, history before the ring is silence.
        columns(&[f32::NAN, 0.9, 0.9, 0.9], 8.0, 2.0, &mut out);
        assert_eq!(out[half - 1], [1.0, 1.0]);
        assert_eq!(out[0], [0.0, 0.0]);
    }

    #[test]
    fn roll_marks_held_and_onset_rows_and_loops() {
        let mut out = [[0u32; 2]; ROLL_COLUMNS];
        // A note at tick 100 for 50 ticks on row 3, loop 384, view 512 ticks
        // centred on tick 100: one tick per column.
        let notes = [RollNote { start: 100.0, len: 50.0, row: 3 }];
        roll(&notes, 384.0, 100.0, ROLL_COLUMNS as f32, &mut out);
        let half = ROLL_COLUMNS / 2;
        assert_eq!(out[half], [1 << 3, 1 << 3], "starts now");
        assert_eq!(out[half + 49][0], 1 << 3);
        assert_eq!(out[half + 50], [0, 0], "ended");
        assert_eq!(out[half - 1], [0, 0]);
        // A 200-tick loop: the note again a loop ahead and a loop back.
        roll(&notes, 200.0, 100.0, ROLL_COLUMNS as f32, &mut out);
        assert_eq!(out[half + 200], [1 << 3, 1 << 3]);
        assert_eq!(out[half - 200], [1 << 3, 1 << 3]);
        // Rows beyond the roll are ignored.
        roll(&[RollNote { start: 0.0, len: 10.0, row: 40 }], 384.0, 0.0, 512.0, &mut out);
        assert!(out.iter().all(|c| *c == [0, 0]));
    }

    #[test]
    fn uniforms_pack_columns_and_roll() {
        let mut cols = [[0.0; 2]; WAVE_COLUMNS];
        cols[0] = [-0.1, 0.2];
        cols[1] = [-0.3, 0.4];
        let mut r = [[0u32; 2]; ROLL_COLUMNS];
        r[1] = [5, 1];
        let style = WaveStyle {
            tint: Color::WHITE,
            level: 1.0,
            envelope: 0.5,
            fx: WaveFx { weight: 0.1, split: 2.0, crush: 0.5, trail: 0.3, rumble: 0.4 },
            rows: 8,
            muted_rows: 0b1000,
            shake: 1.5,
            swap: 0.25,
        };
        let prim = WavePrimitive { id: 0, columns: Arc::new(cols), roll: Arc::new(r), style, time: 1.0 };
        let u = prim.uniforms(&Rectangle { x: 1.0, y: 2.0, width: 300.0, height: 50.0 }, 2.0);
        assert_eq!(u.bounds, [2.0, 4.0, 600.0, 100.0]);
        assert_eq!(u.cols[0], [-0.1, 0.2, -0.3, 0.4]);
        assert_eq!(u.roll[0], [0, 0, 5, 1]);
        assert_eq!(u.a[3], 3.0);
        assert_eq!(u.fx[1], 1.0, "clamped");
        assert_eq!(u.g, [0.5, 8.0, 8.0, 0.25]);
        assert_eq!(std::mem::size_of::<WaveUniforms>(), 16 * 5 + ROLL_COLUMNS * 8 + WAVE_COLUMNS * 8);
    }
}
