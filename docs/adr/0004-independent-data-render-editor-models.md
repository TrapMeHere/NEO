# ADR-0004 · Data / render / edit models are mutually independent

## Status

- Accepted: 2026-11-15.
- Reversibility: **Medium**. It determines the split and public signatures of the four crates `neo-store` / `neo-scene` / `neo-render` / `neo-editor`.
- Scope: the POD constraint on `SceneFrame`, type firewalls, incremental update strategy ([ADR-0009](./0009-incremental-update.md)).
- Related docs: [01](../01-system-architecture.md) §2.3, [02](../02-workspace-and-dependency-rules.md) §2.4, [05](../05-renderer-wgpu.md).

## Background

The place where an OSM editor rots most easily is "letting rendering read the data structures directly, letting editing modify render objects directly". JOSM's `OsmDataLayer` mixes data, drawing data, and commands, with the result that adding a rendering feature requires changing the data layer; render-cache and edit consistency rely on convention; and unit tests must spin up the UI.

NEO needs incremental updates, parallel scene building, and GPU picking on 10M objects, so it must first nail down the boundaries of the "three models"; otherwise every layer will reach upward.

## Decision

The system contains three models that **share no mutable state**, with only one-way projections allowed:

```
 Edit model                   Data model                     Render model
 EditCommand/EditTx ──commit──► DataSet ──publish──► Snapshot ──build_scene──► SceneFrame
 (write-set, shadow layer)     (SoA, sole mutable)   (immutable, Arc)      (POD vertices/batches)
      ▲                                                                        │
      └────── tool intent (EditCommand) ◄── ObjectId ◄── GPU id pass ◄─────────┘
```

| Model | Owning crate | Contents | Who may read | Who may write |
|---|---|---|---|---|
| Data model | `neo-store` (entities from `neo-osm`) | `DataSet` (SoA columns, indexes, reference sets, dirty), `Snapshot` (frozen view) | All upper layers, read-only | Only `EditTx` (via `Editor`) |
| Edit model | `neo-editor` / `neo-tools` | `EditCommand`, `EditTx`, `History`, `ChangeSet`, `Conflict` | `neo-ui`, `neo-plugin` | Only the editor itself |
| Render model | `neo-scene` / `neo-render` | `SceneFrame` (batches, `MapVertex`, pick id table), GPU buffers | `neo-render` exclusively | Only `neo-scene` builds, `neo-render` uploads |

**Four hard constraints**:

1. `SceneFrame` is POD: no `Arc`, no `Snapshot` reference, no `ObjectRef` borrow; pick ids are `u32`, and the mapping table is owned by `SceneFrame`.
2. `neo-render`'s public signatures contain no `neo_store`/`neo_editor` types (type firewall).
3. `Snapshot` has no `&mut self` methods; rendering always holds an immutable view of some version.
4. The edit model does not produce render data; the render model does not understand OSM semantics (it does not know what a way, a tag, or a relation is).

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| ECS shared world (entities/components/systems) | Parallel systems, data-oriented | Editing needs "global order + inverse operations", and ECS parallel systems naturally break the order; way→node reference sets are not component relationships |
| Render directly from `Snapshot` every frame (no intermediate model) | No derived memory, no consistency problems | Per-primitive drawing at 10M objects is infeasible; cannot batch by layer; GPU picking has nowhere to attach ids |
| Retained scene graph | Naturally incremental, stable structure | The scene graph becomes entangled with data lifetimes; delete/undo easily leaves dangling nodes; style switches require traversing the graph |
| Editing directly modifies render objects and syncs back to data | One less copy | Data is no longer the single source of truth; undo/upload/validation all lose their basis |
| Single "god model" (render cache fields inside data) | Fastest access, no conversion | Cache invalidation logic is scattered; serialization/persistence gets polluted by caches; tests cannot be separated |

## Consequences

**Positive**

- The three models can be tested independently: headless edit tests, `SceneFrame` snapshot tests, and GPU smoke tests do not depend on each other.
- Scene building can be parallelized on Rayon (read-only `Snapshot`), and editing and rendering do not block each other.
- `SceneFrame` is pure data and can be passed directly to the GPU with `bytemuck`; the pick id table means picking does not rely on reverse geometry lookups.

**Negative**

- **Derived memory**: `DataSet` and `SceneFrame` coexist; at 10M objects this must rely on LOD and visible-set culling, otherwise memory doubles (R6).
- **Conversion cost**: every commit must rebuild the affected batches; miscalculating the invalidation scope causes "the picture lags behind the data" (suppressed with version keys and dirty ranges).
- **Mapping maintenance**: the pick id table is rebuilt with batches; pick results must validate `Handle.gen`, otherwise after an undo a pick may hit a deleted object.

## Verification

1. Type firewall: `neo-render`'s API snapshot contains no `neo_store`/`neo_editor` paths.
2. PODness: compile-time `bytemuck::Pod` assertions on `SceneFrame`/`MapVertex`, and the structs contain no `Arc`/`&`/`String`.
3. Pointer identity: after a commit, the `Snapshot` obtained by the render thread and the `Arc` published by the writer satisfy `Arc::ptr_eq`.
4. Consistency: after 10k random edits + scene rebuilds, every id in `pick_ids` resolves to an object in the corresponding `Snapshot` version (property test).
5. Independence: after removing the `neo-render` dependency, all `neo-editor` tests still pass.
6. Picking correctness: on a `Handle.gen` mismatch, must return `None` (not the old object); construct a delete-recreate sequence test.

## References

- OSM data model (Node/Way/Relation semantics): https://wiki.openstreetmap.org/wiki/Elements
- JOSM layers and drawing model (counterexample and behavioral baseline): https://josm.openstreetmap.de/wiki/Help/Concepts/Object
- `bytemuck` (POD and zero-copy upload): https://docs.rs/bytemuck/latest/bytemuck/
- wgpu vertex buffer layout: https://docs.rs/wgpu/30.0.2/wgpu/struct.VertexBufferLayout.html
