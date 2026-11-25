# ADR-0005: Share the same wgpu device with egui (no separate render backend)

- **Status**: Accepted
- **Date**: 2026-11-15
- **Decision scope**: `apps/neo-desktop` (startup), `neo-render` (all drawing), `neo-ui` (paint order)
- **Version baseline**: egui / eframe / egui-wgpu **0.36.2**, wgpu **30.0.2**, winit 0.30.13
- **Related**: [05-renderer-wgpu.md](../05-renderer-wgpu.md) | [01-system-architecture.md](../01-system-architecture.md) | [ADR-0010](0010-mapcss-compatibility.md)

## Background

NEO's UI (panels, menus, dialogs, property tables) uses egui; the map itself has vector batches of 1M–10M objects and must have its own wgpu pipeline, buffer pool, text atlas, and picking pass. Two routes:

1. **Share** the `Instance`/`Device`/`Queue` created by eframe and draw the map inside egui's render pass via `egui_wgpu::CallbackTrait`;
2. **Build a second** `Instance` + `Surface` + event loop (or offscreen texture composition), with the map and UI presenting separately.

This is not a matter of taste but is constrained by hard facts: two `wgpu::Instance`s in the same process mean two sets of adapter enumeration, two swapchains, and two present timings; meanwhile winit's window events, DPI changes, IME, and accessibility are already owned by eframe, and taking them over again causes input latency and focus contention.

At the same time, the cost of sharing is that **we must obey egui's render pass**, and these constraints are runtime errors rather than compile errors (the most typical: when pipeline `MultisampleState.count` / `ColorTargetState.format` / `DepthStencilState.format` do not match the pass, validation errors are only reported at draw time).

## Decision

**Adopt option 1**, and accept the following concrete constraints:

| # | Constraint | Rationale |
|---|---|---|
| D1 | Only use the `adapter` / `instance` / `device` / `queue` / `target_format` / `renderer` provided by `eframe::egui_wgpu::RenderState`; **do not create a second `Instance`** | Single queue, single present; `RenderState` is `Clone` and all fields are public |
| D2 | Map drawing implements `egui_wgpu::CallbackTrait`: `prepare(&self, &Device, &Queue, &ScreenDescriptor, &mut CommandEncoder, &mut CallbackResources) -> Vec<CommandBuffer>` and `paint(&self, PaintCallbackInfo, &mut RenderPass<'static>, &CallbackResources)` | In `paint` we are already inside the pass, so creating a new pass / `submit` is forbidden; work that needs an encoder goes in `prepare` and returns the `CommandBuffer` to egui |
| D3 | Persistent GPU resources (pipelines, `BufferPool`, atlas, picking textures) are stored in `Renderer::callback_resources` (`type_map`), using `get_mut` or otherwise `insert` | Lifetime is bound to `Renderer`; do not hold a `Renderer` write lock across frames (it deadlocks with the read lock in the paint callback) |
| D4 | All pipelines use `RenderState::target_format` for the color format; explicitly set `msaa_samples` and (if needed) `depth_stencil_format` in `RendererOptions` | egui has no depth by default and depth `store = Discard`; MSAA is resolved uniformly by egui |
| D5 | Vertices and colors use the same blend state as egui under **premultiplied alpha** | Consistent with egui itself; avoids dark edges where UI and map overlap |
| D6 | Write only a 32 B camera uniform per frame; geometry buffers are uploaded only by dirty ranges when `SceneFrame.version` changes | Zero re-uploads in steady state, see [05](../05-renderer-wgpu.md) §2.8 |
| D7 | Pick readback must never block the UI thread; `PollType::wait_indefinitely()` may only appear on export/test paths | One wait = whole-pipeline flush, causing hover frame drops ([07](../07-concurrency-and-tasks.md) §D10) |
| D8 | Self-built offscreen rendering (PNG export, golden images) must also use the same `Device`, and explicitly handle `CurrentSurfaceTexture::Lost` / 256 B row pitch | wgpu 30's `get_current_texture` returns an enum, and only `Queue::present(tex)` presents it |

