//! GPU oscilloscope trace.
//!
//! One custom shader primitive draws a decimated signal (up to
//! [`SCOPE_SHADER_POINTS`] points) as an anti-aliased line with a faint fill.
//! The points travel in a 2 KB uniform buffer that is rewritten per frame; the
//! fragment shader samples the two neighbouring points per pixel column, so
//! the cost is one fullscreen triangle per scope and no vertex data.
//!
//! Each scope needs a stable `id` (its GPU buffer is cached under it).

use std::collections::HashMap;
use std::sync::Arc;

use iced::widget::shader;
use iced::{mouse, Color, Element, Length, Rectangle};

/// Points the shader can hold. Longer inputs are truncated to the newest.
pub const SCOPE_SHADER_POINTS: usize = 512;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    bounds: [f32; 4],
    color: [f32; 4],
    // [point count, gain, line thickness px, fill alpha]
    params: [f32; 4],
    points: [[f32; 4]; SCOPE_SHADER_POINTS / 4],
}

/// Display settings of one scope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScopeStyle {
    pub color: Color,
    /// Multiplier applied to the points before clamping to ±1.
    pub gain: f32,
    /// Line thickness in logical pixels.
    pub thickness: f32,
    /// Alpha of the fill between the trace and the centre line.
    pub fill_alpha: f32,
}

impl Default for ScopeStyle {
    fn default() -> Self {
        Self { color: Color::WHITE, gain: 1.0, thickness: 2.0, fill_alpha: 0.18 }
    }
}

/// A scope trace widget. `points` are oldest first, nominally in `-1..=1`.
pub fn scope<'a, Message: 'a>(
    id: u64,
    points: Arc<[f32]>,
    style: ScopeStyle,
    width: Length,
    height: Length,
) -> Element<'a, Message> {
    shader(ScopeProgram { id, points, style }).width(width).height(height).into()
}

struct ScopeProgram {
    id: u64,
    points: Arc<[f32]>,
    style: ScopeStyle,
}

impl<Message> shader::Program<Message> for ScopeProgram {
    type State = ();
    type Primitive = ScopePrimitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, _bounds: Rectangle) -> ScopePrimitive {
        ScopePrimitive { id: self.id, points: Arc::clone(&self.points), style: self.style }
    }
}

#[derive(Debug)]
pub struct ScopePrimitive {
    id: u64,
    points: Arc<[f32]>,
    style: ScopeStyle,
}

impl ScopePrimitive {
    fn uniforms(&self, bounds: &Rectangle, scale: f32) -> Uniforms {
        let src = &self.points[self.points.len().saturating_sub(SCOPE_SHADER_POINTS)..];
        let mut points = [[0.0f32; 4]; SCOPE_SHADER_POINTS / 4];
        for (i, &v) in src.iter().enumerate() {
            points[i / 4][i % 4] = if v.is_finite() { v } else { 0.0 };
        }
        let c = self.style.color;
        Uniforms {
            bounds: [bounds.x * scale, bounds.y * scale, bounds.width * scale, bounds.height * scale],
            color: [c.r, c.g, c.b, c.a],
            params: [src.len() as f32, self.style.gain, self.style.thickness * scale, self.style.fill_alpha],
            points,
        }
    }
}

struct Resources {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct ScopePipeline {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    resources: HashMap<u64, Resources>,
}

impl shader::Pipeline for ScopePipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Scope Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scope.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Scope Bind Group Layout"),
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
            label: Some("Scope Pipeline Layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Scope Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        Self { pipeline, layout, resources: HashMap::new() }
    }
}

impl shader::Primitive for ScopePrimitive {
    type Pipeline = ScopePipeline;

    fn prepare(
        &self,
        pipeline: &mut ScopePipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        let scale = if scale > 0.0 && scale.is_finite() { scale } else { 1.0 };
        let uniforms = self.uniforms(bounds, scale);
        let layout = &pipeline.layout;
        let res = pipeline.resources.entry(self.id).or_insert_with(|| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Scope Uniform Buffer"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Scope Bind Group"),
                layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            Resources { buffer, bind_group }
        });
        queue.write_buffer(&res.buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    fn draw(&self, pipeline: &ScopePipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(res) = pipeline.resources.get(&self.id) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &res.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_pack_newest_points_and_scale_bounds() {
        let points: Arc<[f32]> = (0..600).map(|i| i as f32).collect();
        let prim = ScopePrimitive { id: 0, points, style: ScopeStyle::default() };
        let u = prim.uniforms(&Rectangle { x: 1.0, y: 2.0, width: 100.0, height: 50.0 }, 2.0);
        assert_eq!(u.bounds, [2.0, 4.0, 200.0, 100.0]);
        assert_eq!(u.params[0], SCOPE_SHADER_POINTS as f32);
        // Truncated to the newest 512: first packed point is 600 - 512 = 88.
        assert_eq!(u.points[0][0], 88.0);
        assert_eq!(u.points[127][3], 599.0);
        assert_eq!(std::mem::size_of::<Uniforms>(), 48 + SCOPE_SHADER_POINTS * 4);
    }
}
