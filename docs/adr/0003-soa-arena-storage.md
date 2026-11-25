# ADR-0003: Store OSM data with SoA + Arena + generation handles

- **Status**: accepted
- **Date**: 2026-11-15
- **Related**: docs/03, ADR-0004, ADR-0009

## Background

OSM data is a graph of "many small objects + sparse references":

- Tens of millions of Nodes (ID, coordinates, tags, metadata);
- Millions of Ways (ID, node reference sequence, tags, metadata);
- Hundreds of thousands of Relations (members, roles, tags, metadata);
- Editing frequently modifies individual objects and needs to quickly find the "Ways/Relations that reference it".

Java JOSM's `OsmPrimitive` object model suits Java's style of object management, but translating it directly to Rust brings:

1. One heap allocation per object + pointer chasing (cache-unfriendly);
2. `Vec<Box<Node>>` cannot do batch processing over contiguous memory (GPU upload, parallel geometry building);
3. After deletion/undo, referencing objects may have dangling pointers or silent "old handle points to new object" errors.

## Decision

Adopt **SoA + Arena + typed generation handles**:

```text
NodeStore {
    ids:   Vec<ObjectId>       // 8 B
    lat/lon: Vec<i32>          // 4+4 B (fixed point, 1e-7 degrees)
    tags:  Vec<TagSetId>       // 4 B (a range into the Tag arena)
    meta:  Vec<Meta>           // 32 B
    alloc: SlotAllocator       // generations + alive + freelist
}
```

- **Handle**: `Handle<T> { index: u32, generation: u32 }`, 8 bytes, `Copy`, independent of `T`'s trait bounds.
- **Invalidation detection**: the generation is incremented when a slot is freed, so old handles are guaranteed invalid; `Handle` never reuses an old generation.
- **Reference pool**: a Way's node references live in a contiguous `Vec<Handle<Node>>`, with each way recording `(offset, len)`; edits append a new range and old ranges are reclaimed during compact.
- **Tag arena**: all `(Sym, Sym)` pairs are stored contiguously; a TagSet only stores ranges; the write path is append-only, so editing needs no reference counting and carries no invalidation risk.
- **String interning**: tag text is stored only once, `Sym` is 4 bytes.
- **Reverse indexes**: node→ways / node→relations / way→relations use CSR (built once in O(n+e), with zero-allocation queries).
- **World coordinates do not enter storage**: storage keeps only the authoritative fixed-point lat/lon; WebMercator world coordinates are computed on demand by `neo-geo`.

## Alternatives

| Option | Why rejected |
|---|---|
| `Vec<Box<Node>>` / `Vec<Node>` + pointers | Many allocations, poor cache behavior, cannot do batch SoA processing; deletion is O(n) |
| `slotmap`/`generational-arena` crates | One generic arena per object type is still AoS; cannot process column-wise (e.g., parallel geometry building that only reads lat/lon/tags) |
| ECS (bevy_ecs / hecs) | A general ECS's archetype/Component overhead does not match OSM's "exactly three object types"; edit transaction semantics would have to be built separately |
| `im`/`rpds` persistent data structures | Modifying a single object is expensive (path copying), and memory blows up at tens of millions of objects |
| SQLite as primary storage | Querying a database every frame/for every pick cannot possibly meet the interaction budget; only suitable for persistence/caching (docs/10) |
| Embedding `Vec<(String,String)>` in objects | tag text is stored repeatedly, adding hundreds of MB at 10M objects and exploding the number of allocations |

## Consequences

**Benefits**:

- 10M nodes ≈ 480 MB data area (48 B/node), far below AoS's 100+ B/node + pointers;
- Geometry building and spatial index building can scan whole columns sequentially or in parallel;
- Safe deletion: old Handles never silently point to new objects;
- Rendering/validation/plugins read an immutable view (`DataSet`'s `&` or a snapshot), isolated from the write path.

**Costs**:

- Requires hand-written arena/handle logic and invariant tests (already covered by `neo-core`/`neo-store`);
- Appending reference ranges after edits grows the arena, requiring `compact()` (implemented in phase 2, see docs/03 §2.7);
- ID→handle needs an extra hash table (~20 B/object); can be replaced with "sorted array + binary search" during read-only phases;
- Cross-object invariants (e.g., relation member order, way node order) must be guaranteed by upper-layer commands; the storage layer does not validate them.

## Verification

1. `neo-store` unit tests: generation invalidation, slot reuse, contiguous Way reference ranges, memory estimation (48 B/node).
2. Benchmarks (docs/14): P50/P95 of build/query/modify on 1M objects with SoA; memory estimate vs process RSS.
3. Code review checkpoint: any PR that "adds `Vec<Box<T>>` back into the storage layer" must cite this ADR with a justification.
