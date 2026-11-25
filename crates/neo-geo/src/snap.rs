//! Drawing snapping: nodes first, then segment projection.

use neo_core::{Handle, LatLon, ObjectId};
use neo_store::{DataSet, WaySlot};

use crate::camera::Camera;
use crate::geometry::{self, point_segment_closest};
use crate::picking::HitTester;
use crate::spatial::SpatialIndex;
use crate::world::WorldPoint;

/// Snap target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapTarget {
    /// Reuse an existing node.
    Node {
        /// Node ID.
        id: ObjectId,
    },
    /// Projection point on a way segment (a shared node can then be inserted).
    WaySegment {
        /// Way ID.
        way: ObjectId,
        /// Index of the segment start in the node sequence (the projection point is inserted at `seg + 1`).
        seg: usize,
        /// Projected lon/lat.
        at: LatLon,
    },
}

/// One snap hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    /// Target.
    pub target: SnapTarget,
    /// Screen-pixel distance.
    pub screen_dist: f32,
}

impl SnapHit {
    /// Snapped world coordinates (nodes use the node position; segments use the projection point).
    pub fn world_point(&self, data: &DataSet) -> Option<WorldPoint> {
        match self.target {
            SnapTarget::Node { id } => data
                .node_handle(id)
                .and_then(|handle| data.nodes.position(handle))
                .map(WorldPoint::from_latlon),
            SnapTarget::WaySegment { at, .. } => Some(WorldPoint::from_latlon(at)),
        }
    }

    /// Snapped lon/lat.
    pub fn latlon(&self, data: &DataSet) -> Option<LatLon> {
        match self.target {
            SnapTarget::Node { id } => data
                .node_handle(id)
                .and_then(|handle| data.nodes.position(handle)),
            SnapTarget::WaySegment { at, .. } => Some(at),
        }
    }
}

/// Resolve snapping: nodes take priority over segments; at equal distance the nearer one wins.
///
/// `accept_node` / `accept_way` are used to skip hidden objects.
pub fn resolve_snap(
    data: &DataSet,
    index: &SpatialIndex,
    camera: &Camera,
    screen: [f32; 2],
    radius_px: f32,
    accept_node: impl Fn(ObjectId) -> bool,
    accept_way: impl Fn(ObjectId) -> bool,
) -> Option<SnapHit> {
    if radius_px <= 0.0 {
        return None;
    }
    let tester = HitTester::new(data, index);
    if let Some(hit) = tester.pick_node(camera, screen, radius_px) {
        let id = hit
            .node
            .and_then(|handle| data.nodes.id(handle))
            .unwrap_or(hit.id);
        if id != ObjectId::NONE && accept_node(id) {
            return Some(SnapHit {
                target: SnapTarget::Node { id },
                screen_dist: hit.distance_px,
            });
        }
    }
    resolve_segment_snap(data, index, camera, screen, radius_px, accept_way)
}

fn resolve_segment_snap(
    data: &DataSet,
    index: &SpatialIndex,
    camera: &Camera,
    screen: [f32; 2],
    radius_px: f32,
    accept_way: impl Fn(ObjectId) -> bool,
) -> Option<SnapHit> {
    let world = camera.screen_to_world(screen);
    let radius_m = camera.px_to_meters(radius_px);
    let bbox = crate::world::WorldBounds::from_point(world).expanded_by(radius_m);
    let ppm = camera.px_per_meter();

    let mut best: Option<(f32, SnapTarget)> = None;
    for entry in index.query_ways(bbox) {
        let Some(handle) = entry.as_way() else {
            continue;
        };
        let id = data.ways.id(handle).unwrap_or(ObjectId::NONE);
        if id == ObjectId::NONE || !accept_way(id) {
            continue;
        }
        let Some(node_handles) = data.ways.nodes(handle) else {
            continue;
        };
        if node_handles.len() < 2 {
            continue;
        }
        for seg in 0..node_handles.len() - 1 {
            let Some(pa) = data.nodes.position(node_handles[seg]) else {
                continue;
            };
            let Some(pb) = data.nodes.position(node_handles[seg + 1]) else {
                continue;
            };
            let a = WorldPoint::from_latlon(pa);
            let b = WorldPoint::from_latlon(pb);
            let (proj, dist_m, t) = point_segment_closest(world, a, b);
            let distance_px = (dist_m * ppm) as f32;
            if distance_px > radius_px {
                continue;
            }
            // When the projection nearly lands on an endpoint, snap to that node instead to avoid inserting a duplicate point.
            const END_EPS: f64 = 1e-6;
            let target = if t <= END_EPS {
                let node_id = data.nodes.id(node_handles[seg]).unwrap_or(ObjectId::NONE);
                if node_id == ObjectId::NONE {
                    continue;
                }
                SnapTarget::Node { id: node_id }
            } else if t >= 1.0 - END_EPS {
                let node_id = data.nodes.id(node_handles[seg + 1]).unwrap_or(ObjectId::NONE);
                if node_id == ObjectId::NONE {
                    continue;
                }
                SnapTarget::Node { id: node_id }
            } else {
                SnapTarget::WaySegment {
                    way: id,
                    seg,
                    at: proj.to_latlon(),
                }
            };
            if best.as_ref().map(|(d, _)| distance_px < *d).unwrap_or(true) {
                best = Some((distance_px, target));
            }
        }
    }
    best.map(|(screen_dist, target)| SnapHit { target, screen_dist })
}

