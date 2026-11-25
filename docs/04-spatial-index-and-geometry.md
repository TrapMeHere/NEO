# 04 Spatial Index and Geometry Construction

> Status: draft v0.1 | Date: 2026-11-15 | Owning crates: `neo-geo`, `neo-scene`
> Related: [01-system-architecture.md](01-system-architecture.md) | [02-workspace-and-dependency-rules.md](02-workspace-and-dependency-rules.md) | [05-renderer-wgpu.md](05-renderer-wgpu.md) | [11-style-presets-mapcss.md](11-style-presets-mapcss.md) | [12-validation.md](12-validation.md) | [ADR-0005](adr/0005-wgpu-egui-shared-device.md) | [14-performance-and-verification.md](14-performance-and-verification.md)

## Summary

- NEO's geometry layer has two stages: `neo-geo` (projection, geometry generation, indexing, picking, **no GPU dependency**) and `neo-scene` (visible set, LOD, batch assembly `SceneFrame`/`MapVertex`, **no GPU dependency**).
- The coordinate stack has four levels: `LonLat` (WGS84 degrees) → `WorldPoint` (projection plane, EPSG:3857 meters by default, f64) → **rebasing** (f64→f32, origin aligned to the 2^16 m grid) → `Screen` (physical pixels).
- The precision strategy is "store in f64, draw in f32, rebase per viewport": the f32 ulp at the world extreme of ±2.0e7 m is about 2 m, which would make a building at zoom level 19 jitter by a dozen pixels; after rebasing to ≤1e5 m the error drops to millimeter level.
- Draw volume is determined by the **viewport**, not by the total object count: the object R-tree only produces candidates in `O(log n + k)`, after which everything is controlled by the visible set, LOD, and batches, so 1M and 10M objects cost the same order of work in the same viewport.
- Two-tier indexing: **object tier** (rstar 0.14 `RTree<ObjEntry>`, Node points / precomputed Way AABBs / derived Relation bboxes) + **viewport tier** (256 px screen-space buckets, rebuilt with the visible set, serving hover and precise picking).
- Both indexes and geometry implement `RTreeObject`/`PointDistance` with NEO's own types; **geo 0.33.1's rstar interop is not enabled** (see §2.8, §5).
- Area geometry goes through earcutr 0.5.0 (the correct choice for flat fills); failure never panics: it degrades to stroking and reports self-intersection to the Validator.
- Hit testing has both CPU and GPU implementations, with CPU as the default (the index already exists); the GPU ID buffer is a fallback for dense regions/polygon interiors and is off by default (see [05-renderer-wgpu.md](05-renderer-wgpu.md) §2.10).

## 1 Goals and Scope

### 1.1 Goals

1. On full data layers of 1M–10M+ OSM objects, geometry generation and picking within the viewport must not degrade linearly with the total count.
2. Fully cover the geometry semantics needed for JOSM's workflows: closure detection, multipolygon/inner rings, `area=no`, unclosed multipolygons, and relation-member-order-independent bboxes.
3. Support multiple projections (EPSG:3857 by default, switchable to EPSG:4326 and other CRSs provided by proj 0.32), and projection switches must not rebuild the index.
4. Picking behavior consistent with JOSM: node takes priority over way, way over area; tolerance is expressed in screen pixels and varies with zoom; box selection is a pure geometric selection unaffected by render LOD and picking priority.
5. Provide `neo-render` with **quantized, directly uploadable** vertex/index batches; the render layer must never query back into the data model.

### 1.2 Non-goals

- No topological editing or command merging (that belongs to `neo-editor`/`neo-store`); the geometry layer only consumes immutable `WayView`/`RelationView` snapshots.
- No style semantics (MapCSS evaluation belongs to `neo-style`); the geometry layer only receives "already-resolved drawing parameters": width, color, dash pattern, whether to fill.
- No normal mapping, extrusion modeling, or true 3D buildings (a Phase 2 topic, see [05](05-renderer-wgpu.md) §2.7).

### 1.3 Layering and Dependency Direction

