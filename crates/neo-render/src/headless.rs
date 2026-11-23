//! 离屏（headless）渲染：渲染到纹理 → 读回 RGBA → 写 PNG。
//!
//! 用途：
//!
//! * 性能验证原型 `neo-prototype render` 子命令；
//! * CI 的 GPU 图像回归（不依赖桌面窗口）；
//! * 文档截图。
//!
//! 实现注意（wgpu 30）：
//! * `TextureDescriptor.view_formats` 是 `&[TextureFormat]`；
//! * `copy_texture_to_buffer` 的 `bytes_per_row` 必须 256 字节对齐 → 使用对齐行距再裁剪；
//! * `device.poll(PollType::wait_indefinitely())` 取代旧的 `Maintain::Wait`；
//! * `Buffer::get_mapped_range` 返回 `Result`。

use std::time::Instant;

use neo_geo::Camera;
use neo_scene::SceneFrame;

use crate::gpu::{GpuContext, RenderError};
use crate::pipeline::MapPipeline;
use crate::scene_gpu::{GpuScene, UploadStats};

/// 一次 GPU 帧的耗时分解（不含读回）。
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuFrameTiming {
    /// 编码 + 提交 + 等待完成的总耗时（微秒）。
    pub total_micros: u64,
    /// 其中 buffer 上传耗时（微秒；复用缓存时为 0）。
    pub upload_micros: u64,
    /// 本次上传字节数（复用缓存时为 0）。
    pub uploaded_bytes: u64,
    /// draw call 数。
    pub draw_calls: u32,
    /// 三角形数。
    pub triangles: u32,
}

/// 离屏渲染器。
pub struct HeadlessRenderer {
    /// GPU 上下文。
    pub ctx: GpuContext,
    /// 管线。
    pub pipeline: MapPipeline,
    /// GPU 场景缓存。
    pub gpu_scene: GpuScene,
    /// 纹理。
    pub target: wgpu::Texture,
    /// 纹理视图。
    pub target_view: wgpu::TextureView,
    /// 读回 staging buffer。
    pub staging: wgpu::Buffer,
    /// 尺寸（像素）。
    pub size: [u32; 2],
    /// 最近一次渲染耗时（微秒）。
    pub last_render_micros: u64,
    /// 最近一次上传统计。
    pub last_upload: UploadStats,
}

/// 8 位 RGBA 图像。
#[derive(Clone, Debug)]
pub struct RgbaImage {
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// RGBA 数据（行优先，无 padding）。
    pub data: Vec<u8>,
}

impl HeadlessRenderer {
    /// 创建指定尺寸的离屏渲染器（RGBA8）。
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let width = width.max(1);
        let height = height.max(1);
        let ctx = GpuContext::new_headless()?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let pipeline = MapPipeline::new(&ctx.device, format, 1);

        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("neo-headless-target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = width * 4;
        let padded_row = padded_row.div_ceil(align) * align;
        let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("neo-headless-readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Ok(Self {
            ctx,
            pipeline,
            gpu_scene: GpuScene::new(),
            target,
            target_view,
            staging,
            size: [width, height],
            last_render_micros: 0,
            last_upload: UploadStats::default(),
        })
    }

    /// 只做"编码 + 提交 + 等待 GPU 完成"，不做读回。
    ///
    /// 用于隔离 GPU 帧时间与 `copy_texture_to_buffer` + `map_async` 的读回成本；
    /// 性能报告里两者分开列（见 docs/14）。
    pub fn render_encode_submit(
        &mut self,
        scene: &SceneFrame,
        camera: &Camera,
        background: [u8; 4],
    ) -> Result<GpuFrameTiming, RenderError> {
        let started = Instant::now();
        let upload = self.gpu_scene.upload(&self.ctx.device, scene, scene.origin);
        self.pipeline
            .update_camera(&self.ctx.queue, camera, scene.origin, self.size);

        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("neo-headless-timing-encoder"),
            });
        {
            let color = wgpu::Color {
                r: f64::from(background[0]) / 255.0,
                g: f64::from(background[1]) / 255.0,
                b: f64::from(background[2]) / 255.0,
                a: f64::from(background[3]) / 255.0,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("neo-map-timing-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.pipeline.bind(&mut pass);
            self.gpu_scene.draw(&mut pass);
        }
        self.ctx.queue.submit([encoder.finish()]);
        self.ctx
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Backend(e.to_string()))?;
        let total_micros = started.elapsed().as_micros() as u64;
        self.last_upload = upload;
        Ok(GpuFrameTiming {
            total_micros,
            upload_micros: upload.micros,
            uploaded_bytes: upload.bytes,
            draw_calls: self.gpu_scene.batches.len() as u32,
            triangles: scene.total_triangles() as u32,
        })
    }

    /// 渲染一帧并读回 RGBA。
    pub fn render(
        &mut self,
        scene: &SceneFrame,
        camera: &Camera,
        background: [u8; 4],
    ) -> Result<RgbaImage, RenderError> {
        let started = Instant::now();
        self.last_upload = self.gpu_scene.upload(&self.ctx.device, scene, scene.origin);

        self.pipeline
            .update_camera(&self.ctx.queue, camera, scene.origin, self.size);

        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("neo-headless-encoder"),
            });

        {
            let color = wgpu::Color {
                r: f64::from(background[0]) / 255.0,
                g: f64::from(background[1]) / 255.0,
                b: f64::from(background[2]) / 255.0,
                a: f64::from(background[3]) / 255.0,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("neo-map-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.pipeline.bind(&mut pass);
            self.gpu_scene.draw(&mut pass);
        }

        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = (self.size[0] * 4).div_ceil(align) * align;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(self.size[1]),
                },
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
        );
        self.ctx.queue.submit([encoder.finish()]);

        let size = u64::from(padded_row) * u64::from(self.size[1]);
        let raw = self.ctx.read_buffer(&self.staging, size)?;
        let mut data = Vec::with_capacity((self.size[0] * self.size[1] * 4) as usize);
        for row in 0..self.size[1] {
            let start = (row * padded_row) as usize;
            let end = start + (self.size[0] * 4) as usize;
            data.extend_from_slice(&raw[start..end]);
        }
        self.last_render_micros = started.elapsed().as_micros() as u64;
        Ok(RgbaImage {
            width: self.size[0],
            height: self.size[1],
            data,
        })
    }

    /// 渲染并写 PNG。
    pub fn render_to_png(
        &mut self,
        scene: &SceneFrame,
        camera: &Camera,
        background: [u8; 4],
        path: &std::path::Path,
    ) -> Result<RgbaImage, RenderError> {
        let image = self.render(scene, camera, background)?;
        let rgba = image::RgbaImage::from_raw(image.width, image.height, image.data.clone())
            .ok_or_else(|| RenderError::Image("invalid RGBA buffer dimensions".to_owned()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rgba.save(path).map_err(|e| RenderError::Image(e.to_string()))?;
        Ok(image)
    }
}