## Alternatives (evaluated and rejected)

| Option | Pros | Why rejected |
|---|---|---|
| Build a separate `Instance` + `Surface` + self-managed winit loop | Full control over present timing and load op | Input/DPI/IME/accessibility would contend with eframe for ownership; the two presents tear against each other; VRAM and adapter enumeration double |
| Render the map to an offscreen texture, then upload it as an egui texture | No `CallbackTrait`, decoupled from egui | One extra full-screen composition (bandwidth = viewport pixels × 4 B × frame rate), and picking/viewport/DPI must be aligned manually; this bandwidth is non-negligible in 10M-object scenes |
| Draw the map directly with egui's `Mesh` (bypassing wgpu) | Zero new infrastructure | Re-tessellating millions of line segments every frame is infeasible for both CPU and memory (egui's mesh is immediate) |
| Wait for official 3D integration with multiple viewports from egui | Cleaner in the future | Not available today; `CallbackTrait` is the officially supported extension point with a stable signature |

## Consequences

**Positive**

- Single device, single queue; UI and map share the same present, with no tearing and no second swapchain.
- DPI/scaling/window lifecycle are handled by eframe; we only need to treat `PaintCallbackInfo::viewport_in_pixels()` as the viewport and `pixels_per_point` as the UI scale.
- Picking, hover, and selection highlights are in the same frame as the UI, with no 1-frame input latency.
- Small dependency surface: `neo-render` depends only on `wgpu` + `egui-wgpu` + `bytemuck`, not on `neo-store`/`neo-editor`/`neo-ui`.

**Negative / costs**

- Constrained by egui's pass: cannot change the load op, cannot reuse depth across callbacks, MSAA is decided by `RendererOptions`, and the color format must exactly match the swapchain (including the sRGB branch).
- Pipeline creation parameters and the runtime pass must be derived from a **single source** (`GpuCaps`), otherwise validation errors appear that trigger only on certain backends.
- In-frame order is decided by egui's primitive order: the map callback must be added at the correct `ui` level so that overlays (selection boxes, handles) are drawn above the map.
- egui version upgrades are **breaking**: in 0.36 the `eframe::App` entry point is `ui()` (`update` was removed), and 0.36 → 0.37 brings further `RenderState`/panel API changes; upgrade points must be confined to `apps/neo-desktop` and `neo-render`.

**Constraints (must be encoded in CI)**

1. `cargo tree -p neo-render` contains no `neo-store` / `neo-editor` / `neo-ui`.
2. Only one `wgpu::Instance` is allowed across the repo (source grep for `Instance::new` / `new_without_display_handle`).
3. Pipelines may only be created via `MapRenderer::new`; creating resources in `paint` is forbidden.

## Verification

- **Startup self-check**: on the first frame, validate each pipeline's `format` / `sample_count` / `depth` against the current pass (panic immediately with a readable message if mismatched), replacing "black screen after a few minutes".
- **Steady-state assertion**: idle and panning for 60 frames, `PrepareStats.uploaded_bytes ≤ 128 B/frame` (uniform only).
- **Color**: the pixel difference of the same golden image under `Bgra8Unorm` and `Bgra8UnormSrgb` `target_format` is ≤ 2/255.
- **Picking**: no frame calls `PollType::wait_*` (source grep + unit test); GPU and CPU picking agreement ≥ 99%.
- **Degradation**: combinations of WARP / no MSAA / no timestamp can start, interact, and do not panic.
- Frame-time acceptance numbers see [14](../14-performance-and-verification.md) [to be measured].

## References

- `egui-wgpu` 0.36.2's `CallbackTrait` / `RenderState` / `RendererOptions`: <https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/>
- The depth `Clear(1.0)` / `StoreOp::Discard` and MSAA resolve of egui's render pass: `egui-wgpu`'s `paint_and_update_textures`
- wgpu 30 breaking changes (`Maintain` removal → `PollType`, `DeviceDescriptor`, `multiview_mask`): <https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md>
- `egui_dock` 0.21.1 requires `egui ^0.36` (this project therefore pins 0.36.2): <https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies>
