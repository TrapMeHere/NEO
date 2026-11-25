//! Geometry construction: Way → world-coordinate polyline → triangle batches.
//!
//! Clear responsibility boundaries:
//!
//! * this module converts lon/lat polylines into world coordinates, simplifies them, generates stroke triangles, and
//!   triangulates polygons, all on the **CPU**;
//! * it does no screen transform (that is the camera's job) and never touches GPU buffers (that is `neo-render`'s job);
//! * results are expressed as `[f32; 2]` meter coordinates + `u32` indices, ready for zero-copy upload.
//!
//! Why not generate geometry on the GPU: OSM editing is "few objects, high-frequency modification"; CPU incremental rebuild + partial upload
//! makes correctness and maintainability easier to guarantee than geometry shaders on the GPU (see the workload split table in docs/05).

use neo_core::Handle;
use neo_store::{DataSet, WaySlot};

use crate::world::{WorldBounds, WorldPoint};

/// Parse one way into a world-coordinate polyline (skipping invalid/missing nodes).
pub fn way_world_points(data: &DataSet, way: Handle<WaySlot>) -> Vec<WorldPoint> {
    let Some(refs) = data.ways.nodes(way) else {
        return Vec::new();
    };
    refs.iter()
        .filter_map(|h| data.nodes.position(*h))
        .map(WorldPoint::from_latlon)
        .collect()
}

/// World-coordinate bounding box of one way.
pub fn way_bounds(data: &DataSet, way: Handle<WaySlot>) -> Option<WorldBounds> {
    let points = way_world_points(data, way);
    if points.is_empty() {
        return None;
    }
    Some(WorldBounds::from_points_iter(points))
}

/// CAD / JOSM style rectangle-selection mode (determined by the horizontal drag direction).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSelectMode {
    /// Left → right: select only features entirely inside the rectangle (window select).
    Window,
    /// Right → left: select features intersecting the rectangle or entirely inside it (crossing select).
    Crossing,
}

/// Determine the mode from the drag start/end points: `start.x > end.x` is crossing select, otherwise window select.
#[inline]
pub fn box_select_mode(start: WorldPoint, end: WorldPoint) -> BoxSelectMode {
    if start.x > end.x {
        BoxSelectMode::Crossing
    } else {
        BoxSelectMode::Window
    }
}

/// Whether a segment intersects an axis-aligned bounding box (including endpoints inside the box and crossing edges).
pub fn segment_intersects_bounds(a: WorldPoint, b: WorldPoint, bounds: &WorldBounds) -> bool {
    if bounds.contains(a) || bounds.contains(b) {
        return true;
    }
    let seg = WorldBounds::from_points(a, b);
    if !bounds.intersects(&seg) {
        return false;
    }
    let c0 = WorldPoint::new(bounds.min.x, bounds.min.y);
    let c1 = WorldPoint::new(bounds.max.x, bounds.min.y);
    let c2 = WorldPoint::new(bounds.max.x, bounds.max.y);
    let c3 = WorldPoint::new(bounds.min.x, bounds.max.y);
    segments_cross(a, b, c0, c1)
        || segments_cross(a, b, c1, c2)
        || segments_cross(a, b, c2, c3)
        || segments_cross(a, b, c3, c0)
}

/// Whether a polyline (way) matches the selection mode.
///
/// * window select: all vertices are inside the box (an AABB is convex, so the whole polyline is inside);
/// * crossing select: any point is inside the box, or any segment intersects the box.
pub fn way_matches_box(points: &[WorldPoint], bounds: &WorldBounds, mode: BoxSelectMode) -> bool {
    if points.is_empty() {
        return false;
    }
    match mode {
        BoxSelectMode::Window => points.iter().all(|p| bounds.contains(*p)),
        BoxSelectMode::Crossing => {
            if points.iter().any(|p| bounds.contains(*p)) {
                return true;
            }
            points
                .windows(2)
                .any(|w| segment_intersects_bounds(w[0], w[1], bounds))
        }
    }
}

/// Polyline length (world-coordinate meters; used only for LOD and simplification tolerance).
pub fn polyline_length(points: &[WorldPoint]) -> f64 {
    points.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// Nearest point and distance from a point to a segment (f64 meters). Returns `(projected point, distance, t∈[0,1])`.
pub fn point_segment_closest(p: WorldPoint, a: WorldPoint, b: WorldPoint) -> (WorldPoint, f64, f64) {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let (apx, apy) = (p.x - a.x, p.y - a.y);
    let len_sq = abx * abx + aby * aby;
    if len_sq <= f64::EPSILON {
        return (a, p.dist(a), 0.0);
    }
    let t = ((apx * abx + apy * aby) / len_sq).clamp(0.0, 1.0);
    let proj = WorldPoint::new(a.x + abx * t, a.y + aby * t);
    (proj, p.dist(proj), t)
}

/// Shortest distance from a point to a segment (f64 meters).
pub fn point_segment_distance(p: WorldPoint, a: WorldPoint, b: WorldPoint) -> f64 {
    point_segment_closest(p, a, b).1
}

/// Shortest distance from a point to a polyline (meters).
pub fn point_polyline_distance(p: WorldPoint, points: &[WorldPoint]) -> f64 {
    if points.is_empty() {
        return f64::INFINITY;
    }
    if points.len() == 1 {
        return p.dist(points[0]);
    }
    points
        .windows(2)
        .map(|w| point_segment_distance(p, w[0], w[1]))
        .fold(f64::INFINITY, f64::min)
}

/// Ramer–Douglas–Peucker polyline simplification.
///
/// `tolerance_m` is the maximum allowed deviation (meters). Used by both LOD and render simplification.
pub fn simplify_rdp(points: &[WorldPoint], tolerance_m: f64) -> Vec<WorldPoint> {
    let mut out = Vec::new();
    let mut keep = Vec::new();
    simplify_rdp_into(points, tolerance_m, &mut out, &mut keep);
    out
}

/// Triangulate a polygon with holes (earcut). `holes` may be in any point order, but each must be a closed ring (the first/last point may repeat).
pub fn triangulate_polygon_with_holes(outer: &[WorldPoint], holes: &[Vec<WorldPoint>]) -> Vec<u32> {
    if outer.len() < 3 {
        return Vec::new();
    }
    let mut vertices = Vec::with_capacity((outer.len() + holes.iter().map(Vec::len).sum::<usize>()) * 2);
    let mut hole_indices = Vec::with_capacity(holes.len());
    let push_ring = |ring: &[WorldPoint], vertices: &mut Vec<f64>| {
        let end = if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
            ring.len() - 1
        } else {
            ring.len()
        };
        for point in &ring[..end] {
            vertices.push(point.x);
            vertices.push(point.y);
        }
        end
    };
    push_ring(outer, &mut vertices);
    for hole in holes {
        if hole.len() < 3 {
            continue;
        }
        hole_indices.push(vertices.len() / 2);
        push_ring(hole, &mut vertices);
    }
    earcutr::earcut(&vertices, &hole_indices, 2)
        .map(|indices| indices.into_iter().map(|index| index as u32).collect())
        .unwrap_or_default()
}

