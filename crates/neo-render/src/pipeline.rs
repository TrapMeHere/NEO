//! Map render pipeline: the vertex shader performs the world→screen transform and the fragment shader outputs color directly.
//!
//! Why pan/zoom do not go into the vertex buffer: the vast majority of map interaction frames are panning/zooming; if vertices
//! were recomputed and re-uploaded every frame, both bandwidth and CPU would be wasted. Here the camera goes into a 32-byte uniform, and vertices are uploaded
//! only when data changes (docs/05 §2.6).

use bytemuck::{Pod, Zeroable};
use neo_geo::{Camera, WorldPoint};
use neo_scene::MapVertex;
use wgpu::util::DeviceExt as _;

/// Camera uniform (32 bytes).
///
/// The layout must match `Camera` in `shaders/map.wgsl`:
/// `center: vec2<f32>, viewport: vec2<f32>, scale: f32, grey: f32, pad: vec2<f32>`
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CameraUniform {
    /// Offset of the camera center relative to the batch origin (meters).
    pub center: [f32; 2],
    /// Viewport size (physical pixels).
    pub viewport: [f32; 2],
    /// Pixels per meter.
    pub scale: f32,
    /// Gray factor: 0 = normal colors, 1 = fully desaturated (inactive layers).
    pub grey: f32,
    /// Alignment padding.
    pub _pad: [f32; 2],
}

const _: () = assert!(core::mem::size_of::<CameraUniform>() == 32);

impl CameraUniform {
    /// Build from camera + batch origin.
    pub fn new(camera: &Camera, origin: WorldPoint, viewport_px: [u32; 2]) -> Self {
        Self {
            center: [
                (camera.center.x - origin.x) as f32,
                (camera.center.y - origin.y) as f32,
            ],
            viewport: [viewport_px[0] as f32, viewport_px[1] as f32],
            scale: camera.px_per_meter() as f32,
            grey: 0.0,
            _pad: [0.0; 2],
        }
    }

    /// Build with a gray toggle (used for inactive layers).
    pub fn with_grey(camera: &Camera, origin: WorldPoint, viewport_px: [u32; 2], grey: bool) -> Self {
        let mut uniform = Self::new(camera, origin, viewport_px);
        uniform.grey = if grey { 1.0 } else { 0.0 };
        uniform
    }

    /// Raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// Map pipeline (one pipeline handles Fill/Stroke/Point; the blend mode is the same).
pub struct MapPipeline {
    /// Pipeline.
    pub pipeline: wgpu::RenderPipeline,
    /// Camera uniform buffer.
    pub uniform_buffer: wgpu::Buffer,
    /// Bind group.
    pub bind_group: wgpu::BindGroup,
    /// Color attachment format (must match the render pass).
    pub color_format: wgpu::TextureFormat,
    /// MSAA sample count (must match the render pass).
    pub sample_count: u32,
}

impl MapPipeline {
    /// Create the pipeline.
    ///
    /// `color_format` must equal the render pass's color attachment format; `sample_count` must equal
    /// the pass's MSAA sample count (egui uses `egui_wgpu::RendererOptions::msaa_samples`,
    /// eframe uses `NativeOptions::multisampling`). A mismatch causes a runtime validation failure.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neo-map-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/map.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("neo-map-camera-bgl"),
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

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("neo-map-camera-uniform"),
            contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("neo-map-camera-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("neo-map-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("neo-map-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: core::mem::size_of::<MapVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Unorm8x4,
                            offset: 8,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: sample_count,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            color_format,
            sample_count,
        }
    }

    /// Update the camera uniform (once per frame, 32 bytes).
    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera,
        origin: WorldPoint,
        viewport_px: [u32; 2],
    ) {
        self.update_camera_with_grey(queue, camera, origin, viewport_px, false);
    }

    /// Update the camera uniform and specify whether to desaturate the data (inactive layers).
    pub fn update_camera_with_grey(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera,
        origin: WorldPoint,
        viewport_px: [u32; 2],
        grey: bool,
    ) {
        let uniform = CameraUniform::with_grey(camera, origin, viewport_px, grey);
        queue.write_buffer(&self.uniform_buffer, 0, uniform.as_bytes());
    }

    /// Bind the pipeline and camera inside the render pass.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
    }
}
