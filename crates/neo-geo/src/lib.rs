//! # neo-geo — 空间与几何层
//!
//! 职责：
//!
//! * [`world`]：WebMercator 世界坐标（f64 米）与包围盒；
//! * [`camera`]：世界坐标 ↔ 屏幕像素，缩放/平移/适配；
//! * [`spatial`]：rstar R-tree 对象索引（节点点 + way 的 AABB），支持增量更新；
//! * [`geometry`]：折线/多边形三角化/描边生成/简化/距离；
//! * [`picking`]：屏幕像素容差的命中测试（不遍历全部对象）。
//!
//! 依赖方向：`neo-core`、`neo-osm`、`neo-store`（只读数据集）→ 本 crate → `neo-scene`。
//! 本 crate 不依赖 GPU/UI/IO。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod camera;
pub mod geometry;
pub mod picking;
pub mod snap;
pub mod spatial;
pub mod world;
pub mod world_cache;

pub use camera::{Camera, MAX_ZOOM, MIN_ZOOM, REBASE_THRESHOLD_M, TILE_SIZE};
pub use geometry::{BoxSelectMode, box_select_mode, segment_intersects_bounds, way_matches_box};
pub use picking::{Hit, HitTester};
pub use snap::{SnapHit, SnapTarget, closest_way_segment, resolve_snap, resolve_snap_unfiltered};
pub use spatial::{SpatialEntry, SpatialIndex};
pub use world::{
    EARTH_CIRCUMFERENCE_M, EARTH_RADIUS_M, MAX_MERCATOR_LAT, WorldBounds, WorldPoint, distance_meters,
};
pub use world_cache::WorldCache;
