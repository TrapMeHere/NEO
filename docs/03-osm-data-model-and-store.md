# 03 · OSM Data Model and Store

> Goal: make 10 million-scale OSM objects **editable, renderable, and uploadable** in single-machine memory, while ensuring that deletion/undo never causes silent data corruption where "an old handle points to a new object".
>
> This document maps one-to-one to `crates/neo-core`, `crates/neo-osm`, and `crates/neo-store`; interface names follow the code.

## 1 Goals and Scope

- Define NEO's **authoritative in-memory representation** (not a disk format, not a render format, not an API message format).
- Specify the per-object byte ledger, index structures, and edit hot-path semantics.
- Clarify what this document is **not** responsible for: projection/geometry ([04](./04-spatial-index-and-geometry.md)), render batches ([05](./05-renderer-wgpu.md)), Command/Undo ([06](./06-editing-core-and-undo.md)), persistence encoding ([10](./10-persistence-and-recovery.md)).

## 2 Design

### 2.1 Three Layers, Deliberately Separated

```text
neo-osm   : Node / Way / Relation / Tags / ChangeState   ← domain semantics (DTOs for IO/API/clipboard)
neo-store : NodeStore / WayStore / RelationStore / DataSet ← physical storage (SoA for editing/rendering)
neo-core  : ObjectId / Handle / Sym / Coord / Meta        ← primitives
```

- `neo-osm` DTOs are **not** a storage format: they allocate and own `Tags`, and appear only at boundaries.
- `neo-store`'s SoA is **not** a domain model: it does not guarantee element-level invariants (such as relation member order); the editing layer guarantees them.
- This is the data-side concrete form of ADR-0004's "three-model separation".

### 2.2 External IDs and Internal Handles

```rust
pub struct ObjectId(pub i64);          // negative = locally created (JOSM convention)
pub struct Handle<T> { index: u32, generation: u32 }  // 8 bytes, Copy
```

Rules:

1. **OSM IDs are never used as array indices** (they change after deletion/upload mapping, and negatives even more so).
2. A handle's `generation` is incremented when the slot is released; an old handle's `Handle::index()/generation()` is necessarily inconsistent with the slot → invalid.
3. A handle's `PhantomData<fn() -> T>` makes `Handle<T>` always `Send+Sync+Copy`, regardless of `T`.
4. `SlotAllocator` (`neo-core/src/handle.rs`) provides `alloc/free/is_valid/generation` and is shared by the three store types.

### 2.3 Coordinates: Fixed-Point `i32` (1e-7 Degrees)

```rust
pub struct Coord(pub i32);            // 1 unit = 1e-7 degrees = OSM API precision
pub struct LatLon { pub lat: Coord, pub lon: Coord }
```

| Option | Per coordinate | Coordinate memory for 10M nodes | Problem |
|---|---|---|---|
| `f64` | 8 B | 160 MB | Floating-point drift makes equality comparison unreliable; PBF→upload rounding is uncontrollable |
| `i64` nanodegree | 8 B | 160 MB | PBF-faithful but doubles memory |
| **`i32` 1e-7** | **4 B** | **80 MB** | Matches API precision; PBF 1e-9 values are rounded during import |

`i32` 1e-7 is chosen: longitude ±180° → ±1.8e9, which fits in `i32` (±2.147e9); latitude ±90° → ±9e8. PBF's `decimicro_lat/lon` are exactly 1e-7-degree integers, so **import needs zero conversion and zero floating point** (`osmpbf` provides them directly).

Side effects and countermeasures:

- 1e-9 precision in PBF is normalized on import by "rounding to 1e-7"; this matches the final precision of the OSM API.
- If byte-level PBF round-tripping is needed in the future, add an optional `i64` high-precision variant (the storage layer is already abstracted around `Coord`, so the replacement points are concentrated).
- The validator must reject out-of-range coordinates (`Coord::is_valid_lat/is_valid_lon`).

### 2.4 Metadata: 32-Byte `Copy`

```rust
#[repr(C)]
pub struct Meta {
    pub version: u32,
    pub timestamp: i64,     // Unix seconds
    pub changeset: i64,
    pub uid: u32,
    pub user: Sym,          // 4 B symbol
    pub visible: bool,
}
```

- Fully retain version/timestamp/changeset/uid/user/visible: upload diffs, conflict resolution, and history viewing all need them; they **must not be dropped for performance**.
- 10M objects × 32 B = 320 MB, the largest single item in the memory ledger; optional compression (varint arena for changeset, reverse lookup by uid for user) is evaluated in Phase 2.

### 2.5 Tags: Interned Symbols + Append-Only Arena

```rust
pub struct Sym(u32);                    // 4 bytes, index 0 = empty string
pub struct StringPool { ... }           // &str -> Sym, Sym -> &str
pub struct TagSetId(u32);               // 4 bytes
pub struct TagStore { pairs: Vec<(Sym, Sym)>, spans: Vec<u32> }  // range-based arena
```

