//! GPU pipeline for knob rendering

use iced::mouse;
use iced::widget::shader;
use iced::{Color, Rectangle};
use std::collections::HashMap;

/// Modulation range indicator
#[derive(Debug, Clone, Copy)]
pub struct ModulationRange {
    /// Minimum value of the range (0.0 - 1.0)
    pub min: f32,
    /// Maximum value of the range (0.0 - 1.0)
    pub max: f32,
    /// Color of this modulation indicator
    pub color: Color,
}

impl ModulationRange {
    /// Create a new modulation range
    pub fn new(min: f32, max: f32, color: Color) -> Self {
        Self {
            min: min.clamp(0.0, 1.0),
            max: max.clamp(0.0, 1.0),
            color,
        }
    }
}

/// Uniform data sent to the shader (per-primitive)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    // Widget bounds: [x, y, width, height] in physical pixels (scaled)
    bounds: [f32; 4],
    // [value, dragging, bipolar, mod_count]
    params: [f32; 4],
    // [min0, max0, min1, max1]
    mod_ranges_01: [f32; 4],
    // [min2, max2, min3, max3]
    mod_ranges_23: [f32; 4],

    mod_color_0: [f32; 4],
    mod_color_1: [f32; 4],
    mod_color_2: [f32; 4],
    mod_color_3: [f32; 4],

    bg_color: [f32; 4],
    track_color: [f32; 4],
    value_color: [f32; 4],
    notch_color: [f32; 4],
}

/// Per-primitive GPU resources
struct PrimitiveResources {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

fn color_to_array(c: Color) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

/// Shader program for knob rendering
///
/// This holds the data needed to render a single knob frame.
/// The `id` field is used to look up GPU resources in the pipeline cache.
#[derive(Debug, Clone)]
pub(crate) struct KnobProgram {
    /// Stable ID from the parent Knob widget
    pub id: u64,
    /// Current value (0.0 - 1.0)
    pub value: f32,
    /// Whether the knob is being dragged
    pub dragging: bool,
    /// Bipolar mode (value arc from center instead of min)
    pub bipolar: bool,
    /// Modulation ranges to display
    pub modulations: Vec<ModulationRange>,
    /// Background color
    pub bg_color: Color,
    /// Track color (unfilled arc)
    pub track_color: Color,
    /// Value color (filled arc)
    pub value_color: Color,
    /// Notch/indicator color
    pub notch_color: Color,
}

impl shader::Program<()> for KnobProgram {
    type State = ();
    type Primitive = KnobPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        KnobPrimitive {
            id: self.id,
            value: self.value,
            dragging: self.dragging,
            bipolar: self.bipolar,
            modulations: self.modulations.clone(),
            bg_color: self.bg_color,
            track_color: self.track_color,
            value_color: self.value_color,
            notch_color: self.notch_color,
        }
    }
}

/// Primitive for knob rendering - created by KnobProgram::draw()
#[derive(Debug, Clone)]
pub struct KnobPrimitive {
    /// Stable ID for GPU resource lookup
    id: u64,
    value: f32,
    dragging: bool,
    bipolar: bool,
    modulations: Vec<ModulationRange>,
    bg_color: Color,
    track_color: Color,
    value_color: Color,
    notch_color: Color,
}

impl KnobPrimitive {
    /// Build uniforms with the given bounds and scale factor
    fn build_uniforms(&self, bounds: &Rectangle, scale: f32) -> Uniforms {
        let mut mod_ranges_01 = [0.0f32; 4];
        let mut mod_ranges_23 = [0.0f32; 4];

        // Pack modulation ranges
        if let Some(m) = self.modulations.get(0) {
            mod_ranges_01[0] = m.min;
            mod_ranges_01[1] = m.max;
        }
        if let Some(m) = self.modulations.get(1) {
            mod_ranges_01[2] = m.min;
            mod_ranges_01[3] = m.max;
        }
        if let Some(m) = self.modulations.get(2) {
            mod_ranges_23[0] = m.min;
            mod_ranges_23[1] = m.max;
        }
        if let Some(m) = self.modulations.get(3) {
            mod_ranges_23[2] = m.min;
            mod_ranges_23[3] = m.max;
        }

        // Scale bounds to physical pixels for HiDPI support
        let mut uniforms = Uniforms {
            bounds: [
                bounds.x * scale,
                bounds.y * scale,
                bounds.width * scale,
                bounds.height * scale,
            ],
            params: [
                self.value,
                if self.dragging { 1.0 } else { 0.0 },
                if self.bipolar { 1.0 } else { 0.0 },
                self.modulations.len() as f32,
            ],
            mod_ranges_01,
            mod_ranges_23,
            mod_color_0: [0.0; 4],
            mod_color_1: [0.0; 4],
            mod_color_2: [0.0; 4],
            mod_color_3: [0.0; 4],
            bg_color: color_to_array(self.bg_color),
            track_color: color_to_array(self.track_color),
            value_color: color_to_array(self.value_color),
            notch_color: color_to_array(self.notch_color),
        };

        // Fill in modulation colors
        for (i, m) in self.modulations.iter().take(4).enumerate() {
            let color = color_to_array(m.color);
            match i {
                0 => uniforms.mod_color_0 = color,
                1 => uniforms.mod_color_1 = color,
                2 => uniforms.mod_color_2 = color,
                3 => uniforms.mod_color_3 = color,
                _ => {}
            }
        }

        uniforms
    }
}

/// The GPU pipeline for rendering knobs
pub struct KnobPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// Per-primitive resources, keyed by stable knob ID
    primitive_resources: HashMap<u64, PrimitiveResources>,
}

impl shader::Pipeline for KnobPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        // Load shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Knob Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("knob.wgsl").into()),
        });

        // Create bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Knob Bind Group Layout"),
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

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Knob Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Knob Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            primitive_resources: HashMap::new(),
        }
    }
}

impl shader::Primitive for KnobPrimitive {
    type Pipeline = KnobPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        // Build uniforms with this primitive's data, scaling for HiDPI
        let scale = viewport.scale_factor() as f32;

        // Safety: ensure scale factor is valid
        let scale = if scale > 0.0 && scale.is_finite() { scale } else { 1.0 };

        let uniforms = self.build_uniforms(bounds, scale);

        // Get or create resources for this knob using its stable ID
        let resources = pipeline.primitive_resources.entry(self.id).or_insert_with(|| {
            // Create a new buffer for this knob
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Knob Uniform Buffer"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Create bind group for this buffer
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Knob Bind Group"),
                layout: &pipeline.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });

            PrimitiveResources { buffer, bind_group }
        });

        // Update the buffer with current data
        queue.write_buffer(&resources.buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        // Look up resources using the stable ID
        let Some(resources) = pipeline.primitive_resources.get(&self.id) else {
            return; // No resources prepared for this knob
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Knob Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &resources.bind_group, &[]);

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );

        // Draw fullscreen triangle (3 vertices, no vertex buffer)
        pass.draw(0..3, 0..1);
    }
}

