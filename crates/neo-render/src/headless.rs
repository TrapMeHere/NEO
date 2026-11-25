//! Offscreen (headless) rendering: render to a texture → read back RGBA → write a PNG.
//!
//! Uses:
//!
//! * the `neo-prototype render` subcommand of the performance verification prototype;
//! * CI GPU image regressions (no desktop window required);
//! * documentation screenshots.
//!
//! Implementation notes (wgpu 30):
//! * `TextureDescriptor.view_formats` is `&[TextureFormat]`;
//! * `bytes_per_row` of `copy_texture_to_buffer` must be 256-byte aligned → use an aligned row pitch and then crop;
//! * `device.poll(PollType::wait_indefinitely())` replaces the old `Maintain::Wait`;
//! * `Buffer::get_mapped_range` returns `Result`.

use std::time::Instant;

use neo_geo::Camera;
use neo_scene::SceneFrame;

use crate::gpu::{GpuContext, RenderError};
use crate::pipeline::MapPipeline;
use crate::scene_gpu::{GpuScene, UploadStats};

/// Time breakdown of one GPU frame (excluding readback).
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuFrameTiming {
    /// Total time of encode + submit + wait for completion (microseconds).
    pub total_micros: u64,
    /// Of which buffer upload time (microseconds; 0 when the cache is reused).
    pub upload_micros: u64,
    /// Bytes uploaded this time (0 when the cache is reused).
    pub uploaded_bytes: u64,
    /// Number of draw calls.
    pub draw_calls: u32,
    /// Triangle count.
    pub triangles: u32,
}

/// Offscreen renderer.
pub struct HeadlessRenderer {
    /// GPU context.
    pub ctx: GpuContext,
    /// Pipeline.
    pub pipeline: MapPipeline,
    /// GPU scene cache.
    pub gpu_scene: GpuScene,
    /// Texture.
    pub target: wgpu::Texture,
    /// Texture view.
    pub target_view: wgpu::TextureView,
    /// Readback staging buffer.
    pub staging: wgpu::Buffer,
    /// Size (pixels).
    pub size: [u32; 2],
    /// Duration of the most recent render (microseconds).
    pub last_render_micros: u64,
    /// Most recent upload stats.
    pub last_upload: UploadStats,
}

/// 8-bit RGBA image.
#[derive(Clone, Debug)]
pub struct RgbaImage {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// RGBA data (row-major, no padding).
    pub data: Vec<u8>,
}

impl HeadlessRenderer {
    /// Create an offscreen renderer of the given size (RGBA8).
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

    /// Only "encode + submit + wait for the GPU to finish", no readback.
    ///
    /// Used to isolate GPU frame time from the readback cost of `copy_texture_to_buffer` + `map_async`;
    /// the two are listed separately in performance reports (see docs/14).
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

    /// Render one frame and read back RGBA.
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

    /// Render and write a PNG.
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
