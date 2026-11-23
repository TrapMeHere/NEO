//! egui-wgpu 集成的全部代码（~60 行）。
//!
//! 这是 UI 层与渲染层唯一的接触点：`MapCallback` 实现 [`egui_wgpu::CallbackTrait`]，
//! 从 `Renderer::callback_resources` 里取出 [`MapGpu`]，在 `prepare` 阶段上传/更新
//! uniform，在 `paint` 阶段把 draw call 发进 egui 自己的 render pass。
//!
//! 关键约束（docs/05 §2.4，已按 wgpu 30 / egui-wgpu 0.36.2 校准）：
//! * pipeline 的 color format 必须等于 `RenderState::target_format`；
//! * pipeline 的 sample_count 必须等于 `RendererOptions::msaa_samples`
//!   （eframe 用 `NativeOptions::multisampling`，`0` 与 `1` 都是"关"）；
//! * `paint` 给的 pass 生命周期是 `'static`，资源来自 `CallbackResources` 借用。

use std::sync::Arc;

use egui_wgpu::wgpu;
use neo_geo::Camera;
use neo_render::MapGpu;
use neo_scene::SceneFrame;

/// 一帧地图绘制回调。
pub struct MapCallback {
    /// 场景（不可变，跨线程共享）。
    pub scene: Arc<SceneFrame>,
    /// 当前相机。
    pub camera: Camera,
    /// 画布尺寸（物理像素，来自 egui rect × pixels_per_point）。
    pub viewport_px: [u32; 2],
    /// 是否对数据去色（非活动图层只读）。
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
