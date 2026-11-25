# 05 Renderer (wgpu 30.0.2 + egui-wgpu 0.36.2)

> Status: draft v0.1 | Date: 2026-11-15 | Owning crate: `neo-render` (consumes `SceneFrame`/`MapVertex` from `neo-scene`)
> Version baseline: **wgpu 30.0.2**, **egui / eframe / egui-wgpu 0.36.2**, winit 0.30.13, bytemuck 1.25
> Related: [01-system-architecture.md](01-system-architecture.md) | [02-workspace-and-dependency-rules.md](02-workspace-and-dependency-rules.md) | [04-spatial-index-and-geometry.md](04-spatial-index-and-geometry.md) | [11-style-presets-mapcss.md](11-style-presets-mapcss.md) | [ADR-0005](adr/0005-wgpu-egui-shared-device.md) | [14-performance-and-verification.md](14-performance-and-verification.md)

## Summary

- NEO does not build its own winit event loop or a second `wgpu::Instance`: `eframe` creates the surface and device, and `neo-render` obtains the **same** `Device`/`Queue` through `egui_wgpu::RenderState` (`adapter/instance/device/queue/target_format/renderer`), drawing the map with one `CallbackTrait`.
- Four pipeline classes: **A line segments/stroke**, **B areas/fill**, **C points/icon instancing**, **D text and selection overlays**. All four share the 56-byte `MapVertex` and the same set of bind groups.
- Each frame writes only the 32-byte camera uniform; geometry buffers are uploaded by dirty ranges only when `SceneFrame.version` changes. In steady state, GPU-side writes are close to zero.
- Sorting uses the painter's algorithm (stable order by `(layer, z, kind)`), and **Phase 1 does not enable a depth buffer**: egui's render pass sets depth `store` to `Discard` (see §2.7), so any depth reuse across callbacks is unreliable.
- Color space is decided once at startup: whether `target_format` is `*Srgb` or in gamma space (typically `Bgra8Unorm`) directly determines vertex colors and shader output; mixing them causes double gamma.
- Picking uses the CPU by default ([04](04-spatial-index-and-geometry.md) §2.10); the GPU ID buffer (`R32Uint` + 1×1 readback) is enabled only when candidates > 32 or "pixel-exact" precision is required, and it is **asynchronous by default with 1 frame of latency**.
- Map text ultimately goes through a dedicated glyph atlas; Phase 1 delivers the same interface via an egui overlay (`LabelItem` is already in `SceneFrame`, so switching does not touch upper layers).
- Capability probing comes first: `Limits::downlevel_defaults()` is the floor, and indirect drawing/timestamps/MSAA are all runtime-probed with graceful degradation.
- All frame-time numbers are [pending measurement, see docs/14-performance-and-verification.md].

## 1 Goals and Scope

### 1.1 Goals

1. Stably draw the visible subset of 1M–10M objects within 16.6 ms per frame (60 Hz), with no per-frame rebuild in steady state.
2. Draw volume is proportional to "the number of objects within the viewport" and independent of the total data size (LOD and culling happen in `neo-scene`; the renderer only batches on the GPU side).
3. Coexistence with egui without tearing, double gamma, or per-frame device loss; no 1 px seams at DPI 1.25/1.5/1.75.
4. Consistent behavior across Windows(DX12)/Linux(Vulkan)/macOS(Metal); software adapters (WARP/llvmpipe) can start in a degraded mode instead of crashing.
5. Vertices/batches are POD (`bytemuck`), and the render layer has zero `neo-store` dependency.

### 1.2 Non-goals

- No 3D building extrusion, terrain, or shadows (Phase 2; only then are a depth attachment and our own prepass needed).
- No imagery tile decoding (`neo-io`), no style evaluation (`neo-style`), no semantic hit decisions (`neo-geo`).
- No home-grown UI framework: panels, menus, and dialogs are all egui.

### 1.3 Ownership Boundaries

| Resource | Owner | How `neo-render` obtains it | Notes |
|---|---|---|---|
| `wgpu::Instance` / `Adapter` | `eframe` | `RenderState.instance` / `.adapter` | Never created twice (duplicate instances = two blocks of VRAM/two queues) |
| `wgpu::Device` / `Queue` | `eframe` | `RenderState.device` / `.queue` | Used only in `prepare`/resource creation; `submit` is forbidden in `paint` |
| swapchain | `eframe` | Not accessed directly | Color format is observed only via `RenderState.target_format` |
| Pipeline, buffers, textures, atlas | `neo-render` | Stored in `Renderer::callback_resources` (`type_map`) | See [ADR-0005](adr/0005-wgpu-egui-shared-device.md) |
| `RendererOptions` (MSAA/depth) | App startup arguments | Passed when constructing `egui_wgpu::Renderer` | Must match our pipelines |

