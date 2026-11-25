# ADR-0001 · Rust Native + GPU-first

- **Status**: Accepted, 2026-11-15
- **Reversibility**: **Low**. It determines the process model, render ownership, UI framework, and the dependency DAG of 16 crates; changing it would cost nearly a rewrite.
- **Scope**: `apps/neo-desktop`, `neo-ui`, `neo-render`, `neo-scene`, `neo-store`; all performance-related docs.
- **Related**: [01 System Architecture](../01-system-architecture.md) | [02 Dependency Rules](../02-workspace-and-dependency-rules.md) | [05 Renderer](../05-renderer-wgpu.md) | [16 Roadmap and Risks](../16-roadmap-and-risks.md) | [ADR-0005](0005-wgpu-egui-shared-device.md)

## Status

Accepted. If later evidence (prototype Gate metrics, backend differences, maintenance cost) overturns any sub-decision, record the alternative in a new ADR rather than silently editing this document.

## Background

NEO aims to **fully replace JOSM's editing workflow**, with data sets of 1M–10M+ OSM objects, running natively on Windows / Linux / macOS. JOSM is Java + Swing: feature-complete with a mature ecosystem, but its heap memory and per-primitive drawing model hit a ceiling on large data sets, and startup and input latency are also affected by the JVM.

Three independent but coupled choices lie ahead:

1. **Language and runtime**: keep the JVM (Java/Kotlin), .NET, or native Rust?
2. **Drawing model**: keep CPU immediate-mode (Swing/egui built-in painting), or build a custom GPU pipeline?
3. **UI shell**: Tauri/Electron (WebView), Qt/GTK (C++ bindings), or native Rust immediate-mode (egui)?

They cannot be chosen separately: choosing a WebView means giving up a custom wgpu pipeline (it requires embedding an independent window handle, and the three-platform approaches differ); choosing a custom pipeline means the UI layer must share frames with the same `Device`/`Queue`; choosing native Rust means JOSM's GPL code cannot be reused (static linking or rewriting propagates the license), so semantic compatibility can only come from behavioral alignment.

## Decision

**Rust Native + GPU-first, single-process modular monolith.**

| # | Sub-decision | Details |
|---|---|---|
| D1 | Language and runtime | Rust stable (1.100.0 locally, MSRV 1.95), no GC, no JVM; `#![forbid(unsafe_code)]` is the default for library crates |
| D2 | Process model | Single process, multi-threaded; the UI thread is the only writer, everything else reads snapshots ([ADR-0006](0006-single-writer-editor.md)) |
| D3 | UI stack | egui / eframe / egui-wgpu **0.36.2** (all three at the same version); versions are constrained by `egui ^0.36` from `egui_dock 0.21.1` and `egui-phosphor 0.14.0` |
| D4 | Rendering | In-house wgpu **30.0.2** map renderer, sharing `Device`/`Queue` with egui, drawing inside egui's render pass via `egui_wgpu::CallbackTrait` ([ADR-0005](0005-wgpu-egui-shared-device.md)) |
| D5 | No refactoring by "porting Java classes" | No one-to-one translation of JOSM classes, no JNI/transpilation bridge; only rewrite with behavioral equivalence (parity is defined by workflow, see [13](../13-josm-parity.md)) |
| D6 | No Tauri / Electron | Do not use a WebView as the map host layer, and do not put the data plane in JS/IPC |

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| Continue Java/Kotlin (replicating the JOSM architecture) | Can directly reference the existing implementation and plugin ecosystem | Heap memory and GC peaks are uncontrollable; a `HashSet<Node>`-style model cannot meet the target at 10M objects; the GPU pipeline would cross JNI, cancelling out the gain |
| Tauri + React (WebView frontend) | Web ecosystem, frontend talent, fast UI iteration | A WebView cannot host a custom wgpu pipeline (three-platform embedding is non-uniform); 10M primitives over an IPC/JS bridge is infeasible |
| Electron | Same as above, and more mature | Same as above, plus a 100 MB+ runtime and memory overhead |
| Qt (`qmetaobject` / `cxx` bindings) | Mature dock, accessibility, IME; native look and feel | LGPL/commercial dual licensing and static-linking compliance costs; C++ object lifetimes cross Rust's borrow checker, making FFI a long-term bug source |
| GTK (`gtk-rs`) | Best native feel on Linux | Noticeably behind on Windows in look and feel and IME; poor three-platform consistency |
| Pure CPU rendering (draw the map directly with egui `Mesh`) | Zero new infrastructure | Re-tessellating millions of line segments every frame is infeasible for both CPU and memory |
| Reuse JOSM code directly (JNI / transpile / rewrite) | Semantic parity for free | GPL-2.0+ is contagious; JVM startup and heap model conflict with GPU-first; it just moves the performance problem into a new process |
| Browser WASM build (incidental artifact) | Low distribution cost | The 4 GB wasm32 address space cannot hold 10M objects plus indexes; limited filesystem |

