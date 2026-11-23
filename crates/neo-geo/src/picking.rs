//! 命中测试（拾取）。
//!
//! 算法（每次点击的固定成本，不随对象总数线性增长）：
//!
//! ```text
//! screen (px)
//!   │ Camera::screen_to_world
//!   ▼
//! world point  ──radius_m = radius_px / px_per_meter──►  放大后的 bbox
//!   │ SpatialIndex::query(bbox)
//!   ▼
//! 候选 entries（通常个位数）
//!   │ 精确距离：node=点距；way=点到折线；按屏幕像素比较
//!   ▼
//! Hit（node 优先于 way，同距离取最近）
//! ```
//!
//! 注意：距离判定必须在**屏幕像素**里做，而不是世界米——因为 OSM 编辑器的容差语义
//! 是"鼠标周围 N 像素"，它随 zoom 变化（见 docs/04）。

use neo_core::{Handle, ObjectId, ObjectKind};
use neo_store::{DataSet, NodeSlot, WaySlot};

use crate::camera::Camera;
use crate::geometry;
use crate::spatial::SpatialIndex;
use crate::world::WorldPoint;

/// 拾取结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 对象 ID。
    pub id: ObjectId,
    /// 节点句柄（若命中节点）。
    pub node: Option<Handle<NodeSlot>>,
    /// Way 句柄（若命中 way）。
    pub way: Option<Handle<WaySlot>>,
    /// 屏幕像素距离。
    pub distance_px: f32,
}

impl Hit {
    /// 是否命中节点。
    #[inline]
    pub fn is_node(&self) -> bool {
        self.kind == ObjectKind::Node
    }

    /// 是否命中 way。
    #[inline]
    pub fn is_way(&self) -> bool {
        self.kind == ObjectKind::Way
    }
}

/// 命中测试器。
pub struct HitTester<'a> {
    data: &'a DataSet,
    index: &'a SpatialIndex,
}

impl<'a> HitTester<'a> {
    /// 构造。
    pub fn new(data: &'a DataSet, index: &'a SpatialIndex) -> Self {
        Self { data, index }
    }

    /// 拾取节点。
    pub fn pick_node(&self, camera: &Camera, screen: [f32; 2], radius_px: f32) -> Option<Hit> {
        let world = camera.screen_to_world(screen);
        let radius_m = camera.px_to_meters(radius_px);
        let bbox = crate::world::WorldBounds::from_point(world).expanded_by(radius_m);
        let ppm = camera.px_per_meter();

        let mut best: Option<Hit> = None;
        for entry in self.index.query_nodes(bbox) {
            let Some(handle) = entry.as_node() else { continue };
            let Some(pos) = self.data.nodes.position(handle) else {
                continue;
            };
            let wp = WorldPoint::from_latlon(pos);
            let distance_px = (world.dist(wp) * ppm) as f32;
            if distance_px > radius_px {
                continue;
            }
            let id = self.data.nodes.id(handle).unwrap_or(ObjectId::NONE);
            let candidate = Hit {
                kind: ObjectKind::Node,
                id,
                node: Some(handle),
                way: None,
                distance_px,
            };
            if best
                .map(|b| candidate.distance_px < b.distance_px)
                .unwrap_or(true)
            {
                best = Some(candidate);
            }
        }
        best
    }

    /// 拾取 way（点在折线上的最近距离）。
    pub fn pick_way(&self, camera: &Camera, screen: [f32; 2], radius_px: f32) -> Option<Hit> {
        let world = camera.screen_to_world(screen);
        let radius_m = camera.px_to_meters(radius_px);
        let bbox = crate::world::WorldBounds::from_point(world).expanded_by(radius_m);
        let ppm = camera.px_per_meter();

        let mut best: Option<Hit> = None;
        for entry in self.index.query_ways(bbox) {
            let Some(handle) = entry.as_way() else { continue };
            let points = geometry::way_world_points(self.data, handle);
            if points.is_empty() {
                continue;
            }
            let distance_px = (geometry::point_polyline_distance(world, &points) * ppm) as f32;
            if distance_px > radius_px {
                continue;
            }
            let id = self.data.ways.id(handle).unwrap_or(ObjectId::NONE);
            let candidate = Hit {
                kind: ObjectKind::Way,
                id,
                node: None,
                way: Some(handle),
                distance_px,
            };
            if best
                .map(|b| candidate.distance_px < b.distance_px)
                .unwrap_or(true)
            {
                best = Some(candidate);
            }
        }
        best
    }