| crate / module | Responsibility | Allowed dependencies | Explicitly prohibited |
|---|---|---|---|
| `neo-geo::projection` | Projection, tiles, camera, coordinate transforms | `proj` 0.32, `geo` 0.33.1 (geometry algorithms only) | Any GPU/egui types |
| `neo-geo::geometry` | Polyline/area construction, simplification, triangulation, normals | `earcutr` 0.5, `geo` 0.33.1 | Indexes, styles |
| `neo-geo::index` | Object R-tree, segment index, dirty queue | `rstar` 0.14 | `neo-store` (takes snapshots only) |
| `neo-geo::hit` | Hit testing and priority | The three above | GPU |
| `neo-scene::visible` | Visible set, LOD, screen-space buckets | `neo-geo`, `neo-style` (style rule results only) | wgpu |
| `neo-scene::frame` | `SceneFrame`/`RenderBatch`/`MapVertex` | `bytemuck` 1.25 | wgpu |

> Dependencies flow one way: `store → editor → geo → scene → render`. `neo-render` must not depend on `neo-store`/`neo-editor`/`neo-ui` (see [ADR-0005](adr/0005-wgpu-egui-shared-device.md)).

## 2 Design

### 2.1 Coordinate Stack

| Level | Type | Unit | Precision requirement | Owner | Lifetime |
|---|---|---|---|---|---|
| L0 `LonLat` | `{lon: f64, lat: f64}` | degrees (WGS84) | Storage layer uses E7 fixed point (i32, 1e-7°≈1.1 cm), bit-aligned with PBF `granularity=100` | `neo-osm` | permanent |
| L1 `WorldPoint` | `{x: f64, y: f64}` | projection plane units (meters for 3857) | f64, error <1e-9 m | `neo-geo::projection` | cached |
| L2 `Rebased` | `[f32; 2]` | meters, relative to `rebase_origin` | ≤1e-2 m after rebasing | `neo-scene::frame` | keyed by `version` |
| L3 `Screen` | `[f32; 2]` | physical pixels | 0.5 px | `neo-render` (vertex-shader affine) | per frame |

`MapVertex.pos` carries **L2**, not L3. The CPU performs projection and rebasing (geometry-related, recomputed only when `version` changes), and the GPU performs the per-frame affine with scale/offset from a uniform. This is the concrete split for "CPU does coordinate transforms, GPU does batch compositing": it neither moves geometry onto the GPU nor rewrites vertices every frame.

### 2.2 WebMercator and Fixed-Point Precision

```
Forward (R = 6378137):
  x = R * λ                                   λ in radians
  y = R * asinh(tan φ) = R * ln(tan(π/4 + φ/2))
Inverse:
  φ = 2 * atan(exp(y / R)) - π/2
  λ = x / R
World extent: ±20037508.342789244 m (i.e. ±R*π), valid latitude domain ±85.05112878°
```

Precision budget (this is the selection rationale, not measured performance):

| Quantity | Usable f32 precision | Conclusion |
|---|---|---|
| ulp at the world extreme of 2.0e7 m | 2^(24-23) = **2 m** | World coordinates cannot be stored directly |
| ulp at 1.0e5 m after rebasing | 2^(16-23) ≈ **7.8 mm** | Sufficient for level 19 (about 0.18 ground meters/px) |
| f64 ulp at 2.0e7 m | ≈3.7e-9 m | Plenty for projection/index layers |

Rebasing rule: `rebase_origin = round(viewport_center / 65536 m) * 65536 m`. While the camera pans within the same 64 km grid, `MapVertex` is untouched; only crossing a cell triggers `version += 1` and rewrites vertices. This way "dragging the map" updates just one uniform in the vast majority of frames, instead of re-uploading vertices.

### 2.3 Multiple Projections and proj 0.32

JOSM supports EPSG:3857 (default), EPSG:4326, UTM zones, Gauss-Krüger, Swiss LV95/LV03, etc., together with its own EPSG definition library and on-demand NTv2 `.gsb` grids. NEO's corresponding strategy:

| Projection family | Use | Implementation | Notes |
|---|---|---|---|
| WebMercator (3857) | Default rendering, tile imagery | Hand-written (§2.2) | No external dependency, works offline; conformal |
| Geographic (4326) | Lon/lat display, debugging, `project` subcommand | Identity + degrees→meters decoration | Not conformal; line width needs `scale_factor` compensation |
| Other EPSG (UTM/GK/LV95/…) | JOSM migrators, national coordinate systems | `proj` 0.32 + PROJ runtime | CRS strings come from config, see below |