/// Whether a point is inside a ring (ray casting, approximate at boundaries).
pub fn point_in_ring(point: WorldPoint, ring: &[WorldPoint]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[j];
        if ((a.y > point.y) != (b.y > point.y))
            && (point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Indices kept after RDP simplification (ascending, including first and last).
pub fn simplify_rdp_indices(points: &[WorldPoint], tolerance_m: f64) -> Vec<usize> {
    if points.len() <= 2 {
        return (0..points.len()).collect();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_mark(points, tolerance_m, &mut keep);
    keep.iter()
        .enumerate()
        .filter_map(|(index, keep)| keep.then_some(index))
        .collect()
}

/// Iterative RDP marking (explicit stack): pathological polylines cannot blow the call stack through recursion depth.
fn rdp_mark(points: &[WorldPoint], tolerance_m: f64, keep: &mut [bool]) {
    let mut stack: Vec<(usize, usize)> = vec![(0, points.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        if last <= first + 1 {
            continue;
        }
        let mut max_dist = 0.0;
        let mut index = first;
        for i in (first + 1)..last {
            let d = point_segment_distance(points[i], points[first], points[last]);
            if d > max_dist {
                max_dist = d;
                index = i;
            }
        }
        if max_dist > tolerance_m {
            keep[index] = true;
            stack.push((first, index));
            stack.push((index, last));
        }
    }
}

/// Signed area of a ring (world-coordinate square meters; positive = counterclockwise).
pub fn signed_area(ring: &[WorldPoint]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        sum += a.x * b.y - b.x * a.y;
    }
    sum * 0.5
}

/// Area centroid of a ring. Returns `None` for degenerate rings. A duplicated closing point at the end is ignored.
pub fn ring_centroid(ring: &[WorldPoint]) -> Option<WorldPoint> {
    let ring = strip_closing_point(ring);
    if ring.len() < 3 {
        return None;
    }
    let mut twice = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    let n = ring.len();
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + 1) % n];
        let cross = a.x * b.y - b.x * a.y;
        twice += cross;
        cx += (a.x + b.x) * cross;
        cy += (a.y + b.y) * cross;
    }
    if twice.abs() < 1e-6 {
        return None;
    }
    Some(WorldPoint::new(cx / (3.0 * twice), cy / (3.0 * twice)))
}

/// Anchor point for area labels: prefer the area centroid. When the centroid falls outside the ring or inside a hole, use the centroid of the largest triangle instead.
pub fn area_label_point(outer: &[WorldPoint], holes: &[Vec<WorldPoint>]) -> Option<WorldPoint> {
    let centroid = ring_centroid(outer);
    if let Some(point) = centroid
        && point_in_ring(point, outer)
        && !holes.iter().any(|hole| point_in_ring(point, hole))
    {
        return Some(point);
    }
    let (points, indices) = triangulate_polygon(outer, holes);
    let mut best_area = 0.0;
    let mut best = None;
    for triangle in indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (
            points.get(triangle[0] as usize),
            points.get(triangle[1] as usize),
            points.get(triangle[2] as usize),
        ) else {
            continue;
        };
        let area = ((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)).abs();
        if area > best_area {
            best_area = area;
            best = Some(WorldPoint::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0));
        }
    }
    best.or(centroid)
}

/// Whether the regions of two rings overlap (bounding boxes intersect, a vertex falls inside the other, or an edge crosses).
pub fn rings_overlap(a: &[WorldPoint], b: &[WorldPoint]) -> bool {
    let a = strip_closing_point(a);
    let b = strip_closing_point(b);
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let bounds_a = WorldBounds::from_points_iter(a.iter().copied());
    let bounds_b = WorldBounds::from_points_iter(b.iter().copied());
    if !bounds_a.intersects(&bounds_b) {
        return false;
    }
    if a.iter().any(|point| point_in_ring(*point, b)) || b.iter().any(|point| point_in_ring(*point, a)) {
        return true;
    }
    if a.len() > 80 || b.len() > 80 {
        return false;
    }
    for i in 0..a.len() {
        let a0 = a[i];
        let a1 = a[(i + 1) % a.len()];
        for j in 0..b.len() {
            if segments_cross(a0, a1, b[j], b[(j + 1) % b.len()]) {
                return true;
            }
        }
    }
    false
}

/// Whether a point falls on the inner band of a ring. `inset` is the start from the boundary toward the interior of the enclosed region, and `width` is the band width.
/// Points inside holes do not count. A band of the same width on the hole boundary also counts.
pub fn point_in_inner_band(
    point: WorldPoint,
    outer: &[WorldPoint],
    holes: &[Vec<WorldPoint>],
    inset: f64,
    width: f64,
) -> bool {
    if width <= 0.0 || !point_in_ring(point, outer) || holes.iter().any(|hole| point_in_ring(point, hole)) {
        return false;
    }
    let near = |ring: &[WorldPoint]| {
        let distance = boundary_distance(point, ring);
        distance + width * 0.08 >= inset && distance <= inset + width * 1.08
    };
    near(outer) || holes.iter().any(|hole| near(hole))
}

/// Whether a ring band (the `width`-wide strip inside `outer`) completely avoids all holes.
///
/// When only the band is drawn, band geometry does not subtract holes; therefore the "band + hole-edge band" drawing is safe only when every hole lies entirely inside the band's inner boundary
/// (away from the boundary, so the band does not sweep over it). Otherwise the caller should switch to "solid + hole cutout",
/// otherwise holes would be covered by the band (small inner courtyards would become solid at mid zoom levels).
///
/// Return `true` when the band degenerates (offset fails): the caller then takes the solid path because the band is empty.
pub fn band_clears_holes(outer: &[WorldPoint], holes: &[Vec<WorldPoint>], width: f64) -> bool {
    let base_len = strip_closing_point(outer).len();
    let (vertices, indices) = ring_band(outer, 0.0, width, true);
    if indices.is_empty() || vertices.len() != base_len * 2 {
        return true;
    }
    let inner = &vertices[base_len..];
    holes.iter().all(|hole| {
        let ring = strip_closing_point(hole);
        if ring.len() < 3 {
            return true;
        }
        // Holes must lie entirely inside the inner boundary, and hole edges must not cross outside the inner boundary.
        ring.iter().all(|point| point_in_ring(*point, inner)) && !ring_edges_cross(ring, inner)
    })
}

