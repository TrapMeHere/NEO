//! Render batches and scene frames (CPU side, no GPU types at all).
//!
//! Layering principle (ADR-0004): `neo-scene` only produces "vertices + indices + colors"; only `neo-render` turns
//! them into wgpu buffers. This way swapping the render backend does not require changing scene assembly, and tests need no GPU.

use bytemuck::{Pod, Zeroable};
use neo_core::{ObjectId, ObjectKind};
use neo_geo::{Camera, WorldPoint};

/// Map vertex (12 bytes, directly mappable to the GPU).
///
/// `position` is **WebMercator meter coordinates relative to the batch origin (`SceneFrame::origin`)** (f32).
/// Relative coordinates rather than lon/lat: lon/lat lacks precision in f32, whereas relative meter coordinates within the 4 km rebasing range
/// have errors far below 1 pixel (see docs/04 §2.1).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct MapVertex {
    /// Position relative to the batch origin (meters).
    pub position: [f32; 2],
    /// RGBA, not premultiplied.
    pub color: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<MapVertex>() == 12,
    "MapVertex must stay 12 bytes to keep GPU bandwidth low"
);

impl MapVertex {
    /// Construct.
    #[inline]
    pub fn new(position: [f32; 2], color: [u8; 4]) -> Self {
        Self { position, color }
    }
}

/// Batch type, corresponding to a render pipeline's blend/topology strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BatchKind {
    /// Area fill (buildings, water).
    #[default]
    Fill,
    /// Structure outline (bridge outlines, tunnel dashed outlines, water surfaces on fords).
    ///
    /// Same topology as `Stroke`, but **must** come before the main line: the outline is wider than the line and must not be covered by
    /// other objects' main lines in the same layer.
    Casing,
    /// Line stroke (roads, railways, boundaries).
    Stroke,
    /// Point markers (nodes).
    Point,
}

/// One draw batch: a set of triangles sharing kind and material.
#[derive(Clone, Debug, Default)]
pub struct RenderBatch {
    /// Batch type.
    pub kind: BatchKind,
    /// Vertices.
    pub vertices: Vec<MapVertex>,
    /// Triangle indices.
    pub indices: Vec<u32>,
}

impl RenderBatch {
    /// Create an empty batch.
    pub fn new(kind: BatchKind) -> Self {
        Self {
            kind,
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Preallocate.
    pub fn with_capacity(kind: BatchKind, vertices: usize, indices: usize) -> Self {
        Self {
            kind,
            vertices: Vec::with_capacity(vertices),
            indices: Vec::with_capacity(indices),
        }
    }

    /// Whether it is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Vertex count.
    #[inline]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Triangle count.
    #[inline]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Add one independent triangle.
    #[inline]
    pub fn push_triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        self.vertices.push(MapVertex::new(a, color));
        self.vertices.push(MapVertex::new(b, color));
        self.vertices.push(MapVertex::new(c, color));
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// Add a batch of "vertex array + local indices" geometry.
    pub fn push_indexed(&mut self, positions: &[[f32; 2]], local_indices: &[u32], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(positions.iter().map(|p| MapVertex::new(*p, color)));
        self.indices.extend(local_indices.iter().map(|i| base + *i));
    }

    /// Clear (keeping capacity so it can be reused next frame).
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }
}

/// Fallback label color when the feature has no color. Normally the name follows the feature color.
pub const NAME_COLOR: [u8; 4] = [34, 34, 34, 255];

/// Label color. If the style specifies `text-color`, use it; otherwise follow the feature color.
/// `lift` brightens an area feature's line color toward white by one step so it does not sit flat on the fill.
pub fn name_color(explicit: Option<[u8; 4]>, feature: [u8; 4], lift: bool) -> [u8; 4] {
    if let Some(color) = explicit {
        return [color[0], color[1], color[2], 255];
    }
    if !lift {
        return [feature[0], feature[1], feature[2], 255];
    }
    let mix = |channel: u8| ((u16::from(channel) * 2 + 255 * 3) / 5) as u8;
    [mix(feature[0]), mix(feature[1]), mix(feature[2]), 255]
}

/// How the label anchors relative to the geometry. Converted to a screen anchor at draw time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabelPlace {
    /// Area: the text center is at the area centroid, written horizontally.
    Center,
    /// Line: the text center is at the length midpoint, `angle` is the screen clockwise angle already flipped upright.
    Along {
        /// Readable direction, range (-π/2, π/2].
        angle: f32,
    },
    /// Point: the left-middle of the text is to the right of the symbol, and `gap_px` is the distance from the symbol center.
    Right {
        /// Screen distance from the symbol center to the left edge of the text.
        gap_px: f32,
    },
}

