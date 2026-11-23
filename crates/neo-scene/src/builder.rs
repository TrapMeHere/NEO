//! 场景装配：视口 → 可见对象 → 顶点批次。
//!
//! 关键性能原则（docs/04 §2.9）：
//!
//! * **绘制量由视口决定，而不是由总对象数决定**：先经空间索引裁剪，再做 LOD；
//! * 几何在 CPU 上生成，但只生成"看得见"的部分；
//! * 平移/缩放不重建几何（GPU 用 uniform 变换），只有数据变化或需要原点重基时才重建；
//! * 顶点坐标 = 世界坐标 - 批次原点（f32 米），原点取相机中心。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use neo_core::{Handle, ObjectId, ObjectKind};
use neo_geo::{Camera, SpatialIndex, WorldCache, WorldPoint, geometry};
use neo_store::DataSet;
use neo_style::{Colors, MapCss, StyleKeys, StyleSheet};

use crate::batches::{
    AreaBand, BatchKind, LabelPlace, MapVertex, RenderBatch, SceneFrame, SceneIcon, SceneLabel, SceneStats,
    name_color,
};

/// 每帧复用的几何 scratch 缓冲区（避免每 way 一次堆分配）。
#[derive(Default)]
struct Scratch {
    world: Vec<WorldPoint>,
    decimated: Vec<WorldPoint>,
    simplified: Vec<WorldPoint>,
    keep: Vec<bool>,
    stroke_verts: Vec<WorldPoint>,
    stroke_indices: Vec<u32>,
    dash: Vec<WorldPoint>,
    positions: Vec<[f32; 2]>,
}

/// 进入 RDP 前的最大点数：超过则先做等比抽稀。
/// RDP 在病态折线（2000+ 节点且形状碎裂）上可能退化为 O(n²)，预抽稀把最坏情况钉住。
const RDP_MAX_INPUT: usize = 256;

/// 按 OSM `layer` 分桶的绘制批次；按 layer 升序提交，layer 大的压在上面。
struct LayerBatches {
    fill: RenderBatch,
    stroke: RenderBatch,
    point: RenderBatch,
}

impl Default for LayerBatches {
    fn default() -> Self {
        Self {
            fill: RenderBatch::with_capacity(BatchKind::Fill, 8192, 16384),
            stroke: RenderBatch::with_capacity(BatchKind::Stroke, 65536, 131072),
            point: RenderBatch::with_capacity(BatchKind::Point, 4096, 8192),
        }
    }
}

fn total_layer_vertices(layers: &BTreeMap<i16, LayerBatches>) -> usize {
    layers
        .values()
        .map(|batch| batch.fill.vertices.len() + batch.stroke.vertices.len() + batch.point.vertices.len())
        .sum()
}

fn way_layer(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)]) -> i16 {
    tags.iter()
        .find(|(key, _)| data.resolve(*key) == "layer")
        .and_then(|(_, value)| data.resolve(*value).trim().parse::<i16>().ok())
        .unwrap_or(0)
        .clamp(-5, 5)
}

fn resolved_tag_names(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (data.resolve(*key).to_owned(), data.resolve(*value).to_owned()))
        .collect()
}

/// 低于该 zoom 不绘制节点（z16 及以下不显示节点）。
pub const NODE_MIN_ZOOM: f64 = 17.0;
/// 达到该 zoom 绘制全部节点；17～18 之间只绘制共享节点。
pub const NODE_ALL_ZOOM: f64 = 19.0;
/// 共享节点判定：被至少这么多条 way 引用。
pub const SHARED_NODE_MIN_WAYS: usize = 2;
/// 节点图标和名称从这个 zoom 才画。顶点本身仍按 17/19 的规则。
const NODE_ICON_MIN_ZOOM: f64 = 21.0;
/// 面名最早出现的 zoom。更早只剩整片大陆，名字没有参照。
const AREA_NAME_MIN_ZOOM: f64 = 13.0;
/// 面在屏幕上至少这么大（平方像素）才写名字。面积大的更早达到。
const AREA_NAME_MIN_PX2: f64 = 2_800.0;
/// 主干道（线宽 ≥ 3px）写名字的 zoom。普通路更晚，避免细线挤成一团。
const LINE_MAJOR_MIN_ZOOM: f64 = 16.5;
const LINE_MINOR_MIN_ZOOM: f64 = 18.0;
/// 线在屏幕上短于这个长度就不写名字。
const LINE_MAJOR_MIN_PX: f64 = 200.0;
const LINE_MINOR_MIN_PX: f64 = 160.0;
/// 一帧保留的注记上限。满了就换掉优先级最低的。
const LABEL_KEEP: usize = 700;
/// 面的内圈宽度（屏幕像素）。外缘贴着面的边界，向内铺这么宽。
const AREA_BAND_PX: f64 = 20.0;
/// 水路和单行道的方向箭头：沿屏幕每隔这么远放一个。
const ARROW_SPACING_PX: f64 = 80.0;
const ARROW_LENGTH_PX: f64 = 12.0;
const ARROW_HALF_WIDTH_PX: f64 = 4.5;