## 2 Design

### 2.1 Overall Pipeline

```
neo-scene::SceneFrame (POD: MapVertex[] / indices / RenderBatch[] / LabelItem[])
   │  prepare (CallbackTrait::prepare, before this frame's egui pass)
   │  1) pools.begin_frame(version)  2) merge dirty ranges → write_buffer / staging
   │  3) camera uniform 32 B         4) optional: encode pick pass (separate encoder)
   ▼  eframe opens the egui_render pass (color = target_format, no depth, MSAA decided by egui)
paint (CallbackTrait::paint): set_viewport(viewport_in_pixels)
   ├─ B fill    draw_indexed (earcut triangles, premultiplied alpha)
   ├─ A stroke  draw_indexed (4 vertices per segment; casing wide first, then narrow)
   ├─ C point   draw_indexed(0..6, 0, 0..instances)
   ├─ D glyph   draw_indexed (atlas quads)
   └─ D overlay delegated to egui's own mesh (does not hijack the painter)
   ▼  eframe: egui UI mesh → present
```

The division of labor between `prepare` and `paint` is a hard constraint: `paint` receives `&CallbackResources` (immutable) and is already inside a `RenderPass<'static>`, so it **must not create new passes or submit**; any work needing an encoder must go into `prepare` and return the `CommandBuffer` to egui (egui submits it after its own commands).

> Naming note: in [01](01-system-architecture.md) and [07](07-concurrency-and-tasks.md) this method is named `MapRenderer::encode`, which is the same method as `render` here — it was renamed to align with the semantics of `CallbackTrait::paint`, and this document is authoritative in all three places.

### 2.2 Vertex Format and Batches

```rust
// crates/neo-scene/src/frame.rs —— no GPU dependency; the render layer only consumes it
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MapVertex {
    pub pos: [f32; 2],     // L2: projection-plane meters, already rebased by rebase_origin
    pub normal: [f32; 2],  // unit normal (used by stroke; 0 for fill/point)
    pub across: f32,       // quad side -1.0/+1.0; 0 for fill/point
    pub half_width: f32,   // screen pixels (independent of zoom)
    pub color: [f32; 4],   // premultiplied alpha, sRGB-encoded (see §2.7)
    pub uv: [f32; 2],      // icon/atlas; (0,0) for fill
    pub flags: u32,        // bit0 dash, bit1 casing, bit2 textured, bit3 aa_off
    pub z: f32,            // for sort validation (not part of depth testing)
}
const _: () = assert!(core::mem::size_of::<MapVertex>() == 56);
```

Vertex attribute layout for `MapVertex` (`VertexBufferLayout`, single buffer, stride 56):

Vertex attribute layout (single buffer, stride 56 B): `pos` Float32x2 @0, `normal` Float32x2 @8, `across` Float32 @16, `half_width` Float32 @20, `color` Float32x4 @24, `uv` Float32x2 @40, `flags` Uint32 @48, `z` Float32 @52.

```rust
pub enum BatchKind { Fill, Stroke, Point, Glyph }
pub struct RenderBatch {
    pub kind: BatchKind,
    pub layer: LayerId,          // MapCSS ::layer
    pub z: i16,                  // z-index compositing key
    pub texture: Option<TexId>,  // icon/atlas
    pub blend: BlendMode,        // Premul | Alpha | Additive
    pub vr: Range<u32>,          // vertex range (within the same FrameSlot)
    pub ir: Range<u32>,          // index range (used by Fill/A; empty for Point)
    pub inst: Range<u32>,        // Point/Glyph instance range
    pub pick_base: u32,          // pick ID base (parallel to vr)
    pub flags: BatchFlags,       // casing_of / overlay / dash_reset / no_pick
}

pub struct SceneFrame {
    pub version: u64,            // geometry+style version; upload only when it changes
    pub camera: CameraUniform,   // per frame
    pub batches: Vec<RenderBatch>,
    pub labels: Vec<LabelItem>,  // world coordinates + text + style (class D consumer)
    pub stats: SceneStats,       // vertex count/batch count/culled count (for acceptance)
}

#[repr(C)]
pub struct CameraUniform {       // 32 B, 16 B aligned
    pub scale: f32,              // world meters → physical pixels
    pub offset: [f32; 2],        // post-rebase translation (pixels)
    pub viewport: [f32; 2],      // physical pixels
    pub _pad: [f32; 3],
}
```

