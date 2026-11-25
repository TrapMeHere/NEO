//! Scene assembly: viewport → visible objects → vertex batches.
//!
//! Key performance principles (docs/04 §2.9):
//!
//! * **draw volume is determined by the viewport, not by the total object count**: cull via the spatial index first, then apply LOD;
//! * geometry is generated on the CPU, but only the part that is "visible";
//! * pan/zoom do not rebuild geometry (the GPU applies the transform via a uniform); rebuild only when data changes or origin rebasing is needed;
//! * vertex coordinates = world coordinates - batch origin (f32 meters), with the origin taken at the camera center.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Instant;

use neo_core::{Handle, ObjectKind};
use neo_geo::{Camera, SpatialIndex, WorldCache, WorldPoint, geometry};
use neo_store::DataSet;
use neo_style::{Colors, HachureSide, MapCss, StyleKeys, StyleSheet};

use crate::batches::{
    AreaBand, BatchKind, LabelPlace, MapVertex, RenderBatch, SceneFrame, SceneIcon, SceneLabel, SceneStats,
    name_color,
};

/// Geometry scratch buffers reused every frame (avoiding a heap allocation per way).
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
    offset: Vec<WorldPoint>,
}

/// Maximum number of points before RDP: above this, thin out proportionally first.
/// RDP can degrade to O(n²) on pathological polylines (2000+ nodes with fragmented shape); pre-thinning pins the worst case.
const RDP_MAX_INPUT: usize = 256;

/// Draw batches bucketed by OSM `layer`; submitted in ascending layer order, so larger layers stack on top.
///
/// The per-layer submission order is fixed at fill → casing → stroke → point:
/// * `casing` is the structure outline (bridges/tunnels/water surfaces on fords) and must sit below the main line;
/// * `point` comes last so nodes are not covered by lines.
struct LayerBatches {
    fill: RenderBatch,
    casing: RenderBatch,
    stroke: RenderBatch,
    point: RenderBatch,
}

impl Default for LayerBatches {
    fn default() -> Self {
        Self {
            fill: RenderBatch::with_capacity(BatchKind::Fill, 8192, 16384),
            // Bridges/tunnels are only a small fraction of features, so the capacity is an order of magnitude smaller than stroke.
            casing: RenderBatch::with_capacity(BatchKind::Casing, 4096, 8192),
            stroke: RenderBatch::with_capacity(BatchKind::Stroke, 65536, 131072),
            point: RenderBatch::with_capacity(BatchKind::Point, 4096, 8192),
        }
    }
}

fn total_layer_vertices(layers: &BTreeMap<i16, LayerBatches>) -> usize {
    layers
        .values()
        .map(|batch| {
            batch.fill.vertices.len()
                + batch.casing.vertices.len()
                + batch.stroke.vertices.len()
                + batch.point.vertices.len()
        })
        .sum()
}

/// Drawing layer of a way.
///
/// An explicit `layer` wins (consistent with JOSM); otherwise take a `bias` from structure semantics (bridge +1, tunnel -1),
/// else 0. Thus a road on a bridge is not pressed down by an intersecting road below, and a tunnel does not cover the surface road, without relying on traversal order.
fn way_layer(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)], bias: i16) -> i16 {
    tags.iter()
        .find(|(key, _)| data.resolve(*key) == "layer")
        .and_then(|(_, value)| data.resolve(*value).trim().parse::<i16>().ok())
        .unwrap_or(bias)
        .clamp(-5, 5)
}

fn resolved_tag_names(data: &DataSet, tags: &[(neo_core::Sym, neo_core::Sym)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| (data.resolve(*key).to_owned(), data.resolve(*value).to_owned()))
        .collect()
}