- Tag text is stored only once; high-frequency keys/values such as `highway` and `building` bring the tag memory of 10M objects down from GB scale to hundreds of MB.
- The write path is **append-only**: editing an object's tags = appending a new TagSet + updating that object's `TagSetId`; the old range leaks temporarily and is reclaimed by `compact()`. The benefit is reference-count-free modification, no invalidation risk, and predictable writes.
- `Tags::content_hash()` provides an order-independent content hash (style/geometry cache key).

### 2.6 SoA Layout of the Three Element Types

```rust
pub struct NodeStore {
    ids:   Vec<ObjectId>,     //  8 B
    lat:   Vec<i32>,          //  4 B
    lon:   Vec<i32>,          //  4 B
    tags:  Vec<TagSetId>,     //  4 B
    meta:  Vec<Meta>,         // 32 B
    alloc: SlotAllocator,
}

pub struct WayStore {
    ids: Vec<ObjectId>, tags: Vec<TagSetId>, meta: Vec<Meta>,
    offsets: Vec<u32>, counts: Vec<u32>,          // reference range of each way
    node_refs: Vec<Handle<NodeSlot>>,             // contiguous reference pool
    alloc: SlotAllocator,
}

pub struct RelationStore {
    ids: Vec<ObjectId>, tags: Vec<TagSetId>, meta: Vec<Meta>,
    offsets: Vec<u32>, counts: Vec<u32>,
    members: Vec<StoredMember>,                   // role: Sym + target: AnyHandle
    alloc: SlotAllocator,
}
```

`StoredMember { role: Sym, target: AnyHandle }`, where `AnyHandle = (kind, index, generation)`; member **order is data** (route/multipolygon semantics), so append only, never sort.

### 2.7 Memory Ledger (Estimates, Excluding Indexes)

| Type | Per object | At 10M scale |
|---|---|---|
| Node | 8+4+4+4+32 = **48 B** | 480 MB |
| Way (5 nodes on average) | 8+4+32+4+4 + 5×8 = **92 B** | 920 MB if 10M ways |
| Relation (4 members on average) | 8+4+32+4+4 + 4×16 = **116 B** | — |
| TagSet (4 tags on average) | 4 B/object + 4×8 B tag pairs | + ≈ 360 MB |
| ID index (FxHashMap) | ≈ 20 B/object | ≈ 200 MB |
| Reverse references CSR | ≈ 4–12 B/object + references | ≈ 100–300 MB |
| String pool | Unique text | Usually < 20 MB |