/// 某 zoom 下节点是否可见/可拾取（渲染与拾取共用同一规则）。
///
/// * `zoom < NODE_MIN_ZOOM(17)`：不可见；
/// * `17 <= zoom < NODE_ALL_ZOOM(19)`：只有共享节点（被 ≥2 条 way 引用）可见；
/// * `zoom >= 19`：全部可见。
pub fn node_visible_at(zoom: f64, way_ref_count: usize) -> bool {
    if zoom < NODE_MIN_ZOOM {
        false
    } else if zoom < NODE_ALL_ZOOM {
        way_ref_count >= SHARED_NODE_MIN_WAYS
    } else {
        true
    }
}

/// 场景装配配置。
#[derive(Clone, Debug)]
pub struct SceneConfig {
    /// 折线简化容差（屏幕像素）。
    pub simplify_tolerance_px: f64,
    /// 视口外扩比例（避免边缘对象突然出现）。
    pub cull_margin: f64,
    /// 单帧顶点上限（安全阀，防止病态数据卡死）。
    pub max_vertices: usize,
    /// 背景色（由 UI 读取，不在顶点里）。
    pub background: [u8; 4],
    /// Way 抽样步长。`1` 表示视口内的 way 全部进入几何；
    /// 大于 1 时只保留每 N 条（远景 LOD / 流式总览）。
    pub way_stride: usize,
}

impl Default for SceneConfig {
    fn default() -> Self {
        Self {
            simplify_tolerance_px: 0.5,
            cull_margin: 0.05,
            max_vertices: 8_000_000,
            background: Colors::default().background,
            way_stride: 1,
        }
    }
}

/// 场景构建器（可跨帧复用，样式与配置固定）。
pub struct SceneBuilder {
    /// 样式表。
    pub style: StyleSheet,
    /// 配置。
    pub config: SceneConfig,
    /// 被过滤器隐藏的对象（场景与拾取共用同一集合）。
    hidden: HashSet<(ObjectKind, ObjectId)>,
    /// 复用缓冲区。
    scratch: RefCell<Scratch>,
}

impl SceneBuilder {
    /// 构造。
    pub fn new(style: StyleSheet, config: SceneConfig) -> Self {
        Self {
            style,
            config,
            hidden: HashSet::new(),
            scratch: RefCell::new(Scratch::default()),
        }
    }

    /// 用调用方提供的符号驻留器构建默认样式。
    ///
    /// 编辑器把名字留在自己的 `DataSet` 里，场景层不再借用可变数据集。
    pub fn with_intern(mut intern: impl FnMut(&str) -> neo_core::Sym) -> Self {
        let keys = StyleKeys {
            highway: intern("highway"),
            building: intern("building"),
            waterway: intern("waterway"),
            natural: intern("natural"),
            railway: intern("railway"),
            area: intern("area"),
        };
        let mapcss = MapCss::parse(include_str!("../../../assets/styles/neo.mapcss"))
            .ok()
            .map(|css| css.compile(|name| intern(name)));
        let mut style = StyleSheet::new(Colors::default(), keys);
        if let Some(mapcss) = mapcss {
            style.set_mapcss(mapcss);
        }
        Self::new(style, SceneConfig::default())
    }

    /// 为数据集构建默认样式（把 tag key 驻留进这份数据）。
    pub fn with_dataset(data: &mut DataSet) -> Self {
        Self::with_intern(|name| data.intern(name))
    }

    /// 设置过滤器隐藏集合；下一次 build 生效。
    pub fn set_hidden(&mut self, hidden: HashSet<(ObjectKind, ObjectId)>) {
        self.hidden = hidden;
    }

