//! [`MapGpu`]：把 [`MapPipeline`] + [`GpuScene`] 组合成可直接嵌入 egui paint callback 的对象。
//!
//! 这一层**不依赖 egui**：应用层在自己的 `CallbackTrait` 实现里持有 `MapGpu`，
//! `prepare` 时上传/更新 uniform，`paint` 时发 draw call。这样渲染后端保持 UI 无关，
//! 同时 egui 集成代码只有 ~30 行（见 `apps/neo-desktop`）。

use neo_geo::{Camera, WorldPoint};
use neo_scene::SceneFrame;

use crate::pipeline::MapPipeline;
use crate::scene_gpu::{GpuScene, UploadStats};

/// 可嵌入 egui paint callback 的地图 GPU 资源。
pub struct MapGpu {
    /// 管线（含 camera uniform）。
    pub pipeline: MapPipeline,
    /// 几何缓存。
    pub scene: GpuScene,
}

impl MapGpu {
    /// 创建。`color_format`/`sample_count` 必须与目标 render pass 完全一致。
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat, sample_count: u32) -> Self {
        Self {
            pipeline: MapPipeline::new(device, color_format, sample_count),
            scene: GpuScene::new(),
        }
    }

    /// 每帧调用：必要时上传几何，并写 32 字节相机 uniform。
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &SceneFrame,
        camera: &Camera,
        viewport_px: [u32; 2],
        grey: bool,
    ) -> UploadStats {
        let stats = self.scene.upload(device, scene, scene.origin);
        if stats.uploaded {
            tracing::debug!(
                "gpu upload: {:.2} ms, {:.1} KiB, {} draw calls, zoom {:.2}",
                stats.micros as f64 / 1000.0,
                stats.bytes as f64 / 1024.0,
                stats.draw_calls,
                camera.zoom
            );
        }
        self.pipeline
            .update_camera_with_grey(queue, camera, scene.origin, viewport_px, grey);
        stats
    }

    /// 在 render pass 中绘制（按批次顺序：fill → stroke → point）。
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.pipeline.bind(pass);
        self.scene.draw(pass);
    }

    /// 最近一次上传统计。
    pub fn upload_stats(&self) -> UploadStats {
        self.scene.stats
    }

    /// 当前批次原点。
    pub fn origin(&self) -> Option<WorldPoint> {
        self.scene.origin
    }
}
