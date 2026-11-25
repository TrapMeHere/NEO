//! # neo-render — wgpu rendering backend
//!
//! Responsible only for the GPU: device/queue, pipelines, vertex/index buffers, drawing, offscreen rendering and readback.
//! **Does not depend on** store/editor/ui (layering see ADR-0004); egui integration is implemented in the application layer
//! (`apps/neo-desktop` implements `egui_wgpu::CallbackTrait` and uses this crate's
//! [`MapGpu`] as the paint callback resource).
//!
//! Key design (docs/05):
//!
//! * Vertex coordinates are **f32 meters relative to the batch origin**; pan/zoom only changes a 32-byte uniform and never re-uploads geometry;
//! * Batches are re-uploaded only when data changes (decided by `SceneFrame::version` + `origin`);
//! * One draw call covers one batch (one each for Fill / Stroke / Point), instead of one per object;
//! * Offscreen rendering + PNG readback is used for windowless verification (CI/benchmarks/screenshots).
//!
//! wgpu 30's API differs a lot from older versions (`PollType`, `CurrentSurfaceTexture`,
//! `multiview_mask`, `entry_point: Option<&str>`, etc.); the code is written for 30.0.2.

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
