//! 地图渲染管线：顶点着色器做世界→屏幕变换，片元直出颜色。
//!
//! 为什么平移/缩放不进顶点 buffer：地图交互的绝大多数帧都在平移/缩放，如果每帧
//! 重算并重传顶点，带宽和 CPU 都浪费。这里把相机放进 32 字节 uniform，顶点只在
//! 数据变化时上传（docs/05 §2.6）。

use bytemuck::{Pod, Zeroable};
use neo_geo::{Camera, WorldPoint};
use neo_scene::MapVertex;
use wgpu::util::DeviceExt as _;

/// 相机 uniform（32 字节）。
///
/// 布局必须与 `shaders/map.wgsl` 的 `Camera` 一致：
/// `center: vec2<f32>, viewport: vec2<f32>, scale: f32, grey: f32, pad: vec2<f32>`
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CameraUniform {
    /// 相机中心相对批次原点的偏移（米）。
    pub center: [f32; 2],
    /// 视口尺寸（物理像素）。
    pub viewport: [f32; 2],
    /// 每米像素数。
    pub scale: f32,
    /// 灰度系数：0 = 正常颜色，1 = 完全去色（只读图层）。
    pub grey: f32,
    /// 对齐填充。
    pub _pad: [f32; 2],
}

const _: () = assert!(core::mem::size_of::<CameraUniform>() == 32);

impl CameraUniform {
    /// 从相机 + 批次原点构造。
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

    /// 带灰度开关的构造（非活动图层用）。
    pub fn with_grey(camera: &Camera, origin: WorldPoint, viewport_px: [u32; 2], grey: bool) -> Self {
        let mut uniform = Self::new(camera, origin, viewport_px);
        uniform.grey = if grey { 1.0 } else { 0.0 };
        uniform
    }

    /// 原始字节。
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// 地图管线（一个 pipeline 处理 Fill/Stroke/Point；混合模式相同）。
pub struct MapPipeline {
    /// 管线。
    pub pipeline: wgpu::RenderPipeline,
    /// 相机 uniform buffer。
    pub uniform_buffer: wgpu::Buffer,
    /// 绑组。
    pub bind_group: wgpu::BindGroup,
    /// 颜色附件格式（必须与 render pass 一致）。
    pub color_format: wgpu::TextureFormat,
    /// MSAA 采样数（必须与 render pass 一致）。
    pub sample_count: u32,
}

impl MapPipeline {
    /// 创建管线。
    ///
    /// `color_format` 必须等于 render pass 的颜色附件格式；`sample_count` 必须等于
    /// pass 的 MSAA 采样数（egui 用 `egui_wgpu::RendererOptions::msaa_samples`，
    /// eframe 用 `NativeOptions::multisampling`）。不匹配会导致运行时校验失败。
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

    /// 更新相机 uniform（每帧一次，32 字节）。
    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        camera: &Camera,
        origin: WorldPoint,
        viewport_px: [u32; 2],
    ) {
        self.update_camera_with_grey(queue, camera, origin, viewport_px, false);
    }

    /// 更新相机 uniform，并指定是否对数据去色（非活动图层）。
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

    /// 在 render pass 中绑定管线与相机。
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
    }
}