    /// 拾取：节点优先（OSM 编辑习惯：可移动点优先级高于线）。
    pub fn pick(&self, camera: &Camera, screen: [f32; 2], radius_px: f32) -> Option<Hit> {
        self.pick_node(camera, screen, radius_px)
            .or_else(|| self.pick_way(camera, screen, radius_px))
    }

    /// 按不同元素类型各自的"可选择外边距"拾取。
    ///
    /// 规则（与 OSM 编辑习惯一致）：
    /// * 鼠标在节点外边距内 → 选**节点**（即使它压在 way 上，也不会误选 way）；
    /// * 否则在 way 外边距内 → 选 way；
    /// * 两类元素使用**互相独立**的边距（节点通常更大，因为点目标小）。
    ///
    /// 这样"外边距"既是可拖动范围，也是可选择范围，而两种元素的边距不会互相吞并。
    pub fn pick_with_margins(
        &self,
        camera: &Camera,
        screen: [f32; 2],
        node_margin_px: f32,
        way_margin_px: f32,
    ) -> Option<Hit> {
        self.pick_node(camera, screen, node_margin_px)
            .or_else(|| self.pick_way(camera, screen, way_margin_px))
    }

    /// 带节点过滤的外边距拾取：`accept_node` 返回 false 的节点（如按 LOD 不可见）会被忽略，
    /// 并**回退到 way**——否则隐藏节点会"截胡"其下方 way 的点击，导致点击要素本身无法选中。
    pub fn pick_with_margins_filtered(
        &self,
        camera: &Camera,
        screen: [f32; 2],
        node_margin_px: f32,
        way_margin_px: f32,
        accept_node: impl Fn(Handle<NodeSlot>) -> bool,
    ) -> Option<Hit> {
        let node = self
            .pick_node(camera, screen, node_margin_px)
            .filter(|hit| hit.node.map(accept_node).unwrap_or(false));
        node.or_else(|| self.pick_way(camera, screen, way_margin_px))
    }

    /// 拾取全部候选（按距离升序，供多选/循环选择）。
    pub fn pick_all(&self, camera: &Camera, screen: [f32; 2], radius_px: f32) -> Vec<Hit> {
        let world = camera.screen_to_world(screen);
        let radius_m = camera.px_to_meters(radius_px);
        let bbox = crate::world::WorldBounds::from_point(world).expanded_by(radius_m);
        let ppm = camera.px_per_meter();
        let mut hits = Vec::new();

        for entry in self.index.query(bbox) {
            let distance_px = match entry.kind {
                ObjectKind::Node => {
                    let Some(handle) = entry.as_node() else { continue };
                    let Some(pos) = self.data.nodes.position(handle) else {
                        continue;
                    };
                    (world.dist(WorldPoint::from_latlon(pos)) * ppm) as f32
                }
                ObjectKind::Way => {
                    let Some(handle) = entry.as_way() else { continue };
                    let points = geometry::way_world_points(self.data, handle);
                    (geometry::point_polyline_distance(world, &points) * ppm) as f32
                }
                _ => continue,
            };
            if distance_px > radius_px {
                continue;
            }
            let id = match entry.kind {
                ObjectKind::Node => entry.as_node().and_then(|h| self.data.nodes.id(h)),
                ObjectKind::Way => entry.as_way().and_then(|h| self.data.ways.id(h)),
                _ => None,
            };
            hits.push(Hit {
                kind: entry.kind,
                id: id.unwrap_or(ObjectId::NONE),
                node: entry.as_node(),
                way: entry.as_way(),
                distance_px,
            });
        }
        hits.sort_by(|a, b| a.distance_px.total_cmp(&b.distance_px));
        hits
    }
}