### 2.3 Pipeline A: Lines / Road Stroke

- Topology `TriangleList`, **4 vertices + 6 indices per segment** (`[0,1,2, 0,2,3]`). Vertex shader: `p_px = affine(pos) + normal * half_width * across`; both coordinates and line width are in pixel space, so line width is naturally independent of zoom.
- Joins (miter/bevel/round) are filled in by extra quads generated by `neo-geo`; round joins do not generate fans but use endpoint discs + analytic AA.
- Anti-aliasing: the fragment computes a 1 px coverage band with `cov = saturate(1.0 - (abs(across) - 1.0 + 1.0/px))`, **without relying on MSAA**.
- Dashes: `flags.dash` + `uv.x = cumulative arc length / dash_period`, with the fragment discarding by fract; `dash_reset` means the phase resets at a new object's start.
- Casing: draw the wide casing batch **first** and the narrow fill batch **after**; `BatchFlags::casing_of` guarantees adjacency, and any z-index change must move the whole pair together (`neo-scene` handles this; the render layer only validates order and logs a `debug!` on violation). Current implementation ([11](11-style-presets-mapcss.md) §0): one independent `BatchKind::Casing` batch per layer, with submission order `fill → casing → stroke → point`; bridge outlines, tunnel dashed outlines, and water surfaces on fords all go through this channel.
- `POLYGON_MODE_LINE` is not used: its line width is unreliable on all backends (most implementations fix it at 1 px), so it remains a debug toggle only.

### 2.4 Pipeline B: Areas / Building Fill

- Input is the `verts + tris` produced by earcutr ([04](04-spatial-index-and-geometry.md) §2.6), with `Uint32` indices; multiple areas (adjacent, same style) can be merged within one batch to reduce draw calls.
- Blending uses **premultiplied alpha** (`wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING`); straight alpha colors in styles are premultiplied once when `neo-scene` packs them, avoiding "areas darken where they overlap".
- Two edge-handling tiers: with `msaa_samples > 1`, rely on MSAA; with `msaa_samples == 1`, accept hard edges and record that fact in `GpuCaps` (no supersampling post-process; the cost is too high).
- Holes/degenerate triangles: areas with `AreaGeom.ok == false` **do not enter the fill pipeline** (already culled by `neo-scene` and converted to stroke); the render layer performs no geometry repair.

### 2.5 Pipeline C: Points / Icon Instancing

- A 6-vertex quad is generated from `vertex_index`, with an instance buffer stride of 48 B:

```rust
#[repr(C)]
pub struct PointInstance {
    pub pos: [f32; 2],     // L2 world
    pub size_px: [f32; 2], // icon width/height (pixels)
    pub rot: f32,          // radians
    pub tint: [f32; 4],    // premultiplied
    pub uv_rect: [f32; 4], // atlas sub-rectangle
}
```

- One `draw_indexed(0..6, 0, 0..n)` draws all points of a layer; different atlas pages = different batches.
- Node handles and selection boxes **do not** enter C: they are UI semantics (hoverable, draggable) and go through class D overlays, avoiding per-frame re-upload of the instance buffer for handles.

### 2.6 Pipeline D: Text and Selection Overlays (with Decision)

