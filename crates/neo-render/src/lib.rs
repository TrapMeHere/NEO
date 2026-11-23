//! # neo-render — wgpu 渲染后端
//!
//! 只负责 GPU：设备/队列、管线、顶点/索引 buffer、绘制、离屏渲染与读回。
//! **不依赖** store/editor/ui（分层见 ADR-0004）；egui 集成在应用层实现
//! （`apps/neo-desktop` 里实现 `egui_wgpu::CallbackTrait`，把本 crate 的
//! [`MapGpu`] 当作 paint callback 资源）。
//!
//! 关键设计（docs/05）：
//!
//! * 顶点坐标是**相对批次原点的 f32 米**；平移/缩放只改 32 字节 uniform，不重传几何；
//! * 数据变化才重新上传批次（按 `SceneFrame::version` + `origin` 判断）；
//! * 一次 draw call 覆盖一个批次（Fill / Stroke / Point 各一个），而不是每个对象一次；
//! * 离屏渲染 + PNG 读回用于无窗口验证（CI/基准/截图）。
//!
//! wgpu 30 的 API 与旧版差异很大（`PollType`、`CurrentSurfaceTexture`、
//! `multiview_mask`、`entry_point: Option<&str>` 等），代码按 30.0.2 编写。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cull_gpu;
pub mod gpu;
pub mod headless;
pub mod map_gpu;
pub mod pipeline;
pub mod scene_gpu;

pub use cull_gpu::zero_instance_count;

pub use gpu::{GpuContext, RenderError, create_buffer_init};
pub use headless::{GpuFrameTiming, HeadlessRenderer, RgbaImage};
pub use map_gpu::MapGpu;
pub use pipeline::{CameraUniform, MapPipeline};
pub use scene_gpu::{GpuBatch, GpuScene, UploadStats};