## Consequences

**Positive**

- Single process, single address space: snapshot swaps are zero-copy, picking and rendering happen in the same frame, no IPC latency.
- No GC: predictable frame times; memory layout (SoA) is explicitly controllable, which is the only viable path at 10M objects.
- Custom wgpu pipeline: batching, instancing, GPU picking, and text atlases can be designed as needed without being limited by the UI framework's immediate mode.
- Clean licensing: MIT/Apache-2.0 dual license, no mixing with GPL assets; JOSM is a behavioral reference only.
- Controllable dependency surface: `neo-render` depends only on `wgpu` + `egui-wgpu` + `bytemuck`, not on store/editor/ui.

**Negative / costs**

- **Own the ecosystem**: dock, tables, property editors, i18n, and accessibility all have to be built on egui by ourselves; `egui_dock` is only the starting point for docking.
- **GUI stack version lock**: 0.36.2 was chosen for compatibility with `egui_dock`/`egui-phosphor`; 0.36 → 0.37 is a breaking upgrade (entry trait shape, `RenderState` field changes), and upgrading requires a dedicated rehearsal ([02](../02-workspace-and-dependency-rules.md) §2.6).
- **Constrained by egui's pass**: cannot change the load op, cannot reuse depth across callbacks, color format must match the swapchain; pipeline parameter errors are **runtime** validation errors rather than compile errors.
- **No crash domain in a single process**: a plugin/driver crash takes all unsaved edits with it; only journal + autosave + WASM sandbox provide a safety net.
- **No dedicated render thread**: the UI thread triples as input + command + submit, so the frame budget is tighter; extreme cases can only lower LOD.

## Verification

1. **Buildable**: `cargo build --workspace --no-default-features --features desktop` succeeds on all three platforms (minimal build set).
2. **Single device**: `Instance::new` / `new_without_display_handle` appears exactly once across the repo (grep assertion); `cargo tree -p neo-render` contains no `neo-store`/`neo-editor`/`neo-ui`.
3. **Prototype baseline**: `apps/neo-prototype` runs "import 1M objects → render → pan/zoom → pick" on the reference machine, with the four baselines (import time, first frame, frame time p50/p99, peak RSS) measured and recorded; failing to meet them triggers the G1 veto process in [16](../16-roadmap-and-risks.md) §2.8.
4. **Version consistency**: no duplicate major versions of egui / wgpu / winit / rstar in `cargo tree -d`; `Cargo.lock` changes appear in the PR description.
5. **License**: the dependency tree of `neo-render`/`neo-ui` contains no GPL-contagious licenses (audit script + manual review, see [17](../17-licensing-and-governance.md)).
6. **Falsifiability of behavioral equivalence**: every item in the parity list ([13](../13-josm-parity.md)) is an executable check or a manual step; there are no unfalsifiable statements like "architecturally it should be consistent".

## References

- JOSM project and source (GPL-2.0+, behavioral reference only): https://josm.openstreetmap.de/
- JOSM licensing: https://josm.openstreetmap.de/wiki/Source
- eframe 0.36.2: https://docs.rs/eframe/0.36.2/eframe/
- egui-wgpu 0.36.2 (`CallbackTrait` / `RenderState`): https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/
- wgpu 30.0.2: https://docs.rs/wgpu/30.0.2/wgpu/
- The `egui ^0.36` constraint of `egui_dock` 0.21.1: https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies
- Qt licensing (LGPL/commercial): https://www.qt.io/licensing
- WebGPU specification: https://www.w3.org/TR/webgpu/

> To be verified: the `eframe::App`/panel API differences between the 0.37 line and 0.36.2 (`App::ui` is already in place in 0.36; verify item by item during the upgrade rehearsal); whether egui's accessibility (accesskit) coverage meets the minimum requirements for a desktop editor.