| Option | Implementation | Pros | Cons | Conclusion |
|---|---|---|---|---|
| D1 egui overlay | `egui::Painter::text` per label | No new infrastructure; i18n/fonts/scaling reuse egui directly | One layout per label (egui's galley cache mitigates but does not eliminate); 10k+ labels are infeasible | **Phase 1 ship**, label count capped at ≤ 2k/frame |
| D2 dedicated glyph atlas | Shape with `cosmic-text` → own atlas → instanced glyph quads | Single draw, no layout jitter, SDF possible | Needs a full set of glyph caching, line breaking/collision avoidance, RTL, and CJK fallback | **Phase 2 target**; the `LabelItem` interface is already reserved |
| D3 hybrid | Map names via D2; selection/measurement/tooltips via D1 | Each used where it is strong | Two coordinate systems must align (both use `PaintCallbackInfo` pixels) | **Final form** |

Decision: **D3**, delivered in the order D1 → D2; the shape of `SceneFrame.labels` does not change with the implementation, and switching only changes the internals of `LabelRenderer`.

### 2.7 Depth, Blending, MSAA, sRGB

| Item | Decision | Rationale / caveats |
|---|---|---|
| Depth | Phase 1 **off** (`RendererOptions.depth_stencil_format = None`), painter's-algorithm sorting | In egui's pass, depth is `load = Clear(1.0)`, `store = Discard`; reusing depth across callbacks is unreliable |
| Depth (Phase 2) | `Some(Depth32Float)`, pipeline `DepthStencilState { depth_write_enabled: Some(true), depth_compare: Some(Less) }` | Both fields are `Option`s in wgpu 30; the prepass must live in `prepare` and create its own textures |
| Blending | Premultiplied alpha (fill/point/glyph); `Additive` only for highlight glows | `BlendState::PREMULTIPLIED_ALPHA_BLENDING` matches egui itself, avoiding darkened overlaps between UI and map |
| MSAA | Probe at startup whether `get_texture_format_features(fmt).flags` contains `MULTISAMPLE_X4`; prefer 4x, else 1 | The pipeline's `MultisampleState.count` must **equal** `RendererOptions.msaa_samples.max(1)`, otherwise it is a runtime validation error (not a compile error) |
| sRGB | Read `target_format.is_srgb()` once and branch (`GpuCaps.srgb`) | egui recommends a gamma-space swapchain (`Bgra8Unorm`); if the platform forces `*Srgb`, our fragment must output linear values, otherwise double gamma makes it look washed out |
| Color path | Style color (sRGB) → premultiply → vertex; the shader does **not** apply gamma transforms, only the `srgb` branch's `linear = pow(color, 2.2)` | Enable that branch only when `GpuCaps.srgb == true`; validate both paths against the same golden images |
| DPI | `viewport_in_pixels()` rounded for `set_viewport`; `pixels_per_point` used only for UI sizing | Fractional DPI (1.25/1.5/1.75) must not enter the projection matrix, otherwise 1 px seams appear |

### 2.8 BufferPool and Incremental Updates

- **Slots**: `FRAMES_IN_FLIGHT = 2`. Each slot independently holds vertex/index/instance/uniform buffers, avoiding "the previous frame is still reading while this frame overwrites".
- **Logical pool**: stable allocation keyed by `BatchKey { layer, z, kind, texture }`; the `RenderBatch` ranges stay **completely unchanged** while `version` is unchanged — this is the key to "zero per-frame rebuild".
- **Dirty-range upload**: when `version` changes, diff the old and new batch ranges → merge adjacent ranges (merge when the gap is < 4 KiB) → for segments < 64 KiB use `queue.write_buffer`, otherwise write to a staging buffer and `copy_buffer_to_buffer` (large `write_buffer` calls allocate staging implicitly and may block).
- **Growth**: capacity grows by 1.5× and never shrinks; only when a single slot exceeds 512 MiB is a defragmentation done into a new slot (no in-place moves).
- **Steady-state write volume**: 32 B uniform + a few dirty segments; `PrepareStats { uploaded_bytes, draw_calls, batches }` feeds puffin and acceptance assertions.
- **Prohibited**: per-frame `create_buffer_init`, per-frame `create_bind_group`, `queue.submit` inside `paint`.

### 2.9 Indirect Drawing and Degradation

| Capability | Probe | Use | When missing |
|---|---|---|---|
| `multi_draw_indexed_indirect` | **no longer a feature** in wgpu 30 (merged into core), available directly | Submit tens to hundreds of batches at once | — |
| `MULTI_DRAW_INDIRECT_COUNT` | `adapter.features().contains(..)` | Determine draw count on the GPU (pick pass, dynamic LOD) | Read the count back on the CPU, degrading to a fixed count |
| `INDIRECT_FIRST_INSTANCE` | Same as above | Merge multiple instanced batches into one call | One `draw_indexed` per batch |
| `TIMESTAMP_QUERY` (including `_INSIDE_ENCODERS`/`_INSIDE_PASSES`) | Same as above | GPU stage timing | Only puffin's CPU spans |
| `POLYGON_MODE_LINE` | Same as above | Debug wireframe only | None |

Strategy: **default to the `draw_indexed` loop** (batch counts are on the order of tens, so indirect drawing yields limited benefit); switch to the indirect path only when `batches > 512` (extreme styles/many layers) or for the pick pass; both paths share the same `RenderBatch` array, and switching is purely an encoding difference.

### 2.10 GPU Picking (ID Buffer)

- Target: an `R32Uint` color attachment + `Depth32Float`; no blending, no AA; each visible primitive writes `(obj_index << 8) | kind`.
- Readback copies only **1×1 texel**: `copy_texture_to_buffer` into a 256 B-aligned buffer (`bytes_per_row = 256`, a hard alignment requirement that cannot be 4), then `map_async`.
- Synchronization cost: `device.poll(PollType::wait_indefinitely())` blocks until the GPU finishes the current submission — equivalent to a full pipeline flush, and it is **never placed on the hover path**.
- Async path: `request()` on click → encode in `prepare` → `try_take()` on the next frame (`PollType::Poll`, non-blocking) → update selection on a hit. One frame of latency is acceptable for clicks; hover still uses CPU picking.
- Disabling conditions: `neo-render`'s `pick-readback` feature off, `max_texture_dimension_2d < viewport`, or a software adapter → return `None` directly, and upper layers fall back to CPU automatically (feature table in [02](02-workspace-and-dependency-rules.md) §3).

> Note: wgpu 30's `Surface::get_current_texture()` returns `CurrentSurfaceTexture` (`Success/Suboptimal/Timeout/Occluded/Outdated/Lost/Validation`), and `Queue::present(t)` performs presentation. NEO does not own the surface; this path is used only for self-managed offscreen rendering (PNG export, golden tests), and `Lost` must be handled (recreate the surface rather than merely `configure`).

### 2.11 Text Atlas and CJK Font Assets

- Atlas: `R8Unorm` 1024×1024, starting with two pages (Latin/symbols + CJK), growing to 4 pages as needed; glyph cache key `(font_id, px_size_bucket, glyph_id, subpixel_x)`, with `px_size_bucket` quantized to 1/4 pixel to avoid cache explosion.
- Shaping: `cosmic-text` (version unverified **> To be verified**) handles line breaking, bidirectional text, and font fallback; `glyphon` is considered only if it already supports wgpu 30, otherwise we build our own atlas sampling path (the atlas itself depends on no crate).
- CJK asset strategy: **do not compile fonts into the binary**. At startup, enumerate platform system fonts (Windows `msyh`/`simhei`, Linux Noto CJK, macOS `PingFang`) and prompt the user to choose if none is found; optionally download OFL-licensed Noto Sans CJK to the user data directory (both due to size and GPL isolation, see [11](11-style-presets-mapcss.md) §2.13).
- Label collision avoidance happens in `neo-scene` (screen-space grid, [04](04-spatial-index-and-geometry.md) §2.8); the render layer only draws labels that have "already been decided to be drawn" and performs no collision handling.

### 2.12 LOD, Culling, Batching

- Culling: CPU-side frustum (AABB) culling is already done in the visible set; the GPU side does no per-primitive culling, relying on viewport clipping.
- Batching: sort key `(layer, z, kind, texture, blend)`; adjacent batches with the same key are merged. Stable sorting guarantees `ObjId` order within the same `z` → rendering of the same data is reproducible (golden tests rely on this).
- LOD thresholds are decided only in `neo-scene`; the render layer receives "what should be drawn", avoiding conflicting thresholds in two places.
- Large pans **do not** trigger a `version` change (rebasing stays within the same 64 km grid) and only update the uniform; rebuilds happen only when zoom crosses LOD levels.

### 2.13 Frame Budget (16.6 ms)

> The table below is a **design budget** (allocation target), not measured values. Real numbers are [pending measurement, see docs/14-performance-and-verification.md].

| Stage | Thread | Budget (ms) | Action when over budget |
|---|---|---|---|
| Input + egui UI logic | Main | 3.0 | Collapse panels/defer layout |
| Scene diff + batching | Main | 1.5 | Reuse the previous frame's `SceneFrame` |
| uniform + dirty-range upload | Main | 0.8 | Defer dirty ranges by one frame (visually imperceptible) |
| egui tessellation + callback encoding | Main | 2.7 | Reduce label count |
| Main-thread subtotal | Main | **8.0** | 8.6 ms of headroom |
| Geometry rebuild (resumable across frames) | Worker | 4.0 | Raise LOD thresholds, reduce objects per frame |
| Style evaluation (resumable across frames) | Worker | 2.0 | Hit the tag-fingerprint cache |
| fill / stroke / point / glyph | GPU | 2.5 / 2.5 / 0.6 / 1.2 | Merge batches, lower MSAA |
| egui composite + present | GPU | 1.5 + 0.5 | Disable dithering |

### 2.14 GPU Capability Matrix and Degradation

| Capability | DX12 | Vulkan | Metal | Software (WARP/llvmpipe) | NEO usage | When missing |
|---|---|---|---|---|---|---|
| `Bgra8Unorm` / `Rgba8Unorm` presentation | ✅ | ✅ | ✅ | ✅ | Preferred format | Fall back to `*Srgb` + linear-output branch |
| `Depth32Float` | ✅ | ✅ | ✅ | ✅ | Phase 2 | No depth (Phase 1 does not need it anyway) |
| MSAA 4x | ✅ | ✅ | ✅ | Commonly missing | Areas/icons | `msaa_samples = 1`, hard edges on area borders |
| `R32Uint` color attachment | ✅ | ✅ | ✅ | ✅ | GPU picking | Disable GPU picking |
| `TIMESTAMP_QUERY` | ✅ | ✅ | ✅ | Often missing | Stage timing | puffin CPU timing |
| Indirect drawing (core) | ✅ | ✅ | ✅ | ✅ | Large batches | Loop draw |
| `MULTI_DRAW_INDIRECT_COUNT` | ✅ | ✅(1.2+) | ❌ | ❌ | Pick pass | CPU count |
| `INDIRECT_FIRST_INSTANCE` | ✅ | ✅ | ✅ | ❌ | Instance batching | One draw per batch |

**Floor**: `Limits` requests `Limits::downlevel_defaults()` (`max_texture_dimension_2d = 2048`, `max_uniform_buffer_binding_size = 16 KiB`, `max_vertex_attributes = 16`, `max_inter_stage_shader_variables = 15`, `max_color_attachments = 4`), then takes `min` with `adapter.limits()` and negotiates upward. `DeviceDescriptor { label, required_features, required_limits, experimental_features, memory_hints, trace }` is filled in one go (wgpu 30's `request_device` takes only `&DeviceDescriptor`, with no second argument).

## 3 Key Interfaces

```rust
// ── neo-render/src/caps.rs ──────────────────────────────────────────────────
pub struct GpuCaps {
    pub target_format: wgpu::TextureFormat,
    pub srgb: bool,
    pub msaa_samples: u32,             // 1 or 4
    pub limits: wgpu::Limits,
    pub has_multi_draw_count: bool,
    pub has_indirect_first_instance: bool,
    pub has_timestamps: bool,
    pub software_adapter: bool,
}
impl GpuCaps {
    pub fn probe(adapter: &wgpu::Adapter, target_format: wgpu::TextureFormat, want_msaa: u32) -> Self;
    pub fn renderer_options(&self) -> egui_wgpu::RendererOptions; // msaa_samples / depth_stencil_format
}

// ── neo-render/src/renderer.rs ──────────────────────────────────────────────
pub struct MapRenderer {
    caps: GpuCaps,
    fill: FillPipeline,
    stroke: StrokePipeline,
    point: PointPipeline,
    glyph: GlyphPipeline,
    pools: BufferPool,
    atlas: GlyphAtlas,
    picking: PickingPass,
    timestamps: Option<TimestampQueries>,
}

impl MapRenderer {
    /// Creates pipelines and static resources only; does not own the Instance/Device.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, caps: GpuCaps) -> Self;

    /// Once per frame: upload dirty ranges + write the uniform + optionally encode the pick pass.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &SceneFrame,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
    ) -> PrepareStats;

    /// Forwarded by `CallbackTrait::paint`; only issues draws, never creates passes or submits.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'static>, info: egui_wgpu::PaintCallbackInfo);
}

// ── neo-render/src/buffer.rs ────────────────────────────────────────────────
pub struct FrameSlot {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub instances: wgpu::Buffer,
    pub uniform: wgpu::Buffer,
}
pub struct BufferPool { /* slots: [FrameSlot; 2], cur: usize, keys: HashMap<BatchKey, Range<u32>> */ }
impl BufferPool {
    pub fn new(device: &wgpu::Device, caps: &GpuCaps) -> Self;
    pub fn begin_frame(&mut self, version: u64) -> bool;      // true = version changed
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &SceneFrame) -> UploadStats;
    pub fn slot(&self) -> &FrameSlot;
    pub fn trim(&mut self, budget_bytes: u64) -> bool;        // called only during cross-slot defragmentation
}

// ── neo-render/src/picking.rs ───────────────────────────────────────────────
pub struct PickingPass { /* R32Uint target, Depth32Float, 256B-aligned readback, pending queue */ }
impl PickingPass {
    pub fn new(device: &wgpu::Device, caps: &GpuCaps) -> Self;
    pub fn resize(&mut self, device: &wgpu::Device, size_px: [u32; 2]);
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, frame: &SceneFrame, pool: &BufferPool) -> wgpu::CommandBuffer;
    pub fn request(&mut self, px: [u32; 2]) -> PickRequestId;                  // asynchronous
    pub fn try_take(&mut self, device: &wgpu::Device, id: PickRequestId) -> Option<Option<ObjId>>; // Poll, non-blocking
    pub fn take_blocking(&mut self, device: &wgpu::Device, id: PickRequestId) -> Option<ObjId>;    // PollType::wait_indefinitely()
}

```

## 4 Sequencing

```
Frame N (steady state, version unchanged)
  egui::Context::run
    │
    ├─ ui.painter().add( MapCallback { frame } )
    │
    ├─ prepare():  pools.begin_frame(v) → false (unchanged)
    │              upload: uniform 32 B only
    │
    ├─ egui tessellate → clipped_primitives (including MapCallback)
    │
    ├─ eframe: begin_render_pass("egui_render")
    │    color: swapchain/msaa view, load = Clear(clear_color)
    │    depth: only when RendererOptions::depth_stencil_format.is_some()
    │
    ├─ renderer.render(): egui's own mesh and our paint execute in primitive order
    │    └─ MapCallback::paint → fill → stroke → point → glyph
    │
    └─ present
```

```
click(x,y) ─► PickingPass::request(px)                 // frame N, returns PickRequestId
prepare(frame N)  encode: R32Uint pass (visible set only) → copy_texture_to_buffer(1×1, bytes_per_row = 256)
prepare(frame N+1) try_take(): PollType::Poll (non-blocking) → ObjId = id >> 8; Pending waits another frame, gives up after >3 frames
hover path always uses the CPU (neo-geo::HitTester) and never triggers readback
```

## 5 Tradeoffs and Alternatives

| Decision | Alternative | Why this one | Cost |
|---|---|---|---|
| Share eframe's Device/Queue and draw via `CallbackTrait` | Build our own `Instance`/`Surface`/event loop | Single queue, single present, no second VRAM allocation; DPI/window/IME all handled by eframe | Constrained by egui's pass: load op cannot be changed, depth must be self-provided, MSAA is decided by `RendererOptions` |
| Pre-generated geometry + dirty-range upload | Write the visible set into vertex buffers every frame | Steady-state upload ≈ 0, CPU and GPU decoupled | One upload spike at `version` boundaries, needs frame budgeting |
| Painter's algorithm (no depth) | Depth attachment + depth test | A 2D map already needs style-based z sorting; egui's depth `store = Discard` makes cross-callback depth unreliable | Phase 2 extrusion would need its own prepass |
| Analytic AA (stroke) | Rely entirely on MSAA | Line width/dashes stay clean at 1x; turning MSAA off loses no quality | A few extra fragment instructions |
| Premultiplied alpha end to end | Straight alpha + per-pipeline blend state | Consistent with egui; overlapping areas do not darken | One extra multiply when packing colors (cached) |
| CPU picking as primary | Full GPU ID buffer | No flush, no latency, reuses the existing index | Interior-area precision depends on triangulation consistency |
| `write_buffer` for small segments + staging for large ones | All `write_buffer` | Avoids implicit staging and blocking on large writes | Two upload paths must be tested separately |
| Indirect drawing enabled on demand | Always indirect | Loop draws are simpler and more readable for tens of batches | Both paths must be maintained under extreme styles |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Pipelines do not match egui's pass (format/count/depth) | Runtime validation errors, whole frame black | Single source of truth `GpuCaps` generates pipelines; 1-frame startup self-check |
| R2 | Double gamma (`*Srgb` platforms) | Map looks washed out/gray, inconsistent with UI | Startup branching + golden images for both paths |
| R3 | Silent regression to per-frame buffer rebuilds | Frame time rises linearly with object count | `PrepareStats.uploaded_bytes` assertion (steady state < 4 KiB) + CI benchmarks |
| R4 | Pick readback blocks the main thread | Click drops frames to < 20 fps | Force the async path; hover forbids readback; fallback on timeout |
| R5 | Software adapter (WARP) lacks MSAA/timestamps | Startup failure or black screen | Capability probing + degradation table; `Error::Wgpu` shows readable information instead of panicking |
| R6 | Font/atlas cache bloat | Memory and first-popup latency | Size buckets + LRU page reclamation; lazy CJK loading |
| R7 | egui version drift (0.36.x → 0.37) | `RenderState`/panel/callback details change (the `App::ui` entry point is already in place in 0.36) | Confine changes to `apps/neo-desktop` and `neo-render` only (see the version note in §8) |

## 7 Acceptance Criteria

1. **Zero re-upload in steady state**: over 60 idle frames while only panning, `PrepareStats.uploaded_bytes` is ≤ 128 B per frame (uniform), and `draw_calls` is stable.
2. **Frame time**: on the reference machine (i5-12400F / 32 GB / RTX 3070 / Windows, DX12) at 1M and 10M objects and zoom level 16, main-thread and GPU times are [pending measurement, see docs/14-performance-and-verification.md]; the acceptance threshold is ±20% of the design budget table.
3. **Visual consistency**: with the same dataset and styles from [11](11-style-presets-mapcss.md), screenshot SSIM across DX12/Vulkan/Metal is ≥ 0.99 (allowing one tier of MSAA/font difference).
4. **Color**: under both `Bgra8Unorm` and `Bgra8UnormSrgb` `target_format`s, pixel differences on the same golden image are ≤ 2/255.
5. **Picking**: GPU and CPU results agree on ≥ 99% of 1000 random points (differences must be attributable to triangulation vs analytic geometry); no functionality is missing when readback is disabled.
6. **Robustness**: with WARP, `msaa_samples = 1`, and no timestamps, the application can start, be interacted with, and never panics.
7. **Dependency discipline**: `cargo tree -p neo-render` does not contain `neo-store`/`neo-editor`/`neo-ui` ([02](02-workspace-and-dependency-rules.md) §7 item 2).

## 8 References

- egui-wgpu `CallbackTrait` / `RenderState` / `RendererOptions` (**0.36.2**; this repository follows that version): <https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/>
- `egui_dock` 0.21.1 depends on `egui ^0.36` (hence 0.37 is not possible): <https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies>
- Breaking changes in wgpu 30 (`Maintain` removed, `DeviceDescriptor`, `multiview_mask`): <https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md>
- `Limits::downlevel_defaults` / feature tables: <https://docs.rs/wgpu-types/30.0.2/wgpu_types/struct.Limits.html>
- WebGPU spec (`copyTextureToBuffer`'s 256 B `bytesPerRow` alignment): <https://www.w3.org/TR/webgpu/#dom-gpucommandencoder-copytexturetobuffer>
- Unverified: current versions of `cosmic-text`/`glyphon` and their support for wgpu 30 **> To be verified**; WARP's exact capability matrix **> To be verified** (also noted in [01](01-system-architecture.md) §8).


## Node LOD (from 2026-11-15)

Node icons are not drawn unconditionally but fall into three tiers with zoom (`neo-scene::builder::{NODE_MIN_ZOOM, NODE_ALL_ZOOM}`):

| zoom | What is drawn | Shape |
|---|---|---|
| ≤ 16 (z16 and below) | No nodes are drawn | — |
| 17 – 18 | Only **shared nodes** (referenced by ≥2 ways, `Referrers::ways_of_node().len() >= 2`) | Small dot (hexagon approximation, 7 vertices/6 triangles) |
| ≥ 19 | All nodes | Shared node = small square (4 vertices/2 triangles); ordinary node = small dot |

The shared-node check relies on `Referrers` (the CSR reverse lookup table), so the signature of `SceneBuilder::build` must accept `&Referrers`;
the desktop reuses the reverse lookup table already maintained by `Editor`, with no extra preprocessing cost. The toolbar no longer has a "show nodes" toggle,
and behavior is fully determined by zoom, avoiding dense-area noise caused by manual toggling.

**Picking and rendering share the same rule** `neo_scene::node_visible_at(zoom, way_ref_count)`:
invisible nodes cannot be clicked or dragged; after zooming, the application layer automatically clears selections of nodes that have become invisible (way selections are kept).


## Selection/Drag Screen Margins

| Purpose | Node | way |
|---|---|---|
| Click selection (`NODE_PICK_MARGIN_PX` / `WAY_PICK_MARGIN_PX`) | 16 px | 16 px |
| Drag after selection (`SELECTION_DRAG_MARGIN_PX`) | 16 px | 16 px (distance to the polyline) |

The margins for the two element types are **computed independently** (`HitTester::pick_with_margins`): if the mouse is within the node margin, a node is selected (even if the node lies on a way, the way is not wrongly selected);
otherwise a way within the way margin is selected. Decisions use the mouse **press origin**, avoiding misjudgment when the pointer has already moved by the time egui's drag threshold triggers.
Invisible nodes (per zoom LOD) do not enter the picking flow: `pick_with_margins_filtered` filters hidden nodes **at the picking stage**
and falls back to the way beneath them — otherwise hidden nodes would intercept way clicks, producing the problem that "clicking the feature itself does not select it, but clicking a few pixels beside it does".


## Relationship Between Dragging and Selection

Dragging requires a selection as a precondition, and **pressing automatically completes the selection**:

1. Pressing the left button on an unselected node/way → immediately make it the current selection (red box appears), then begin dragging;
2. Dragging an already-selected feature (or within its margin) → drag the whole selection set together (when multiple are selected);
3. On release, commit **1 history entry** (batched `MoveNode`); the selection is retained and the red box is displayed at the moved position.

Thus "click-drag" completes "select + move" in one step, and there is no state where "it was dragged but not selected".