/// Whether the edges of two rings intersect (used to decide whether a hole crosses outside the band's inner boundary).
fn ring_edges_cross(a: &[WorldPoint], b: &[WorldPoint]) -> bool {
    let na = a.len();
    let nb = b.len();
    if na < 2 || nb < 2 {
        return false;
    }
    for i in 0..na {
        let a0 = a[i];
        let a1 = a[(i + 1) % na];
        for j in 0..nb {
            if segments_cross(a0, a1, b[j], b[(j + 1) % nb]) {
                return true;
            }
        }
    }
    false
}

/// Generate a constant-width band along a ring, inward (or outward from the enclosed region). Returns empty when the offset breaks the ring.
pub fn ring_band(
    ring: &[WorldPoint],
    inset: f64,
    width: f64,
    into_interior: bool,
) -> (Vec<WorldPoint>, Vec<u32>) {
    if width <= 1e-9 {
        return (Vec::new(), Vec::new());
    }
    let source = strip_closing_point(ring);
    if source.len() < 3 {
        return (Vec::new(), Vec::new());
    }
    let base = signed_area(source);
    if base.abs() < 1e-6 {
        return (Vec::new(), Vec::new());
    }
    let outer = offset_ring(source, inset.max(0.0), into_interior);
    let inner = offset_ring(source, inset.max(0.0) + width, into_interior);
    if outer.len() != source.len() || inner.len() != source.len() {
        return (Vec::new(), Vec::new());
    }
    let inner_area = signed_area(&inner);
    if inner_area.signum() != base.signum() || inner_area.abs() < 1e-4 {
        return (Vec::new(), Vec::new());
    }
    let mut vertices = Vec::with_capacity(outer.len() * 2);
    vertices.extend(outer);
    let inner_base = vertices.len() as u32;
    vertices.extend(inner);
    let mut indices = Vec::with_capacity(source.len() * 6);
    let count = source.len() as u32;
    for index in 0..count {
        let next = (index + 1) % count;
        let a = index;
        let b = next;
        let c = inner_base + index;
        let d = inner_base + next;
        indices.extend_from_slice(&[a, b, c, b, d, c]);
    }
    (vertices, indices)
}

fn boundary_distance(point: WorldPoint, ring: &[WorldPoint]) -> f64 {
    let ring = strip_closing_point(ring);
    if ring.len() < 2 {
        return f64::MAX;
    }
    let mut distance = point_polyline_distance(point, ring);
    distance = distance.min(point_segment_distance(point, ring[0], ring[ring.len() - 1]));
    distance
}

fn offset_ring(ring: &[WorldPoint], distance: f64, into_interior: bool) -> Vec<WorldPoint> {
    if ring.len() < 3 {
        return Vec::new();
    }
    if distance <= 1e-9 {
        return ring.to_vec();
    }
    let left_is_in = signed_area(ring) >= 0.0;
    let side = if into_interior { 1.0 } else { -1.0 };
    let count = ring.len();
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let prev = ring[(index + count - 1) % count];
        let current = ring[index];
        let next = ring[(index + 1) % count];
        let n0 = inward_normal(current.x - prev.x, current.y - prev.y, left_is_in, side);
        let n1 = inward_normal(next.x - current.x, next.y - current.y, left_is_in, side);
        let (mut mx, mut my) = (n0.0 + n1.0, n0.1 + n1.1);
        let length = (mx * mx + my * my).sqrt();
        if length < 1e-9 {
            out.push(WorldPoint::new(
                current.x + n0.0 * distance,
                current.y + n0.1 * distance,
            ));
            continue;
        }
        mx /= length;
        my /= length;
        let cos_half = mx * n0.0 + my * n0.1;
        let scale = if cos_half > 1e-3 {
            (1.0 / cos_half).min(3.0)
        } else {
            3.0
        };
        let offset = distance * scale;
        out.push(WorldPoint::new(current.x + mx * offset, current.y + my * offset));
    }
    out
}

fn inward_normal(dx: f64, dy: f64, left_is_in: bool, side: f64) -> (f64, f64) {
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1e-12 {
        return (0.0, 0.0);
    }
    let (mut nx, mut ny) = (-dy / length, dx / length);
    if !left_is_in {
        nx = -nx;
        ny = -ny;
    }
    (nx * side, ny * side)
}

fn segments_cross(a: WorldPoint, b: WorldPoint, c: WorldPoint, d: WorldPoint) -> bool {
    let orient =
        |p: WorldPoint, q: WorldPoint, r: WorldPoint| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    orient(a, b, c) * orient(a, b, d) < 0.0 && orient(c, d, a) * orient(c, d, b) < 0.0
}

/// Line labels: placed at the polyline's length midpoint, with the angle flipped to read left-to-right.
#[derive(Clone, Copy, Debug)]
pub struct LineLabel {
    /// Length midpoint.
    pub pos: WorldPoint,
    /// Screen clockwise angle, range (-π/2, π/2].
    pub angle: f32,
}

/// Take the length midpoint and a readable direction along the polyline. Returns `None` with fewer than two points.
pub fn line_label(points: &[WorldPoint]) -> Option<LineLabel> {
    if points.len() < 2 {
        return None;
    }
    let mut total = 0.0;
    for window in points.windows(2) {
        total += window[0].dist(window[1]);
    }
    if total < 1e-6 {
        return Some(LineLabel {
            pos: points[0],
            angle: 0.0,
        });
    }
    let mut remain = total * 0.5;
    let last = points.len() - 2;
    for (index, window) in points.windows(2).enumerate() {
        let len = window[0].dist(window[1]);
        if remain <= len || index == last {
            let t = if len < 1e-9 {
                0.0
            } else {
                (remain / len).clamp(0.0, 1.0)
            };
            let pos = WorldPoint::new(
                window[0].x + (window[1].x - window[0].x) * t,
                window[0].y + (window[1].y - window[0].y) * t,
            );
            let dx = window[1].x - window[0].x;
            let dy = window[1].y - window[0].y;
            return Some(LineLabel {
                pos,
                angle: readable_angle(dx, dy),
            });
        }
        remain -= len;
    }
    None
}

/// `atan2(dy, dx)` is the screen clockwise angle when y points down. Segments facing left are flipped 180° to avoid upside-down text.
fn readable_angle(dx: f64, dy: f64) -> f32 {
    if dx == 0.0 && dy == 0.0 {
        return 0.0;
    }
    let mut angle = dy.atan2(dx);
    if angle > std::f64::consts::FRAC_PI_2 {
        angle -= std::f64::consts::PI;
    } else if angle < -std::f64::consts::FRAC_PI_2 {
        angle += std::f64::consts::PI;
    }
    angle as f32
}

