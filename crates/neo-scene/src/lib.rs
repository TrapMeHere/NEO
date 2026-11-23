//! # neo-scene — 场景装配层
//!
//! 输入：只读 [`neo_store::DataSet`] + [`neo_geo::SpatialIndex`] + 相机 + 样式；
//! 输出：[`batches::SceneFrame`]（顶点/索引/颜色批次）。
//!
//! 这一层**不依赖 GPU、不依赖 UI**，因此可以：
//!
//! * 在无 GPU 的 CI 里单测；
//! * 被基准直接测（场景构建耗时是 CPU 侧最大的一块）；
//! * 被未来的其他后端（SVG/PDF 导出、headless 渲染）复用。
//!
//! 增量策略：几何只在"数据变化 / 相机移动出重基阈值"时重建；平移缩放靠 GPU uniform。
//! 详见 docs/05 与 ADR-0009。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod batches;
pub mod builder;
pub mod cull;
pub mod lod;
pub mod parallel;

pub use batches::{
    AreaBand, BatchKind, LabelPlace, MapVertex, NAME_COLOR, RenderBatch, SceneFrame, SceneIcon, SceneLabel,
    SceneStats, name_color,
};
pub use builder::{
    NODE_ALL_ZOOM, NODE_MIN_ZOOM, SHARED_NODE_MIN_WAYS, SceneBuilder, SceneConfig, node_visible_at,
};
pub use cull::instance_count;
pub use lod::{
    LodBand, StreamingScene, TileCache, TileKey, apply_band, camera_tile, lod_band, tile_key, way_stride,
};
pub use parallel::{build_parallel, build_serial};
