//! GPU pipeline for waveform rendering
//!
//! Two bindings per view:
//! - Binding 0: Uniform buffer (WaveformUniforms, 384 bytes, updated every frame)
//! - Binding 1: Storage buffer (peak data, updated only on track load)

use super::PeakBuffer;
use iced::widget::shader;
use iced::Rectangle;
use std::collections::HashMap;
use std::sync::Arc;

// =============================================================================
// Uniform buffer layout (must match waveform.wgsl exactly)
// =============================================================================

/// Uniform data for a single waveform view, packed as 24 vec4s (384 bytes).
///
/// This is uploaded to the GPU every frame but is only 384 bytes — trivial
/// compared to the old canvas approach that rebuilt ~1MB of geometry per frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WaveformUniforms {
    /// Widget bounds in logical pixels [x, y, width, height]
    pub bounds: [f32; 4],
    /// [playhead_norm, height_scale, peaks_per_stem, is_overview]
    pub view_params: [f32; 4],
    /// [window_start_norm, window_end_norm, window_total_peaks, bpm_scale]
    pub window_params: [f32; 4],
    /// Per-stem active flags [0.0 or 1.0 × 4]
    pub stem_active: [f32; 4],
    /// Stem colors (RGBA) × 4
    pub stem_color_0: [f32; 4],
    pub stem_color_1: [f32; 4],
    pub stem_color_2: [f32; 4],
    pub stem_color_3: [f32; 4],
    /// [loop_start, loop_end, loop_active, has_track]
    pub loop_params: [f32; 4],
    /// [grid_step_norm, first_beat_norm, beats_per_bar, volume]
    pub beat_params: [f32; 4],
    /// [cue_count, main_cue_pos, has_main_cue, slicer_active]
    pub cue_params: [f32; 4],
    /// [slicer_start, slicer_end, current_slice, peaks_per_pixel]
    pub slicer_params: [f32; 4],
    /// Cue positions 0-3 (normalized)
    pub cue_pos_0_3: [f32; 4],
    /// Cue positions 4-7 (normalized)
    pub cue_pos_4_7: [f32; 4],
    /// Cue colors (RGBA) × 8
    pub cue_color_0: [f32; 4],
    pub cue_color_1: [f32; 4],
    pub cue_color_2: [f32; 4],
    pub cue_color_3: [f32; 4],
    pub cue_color_4: [f32; 4],
    pub cue_color_5: [f32; 4],
    pub cue_color_6: [f32; 4],
    pub cue_color_7: [f32; 4],
    /// Reserved (was: per-stem Gaussian smooth radius multiplier).
    /// Kept to maintain uniform layout alignment.
    pub stem_smooth: [f32; 4],
}

// =============================================================================
// Primitive (per-frame data passed from Program::draw to Pipeline::prepare)
// =============================================================================

/// Per-frame primitive for a single waveform view.
///
/// Created by `WaveformProgram::draw()`, consumed by the pipeline's `prepare()`.
/// The `peaks` field is an `Arc`-clone — zero-cost per frame.
#[derive(Debug, Clone)]
pub struct WaveformPrimitive {
    /// Stable ID for GPU resource lookup (deck_idx * 2 + is_overview)
    pub id: u64,
    /// Packed uniform data for this frame
    pub uniforms: WaveformUniforms,
    /// Peak data for storage buffer (None = no track loaded)
    pub peaks: Option<PeakBuffer>,
}

// =============================================================================
// Pipeline (GPU resources, created once)
// =============================================================================

/// Per-view GPU resources cached across frames.
struct ViewResources {
    uniform_buffer: wgpu::Buffer,
    peak_buffer: wgpu::Buffer,
    peak_capacity: usize,
    bind_group: wgpu::BindGroup,
    /// Pointer to the Arc<Vec<f32>> data for change detection.
    /// If the pointer hasn't changed, we skip re-uploading peak data.
    last_peak_ptr: usize,
}

/// GPU pipeline for waveform rendering.
///
/// Manages shader, render pipeline, and per-view resource caches.
/// Peak data is uploaded once at track load; only the 384-byte uniform
/// buffer is updated per frame.
pub struct WaveformPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// Per-view resources keyed by stable view ID
    view_resources: HashMap<u64, ViewResources>,
}

impl shader::Pipeline for WaveformPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Waveform Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("waveform.wgsl").into()),
        });

        // Two bindings: uniform (0) + storage (1)
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Waveform Bind Group Layout"),
            entries: &[
                // Binding 0: Uniforms
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
                // Binding 1: Peak data (read-only storage buffer)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Waveform Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Waveform Render Pipeline"),
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
            view_resources: HashMap::new(),
        }
    }
}

impl shader::Primitive for WaveformPrimitive {
    type Pipeline = WaveformPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        // Determine peak data pointer for change detection
        let peak_ptr = self
            .peaks
            .as_ref()
            .map(|p| Arc::as_ptr(&p.data) as usize)
            .unwrap_or(0);

        let peak_data_len = self
            .peaks
            .as_ref()
            .map(|p| p.data.len())
            .unwrap_or(0);

        // Check if we need to recreate resources (peak buffer size changed or first time)
        let needs_recreate = match pipeline.view_resources.get(&self.id) {
            None => true,
            Some(res) => peak_data_len > res.peak_capacity,
        };

        if needs_recreate {
            // Minimum storage buffer size: 4 bytes (wgpu requires non-zero for storage buffers)
            let peak_buf_size = if peak_data_len > 0 {
                (peak_data_len * std::mem::size_of::<f32>()) as u64
            } else {
                4 // Minimum valid storage buffer size
            };

            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Waveform Uniform Buffer"),
                size: std::mem::size_of::<WaveformUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let peak_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Waveform Peak Buffer"),
                size: peak_buf_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Waveform Bind Group"),
                layout: &pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: peak_buffer.as_entire_binding(),
                    },
                ],
            });

            pipeline.view_resources.insert(
                self.id,
                ViewResources {
                    uniform_buffer,
                    peak_buffer,
                    peak_capacity: peak_data_len,
                    bind_group,
                    last_peak_ptr: 0, // Force upload on first frame
                },
            );
        }

        let resources = pipeline.view_resources.get_mut(&self.id).unwrap();

        // Upload uniforms every frame (384 bytes — trivial)
        queue.write_buffer(
            &resources.uniform_buffer,
            0,
            bytemuck::bytes_of(&self.uniforms),
        );

        // Upload peak data only when it changes (Arc pointer comparison)
        if peak_ptr != resources.last_peak_ptr {
            if let Some(peaks) = &self.peaks {
                queue.write_buffer(
                    &resources.peak_buffer,
                    0,
                    bytemuck::cast_slice(&peaks.data),
                );
            }
            resources.last_peak_ptr = peak_ptr;
        }
    }

    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool {
        let Some(resources) = pipeline.view_resources.get(&self.id) else {
            return true;
        };

        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);

        // Fullscreen triangle: 3 vertices, no vertex buffer
        render_pass.draw(0..3, 0..1);

        true
    }
}