/// Remove the duplicated closing point at the end of a closed ring.
pub fn strip_closing_point(ring: &[WorldPoint]) -> &[WorldPoint] {
    if ring.len() >= 2 && ring[0] == ring[ring.len() - 1] {
        &ring[..ring.len() - 1]
    } else {
        ring
    }
}

/// Whether a closed ring is valid (deduplicated, non-zero area).
pub fn is_valid_ring(ring: &[WorldPoint]) -> bool {
    let r = strip_closing_point(ring);
    r.len() >= 3 && signed_area(r).abs() > 1e-6
}

/// Polygon triangulation (ear clipping). Returns indices relative to `ring`.
pub fn triangulate_ring(ring: &[WorldPoint]) -> Vec<u32> {
    let r = strip_closing_point(ring);
    if r.len() < 3 {
        return Vec::new();
    }
    let mut flat = Vec::with_capacity(r.len() * 2);
    for p in r {
        flat.push(p.x);
        flat.push(p.y);
    }
    match earcutr::earcut(&flat, &[], 2) {
        Ok(indices) => indices.into_iter().map(|i| i as u32).collect(),
        Err(_) => Vec::new(),
    }
}

/// Triangulate a polygon with holes: `outer` is the outer ring, `holes` the list of inner rings.
///
/// Returns (vertex coordinates, indices). Vertex order: outer ring first, then inner rings concatenated.
pub fn triangulate_polygon(outer: &[WorldPoint], holes: &[Vec<WorldPoint>]) -> (Vec<WorldPoint>, Vec<u32>) {
    let outer = strip_closing_point(outer);
    if outer.len() < 3 {
        return (Vec::new(), Vec::new());
    }
    let mut vertices: Vec<WorldPoint> = outer.to_vec();
    let mut flat: Vec<f64> = Vec::with_capacity(outer.len() * 2);
    for p in outer {
        flat.push(p.x);
        flat.push(p.y);
    }
    let mut hole_indices: Vec<usize> = Vec::with_capacity(holes.len());
    for hole in holes {
        let h = strip_closing_point(hole);
        if h.len() < 3 {
            continue;
        }
        hole_indices.push(vertices.len());
        vertices.extend_from_slice(h);
        for p in h {
            flat.push(p.x);
            flat.push(p.y);
        }
    }
    match earcutr::earcut(&flat, &hole_indices, 2) {
        Ok(indices) => (vertices, indices.into_iter().map(|i| i as u32).collect()),
        Err(_) => (vertices, Vec::new()),
    }
}

/// Stroke triangles: each polyline point generates a pair of vertices (left/right offsets), with 2 triangles between adjacent points.
///
/// * the offset direction is the average of adjacent segment normals (miter), with a miter-length cap to avoid spikes at sharp corners;
/// * returns (vertices, indices), where vertices are world-coordinate meters and indices are `u32`.
pub fn stroke_polyline(points: &[WorldPoint], width_m: f64) -> (Vec<WorldPoint>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    stroke_polyline_into(points, width_m, &mut vertices, &mut indices);
    (vertices, indices)
}

fn normalize(v: WorldPoint) -> WorldPoint {
    let len = (v.x * v.x + v.y * v.y).sqrt();
    if len < 1e-12 {
        WorldPoint::new(0.0, 0.0)
    } else {
        WorldPoint::new(v.x / len, v.y / len)
    }
}

/// Convenience: use `MissingPolicy` to decide whether a way's geometry is complete.
pub fn way_is_complete(data: &DataSet, way: Handle<WaySlot>) -> bool {
    match data.ways.nodes(way) {
        Some(refs) => refs.iter().all(|h| data.nodes.contains(*h)),
        None => false,
    }
}

