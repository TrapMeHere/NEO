//! [`MapGpu`]: combines [`MapPipeline`] + [`GpuScene`] into an object that can be embedded directly in an egui paint callback.
//!
//! This layer **does not depend on egui**: the application layer holds `MapGpu` inside its own `CallbackTrait` implementation,
//! uploads/updates the uniform in `prepare`, and issues draw calls in `paint`. This keeps the render backend UI-agnostic,
//! while the egui integration code is only ~30 lines (see `apps/neo-desktop`).

use neo_geo::{Camera, WorldPoint};
use neo_scene::SceneFrame;

use crate::pipeline::MapPipeline;
use crate::scene_gpu::{GpuScene, UploadStats};

/// Map GPU resources that can be embedded in an egui paint callback.
pub struct MapGpu {
    /// Pipeline (including the camera uniform).
    pub pipeline: MapPipeline,
    /// Geometry cache.
    pub scene: GpuScene,
}

impl MapGpu {
    /// Create. `color_format`/`sample_count` must exactly match the target render pass.
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat, sample_count: u32) -> Self {
        Self {
            pipeline: MapPipeline::new(device, color_format, sample_count),
            scene: GpuScene::new(),
        }
    }

    /// Called every frame: upload geometry when necessary and write the 32-byte camera uniform.
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

    /// Draw inside the render pass (in batch order: fill → stroke → point).
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.pipeline.bind(pass);
        self.scene.draw(pass);
    }

    /// Most recent upload stats.
    pub fn upload_stats(&self) -> UploadStats {
        self.scene.stats
    }

    /// Current batch origin.
    pub fn origin(&self) -> Option<WorldPoint> {
        self.scene.origin
    }
}