```rust
// neo-geo/src/projection/proj_bridge.rs (signatures follow the proj 0.32 docs)
let ctx = proj::Proj::new_known_crs("EPSG:4326", "EPSG:32633", None)?; // > To be verified: exact constructor name in proj 0.32
let p: (f64, f64) = ctx.convert((lon, lat))?;
```

- The **PROJ runtime** (libproj / proj.db) is an external asset distributed with the installer rather than compiled into the binary; when missing, it degrades to "3857/4326 only" and grays out projection options — never panics.
- NTv2 `.gsb` grids have separate terms of use (JOSM also downloads them on demand); NEO does not download or bundle them by default and only tells the user to place them manually. **> To be verified**: the redistribution terms of `.gsb` and PROJ's default lookup paths.
- Projection switches do not rebuild the index: the index stores `WorldPoint` (in the current projection); when the projection changes, AABBs are recomputed in the background under the new projection and swapped via double buffering, while the old model keeps rendering (see the rebuild strategy in §2.8).

### 2.4 Camera, Zoom, and Tiles (`z/x/y`)

```rust
World width W = 40075016.685578488 m
px_per_world_m = 2^zoom * 256.0 / W              // 256 px tile baseline
world_m_per_px = W / (2^zoom * 256.0)
ground_m_per_px = world_m_per_px * cos(lat)      // inverse of the Mercator scale factor 1/cos φ
Tile z/x/y: n = 2^z; x = floor((lon+180)/360 * n)
      y = floor((1 - asinh(tan φ)/π) / 2 * n)
```

- `zoom` is a continuous f64 (the wheel can stop at fractional values); LOD switches only when crossing **integer levels ±0.01** (§2.9), avoiding jittery recomputation while scrolling.
- Viewport queries use `view_bounds()`: the screen rectangle is unprojected into a projection-plane AABB, then expanded by **one tile (≤256 m or the current LOD threshold)** to offset overflow from screen-space line width and labels.
- Tile numbering is used only for the imagery layer and `bounds` alignment; vector objects are not split by tile (objects span tiles), which differs from raster map libraries.

### 2.5 Way → polyline

1. **Fetch references**: take the member node snapshots from `store` (a missing node → record `incomplete`, split the polyline into multiple parts, do not guess).
2. **Project**: `to_world` point by point into `Vec<[f64;2]>`; use the hand-written fast path for 3857 and proj for other projections.
3. **Simplify (screen space)**: Douglas–Peucker with epsilon = `0.5 px * world_m_per_px`, performed **on the projection plane** (not in lon/lat, otherwise the error at mid-latitudes is amplified by cos φ). Simplification results are tied to zoom: they are recomputed only when the LOD level changes.
4. **Closure detection**: `closed = pts.len() >= 4 && first == last` (compared by index to 1e-9, not by absolute distance); closed **does not** equal area: it also requires `area != 0`, `area=no` not set, and not a linear feature such as `natural=coastline`. JOSM's MapCSS `area` selector even matches unclosed ways; the semantic difference list is in [11](11-style-presets-mapcss.md) §2.5.
5. **Arc length and dash phase**: precompute `cum_len`; dashes are taken modulo arc length in the fragment shader, avoiding CPU segment splitting.
6. **Throttling**: geometry rebuilds are chunked by **frame budget** — at most `min(dirty.len(), 2000)` objects per pass, with the rest deferred to the next frame; the `dirty` set is deduplicated by `ObjId` (dragging a shared node produces hundreds of Way dirty marks at once).

### 2.6 multipolygon Assembly, Inner Rings, and Triangulation

```
relation(type=multipolygon|boundary)
   │ classify by role: outer / inner / (empty role treated as outer, as in JOSM)
   ▼
ring assembly: way chains joined end to end → closed rings
   │ unclosed → :unclosed_multipolygon semantics, stroke only, no fill
   ▼
nesting depth: containment counts between rings → even = outer ring, odd = inner ring
   │ (supports nested relations among members: recursively expand, depth cap 8 to prevent cycles)
   ▼
earcutr: flatten [outer ring, hole0, hole1, …] + hole_indices
   │ dims = 2, rings must be closed and non-self-intersecting
   ▼
AreaGeom { verts, tris: Vec<u32>, rings, ok }
```