/// Synthetic polylines for tests/benchmarks.
pub fn synthetic_line(start: WorldPoint, segments: usize, step_m: f64) -> Vec<WorldPoint> {
    (0..=segments)
        .map(|i| {
            WorldPoint::new(
                start.x + i as f64 * step_m,
                start.y + (i as f64 * 0.1).sin() * step_m,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Zero-allocation variant (scene-build hot path; reuses one scratch buffer to avoid a heap allocation per way)
// ---------------------------------------------------------------------------

/// Parse a way into `out` (overwrite, using the precomputed world-coordinate cache to avoid re-projecting).
pub fn way_points_cached_into(
    data: &DataSet,
    way: Handle<WaySlot>,
    cache: &crate::world_cache::WorldCache,
    out: &mut Vec<WorldPoint>,
) {
    out.clear();
    if let Some(refs) = data.ways.nodes(way) {
        for h in refs {
            // The handle may have expired (slot reused): validate it first, otherwise the cache would return
            // the coordinates of a "new node" and the old way would be connected to the wrong place.
            if !data.nodes.contains(*h) {
                continue;
            }
            if let Some(w) = cache.get(*h) {
                out.push(w);
            }
        }
    }
}

/// Parse a way into `out` (overwrite).
pub fn way_points_into(data: &DataSet, way: Handle<WaySlot>, out: &mut Vec<WorldPoint>) {
    out.clear();
    if let Some(refs) = data.ways.nodes(way) {
        for h in refs {
            if let Some(pos) = data.nodes.position(*h) {
                out.push(WorldPoint::from_latlon(pos));
            }
        }
    }
}

/// RDP simplification into `out`, with `keep` as a reusable flag buffer.
pub fn simplify_rdp_into(
    points: &[WorldPoint],
    tolerance_m: f64,
    out: &mut Vec<WorldPoint>,
    keep: &mut Vec<bool>,
) {
    out.clear();
    if points.len() <= 2 {
        out.extend_from_slice(points);
        return;
    }
    keep.clear();
    keep.resize(points.len(), false);
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_mark(points, tolerance_m, keep);
    for (i, p) in points.iter().enumerate() {
        if keep[i] {
            out.push(*p);
        }
    }
}

/// Stroke triangles into `verts`/`indices` (overwrite).
pub fn stroke_polyline_into(
    points: &[WorldPoint],
    width_m: f64,
    verts: &mut Vec<WorldPoint>,
    indices: &mut Vec<u32>,
) {
    verts.clear();
    indices.clear();
    stroke_polyline_append(points, width_m, verts, indices);
}

/// Miter normal of a polyline at a given vertex; returns `(nx, ny, miter scale factor)`.
///
/// The normal is the average of adjacent segment normals (the left side is `(-dy, dx)`, consistent with
/// [stroke_polyline_append]'s stroke direction convention); reversal degenerates to a single-side normal,
/// and adjacent duplicate points are not miter-scaled, avoiding spikes on screen.
fn join_normal(prev: Option<WorldPoint>, cur: WorldPoint, next: Option<WorldPoint>) -> (f64, f64, f64) {
    let normal_of = |d: WorldPoint| WorldPoint::new(-d.y, d.x);
    let dir_prev = prev.map(|p| normalize(WorldPoint::new(cur.x - p.x, cur.y - p.y)));
    let dir_next = next.map(|p| normalize(WorldPoint::new(p.x - cur.x, p.y - cur.y)));
    let n_prev = dir_prev.map(normal_of);
    let n_next = dir_next.map(normal_of);
    match (n_prev, n_next) {
        (Some(a), Some(b)) => {
            let (mut mx, mut my) = (a.x + b.x, a.y + b.y);
            let mlen = (mx * mx + my * my).sqrt();
            if mlen < 1e-9 {
                // Reversal: degenerate to a single-side normal.
                (a.x, a.y, 1.0)
            } else {
                mx /= mlen;
                my /= mlen;
                let cos_half = mx * a.x + my * a.y;
                let s = if cos_half > 1e-3 {
                    (1.0 / cos_half).min(3.0)
                } else {
                    3.0
                };
                if a.x == 0.0 && a.y == 0.0 {
                    (b.x, b.y, 1.0)
                } else if b.x == 0.0 && b.y == 0.0 {
                    (a.x, a.y, 1.0)
                } else {
                    (mx, my, s)
                }
            }
        }
        (Some(a), None) => (a.x, a.y, 1.0),
        (None, Some(b)) => (b.x, b.y, 1.0),
        (None, None) => (0.0, 0.0, 0.0),
    }
}

/// Translate an entire polyline sideways by `distance_m` (`side`: `1` left, `-1` right).
///
/// Used to draw the slope-foot line of an embankment: the offset direction is the miter average of adjacent segment normals, keeping constant distance at corners.
/// Overwrites `out`; clears it when parameters are invalid (distance non-finite or ≤0, fewer than 2 points).
pub fn offset_polyline_into(points: &[WorldPoint], distance_m: f64, side: i8, out: &mut Vec<WorldPoint>) {
    out.clear();
    if points.len() < 2 || !distance_m.is_finite() || distance_m <= 1e-9 {
        return;
    }
    let sign = if side == -1 { -1.0 } else { 1.0 };
    let n = points.len();
    for i in 0..n {
        let prev = if i > 0 { Some(points[i - 1]) } else { None };
        let cur = points[i];
        let next = if i + 1 < n { Some(points[i + 1]) } else { None };
        let (nx, ny, scale) = join_normal(prev, cur, next);
        let off = distance_m * scale * sign;
        out.push(WorldPoint::new(cur.x + nx * off, cur.y + ny * off));
    }
}

/// Append a polyline's stroke after existing triangles. Dashes must call this per segment; it must not overwrite each time.
pub fn stroke_polyline_append(
    points: &[WorldPoint],
    width_m: f64,
    verts: &mut Vec<WorldPoint>,
    indices: &mut Vec<u32>,
) {
    if points.len() < 2 || width_m <= 0.0 {
        return;
    }
    let half = width_m * 0.5;
    let n = points.len();
    let base = verts.len() as u32;

    for i in 0..n {
        let prev = if i > 0 { Some(points[i - 1]) } else { None };
        let cur = points[i];
        let next = if i + 1 < n { Some(points[i + 1]) } else { None };
        let (nx, ny, scale) = join_normal(prev, cur, next);
        let off = half * scale;
        verts.push(WorldPoint::new(cur.x + nx * off, cur.y + ny * off));
        verts.push(WorldPoint::new(cur.x - nx * off, cur.y - ny * off));
    }

    indices.reserve((n - 1) * 6);
    for i in 0..(n - 1) {
        let a = base + (i * 2) as u32;
        let b = a + 1;
        let c = a + 2;
        let d = a + 3;
        indices.extend_from_slice(&[a, b, c, c, b, d]);
    }
}

/// Place a set of direction arrows along a polyline. When `forward` is false the arrows point opposite to the point order.
///
/// Spacing and size are in meters. Returns empty when the line is too short for even one arrow.
pub fn direction_arrows(
    points: &[WorldPoint],
    spacing_m: f64,
    length_m: f64,
    half_width_m: f64,
    forward: bool,
) -> (Vec<WorldPoint>, Vec<u32>) {
    if points.len() < 2 || spacing_m <= 1e-6 || length_m <= 1e-6 || half_width_m <= 1e-6 {
        return (Vec::new(), Vec::new());
    }
    let total = polyline_length(points);
    let margin = length_m * 0.5;
    if total < length_m * 1.5 {
        return (Vec::new(), Vec::new());
    }
    let mut samples = Vec::new();
    if total < spacing_m + margin {
        samples.push(total * 0.5);
    } else {
        let mut distance = spacing_m * 0.5;
        while distance <= total - margin {
            samples.push(distance);
            distance += spacing_m;
        }
    }
    let sign = if forward { 1.0 } else { -1.0 };
    let mut vertices = Vec::with_capacity(samples.len() * 3);
    let mut indices = Vec::with_capacity(samples.len() * 3);
    for distance in samples {
        let Some((pos, dir_x, dir_y)) = point_along(points, distance) else {
            continue;
        };
        let (dx, dy) = (dir_x * sign, dir_y * sign);
        let (px, py) = (-dy, dx);
        let tip = WorldPoint::new(pos.x + dx * margin, pos.y + dy * margin);
        let base_x = pos.x - dx * margin;
        let base_y = pos.y - dy * margin;
        let left = WorldPoint::new(base_x + px * half_width_m, base_y + py * half_width_m);
        let right = WorldPoint::new(base_x - px * half_width_m, base_y - py * half_width_m);
        let base = vertices.len() as u32;
        vertices.extend([tip, left, right]);
        indices.extend([base, base + 1, base + 2]);
    }
    (vertices, indices)
}

fn point_along(points: &[WorldPoint], distance: f64) -> Option<(WorldPoint, f64, f64)> {
    let mut remain = distance.max(0.0);
    for window in points.windows(2) {
        let dx = window[1].x - window[0].x;
        let dy = window[1].y - window[0].y;
        let len = (dx * dx + dy * dy).sqrt();
        if len <= 1e-9 {
            continue;
        }
        if remain <= len {
            let t = remain / len;
            return Some((
                WorldPoint::new(window[0].x + dx * t, window[0].y + dy * t),
                dx / len,
                dy / len,
            ));
        }
        remain -= len;
    }
    None
}

/// Place slope-side hachures along a polyline (embankments/cuttings). `side`: 1 = left of the point order, -1 = right, 0 = both.
///
/// Each hachure is a short line perpendicular to the polyline at a sample point, of length `length_m` and width `width_m`; sampling runs along arc length
/// starting at `spacing_m * 0.5` with a step of `spacing_m`. Overwrites `verts`/`indices`; with invalid parameters
/// (non-finite, ≤0) or fewer than 2 points both are cleared.
///
/// When the polyline's total length is less than one `spacing_m`, only one hachure is placed at the arc-length midpoint; `side` only recognizes `1`/`-1`, any other value is treated as
/// `0` (both sides). The left normal is `(-dy, dx)`, consistent with the `normal_of` convention inside [stroke_polyline_append].
/// Hachure tips may extend past the polyline endpoints (hachures are not part of the path).
pub fn hachure_ticks(
    points: &[WorldPoint],
    spacing_m: f64,
    length_m: f64,
    width_m: f64,
    side: i8,
    verts: &mut Vec<WorldPoint>,
    indices: &mut Vec<u32>,
) {
    verts.clear();
    indices.clear();
    let valid = |value: f64| value.is_finite() && value > 1e-9;
    if !valid(spacing_m) || !valid(length_m) || !valid(width_m) || points.len() < 2 {
        return;
    }
    let total = polyline_length(points);
    if total <= 1e-9 {
        return;
    }
    // Only ±1 is recognized; other values are treated as 0 (both sides). Order is fixed left then right, so the scene layer can overlay them uniformly.
    let sides: &[f64] = match side {
        1 => &[1.0],
        -1 => &[-1.0],
        _ => &[1.0, -1.0],
    };
    let push_tick = |distance: f64, verts: &mut Vec<WorldPoint>, indices: &mut Vec<u32>| {
        let Some((base, dx, dy)) = point_along(points, distance) else {
            return;
        };
        // Left normal = (-dy, dx); the right side negates it.
        let (nx, ny) = (-dy, dx);
        for &sign in sides {
            let tip = WorldPoint::new(base.x + nx * sign * length_m, base.y + ny * sign * length_m);
            // Reuse a stack array; the hot path allocates no extra Vec.
            let segment = [base, tip];
            stroke_polyline_append(&segment, width_m, verts, indices);
        }
    };
    if total < spacing_m {
        push_tick(total * 0.5, verts, indices);
        return;
    }
    let mut distance = spacing_m * 0.5;
    while distance <= total - 1e-9 {
        push_tick(distance, verts, indices);
        distance += spacing_m;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_band_hugs_the_boundary_and_leaves_the_middle_empty() {
        let square = vec![
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(0.0, 10.0),
        ];
        let (vertices, indices) = ring_band(&square, 0.0, 2.0, true);
        assert!(!indices.is_empty());
        assert!(vertices.len() >= 8);
        assert!(point_in_inner_band(
            WorldPoint::new(0.4, 5.0),
            &square,
            &[],
            0.0,
            2.0
        ));
        assert!(!point_in_inner_band(
            WorldPoint::new(5.0, 5.0),
            &square,
            &[],
            0.0,
            2.0
        ));
        let offset = offset_ring(&square, 1.0, true);
        assert!((offset[0].x - 1.0).abs() < 1e-6);
        assert!((offset[0].y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlapping_rings_are_detected() {
        let left = [
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(0.0, 10.0),
        ];
        let overlap = [
            WorldPoint::new(8.0, 0.0),
            WorldPoint::new(18.0, 0.0),
            WorldPoint::new(18.0, 10.0),
            WorldPoint::new(8.0, 10.0),
        ];
        let apart = [
            WorldPoint::new(30.0, 0.0),
            WorldPoint::new(40.0, 0.0),
            WorldPoint::new(40.0, 10.0),
            WorldPoint::new(30.0, 10.0),
        ];
        assert!(rings_overlap(&left, &overlap));
        assert!(!rings_overlap(&left, &apart));
    }

    #[test]
    fn square_label_sits_on_the_centroid() {
        let square = vec![
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(0.0, 10.0),
            WorldPoint::new(0.0, 0.0),
        ];
        let point = area_label_point(&square, &[]).unwrap();
        assert!((point.x - 5.0).abs() < 1e-6);
        assert!((point.y - 5.0).abs() < 1e-6);
    }

    #[test]
    fn label_point_stays_out_of_a_hole() {
        let outer = vec![
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(0.0, 10.0),
        ];
        let hole = vec![
            WorldPoint::new(3.0, 3.0),
            WorldPoint::new(7.0, 3.0),
            WorldPoint::new(7.0, 7.0),
            WorldPoint::new(3.0, 7.0),
        ];
        let point = area_label_point(&outer, std::slice::from_ref(&hole)).unwrap();
        assert!(point_in_ring(point, &outer));
        assert!(!point_in_ring(point, &hole));
    }

    #[test]
    fn line_label_follows_the_path_and_stays_upright() {
        let east = line_label(&[WorldPoint::new(0.0, 0.0), WorldPoint::new(10.0, 0.0)]).unwrap();
        assert!((east.pos.x - 5.0).abs() < 1e-6);
        assert!(east.angle.abs() < 1e-5);
        let west = line_label(&[WorldPoint::new(10.0, 0.0), WorldPoint::new(0.0, 0.0)]).unwrap();
        assert!((west.pos.x - 5.0).abs() < 1e-6);
        assert!(west.angle.abs() < 1e-5);
        let south = line_label(&[WorldPoint::new(0.0, 0.0), WorldPoint::new(0.0, 10.0)]).unwrap();
        assert!((south.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    }

    #[test]
    fn closed_rectangle_area_label_is_center_not_corner() {
        // Mercator scale: a line label's half-perimeter would land on the diagonal, so area labels must use the centroid.
        let ox = 11_560_000.0;
        let oy = 3_580_000.0;
        let w = 80.0;
        let h = 50.0;
        let ring = [
            WorldPoint::new(ox, oy),
            WorldPoint::new(ox + w, oy),
            WorldPoint::new(ox + w, oy + h),
            WorldPoint::new(ox, oy + h),
            WorldPoint::new(ox, oy),
        ];
        let area = area_label_point(&ring, &[]).unwrap();
        assert!((area.x - (ox + w * 0.5)).abs() < 1e-6);
        assert!((area.y - (oy + h * 0.5)).abs() < 1e-6);
        let line = line_label(&ring).unwrap();
        assert!((line.pos.x - (ox + w)).abs() < 1e-6);
        assert!((line.pos.y - (oy + h)).abs() < 1e-6);
    }

    #[test]
    fn polygon_with_hole_is_triangulated() {
        let outer = vec![
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(0.0, 10.0),
            WorldPoint::new(0.0, 0.0),
        ];
        let hole = vec![
            WorldPoint::new(3.0, 3.0),
            WorldPoint::new(7.0, 3.0),
            WorldPoint::new(7.0, 7.0),
            WorldPoint::new(3.0, 7.0),
            WorldPoint::new(3.0, 3.0),
        ];
        assert!(point_in_ring(WorldPoint::new(5.0, 5.0), &hole));
        assert!(!point_in_ring(WorldPoint::new(1.0, 1.0), &hole));
        let indices = triangulate_polygon_with_holes(&outer, &[hole]);
        assert!(!indices.is_empty());
        assert_eq!(indices.len() % 3, 0);
    }

    #[test]
    fn rdp_keeps_only_endpoints_for_collinear_points() {
        let points: Vec<WorldPoint> = (0..5).map(|i| WorldPoint::new(f64::from(i), 0.0)).collect();
        assert_eq!(simplify_rdp_indices(&points, 0.5), vec![0, 4]);
    }

    #[test]
    fn rdp_keeps_a_corner() {
        let points = vec![
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(1.0, 0.0),
            WorldPoint::new(1.0, 1.0),
        ];
        assert_eq!(simplify_rdp_indices(&points, 0.1), vec![0, 1, 2]);
    }

    #[test]
    fn direction_arrows_follow_the_line_and_can_flip() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        let (forward, indices) = direction_arrows(&line, 40.0, 10.0, 4.0, true);
        assert!(!indices.is_empty());
        assert!(forward[0].x > forward[1].x && forward[0].x > forward[2].x);
        let (backward, _) = direction_arrows(&line, 40.0, 10.0, 4.0, false);
        assert!(backward[0].x < backward[1].x && backward[0].x < backward[2].x);
    }

    #[test]
    fn appended_strokes_keep_earlier_segments() {
        let first = [WorldPoint::new(0.0, 0.0), WorldPoint::new(4.0, 0.0)];
        let second = [WorldPoint::new(6.0, 0.0), WorldPoint::new(10.0, 0.0)];
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        stroke_polyline_append(&first, 1.0, &mut verts, &mut indices);
        let after_first = (verts.len(), indices.len());
        stroke_polyline_append(&second, 1.0, &mut verts, &mut indices);
        assert_eq!(verts.len(), after_first.0 * 2);
        assert_eq!(indices.len(), after_first.1 * 2);
        assert!(indices[after_first.1] >= after_first.0 as u32);
    }

    #[test]
    fn box_select_mode_follows_drag_direction() {
        let a = WorldPoint::new(0.0, 0.0);
        let b = WorldPoint::new(10.0, 5.0);
        assert_eq!(box_select_mode(a, b), BoxSelectMode::Window);
        assert_eq!(box_select_mode(b, a), BoxSelectMode::Crossing);
        assert_eq!(box_select_mode(a, a), BoxSelectMode::Window);
    }

    #[test]
    fn window_select_requires_full_containment() {
        let bounds = WorldBounds::from_points(WorldPoint::new(0.0, 0.0), WorldPoint::new(10.0, 10.0));
        let inside = [WorldPoint::new(1.0, 1.0), WorldPoint::new(9.0, 9.0)];
        let partial = [WorldPoint::new(1.0, 1.0), WorldPoint::new(20.0, 1.0)];
        assert!(way_matches_box(&inside, &bounds, BoxSelectMode::Window));
        assert!(!way_matches_box(&partial, &bounds, BoxSelectMode::Window));
        assert!(way_matches_box(&partial, &bounds, BoxSelectMode::Crossing));
    }

    #[test]
    fn crossing_select_hits_segment_without_nodes_inside() {
        let bounds = WorldBounds::from_points(WorldPoint::new(0.0, 0.0), WorldPoint::new(10.0, 10.0));
        // A horizontal line crossing the middle of the box, with both ends outside the box
        let crossing = [WorldPoint::new(-5.0, 5.0), WorldPoint::new(15.0, 5.0)];
        assert!(way_matches_box(&crossing, &bounds, BoxSelectMode::Crossing));
        assert!(!way_matches_box(&crossing, &bounds, BoxSelectMode::Window));
        let outside = [WorldPoint::new(20.0, 20.0), WorldPoint::new(30.0, 20.0)];
        assert!(!way_matches_box(&outside, &bounds, BoxSelectMode::Crossing));
    }

    #[test]
    fn hachure_ticks_place_evenly_on_the_left_side() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        // Pre-fill with garbage to verify overwrite rather than append.
        let mut verts = vec![WorldPoint::new(-999.0, -999.0)];
        let mut indices = vec![7_u32];
        hachure_ticks(&line, 10.0, 5.0, 1.0, 1, &mut verts, &mut indices);
        // total=100, sample arc lengths 5,15,…,95 for 10 hachures; each hachure has a 2-point stroke = 2 points × 2 vertices, 6 indices.
        assert_eq!(indices.len(), 60);
        assert_eq!(verts.len(), 40);
        assert_eq!(indices.len() % 6, 0);
        assert!(indices.iter().all(|&index| (index as usize) < verts.len()));
        // side=1 uses the left normal (0,1): all vertices have y ≥ 0 and do not exceed the hachure length.
        assert!(verts.iter().all(|v| v.y >= -1e-9));
        assert!(verts.iter().all(|v| v.y <= 5.0 + 1e-9));
        assert!(verts.iter().any(|v| (v.y - 5.0).abs() < 1e-9));
        // The first hachure has base arc length 5, the last 95 (the stroke is shifted ±half width 0.5 along x).
        assert!((verts[0].x - 4.5).abs() < 1e-9);
        assert!((verts[36].x - 94.5).abs() < 1e-9);
    }

    #[test]
    fn hachure_ticks_right_side_points_away_from_the_point_order() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        hachure_ticks(&line, 10.0, 5.0, 1.0, -1, &mut verts, &mut indices);
        assert_eq!(indices.len(), 60);
        // Right normal (0,-1): tips are below the polyline.
        assert!(verts.iter().all(|v| v.y <= 1e-9));
        assert!(verts.iter().any(|v| v.y < -4.9));
    }

    #[test]
    fn hachure_ticks_both_sides_and_unknown_side_defaults_to_both() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        let mut left = Vec::new();
        let mut left_indices = Vec::new();
        hachure_ticks(&line, 10.0, 5.0, 1.0, 1, &mut left, &mut left_indices);
        let mut right = Vec::new();
        let mut right_indices = Vec::new();
        hachure_ticks(&line, 10.0, 5.0, 1.0, -1, &mut right, &mut right_indices);
        for side in [0_i8, 7] {
            let mut verts = Vec::new();
            let mut indices = Vec::new();
            hachure_ticks(&line, 10.0, 5.0, 1.0, side, &mut verts, &mut indices);
            assert_eq!(indices.len(), left_indices.len() + right_indices.len());
            assert_eq!(verts.len(), left.len() + right.len());
            assert!(verts.iter().any(|v| v.y > 4.9));
            assert!(verts.iter().any(|v| v.y < -4.9));
        }
    }

    #[test]
    fn hachure_ticks_left_normal_follows_the_point_order() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(0.0, 100.0)];
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        hachure_ticks(&line, 10.0, 5.0, 1.0, 1, &mut verts, &mut indices);
        assert_eq!(indices.len(), 60);
        // With the point order going up, the left normal is (-1,0), so hachures fall on the x<0 side.
        assert!(verts.iter().all(|v| v.x >= -5.0 - 1e-9 && v.x <= 1e-9));
        assert!(verts.iter().any(|v| v.x < -4.9));
    }

    #[test]
    fn hachure_ticks_clear_output_for_invalid_parameters() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        let single = [WorldPoint::new(1.0, 2.0)];
        let degenerate = [WorldPoint::new(3.0, 4.0), WorldPoint::new(3.0, 4.0)];
        let cases: &[(&[WorldPoint], f64, f64, f64)] = &[
            (&line, 0.0, 5.0, 1.0),
            (&line, -10.0, 5.0, 1.0),
            (&line, 10.0, -1.0, 1.0),
            (&line, 10.0, 5.0, 0.0),
            (&line, f64::NAN, 5.0, 1.0),
            (&line, f64::INFINITY, 5.0, 1.0),
            (&line, 10.0, f64::NAN, 1.0),
            (&single, 10.0, 5.0, 1.0),
            (&[], 10.0, 5.0, 1.0),
            (&degenerate, 10.0, 5.0, 1.0),
        ];
        for &(points, spacing, length, width) in cases {
            let mut verts = vec![WorldPoint::new(-999.0, -999.0)];
            let mut indices = vec![7_u32];
            hachure_ticks(points, spacing, length, width, 1, &mut verts, &mut indices);
            assert!(
                verts.is_empty(),
                "verts not cleared: {points:?} {spacing} {length} {width}"
            );
            assert!(indices.is_empty(), "indices not cleared: {points:?}");
        }
    }

    #[test]
    fn hachure_ticks_single_tick_for_a_polyline_shorter_than_spacing() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(4.0, 0.0)];
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        hachure_ticks(&line, 10.0, 5.0, 1.0, 1, &mut verts, &mut indices);
        // total=4 < spacing=10: only one hachure at arc-length midpoint 2.0.
        assert_eq!(indices.len(), 6);
        assert_eq!(verts.len(), 4);
        assert!((verts[0].x - 1.5).abs() < 1e-9);
        assert!(verts.iter().all(|v| v.y >= -1e-9 && v.y <= 5.0 + 1e-9));
        assert!(verts.iter().any(|v| (v.y - 5.0).abs() < 1e-9));
    }

    #[test]
    fn offset_polyline_moves_to_the_requested_side() {
        let line = [WorldPoint::new(0.0, 0.0), WorldPoint::new(100.0, 0.0)];
        let mut out = Vec::new();
        offset_polyline_into(&line, 5.0, 1, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0].x - 0.0).abs() < 1e-9 && (out[0].y - 5.0).abs() < 1e-9);
        assert!((out[1].x - 100.0).abs() < 1e-9 && (out[1].y - 5.0).abs() < 1e-9);

        offset_polyline_into(&line, 5.0, -1, &mut out);
        assert_eq!(out.len(), 2);
        assert!((out[0].y + 5.0).abs() < 1e-9 && (out[1].y + 5.0).abs() < 1e-9);

        // Overwrite, not append; invalid parameters clear the buffers.
        for &(distance, points) in &[(0.0_f64, &line[..]), (f64::NAN, &line[..]), (5.0, &[][..])] {
            let mut out = vec![WorldPoint::new(-999.0, -999.0)];
            offset_polyline_into(points, distance, 1, &mut out);
            assert!(out.is_empty(), "output must be cleared at distance {distance}");
        }
    }

    #[test]
    fn offset_polyline_keeps_a_constant_distance_around_a_corner() {
        let corner = [
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(10.0, 0.0),
            WorldPoint::new(10.0, 10.0),
        ];
        let mut out = Vec::new();
        offset_polyline_into(&corner, 2.0, 1, &mut out);
        assert_eq!(out.len(), 3);
        // Miter intersection at a 90° corner: (10,0) shifted left 2 and up 2 → (8, 2).
        let miter = out[1];
        assert!((miter.x - 8.0).abs() < 1e-6);
        assert!((miter.y - 2.0).abs() < 1e-6);
        // The two translation directions are mirror images of each other.
        let mut right = Vec::new();
        offset_polyline_into(&corner, 2.0, -1, &mut right);
        assert!((right[1].x - 12.0).abs() < 1e-6);
        assert!((right[1].y + 2.0).abs() < 1e-6);
    }

    #[test]
    fn bands_only_clear_holes_that_sit_deep_inside() {
        let outer = [
            WorldPoint::new(0.0, 0.0),
            WorldPoint::new(100.0, 0.0),
            WorldPoint::new(100.0, 100.0),
            WorldPoint::new(0.0, 100.0),
        ];
        // Band width 20: the band's inner boundary is the ring 20..80.
        let deep = vec![
            WorldPoint::new(40.0, 40.0),
            WorldPoint::new(60.0, 40.0),
            WorldPoint::new(60.0, 60.0),
            WorldPoint::new(40.0, 60.0),
        ];
        assert!(band_clears_holes(&outer, std::slice::from_ref(&deep), 20.0));

        // A hole falls within the band range: band geometry does not subtract holes, so it would be covered entirely.
        let in_band = vec![
            WorldPoint::new(5.0, 5.0),
            WorldPoint::new(15.0, 5.0),
            WorldPoint::new(15.0, 15.0),
            WorldPoint::new(5.0, 15.0),
        ];
        assert!(!band_clears_holes(&outer, &[in_band], 20.0));

        // A hole crosses the band's inner boundary.
        let crossing = vec![
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(30.0, 10.0),
            WorldPoint::new(30.0, 30.0),
            WorldPoint::new(10.0, 30.0),
        ];
        assert!(!band_clears_holes(&outer, &[crossing], 20.0));

        // A hole encloses the entire inner boundary.
        let huge = vec![
            WorldPoint::new(10.0, 10.0),
            WorldPoint::new(90.0, 10.0),
            WorldPoint::new(90.0, 90.0),
            WorldPoint::new(10.0, 90.0),
        ];
        assert!(!band_clears_holes(&outer, &[huge], 20.0));

        // With no holes, or a degenerate band (width 0), both cases are left to the solid path.
        assert!(band_clears_holes(&outer, &[], 20.0));
        assert!(band_clears_holes(&outer, &[deep], 0.0));
    }
}
