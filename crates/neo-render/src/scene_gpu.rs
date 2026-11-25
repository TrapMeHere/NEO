//! GPU-side batch cache: uploads [`neo_scene::SceneFrame`] into vertex/index buffers.
//!
//! Incremental upload strategy:
//!
//! * Compare `SceneFrame::version` with `origin`: if the data and origin are unchanged, **skip the upload entirely**;
//! * One buffer per batch (Fill/Stroke/Point at most 3 draw calls), no per-object buffers;
//! * Later this can be replaced by a "persistent buffer pool + dirty-range `write_buffer`"; the interface is already designed for it (`UploadStats`).

use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use neo_geo::WorldPoint;
use neo_scene::SceneFrame;
use wgpu::util::DeviceExt as _;

/// Parameter layout of `draw_indexed_indirect` (matches wgpu), padded at the end to 32 bytes for storage binding.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct IndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
    _pad: [u32; 3],
}

const _: () = assert!(core::mem::size_of::<IndirectArgs>() == 32);

use crate::gpu::RenderError;

/// GPU resources of one batch.
pub struct GpuBatch {
    /// Vertex buffer.
    pub vertex_buffer: wgpu::Buffer,
    /// Index buffer.
    pub index_buffer: wgpu::Buffer,
    /// Index count.
    pub index_count: u32,
    /// Batch type.
    pub kind: neo_scene::BatchKind,
    /// `draw_indexed_indirect` parameters (20-byte params + padding). `instance_count = 1` on upload.
    pub indirect: wgpu::Buffer,
}

/// Upload stats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadStats {
    /// Bytes uploaded this time.
    pub bytes: u64,
    /// Upload duration (microseconds).
    pub micros: u64,
    /// Whether an upload actually happened (false means the cache was reused).
    pub uploaded: bool,
    /// Number of draw calls.
    pub draw_calls: u32,
}

/// GPU-side scene cache.
#[derive(Default)]
pub struct GpuScene {
    /// Uploaded data version.
    pub version: u64,
    /// Coordinate-system origin of the uploaded batches.
    pub origin: Option<WorldPoint>,
    /// Camera zoom of the uploaded batches (LOD-related; must re-upload after a zoom-triggered rebuild).
    pub zoom: Option<f64>,
    /// Batches.
    pub batches: Vec<GpuBatch>,
    /// Cumulative/most recent upload stats.
    pub stats: UploadStats,
    /// When true, `draw` uses indirect draw instead of `draw_indexed`.
    pub indirect: bool,
}

impl GpuScene {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a re-upload is needed.
    pub fn needs_upload(&self, scene: &SceneFrame) -> bool {
        let Some(origin) = self.origin else {
            return true;
        };
        let Some(zoom) = self.zoom else {
            // Origin present but zoom missing: the frame cannot be proven valid, so it must be uploaded.
            return true;
        };
        self.version != scene.version
            || (origin.x - scene.origin.x).abs() > 1e-9
            || (origin.y - scene.origin.y).abs() > 1e-9
            || (zoom - scene.camera.zoom).abs() > 1e-6
    }

    /// Upload the scene (when needed).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        scene: &SceneFrame,
        camera_origin: WorldPoint,
    ) -> UploadStats {
        #[cfg(feature = "profile")]
        puffin::profile_function!();
        if !self.needs_upload(scene) {
            self.stats.uploaded = false;
            self.stats.bytes = 0;
            self.stats.micros = 0;
            self.stats.draw_calls = self.batches.len() as u32;
            return self.stats;
        }

        let started = Instant::now();
        let mut bytes = 0u64;
        let mut batches = Vec::with_capacity(scene.batches.len());

        for batch in &scene.batches {
            if batch.vertices.is_empty() || batch.indices.is_empty() {
                continue;
            }
            let vertex_bytes = bytemuck::cast_slice::<_, u8>(&batch.vertices);
            let index_bytes = bytemuck::cast_slice::<_, u8>(&batch.indices);

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("neo-map-vertices"),
                contents: vertex_bytes,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("neo-map-indices"),
                contents: index_bytes,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            });
            let index_count = batch.indices.len() as u32;
            let indirect_args = IndirectArgs {
                index_count,
                instance_count: 1,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0,
                _pad: [0; 3],
            };
            let indirect = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("neo-map-indirect"),
                contents: bytemuck::bytes_of(&indirect_args),
                usage: wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
            });
            bytes += vertex_bytes.len() as u64 + index_bytes.len() as u64 + 32;
            batches.push(GpuBatch {
                vertex_buffer,
                index_buffer,
                index_count,
                kind: batch.kind,
                indirect,
            });
        }

        self.batches = batches;
        self.version = scene.version;
        self.origin = Some(camera_origin);
        self.zoom = Some(scene.camera.zoom);
        let draw_calls = self.batches.len() as u32;
        self.stats = UploadStats {
            bytes,
            micros: started.elapsed().as_micros() as u64,
            uploaded: true,
            draw_calls,
        };
        self.stats
    }

    /// Issue all draw calls inside the render pass.
    ///
    /// With `indirect == false`, call `draw_indexed` directly (the Phase 1–4 path).
    /// With `indirect == true`, go through `draw_indexed_indirect`; the instance count comes from the indirect buffer,
    /// and GPU culling can write `instance_count` to 0 to drop the whole batch.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        for batch in &self.batches {
            pass.set_vertex_buffer(0, batch.vertex_buffer.slice(..));
            pass.set_index_buffer(batch.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            if self.indirect {
                pass.draw_indexed_indirect(&batch.indirect, 0);
            } else {
                pass.draw_indexed(0..batch.index_count, 0, 0..1);
            }
        }
    }

    /// Statistics.
    pub fn upload_stats(&self) -> UploadStats {
        self.stats
    }

    /// Check for tests/benchmarks: ensure buffer sizes and contents are consistent.
    pub fn validate(&self) -> Result<(), RenderError> {
        for batch in &self.batches {
            let vertex_size = batch.vertex_buffer.size();
            if vertex_size % core::mem::size_of::<neo_scene::MapVertex>() as u64 != 0 {
                return Err(RenderError::Backend(
                    "vertex buffer size is not a multiple of MapVertex size".to_owned(),
                ));
            }
            if batch.index_count as u64 * 4 != batch.index_buffer.size() {
                return Err(RenderError::Backend(
                    "index buffer size does not match index count".to_owned(),
                ));
            }
        }
        Ok(())
    }
}