```rust
// neo-geo/src/geometry/tess.rs
let mut verts: Vec<f64> = Vec::new();     // flat [x0,y0, x1,y1, …]
let mut holes: Vec<usize> = Vec::new();   // **vertex index** where each hole ring starts (not a coordinate index)
verts.extend_from_slice(outer_flat);
for hole in holes_flat { holes.push(verts.len() / 2); verts.extend_from_slice(hole); }
let idx: Vec<usize> = earcutr::earcut(&verts, &holes, 2)?;  // vertex indices → u32 indices
```

- earcutr 0.5's `earcut` returns `Result<Vec<usize>, Error>`, but **only covers very few failure paths**: self-intersecting rings silently produce degenerate triangles, so NEO runs `ring_is_simple()` before calling it (a `geo::Intersects` self-check, performed only for rings above an area threshold and on first generation, cached by `(rel_id, version)`).
- Failure/degeneration fallback: `ok = false` → that relation uses only the stroke pipeline, and a `MultipolygonTest`-level geometry issue is posted to `neo-validate`; rendering does not panic and leaves no hole-colored patches.
- A closed way with `area=no` goes through polyline rather than fill; a roundabout is a line, not an area — do not mistake "closed" for "area".
- Ring orientation (clockwise/counterclockwise) is **not normalized and written back into the data**: orientation is semantics of OSM data (`:clockwise`/`:anticlockwise` pseudo-classes need it); it is only flipped on a copy at the triangulation entry point when necessary.

### 2.7 Line Width Expansion and Screen-Space Anti-Aliasing