/// Below this zoom nodes are not drawn (z16 and below show no nodes).
pub const NODE_MIN_ZOOM: f64 = 17.0;
/// At this zoom all nodes are drawn; between 17 and 18 only shared nodes are drawn.
pub const NODE_ALL_ZOOM: f64 = 19.0;
/// Shared-node criterion: referenced by at least this many ways.
pub const SHARED_NODE_MIN_WAYS: usize = 2;
/// Node icons and names are drawn only from this zoom on. Vertices themselves still follow the 17/19 rules.
const NODE_ICON_MIN_ZOOM: f64 = 21.0;
/// Earliest zoom at which area names appear. Earlier than this only whole continents remain, and names have no reference.
const AREA_NAME_MIN_ZOOM: f64 = 13.0;
/// An area must be at least this large on screen (square pixels) to get a name. Larger areas reach it earlier.
const AREA_NAME_MIN_PX2: f64 = 2_800.0;
/// Zoom at which trunk roads (line width ≥ 3px) get names. Ordinary roads are later, to avoid crowding thin lines.
const LINE_MAJOR_MIN_ZOOM: f64 = 16.5;
const LINE_MINOR_MIN_ZOOM: f64 = 18.0;
/// A line shorter than this on screen gets no name.
const LINE_MAJOR_MIN_PX: f64 = 200.0;
const LINE_MINOR_MIN_PX: f64 = 160.0;
/// Maximum number of labels kept for one frame. When full, the lowest-priority one is replaced.
const LABEL_KEEP: usize = 700;
/// Inner-ring width of areas (screen pixels). The outer edge hugs the area boundary and the ring is laid inward with this width.
const AREA_BAND_PX: f64 = 20.0;
/// Direction arrows for waterways and oneways: place one every this distance along the screen.
const ARROW_SPACING_PX: f64 = 80.0;
const ARROW_LENGTH_PX: f64 = 12.0;
const ARROW_HALF_WIDTH_PX: f64 = 4.5;

/// Whether a node is visible/pickable at a given zoom (rendering and picking share the same rule).
///
/// * `zoom < NODE_MIN_ZOOM(17)`: invisible;
/// * `17 <= zoom < NODE_ALL_ZOOM(19)`: only shared nodes (referenced by ≥2 ways) are visible;
/// * `zoom >= 19`: all are visible.
pub fn node_visible_at(zoom: f64, way_ref_count: usize) -> bool {
    if zoom < NODE_MIN_ZOOM {
        false
    } else if zoom < NODE_ALL_ZOOM {
        way_ref_count >= SHARED_NODE_MIN_WAYS
    } else {
        true
    }
}

/// Scene assembly configuration.
#[derive(Clone, Debug)]
pub struct SceneConfig {
    /// Polyline simplification tolerance (screen pixels).
    pub simplify_tolerance_px: f64,
    /// Viewport expansion ratio (to avoid edge objects popping in suddenly).
    pub cull_margin: f64,
    /// Per-frame vertex cap (safety valve against pathological data locking up).
    pub max_vertices: usize,
    /// Background color (read by the UI, not stored in vertices).
    pub background: [u8; 4],
    /// Way sampling stride. `1` means all ways in the viewport enter geometry;
    /// values greater than 1 keep every Nth way (distant LOD / streaming overview).
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

/// Scene builder (reusable across frames with fixed style and config).
pub struct SceneBuilder {
    /// Stylesheet.
    pub style: StyleSheet,
    /// Configuration.
    pub config: SceneConfig,
    /// Reusable buffers.
    scratch: RefCell<Scratch>,
}

impl SceneBuilder {
    /// Construct.
    pub fn new(style: StyleSheet, config: SceneConfig) -> Self {
        Self {
            style,
            config,
            scratch: RefCell::new(Scratch::default()),
        }
    }

    /// Build the default style using a symbol interner supplied by the caller.
    ///
    /// The editor keeps names in its own `DataSet`, so the scene layer no longer borrows a mutable dataset.
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

    /// Build the default style for a dataset (interning the tag keys into this data).
    pub fn with_dataset(data: &mut DataSet) -> Self {
        Self::with_intern(|name| data.intern(name))
    }

    /// Build one scene frame.
    ///
    /// Node LOD: `zoom < 17` draws none (z16 and below show nothing); `17..19` draws only shared nodes
    /// (referenced by ≥2 ways, small dots); `zoom >= 19` draws all nodes, with shared nodes as small squares and
    /// ordinary nodes as dots. Node icons and names only appear at zoom 21, with the name written to the right of the icon.
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

        // 1) viewport culling (expanded by cull_margin).
        let mut bounds = camera.visible_bounds();
        let (w, h) = bounds.size();
        bounds = bounds::expand(bounds, w * self.config.cull_margin, h * self.config.cull_margin);

        // 2) bucketing (preallocated to avoid repeated growth and copying for large scenes).
        let mut layers: BTreeMap<i16, LayerBatches> = BTreeMap::new();
        let mut labels = Vec::new();
        let mut icons = Vec::new();
        let mut areas: Vec<PendingArea> = Vec::new();

        // 3) allocation-free traversal: feed geometry construction directly through the rstar iterator.
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
        // 3b) multipolygon: both outer rings and holes are recorded as areas; inner rings are drawn later.
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