    /// 构建一帧场景。
    ///
    /// 节点 LOD：`zoom < 17` 不绘制（z16 及以下不显示）；`17..19` 只绘制共享节点
    /// （被 ≥2 条 way 引用，小圆点）；`zoom >= 19` 绘制全部节点，其中共享节点画小方框、
    /// 普通节点画圆点。节点图标和名称要到 zoom 21 才出现，名称写在图标右侧。
    pub fn build(
        &self,
        data: &DataSet,
        index: &SpatialIndex,
        referrers: &neo_store::Referrers,
        cache: &WorldCache,
        camera: &Camera,
    ) -> SceneFrame {
        #[cfg(feature = "profile")]
        puffin::profile_function!();
        let started = Instant::now();
        let mut stats = SceneStats::default();
        let origin = camera.center;

        // 1) 视口裁剪（外扩 cull_margin）。
        let mut bounds = camera.visible_bounds();
        let (w, h) = bounds.size();
        bounds = bounds::expand(bounds, w * self.config.cull_margin, h * self.config.cull_margin);

        // 2) 分桶（预分配，避免大场景下反复扩容拷贝）。
        let mut layers: BTreeMap<i16, LayerBatches> = BTreeMap::new();
        let mut labels = Vec::new();
        let mut icons = Vec::new();
        let mut areas: Vec<PendingArea> = Vec::new();

        // 3) 零分配遍历：直接经 rstar 迭代器喂给几何构建。
        let stride = self.config.way_stride.max(1);
        let mut seen_ways = 0usize;
        index.for_each_way(bounds, |entry| {
            if stats.clamped {
                return;
            }
            if total_layer_vertices(&layers) >= self.config.max_vertices {
                stats.clamped = true;
                return;
            }
            seen_ways += 1;
            if stride > 1 && !seen_ways.is_multiple_of(stride) {
                stats.skipped += 1;
                return;
            }
            let Some(handle) = entry.as_way() else {
                return;
            };
            if let Some(id) = data.ways.id(handle)
                && self.hidden.contains(&(ObjectKind::Way, id))
            {
                stats.skipped += 1;
                return;
            }
            self.build_way(
                data,
                handle,
                entry.bounds,
                cache,
                camera,
                origin,
                &mut layers,
                &mut labels,
                &mut icons,
                &mut stats,
                &mut areas,
            );
        });
        // 3b) multipolygon：外环和洞都记成面，稍后画内圈。
        for relation in data.iter_relations() {
            if stats.clamped {
                break;
            }
            if !relation.meta.visible {
                continue;
            }
            self.build_multipolygon(
                data,
                relation.handle,
                cache,
                camera,
                origin,
                &mut layers,
                &mut labels,
                &mut stats,
                &mut areas,
            );
        }
        if camera.zoom >= NODE_MIN_ZOOM {
            let show_all = camera.zoom >= NODE_ALL_ZOOM;
            index.for_each_node(bounds, |entry| {
                if stats.clamped {
                    return;
                }
                if total_layer_vertices(&layers) >= self.config.max_vertices {
                    stats.clamped = true;
                    return;
                }
                if let Some(handle) = entry.as_node() {
                    if let Some(id) = data.nodes.id(handle)
                        && self.hidden.contains(&(ObjectKind::Node, id))
                    {
                        stats.skipped += 1;
                        return;
                    }
                    let ref_count = referrers.ways_of_node(handle).len();
                    let visible = node_visible_at(camera.zoom, ref_count);
                    let shared = ref_count >= SHARED_NODE_MIN_WAYS;
                    self.build_node(
                        data,
                        handle,
                        camera,
                        origin,
                        &mut layers,
                        &mut labels,
                        &mut icons,
                        &mut stats,
                        visible,
                        show_all && shared && visible,
                    );
                }
            });
        }

        let area_bands = paint_area_bands(areas, camera, origin, &mut layers);

        // 3) 空批次不提交。
        let mut batches: Vec<RenderBatch> = Vec::new();
        for (_, batch) in layers {
            if !batch.fill.is_empty() {
                batches.push(batch.fill);
            }
            if !batch.stroke.is_empty() {
                batches.push(batch.stroke);
            }
            if !batch.point.is_empty() {
                batches.push(batch.point);
            }
        }
        let vertices = batches.iter().map(|b| b.vertices.len()).sum();
        let triangles = batches.iter().map(|b| b.indices.len() / 3).sum();
        stats.vertices = vertices;
        stats.triangles = triangles;
        stats.build_micros = started.elapsed().as_micros() as u64;
        labels.sort_by(|left, right| right.rank.total_cmp(&left.rank));

        SceneFrame {
            version: data.version(),
            camera: *camera,
            origin,
            batches,
            labels,
            icons,
            area_bands,
            stats,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_multipolygon(
        &self,
        data: &DataSet,
        handle: Handle<neo_store::RelationSlot>,
        cache: &WorldCache,
        camera: &Camera,
        _origin: WorldPoint,
        layers: &mut BTreeMap<i16, LayerBatches>,
        labels: &mut Vec<SceneLabel>,
        stats: &mut SceneStats,
        areas: &mut Vec<PendingArea>,
    ) {
        let Some(view) = data.relation_view(handle) else {
            return;
        };
        let is_multipolygon = view.tag_pairs.iter().any(|(key, value)| {
            data.resolve(*key) == "type" && matches!(data.resolve(*value), "multipolygon" | "boundary")
        });
        if !is_multipolygon {
            return;
        }
        let names = resolved_tag_names(data, view.tag_pairs);
        let name_refs: Vec<(&str, &str)> = names.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let style = self
            .style
            .way_style(view.tag_pairs, camera.zoom as f32, &name_refs);
        let Some(color) = style.fill_color else {
            return;
        };
        let layer = way_layer(data, view.tag_pairs);
        let _ = layers;
        let mut outers: Vec<Vec<WorldPoint>> = Vec::new();
        let mut inners: Vec<Vec<WorldPoint>> = Vec::new();
        let mut scratch2: Vec<WorldPoint> = Vec::new();
        for member in view.members {
            let role = data.resolve(member.role);
            if !matches!(role, "" | "outer" | "inner") {
                continue;
            }
            let Some(way) = member.target.as_way() else {
                continue;
            };
            scratch2.clear();
            geometry::way_points_cached_into(data, way, cache, &mut scratch2);
            if scratch2.len() < 3 {
                continue;
            }
            if scratch2[0] != scratch2[scratch2.len() - 1] {
                scratch2.push(scratch2[0]);
            }
            if role == "inner" {
                inners.push(scratch2.clone());
            } else {
                outers.push(scratch2.clone());
            }
        }
        if outers.is_empty() {
            return;
        }
        let relation_id = data.relations.id(handle).unwrap_or(neo_core::ObjectId::NONE);
        for outer in &outers {
            let holes: Vec<Vec<WorldPoint>> = inners
                .iter()
                .filter(|inner| geometry::point_in_ring(inner[0], outer))
                .cloned()
                .collect();
            areas.push(PendingArea {
                kind: ObjectKind::Relation,
                id: relation_id,
                layer,
                outer: outer.clone(),
                holes: holes.clone(),
                color,
            });
            stats.ways += 1;
            if let Some((_, value)) = view
                .tag_pairs
                .iter()
                .find(|(key, _)| data.resolve(*key) == style.text.as_deref().unwrap_or("name"))
                && let Some(pos) = geometry::area_label_point(outer, &holes)
            {
                push_area_label(
                    labels,
                    camera,
                    pos,
                    data.resolve(*value),
                    name_color(style.text_color, style.color, true),
                    style.font_size_px.unwrap_or(11.5),
                    polygon_area(outer, &holes),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_way(
        &self,
        data: &DataSet,
        handle: Handle<neo_store::WaySlot>,
        bounds: neo_geo::WorldBounds,
        cache: &WorldCache,
        camera: &Camera,
        origin: WorldPoint,
        layers: &mut BTreeMap<i16, LayerBatches>,
        labels: &mut Vec<SceneLabel>,
        icons: &mut Vec<SceneIcon>,
        stats: &mut SceneStats,
        areas: &mut Vec<PendingArea>,
    ) {
        // LOD 预裁剪（用索引 bbox，先于几何生成）+ 密度抽样：缩得很远时按 stride 抽样，
        // 既保留路网结构，又把"每帧重建"的 way 数压到可控范围。
        let (bw, bh) = bounds.size();
        let screen_diag = ((bw * bw + bh * bh).sqrt() * camera.px_per_meter()) as f32;
        let min_px = self.style.min_way_length_px;
        if screen_diag < min_px {
            // 密度 LOD：目标抽样数与"低于目标像素数的平方"成正比，使屏幕单位面积内
            // 实际绘制的 way 数大致恒定（标准矢量瓦片做法），而不是把整类要素砍掉。
            let ratio = (min_px / screen_diag.max(0.05)).ceil() as u32;
            let stride = (ratio * ratio).clamp(2, 32);
            if !handle.index().is_multiple_of(stride) {
                stats.skipped += 1;
                return;
            }
        }

        let mut scratch = self.scratch.borrow_mut();
        let Scratch {
            world,
            decimated,
            simplified,
            keep,
            stroke_verts,
            stroke_indices,
            dash,
            positions,
        } = &mut *scratch;

        geometry::way_points_cached_into(data, handle, cache, world);
        if world.len() < 2 {
            return;
        }
        let Some(view) = data.way_view(handle) else {
            return;
        };
        let tags = view.tag_pairs;
        let names = resolved_tag_names(data, tags);
        let name_refs: Vec<(&str, &str)> = names.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let way_style = self.style.way_style(tags, camera.zoom as f32, &name_refs);
        // 节点引用闭合优先；世界坐标首尾相等作兜底（缺节点时仍可能画得出来）。
        let closed = view.is_closed() || (world.len() >= 4 && world[0] == world[world.len() - 1]);
        let layer = way_layer(data, tags);
        let batch = layers.entry(layer).or_default();
        let stroke = &mut batch.stroke;
        if let Some(label_key) = way_style.text.as_deref().or(Some("name"))
            && let Some((_, value)) = tags.iter().find(|(key, _)| data.resolve(*key) == label_key)
        {
            let text = data.resolve(*value);
            // 面状闭合环用质心注记；纯闭合道路仍沿线，避免半周长落在对角。
            let area_like = way_style.fill_color.is_some() || neo_style::tags_imply_area(&name_refs);
            if closed && area_like {
                if let Some(pos) = geometry::area_label_point(world, &[]) {
                    push_area_label(
                        labels,
                        camera,
                        pos,
                        text,
                        name_color(way_style.text_color, way_style.color, true),
                        way_style.font_size_px.unwrap_or(11.5),
                        geometry::signed_area(world).abs(),
                    );
                }
            } else if let Some(line) = geometry::line_label(world) {
                let screen_len = geometry::polyline_length(world) * camera.px_per_meter();
                let major = way_style.width_px >= 3.0;
                if line_name_allowed(major, camera.zoom, screen_len) {
                    consider_label(
                        labels,
                        SceneLabel {
                            pos: line.pos,
                            place: LabelPlace::Along { angle: line.angle },
                            text: text.to_owned(),
                            color: name_color(way_style.text_color, way_style.color, false),
                            size_px: way_style.font_size_px.unwrap_or(11.0),
                            rank: (screen_len as f32 * 0.02).min(30.0),
                            wrap_px: 110.0,
                        },
                    );
                }
            }
        }

        if let Some(icon) = &way_style.icon_image
            && icons.len() < 400
        {
            icons.push(SceneIcon {
                pos: world[world.len() / 2],
                image: icon.clone(),
                width_px: way_style.icon_width_px.unwrap_or(16.0),
                color: way_style.color,
            });
        }
        // 面先记下来。够大画内圈，屏幕上不足一圈宽时再改实心。
        if closed && let Some(color) = way_style.fill_color {
            areas.push(PendingArea {
                kind: ObjectKind::Way,
                id: data.ways.id(handle).unwrap_or(neo_core::ObjectId::NONE),
                layer,
                outer: world.clone(),
                holes: Vec::new(),
                color,
            });
            stats.ways += 1;
        }

        // 屏幕空间简化：容差换算成米；短 way 直接跳过 RDP。
        let tolerance_m = self.config.simplify_tolerance_px * camera.meters_per_px();
        if world.len() > 4 {
            let source: &[WorldPoint] = if world.len() > RDP_MAX_INPUT {
                decimated.clear();
                let step = world.len().div_ceil(RDP_MAX_INPUT);
                let mut i = 0;
                while i < world.len() {
                    decimated.push(world[i]);
                    i += step;
                }
                if decimated.last() != world.last() {
                    decimated.push(world[world.len() - 1]);
                }
                decimated
            } else {
                world
            };
            geometry::simplify_rdp_into(source, tolerance_m, simplified, keep);
        } else {
            simplified.clear();
            simplified.extend_from_slice(world);
        }
        if simplified.len() < 2 {
            if !closed {
                stats.skipped += 1;
            }
            return;
        }

        // 面填充成功后仍然绘制描边（MapCSS 的 fill + casing 组合）。
        let width_px = way_style.width_px;
        if width_px <= 0.0 {
            return;
        }
        let width_m = camera.px_to_meters(width_px);
        let mut dashed = false;
        if let Some(pattern) = &way_style.dashes {
            let pattern_m: Vec<f64> = pattern
                .iter()
                .filter_map(|px| {
                    let value = camera.px_to_meters(*px);
                    (value > 1e-6).then_some(value)
                })
                .collect();
            if pattern_m.len() >= 2 {
                dashed = true;
                stroke_verts.clear();
                stroke_indices.clear();
                let mut on = true;
                let mut index = 0usize;
                let mut remaining = pattern_m[0];
                for pair in simplified.windows(2) {
                    let a = pair[0];
                    let b = pair[1];
                    let dx = b.x - a.x;
                    let dy = b.y - a.y;
                    let len = (dx * dx + dy * dy).sqrt();
                    if len <= 1e-9 {
                        continue;
                    }
                    let (ux, uy) = (dx / len, dy / len);
                    let mut t = 0.0f64;
                    while t < len - 1e-9 {
                        if on && dash.is_empty() {
                            dash.push(WorldPoint::new(a.x + ux * t, a.y + uy * t));
                        }
                        let step = (len - t).min(remaining);
                        t += step;
                        remaining -= step;
                        if on {
                            dash.push(WorldPoint::new(a.x + ux * t, a.y + uy * t));
                        }
                        if remaining <= 1e-9 {
                            if on && dash.len() >= 2 {
                                geometry::stroke_polyline_append(dash, width_m, stroke_verts, stroke_indices);
                            }
                            dash.clear();
                            on = !on;
                            index = (index + 1) % pattern_m.len();
                            remaining = pattern_m[index];
                        }
                    }
                }
                if on && dash.len() >= 2 {
                    geometry::stroke_polyline_append(dash, width_m, stroke_verts, stroke_indices);
                }
                dash.clear();
            }
        }
        if !dashed {
            geometry::stroke_polyline_into(simplified, width_m, stroke_verts, stroke_indices);
        }
        if stroke_indices.is_empty() {
            if dashed {
                geometry::stroke_polyline_into(simplified, width_m, stroke_verts, stroke_indices);
            }
            if stroke_indices.is_empty() {
                return;
            }
        }
        positions.clear();
        positions.extend(stroke_verts.iter().map(|p| rel(*p, origin)));
        stroke.push_indexed(positions, stroke_indices, way_style.color);
        if let Some(forward) = way_arrow_forward(data, tags, closed && way_style.fill_color.is_some()) {
            let (arrow_verts, arrow_indices) = geometry::direction_arrows(
                simplified,
                camera.px_to_meters(ARROW_SPACING_PX as f32),
                camera.px_to_meters(ARROW_LENGTH_PX as f32),
                camera.px_to_meters(ARROW_HALF_WIDTH_PX as f32),
                forward,
            );
            if !arrow_indices.is_empty() {
                positions.clear();
                positions.extend(arrow_verts.iter().map(|point| rel(*point, origin)));
                stroke.push_indexed(positions, &arrow_indices, arrow_ink(way_style.color));
            }
        }
        if !closed {
            stats.ways += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_node(
        &self,
        data: &DataSet,
        handle: Handle<neo_store::NodeSlot>,
        camera: &Camera,
        origin: WorldPoint,
        layers: &mut BTreeMap<i16, LayerBatches>,
        labels: &mut Vec<SceneLabel>,
        icons: &mut Vec<SceneIcon>,
        stats: &mut SceneStats,
        draw_vertex: bool,
        square: bool,
    ) {
        let Some(pos) = data.nodes.position(handle) else {
            return;
        };
        let world = WorldPoint::from_latlon(pos);
        let view = data.node_view(handle);
        let tags = view.as_ref().map(|view| view.tag_pairs).unwrap_or(&[]);
        let node_style = self.style.node_style(tags, camera.zoom as f32);
        if !draw_vertex {
            return;
        }
        let show_label = camera.zoom >= NODE_ICON_MIN_ZOOM;
        let has_icon = show_label && node_style.icon_image.is_some();
        if has_icon
            && icons.len() < 400
            && let Some(icon) = node_style.icon_image.clone()
        {
            icons.push(SceneIcon {
                pos: world,
                image: icon,
                width_px: node_style.icon_width_px.unwrap_or(18.0),
                color: node_style.color,
            });
        }
        if show_label && let Some((_, value)) = tags.iter().find(|(key, _)| data.resolve(*key) == "name") {
            let icon_px = node_style
                .icon_width_px
                .unwrap_or(if has_icon { 18.0 } else { 0.0 });
            let gap_px = if has_icon { icon_px * 0.5 + 4.0 } else { 8.0 };
            consider_label(
                labels,
                SceneLabel {
                    pos: world,
                    place: LabelPlace::Right { gap_px },
                    text: data.resolve(*value).to_owned(),
                    color: name_color(node_style.text_color, node_style.color, false),
                    size_px: node_style.font_size_px.unwrap_or(10.5),
                    rank: if has_icon { 55.0 } else { 35.0 },
                    wrap_px: 120.0,
                },
            );
        }
        if !draw_vertex {
            return;
        }
        let point = &mut layers.entry(0).or_default().point;
        let center = rel(world, origin);
        let r = camera.px_to_meters(self.style.node_radius_px) as f32;
        let color = node_style.color;
        if square {
            // 共享节点（z16+）：小方框（两个三角形）。
            point
                .vertices
                .push(MapVertex::new([center[0] - r, center[1] - r], color));
            point
                .vertices
                .push(MapVertex::new([center[0] + r, center[1] - r], color));
            point
                .vertices
                .push(MapVertex::new([center[0] + r, center[1] + r], color));
            point
                .vertices
                .push(MapVertex::new([center[0] - r, center[1] + r], color));
            let base = (point.vertices.len() - 4) as u32;
            point
                .indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            // 普通节点/低倍率共享节点：小圆点（六边形近似，7 顶点 / 6 三角形）。
            const SEGMENTS: usize = 6;
            point.vertices.push(MapVertex::new(center, color));
            for i in 0..SEGMENTS {
                let angle = core::f32::consts::TAU * i as f32 / SEGMENTS as f32;
                point.vertices.push(MapVertex::new(
                    [center[0] + angle.cos() * r, center[1] + angle.sin() * r],
                    color,
                ));
            }
            let base = (point.vertices.len() - (SEGMENTS + 1)) as u32;
            for i in 0..SEGMENTS as u32 {
                let next = (i + 1) % SEGMENTS as u32;
                point
                    .indices
                    .extend_from_slice(&[base, base + 1 + i, base + 1 + next]);
            }
        }
        stats.nodes += 1;
    }
}

#[inline]
fn rel(p: WorldPoint, origin: WorldPoint) -> [f32; 2] {
    // 日期变更线：把顶点放到离相机原点最近的"世界副本"，跨 180° 的 way
    // 不会横穿整张地图（顶点坐标仍是相对原点的米）。
    let mut dx = p.x - origin.x;
    if dx > neo_geo::EARTH_CIRCUMFERENCE_M * 0.5 {
        dx -= neo_geo::EARTH_CIRCUMFERENCE_M;
    } else if dx < -neo_geo::EARTH_CIRCUMFERENCE_M * 0.5 {
        dx += neo_geo::EARTH_CIRCUMFERENCE_M;
    }
    [dx as f32, (p.y - origin.y) as f32]
}

mod bounds {
    use neo_geo::WorldBounds;

    pub fn expand(mut b: WorldBounds, dx: f64, dy: f64) -> WorldBounds {
        b.min.x -= dx;
        b.min.y -= dy;
        b.max.x += dx;
        b.max.y += dy;
        b
    }
}

struct PendingArea {
    kind: ObjectKind,
    id: neo_core::ObjectId,
    layer: i16,
    outer: Vec<WorldPoint>,
    holes: Vec<Vec<WorldPoint>>,
    color: [u8; 4],
}

fn paint_area_bands(
    areas: Vec<PendingArea>,
    camera: &Camera,
    origin: WorldPoint,
    layers: &mut BTreeMap<i16, LayerBatches>,
) -> Vec<AreaBand> {
    let width_m = camera.px_to_meters(AREA_BAND_PX as f32);
    if areas.is_empty() || width_m <= 1e-6 {
        return Vec::new();
    }
    let px_per_meter = camera.px_per_meter();
    let mut hits = Vec::with_capacity(areas.len());
    for area in areas {
        let fill = &mut layers.entry(area.layer).or_default().fill;
        let band = area_band_geometry(&area.outer, px_per_meter, width_m);
        let solid = band.is_none();
        let drawn = if let Some((outer_band, outer_indices)) = band {
            push_band(fill, origin, &outer_band, &outer_indices, area.color);
            for hole in &area.holes {
                let (vertices, indices) = geometry::ring_band(hole, 0.0, width_m, false);
                push_band(fill, origin, &vertices, &indices, area.color);
            }
            true
        } else {
            push_solid(fill, origin, &area.outer, &area.holes, area.color)
        };
        if !drawn {
            continue;
        }
        hits.push(AreaBand {
            kind: area.kind,
            id: area.id,
            outer: area.outer,
            holes: area.holes,
            inset_m: 0.0,
            width_m,
            solid,
        });
    }
    hits
}

/// 短边够放下内圈时返回条带；否则返回 `None`，调用方改画实心。随当前缩放重算。
fn area_band_geometry(
    outer: &[WorldPoint],
    px_per_meter: f64,
    width_m: f64,
) -> Option<(Vec<WorldPoint>, Vec<u32>)> {
    if area_span_px(outer, px_per_meter) < AREA_BAND_PX {
        return None;
    }
    let band = geometry::ring_band(outer, 0.0, width_m, true);
    if band.1.is_empty() { None } else { Some(band) }
}

fn area_span_px(outer: &[WorldPoint], px_per_meter: f64) -> f64 {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for point in outer {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    if !min_x.is_finite() {
        return 0.0;
    }
    let width = (max_x - min_x) * px_per_meter;
    let height = (max_y - min_y) * px_per_meter;
    width.min(height)
}

fn push_solid(
    fill: &mut RenderBatch,
    origin: WorldPoint,
    outer: &[WorldPoint],
    holes: &[Vec<WorldPoint>],
    color: [u8; 4],
) -> bool {
    let (vertices, indices) = geometry::triangulate_polygon(outer, holes);
    if indices.is_empty() {
        return false;
    }
    push_band(fill, origin, &vertices, &indices, color);
    true
}

fn push_band(
    fill: &mut RenderBatch,
    origin: WorldPoint,
    vertices: &[WorldPoint],
    indices: &[u32],
    color: [u8; 4],
) {
    if indices.is_empty() {
        return;
    }
    let positions: Vec<[f32; 2]> = vertices.iter().map(|point| rel(*point, origin)).collect();
    fill.push_indexed(&positions, indices, color);
}

/// 水路沿点序（下游）。`highway` 且 `oneway=yes/1` 沿点序，`oneway=-1` 相反。
/// 已经画成面的水路（河岸等）不再铺箭头。
fn way_arrow_forward(
    data: &DataSet,
    tags: &[(neo_core::Sym, neo_core::Sym)],
    area_fill: bool,
) -> Option<bool> {
    let mut highway = false;
    let mut waterway = false;
    let mut oneway = None;
    for (key, value) in tags {
        match data.resolve(*key) {
            "highway" => highway = true,
            "waterway" if !area_fill => waterway = true,
            "oneway" => {
                oneway = match data.resolve(*value) {
                    "yes" | "true" | "1" => Some(true),
                    "-1" | "reverse" => Some(false),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    if highway && let Some(direction) = oneway {
        return Some(direction);
    }
    waterway.then_some(true)
}

fn arrow_ink(stroke: [u8; 4]) -> [u8; 4] {
    let luma = u32::from(stroke[0]) * 30 + u32::from(stroke[1]) * 59 + u32::from(stroke[2]) * 11;
    if luma > 14_000 {
        [22, 24, 28, 235]
    } else {
        [245, 247, 250, 235]
    }
}

fn polygon_area(outer: &[WorldPoint], holes: &[Vec<WorldPoint>]) -> f64 {
    let mut area = geometry::signed_area(outer).abs();
    for hole in holes {
        area = (area - geometry::signed_area(hole).abs()).max(0.0);
    }
    area
}

fn push_area_label(
    labels: &mut Vec<SceneLabel>,
    camera: &Camera,
    pos: WorldPoint,
    text: &str,
    color: [u8; 4],
    size_px: f32,
    area_m2: f64,
) {
    if text.is_empty() || !area_name_allowed(camera.zoom, area_m2, camera.px_per_meter()) {
        return;
    }
    let span = (area_m2 * camera.px_per_meter() * camera.px_per_meter()).sqrt() as f32;
    consider_label(
        labels,
        SceneLabel {
            pos,
            place: LabelPlace::Center,
            text: text.to_owned(),
            color,
            size_px,
            rank: area_m2 as f32,
            wrap_px: (span * 0.7).clamp(72.0, 220.0),
        },
    );
}

fn consider_label(labels: &mut Vec<SceneLabel>, label: SceneLabel) {
    if label.text.is_empty() {
        return;
    }
    if labels.len() < LABEL_KEEP {
        labels.push(label);
        return;
    }
    let Some(slot) = labels
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| left.rank.total_cmp(&right.rank))
        .map(|(index, _)| index)
    else {
        return;
    };
    if label.rank > labels[slot].rank {
        labels[slot] = label;
    }
}

fn line_name_allowed(major: bool, zoom: f64, screen_len_px: f64) -> bool {
    if major {
        zoom >= LINE_MAJOR_MIN_ZOOM && screen_len_px >= LINE_MAJOR_MIN_PX
    } else {
        zoom >= LINE_MINOR_MIN_ZOOM && screen_len_px >= LINE_MINOR_MIN_PX
    }
}

fn area_name_allowed(zoom: f64, area_m2: f64, px_per_meter: f64) -> bool {
    zoom >= AREA_NAME_MIN_ZOOM && area_m2 * px_per_meter * px_per_meter >= AREA_NAME_MIN_PX2
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_store::DataSet;

    #[test]
    fn layer_tag_is_parsed_and_clamped() {
        let mut data = DataSet::new();
        let key = data.intern("layer");
        let value = data.intern("99");
        let tags = vec![(key, value)];
        assert_eq!(way_layer(&data, &tags), 5);
        let value = data.intern("-2");
        let tags = vec![(key, value)];
        assert_eq!(way_layer(&data, &tags), -2);
        assert_eq!(way_layer(&data, &[]), 0);
    }

    #[test]
    fn waterways_and_oneway_highways_get_a_direction() {
        let mut data = DataSet::new();
        let highway = data.intern("highway");
        let waterway = data.intern("waterway");
        let oneway = data.intern("oneway");
        let yes = data.intern("yes");
        let reverse = data.intern("-1");
        let no = data.intern("no");
        let stream = data.intern("stream");
        assert_eq!(way_arrow_forward(&data, &[(waterway, stream)], false), Some(true));
        assert_eq!(way_arrow_forward(&data, &[(waterway, stream)], true), None);
        assert_eq!(
            way_arrow_forward(&data, &[(highway, yes), (oneway, yes)], false),
            Some(true)
        );
        assert_eq!(
            way_arrow_forward(&data, &[(highway, yes), (oneway, reverse)], false),
            Some(false)
        );
        assert_eq!(
            way_arrow_forward(&data, &[(highway, yes), (oneway, no)], false),
            None
        );
        assert_eq!(way_arrow_forward(&data, &[(oneway, yes)], false), None);
    }

    #[test]
    fn line_names_wait_until_the_road_is_long_enough() {
        assert!(!line_name_allowed(false, 17.0, 400.0));
        assert!(!line_name_allowed(false, 18.0, 100.0));
        assert!(line_name_allowed(false, 18.0, 160.0));
        assert!(line_name_allowed(true, 16.5, 200.0));
        assert!(!line_name_allowed(true, 16.0, 500.0));
    }

    #[test]
    fn areas_smaller_than_the_band_are_filled_solid() {
        let small = square(0.0, 10.0);
        let large = square(0.0, 100.0);
        assert!(area_band_geometry(&small, 1.0, 20.0).is_none());
        assert!(area_band_geometry(&large, 1.0, 20.0).is_some());
        assert!(area_band_geometry(&small, 5.0, AREA_BAND_PX / 5.0).is_some());
    }

    fn square(min: f64, max: f64) -> Vec<WorldPoint> {
        vec![
            WorldPoint::new(min, min),
            WorldPoint::new(max, min),
            WorldPoint::new(max, max),
            WorldPoint::new(min, max),
        ]
    }

    #[test]
    fn large_areas_label_before_small_ones() {
        let ppm = 256.0 * 32_768.0 / 40_075_017.0;
        assert!(area_name_allowed(15.0, 80_000.0, ppm));
        assert!(!area_name_allowed(15.0, 1_000.0, ppm));
        assert!(!area_name_allowed(12.0, 1.0e8, ppm));
    }

    #[test]
    fn explicit_text_color_wins_over_feature_color() {
        assert_eq!(
            name_color(Some([1, 2, 3, 9]), [9, 9, 9, 255], true),
            [1, 2, 3, 255]
        );
        assert_eq!(name_color(None, [10, 20, 30, 128], false), [10, 20, 30, 255]);
        let lifted = name_color(None, [0, 0, 0, 255], true);
        assert!(lifted[0] > 100);
    }
}
