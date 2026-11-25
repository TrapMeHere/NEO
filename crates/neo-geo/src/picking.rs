//! Hit testing (picking).
//!
//! Algorithm (constant cost per click, not linear in the total object count):
//!
//! ```text
//! screen (px)
//!   │ Camera::screen_to_world
//!   ▼
//! world point  ──radius_m = radius_px / px_per_meter──►  expanded bbox
//!   │ SpatialIndex::query(bbox)
//!   ▼
//! candidate entries (usually single digits)
//!     │ exact distance: node=point distance; way=point-to-polyline; compared in screen pixels
//!   ▼
//! Hit (node takes priority over way; nearest wins at equal distance)
//! ```
//!
//! Note: distance must be measured in **screen pixels**, not world meters — because an OSM editor's tolerance semantics
//! are "N pixels around the mouse", which vary with zoom (see docs/04).

use neo_core::{Handle, ObjectId, ObjectKind};
use neo_store::{DataSet, NodeSlot, WaySlot};

use crate::camera::Camera;
use crate::geometry;
use crate::spatial::SpatialIndex;
use crate::world::WorldPoint;

/// Pick result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Object kind.
    pub kind: ObjectKind,
    /// Object ID.
    pub id: ObjectId,
    /// Node handle (if a node was hit).
    pub node: Option<Handle<NodeSlot>>,
    /// Way handle (if a way was hit).
    pub way: Option<Handle<WaySlot>>,
    /// Screen-pixel distance.
    pub distance_px: f32,
}

impl Hit {
    /// Whether a node was hit.
    #[inline]
    pub fn is_node(&self) -> bool {
        self.kind == ObjectKind::Node
    }

    /// Whether a way was hit.
    #[inline]
    pub fn is_way(&self) -> bool {
        self.kind == ObjectKind::Way
    }
}

/// Hit tester.
pub struct HitTester<'a> {
    data: &'a DataSet,
    index: &'a SpatialIndex,
}

impl<'a> HitTester<'a> {
    /// Construct.
    pub fn new(data: &'a DataSet, index: &'a SpatialIndex) -> Self {
        Self { data, index }
    }

    /// Pick a node.
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

    /// Pick a way (nearest distance from the point to the polyline).
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

    /// Pick: nodes first (OSM editing habit: movable points take priority over lines).
    pub fn pick(&self, camera: &Camera, screen: [f32; 2], radius_px: f32) -> Option<Hit> {
        self.pick_node(camera, screen, radius_px)
            .or_else(|| self.pick_way(camera, screen, radius_px))
    }

    /// Pick using per-element-type "selectable margins".
    ///
    /// Rules (consistent with OSM editing habits):
    /// * if the mouse is within the node margin → select the **node** (even if it lies on a way, the way is not wrongly selected);
    /// * otherwise, within the way margin → select the way;
    /// * the two element types use **mutually independent** margins (nodes are usually larger because point targets are small).
    ///
    /// Thus a "margin" is both the draggable range and the selectable range, and the margins of the two element types do not swallow each other.
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

    /// Margin picking with node filtering: nodes for which `accept_node` returns false (e.g. invisible by LOD) are ignored,
    /// and the pick **falls back to the way** — otherwise hidden nodes would "intercept" clicks on the way beneath them, making the feature itself unselectable.
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

    /// Pick all candidates (ascending by distance; for multi-select/cycling selection).
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
