//! GPU 侧的批次缓存：把 [`neo_scene::SceneFrame`] 上传为顶点/索引 buffer。
//!
//! 增量上传策略：
//!
//! * 比较 `SceneFrame::version` 与 `origin`：数据没变且原点没变就**完全跳过上传**；
//! * 一个批次一个 buffer（Fill/Stroke/Point 最多 3 个 draw call），不做 per-object buffer；
//! * 后续可换成"持久 buffer 池 + 脏区间 `write_buffer`"，接口已按此设计（`UploadStats`）。

use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use neo_geo::WorldPoint;
use neo_scene::SceneFrame;
use wgpu::util::DeviceExt as _;

/// `draw_indexed_indirect` 的参数布局（与 wgpu 一致），尾部填充到 32 字节以便 storage 绑定。
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

/// 一个批次的 GPU 资源。
pub struct GpuBatch {
    /// 顶点 buffer。
    pub vertex_buffer: wgpu::Buffer,
    /// 索引 buffer。
    pub index_buffer: wgpu::Buffer,
    /// 索引数量。
    pub index_count: u32,
    /// 批次类型。
    pub kind: neo_scene::BatchKind,
    /// `draw_indexed_indirect` 参数（20 字节参数 + 填充）。上传时 `instance_count = 1`。
    pub indirect: wgpu::Buffer,
}

/// 上传统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UploadStats {
    /// 本次上传字节数。
    pub bytes: u64,
    /// 上传耗时（微秒）。
    pub micros: u64,
    /// 是否真的上传（false 表示复用缓存）。
    pub uploaded: bool,
    /// draw call 数。
    pub draw_calls: u32,
}

/// GPU 侧场景缓存。
#[derive(Default)]
pub struct GpuScene {
    /// 已上传的数据版本。
    pub version: u64,
    /// 已上传批次的坐标系原点。
    pub origin: Option<WorldPoint>,
    /// 已上传批次的相机 zoom（LOD 相关，缩放重建后必须重传）。
    pub zoom: Option<f64>,
    /// 批次。
    pub batches: Vec<GpuBatch>,
    /// 累计/最近一次上传统计。
    pub stats: UploadStats,
    /// 为 true 时 `draw` 走 indirect draw，而不是 `draw_indexed`。
    pub indirect: bool,
}

impl GpuScene {
    /// 空缓存。
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否需要重新上传。
    pub fn needs_upload(&self, scene: &SceneFrame) -> bool {
        let Some(origin) = self.origin else {
            return true;
        };
        let Some(zoom) = self.zoom else {
            // 原点存在但 zoom 缺失：无法证明画面仍然有效，必须上传。
            return true;
        };
        self.version != scene.version
            || (origin.x - scene.origin.x).abs() > 1e-9
            || (origin.y - scene.origin.y).abs() > 1e-9
            || (zoom - scene.camera.zoom).abs() > 1e-6
    }

    /// 上传场景（需要时）。
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

    /// 在 render pass 中发出所有 draw call。
    ///
    /// `indirect == false` 时直接 `draw_indexed`（阶段 1–4 的路径）。
    /// `indirect == true` 时走 `draw_indexed_indirect`，实例数由 indirect buffer 决定，
    /// GPU culling 可以把 `instance_count` 写成 0 来丢掉整批。
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

    /// 统计数据。
    pub fn upload_stats(&self) -> UploadStats {
        self.stats
    }

    /// 供测试/基准的校验：确保 buffer 大小与内容一致。
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
