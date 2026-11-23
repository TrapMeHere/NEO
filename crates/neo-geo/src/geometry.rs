//! 几何构建：Way → 世界坐标折线 → 三角形批次。
//!
//! 明确的职责边界：
//!
//! * 本模块在 **CPU** 上把经纬度折线转成世界坐标、做简化、生成描边三角形、
//!   三角化多边形；
//! * 不做屏幕变换（那是相机的事），也不碰 GPU buffer（那是 `neo-render` 的事）；
//! * 结果用 `[f32; 2]` 米坐标 + `u32` 索引表达，可以零拷贝上传。
//!
//! 为什么不用 GPU 做几何生成：OSM 编辑是"少量对象高频修改"，CPU 增量重建 + 局部上传
//! 比 GPU 上跑几何 shader 更容易保证正确性与可维护性（见 docs/05 的负载划分表）。

use neo_core::Handle;
use neo_store::{DataSet, WaySlot};

use crate::world::{WorldBounds, WorldPoint};

/// 把一条 way 解析成世界坐标折线（跳过失效/缺失节点）。
pub fn way_world_points(data: &DataSet, way: Handle<WaySlot>) -> Vec<WorldPoint> {
    let Some(refs) = data.ways.nodes(way) else {
        return Vec::new();
    };
    refs.iter()
        .filter_map(|h| data.nodes.position(*h))
        .map(WorldPoint::from_latlon)
        .collect()
}

/// 一条 way 的世界坐标包围盒。
pub fn way_bounds(data: &DataSet, way: Handle<WaySlot>) -> Option<WorldBounds> {
    let points = way_world_points(data, way);
    if points.is_empty() {
        return None;
    }
    Some(WorldBounds::from_points_iter(points))
}

/// CAD / JOSM 风格框选模式（由拖拽水平方向决定）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxSelectMode {
    /// 左 → 右：仅选中整体落在框内的要素（窗选）。
    Window,
    /// 右 → 左：选中与框相交或整体在框内的要素（相交选）。
    Crossing,
}

/// 由框选起止点判定模式：`start.x > end.x` 为相交选，否则为窗选。
#[inline]
pub fn box_select_mode(start: WorldPoint, end: WorldPoint) -> BoxSelectMode {
    if start.x > end.x {
        BoxSelectMode::Crossing
    } else {
        BoxSelectMode::Window
    }
}

/// 线段是否与轴对齐包围盒相交（含端点落在盒内、穿边）。
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

/// 折线（way）是否匹配框选模式。
///
/// * 窗选：全部顶点在框内（轴对齐盒为凸，故整条折线也在框内）；
/// * 相交选：任一点在框内，或任一线段与框相交。
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