- `MapVertex` carries `normal` (the segment direction's normal, a unit vector in the projection plane), `across ∈ {-1, +1}`, and `half_width` (**pixels**). Vertex shader: `pos_px = affine(pos_world) + normal * half_width * across`, so line width is independent of zoom and the GPU side does only one multiply-add.
- Joins: miter (default, limited to `miter_limit = 2.0`, falling back to bevel beyond that) / bevel / round. Round joins are implemented by "adding a disc of radius = half_width at the endpoint" rather than generating fan geometry.
- Anti-aliasing at corners: the fragment shader attenuates alpha using the analytic coverage `abs(across) - 1.0 + 1.0/px` (a 1 px transition band). Thus line segments **do not depend on MSAA**; MSAA is used only for the slanted edges of areas and icons (see [05](05-renderer-wgpu.md) §2.7).
- Casing (road outline) is not as simple as "draw a wider line + a narrower line stacked": it requires two passes with adjacent z-order, so `RenderBatch` explicitly carries a `casing_of` relationship, and `neo-scene` guarantees adjacent batches so other objects' z-index cannot interleave.

### 2.8 Two-Tier Spatial Index

**Object tier** (`rstar::RTree<ObjEntry>`, NEO's own type, see below):

| Object | envelope | Notes |
|---|---|---|
| Node | `AABB::from_point([x, y])` | Single point; clusters (multiple points at the same coordinate) are not merged; the render layer batches them |
| Way | Precomputed AABB (vertex bounding box) | Computed once at parse time; recomputed and `upsert`ed after node moves |
| Relation | Union of member bboxes (**derived, cached**) | Member add/remove/move → invalidated and recomputed; nested relations recurse with a cap |
| Segment | One `AABB::from_corners` per segment | Separate segment tree, used only for line picking (`PointDistance` segment distance) |

```rust
impl rstar::RTreeObject for ObjEntry {
    type Envelope = rstar::AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope { self.env }
}
impl rstar::PointDistance for SegmentEntry {
    fn distance_2(&self, p: &[f64; 2]) -> f64 { /* squared distance to the clamped projection onto the segment */ }
}
```

**Viewport tier** (screen-space buckets, rebuilt with `VisibleSet`):

- 256×256 px grid; each cell stores the list of `ObjId`s in that region (in draw order).
- Uses: O(1) starting point for hover picking, neighborhood queries for label avoidance, candidate narrowing for `hit_all`.
- Rebuilt only when `version` or the camera changes, produced in the same batch as the visible set, not maintained separately.

**bulk_load timing and incremental strategy**:

| Timing | Operation | Notes |
|---|---|---|
| Initial parse/import completes | `RTree::bulk_load(Vec<ObjEntry>)` | `O(n log n)`, much faster than inserting one by one; **the only allowed whole-tree construction path** |
| Single edit (including undo/redo) | `remove(&old)` + `insert(new)` | Rebalances each time; the dirty set is flushed in per-frame batches |
| Download region merge | Sort by key, then batch `insert` | New ids are dense; sorted insertion reduces splits |
| churn > 10% or per-frame dirty > 50k | Background `rayon` rebuild + double-buffer swap | The old tree is read during rebuild; the swap happens at a frame boundary and the old tree is released 1 frame after the pointer swap |
| Projection switch | Rebuild as above (all coordinates become invalid after a CRS change) | Reuses the same rebuild channel |

> Key constraint: `geo` 0.33.1 binds **rstar 0.12** through the `rstar_0_12` feature of `geo-types`, while NEO uses **rstar 0.14**. Therefore `neo-geo` **does not enable** geo's rstar interop (the rstar feature of `geo-types` is off), and `geo` is used purely as a geometry algorithm library; when geo types must be fed into the index, they are adapted with newtypes (`struct GeoAabb(geo::Rect<f64>)` with a hand-written `RTreeObject`), with no global type aliases.

### 2.9 LOD Strategy

| zoom | What is drawn | Simplify epsilon | Notes |
|---|---|---|---|
| 0–7 | `highway ∈ {motorway,trunk,primary}` skeleton, `place` points, large water/landuse areas | 2.0 px | Small areas are dropped outright (<4 px² is invisible) |
| 8–12 | + secondary/tertiary/rail/waterway | 1.0 px | Buildings merged into footprint color patches |
| 13–15 | All way styles | 0.5 px | Buildings filled individually |
| 16–18 | All vertices, POI icons, name labels | 0 (dedupe points only) | Labels begin avoiding collisions |
| ≥19 | Full detail + editing handles/node markers (overlay) | 0 | Handles belong to the UI layer and do not enter `SceneFrame` |

LOD decisions are centralized in one place (`Lod::for_zoom`), so styles, geometry, and rendering do not each duplicate the thresholds. `VisibleStats` exposes `objects / vertices / batches / culled_lod` for acceptance checks.

### 2.10 Hit Testing

1. **Screen radius → projection-plane distance**: `r_world = radius_px / px_per_world_m`. Note this is **not** ground meters: ground tolerance = `r_world * cos(lat)`. JOSM tests distance in projected space, and NEO stays consistent while expressing tolerance as "pixels" rather than "meters" in the UI.
2. **Candidates**: `locate_in_envelope_intersecting(AABB::from_point(p).expanded(r_world))`.
3. **Exact distance**: the segment tree uses `nearest_neighbor_iter_with_distance_2` for ordering; nodes use Euclidean distance.
4. **Priority** (at equal distance, consistent with JOSM's mental model): `node` > `way` (segment) > `closed way` interior area > `relation` member. Interior-area hits only apply when there is no closer line/point and the style allows area picking.
5. **Polygon interior**: `geo`'s `PreparedGeometry` (`algorithm::indexed::prepared_geometry`) is built once and queried many times, faster than calling `Contains` each time; still prefilter with object AABBs first.
6. **Tolerance varies with zoom**: `radius_px` is fixed at 8 px (configurable) because it is a screen-space quantity, so tolerance converges naturally as zoom changes; do not use a fixed ground-meter tolerance — at low zoom it would hit a whole cluster of objects.
7. **Candidate-stage filtering**: visibility (such as node LOD) is decided at the candidate stage, not by falling back after a hit, otherwise "the topmost invisible object" would cover selectable objects below it.

| Option | Precision | Cost | Conclusion |
|---|---|---|---|
| CPU (index + analytic distance) | Line/point sub-pixel; areas need `Contains` | Same order as candidate count | **Default**; used for both hover and click |
| GPU ID buffer (R32Uint) | Exactly pixel-consistent, including triangulation error | One pass + readback synchronization | Fallback for dense regions/area interiors, off by default (details in [05](05-renderer-wgpu.md) §2.10) |
| Hybrid | CPU produces candidates first; GPU only intervenes when candidates > 32 | Two result sets must agree | Phase 2; run consistency regressions before enabling |

## 3 Key Interfaces

```rust
// ── neo-geo/src/projection.rs ───────────────────────────────────────────────
pub struct LonLat { pub lon: f64, pub lat: f64 }
pub struct WorldPoint { pub x: f64, pub y: f64 }
pub struct Aabb { pub min: [f64; 2], pub max: [f64; 2] }

pub enum ProjectionKind {
    WebMercator,                                   // EPSG:3857, default, no external dependency
    Geographic,                                    // EPSG:4326, debugging/display
    Proj { epsg: u32, crs: String },               // proj 0.32 backend
}

pub trait Projection: Send + Sync {
    fn kind(&self) -> ProjectionKind;
    fn to_world(&self, ll: LonLat) -> WorldPoint;
    fn to_lonlat(&self, w: WorldPoint) -> LonLat;
    fn is_conformal(&self) -> bool;                // conformality: decides line width/circle radius compensation
    fn scale_factor(&self, ll: LonLat) -> f64;     // local scale: projected meters → ground meters
    fn tile_scheme(&self) -> Option<TileScheme>;   // WebMercator only
}

// ── neo-geo/src/camera.rs ───────────────────────────────────────────────────
pub struct Camera {
    pub center: LonLat,
    pub zoom: f64,             // continuous zoom level
    pub viewport_px: [u32; 2], // physical pixels
    pub ppp: f32,              // pixels_per_point (fractional; do not round)
}
impl Camera {
    pub fn px_per_world_m(&self) -> f64;          // 2^zoom * 256 / W
    pub fn world_m_per_px(&self) -> f64;
    pub fn ground_m_per_px(&self) -> f64;         // multiplied by cos(lat)
    pub fn view_bounds(&self, margin_px: f32) -> Aabb;
    pub fn rebase_origin(&self) -> WorldPoint;    // 2^16 m grid aligned
    pub fn to_screen(&self, w: WorldPoint) -> [f32; 2];   // UI/debugging only
    pub fn screen_to_lonlat(&self, px: [f32; 2]) -> LonLat;
    pub fn screen_to_world(&self, px: [f32; 2]) -> WorldPoint;
}

// ── neo-geo/src/geometry.rs ─────────────────────────────────────────────────
pub struct Polyline { pub pts: Vec<[f64; 2]>, pub closed: bool, pub cum_len: Vec<f32>, pub aabb: Aabb }
pub struct AreaGeom { pub verts: Vec<[f64; 2]>, pub tris: Vec<u32>, pub rings: Vec<Ring>, pub ok: bool }

pub struct GeometryBuilder { /* earcutr reusable buffers, simplification workspace */ }
impl GeometryBuilder {
    pub fn build_way(&mut self, way: WayView<'_>, cam: &Camera, lod: Lod) -> Polyline;
    pub fn build_area(&mut self, mp: MultiPolygonView<'_>) -> AreaGeom;
    pub fn simplify_screen(&self, pts: &[[f64; 2]], px_eps: f32, world_m_per_px: f64) -> Vec<[f64; 2]>;
    pub fn is_closed_area(&self, pts: &[[f64; 2]], tags: &TagView<'_>) -> bool; // closed ≠ area
}

// ── neo-geo/src/index.rs ────────────────────────────────────────────────────
pub struct ObjEntry { pub id: ObjId, pub env: Aabb, pub osm_id: i64, pub derived: bool }

pub struct SpatialIndex { /* rstar::RTree<ObjEntry> + segment tree + dirty queue + rebuild state */ }
impl SpatialIndex {
    pub fn new() -> Self;
    pub fn bulk_load(&mut self, entries: Vec<ObjEntry>);
    pub fn upsert(&mut self, e: ObjEntry);
    pub fn remove(&mut self, id: ObjId);
    pub fn flush_dirty(&mut self, budget: usize) -> DirtyStats;
    pub fn rebuild_if_needed(&mut self, force: bool) -> Option<RebuildHandle>; // background + swap
    pub fn candidates(&self, area: Aabb) -> impl Iterator<Item = ObjId> + '_;
    pub fn nearest_segments(&self, p: [f64; 2], max_d: f64) -> impl Iterator<Item = (ObjId, f64)> + '_;
    pub fn stats(&self) -> IndexStats;
}

// ── neo-geo/src/hit.rs ──────────────────────────────────────────────────────
pub struct HitOpts { pub radius_px: f32, pub allow_area: bool, pub layers: LayerMask }
pub enum HitCandidate {
    Node(ObjId),
    WaySegment { way: ObjId, seg: u32, t: f32, dist: f64 },
    Area(ObjId),
}

pub struct HitTester<'a> { pub index: &'a SpatialIndex }
impl<'a> HitTester<'a> {
    pub fn hit(&self, cam: &Camera, px: [f32; 2], opts: HitOpts) -> Option<HitCandidate>;
    pub fn hit_all(&self, cam: &Camera, px: [f32; 2], opts: HitOpts) -> Vec<HitCandidate>;
}

// ── neo-scene/src/visible.rs ────────────────────────────────────────────────
pub struct VisibleSet { pub objects: Vec<ObjId>, pub batches: Vec<RenderBatch>, pub stats: VisibleStats }
impl VisibleSet {
    pub fn rebuild(index: &SpatialIndex, cam: &Camera, style: &StyleSheet) -> Self;
    pub fn bucket_at(&self, px: [f32; 2]) -> &[ObjId];   // screen-space index, 256 px grid
}
```

## 4 Data Flow

```
logical edit / download / undo
        │  Command (incremental)
        ▼
  neo-store  ──►  dirty(objects) ──►  VisibleSet invalidated
        │
        ├─►  neo-geo::geometry   project → simplify → closure test → normals → triangulate
        │        └─ Polyline / AreaGeom (projection-plane f64 + screen-space parameters)
        ├─►  neo-geo::index      upsert/remove; churn>10% triggers background rebuild
        ▼
  neo-scene::visible  (cam, style) ──► VisibleSet { objects, batches, buckets }
        │  rebase f64→f32, LOD filtering, sort and batch by (layer, z-index, casing)
        ▼
  neo-scene::frame    SceneFrame { MapVertex[], indices, RenderBatch[], LabelBatch }
        ▼
  neo-render          BufferPool upload (only when `version` changes) → egui paint callback
```

Decision order within a frame (avoiding "full rebuild every frame"):

```
frame:
  input ──► Camera update (f64)
     │
     ├─ zoom crosses LOD level or rebase crosses grid ──► version+1 ──► geometry rebuild (frame budget)
     ├─ style change ──────────────────────────────────► version+1 ──► recompute only affected tag-fingerprint buckets
     │
     ├─ index query view_bounds(margin) ──► candidates (O(log n + k))
     ├─ LOD culling + batching ──► RenderBatch[]
     └─ upload only when `version` changes; every frame: uniform update (camera scale/offset)
```

## 5 Tradeoffs and Alternatives

| Decision | Alternative | Why this one | Cost |
|---|---|---|---|
| Own types + rstar 0.14 | Use geo's rstar 0.12 interop | A single rstar version; `PointDistance` can be customized for the segment tree | Must hand-write 4 trait impls; cannot use the ready-made `geo::algorithm::rstar` wrappers |
| f64 world + f32 rebasing | All-f64 vertices | Halves vertex bandwidth, and error at level 19 is <0.1 px | One extra rebase layer and cross-cell rebuilds |
| Rebuild geometry per `version` | Reproject everything every frame | Decouples CPU work from zoom changes; panning costs nearly nothing | One-time cost spikes at `version` boundaries need a frame budget |
| Screen-space buckets | Pick directly from the object R-tree | Hover queries run every frame; buckets reduce the constant to O(1) | Extra memory (~8 B/visible object), rebuilt with the visible set |
| earcutr rather than spade/Delaunay | `geo`'s `spade` feature | Flat 2D fills do not need well-shaped triangles; earcut is cheaper | Self-intersecting input silently degrades, requiring self-checks |
| CPU picking on by default | Full GPU ID buffer | No synchronization cost, no 1-frame latency, no extra pass | Interior-area detection depends on triangulation consistency |
| Disable geo's default features (`earcut`/`spade`/`multithreading`) | `geo = "0.33.1"` defaults | Defaults pull in **earcut 0.4.5** (duplicating earcutr 0.5) and rstar 0.12 | Enable only the features needed for `Contains`/`Simplify`; re-verify on upgrades |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Insufficient precision: rebasing omitted or origin not aligned | Geometry jitter/cracking at high zoom | Unit tests assert "extreme coordinates + rebased error < 1 cm"; debug assertions on coordinate range |
| R2 | earcutr silently produces bad triangles for self-intersecting rings | Artifacts/punch-through in areas | Self-check on first generation + stroke fallback on failure + Validator report |
| R3 | Index rebuild races with the render thread | Dropped frames, occasional stalls | Double buffering + swap at frame boundaries; rebuild on a dedicated thread outside the rayon pool to avoid contending with rendering |
| R4 | Incomplete invalidation of derived relation bboxes | Large relations disappear wholesale | All member changes go through `invalidate_relation()`; daily consistency checks (bbox ⊇ member bboxes) |
| R5 | AABB is non-rectangular under multiple projections (still an AABB after projection, but proportions differ) | Tolerance unequal east-west vs north-south | When `is_conformal()` is false, use a single tolerance along the principal axis and explain it in the UI |
| R6 | JOSM semantic drift (`area` matching unclosed ways, unclosed multipolygons) | Visual inconsistency with JOSM | The semantic difference list goes into [11](11-style-presets-mapcss.md) §2.5, with side-by-side render regressions |
| R7 | proj runtime missing/`.gsb` grids missing | Projection options unavailable or precision degraded | Startup probing + explicit degradation notice; panicking is prohibited |

## 7 Acceptance Criteria

1. **Precision**: near the EPSG:3857 extremes (±180°, ±85°) and at a target city on the reference machine, sample 1e4 points each; the round-trip error of `to_world → rebase → back-project` is < 1 cm; the rendered position deviates from CPU `to_screen` by < 0.5 px (level 19).
2. **Index**: for 1M / 10M object datasets, `view_bounds` candidate results are correct (compared item by item with a brute-force scan); after `bulk_load`, `size()` matches the input; after 1000 random `upsert`/`remove` operations it still matches the brute-force scan.
3. **Geometry**: 20 cases each of closure detection, `area=no`, unclosed multipolygons, and relations with 2 levels of holes pass manual comparison against JOSM render screenshots.
4. **LOD**: at zoom levels 0/8/13/16/19, `VisibleStats.vertices` is monotonic with zoom (allowing ±10% jitter) and **does not grow significantly with total object count** from 1M to 10M.
5. **Picking**: at least 95% agreement on hit objects with the same-named JOSM operations (200 manual click regressions).
6. **Timing**: single-run timings for index queries, geometry rebuilds, and picking on the reference machine (i5-12400F / 32 GB / RTX 3070 / Windows) are [pending measurement, see docs/14-performance-and-verification.md].

## 8 References

- Coordinate precision of OSM PBF (`granularity=100`, `lat = 1e-9*(lat_offset + granularity*lat)`): <https://wiki.openstreetmap.org/wiki/PBF_Format>
- Web Mercator definition and world extent: <https://epsg.io/3857>
- JOSM map style semantics (`area` matching unclosed ways): <https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- rstar 0.14 traits and methods (`bulk_load`, `locate_in_envelope_intersecting`, `PointDistance`): <https://docs.rs/rstar/0.14.0/rstar/>
- earcutr 0.5.0's `earcut(vertices, hole_indices, dims)`: <https://docs.rs/earcutr/0.5.0/earcutr/>
- geo 0.33.1 `Contains`/`PreparedGeometry`/`Simplify`: <https://docs.rs/geo/0.33.1/geo/>
- JOSM projection/`--projection` and `.gsb` grids: <https://josm.openstreetmap.de/wiki/Help/CommandLineOptions>
- Not verified against a vendored copy in this repository: the constructor API and thread safety of `proj` 0.32 **> To be verified**; distribution terms for PROJ `proj.db`/`.gsb` **> To be verified**.