/// Text label in the scene (drawn by the UI overlay, does not enter GPU vertex batches).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneLabel {
    /// World anchor. Center for areas and lines, symbol center for points.
    pub pos: WorldPoint,
    /// Placement relative to the geometry.
    pub place: LabelPlace,
    /// Text.
    pub text: String,
    /// RGBA color.
    pub color: [u8; 4],
    /// Font size (screen points).
    pub size_px: f32,
    /// Larger values win position priority. Areas use world area; lines and points use smaller constants.
    pub rank: f32,
    /// Word-wrap width (screen points).
    pub wrap_px: f32,
}

/// Icon in the scene (drawn by the UI overlay).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneIcon {
    /// World coordinates.
    pub pos: WorldPoint,
    /// Icon path relative to `assets/`.
    pub image: String,
    /// Screen width (points).
    pub width_px: f32,
    /// Icon color. Image icons ignore it; `poi:` icons use it to fill the base.
    pub color: [u8; 4],
}

/// Fill of one area. When large enough it is an inset inner ring; when the ring does not fit on screen it is a solid area.
#[derive(Clone, Debug)]
pub struct AreaBand {
    /// The area is a closed way, or a multipolygon relation.
    pub kind: ObjectKind,
    /// Object ID.
    pub id: ObjectId,
    /// Outer ring (world coordinates).
    pub outer: Vec<WorldPoint>,
    /// Holes.
    pub holes: Vec<Vec<WorldPoint>>,
    /// Start point inward from the boundary (meters). The outer edge of the inner ring hugs the area, so this is 0 here.
    pub inset_m: f64,
    /// Ring width (meters). Unused for solid fill.
    pub width_m: f64,
    /// At the current zoom this area cannot fit one inner ring, so it is drawn solid and clicking anywhere inside selects it.
    pub solid: bool,
}

/// One scene frame.
///
/// `version` corresponds to `DataSet::version()`; camera pan/zoom **do not need to rebuild** `SceneFrame`
/// (the GPU applies the transform via a uniform); it is rebuilt only when data changes or origin rebasing is needed.
#[derive(Clone, Debug)]
pub struct SceneFrame {
    /// Data version it was built from.
    pub version: u64,
    /// Camera at build time (pan/zoom are overwritten by the current camera uniform at render time).
    pub camera: Camera,
    /// Origin of vertex coordinates (world-coordinate meters).
    pub origin: WorldPoint,
    /// Batch list.
    pub batches: Vec<RenderBatch>,
    /// Text labels.
    pub labels: Vec<SceneLabel>,
    /// Icons.
    pub icons: Vec<SceneIcon>,
    /// Inner rings of areas, for click hit testing. The geometry has already been drawn into `batches`.
    pub area_bands: Vec<AreaBand>,
    /// Statistics.
    pub stats: SceneStats,
}

impl SceneFrame {
    /// Empty scene.
    pub fn empty(version: u64, camera: Camera) -> Self {
        Self {
            version,
            origin: camera.center,
            camera,
            batches: Vec::new(),
            labels: Vec::new(),
            icons: Vec::new(),
            area_bands: Vec::new(),
            stats: SceneStats::default(),
        }
    }

    /// Total vertex count.
    pub fn total_vertices(&self) -> usize {
        self.batches.iter().map(|b| b.vertices.len()).sum()
    }

    /// Total triangle count.
    pub fn total_triangles(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }

    /// Whether the camera needs origin rebasing.
    pub fn needs_rebase(&self, camera: &Camera) -> bool {
        camera.needs_rebase(self.origin)
    }

    /// Whether the scene is empty.
    pub fn is_empty(&self) -> bool {
        self.batches.iter().all(|b| b.is_empty())
    }
}

/// Scene-build statistics (for the status bar and performance benchmarks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    /// Number of ways actually drawn.
    pub ways: usize,
    /// Number of nodes actually drawn.
    pub nodes: usize,
    /// Number of ways skipped by LOD/culling.
    pub skipped: usize,
    /// Total vertices.
    pub vertices: usize,
    /// Total triangles.
    pub triangles: usize,
    /// Build duration (microseconds).
    pub build_micros: u64,
    /// Whether it was truncated because of the vertex cap.
    pub clamped: bool,
}

impl core::fmt::Display for SceneStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "ways={} nodes={} skipped={} verts={} tris={} build={}us{}",
            self.ways,
            self.nodes,
            self.skipped,
            self.vertices,
            self.triangles,
            self.build_micros,
            if self.clamped { " CLAMPED" } else { "" }
        )
    }
}
