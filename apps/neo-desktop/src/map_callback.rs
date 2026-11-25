//! All of the egui-wgpu integration (~60 lines).
//!
//! This is the only contact point between the UI layer and the render layer: `MapCallback` implements [`egui_wgpu::CallbackTrait`],
//! pulls [`MapGpu`] out of `Renderer::callback_resources`, uploads/updates the uniform in `prepare`,
//! and issues draw calls into egui's own render pass in `paint`.
//!
//! Key constraints (docs/05 §2.4, calibrated against wgpu 30 / egui-wgpu 0.36.2):
//! * The pipeline's color format must equal `RenderState::target_format`;
//! * The pipeline's sample_count must equal `RendererOptions::msaa_samples`
//!   (eframe uses `NativeOptions::multisampling`; both `0` and `1` mean "off");
//! * The pass lifetime given to `paint` is `'static`; resources come from a `CallbackResources` borrow.

use std::sync::Arc;

use egui_wgpu::wgpu;
use neo_geo::Camera;
use neo_render::MapGpu;
use neo_scene::SceneFrame;

/// One map frame draw callback.
pub struct MapCallback {
    /// Scene (immutable, shared across threads).
    pub scene: Arc<SceneFrame>,
    /// Current camera.
    pub camera: Camera,
    /// Canvas size (physical pixels, from egui rect × pixels_per_point).
    pub viewport_px: [u32; 2],
    /// Whether to desaturate the data (inactive layers are read-only).
    pub grey: bool,
}

impl egui_wgpu::CallbackTrait for MapCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = callback_resources.get_mut::<MapGpu>() {
            gpu.prepare(
                device,
                queue,
                &self.scene,
                &self.camera,
                self.viewport_px,
                self.grey,
            );
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(gpu) = callback_resources.get::<MapGpu>() {
            gpu.paint(render_pass);
        }
    }
}