/// 折线长度（世界坐标米，仅用于 LOD 与简化容差）。
pub fn polyline_length(points: &[WorldPoint]) -> f64 {
    points.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// 点到线段的最近点与距离（f64 米）。返回 `(投影点, 距离, t∈[0,1])`。
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

/// 点到线段的最短距离（f64 米）。
pub fn point_segment_distance(p: WorldPoint, a: WorldPoint, b: WorldPoint) -> f64 {
    point_segment_closest(p, a, b).1
}

/// 点到折线的最短距离（米）。
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

/// Ramer–Douglas–Peucker 折线简化。
///
/// `tolerance_m` 是允许的最大偏离（米）。LOD 与渲染抽稀都用它。
pub fn simplify_rdp(points: &[WorldPoint], tolerance_m: f64) -> Vec<WorldPoint> {
    let mut out = Vec::new();
    let mut keep = Vec::new();
    simplify_rdp_into(points, tolerance_m, &mut out, &mut keep);
    out
}

/// 带洞多边形三角化（earcut）。`holes` 的点序不限，但必须是闭合环（首尾可重复）。
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

/// 点是否在环内（射线法，包含边界近似）。
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

/// RDP 简化后保留的下标（升序，含首尾）。
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

/// 迭代版 RDP 标记（显式栈）：病态折线不会把递归深度推爆调用栈。
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

/// 环的有符号面积（世界坐标平方米；正 = 逆时针）。
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

/// 环的面积质心。退化环返回 `None`。末尾重复的闭合点会被忽略。
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

/// 面状注记落点：优先面积质心。质心落在环外或洞内时，改用面积最大的三角形重心。
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

/// 两个环的区域是否重叠（包围盒相交，且有顶点落入对方或边相交）。
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

/// 点是否落在环的内圈上。`inset` 是从边界向围成区域内部的起点，`width` 是圈宽。
/// 洞内不算。洞边界上同样宽的一圈也算。
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

/// 沿环生成一条向内（或向围成区域外）的等宽条带。偏移把环撑破时返回空。
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

/// 线状注记：落在折线长度中点，角度已翻成从左向右可读。
#[derive(Clone, Copy, Debug)]
pub struct LineLabel {
    /// 长度中点。
    pub pos: WorldPoint,
    /// 屏幕顺时针弧度，范围 (-π/2, π/2]。
    pub angle: f32,
}

/// 沿折线取长度中点与可读走向。少于两个点时返回 `None`。
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

/// y 向下时 `atan2(dy, dx)` 就是屏幕顺时针角。朝左的线段翻转 180°，避免倒字。
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

/// 去掉闭合环末尾的重复点。
pub fn strip_closing_point(ring: &[WorldPoint]) -> &[WorldPoint] {
    if ring.len() >= 2 && ring[0] == ring[ring.len() - 1] {
        &ring[..ring.len() - 1]
    } else {
        ring
    }
}

/// 闭合环是否有效（去重、面积非零）。
pub fn is_valid_ring(ring: &[WorldPoint]) -> bool {
    let r = strip_closing_point(ring);
    r.len() >= 3 && signed_area(r).abs() > 1e-6
}

/// 多边形三角化（耳切法）。返回相对于 `ring` 的索引。
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

/// 带孔多边形三角化：`outer` 外环，`holes` 内环列表。
///
/// 返回 (顶点坐标, 索引)。顶点顺序：外环在前，内环依次拼接。
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

/// 描边三角形：每个折线点生成一对顶点（左右偏移），相邻点之间 2 个三角形。
///
/// * 偏移方向取相邻线段法线的平均（斜接），斜接长度做上限保护，避免尖角爆边；
/// * 返回 (顶点, 索引)，顶点为世界坐标米，索引为 `u32`。
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

/// 便捷：用 `MissingPolicy` 判断 way 几何是否完整。
pub fn way_is_complete(data: &DataSet, way: Handle<WaySlot>) -> bool {
    match data.ways.nodes(way) {
        Some(refs) => refs.iter().all(|h| data.nodes.contains(*h)),
        None => false,
    }
}

/// 便于测试/基准的合成折线。
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
// 零分配变体（场景构建热路径；复用一个 scratch 缓冲区，避免每 way 一次堆分配）
// ---------------------------------------------------------------------------

/// 把 way 解析进 `out`（覆盖写，使用预计算的世界坐标缓存，避免重复投影）。
pub fn way_points_cached_into(
    data: &DataSet,
    way: Handle<WaySlot>,
    cache: &crate::world_cache::WorldCache,
    out: &mut Vec<WorldPoint>,
) {
    out.clear();
    if let Some(refs) = data.ways.nodes(way) {
        for h in refs {
            // 句柄可能已失效（槽位被复用）：必须先校验，否则缓存会返回
            // "新节点"的坐标，旧 way 被连到错误位置。
            if !data.nodes.contains(*h) {
                continue;
            }
            if let Some(w) = cache.get(*h) {
                out.push(w);
            }
        }
    }
}

/// 把 way 解析进 `out`（覆盖写）。
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

/// RDP 简化进 `out`，`keep` 为复用标志位缓冲。
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

/// 描边三角形进 `verts`/`indices`（覆盖写）。
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

/// 把一条折线的描边接到已有三角形后面。虚线要逐段调用，不能每次覆盖。
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
    let normal_of = |d: WorldPoint| WorldPoint::new(-d.y, d.x);

    for i in 0..n {
        let prev = if i > 0 { Some(points[i - 1]) } else { None };
        let cur = points[i];
        let next = if i + 1 < n { Some(points[i + 1]) } else { None };

        let dir_prev = prev.map(|p| normalize(WorldPoint::new(cur.x - p.x, cur.y - p.y)));
        let dir_next = next.map(|p| normalize(WorldPoint::new(p.x - cur.x, p.y - cur.y)));
        let n_prev = dir_prev.map(normal_of);
        let n_next = dir_next.map(normal_of);

        let (nx, ny, scale) = match (n_prev, n_next) {
            (Some(a), Some(b)) => {
                let (mut mx, mut my) = (a.x + b.x, a.y + b.y);
                let mlen = (mx * mx + my * my).sqrt();
                if mlen < 1e-9 {
                    // 反向折返：退化为单侧法线。
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
                    // 相邻重复点会得到零方向法线：此时不要再做斜接放大，
                    // 否则 1 个重复点会在屏幕上长出 3 倍宽的尖刺。
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
        };

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

/// 沿折线放一组方向箭头。`forward` 为假时箭头与点序相反。
///
/// 间距和尺寸都是米。线太短放不下一个箭头时返回空。
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
        // 墨卡托量级：line_label 半周长会落在对角，面注记必须用质心。
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
        // 水平线穿过框中部，两端都在框外
        let crossing = [WorldPoint::new(-5.0, 5.0), WorldPoint::new(15.0, 5.0)];
        assert!(way_matches_box(&crossing, &bounds, BoxSelectMode::Crossing));
        assert!(!way_matches_box(&crossing, &bounds, BoxSelectMode::Window));
        let outside = [WorldPoint::new(20.0, 20.0), WorldPoint::new(30.0, 20.0)];
        assert!(!way_matches_box(&outside, &bounds, BoxSelectMode::Crossing));
    }
}
