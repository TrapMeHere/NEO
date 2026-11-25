# Architecture Overview

NEO is a **modular monolith**, not a microservice architecture. Core principles:

1. **Business logic does not depend on the GUI**: editing/IO/querying can be tested and run without a UI.
2. **Map rendering does not depend on UI widgets**: the render layer only receives an immutable `SceneFrame`.
3. **The data model does not depend on the render format**: `DataSet` is SoA + Arena, and GPU vertices are built separately by `neo-scene`.
4. **Single writer**: all modifications go through `neo-editor`'s Command/Transaction; the GUI must not modify `DataSet` directly.
5. **Incremental**: geometry/index/GPU upload are all incremental; panning and zooming only update uniforms.

## Layers

```
apps/neo-desktop        eframe + wgpu integration, input translation, panel orchestration
      │
crates/neo-ui           egui panels (pure functions: read-only data + UiActions)
crates/neo-tools        EditController: select/draw/delete/drag/split/reverse/merge
      │
crates/neo-editor       commands, transactions, history, journal, selection, Editor
      │
crates/neo-io  neo-persist  neo-api (phase 5)
      │
crates/neo-scene        SceneBuilder, LOD, batching
crates/neo-render       wgpu pipeline, GpuScene, headless rendering
crates/neo-validate / neo-query / neo-plugin / neo-style (phases 4–6)
      │
crates/neo-store        DataSet (SoA + Arena), indexes, backreferences, integrity
crates/neo-osm          OSM entities/change types/tag model
crates/neo-geo          world coordinates, camera, picking, geometry
crates/neo-core         IDs, coordinates, generational handles, Meta, errors
crates/neo-i18n         UI text catalogs (embedded at compile time)
```

## Data flow of one edit

```
egui input
  → neo-tools::EditController (pick + command)
  → neo-editor::Command (transaction/history/journal)
  → DataSet change (version++)
  → neo-scene::SceneBuilder (incremental rebuild)
  → neo-render::GpuScene upload
  → wgpu draw (egui paint callback)
```

Picking is independent of rendering: `neo-geo::HitTester` uses the same index and selects nodes/ways by screen-space margin.

## Key invariants

- Handles carry a generation; after deletion the old handle is invalid (it will not point to a new object).
- `DataSet::version` increments on every structural change; the scene cache is keyed by it and (origin, zoom).
- A node referenced by a way/relation cannot be deleted directly (the command returns `Referenced`).
- Drag previews do not write data; only on mouse release is one batch-move command produced.
- When the layer is read-only: picking/dragging/drawing/measuring are all disabled, and the selection is cleared.

## Further reading

- [01 System Architecture](../01-system-architecture.md)
- [02 Workspace and Dependency Rules](../02-workspace-and-dependency-rules.md)
- [03 Data Model and Storage](../03-osm-data-model-and-store.md)
- [04 Spatial Index and Geometry](../04-spatial-index-and-geometry.md)
- [05 wgpu Renderer](../05-renderer-wgpu.md)
- [06 Editing Core and Undo](../06-editing-core-and-undo.md)
- [ADR List](../SUMMARY.md)