/// Nearest segment index and projection on a polyline (for unit tests/tool reuse).
pub fn closest_way_segment(
    data: &DataSet,
    way: Handle<WaySlot>,
    world: WorldPoint,
) -> Option<(usize, LatLon, f64)> {
    let points = geometry::way_world_points(data, way);
    if points.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, LatLon, f64)> = None;
    for (seg, pair) in points.windows(2).enumerate() {
        let (proj, dist, _) = point_segment_closest(world, pair[0], pair[1]);
        if best.as_ref().map(|(_, _, d)| dist < *d).unwrap_or(true) {
            best = Some((seg, proj.to_latlon(), dist));
        }
    }
    best
}

/// Convenience wrapper for callers with no filters.
pub fn resolve_snap_unfiltered(
    data: &DataSet,
    index: &SpatialIndex,
    camera: &Camera,
    screen: [f32; 2],
    radius_px: f32,
) -> Option<SnapHit> {
    resolve_snap(data, index, camera, screen, radius_px, |_| true, |_| true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::spatial::SpatialIndex;
    use crate::world::WorldPoint;
    use neo_core::{LatLon, Meta, ObjectId};
    use neo_osm::{Node, Tags, Way};
    use neo_store::{DataSet, MissingPolicy};

    fn latlon(lat: f64, lon: f64) -> LatLon {
        LatLon::from_degrees(lat, lon)
    }

    fn sample_line() -> (DataSet, SpatialIndex, ObjectId, ObjectId) {
        let mut data = DataSet::new();
        let n1 = ObjectId(1);
        let n2 = ObjectId(2);
        let w = ObjectId(10);
        data.push_node(Node {
            id: n1,
            pos: latlon(0.0, 0.0),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
        data.push_node(Node {
            id: n2,
            pos: latlon(0.0, 0.001),
            tags: Tags::new(),
            meta: Meta::new_local(),
        })
        .unwrap();
        data.push_way(
            Way {
                id: w,
                nodes: vec![n1, n2].into(),
                tags: Tags::new(),
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )
        .unwrap();
        let index = SpatialIndex::build(&data);
        (data, index, n1, w)
    }

    #[test]
    fn segment_snap_projects_within_radius() {
        let (data, index, _, w) = sample_line();
        let mid = WorldPoint::from_latlon(latlon(0.0, 0.0005));
        let camera = Camera::new(mid, 18.0, [800, 600]);
        let screen = camera.world_to_screen(mid);
        // Shift slightly to one side of the segment to avoid snapping to the endpoint node.
        let offset_screen = [screen[0], screen[1] + 4.0];
        let hit = resolve_snap_unfiltered(&data, &index, &camera, offset_screen, 16.0).expect("should snap");
        match hit.target {
            SnapTarget::WaySegment { way, seg, .. } => {
                assert_eq!(way, w);
                assert_eq!(seg, 0);
            }
            SnapTarget::Node { .. } => panic!("expected segment snap away from endpoints"),
        }
    }

    #[test]
    fn node_snap_beats_segment() {
        let (data, index, n1, _) = sample_line();
        let camera = Camera::new(WorldPoint::from_latlon(latlon(0.0, 0.0)), 18.0, [800, 600]);
        let screen = camera.world_to_screen(WorldPoint::from_latlon(latlon(0.0, 0.0)));
        let hit = resolve_snap_unfiltered(&data, &index, &camera, screen, 16.0).expect("snap");
        assert!(matches!(hit.target, SnapTarget::Node { id } if id == n1));
    }

    #[test]
    fn radius_zero_disables_snap() {
        let data = DataSet::new();
        let index = SpatialIndex::new();
        let camera = Camera::default();
        assert!(resolve_snap_unfiltered(&data, &index, &camera, [0.0, 0.0], 0.0).is_none());
    }
}