> These are **estimates**; measurements are in the "memory measurements" table of [14](./14-performance-and-verification.md) (prototype reports `out/prototype-bench.md` and PowerShell's PeakWorkingSet).

### 2.8 `DataSet` and the Edit Hot Path

```rust
pub struct DataSet {
    pub strings: StringPool,
    pub tags: TagStore,
    pub nodes: NodeStore,
    pub ways: WayStore,
    pub relations: RelationStore,
    pub ids: IdIndex,
    pub changes: ChangeTrack,      // sparse: records only edited objects
    version: u64,                  // incremented on every commit; basis for render-cache invalidation
    next_local_id: i64,            // decrements for negatives
    dirty_nodes/dirty_ways: BitSet,
}
```

Full semantics of editing one node coordinate:

```text
set_node_position(handle, pos)
  ├─ validate latitude/longitude range (otherwise InvalidCoordinate)
  ├─ validate handle generation (otherwise StaleHandle)
  ├─ write lat/lon columns
  ├─ dirty_nodes.set(index)        ← incremental basis for rendering/picking/validation
  └─ version += 1                  ← SceneFrame/cache invalidation
```

**Not done here**: updating the spatial index, rebuilding geometry, marking related Ways. Those are done by `neo-editor` Commands ([06](./06-editing-core-and-undo.md)), because the effect of "moving a node" on Ways/Relations/indexes belongs to editing semantics, not storage semantics.

### 2.9 Index Layer

| Index | Structure | Purpose | Build cost |
|---|---|---|---|
| `IdIndex` | `FxHashMap<ObjectId, Handle>` × 3 | ID→handle, download/upload/jump | O(n) |
| `Referrers` | CSR (3 groups) | node→ways, node→relations, way→relations | O(n+e) |
| `SpatialIndex` | rstar R-tree (in `neo-geo`) | viewport culling, picking | O(n log n) |
| `dirty_*` | `BitSet` | basis for incremental rebuilds | O(1)/object |
| `ChangeTrack` | three sparse `FxHashMap`s | upload diff, dirty state | O(dirty objects) |

See ADR-0003 for the rationale and alternatives.

### 2.10 Views and Borrow Model

```rust
DataSet::node_view(handle) -> Option<NodeView<'_>>     // { info, tag_pairs }
DataSet::way_view(handle)  -> Option<WayView<'_>>      // { info, tag_pairs, node_refs }
DataSet::relation_view(handle) -> Option<RelationView<'_>>
```

- Views borrow only `&DataSet`; they do not copy data and do not hold references beyond the handle's lifetime.
- The write path is `&mut DataSet`, held by the single writer (the editing thread) (ADR-0006); rendering/background/plugins read an `Arc` snapshot or version-validated results ([07](./07-concurrency-and-tasks.md), [15](./15-plugin-system.md)).
- `neo-query` extracts "read-only queries" into the `MapQuery` trait; plugins/validators depend only on it, not on the store's write interface.

## 3 Key Interfaces

```rust
// neo-core
impl ObjectId { fn is_local(self) -> bool; fn raw(self) -> i64; }
impl<T> Handle<T> { fn index(self) -> u32; fn generation(self) -> u32; }
impl Coord { fn from_units(i32) -> Self; fn units(self) -> i32; fn from_nanodegrees(i64) -> Self; }
pub struct StringPool { fn intern(&mut self, &str) -> Sym; fn resolve(&self, Sym) -> &str; }

// neo-store
impl DataSet {
    fn new() -> Self;
    fn next_local_id(&mut self) -> ObjectId;
    fn intern(&mut self, &str) -> Sym;
    fn push_node(&mut self, Node) -> Handle<NodeSlot>;
    fn push_way(&mut self, Way, MissingPolicy) -> Result<Option<Handle<WaySlot>>>;
    fn push_relation(&mut self, Relation, MissingPolicy) -> Result<Option<Handle<RelationSlot>>>;
    fn set_node_position(&mut self, Handle<NodeSlot>, LatLon) -> Result<LatLon>;
    fn set_way_nodes(&mut self, Handle<WaySlot>, &[Handle<NodeSlot>]);
    fn node_view/way_view/relation_view(..) -> Option<..View<'_>>;
    fn take_dirty_ways/take_dirty_nodes(&mut self) -> Vec<Handle<..>>;
    fn build_referrers(&self) -> Referrers;
    fn stats(&self) -> StoreStats;   // Display: counts + memory
    fn mem_bytes(&self) -> ByteSize;
}
```

## 4 Data Flow

```text
PBF/XML  ──neo-io──►  DTO(Node/Way/Relation)  ──DataSet::push_*──►  SoA
                                                                     │
                             ┌───────────────────────────────────────┤
                             ▼                                      ▼
                   SpatialIndex::build                    Referrers::build
                   (viewport/picking)                     (parent refs/incremental invalidation)
                             │                                      │
                             ▼                                      ▼
                   SceneBuilder::build                    Command/Validator/Plugin
                   (SceneFrame)                           (read-only queries)
```

## 5 Tradeoffs and Alternatives

See the comparison table in ADR-0003 (`Vec<Box<Object>>`, slotmap, ECS, persistent data structures, SQLite as primary storage, embedded String).

Two additional decisions **not done this phase but with seams left open**:

1. **Snapshot COW**: `neo-query::MapSnapshot` is currently `Arc<DataSet>` (publish cost O(n)). Truly high-frequency publishing needs chunked COW (`ChunkedVec<T>`, cloning only modified 64K chunks). The interface is already designed around "immutable snapshots", so replacing the implementation does not affect upper layers.
2. **Arena compaction**: `TagStore` and the Way/Relation reference pools are append-only and will grow under long editing sessions. `compact()` needs to update references in all stores; it is planned for Phase 2 together with autosave.

## 6 Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Append-only arenas grow without bound (long editing) | Memory bloat | Monitor `stats()`; compact in Phase 2; autosave rewrites the snapshot |
| ID index at 20 B/object is ~200 MB at 10M | Memory pressure | In read-only phases switch to "sorted array + binary search"; or sharded hashing |
| Fixed-point 1e-7 coordinates are not equivalent to PBF 1e-9 | False positives in round-trip comparisons | Use **semantic equivalence** in golden tests (compare after normalizing to 1e-7), docs/18 |
| Handle generation overflow (u32) | Extreme reuse misjudged as valid | Panic/diagnostic log on overflow; practically unreachable (requires 4.2 billion reuses of the same slot) |
| Mixing views/snapshots leads to reading half-updated data | Render tearing/validation errors | Single writer + snapshot publication ([07](./07-concurrency-and-tasks.md)) |

## 7 Acceptance Criteria

1. `cargo test -p neo-core -p neo-osm -p neo-store` is all green, covering: generation invalidation, slot reuse, contiguous Way references, append-only Tags, memory estimates.
2. Measured memory and time for importing 10 million nodes + indexing are recorded in [14](./14-performance-and-verification.md); deviations > 30% must be explained.
3. There is no second mutable `DataSet` in `NeoApp` besides `data`; `grep -r "&mut DataSet"` only appears on editing/import paths.
4. All externally exposed handles go through generation validation (accessing with an old handle in tests must return `None`).

## 8 References

- OSM PBF format (node/way/relation, DenseNodes, string table): <https://wiki.openstreetmap.org/wiki/PBF_Format>
- OSM API 0.6 (1e-7 degree precision, changeset semantics): <https://wiki.openstreetmap.org/wiki/API_v0.6>
- rstar R-tree: <https://docs.rs/rstar/0.14.0/rstar/>
- SoA/AoS and caching: <https://en.wikipedia.org/wiki/AoS_and_SoA>
- Project code: `crates/neo-core/src/{handle,units,sym,meta}.rs`, `crates/neo-store/src/{node_store,way_store,relation_store,tag_store,indexes,dataset}.rs`