        // 3) empty batches are not submitted.
        let mut batches: Vec<RenderBatch> = Vec::new();
        for (_, batch) in layers {
            if !batch.fill.is_empty() {
                batches.push(batch.fill);
            }
            if !batch.casing.is_empty() {
                batches.push(batch.casing);
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
        let layer = way_layer(data, view.tag_pairs, 0);
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
        // LOD pre-culling (using index bboxes before geometry generation) + density sampling: when zoomed far out, sample by stride
        // to keep the road network structure while holding the number of ways rebuilt per frame in a controllable range.
        let (bw, bh) = bounds.size();
        let screen_diag = ((bw * bw + bh * bh).sqrt() * camera.px_per_meter()) as f32;
        let min_px = self.style.min_way_length_px;
        if screen_diag < min_px {
            // Density LOD: the target sample count is proportional to the square of "below-target pixel count", keeping the number of ways
            // actually drawn per screen area roughly constant (the standard vector-tile approach), rather than cutting a whole feature class.
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
            offset,
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
        // Prefer node-reference closure; fall back to world-coordinate first/last equality (still drawable when nodes are missing).
        let closed = view.is_closed() || (world.len() >= 4 && world[0] == world[world.len() - 1]);
        let layer = way_layer(data, tags, way_style.layer_bias);
        if let Some(label_key) = way_style.text.as_deref().or(Some("name"))
            && let Some((_, value)) = tags.iter().find(|(key, _)| data.resolve(*key) == label_key)
        {
            let text = data.resolve(*value);
            // Closed area rings use centroid labels; purely closed roads still follow the line, avoiding a half-perimeter landing on the diagonal.
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
        // Record the area first. Draw an inner ring when big enough; switch to solid when it is less than one ring wide on screen.
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

        // Screen-space simplification: convert tolerance to meters; skip RDP for short ways.
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

        // After a successful area fill, still draw the stroke (MapCSS's fill + casing combination).
        let width_px = way_style.width_px;
        if width_px <= 0.0 {
            return;
        }
        let width_m = camera.px_to_meters(width_px);
        let batch = layers.entry(layer).or_default();

        // ① Structure outlines: bridge outlines, tunnel dashed outlines, water surfaces on fords.
        //       A separate batch drawn first (wider than the main line, must not be covered by other main lines in the same layer).
        if let Some(casing) = &way_style.casing
            && casing.width_px > 0.0
        {
            let casing_m = camera.px_to_meters(casing.width_px);
            let mut drawn = false;
            if let Some(pattern) = &casing.dashes {
                drawn = dashed_stroke_into(
                    simplified,
                    casing_m,
                    pattern,
                    camera.meters_per_px(),
                    dash,
                    stroke_verts,
                    stroke_indices,
                );
            }
            if !drawn {
                geometry::stroke_polyline_into(simplified, casing_m, stroke_verts, stroke_indices);
            }
            if !stroke_indices.is_empty() {
                positions.clear();
                positions.extend(stroke_verts.iter().map(|point| rel(*point, origin)));
                batch.casing.push_indexed(positions, stroke_indices, casing.color);
            }
        }

        // ② Embankment/cutting side hachures: drawn along the main line, with short-line colors given by the style layer.
        //       Embankments also get an outer slope-foot line (`baseline`), together forming a terraced slope; cuttings only get the comb-like teeth.
        if let Some(hachure) = &way_style.hachure {
            let sides: &[i8] = match hachure.side {
                HachureSide::Left => &[1],
                HachureSide::Right => &[-1],
                HachureSide::Both => &[1, -1],
            };
            let length_m = camera.px_to_meters(hachure.length_px);
            let width_m = camera.px_to_meters(hachure.width_px);
            if hachure.baseline {
                stroke_verts.clear();
                stroke_indices.clear();
                for &side in sides {
                    geometry::offset_polyline_into(simplified, length_m, side, offset);
                    geometry::stroke_polyline_append(offset, width_m, stroke_verts, stroke_indices);
                }
                if !stroke_indices.is_empty() {
                    positions.clear();
                    positions.extend(stroke_verts.iter().map(|point| rel(*point, origin)));
                    batch
                        .stroke
                        .push_indexed(positions, stroke_indices, hachure.color);
                }
            }
            geometry::hachure_ticks(
                simplified,
                camera.px_to_meters(hachure.spacing_px),
                length_m,
                width_m,
                if sides.len() == 2 { 0 } else { sides[0] },
                stroke_verts,
                stroke_indices,
            );
            if !stroke_indices.is_empty() {
                positions.clear();
                positions.extend(stroke_verts.iter().map(|point| rel(*point, origin)));
                batch
                    .stroke
                    .push_indexed(positions, stroke_indices, hachure.color);
            }
        }

        // ③ Main line: MapCSS dashes take priority, otherwise solid.
        let mut dashed = false;
        if let Some(pattern) = &way_style.dashes {
            dashed = dashed_stroke_into(
                simplified,
                width_m,
                pattern,
                camera.meters_per_px(),
                dash,
                stroke_verts,
                stroke_indices,
            );
        }
        if !dashed {
            geometry::stroke_polyline_into(simplified, width_m, stroke_verts, stroke_indices);
        }
        if stroke_indices.is_empty() {
            return;
        }
        positions.clear();
        positions.extend(stroke_verts.iter().map(|p| rel(*p, origin)));
        batch
            .stroke
            .push_indexed(positions, stroke_indices, way_style.color);
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
                batch
                    .stroke
                    .push_indexed(positions, &arrow_indices, arrow_ink(way_style.color));
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
            // Shared nodes (z16+): small squares (two triangles).
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
            // Ordinary nodes / shared nodes at low zoom: small dots (hexagon approximation, 7 vertices / 6 triangles).
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

/// Stroke along a screen-pixel dash pattern; returns whether at least one segment was actually drawn.
///
/// `pattern_px` is in screen pixels (alternating solid/dash); converted to meters and walked along the polyline. When the pattern has fewer than two segments
/// or converts to 0, nothing is drawn and the caller falls back to a solid line. The result overwrites `verts`/`indices`.
#[allow(clippy::too_many_arguments)]
fn dashed_stroke_into(
    points: &[WorldPoint],
    width_m: f64,
    pattern_px: &[f32],
    meters_per_px: f64,
    dash: &mut Vec<WorldPoint>,
    verts: &mut Vec<WorldPoint>,
    indices: &mut Vec<u32>,
) -> bool {
    verts.clear();
    indices.clear();
    if points.len() < 2 || width_m <= 0.0 {
        return false;
    }
    let pattern_m: Vec<f64> = pattern_px
        .iter()
        .map(|px| f64::from(*px) * meters_per_px)
        .filter(|value| *value > 1e-6)
        .collect();
    if pattern_m.len() < 2 {
        return false;
    }
    let mut drew = false;
    let mut on = true;
    let mut index = 0usize;
    let mut remaining = pattern_m[0];
    for pair in points.windows(2) {
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
                    geometry::stroke_polyline_append(dash, width_m, verts, indices);
                    drew = true;
                }
                dash.clear();
                on = !on;
                index = (index + 1) % pattern_m.len();
                remaining = pattern_m[index];
            }
        }
    }
    if on && dash.len() >= 2 {
        geometry::stroke_polyline_append(dash, width_m, verts, indices);
        drew = true;
    }
    dash.clear();
    drew
}

#[inline]
fn rel(p: WorldPoint, origin: WorldPoint) -> [f32; 2] {
    // Date line: place vertices on the "world copy" nearest the camera origin, so a way crossing 180°
    // does not sweep across the whole map (vertex coordinates are still meters relative to the origin).
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
        let band = area_band_geometry(&area.outer, &area.holes, px_per_meter, width_m);
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

/// Returns a band when the short side can fit the inner ring; otherwise returns `None` and the caller draws solid. Recomputed with the current zoom.
///
/// With holes it also requires every hole to be far from the band's inner boundary: band geometry does not subtract holes, so a hole within the band range would be covered
/// entirely (small inner-courtyard buildings become solid across the whole hole at z≈15.7–16); in that case switch to "solid + hole cutout".
fn area_band_geometry(
    outer: &[WorldPoint],
    holes: &[Vec<WorldPoint>],
    px_per_meter: f64,
    width_m: f64,
) -> Option<(Vec<WorldPoint>, Vec<u32>)> {
    if area_span_px(outer, px_per_meter) < AREA_BAND_PX {
        return None;
    }
    if !geometry::band_clears_holes(outer, holes, width_m) {
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

/// Waterways follow point order (downstream). `highway` with `oneway=yes/1` follows point order, `oneway=-1` the opposite.
/// Waterways already drawn as areas (riverbanks etc.) get no arrows.
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
    use neo_core::{LatLon, Meta, ObjectId};
    use neo_osm::{Node, Tags, Way};
    use neo_store::{DataSet, MissingPolicy};

    #[test]
    fn layer_tag_is_parsed_and_clamped() {
        let mut data = DataSet::new();
        let key = data.intern("layer");
        let value = data.intern("99");
        let tags = vec![(key, value)];
        assert_eq!(way_layer(&data, &tags, 0), 5);
        let value = data.intern("-2");
        let tags = vec![(key, value)];
        assert_eq!(way_layer(&data, &tags, 0), -2);
        assert_eq!(way_layer(&data, &[], 0), 0);
    }

    #[test]
    fn structure_bias_only_applies_without_explicit_layer() {
        let mut data = DataSet::new();
        let layer = data.intern("layer");
        let explicit = data.intern("2");
        assert_eq!(
            way_layer(&data, &[(layer, explicit)], 1),
            2,
            "explicit layer takes precedence over the structure offset"
        );
        assert_eq!(way_layer(&data, &[], 1), 1, "bridge defaults to +1");
        assert_eq!(way_layer(&data, &[], -1), -1, "tunnel defaults to -1");
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
        assert!(area_band_geometry(&small, &[], 1.0, 20.0).is_none());
        assert!(area_band_geometry(&large, &[], 1.0, 20.0).is_some());
        assert!(area_band_geometry(&small, &[], 5.0, AREA_BAND_PX / 5.0).is_some());
    }

    #[test]
    fn donut_with_a_hole_inside_the_band_falls_back_to_solid() {
        // 1 px/m, band width 20px: a 100px outer ring enters band mode, with the inner boundary being the ring 20..80.
        let outer = square(0.0, 100.0);
        // The hole is deep inside: the band does not reach it, so keep using band + hole-edge band.
        assert!(area_band_geometry(&outer, &[square(40.0, 60.0)], 1.0, 20.0).is_some());
        // The hole hugs the outer boundary and falls inside the band: the band would cover it, so fall back to solid with hole cutout.
        assert!(area_band_geometry(&outer, &[square(5.0, 15.0)], 1.0, 20.0).is_none());
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

    // ---- scene assembly for bridges / tunnels / embankments / cuttings / fords ----

    /// Build a horizontal way. Each way has its own nodes, avoiding shared nodes changing the node LOD
    /// and avoiding cross-way node snapping interfering with assertions.
    fn add_way(data: &mut DataSet, id: i64, lat: f64, tags: &[(&str, &str)]) {
        let mut way = Way {
            id: ObjectId::new(id),
            nodes: Default::default(),
            tags: Tags::new(),
            meta: Meta::new_local(),
        };
        for step in 0..2i64 {
            let node_id = ObjectId::new(id * 10 + step);
            data.push_node(Node {
                id: node_id,
                pos: LatLon::from_degrees(lat, 0.0001 + 0.0005 * step as f64),
                tags: Tags::new(),
                meta: Meta::new_local(),
            })
            .expect("node");
            way.nodes.push(node_id);
        }
        for (key, value) in tags {
            let key = data.intern(key);
            let value = data.intern(value);
            way.tags.set(key, value);
        }
        data.push_way(way, MissingPolicy::Error)
            .expect("way")
            .expect("way handle");
    }

    /// Build a scene frame containing only these horizontal ways from a `(latitude, tag)` list.
    fn scene_for(ways: &[(f64, &[(&str, &str)])]) -> SceneFrame {
        let mut data = DataSet::new();
        let builder = SceneBuilder::with_dataset(&mut data);
        for (index, (lat, tags)) in ways.iter().enumerate() {
            add_way(&mut data, index as i64 + 1, *lat, tags);
        }
        let index = SpatialIndex::build(&data);
        let referrers = data.build_referrers();
        let cache = WorldCache::build(&data);
        let camera = Camera::from_latlon(LatLon::from_degrees(0.0003, 0.00035), 18.0, [800, 600]);
        builder.build(&data, &index, &referrers, &cache, &camera)
    }

    fn batch_kinds(frame: &SceneFrame) -> Vec<BatchKind> {
        frame.batches.iter().map(|batch| batch.kind).collect()
    }

    fn stroke_triangles(frame: &SceneFrame) -> usize {
        frame
            .batches
            .iter()
            .filter(|batch| batch.kind == BatchKind::Stroke)
            .map(RenderBatch::triangle_count)
            .sum()
    }

    #[test]
    fn bridge_casing_is_drawn_above_the_crossing_road() {
        let frame = scene_for(&[
            (0.0, &[("highway", "primary")]),
            (0.0002, &[("highway", "primary"), ("bridge", "yes")]),
        ]);
        // Bridges have no explicit layer, so +1 by OSM convention: ground roads are drawn first, then the bridge outline + deck.
        assert_eq!(
            batch_kinds(&frame),
            vec![BatchKind::Stroke, BatchKind::Casing, BatchKind::Stroke]
        );
        assert_eq!(
            frame.batches[1].vertices[0].color,
            Colors::default().bridge_casing
        );
    }

    #[test]
    fn tunnel_casing_is_dashed_and_drawn_below_roads() {
        let frame = scene_for(&[
            (0.0, &[("highway", "primary")]),
            (0.0002, &[("highway", "primary"), ("tunnel", "yes")]),
        ]);
        // Tunnels are drawn first at layer -1: dashed outline, then main line, and only then ground roads.
        assert_eq!(
            batch_kinds(&frame),
            vec![BatchKind::Casing, BatchKind::Stroke, BatchKind::Stroke]
        );
        let casing = &frame.batches[0];
        assert_eq!(casing.vertices[0].color, Colors::default().tunnel_casing);
        assert!(
            casing.triangle_count() > 2,
            "the tunnel outline must be several dashed segments, not one continuous band"
        );
    }

    #[test]
    fn ford_gets_a_water_casing() {
        let frame = scene_for(&[(0.0, &[("highway", "residential"), ("ford", "yes")])]);
        assert_eq!(batch_kinds(&frame), vec![BatchKind::Casing, BatchKind::Stroke]);
        assert_eq!(frame.batches[0].vertices[0].color, Colors::default().ford_water);
    }

    #[test]
    fn plain_ways_have_no_casing() {
        let frame = scene_for(&[(0.0, &[("highway", "primary")])]);
        assert!(!frame.batches.iter().any(|batch| batch.kind == BatchKind::Casing));
    }

    fn stroke_has_color(frame: &SceneFrame, want: [u8; 4]) -> bool {
        frame
            .batches
            .iter()
            .filter(|batch| batch.kind == BatchKind::Stroke)
            .flat_map(|batch| batch.vertices.iter())
            .any(|vertex| vertex.color == want)
    }

    #[test]
    fn embankment_and_cutting_use_distinct_slope_symbols() {
        let plain = scene_for(&[(0.0, &[])]);
        let embankment = scene_for(&[(0.0, &[("man_made", "embankment")])]);
        let cutting = scene_for(&[(0.0, &[("man_made", "cutting")])]);
        let plain_tris = stroke_triangles(&plain);
        assert!(
            stroke_triangles(&embankment) > plain_tris,
            "embankment must draw slope hachures"
        );
        assert!(
            stroke_triangles(&cutting) > plain_tris,
            "cutting must draw slope hachures"
        );
        // The two symbols use different colors: embankments gold (with a slope-foot line), cuttings brick red (comb-like teeth).
        let colors = Colors::default();
        assert!(stroke_has_color(&embankment, colors.embankment));
        assert!(!stroke_has_color(&embankment, colors.cutting));
        assert!(stroke_has_color(&cutting, colors.cutting));
        assert!(!stroke_has_color(&cutting, colors.embankment));
    }

    #[test]
    fn link_roads_keep_their_parent_colour_but_are_thinner() {
        // Verified with the default stylesheet (assets/styles/neo.mapcss): *_link is the same color as its parent and thinner.
        let mut data = DataSet::new();
        let builder = SceneBuilder::with_dataset(&mut data);
        let key = data.intern("highway");
        for (parent, link) in [
            ("motorway", "motorway_link"),
            ("trunk", "trunk_link"),
            ("primary", "primary_link"),
            ("secondary", "secondary_link"),
            ("tertiary", "tertiary_link"),
        ] {
            let parent_tags = [(key, data.intern(parent))];
            let link_tags = [(key, data.intern(link))];
            let parent_style = builder
                .style
                .way_style(&parent_tags, 18.0, &[("highway", parent)]);
            let link_style = builder.style.way_style(&link_tags, 18.0, &[("highway", link)]);
            assert_eq!(
                link_style.color, parent_style.color,
                "{link} color should match {parent}"
            );
            assert_eq!(
                link_style.text_color, parent_style.text_color,
                "{link} text color should match {parent}"
            );
            assert!(
                link_style.width_px < parent_style.width_px,
                "{link} should be thinner than {parent} ({} vs {})",
                link_style.width_px,
                parent_style.width_px
            );
        }
    }
}
